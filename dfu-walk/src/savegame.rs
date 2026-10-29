//! Salvataggio e caricamento dello stato persistente:
//! - follower attivi (esterni animati + interni statici)
//! - NPC interni già reclutati (per non ri-spawnarli)
//! - NPC custom reclutati (follower + nomi, identificati per nome)
//! - posizione, yaw e quota del giocatore
//!
//! Formato: file di testo `savegame.txt` nella cartella di lavoro. Una riga per record.
//! Se il file manca si parte da zero.
//!
//! ## Note sul "salvataggio in interno"
//!
//! Se il giocatore è dentro un edificio quando avviene il salvataggio, la sua posizione
//! è in coordinate interne (Y ≈ 6000, XZ relativi all'edificio). Applicarla al load
//! nel mondo esterno manderebbe il player in cielo. Per questo:
//! - il flag `in_interior` viene salvato nel file;
//! - al load, se `in_interior` è vero, la posizione salvata viene ignorata e si usa
//!   lo spawn di default di `setup`;
//! - se la Y salvata è comunque fuori da un range plausibile (`|y| > 5000`), viene
//!   ignorata anche per file vecchi senza il flag;
//! - un sistema `fix_player_height_after_load` corregge la quota piedi dal terreno
//!   appena il primo tile è caricato, come rete di sicurezza.
use bevy::prelude::*;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

// Import necessari dai vari moduli del progetto
use crate::data::terrain_mesh::{terrain_height, CELL_SIZE};
use crate::interior::{CustomNpcCache, CustomNpcName, InteriorState};
use crate::mod_textures::ModTextures;
use crate::player::{Player, EYE_HEIGHT};
use crate::tiles::{Follower, FollowerWait, NpcSpriteSet};
use crate::wall_textures::WallTextures;
use crate::world::{map_pixel_of, tile_translation, GameData, HeightField, Origin};

pub const SAVEGAME_FILE: &str = "savegame.txt";
/// Soglia oltre la quale una coordinata Y è considerata "interna" / non plausibile
/// per il mondo esterno. `INTERIOR_Y` in `interior.rs` è 6000, quindi 5000 è un
/// margine di sicurezza.
const PLAUSIBLE_Y_LIMIT: f32 = 5000.0;
/// Frame di attesa prima di salvare, per permettere al `Follower` appena
/// spawnato di diventare visibile nella scena (Bevy applica i `Commands`
/// a fine frame: 2 frame sono sufficienti in ogni caso).
const SAVE_DELAY_FRAMES: u32 = 2;

// =============================================================================
// Tipi
// =============================================================================

#[derive(Clone, Debug)]
pub struct SavedFollowerExternal {
    pub face_record: usize,
    pub sprite_archive: u16,
    pub pos: [f32; 3],
    /// Il compagno sta aspettando fermo (comando "Aspettami qui").
    pub wait: bool,
}

#[derive(Clone, Debug)]
pub struct SavedFollowerInterior {
    pub flat_archive: u16,
    pub flat_record: u8,
    pub face_record: usize,
    pub size: [f32; 2],
    pub pos: [f32; 3],
    pub wait: bool,
}

/// Follower nato da un NPC custom (texture `npc_*.png` + nome nel `.txt`).
/// Si identifica per nome: la texture si ricava dalla `CustomNpcCache`.
#[derive(Clone, Debug)]
pub struct SavedFollowerCustom {
    pub name: String,
    pub size: [f32; 2],
    pub pos: [f32; 3],
    pub wait: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SaveGame {
    pub player_pos: Option<[f32; 3]>,
    pub player_yaw: Option<f32>,
    pub player_feet: Option<f32>,
    /// True se il salvataggio è avvenuto dentro un interno. In quel caso
    /// `player_pos` e `player_feet` sono in coordinate interne e non vanno
    /// applicati al load nel mondo esterno.
    pub in_interior: bool,
    pub interior_recruited: Vec<(u16, u8)>,
    pub followers_external: Vec<SavedFollowerExternal>,
    pub followers_interior: Vec<SavedFollowerInterior>,
    /// Nomi degli NPC custom gia' reclutati (non vanno ri-spawnati negli interni).
    pub custom_recruited: Vec<String>,
    pub followers_custom: Vec<SavedFollowerCustom>,
}

/// I nomi custom possono contenere spazi: nel file si codificano i soli
/// caratteri che spezzerebbero la riga o il parsing per spazi.
fn encode_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            '%' => out.push_str("%25"),
            ' ' => out.push_str("%20"),
            '\t' => out.push_str("%09"),
            '\n' => out.push_str("%0A"),
            '\r' => out.push_str("%0D"),
            c => out.push(c),
        }
    }
    out
}

fn decode_name(text: &str) -> String {
    text.replace("%20", " ")
        .replace("%09", "\t")
        .replace("%0A", "\n")
        .replace("%0D", "\r")
        .replace("%25", "%")
}

impl SaveGame {
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(_) => Self::default(),
        }
    }

    pub fn parse(text: &str) -> Self {
        let mut sg = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }
            match parts[0] {
                "player" => {
                    if parts.len() < 2 {
                        continue;
                    }
                    match parts[1] {
                        "pos" => {
                            if parts.len() >= 5 {
                                // Specificare esplicitamente il tipo f32 per evitare ambiguità
                                if let (Ok(x), Ok(y), Ok(z)) = (
                                    parts[2].parse::<f32>(),
                                                                parts[3].parse::<f32>(),
                                                                parts[4].parse::<f32>()
                                ) {
                                    // Rileva automaticamente coordinate interne
                                    if y.abs() > PLAUSIBLE_Y_LIMIT {
                                        sg.in_interior = true;
                                    }
                                    sg.player_pos = Some([x, y, z]);
                                }
                            }
                        }
                        "yaw" => {
                            if parts.len() >= 3 {
                                if let Ok(y) = parts[2].parse::<f32>() {
                                    sg.player_yaw = Some(y);
                                }
                            }
                        }
                        "feet" => {
                            if parts.len() >= 3 {
                                if let Ok(f) = parts[2].parse::<f32>() {
                                    if f.abs() > PLAUSIBLE_Y_LIMIT {
                                        sg.in_interior = true;
                                    }
                                    sg.player_feet = Some(f);
                                }
                            }
                        }
                        "in_interior" => {
                            if parts.len() >= 3 {
                                sg.in_interior = parts[2] == "1";
                            }
                        }
                        _ => {}
                    }
                }
                "interior_recruited" => {
                    if parts.len() >= 3 {
                        if let (Ok(a), Ok(r)) = (parts[1].parse::<u16>(), parts[2].parse::<u8>()) {
                            sg.interior_recruited.push((a, r));
                        }
                    }
                }
                "custom_recruited" => {
                    if parts.len() >= 2 {
                        sg.custom_recruited.push(decode_name(parts[1]));
                    }
                }
                "follower" => {
                    if parts.len() < 2 {
                        continue;
                    }
                    match parts[1] {
                        "external" => {
                            let mut face = None;
                            let mut archive = None;
                            let mut wait = false;
                            let mut pos = [0.0; 3];
                            // Parse key=value pairs
                            for p in &parts[2..] {
                                if *p == "wait=1" {
                                    wait = true;
                                } else if let Some(v) = p.strip_prefix("face=") {
                                    face = v.parse::<usize>().ok();
                                } else if let Some(v) = p.strip_prefix("sprite=") {
                                    archive = v.parse::<u16>().ok();
                                } else if let Some(rest) = p.strip_prefix("pos=") {
                                    let coords: Vec<&str> = rest.split(',').collect();
                                    if coords.len() == 3 {
                                        pos = [
                                            coords[0].parse().unwrap_or(0.0),
                                            coords[1].parse().unwrap_or(0.0),
                                            coords[2].parse().unwrap_or(0.0),
                                        ];
                                    }
                                }
                            }
                            if let (Some(f), Some(s)) = (face, archive) {
                                sg.followers_external.push(SavedFollowerExternal {
                                    face_record: f,
                                    sprite_archive: s,
                                    pos,
                                    wait,
                                });
                            }
                        }
                        "custom" => {
                            let mut name = None;
                            let mut wait = false;
                            let mut size = [1.0, 2.0];
                            let mut pos = [0.0; 3];
                            for p in &parts[2..] {
                                if *p == "wait=1" {
                                    wait = true;
                                } else if let Some(v) = p.strip_prefix("name=") {
                                    name = Some(decode_name(v));
                                } else if let Some(v) = p.strip_prefix("size=") {
                                    let dims: Vec<&str> = v.split(',').collect();
                                    if dims.len() == 2 {
                                        size = [
                                            dims[0].parse().unwrap_or(1.0),
                                            dims[1].parse().unwrap_or(2.0),
                                        ];
                                    }
                                } else if let Some(rest) = p.strip_prefix("pos=") {
                                    let coords: Vec<&str> = rest.split(',').collect();
                                    if coords.len() == 3 {
                                        pos = [
                                            coords[0].parse().unwrap_or(0.0),
                                            coords[1].parse().unwrap_or(0.0),
                                            coords[2].parse().unwrap_or(0.0),
                                        ];
                                    }
                                }
                            }
                            if let Some(name) = name.filter(|n| !n.is_empty()) {
                                sg.followers_custom.push(SavedFollowerCustom { name, size, pos, wait });
                            }
                        }
                        "interior" => {
                            let mut flat_a = None;
                            let mut wait = false;
                            let mut flat_r = None;
                            let mut face = None;
                            let mut size = [1.0, 2.0];
                            let mut pos = [0.0; 3];
                            let mut iter = parts[2..].iter();
                            while let Some(p) = iter.next() {
                                if *p == "wait=1" {
                                    wait = true;
                                } else if let Some(v) = p.strip_prefix("flat=") {
                                    flat_a = v.parse::<u16>().ok();
                                    if let Some(next) = iter.next() {
                                        flat_r = next.parse::<u8>().ok();
                                    }
                                } else if let Some(v) = p.strip_prefix("face=") {
                                    face = v.parse::<usize>().ok();
                                } else if let Some(v) = p.strip_prefix("size=") {
                                    let dims: Vec<&str> = v.split(',').collect();
                                    if dims.len() == 2 {
                                        size = [
                                            dims[0].parse().unwrap_or(1.0),
                                            dims[1].parse().unwrap_or(2.0),
                                        ];
                                    }
                                } else if let Some(rest) = p.strip_prefix("pos=") {
                                    let coords: Vec<&str> = rest.split(',').collect();
                                    if coords.len() == 3 {
                                        pos = [
                                            coords[0].parse().unwrap_or(0.0),
                                            coords[1].parse().unwrap_or(0.0),
                                            coords[2].parse().unwrap_or(0.0),
                                        ];
                                    }
                                }
                            }
                            if let (Some(a), Some(r), Some(f)) = (flat_a, flat_r, face) {
                                sg.followers_interior.push(SavedFollowerInterior {
                                    flat_archive: a,
                                    flat_record: r,
                                    face_record: f,
                                    size,
                                    pos,
                                    wait,
                                });
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        // Retrocompatibilità: file vecchi senza flag `in_interior`, ma con Y
        // palesemente interna → trattati come interni.
        if !sg.in_interior {
            let y_fuori_range = sg
            .player_pos
            .map_or(false, |p| p[1].abs() > PLAUSIBLE_Y_LIMIT)
            || sg
            .player_feet
            .map_or(false, |f| f.abs() > PLAUSIBLE_Y_LIMIT);
            if y_fuori_range {
                sg.in_interior = true;
            }
        }
        sg
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from("# savegame v1\n");
        if self.in_interior {
            out.push_str("player in_interior 1\n");
        } else {
            out.push_str("player in_interior 0\n");
        }
        if let Some(p) = self.player_pos {
            out.push_str(&format!("player pos {:.3} {:.3} {:.3}\n", p[0], p[1], p[2]));
        }
        if let Some(y) = self.player_yaw {
            out.push_str(&format!("player yaw {:.4}\n", y));
        }
        if let Some(f) = self.player_feet {
            out.push_str(&format!("player feet {:.3}\n", f));
        }
        let mut rec: Vec<(u16, u8)> = self.interior_recruited.clone();
        rec.sort();
        rec.dedup();
        for (a, r) in rec {
            out.push_str(&format!("interior_recruited {} {}\n", a, r));
        }
        for f in &self.followers_external {
            out.push_str(&format!(
                "follower external face={} sprite={} pos={:.3},{:.3},{:.3}{}\n",
                f.face_record, f.sprite_archive, f.pos[0], f.pos[1], f.pos[2],
                if f.wait { " wait=1" } else { "" }
            ));
        }
        for f in &self.followers_interior {
            out.push_str(&format!(
                "follower interior flat={} {} face={} size={:.3},{:.3} pos={:.3},{:.3},{:.3}{}\n",
                f.flat_archive,
                f.flat_record,
                f.face_record,
                f.size[0],
                f.size[1],
                f.pos[0],
                f.pos[1],
                f.pos[2],
                if f.wait { " wait=1" } else { "" }
            ));
        }
        let mut crec: Vec<&String> = self.custom_recruited.iter().collect();
        crec.sort();
        crec.dedup();
        for n in crec {
            out.push_str(&format!("custom_recruited {}\n", encode_name(n)));
        }
        for f in &self.followers_custom {
            out.push_str(&format!(
                "follower custom name={} size={:.3},{:.3} pos={:.3},{:.3},{:.3}{}\n",
                encode_name(&f.name),
                f.size[0],
                f.size[1],
                f.pos[0],
                f.pos[1],
                f.pos[2],
                if f.wait { " wait=1" } else { "" }
            ));
        }
        out
    }
}

// =============================================================================
// Stato runtime del salvataggio differito
// =============================================================================

#[derive(Resource, Default)]
pub struct SaveDelayed {
    frames_to_wait: u32,
}

// =============================================================================
// Sistemi
// =============================================================================

pub fn mark_dirty_on_recruit(
    events: EventReader<crate::dialog::RecruitFollowerEvent>,
    mut delayed: ResMut<SaveDelayed>,
) {
    if !events.is_empty() {
        delayed.frames_to_wait = SAVE_DELAY_FRAMES;
        info!("savegame: marked dirty on recruit");
    }
}

/// Marca "da salvare" quando si entra o si esce da un interno: così il flag
/// `in_interior` nel file riflette lo stato attuale e il player può ripartire
/// all'aperto con la sua posizione reale.
pub fn save_on_interior_change(
    interior: Res<InteriorState>,
    mut last: Local<Option<bool>>,
    mut delayed: ResMut<SaveDelayed>,
) {
    let current = interior.is_inside();
    match *last {
        Some(prev) if prev != current => {
            delayed.frames_to_wait = SAVE_DELAY_FRAMES;
        }
        _ => {}
    }
    *last = Some(current);
}

fn build_savegame(
    player: &Query<(&Player, &Transform), With<Player>>,
                  interior: &Res<InteriorState>,
                  followers: &Query<(&Follower, &Transform, Option<&CustomNpcName>, Option<&FollowerWait>), Without<Player>>,
) -> SaveGame {
    let inside = interior.is_inside();
    let mut sg = SaveGame {
        interior_recruited: interior.recruited.iter().copied().collect(),
        custom_recruited: interior.recruited_custom.iter().cloned().collect(),
        in_interior: inside,
        ..Default::default()
    };
    if let Ok((p, tf)) = player.get_single() {
        sg.player_pos = Some(tf.translation.to_array());
        sg.player_yaw = Some(p.yaw);
        sg.player_feet = Some(p.feet_y);
    }
    for (fol, tf, custom, wait) in followers.iter() {
        // Si salva solo l'attesa data all'esterno: quella dentro un edificio non e'
        // ripristinabile dopo un caricamento (l'interno viene ricostruito altrove).
        let wait = wait.map_or(false, |w| w.building.is_none());
        let pos = tf.translation.to_array();
        // NPC custom: flat 0/0 fittizi, si salvano per nome (e non come interni).
        if let Some(c) = custom {
            if let Some(info) = fol.interior_identity() {
                sg.followers_custom.push(SavedFollowerCustom {
                    name: c.name.clone(),
                    size: [info.size.x, info.size.y],
                    pos,
                    wait,
                });
            }
            continue;
        }
        if let Some(info) = fol.external_identity() {
            sg.followers_external.push(SavedFollowerExternal {
                face_record: info.face_record,
                sprite_archive: info.sprite_archive,
                pos,
                wait,
            });
        } else if let Some(info) = fol.interior_identity() {
            sg.followers_interior.push(SavedFollowerInterior {
                flat_archive: info.flat_archive,
                flat_record: info.flat_record,
                face_record: info.face_record,
                size: [info.size.x, info.size.y],
                pos,
                wait,
            });
        }
    }
    sg
}

fn write_savegame(sg: &SaveGame) {
    let text = sg.to_text();
    if let Err(e) = std::fs::write(SAVEGAME_FILE, text) {
        warn!("savegame: impossibile scrivere {}: {e}", SAVEGAME_FILE);
    } else {
        info!(
            "savegame: scritto {} ({} follower esterni, {} interni, {} NPC interni reclutati, in_interior={})",
              SAVEGAME_FILE,
              sg.followers_external.len(),
              sg.followers_interior.len(),
              sg.interior_recruited.len(),
              sg.in_interior
        );
    }
}

pub fn save_after_recruit(
    mut delayed: ResMut<SaveDelayed>,
    player: Query<(&Player, &Transform), With<Player>>,
                          interior: Res<InteriorState>,
                          followers: Query<(&Follower, &Transform, Option<&CustomNpcName>, Option<&FollowerWait>), Without<Player>>,
) {
    if delayed.frames_to_wait == 0 {
        return;
    }
    delayed.frames_to_wait -= 1;
    if delayed.frames_to_wait > 0 {
        return;
    }
    let sg = build_savegame(&player, &interior, &followers);
    write_savegame(&sg);
}

pub fn save_on_exit(
    mut exit: EventReader<AppExit>,
    player: Query<(&Player, &Transform), With<Player>>,
                    interior: Res<InteriorState>,
                    followers: Query<(&Follower, &Transform, Option<&CustomNpcName>, Option<&FollowerWait>), Without<Player>>,
) {
    let mut any = false;
    for _ in exit.read() {
        any = true;
    }
    if !any {
        return;
    }
    let sg = build_savegame(&player, &interior, &followers);
    write_savegame(&sg);
}

// =============================================================================
// Caricamento all'avvio
// =============================================================================

#[allow(clippy::too_many_arguments)]
pub fn load_on_startup(
    mut commands: Commands,
    mut done: Local<bool>,
    mut interior: ResMut<InteriorState>,
    mut player: Query<(&mut Player, &mut Transform), With<Player>>,
                       data: Res<GameData>,
                       mut mods: ResMut<ModTextures>,
                       mut images: ResMut<Assets<Image>>,
                       mut materials: ResMut<Assets<StandardMaterial>>,
                       mut meshes: ResMut<Assets<Mesh>>,
                       existing_followers: Query<(), With<Follower>>,
                       custom_npcs: Res<CustomNpcCache>,
) {
    if *done {
        return;
    }

    // Aspetta che ci sia almeno un tile caricato o che il player esista
    // per evitare crash su assets mancanti.
    if !existing_followers.is_empty() {
        info!("savegame: follower già presenti, salto load");
        *done = true;
        return;
    }

    let path = Path::new(SAVEGAME_FILE);
    if !path.exists() {
        info!("savegame: nessun file {}, parto da zero", SAVEGAME_FILE);
        *done = true;
        return;
    }

    let sg = SaveGame::load(path);
    info!(
        "savegame: caricato ({} ext, {} int, {} recruited, in_int={})",
          sg.followers_external.len(),
          sg.followers_interior.len(),
          sg.interior_recruited.len(),
          sg.in_interior
    );

    // 1. Ripristina set reclutati
    for (a, r) in &sg.interior_recruited {
        interior.mark_recruited(*a, *r);
    }
    for n in &sg.custom_recruited {
        interior.mark_recruited_custom(n);
    }

    // Calcola una posizione di fallback sicura per i follower
    let default_follower_pos = if let Ok((_, tf)) = player.get_single() {
        let fwd = tf.forward();
        let fxz = Vec2::new(fwd.x, fwd.z).normalize_or_zero();
        tf.translation + Vec3::new(fxz.x, 0.0, fxz.y) * 1.5
    } else {
        Vec3::ZERO
    };

    // 2. Posizione giocatore (solo se esterno e plausibile)
    if !sg.in_interior {
        if let Ok((mut p, mut tf)) = player.get_single_mut() {
            if let Some(pos) = sg.player_pos {
                if pos[1].abs() < PLAUSIBLE_Y_LIMIT {
                    tf.translation.x = pos[0];
                    tf.translation.z = pos[2];
                    // La Y verrà corretta da fix_player_height_after_load
                }
            }
            if let Some(f) = sg.player_feet {
                if f.abs() < PLAUSIBLE_Y_LIMIT {
                    p.feet_y = f;
                    p.on_ground = true;
                    p.vel_y = 0.0;
                    tf.translation.y = f + EYE_HEIGHT;
                }
            }
            if let Some(y) = sg.player_yaw {
                p.yaw = y;
            }
        }
    }

    // 3. Spawn follower esterni
    let quad = meshes.add(Rectangle::new(1.0, 1.0));
    let mut sprite_cache: HashMap<u16, Option<Arc<NpcSpriteSet>>> = HashMap::new();

    for f in &sg.followers_external {
        // Cerca nella cache o costruisce gli sprite
        let sprites = if let Some(cached) = sprite_cache.get(&f.sprite_archive) {
            cached.clone()
        } else {
            let s = crate::tiles::build_npc_sprites(
                f.sprite_archive,
                &data,
                &mut mods,
                &mut images,
                &mut materials,
            ).map(Arc::new);
            sprite_cache.insert(f.sprite_archive, s.clone());
            s
        };

        let Some(sprites) = sprites else {
            warn!("savegame: sprite arch {} mancante per ext follower", f.sprite_archive);
            continue;
        };

        // Usa gli accessor pubblici definiti in tiles.rs
        let size = sprites.walk_size(0);
        let mat0_opt = sprites.walk_material(0);

        let Some(mat0) = mat0_opt else {
            warn!("savegame: materiale walk[0] mancante per ext follower (arch {})", f.sprite_archive);
            continue;
        };

        let follower = Follower::from_saved_external(sprites.clone(), f.face_record);

        // Usa posizione salvata se valida, altrimenti dietro al player
        let pos = if f.pos[1].abs() < PLAUSIBLE_Y_LIMIT && !sg.in_interior {
            Vec3::from(f.pos)
        } else {
            default_follower_pos
        };

        let mut e = commands.spawn((
            Mesh3d(quad.clone()),
                        MeshMaterial3d(mat0),
                        Transform::from_translation(pos).with_scale(Vec3::new(size.x, size.y, 1.0)),
                        follower,
        ));
        if f.wait && !sg.in_interior {
            e.insert(FollowerWait::outside());
        }
        info!("savegame: spawned ext follower at {:?}", pos);
    }

    // 4. Spawn follower interni
    let mut wall_textures = WallTextures::default();
    for f in &sg.followers_interior {
        let Some((mat, _, _)) = wall_textures.flat_material(
            f.flat_archive,
            f.flat_record,
            &data,
            &mut mods,
            &mut images,
            &mut materials,
        ) else {
            warn!("savegame: flat {}:{} mancante per int follower", f.flat_archive, f.flat_record);
            continue;
        };

        let size = Vec2::new(f.size[0].max(0.1), f.size[1].max(0.1));
        let follower = Follower::from_static_mobile_full(
            mat.clone(),
                                                         size,
                                                         f.flat_archive,
                                                         f.flat_record,
                                                         f.face_record,
        );

        // I follower interni hanno posizioni interne (Y ≈ 6000):
        // al load esterno vanno spawnati vicino al player.
        let pos = if f.pos[1].abs() < PLAUSIBLE_Y_LIMIT && !sg.in_interior {
            Vec3::from(f.pos)
        } else {
            default_follower_pos
        };

        let mut e = commands.spawn((
            Mesh3d(quad.clone()),
                        MeshMaterial3d(mat),
                        Transform::from_translation(pos).with_scale(Vec3::new(size.x, size.y, 1.0)),
                        crate::interior::FlatBillboard,
                        follower,
        ));
        if f.wait && !sg.in_interior {
            e.insert(FollowerWait::outside());
        }
        info!("savegame: spawned int follower at {:?}", pos);
    }

    // 5. Spawn follower NPC custom (texture npc_*.png trovata dal nome nella cache)
    for f in &sg.followers_custom {
        let Some(data_npc) = custom_npcs.npcs.iter().find(|n| n.name == f.name) else {
            warn!("savegame: NPC custom '{}' non trovato nella cartella textures", f.name);
            continue;
        };
        let Some(image_handle) = mods.image_by_name(&data_npc.texture_stem, &mut images) else {
            warn!("savegame: texture '{}' mancante per NPC custom '{}'", data_npc.texture_stem, f.name);
            continue;
        };
        // Stesso materiale usato in interior.rs per gli NPC custom.
        let mat = materials.add(StandardMaterial {
            base_color_texture: Some(image_handle),
            alpha_mode: AlphaMode::Mask(0.5),
            cull_mode: None,
            double_sided: true,
            perceptual_roughness: 0.9,
            reflectance: 0.1,
            ..default()
        });
        let size = Vec2::new(f.size[0].max(0.1), f.size[1].max(0.1));
        // Stessi metadati fittizi (flat 0/0, face 0) di apply_recruit in dialog.rs.
        let follower = Follower::from_static_mobile_full(mat.clone(), size, 0, 0, 0);
        let pos = if f.pos[1].abs() < PLAUSIBLE_Y_LIMIT && !sg.in_interior {
            Vec3::from(f.pos)
        } else {
            default_follower_pos
        };
        let mut e = commands.spawn((
            Mesh3d(quad.clone()),
            MeshMaterial3d(mat),
            Transform::from_translation(pos).with_scale(Vec3::new(size.x, size.y, 1.0)),
            crate::interior::FlatBillboard,
            follower,
            CustomNpcName { name: f.name.clone() },
        ));
        if f.wait && !sg.in_interior {
            e.insert(FollowerWait::outside());
        }
        // Il nome torna "reclutato" anche se il file era vecchio e non lo elencava.
        interior.mark_recruited_custom(&f.name);
        info!("savegame: spawned custom follower '{}' at {:?}", f.name, pos);
    }

    *done = true;
}

/// Rete di sicurezza: appena il tile del player è caricato, se la Y è
/// chiaramente sbagliata (es. file vecchio, teleport, spawn anomalo),
/// la corregge dal terreno. Gira una sola volta.
pub fn fix_player_height_after_load(
    mut done: Local<bool>,
    field: Res<HeightField>,
    origin: Res<Origin>,
    mut player: Query<(&mut Player, &mut Transform), With<Player>>,
) {
    if *done {
        return;
    }
    let Ok((mut p, mut tf)) = player.get_single_mut() else {
        return;
    };
    let (mx, my) = map_pixel_of(&origin, tf.translation);
    if let Some(samples) = field.tiles.get(&(mx, my)) {
        let tile_org = tile_translation(&origin, mx, my);
        let gx = ((tf.translation.x - tile_org.x) / CELL_SIZE).clamp(0.0, 128.0);
        let gy = (-(tf.translation.z - tile_org.z) / CELL_SIZE).clamp(0.0, 128.0);
        let ground = terrain_height(samples, gx, gy);
        p.feet_y = ground;
        p.vel_y = 0.0;
        p.on_ground = true;
        tf.translation.y = ground + EYE_HEIGHT;
    }
    *done = true;
}

// =============================================================================
// Plugin
// =============================================================================

pub struct SaveGamePlugin;

impl Plugin for SaveGamePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SaveDelayed>()
        .add_systems(
            Update,
            (
                load_on_startup,
             fix_player_height_after_load,
             mark_dirty_on_recruit,
             save_on_interior_change,
             save_after_recruit,
             save_on_exit,
            ),
        );
    }
}

// =============================================================================
// Test
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_minimal() {
        let sg = SaveGame {
            player_pos: Some([10.5, 20.25, -30.75]),
            player_yaw: Some(1.2345),
            player_feet: Some(19.5),
            in_interior: false,
            interior_recruited: vec![(175, 0), (183, 2)],
            followers_external: vec![SavedFollowerExternal {
                face_record: 9,
                sprite_archive: 381,
                pos: [100.0, 50.0, -100.0],
                wait: false,
            }],
            followers_interior: vec![SavedFollowerInterior {
                flat_archive: 175,
                flat_record: 0,
                face_record: 336,
                size: [0.9, 1.8],
                pos: [120.0, 50.0, -80.0],
                wait: false,
            }],
            custom_recruited: vec!["Old Bartolo".to_string()],
            followers_custom: vec![SavedFollowerCustom {
                name: "Old Bartolo".to_string(),
                size: [1.1, 2.2],
                pos: [10.0, 20.0, 30.0],
                wait: true,
            }],
        };
        let text = sg.to_text();
        let back = SaveGame::parse(&text);
        assert_eq!(back.custom_recruited, vec!["Old Bartolo".to_string()]);
        assert_eq!(back.followers_custom.len(), 1);
        assert_eq!(back.followers_custom[0].name, "Old Bartolo");
        assert_eq!(back.followers_custom[0].size, [1.1, 2.2]);
        assert_eq!(back.followers_custom[0].pos, [10.0, 20.0, 30.0]);
        assert!(back.followers_custom[0].wait);
        assert!(!back.followers_interior[0].wait);
        assert!(!back.followers_external[0].wait);
        assert_eq!(back.player_pos, sg.player_pos);
        assert_eq!(back.player_yaw, sg.player_yaw);
        assert_eq!(back.player_feet, sg.player_feet);
        assert_eq!(back.in_interior, sg.in_interior);
        assert_eq!(back.interior_recruited.len(), 2);
        assert_eq!(back.followers_external.len(), 1);
        assert_eq!(back.followers_interior.len(), 1);
        assert_eq!(back.followers_interior[0].size, [0.9, 1.8]);
        assert_eq!(back.followers_interior[0].pos, [120.0, 50.0, -80.0]);
    }

    #[test]
    fn in_interior_round_trip() {
        let sg = SaveGame {
            in_interior: true,
            player_pos: Some([5.0, 6000.0, 10.0]),
            player_feet: Some(6000.0),
            ..Default::default()
        };
        let text = sg.to_text();
        assert!(text.contains("player in_interior 1"));
        let back = SaveGame::parse(&text);
        assert!(back.in_interior);
    }

    #[test]
    fn old_file_without_flag_but_internal_y_is_treated_as_interior() {
        // file vecchio: nessun flag, ma Y=6000
        let text = "player pos 5.0 6000.0 10.0\nplayer feet 6000.0\n";
        let sg = SaveGame::parse(text);
        assert!(sg.in_interior, "Y interna senza flag → trattato come in_interior");
    }

    #[test]
    fn old_file_with_plausible_y_is_not_in_interior() {
        let text = "player pos 5.0 200.0 10.0\nplayer feet 199.0\n";
        let sg = SaveGame::parse(text);
        assert!(!sg.in_interior);
    }

    #[test]
    fn empty_file_gives_default() {
        let sg = SaveGame::parse("");
        assert!(sg.player_pos.is_none());
        assert!(sg.followers_external.is_empty());
        assert!(sg.followers_interior.is_empty());
        assert!(!sg.in_interior);
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let text = "# commento\n\n \nplayer pos 1 2 3\n# altro commento\n";
        let sg = SaveGame::parse(text);
        assert_eq!(sg.player_pos, Some([1.0, 2.0, 3.0]));
    }

    #[test]
    fn missing_fields_are_tolerated() {
        let text = "follower external face=9 sprite=381\n";
        let sg = SaveGame::parse(text);
        assert_eq!(sg.followers_external.len(), 1);
        assert_eq!(sg.followers_external[0].pos, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn no_follower_without_required_fields() {
        let text = "follower external face=9 pos=1 2 3\n";
        let sg = SaveGame::parse(text);
        assert!(sg.followers_external.is_empty());
        let text = "follower interior flat=175 0 size=1 2 pos=1 2 3\n";
        let sg = SaveGame::parse(text);
        assert!(sg.followers_interior.is_empty());
    }
}
