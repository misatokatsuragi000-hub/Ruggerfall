//! Salvataggio e caricamento dello stato persistente.
//!
//! ## Due tipi di salvataggio
//!
//! - **Automatico** (`savegame.txt`): scritto dai trigger automatici
//!   (reclutamento, W/F, cambio mood, ingresso/uscita da interni, chiusura
//!   dell'app) e ad ogni click "Sovrascrivi" nella finestra F2.
//!   È quello che viene letto all'avvio.
//! - **Con nome** (`saves/<nome>.txt`): creato manualmente dalla finestra F2.
//!   Non viene mai letto all'avvio; si carica solo dalla finestra. Non
//!   interferisce con l'autosave.
//!
//! ## Finestra F2
//!
//! F2 apre una finestra egui centrata con titolo rosso "Salva/Carica". Esc o
//! F2 chiudono (Esc dentro il campo "Nome" lo deseleziona prima, come in
//! qualunque TextEdit egui).
//!
//! Mentre la finestra è aperta, `apply_save_menu_state` setta
//! `DialogState.open = true` **e** `DialogState.forced_by_menu = true`. Il flag
//! `forced_by_menu` impedisce a `dialog_menu` e `dialog_input_click` di
//! processare input tastiera (es. W/F follower, Digit1/Digit2, click sul
//! campo del dialogo) mentre si digita nel campo "Nome".
//!
//! ## Mood
//!
//! `Follower.mood` (0 = neutro, 1+ = frame idle statico dedicato) viene salvato
//! in ogni riga `follower ...` come `mood=N`, e ripristinato con
//! `Follower::with_mood(N)` al reload. File vecchi senza `mood=` → default 0.
//!
//! ## Idle per flat normali
//!
//! Anche i follower nati da un flat interno possono avere un set di idle, con
//! la convenzione `<archive>_<record>-0_idle_N.png` (es. `182_11-0.png` →
//! `182_11-0_idle_0.png`, `_idle_1.png`, ...). Stessa cosa per il mood:
//! `mood > 0` mostra staticamente `idle[mood]`. Al reload la scoperta viene
//! rifatta, così basta aggiungere i PNG nella cartella textures.
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, PrimaryWindow};
use bevy_egui::{egui, EguiContexts};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::data::terrain_mesh::{terrain_height, CELL_SIZE};
use crate::interior::{
    load_custom_idle_frames, load_custom_walk_frames, load_flat_idle_frames, CustomNpcCache,
    CustomNpcName, InteriorState,
};
use crate::mod_textures::ModTextures;
use crate::player::{Player, EYE_HEIGHT};
use crate::tiles::{Follower, FollowerMoodEvent, FollowerWait, NpcSpriteSet};
use crate::wall_textures::WallTextures;
use crate::world::{map_pixel_of, tile_translation, GameData, HeightField, Origin};

pub const SAVEGAME_FILE: &str = "savegame.txt";
pub const SAVES_DIR: &str = "saves";
const PLAUSIBLE_Y_LIMIT: f32 = 5000.0;
const SAVE_DELAY_FRAMES: u32 = 2;
const MAX_NAME_LEN: usize = 64;
/// Moltiplicatore dimensionale per gli idle (custom e flat). 1.0 = identico
/// all'idle base, 0.5 = metà. Allineato a `IDLE_SIZE_SCALE` in `dialog.rs`.
const IDLE_SIZE_SCALE: f32 = 0.5;
const IDLE_SIZE_SCALE_FLAT:   f32 = 1.0;
// =============================================================================
// Evento
// =============================================================================
#[derive(Event)]
pub struct FollowerCommandEvent;

// =============================================================================
// Stato della finestra Salva/Carica (F2)
// =============================================================================
#[derive(Resource, Default)]
pub struct SaveMenuState {
    pub open: bool,
    pub last_message: Option<String>,
    pub last_message_ok: bool,
    pub new_name: String,
    pub confirm_delete_path: Option<PathBuf>,
    pub list_cache: Option<Vec<SaveFileInfo>>,
}

#[derive(Clone, Debug)]
pub struct SaveFileInfo {
    pub name: String,
    pub path: PathBuf,
    pub modified_secs_ago: u64,
    pub size_bytes: u64,
}

impl SaveMenuState {
    fn set_ok(&mut self, msg: impl Into<String>) {
        self.last_message = Some(msg.into());
        self.last_message_ok = true;
    }
    fn set_err(&mut self, msg: impl Into<String>) {
        self.last_message = Some(msg.into());
        self.last_message_ok = false;
    }
    fn invalidate_list(&mut self) {
        self.list_cache = None;
    }
}

#[derive(Resource, Default)]
pub struct SaveLoadRequest {
    pub save_auto: bool,
    pub save_named: Option<PathBuf>,
    pub load_path: Option<PathBuf>,
    pub delete_path: Option<PathBuf>,
}

// =============================================================================
// Utility nomi/cartelle
// =============================================================================

fn saves_dir() -> PathBuf {
    PathBuf::from(SAVES_DIR)
}

pub fn sanitize_save_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(trimmed.len());
    for c in trimmed.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
            out.push(c);
        } else if c == ' ' || c == '_' || c == '\'' {
            out.push('_');
        }
        if out.len() >= MAX_NAME_LEN {
            break;
        }
    }
    if out.chars().all(|c| c == '.' || c == '_' || c == '-') {
        return None;
    }
    Some(out)
}

fn list_save_files() -> Vec<SaveFileInfo> {
    let dir = saves_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let now = std::time::SystemTime::now();
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("txt") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let (modified_secs_ago, size_bytes) = match std::fs::metadata(&path) {
            Ok(md) => {
                let secs = md
                .modified()
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
                (secs, md.len())
            }
            Err(_) => (0, 0),
        };
        out.push(SaveFileInfo {
            name: stem.to_string(),
                 path,
                 modified_secs_ago,
                 size_bytes,
        });
    }
    out.sort_by(|a, b| a.modified_secs_ago.cmp(&b.modified_secs_ago));
    out
}

fn humanize_ago(secs: u64) -> String {
    if secs < 60 {
        "adesso".to_string()
    } else if secs < 3600 {
        format!("{}m fa", secs / 60)
    } else if secs < 86400 {
        format!("{}h fa", secs / 3600)
    } else {
        format!("{}g fa", secs / 86400)
    }
}

fn humanize_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f32 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f32 / (1024.0 * 1024.0))
    }
}

// =============================================================================
// Tipi di salvataggio
// =============================================================================
#[derive(Clone, Debug, Default)]
pub struct SavedWait {
    pub outside: bool,
    pub building: Option<String>,
    pub local: [f32; 3],
}

impl SavedWait {
    pub fn from_component(wait: Option<&FollowerWait>) -> Self {
        match wait {
            None => SavedWait::default(),
            Some(w) if w.pending => SavedWait { outside: true, ..Default::default() },
            Some(w) => SavedWait {
                outside: w.building.is_none(),
                building: w.building.clone(),
                local: w.local.to_array(),
            },
        }
    }

    pub fn is_waiting(&self) -> bool {
        self.outside || self.building.is_some()
    }

    pub fn to_component(&self) -> Option<FollowerWait> {
        if self.outside {
            Some(FollowerWait::outside())
        } else if let Some(b) = &self.building {
            Some(FollowerWait {
                building: Some(b.clone()),
                 local: Vec3::from(self.local),
                 pending: false,
            })
        } else {
            None
        }
    }

    fn to_field(&self) -> Option<String> {
        if self.outside {
            Some("wait=outside".to_string())
        } else if let Some(b) = &self.building {
            Some(format!(
                "wait={};{:.3},{:.3},{:.3}",
                b, self.local[0], self.local[1], self.local[2]
            ))
        } else {
            None
        }
    }

    fn from_kv(k: &str, v: &str, out: &mut SavedWait) {
        if k != "wait" {
            return;
        }
        if v == "outside" {
            out.outside = true;
            out.building = None;
            return;
        }
        if let Some((building, coords)) = v.split_once(';') {
            let mut parts = coords.split(',');
            let x = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let y = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let z = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
            out.building = Some(building.to_string());
            out.local = [x, y, z];
            out.outside = false;
        }
    }
}

#[derive(Clone, Debug)]
pub struct SavedFollowerExternal {
    pub face_record: usize,
    pub sprite_archive: u16,
    pub pos: [f32; 3],
    pub wait: SavedWait,
    pub mood: u8,
}

#[derive(Clone, Debug)]
pub struct SavedFollowerInterior {
    pub flat_archive: u16,
    pub flat_record: u8,
    pub face_record: usize,
    pub size: [f32; 2],
    pub pos: [f32; 3],
    pub wait: SavedWait,
    pub mood: u8,
}

#[derive(Clone, Debug)]
pub struct SavedFollowerCustom {
    pub name: String,
    pub size: [f32; 2],
    pub pos: [f32; 3],
    pub wait: SavedWait,
    pub mood: u8,
}

#[derive(Clone, Debug, Default)]
pub struct SaveGame {
    pub player_pos: Option<[f32; 3]>,
    pub player_yaw: Option<f32>,
    pub player_feet: Option<f32>,
    pub in_interior: bool,
    pub interior_recruited: Vec<(u16, u8)>,
    pub followers_external: Vec<SavedFollowerExternal>,
    pub followers_interior: Vec<SavedFollowerInterior>,
    pub custom_recruited: Vec<String>,
    pub followers_custom: Vec<SavedFollowerCustom>,
}

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
                                if let (Ok(x), Ok(y), Ok(z)) = (
                                    parts[2].parse::<f32>(),
                                                                parts[3].parse::<f32>(),
                                                                parts[4].parse::<f32>(),
                                ) {
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
                            let mut pos = [0.0; 3];
                            let mut wait = SavedWait::default();
                            let mut mood = 0u8;
                            for p in &parts[2..] {
                                if let Some(v) = p.strip_prefix("face=") {
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
                                } else if let Some(v) = p.strip_prefix("mood=") {
                                    mood = v.parse().unwrap_or(0);
                                } else if let Some((k, v)) = p.split_once('=') {
                                    SavedWait::from_kv(k, v, &mut wait);
                                }
                            }
                            if let (Some(f), Some(s)) = (face, archive) {
                                sg.followers_external.push(SavedFollowerExternal {
                                    face_record: f, sprite_archive: s, pos, wait, mood,
                                });
                            }
                        }
                        "custom" => {
                            let mut name = None;
                            let mut size = [1.0, 2.0];
                            let mut pos = [0.0; 3];
                            let mut wait = SavedWait::default();
                            let mut mood = 0u8;
                            for p in &parts[2..] {
                                if let Some(v) = p.strip_prefix("name=") {
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
                                } else if let Some(v) = p.strip_prefix("mood=") {
                                    mood = v.parse().unwrap_or(0);
                                } else if let Some((k, v)) = p.split_once('=') {
                                    SavedWait::from_kv(k, v, &mut wait);
                                }
                            }
                            if let Some(name) = name.filter(|n| !n.is_empty()) {
                                sg.followers_custom.push(SavedFollowerCustom { name, size, pos, wait, mood });
                            }
                        }
                        "interior" => {
                            let mut flat_a = None;
                            let mut flat_r = None;
                            let mut face = None;
                            let mut size = [1.0, 2.0];
                            let mut pos = [0.0; 3];
                            let mut wait = SavedWait::default();
                            let mut mood = 0u8;
                            let mut iter = parts[2..].iter();
                            while let Some(p) = iter.next() {
                                if let Some(v) = p.strip_prefix("flat=") {
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
                                } else if let Some(v) = p.strip_prefix("mood=") {
                                    mood = v.parse().unwrap_or(0);
                                } else if let Some((k, v)) = p.split_once('=') {
                                    SavedWait::from_kv(k, v, &mut wait);
                                }
                            }
                            if let (Some(a), Some(r), Some(f)) = (flat_a, flat_r, face) {
                                sg.followers_interior.push(SavedFollowerInterior {
                                    flat_archive: a, flat_record: r, face_record: f,
                                    size, pos, wait, mood,
                                });
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
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
                "follower external face={} sprite={} pos={:.3},{:.3},{:.3} mood={}",
                f.face_record, f.sprite_archive, f.pos[0], f.pos[1], f.pos[2], f.mood
            ));
            if let Some(w) = f.wait.to_field() {
                out.push(' ');
                out.push_str(&w);
            }
            out.push('\n');
        }
        for f in &self.followers_interior {
            out.push_str(&format!(
                "follower interior flat={} {} face={} size={:.3},{:.3} pos={:.3},{:.3},{:.3} mood={}",
                f.flat_archive, f.flat_record, f.face_record,
                f.size[0], f.size[1], f.pos[0], f.pos[1], f.pos[2], f.mood
            ));
            if let Some(w) = f.wait.to_field() {
                out.push(' ');
                out.push_str(&w);
            }
            out.push('\n');
        }
        let mut crec: Vec<&String> = self.custom_recruited.iter().collect();
        crec.sort();
        crec.dedup();
        for n in crec {
            out.push_str(&format!("custom_recruited {}\n", encode_name(n)));
        }
        for f in &self.followers_custom {
            out.push_str(&format!(
                "follower custom name={} size={:.3},{:.3} pos={:.3},{:.3},{:.3} mood={}",
                encode_name(&f.name), f.size[0], f.size[1],
                                  f.pos[0], f.pos[1], f.pos[2], f.mood
            ));
            if let Some(w) = f.wait.to_field() {
                out.push(' ');
                out.push_str(&w);
            }
            out.push('\n');
        }
        out
    }
}

// =============================================================================
// Stato runtime
// =============================================================================
#[derive(Resource, Default)]
pub struct SaveDelayed {
    frames_to_wait: u32,
}

// =============================================================================
// Trigger dei salvataggi automatici
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

pub fn mark_dirty_on_mood(
    events: EventReader<FollowerMoodEvent>,
    mut delayed: ResMut<SaveDelayed>,
) {
    if !events.is_empty() {
        delayed.frames_to_wait = SAVE_DELAY_FRAMES;
        info!("savegame: marked dirty on mood");
    }
}

pub fn mark_dirty_on_follower_command(
    events: EventReader<FollowerCommandEvent>,
    mut delayed: ResMut<SaveDelayed>,
) {
    if !events.is_empty() {
        delayed.frames_to_wait = SAVE_DELAY_FRAMES;
        info!("savegame: marked dirty on follower command");
    }
}

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

// =============================================================================
// Salvataggio
// =============================================================================
#[allow(clippy::type_complexity)]
fn build_savegame(
    player: &Query<(&Player, &Transform), With<Player>>,
                  interior: &Res<InteriorState>,
                  followers: &Query<(
                      Entity,
                      &Follower,
                      &Transform,
                      Option<&FollowerWait>,
                      Option<&CustomNpcName>,
                  ), Without<Player>>,
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
    for (_e, fol, tf, wait, custom) in followers.iter() {
        let pos = tf.translation.to_array();
        let saved_wait = SavedWait::from_component(wait);
        let saved_mood = fol.mood();
        if let Some(c) = custom {
            if let Some(info) = fol.interior_identity() {
                sg.followers_custom.push(SavedFollowerCustom {
                    name: c.name.clone(),
                                         size: [info.size.x, info.size.y],
                                         pos,
                                         wait: saved_wait,
                                         mood: saved_mood,
                });
            }
            continue;
        }
        if let Some(info) = fol.external_identity() {
            sg.followers_external.push(SavedFollowerExternal {
                face_record: info.face_record,
                sprite_archive: info.sprite_archive,
                pos,
                wait: saved_wait,
                mood: saved_mood,
            });
        } else if let Some(info) = fol.interior_identity() {
            sg.followers_interior.push(SavedFollowerInterior {
                flat_archive: info.flat_archive,
                flat_record: info.flat_record,
                face_record: info.face_record,
                size: [info.size.x, info.size.y],
                pos,
                wait: saved_wait,
                mood: saved_mood,
            });
        }
    }
    sg
}

fn write_savegame_to(sg: &SaveGame, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, sg.to_text())
}

pub fn save_after_recruit(
    mut delayed: ResMut<SaveDelayed>,
    player: Query<(&Player, &Transform), With<Player>>,
                          interior: Res<InteriorState>,
                          followers: Query<(
                              Entity,
                              &Follower,
                              &Transform,
                              Option<&FollowerWait>,
                              Option<&CustomNpcName>,
                          ), Without<Player>>,
) {
    if delayed.frames_to_wait == 0 {
        return;
    }
    delayed.frames_to_wait -= 1;
    if delayed.frames_to_wait > 0 {
        return;
    }
    let sg = build_savegame(&player, &interior, &followers);
    if let Err(e) = write_savegame_to(&sg, Path::new(SAVEGAME_FILE)) {
        warn!("savegame: impossibile scrivere {}: {e}", SAVEGAME_FILE);
    } else {
        info!("savegame: autosave {} ({} follower)", SAVEGAME_FILE, sg.followers_custom.len());
    }
}

pub fn save_on_exit(
    mut exit: EventReader<AppExit>,
    player: Query<(&Player, &Transform), With<Player>>,
                    interior: Res<InteriorState>,
                    followers: Query<(
                        Entity,
                        &Follower,
                        &Transform,
                        Option<&FollowerWait>,
                        Option<&CustomNpcName>,
                    ), Without<Player>>,
) {
    let mut any = false;
    for _ in exit.read() {
        any = true;
    }
    if !any {
        return;
    }
    let sg = build_savegame(&player, &interior, &followers);
    let _ = write_savegame_to(&sg, Path::new(SAVEGAME_FILE));
}

// =============================================================================
// Caricamento (condiviso tra startup e pulsante Carica)
// =============================================================================

#[allow(clippy::too_many_arguments)]
fn load_savegame_now(
    commands: &mut Commands,
    interior: &mut InteriorState,
    player: &mut Query<(&mut Player, &mut Transform), With<Player>>,
                     data: &GameData,
                     mods: &mut ModTextures,
                     images: &mut Assets<Image>,
                     materials: &mut Assets<StandardMaterial>,
                     meshes: &mut Assets<Mesh>,
                     custom_npcs: &CustomNpcCache,
                     followers_to_despawn: &[Entity],
                     path: &Path,
) -> Result<SaveGame, String> {
    if !path.exists() {
        return Err(format!("{} non trovato", path.display()));
    }
    let sg = SaveGame::load(path);

    for e in followers_to_despawn {
        commands.entity(*e).despawn_recursive();
    }
    interior.recruited.clear();
    interior.recruited_custom.clear();

    info!(
        "savegame: caricato {} ({} ext, {} int, {} custom, {} recruited, in_int={})",
          path.display(),
          sg.followers_external.len(),
          sg.followers_interior.len(),
          sg.followers_custom.len(),
          sg.interior_recruited.len(),
          sg.in_interior
    );

    for (a, r) in &sg.interior_recruited {
        interior.mark_recruited(*a, *r);
    }
    for n in &sg.custom_recruited {
        interior.mark_recruited_custom(n);
    }

    let default_follower_pos = if let Ok((_, tf)) = player.get_single() {
        let fwd = tf.forward();
        let fxz = Vec2::new(fwd.x, fwd.z).normalize_or_zero();
        tf.translation + Vec3::new(fxz.x, 0.0, fxz.y) * 1.5
    } else {
        Vec3::ZERO
    };

    if !sg.in_interior {
        if let Ok((mut p, mut tf)) = player.get_single_mut() {
            if let Some(pos) = sg.player_pos {
                if pos[1].abs() < PLAUSIBLE_Y_LIMIT {
                    tf.translation.x = pos[0];
                    tf.translation.z = pos[2];
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

    let quad = meshes.add(Rectangle::new(1.0, 1.0));
    let mut sprite_cache: HashMap<u16, Option<Arc<NpcSpriteSet>>> = HashMap::new();
    for f in &sg.followers_external {
        let sprites = if let Some(cached) = sprite_cache.get(&f.sprite_archive) {
            cached.clone()
        } else {
            let s = crate::tiles::build_npc_sprites(
                f.sprite_archive, data, mods, images, materials,
            )
            .map(Arc::new);
            sprite_cache.insert(f.sprite_archive, s.clone());
            s
        };
        let Some(sprites) = sprites else { continue };
        let size = sprites.walk_size(0);
        let Some(mat0) = sprites.walk_material(0) else { continue };
        let follower = Follower::from_saved_external(sprites.clone(), f.face_record)
        .with_mood(f.mood);
        let pos = if f.wait.building.is_some() {
            default_follower_pos
        } else if f.pos[1].abs() < PLAUSIBLE_Y_LIMIT && !sg.in_interior {
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
        if let Some(w) = f.wait.to_component() {
            e.insert(w);
        }
    }

    // Follower interni (flat). Idle multiframe se `<archive>_<record>-0_idle_N.png`
    // esistono nella cartella textures, altrimenti restano statici.
    let mut wall_textures = WallTextures::default();
    for f in &sg.followers_interior {
        let Some((mat, _, _)) = wall_textures.flat_material(
            f.flat_archive, f.flat_record, data, mods, images, materials,
        ) else { continue };
        let size = Vec2::new(f.size[0].max(0.1), f.size[1].max(0.1));
        let mut follower = Follower::from_static_mobile_full(
            mat.clone(), size, f.flat_archive, f.flat_record, f.face_record,
        );

        // Idle per flat normali: stessa convenzione dei custom.
        {
            let base_size_px = mods
            .size_by_name(&format!("{:03}_{}-0", f.flat_archive, f.flat_record))
            .or_else(|| {
                mods.size_by_name(&format!("{}_{}-0", f.flat_archive, f.flat_record))
            })
            .map(|(w, h)| Vec2::new(w as f32, h as f32))
            .unwrap_or(size);
            if let Some((idle_frames, idle_sizes)) = load_flat_idle_frames(
                f.flat_archive,
                f.flat_record,
                0,
                base_size_px,
                size * IDLE_SIZE_SCALE_FLAT,
                mods,
                images,
                materials,
            ) {
                info!(
                    "savegame: flat {}:{} con {} frame idle",
                    f.flat_archive, f.flat_record, idle_frames.len(),
                );
                follower = follower.with_idle(idle_frames, idle_sizes, 1.0);
            }
        }

        follower = follower.with_mood(f.mood);

        let pos = if f.wait.building.is_some() {
            default_follower_pos
        } else if f.pos[1].abs() < PLAUSIBLE_Y_LIMIT && !sg.in_interior {
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
        if let Some(w) = f.wait.to_component() {
            e.insert(w);
        }
    }

    // Follower custom.
    for f in &sg.followers_custom {
        let Some(data_npc) = custom_npcs.npcs.iter().find(|n| n.name == f.name) else {
            warn!("savegame: NPC custom '{}' non trovato", f.name);
            continue;
        };
        let Some(image_handle) = mods.image_by_name(&data_npc.texture_stem, images) else {
            continue;
        };
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
        let mut follower = Follower::from_static_mobile_full(mat.clone(), size, 0, 0, 0);

        let base_size_px = mods
        .image_by_name(&data_npc.texture_stem, images)
        .and_then(|h| {
            mods.image_size(&h)
            .or_else(|| images.get(&h).map(|i| i.size_f32()))
        })
        .unwrap_or(size);

        if let Some((frames, sizes)) = load_custom_walk_frames(
            data_npc, base_size_px, size, mods, images, materials,
        ) {
            follower = follower.with_walk(frames, sizes, data_npc.walk_fps);
        }
        if let Some((idle_frames, idle_sizes)) = load_custom_idle_frames(
            data_npc, base_size_px, size * IDLE_SIZE_SCALE, mods, images, materials,
        ) {
            follower = follower.with_idle(idle_frames, idle_sizes, data_npc.idle_fps);
        }
        follower = follower.with_mood(f.mood);

        let pos = if f.wait.building.is_some() {
            default_follower_pos
        } else if f.pos[1].abs() < PLAUSIBLE_Y_LIMIT && !sg.in_interior {
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
        if let Some(w) = f.wait.to_component() {
            e.insert(w);
        }
        interior.mark_recruited_custom(&f.name);
    }

    Ok(sg)
}

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
                       custom_npcs: Res<CustomNpcCache>,
) {
    if *done {
        return;
    }
    *done = true;
    if !Path::new(SAVEGAME_FILE).exists() {
        info!("savegame: nessun file {}, parto da zero", SAVEGAME_FILE);
        return;
    }
    let _ = load_savegame_now(
        &mut commands,
        &mut interior,
        &mut player,
        &data,
        &mut mods,
        &mut images,
        &mut materials,
        &mut meshes,
        &custom_npcs,
        &[],
        Path::new(SAVEGAME_FILE),
    );
}

pub fn fix_player_height_after_load(
    mut done: Local<bool>,
    field: Res<HeightField>,
    origin: Res<Origin>,
    mut player: Query<(&mut Player, &mut Transform), With<Player>>,
) {
    if *done {
        return;
    }
    let Ok((mut p, mut tf)) = player.get_single_mut() else { return };
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
// Finestra Salva/Carica (F2)
// =============================================================================

fn detect_f2_save_menu(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    dialog: Res<crate::dialog::DialogState>,
    mut menu: ResMut<SaveMenuState>,
) {
    let f2 = keys.just_pressed(KeyCode::F2);
    let esc = keys.just_pressed(KeyCode::Escape);

    if menu.open {
        // Menu aperto: F2 ed Esc chiudono. Esc è utile quando il campo "Nome"
        // non ha il focus, altrimenti egui lo consuma da solo (per
        // deselezionare il TextEdit).
        if f2 || esc {
            menu.open = false;
            menu.confirm_delete_path = None;
            keys.clear_just_pressed(KeyCode::F2);
            keys.clear_just_pressed(KeyCode::Escape);
        }
    } else if f2 && !dialog.open {
        menu.open = true;
        menu.last_message = None;
        menu.confirm_delete_path = None;
        menu.invalidate_list();
        keys.clear_just_pressed(KeyCode::F2);
    }
}

fn suppress_save_menu_keys(mut keys: ResMut<ButtonInput<KeyCode>>) {
    // F2 e Esc sono gestiti da `detect_f2_save_menu` (che gira prima e li
    // consuma): qui restano solo i tasti che dialog/player potrebbero
    // intercettare e che vogliamo bloccare mentre la finestra è aperta.
    for key in [
        KeyCode::KeyE,
        KeyCode::Digit1,
        KeyCode::Numpad1,
        KeyCode::Digit2,
        KeyCode::Numpad2,
        KeyCode::KeyA,
        KeyCode::KeyR,
    ] {
        keys.clear_just_pressed(key);
    }
}

fn apply_save_menu_state(
    menu: Res<SaveMenuState>,
    mut dialog: ResMut<crate::dialog::DialogState>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut last_open: Local<Option<bool>>,
) {
    let prev = *last_open;
    *last_open = Some(menu.open);
    let Ok(mut window) = windows.get_single_mut() else { return };
    if menu.open {
        dialog.open = true;
        dialog.forced_by_menu = true;
        window.cursor_options.grab_mode = CursorGrabMode::None;
        window.cursor_options.visible = true;
    } else if prev == Some(true) {
        dialog.open = false;
        dialog.forced_by_menu = false;
        window.cursor_options.grab_mode = CursorGrabMode::Locked;
        window.cursor_options.visible = false;
    }
}

fn save_menu_ui(
    mut contexts: EguiContexts,
    mut menu: ResMut<SaveMenuState>,
    mut request: ResMut<SaveLoadRequest>,
    interior: Res<InteriorState>,
) {
    if !menu.open {
        return;
    }
    if menu.list_cache.is_none() {
        menu.list_cache = Some(list_save_files());
    }
    let inside = interior.is_inside();
    let ctx = contexts.ctx_mut();

    let gold = egui::Color32::from_rgb(255, 205, 80);
    let amber = egui::Color32::from_rgb(255, 160, 60);
    let leaf = egui::Color32::from_rgb(90, 220, 120);
    let sky = egui::Color32::from_rgb(110, 190, 255);
    let blood = egui::Color32::from_rgb(240, 100, 80);
    let muted = egui::Color32::from_rgb(170, 170, 170);
    let faint = egui::Color32::from_rgb(90, 90, 90);
    let panel_bg = egui::Color32::from_rgba_unmultiplied(28, 22, 16, 245);
    let red_title = egui::Color32::from_rgb(230, 55, 55);

    let mut clicked_save_auto = false;
    let mut clicked_save_named = false;
    let mut clicked_load: Option<PathBuf> = None;
    let mut clicked_overwrite: Option<PathBuf> = None;
    let mut clicked_delete: Option<PathBuf> = None;
    let mut clicked_cancel_delete = false;

    let window = egui::Window::new(
        egui::RichText::new("Salva/Carica")
        .color(red_title)
        .strong()
        .size(18.0),
    )
    .resizable(false)
    .collapsible(false)
    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
    .default_width(440.0)
    .frame(
        egui::Frame::window(&ctx.style())
        .fill(panel_bg)
        .stroke(egui::Stroke::new(1.5, red_title))
        .inner_margin(egui::Margin::same(14))
        .corner_radius(egui::CornerRadius::same(8)),
    );

    window.show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("▸ AUTOMATICO").color(leaf).strong().size(14.0));
            ui.label(egui::RichText::new(format!("({})", SAVEGAME_FILE)).color(muted).size(11.0));
        });
        ui.label(
            egui::RichText::new("Viene sovrascritto ad ogni salvataggio e caricato all'avvio.")
            .color(faint)
            .size(11.0),
        );
        ui.add_space(2.0);
        if ui
            .add(
                egui::Button::new(egui::RichText::new("💾  Sovrascrivi").color(egui::Color32::WHITE).strong())
                .fill(egui::Color32::from_rgb(50, 130, 70))
                .stroke(egui::Stroke::new(1.0, leaf)),
            )
            .clicked()
            {
                clicked_save_auto = true;
            }

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);

            ui.label(egui::RichText::new("▸ SALVATAGGI CON NOME").color(sky).strong().size(14.0));
            ui.label(
                egui::RichText::new(format!("Cartella: {}/", SAVES_DIR)).color(faint).size(11.0),
            );
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Nome:").color(muted).size(13.0));
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut menu.new_name)
                    .hint_text("es. prima_della_battaglia")
                    .desired_width(220.0),
                );
                let can_save = !menu.new_name.trim().is_empty();
                let btn = ui.add_enabled(
                    can_save,
                    egui::Button::new(egui::RichText::new("💾  Salva").color(egui::Color32::WHITE).strong())
                    .fill(egui::Color32::from_rgb(40, 100, 160))
                    .stroke(egui::Stroke::new(1.0, sky)),
                );
                if btn.clicked() || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && can_save) {
                    clicked_save_named = true;
                }
            });

            ui.add_space(8.0);

            let list = menu.list_cache.clone().unwrap_or_default();
            if list.is_empty() {
                ui.label(
                    egui::RichText::new("Nessun salvataggio nominato.")
                    .color(faint)
                    .italics()
                    .size(12.0),
                );
            } else {
                egui::ScrollArea::vertical()
                .max_height(220.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for info in &list {
                        let bg = if menu.confirm_delete_path.as_deref() == Some(info.path.as_path()) {
                            egui::Color32::from_rgba_unmultiplied(80, 20, 20, 220)
                        } else {
                            egui::Color32::from_rgba_unmultiplied(45, 38, 28, 220)
                        };
                        egui::Frame::group(ui.style())
                        .fill(bg)
                        .stroke(egui::Stroke::new(1.0, amber))
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .corner_radius(egui::CornerRadius::same(6))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&info.name).color(gold).strong().size(13.0));
                                ui.label(
                                    egui::RichText::new(format!("· {} · {}", humanize_ago(info.modified_secs_ago), humanize_size(info.size_bytes)))
                                    .color(muted)
                                    .size(11.0),
                                );
                            });
                            ui.add_space(2.0);
                            ui.horizontal(|ui| {
                                let load_btn = ui.add_enabled(
                                    !inside,
                                    egui::Button::new(egui::RichText::new("📂  Carica").color(egui::Color32::WHITE).size(12.0))
                                    .fill(egui::Color32::from_rgb(40, 100, 160))
                                    .stroke(egui::Stroke::new(1.0, sky)),
                                );
                                if load_btn.clicked() {
                                    clicked_load = Some(info.path.clone());
                                }
                                if inside {
                                    ui.label(
                                        egui::RichText::new("(esci dall'edificio)")
                                        .color(amber)
                                        .size(11.0),
                                    );
                                }
                                let overwrite_btn = ui.add(
                                    egui::Button::new(egui::RichText::new("💾  Sovrascrivi").color(egui::Color32::WHITE).size(12.0))
                                    .fill(egui::Color32::from_rgb(50, 130, 70))
                                    .stroke(egui::Stroke::new(1.0, leaf)),
                                );
                                if overwrite_btn.clicked() {
                                    clicked_overwrite = Some(info.path.clone());
                                }
                                let del_btn = ui.add(
                                    egui::Button::new(egui::RichText::new("🗑  Elimina").color(egui::Color32::WHITE).size(12.0))
                                    .fill(egui::Color32::from_rgb(140, 45, 40))
                                    .stroke(egui::Stroke::new(1.0, blood)),
                                );
                                if del_btn.clicked() {
                                    clicked_delete = Some(info.path.clone());
                                }
                            });
                            if menu.confirm_delete_path.as_deref() == Some(info.path.as_path()) {
                                ui.add_space(4.0);
                                ui.colored_label(blood, "Eliminare definitivamente?");
                                ui.horizontal(|ui| {
                                    if ui.button("Sì, elimina").clicked() {
                                        clicked_delete = Some(info.path.clone());
                                    }
                                    if ui.button("Annulla").clicked() {
                                        clicked_cancel_delete = true;
                                    }
                                });
                            }
                        });
                        ui.add_space(4.0);
                    }
                });
            }

            ui.add_space(8.0);
            if let Some(msg) = &menu.last_message {
                let color = if menu.last_message_ok { leaf } else { blood };
                ui.separator();
                ui.colored_label(color, egui::RichText::new(msg).strong().size(12.5));
            }

            ui.add_space(6.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("F2 = chiudi  ·  Esc = chiudi")
                    .color(faint)
                    .size(11.0),
                );
            });
    });

    if clicked_cancel_delete {
        menu.confirm_delete_path = None;
    }
    if clicked_save_auto {
        request.save_auto = true;
        menu.confirm_delete_path = None;
    }
    if clicked_save_named {
        match sanitize_save_name(&menu.new_name) {
            Some(name) => {
                let path = saves_dir().join(format!("{name}.txt"));
                request.save_named = Some(path);
                menu.new_name.clear();
                menu.confirm_delete_path = None;
            }
            None => {
                menu.set_err("Nome non valido: usa lettere, numeri, '_' o '-'");
                menu.confirm_delete_path = None;
            }
        }
    }
    if let Some(path) = clicked_load {
        request.load_path = Some(path);
        menu.confirm_delete_path = None;
    }
    if let Some(path) = clicked_overwrite {
        request.save_named = Some(path);
        menu.confirm_delete_path = None;
    }
    if let Some(path) = clicked_delete {
        if menu.confirm_delete_path.as_deref() == Some(path.as_path()) {
            request.delete_path = Some(path);
            menu.confirm_delete_path = None;
        } else {
            menu.confirm_delete_path = Some(path);
            menu.last_message = None;
        }
    }
}

// =============================================================================
// Applica le richieste
// =============================================================================

fn apply_save_request(
    mut request: ResMut<SaveLoadRequest>,
    mut menu: ResMut<SaveMenuState>,
    player: Query<(&Player, &Transform), With<Player>>,
                      interior: Res<InteriorState>,
                      followers: Query<(
                          Entity,
                          &Follower,
                          &Transform,
                          Option<&FollowerWait>,
                          Option<&CustomNpcName>,
                      ), Without<Player>>,
) {
    let save_auto = std::mem::replace(&mut request.save_auto, false);
    let save_named = request.save_named.take();
    if !save_auto && save_named.is_none() {
        return;
    }
    let sg = build_savegame(&player, &interior, &followers);
    let n = sg.followers_external.len() + sg.followers_interior.len() + sg.followers_custom.len();

    if save_auto {
        match write_savegame_to(&sg, Path::new(SAVEGAME_FILE)) {
            Ok(()) => {
                menu.set_ok(format!("Autosave scritto ({} follower)", n));
                info!("savegame: manual autosave, {} follower", n);
            }
            Err(e) => {
                menu.set_err(format!("Errore autosave: {e}"));
                warn!("savegame: autosave failed: {e}");
            }
        }
    }
    if let Some(path) = save_named {
        match write_savegame_to(&sg, &path) {
            Ok(()) => {
                let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
                menu.set_ok(format!("Salvato '{}' ({} follower)", name, n));
                info!("savegame: manual save '{}', {} follower", path.display(), n);
                menu.invalidate_list();
            }
            Err(e) => {
                menu.set_err(format!("Errore salvataggio: {e}"));
                warn!("savegame: manual save failed: {e}");
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_load_request(
    mut commands: Commands,
    mut request: ResMut<SaveLoadRequest>,
    mut menu: ResMut<SaveMenuState>,
    mut interior: ResMut<InteriorState>,
    mut player: Query<(&mut Player, &mut Transform), With<Player>>,
                      data: Res<GameData>,
                      mut mods: ResMut<ModTextures>,
                      mut images: ResMut<Assets<Image>>,
                      mut materials: ResMut<Assets<StandardMaterial>>,
                      mut meshes: ResMut<Assets<Mesh>>,
                      custom_npcs: Res<CustomNpcCache>,
                      existing_followers: Query<Entity, With<Follower>>,
) {
    let Some(path) = request.load_path.take() else { return };
    if interior.is_inside() {
        menu.set_err("Esci dall'edificio prima di caricare");
        return;
    }
    let followers_vec: Vec<Entity> = existing_followers.iter().collect();
    match load_savegame_now(
        &mut commands,
        &mut interior,
        &mut player,
        &data,
        &mut mods,
        &mut images,
        &mut materials,
        &mut meshes,
        &custom_npcs,
        &followers_vec,
        &path,
    ) {
        Ok(sg) => {
            let n = sg.followers_external.len() + sg.followers_interior.len() + sg.followers_custom.len();
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
            menu.set_ok(format!("Caricato '{}' ({} follower)", name, n));
            info!("savegame: manual load '{}', {} follower", path.display(), n);
        }
        Err(e) => {
            menu.set_err(format!("Errore caricamento: {e}"));
            warn!("savegame: manual load failed: {e}");
        }
    }
}

fn apply_delete_request(mut request: ResMut<SaveLoadRequest>, mut menu: ResMut<SaveMenuState>) {
    let Some(path) = request.delete_path.take() else { return };
    match std::fs::remove_file(&path) {
        Ok(()) => {
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
            menu.set_ok(format!("Eliminato '{}'", name));
            info!("savegame: deleted '{}'", path.display());
            menu.invalidate_list();
        }
        Err(e) => {
            menu.set_err(format!("Errore eliminazione: {e}"));
            warn!("savegame: delete failed: {e}");
        }
    }
}

// =============================================================================
// Plugin
// =============================================================================
pub struct SaveGamePlugin;

impl Plugin for SaveGamePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SaveDelayed>()
        .init_resource::<SaveMenuState>()
        .init_resource::<SaveLoadRequest>()
        .add_event::<FollowerCommandEvent>()
        .add_systems(
            PreUpdate,
            (
                detect_f2_save_menu.before(crate::dialog::update_dialog_followers),
             suppress_save_menu_keys
             .run_if(|m: Res<SaveMenuState>| m.open)
             .after(detect_f2_save_menu)
             .before(crate::dialog::update_dialog_followers),
            ),
        )
        .add_systems(
            Update,
            (
                load_on_startup,
             fix_player_height_after_load,
             mark_dirty_on_recruit,
             mark_dirty_on_mood,
             mark_dirty_on_follower_command,
             save_on_interior_change,
             save_after_recruit,
             save_on_exit,
             apply_save_menu_state
             .after(crate::player::grab_cursor)
             .before(crate::player::player_look),
             save_menu_ui,
             apply_save_request,
             apply_load_request,
             apply_delete_request,
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
    fn mood_round_trip() {
        let sg = SaveGame {
            followers_custom: vec![SavedFollowerCustom {
                name: "Ellie".into(), size: [1.1, 2.2],
                pos: [10.0, 20.0, 30.0], wait: SavedWait::default(), mood: 2,
            }],
            ..Default::default()
        };
        let back = SaveGame::parse(&sg.to_text());
        assert_eq!(back.followers_custom[0].mood, 2);
    }

    #[test]
    fn sanitize_name_accepts_common_inputs() {
        assert_eq!(sanitize_save_name("prima battaglia").as_deref(), Some("prima_battaglia"));
        assert_eq!(sanitize_save_name("  hello  ").as_deref(), Some("hello"));
        assert_eq!(sanitize_save_name("a-b_c.d").as_deref(), Some("a-b_c.d"));
    }

    #[test]
    fn sanitize_name_rejects_empty_and_filler() {
        assert!(sanitize_save_name("").is_none());
        assert!(sanitize_save_name("   ").is_none());
        assert!(sanitize_save_name("___").is_none());
        assert!(sanitize_save_name("...").is_none());
        assert!(sanitize_save_name("!!!").is_none());
    }

    #[test]
    fn sanitize_name_is_bounded() {
        let long = "a".repeat(200);
        let out = sanitize_save_name(&long).unwrap();
        assert!(out.len() <= MAX_NAME_LEN);
    }

    #[test]
    fn humanize_ago_formats() {
        assert_eq!(humanize_ago(0), "adesso");
        assert_eq!(humanize_ago(30), "adesso");
        assert_eq!(humanize_ago(90), "1m fa");
        assert_eq!(humanize_ago(3700), "1h fa");
        assert_eq!(humanize_ago(90_000), "1g fa");
    }

    #[test]
    fn humanize_size_formats() {
        assert_eq!(humanize_size(500), "500 B");
        assert_eq!(humanize_size(2048), "2.0 KB");
        assert!(humanize_size(2 * 1024 * 1024).ends_with("MB"));
    }
}
