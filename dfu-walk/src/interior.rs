//! Interni degli edifici: con E vicino a una Taverna, una Gilda o un Negozio si entra.
//! Con E vicino alla porta (o Backspace ovunque) si esce.
//! Riconosce la vera porta d'ingresso/uscita ed esce con certezza premendo E entro 3.5 metri.
//!
//! ## NPC flat vs NPC custom
//!
//! - **Flat** (dal BlockData interno): sempre presenti nel loro edificio (salvo `InteriorState::recruited`).
//! - **Custom** (`npc_*.png` con `.txt`): dipendono da `position` (vedi `NpcPosition`).
//!
//! ## Animazioni degli NPC custom
//!
//! Accanto a `npc_<nome>.png` (idle base), si possono mettere due set di frame:
//!
//! ```text
//! npc_ellie.png             ← idle base (immagine ferma)
//! npc_ellie_walk_0.png      ← frame walk 0
//! npc_ellie_walk_1.png      ← frame walk 1
//! npc_ellie_idle_0.png      ← frame idle 0
//! npc_ellie_idle_1.png      ← frame idle 1
//! npc_ellie_idle_2.png      ← frame idle 2
//! ```
//!
//! I frame vengono scoperti automaticamente: cerca `_walk_0`, `_walk_1`, ... e
//! `_idle_0`, `_idle_1`, ... finché esistono consecutivi (partendo da 0).
//! Il ciclo walk avviene quando l'NPC è follower e si muove; il ciclo idle
//! alterna periodi statici e periodi animati quando è fermo (vedi
//! `tiles::CustomIdleAnim`). Se non ci sono `_walk_N` / `_idle_N`, l'NPC
//! resta statico (retrocompatibile).
//!
//! ## Idle per i flat normali
//!
//! Anche i flat vanilla possono avere un set di idle, con la convenzione
//! `<archive>_<record>-0_idle_N.png` accanto alla texture originale
//! `<archive>_<record>-0.png` (es. `182_11-0.png` → `182_11-0_idle_0.png`,
//! `182_11-0_idle_1.png`, ...). Vengono caricati da `load_flat_idle_frames` e
//! agganciati al `Follower` con `with_idle`, esattamente come per i custom.
//! Stessa cosa per il mood: `mood > 0` mostra staticamente `idle[mood]`.
//!
//! ## Aspect ratio
//!
//! Ogni frame walk/idle mantiene il proprio aspect ratio. Le dimensioni in
//! metri di ciascun frame si calcolano in proporzione a quelle dell'idle base:
//! se il frame ha pixel `(w_px, h_px)` e l'idle base ha pixel
//! `(base_w_px, base_h_px)` e metri `(base_w_m, base_h_m)`, allora
//! `frame_w_m = base_w_m * w_px / base_w_px` (analogo per l'altezza).
//!
//! **Case-insensitive**: la scoperta dei frame e del `.txt` non distingue
//! maiuscole/minuscole.
//!
//! Le velocità si configurano nel `.txt`:
//! - `walk_fps = 8.0` (default 8.0)
//! - `idle_fps = 4.0` (default 4.0, **non usato** se il frame deve cambiare
//!   ogni `IDLE_FRAME_SECONDS` fissi in `update_followers`; vedi `tiles.rs`)
use crate::dialog::DialogState;
use crate::mod_textures::ModTextures;
use crate::npc_home::NpcHome;
use crate::player::{Player, EYE_HEIGHT};
use crate::tiles::{climate_of, next_rand};
use crate::wall_textures::WallTextures;
use crate::world::*;
use bevy::math::Affine2;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use crate::data::blocks::RmbBlock;
use crate::data::bsa::BsaArchive;
use crate::data::building_gen::DF_UNIT;
use crate::data::city_map::{collect_city_map, MapBuilding, PoiKind};
use crate::data::find_file_ci;
use crate::data::interior::{build_interior, InteriorScene};
use crate::data::mesh_builder::{Submesh, WorldMesh};
use crate::data::terrain_mesh::MAP_PIXEL_SIZE;
use std::collections::HashMap;
use std::fs;

const INTERIOR_Y: f32 = 6000.0;
const ENTER_DISTANCE: f32 = 4.5;
const EXIT_DISTANCE: f32 = 3.5;
const WALK_SPEED: f32 = 4.0;
const RUN_MULTIPLIER: f32 = 2.0;
const GRAVITY: f32 = 20.0;
const JUMP_SPEED: f32 = 5.5;
const RADIUS: f32 = 0.3;
const STEP_UP: f32 = 0.6;
const SNAP_DOWN: f32 = 0.5;
const DOOR_REACH: f32 = 2.2;
const DOOR_OPEN_SECONDS: f32 = 1.5;
const DOOR_OPEN_SIGN: f32 = 1.0;

// =============================================================================
// Posizione di spawn di un NPC custom
// =============================================================================

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum NpcPosition {
    #[default]
    Any,
    None,
    ByKind { kind: PoiKind, index: Option<usize> },
    ByPrefix { prefix: String, index: Option<usize> },
}

fn parse_position_value(value: &str) -> Option<NpcPosition> {
    let v = value.trim().to_ascii_lowercase();
    match v.as_str() {
        "any" | "" => return Some(NpcPosition::Any),
        "none" | "never" => return Some(NpcPosition::None),
        _ => {}
    }
    let parts: Vec<&str> = v.split_whitespace().collect();
    let first = parts.first().copied()?;
    let index: Option<usize> = parts
    .get(1)
    .and_then(|s| s.parse::<usize>().ok())
    .map(|n| n.saturating_sub(1));

    let prefix = match first {
        "mage" | "mages" | "magic" => Some("MAGE"),
        "fighter" | "fighters" | "figh" => Some("FIGH"),
        "thief" | "thieves" | "thie" => Some("THIE"),
        "dark" => Some("DARK"),
        "guil" => Some("GUIL"),
        "temp" | "temple" | "temples" => Some("TEMP"),
        "ptem" => Some("PTEM"),
        "pala" => Some("PALA"),
        "cast" | "castle" => Some("CAST"),
        _ => None,
    };
    if let Some(p) = prefix {
        return Some(NpcPosition::ByPrefix { prefix: p.to_string(), index });
    }
    let kind = match first {
        "tavern" | "taverns" => PoiKind::Tavern,
        "shop" | "shops" => PoiKind::Shop,
        "guild" | "guilds" => PoiKind::Guild,
        "palace" | "palaces" => PoiKind::Palace,
        _ => return None,
    };
    Some(NpcPosition::ByKind { kind, index })
}

// =============================================================================
// FLATS.CFG
// =============================================================================
#[derive(Clone, Copy, Debug)]
struct FlatInfo {
    face: usize,
}

#[derive(Default)]
pub struct FlatsFaceMap {
    entries: HashMap<(u16, u8), Vec<FlatInfo>>,
}

impl FlatsFaceMap {
    fn load(data: &GameData) -> Self {
        let mut map = Self::default();
        let Some(path) = find_file_ci(&data.dir, "FLATS.CFG") else {
            info!("FLATS.CFG non trovato: i volti interni useranno la generazione procedurale");
            return map;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            warn!("FLATS.CFG: impossibile leggere {}", path.display());
            return map;
        };
        let mut valid = 0usize;
        let mut skipped = 0usize;
        let mut lines = text.lines().peekable();
        while let Some(line) = lines.next() {
            let line = line.trim();
            if line.is_empty() { continue; }
            let tokens: Vec<&str> = line.split_whitespace().collect();
            if tokens.len() >= 6
                && tokens[0].parse::<u16>().is_ok()
                && tokens[2].parse::<u32>().is_ok()
                && tokens[3].parse::<usize>().is_ok()
                {
                    let archive = tokens[0].parse::<u16>().unwrap();
                    let record = tokens[1].parse::<u8>().unwrap();
                    let face = tokens[3].parse::<usize>().unwrap();
                    if archive <= 999 {
                        map.entries.entry((archive, record)).or_default().push(FlatInfo { face });
                        valid += 1;
                    } else { skipped += 1; }
                } else if tokens.len() == 2
                    && tokens[0].parse::<u16>().is_ok()
                    && tokens[1].parse::<u8>().is_ok()
                    {
                        let archive = tokens[0].parse::<u16>().unwrap();
                        let record = tokens[1].parse::<u8>().unwrap();
                        for _ in 0..4 { if lines.next().is_none() { break; } }
                        if let Some(face_line) = lines.next() {
                            if let Ok(face) = face_line.trim().parse::<usize>() {
                                if archive <= 999 {
                                    map.entries.entry((archive, record)).or_default().push(FlatInfo { face });
                                    valid += 1;
                                } else { skipped += 1; }
                            } else { skipped += 1; }
                        }
                    } else { skipped += 1; }
        }
        if valid == 0 {
            warn!("FLATS.CFG: nessuna voce valida.");
        } else {
            info!("FLATS.CFG: {} voci valide, {} scartate, {} flat distinti", valid, skipped, map.entries.len());
        }
        map
    }

    fn face_for(&self, archive: u16, record: u8) -> Option<usize> {
        let entries = self.entries.get(&(archive, record))?;
        entries.first().map(|e| e.face)
    }
}

struct Active {
    root: Entity,
    origin: Vec3,
    scene: InteriorScene,
    saved_translation: Vec3,
    saved_feet_y: f32,
    saved_yaw: f32,
    saved_origin: (i32, i32),
    kind: PoiKind,
    building: String,
}

#[derive(Resource, Default)]
pub struct InteriorState {
    active: Option<Active>,
    pub recruited: std::collections::HashSet<(u16, u8)>,
    pub recruited_custom: std::collections::HashSet<String>,
}

impl InteriorState {
    pub fn is_inside(&self) -> bool { self.active.is_some() }
    pub fn building(&self) -> Option<String> { self.active.as_ref().map(|a| a.building.clone()) }
    pub fn origin(&self) -> Option<Vec3> { self.active.as_ref().map(|a| a.origin) }
    pub fn kind(&self) -> Option<PoiKind> { self.active.as_ref().map(|a| a.kind) }
    pub fn mark_recruited(&mut self, archive: u16, record: u8) { self.recruited.insert((archive, record)); }
    pub fn is_recruited(&self, archive: u16, record: u8) -> bool { self.recruited.contains(&(archive, record)) }
    pub fn mark_recruited_custom(&mut self, name: &str) { self.recruited_custom.insert(name.to_string()); }
    pub fn is_recruited_custom(&self, name: &str) -> bool { self.recruited_custom.contains(name) }
}

pub fn outside(state: Res<InteriorState>) -> bool { !state.is_inside() }

#[derive(Component)]
pub struct FlatBillboard;

#[derive(Component)]
pub struct InteriorNpc {
    pub face_record: usize,
    pub flat_archive: u16,
    pub flat_record: u8,
    pub size: Vec2,
}

#[derive(Component)]
pub struct CustomNpcName {
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct CustomNpcData {
    pub name: String,
    pub texture_stem: String,
    pub description: Option<String>,
    pub voice: Option<String>,
    pub position: NpcPosition,
    /// Stem dei file walk (`npc_ellie_walk_0`, `npc_ellie_walk_1`, ...). Vuoto
    /// se l'NPC non ha animazione di camminata.
    pub walk_stems: Vec<String>,
    /// FPS dell'animazione camminata (default 8.0).
    pub walk_fps: f32,
    /// Stem dei file idle (`npc_ellie_idle_0`, `npc_ellie_idle_1`, ...). Vuoto
    /// se l'NPC non ha idle multiframe (in quel caso l'idle resta statico).
    pub idle_stems: Vec<String>,
    /// FPS dell'animazione idle (default 4.0). Nota: il ciclo visivo dell'idle
    /// in `tiles::update_followers` usa `IDLE_FRAME_SECONDS`, non `idle_fps`.
    pub idle_fps: f32,
}

pub fn parse_custom_txt(content: &str) -> (String, Option<String>, Option<String>, NpcPosition) {
    let mut lines = content.trim_start_matches('\u{feff}').lines();
    let name = lines
    .by_ref()
    .map(str::trim)
    .find(|l| !l.is_empty())
    .unwrap_or("")
    .to_string();
    let mut description = None;
    let mut voice = None;
    let mut position: Option<NpcPosition> = None;
    for line in lines {
        let line = line.trim();
        let (key, rest) = if let Some(r) = line.strip_prefix("description") {
            ("description", r)
        } else if let Some(r) = line.strip_prefix("voice") {
            ("voice", r)
        } else if let Some(r) = line.strip_prefix("position") {
            ("position", r)
        } else {
            continue;
        };
        let Some(value) = rest.trim_start().strip_prefix('=') else { continue };
        let value = value.trim();
        let value = value.strip_prefix('"').unwrap_or(value);
        let value = value.strip_suffix('"').unwrap_or(value).trim();
        if value.is_empty() { continue; }
        match key {
            "description" if description.is_none() => description = Some(value.to_string()),
            "voice" if voice.is_none() => voice = Some(value.split_whitespace().collect::<String>()),
            "position" if position.is_none() => {
                position = parse_position_value(value);
                if position.is_none() {
                    warn!("NPC custom: valore di position non riconosciuto: '{}'", value);
                }
            }
            _ => {}
        }
    }
    (name, description, voice, position.unwrap_or(NpcPosition::Any))
}

pub fn parse_walk_fps(content: &str) -> f32 {
    let mut fps = 8.0f32;
    for line in content.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("walk_fps") else { continue };
        let Some(value) = rest.trim_start().strip_prefix('=') else { continue };
        if let Ok(n) = value.trim().parse::<f32>() {
            fps = n.max(0.1);
        }
    }
    fps
}

pub fn parse_idle_fps(content: &str) -> f32 {
    let mut fps = 4.0f32;
    for line in content.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("idle_fps") else { continue };
        let Some(value) = rest.trim_start().strip_prefix('=') else { continue };
        if let Ok(n) = value.trim().parse::<f32>() {
            fps = n.max(0.1);
        }
    }
    fps
}

#[derive(Resource, Default)]
pub struct CustomNpcCache {
    pub npcs: Vec<CustomNpcData>,
    /// Cartella delle texture sciolte: ci finiscono anche i file base degli
    /// NPC classici (`classic_*.txt`, vedi `classic_npc.rs`).
    pub dir: std::path::PathBuf,
}

impl CustomNpcCache {
    /// Scansiona `textures_dir` cercando `<qualcosa>/*.txt` accanto a `<qualcosa>.png`
    /// con prefisso `npc_` (case-insensitive) e, per ognuno, i frame walk
    /// `<stem>_walk_0.png`, ... e idle `<stem>_idle_0.png`, ... (case-insensitive).
    pub fn load(textures_dir: &std::path::Path) -> Self {
        let mut npcs = Vec::new();

        let mut index: HashMap<String, std::path::PathBuf> = HashMap::new();
        let mut all_files: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(entries) = fs::read_dir(textures_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                    index.insert(name.to_ascii_lowercase(), path.clone());
                }
                all_files.push(path);
            }
        }
        let find = |name: &str| -> Option<&std::path::PathBuf> {
            index.get(name.to_ascii_lowercase().as_str())
        };

        for path in &all_files {
            let is_txt = path
            .extension()
            .and_then(|e| e.to_str())
            .map_or(false, |e| e.eq_ignore_ascii_case("txt"));
            if is_txt {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !stem.to_ascii_lowercase().starts_with("npc_") {
                continue;
            }
            if npcs
                .iter()
                .any(|n: &CustomNpcData| n.texture_stem.eq_ignore_ascii_case(stem))
                {
                    continue;
                }
                let Some(txt_path) = find(&format!("{stem}.txt")) else {
                    continue;
                };
            let Ok(content) = fs::read_to_string(txt_path) else {
                continue;
            };
            let (trimmed_name, description, voice, position) = parse_custom_txt(&content);
            if trimmed_name.is_empty() {
                continue;
            }
            let walk_fps = parse_walk_fps(&content);
            let idle_fps = parse_idle_fps(&content);

            let mut walk_stems = Vec::new();
            let mut i = 0usize;
            loop {
                let candidate = format!("{stem}_walk_{i}.png");
                if find(&candidate).is_some() {
                    walk_stems.push(format!("{stem}_walk_{i}"));
                    i += 1;
                } else {
                    break;
                }
            }
            if !walk_stems.is_empty() {
                info!(
                    "NPC custom '{}': {} frame walk @ {} fps",
                    trimmed_name, walk_stems.len(), walk_fps
                );
            }

            let mut idle_stems = Vec::new();
            let mut j = 0usize;
            loop {
                let candidate = format!("{stem}_idle_{j}.png");
                if find(&candidate).is_some() {
                    idle_stems.push(format!("{stem}_idle_{j}"));
                    j += 1;
                } else {
                    break;
                }
            }
            if !idle_stems.is_empty() {
                info!(
                    "NPC custom '{}': {} frame idle @ {} fps",
                    trimmed_name, idle_stems.len(), idle_fps
                );
            }

            npcs.push(CustomNpcData {
                name: trimmed_name,
                texture_stem: stem.to_string(),
                      description,
                      voice,
                      position,
                      walk_stems,
                      walk_fps,
                      idle_stems,
                      idle_fps,
            });
        }
        let specific = npcs.iter().filter(|n| !matches!(n.position, NpcPosition::Any)).count();
        let animated = npcs.iter().filter(|n| !n.walk_stems.is_empty()).count();
        let idle = npcs.iter().filter(|n| !n.idle_stems.is_empty()).count();
        info!(
            "Caricati {} NPC custom ({} con position specifica, {} con walk, {} con idle)",
              npcs.len(), specific, animated, idle
        );
        Self { npcs, dir: textures_dir.to_path_buf() }
    }
}

/// Carica uno dei set di frame (`stems`) come materiali, calcolando per ogni
/// frame la dimensione in metri in proporzione all'idle base.
///
/// `base_size_px`: dimensioni in pixel dell'idle base (il PNG che dà il nome
/// all'NPC o il `<archive>_<record>-0.png` di un flat). Se `x` o `y` sono
/// <= 0.5, la proporzione è disattivata e ogni frame usa `base_size_m`.
/// `base_size_m`: dimensioni in metri dell'idle base.
/// `size_scale`: moltiplicatore finale applicato a ogni frame (1.0 = nessuna
/// riduzione; 0.5 = metà).
///
/// Ritorna `(frames, sizes)` con `sizes[i]` = dimensioni in metri del frame i.
pub(crate) fn load_frames_from_stems(
    stems: &[String],
    npc_name: &str,
    label: &str,
    base_size_px: Vec2,
    base_size_m: Vec2,
    size_scale: f32,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<(Vec<Handle<StandardMaterial>>, Vec<Vec2>)> {
    if stems.is_empty() {
        return None;
    }
    let base_size_px = if base_size_px.x > 0.5 && base_size_px.y > 0.5 {
        base_size_px
    } else {
        Vec2::splat(1.0)
    };
    let scale = size_scale.max(0.05);
    let mut frames = Vec::with_capacity(stems.len());
    let mut sizes = Vec::with_capacity(stems.len());
    for stem in stems {
        let Some(handle) = mods.image_by_name(stem, images) else {
            warn!("{} '{}': manca il frame {label} '{}'", npc_name, label, stem);
            return None;
        };
        let frame_size_px = mods
        .image_size(&handle)
        .or_else(|| images.get(&handle).map(|i| i.size_f32()))
        .unwrap_or(base_size_px);
        let size_m = if frame_size_px.x > 0.5 && frame_size_px.y > 0.5 {
            Vec2::new(
                base_size_m.x * frame_size_px.x / base_size_px.x * scale,
                base_size_m.y * frame_size_px.y / base_size_px.y * scale,
            )
        } else {
            base_size_m * scale
        };
        frames.push(materials.add(StandardMaterial {
            base_color_texture: Some(handle),
                                  alpha_mode: AlphaMode::Mask(0.5),
                                  cull_mode: None,
                                  double_sided: true,
                                  perceptual_roughness: 0.9,
                                  reflectance: 0.1,
                                  ..default()
        }));
        sizes.push(size_m);
    }
    Some((frames, sizes))
}

/// Frame dell'animazione di camminata + le loro dimensioni in metri
/// (per preservare l'aspect ratio di ogni frame).
pub fn load_custom_walk_frames(
    data: &CustomNpcData,
    base_size_px: Vec2,
    base_size_m: Vec2,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<(Vec<Handle<StandardMaterial>>, Vec<Vec2>)> {
    load_frames_from_stems(
        &data.walk_stems,
        &data.name,
        "walk",
        base_size_px,
        base_size_m,
        1.0,
        mods,
        images,
        materials,
    )
}

/// Frame dell'animazione idle + le loro dimensioni in metri
/// (per preservare l'aspect ratio di ogni frame).
pub fn load_custom_idle_frames(
    data: &CustomNpcData,
    base_size_px: Vec2,
    base_size_m: Vec2,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<(Vec<Handle<StandardMaterial>>, Vec<Vec2>)> {
    load_frames_from_stems(
        &data.idle_stems,
        &data.name,
        "idle",
        base_size_px,
        base_size_m,
        1.0,
        mods,
        images,
        materials,
    )
}

/// Carica i frame idle di un flat (NPC normale), con la convenzione
/// `<archive>_<record>-<frame>_idle_<N>.png` (o `<archive:03>_...` con padding).
/// Se non trova nessun `_idle_N` per quel flat, ritorna `None`.
///
/// `base_size_px` = dimensioni in pixel dell'idle base del flat (es. la
/// texture originale `182_11-0.png`). `base_size_m` = dimensioni in metri da
/// assegnare ai frame, tipicamente già scalate dal chiamante (es. `size * 0.5`).
///
/// `frame` di solito è 0 (i flat usano frame singolo); l'argomento serve nel
/// caso in cui gli idle siano associati a una variante specifica.
pub fn load_flat_idle_frames(
    archive: u16,
    record: u8,
    frame: u16,
    base_size_px: Vec2,
    base_size_m: Vec2,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<(Vec<Handle<StandardMaterial>>, Vec<Vec2>)> {
    // Prova entrambi i prefissi (padding a 3 cifre e senza).
    let prefixes = [
        format!("{:03}_{}-{}", archive, record, frame),
            format!("{}_{}-{}", archive, record, frame),
    ];
    for prefix in &prefixes {
        let mut stems = Vec::new();
        let mut i = 0usize;
        loop {
            let candidate = format!("{prefix}_idle_{i}");
            if mods.has_image(&candidate) {
                stems.push(candidate);
                i += 1;
            } else {
                break;
            }
        }
        if !stems.is_empty() {
            info!(
                "flat {}:{}: {} frame idle trovati (prefisso '{}')",
                  archive, record, stems.len(), prefix
            );
            // `size_scale = 1.0`: base_size_m è già quello scalato dal chiamante.
            return load_frames_from_stems(
                &stems,
                &format!("flat {}:{}", archive, record),
                                          "idle",
                                          base_size_px,
                                          base_size_m,
                                          1.0,
                                          mods,
                                          images,
                                          materials,
            );
        }
    }
    None
}

#[derive(Component)]
pub struct RoomDoor {
    index: usize,
    closed_y: f32,
    progress: f32,
}

fn distance_to_building(b: &MapBuilding, px: f32, pz: f32) -> f32 {
    let (s, c) = b.rot_deg.to_radians().sin_cos();
    let (dx, dz) = (px - b.cx, pz - b.cz);
    let lx = dx * c - dz * s;
    let lz = dx * s + dz * c;
    let ox = (lx.abs() - b.half_w).max(0.0);
    let oz = (lz.abs() - b.half_d).max(0.0);
    ox.hypot(oz)
}

fn submesh_to_bevy(mesh: &WorldMesh, sm: &Submesh, size: (f32, f32)) -> Mesh {
    let mut remap: HashMap<u32, u32> = HashMap::new();
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::with_capacity(sm.indices.len());
    for &i in &sm.indices {
        let new = *remap.entry(i).or_insert_with(|| {
            let k = positions.len() as u32;
            positions.push(mesh.vertices[i as usize]);
            normals.push(mesh.normals[i as usize]);
            let uv = mesh.uvs[i as usize];
            uvs.push([uv[0] / size.0, uv[1] / size.1]);
            k
        });
        indices.push(new);
    }
    let mut out = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    out.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    out.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    out.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    out.insert_indices(Indices::U32(indices));
    out
}

fn debug_uv_scale(mesh: &WorldMesh, sm: &Submesh, size: (f32, f32)) {
    let mut ratios = Vec::new();
    for t in sm.indices.chunks_exact(3) {
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let (ia, ib) = (t[a] as usize, t[b] as usize);
            let (pa, pb) = (mesh.vertices[ia], mesh.vertices[ib]);
            let d = ((pa[0] - pb[0]).powi(2) + (pa[1] - pb[1]).powi(2) + (pa[2] - pb[2]).powi(2)).sqrt();
            let (ua, ub) = (mesh.uvs[ia], mesh.uvs[ib]);
            let (du, dv) = ((ua[0] - ub[0]) / size.0, (ua[1] - ub[1]) / size.1);
            let duv = (du * du + dv * dv).sqrt();
            if d > 0.05 && duv > 1e-6 {
                ratios.push(d / duv);
            }
        }
    }
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if let Some(m) = ratios.get(ratios.len() / 2) {
        println!(
            "UV dbg {}/{} size {:?}: {} spigoli, metri per ripetizione (mediana) = {:.4}",
                 sm.texture_archive, sm.texture_record, size, ratios.len(), m
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn interior_enter_exit(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    data: Res<GameData>,
    origin: Res<Origin>,
    mut state: ResMut<InteriorState>,
    mut clear: ResMut<ClearColor>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut wall_textures: ResMut<WallTextures>,
    mut mods: ResMut<ModTextures>,
    mut player: Query<(&mut Player, &mut Transform)>,
                           mut flats_face_map: Local<Option<FlatsFaceMap>>,
                           custom_npcs: Res<CustomNpcCache>,
                           mut npc_home: ResMut<NpcHome>,
) {
    let leave = keys.just_pressed(KeyCode::Backspace);
    let act = keys.just_pressed(KeyCode::KeyE);
    if !leave && !act { return; }
    let Ok((mut p, mut tf)) = player.get_single_mut() else { return; };

    if state.active.is_some() {
        let near_exit = {
            let a = state.active.as_ref().unwrap();
            let local = tf.translation - a.origin;
            let d_spawn = (local.x - a.scene.spawn[0]).hypot(local.z - a.scene.spawn[2]);
            let d_exit = a.scene.exit.map_or(f32::MAX, |e| {
                (local.x - e[0]).hypot(local.z - e[2])
            });
            d_spawn < EXIT_DISTANCE || d_exit < EXIT_DISTANCE
        };
        if leave || near_exit {
            let a = state.active.take().unwrap();
            let (de, dn) = (origin.east - a.saved_origin.0, origin.north - a.saved_origin.1);
            let mut pos = a.saved_translation;
            pos.x -= de as f32 * MAP_PIXEL_SIZE;
            pos.z += dn as f32 * MAP_PIXEL_SIZE;
            p.feet_y = a.saved_feet_y;
            p.vel_y = 0.0;
            p.on_ground = true;
            p.yaw = a.saved_yaw + std::f32::consts::PI;
            tf.translation = pos;
            commands.entity(a.root).despawn_recursive();
            clear.0 = Color::srgb(0.55, 0.72, 0.92);
            println!("Sei uscito dall'edificio.");
        }
        return;
    }

    if !act { return; }
    if flats_face_map.is_none() {
        *flats_face_map = Some(FlatsFaceMap::load(&data));
    }
    let (mx, my) = map_pixel_of(&origin, tf.translation);
    let Some(loc) = data.locations.get(&(mx, my)) else {
        println!("Qui non c'e' nessuna citta'.");
        return;
    };
    let (Ok(blocks), Ok(arch)) = (
        BsaArchive::parse(&data.blocks_buf),
                                  BsaArchive::parse(&data.arch3d_buf),
    ) else {
        println!("Interni: impossibile leggere BLOCKS.BSA o ARCH3D.BSA.");
        return;
    };
    let map = collect_city_map(loc, &blocks, &arch);
    let tile = tile_translation(&origin, mx, my);
    let (px, pz) = (tf.translation.x - tile.x, tf.translation.z - tile.z);
    let mut best: Option<(f32, (usize, usize, u8), PoiKind)> = None;
    let (mut places, mut nearest) = (0usize, f32::MAX);
    for b in &map.buildings {
        let Some(kind) = b.kind.filter(|k| matches!(k, PoiKind::Tavern | PoiKind::Shop | PoiKind::Guild)) else { continue; };
        let Some(src) = b.source else { continue; };
        let d = distance_to_building(b, px, pz);
        places += 1;
        nearest = nearest.min(d);
        if d <= ENTER_DISTANCE && best.map_or(true, |(bd, _, _)| d < bd) {
            best = Some((d, src, kind));
        }
    }
    let Some((_, (bx, by, sub), kind)) = best else {
        if places == 0 {
            println!("Interni: nessuna taverna/gilda/negozio in {} (map pixel {mx},{my}).", loc.name);
        } else {
            println!("Interni: {} edifici in {}, il piu' vicino a {:.1} m (serve entro {ENTER_DISTANCE} m).", places, loc.name, nearest);
        }
        return;
    };
    let Some(name) = loc.block_names.get(by * loc.width + bx) else {
        println!("Interni: blocco ({bx},{by}) fuori dalla location.");
        return;
    };
    let Some(raw) = blocks.get_by_name(name) else {
        println!("Interni: il blocco {name} non esiste in BLOCKS.BSA.");
        return;
    };
    let Ok(rmb) = RmbBlock::parse(raw) else {
        println!("Interni: il blocco {name} non si legge.");
        return;
    };
    let Some(scene) = build_interior(&rmb, sub as usize, &arch) else {
        println!("L'interno di {name} (sotto-record {sub}) non ha modelli 3D.");
        return;
    };

    let building_id = format!("{mx},{my}:{bx},{by},{sub}");
    let current_prefix: String = name.get(..4).unwrap_or(name).to_ascii_uppercase();

    let kind_index = map
    .buildings
    .iter()
    .filter(|b| b.kind == Some(kind))
    .position(|b| b.source == Some((bx, by, sub)))
    .unwrap_or(0);

    let prefix_index = map
    .buildings
    .iter()
    .filter(|b| {
        b.source
        .and_then(|(bx, by, _)| loc.block_names.get(by * loc.width + bx))
        .map(|n| n.get(..4).unwrap_or(n).eq_ignore_ascii_case(&current_prefix))
        .unwrap_or(false)
    })
    .position(|b| b.source == Some((bx, by, sub)))
    .unwrap_or(0);

    let flats: Vec<(u16, u8)> = scene.people.iter().map(|p| (p.archive, p.record)).collect();
    let is_home = crate::npc_home::claim_home_on_enter(&mut npc_home, &building_id, &flats);
    if is_home && npc_home.building.is_some() {
        crate::npc_home::save(&npc_home);
    }

    let origin_world = Vec3::new(tf.translation.x, INTERIOR_Y, tf.translation.z);
    let climate = climate_of(&data, mx, my);
    let face_lookup = flats_face_map.as_ref().expect("inizializzata sopra");
    let root = commands
    .spawn((Transform::from_translation(origin_world), Visibility::default()))
    .with_children(|parent| {
        for sm in &scene.mesh.submeshes {
            if sm.indices.is_empty() { continue; }
            let size = wall_textures.original_size(sm.texture_archive, sm.texture_record, &data);
            debug_uv_scale(&scene.mesh, sm, size);
            let handle = wall_textures.material(
                sm.texture_archive, sm.texture_record, climate,
                &data, &mut mods, &mut images, &mut materials,
            );
            if let Some(m) = materials.get_mut(&handle) {
                m.double_sided = true;
                m.cull_mode = None;
            }
            let mut bevy_mesh = submesh_to_bevy(&scene.mesh, sm, size);
            wall_textures.prepare_mesh(&handle, &mut bevy_mesh);
            parent.spawn((Mesh3d(meshes.add(bevy_mesh)), MeshMaterial3d(handle), Transform::IDENTITY));
        }
        for (i, d) in scene.doors.iter().enumerate() {
            let closed = -d.yaw_deg.to_radians();
            parent
            .spawn((
                Transform {
                    translation: Vec3::from(d.pos),
                    rotation: Quat::from_rotation_y(closed),
                    ..default()
                },
                Visibility::default(),
                    RoomDoor { index: i, closed_y: closed, progress: 0.0 },
            ))
            .with_children(|door| {
                for sm in &d.mesh.submeshes {
                    if sm.indices.is_empty() { continue; }
                    let size = wall_textures.original_size(sm.texture_archive, sm.texture_record, &data);
                    let handle = wall_textures.material(
                        sm.texture_archive, sm.texture_record, climate,
                        &data, &mut mods, &mut images, &mut materials,
                    );
                    if let Some(m) = materials.get_mut(&handle) {
                        m.double_sided = true;
                        m.cull_mode = None;
                    }
                    let mut bevy_mesh = submesh_to_bevy(&d.mesh, sm, size);
                    wall_textures.prepare_mesh(&handle, &mut bevy_mesh);
                    door.spawn((Mesh3d(meshes.add(bevy_mesh)), MeshMaterial3d(handle), Transform::IDENTITY));
                }
            });
        }
        for f in &scene.flats {
            let Some((material, w_px, h_px)) = wall_textures.flat_material(
                f.archive, f.record, &data, &mut mods, &mut images, &mut materials,
            ) else { continue; };
            let (w, h) = (w_px * DF_UNIT, h_px * DF_UNIT);
            parent.spawn((
                Mesh3d(meshes.add(Rectangle::new(w, h))),
                          MeshMaterial3d(material),
                          Transform::from_xyz(f.pos[0], f.pos[1] + h * 0.5, f.pos[2]),
                          FlatBillboard,
            ));
        }
        for (i, f) in scene.people.iter().enumerate() {
            if state.is_recruited(f.archive, f.record) { continue; }
            let Some((material, w_px, h_px)) = wall_textures.flat_material(
                f.archive, f.record, &data, &mut mods, &mut images, &mut materials,
            ) else { continue; };
            let (w, h) = (w_px * DF_UNIT, h_px * DF_UNIT);
            let face_record = face_lookup.face_for(f.archive, f.record).unwrap_or_else(|| {
                ((f.archive as usize * 37) + (f.record as usize * 13) + (i * 7)) % 500
            });
            parent.spawn((
                Mesh3d(meshes.add(Rectangle::new(w, h))),
                          MeshMaterial3d(material),
                          Transform::from_xyz(f.pos[0], f.pos[1] + h * 0.5, f.pos[2]),
                          FlatBillboard,
                          InteriorNpc {
                              face_record,
                              flat_archive: f.archive,
                              flat_record: f.record,
                              size: Vec2::new(w, h),
                          },
            ));
        }
        for custom_npc in &custom_npcs.npcs {
            if state.is_recruited_custom(&custom_npc.name) { continue; }
            let should_spawn = match &custom_npc.position {
                NpcPosition::Any => is_home,
                NpcPosition::None => false,
                NpcPosition::ByKind { kind: target_kind, index } => {
                    *target_kind == kind && index.map_or(true, |i| i == kind_index)
                }
                NpcPosition::ByPrefix { prefix, index } => {
                    current_prefix == *prefix && index.map_or(true, |i| i == prefix_index)
                }
            };
            if !should_spawn { continue; }
            let Some(image_handle) = mods.image_by_name(&custom_npc.texture_stem, &mut images) else { continue; };
            let size_px = mods.image_size(&image_handle)
            .or_else(|| images.get(&image_handle).map(|i| i.size_f32()))
            .unwrap_or(Vec2::new(64.0, 128.0));
            let scale_factor = DF_UNIT * 0.074;
            let w = size_px.x * scale_factor;
            let h = size_px.y * scale_factor;
            let material = materials.add(StandardMaterial {
                base_color_texture: Some(image_handle),
                                         alpha_mode: AlphaMode::Mask(0.5),
                                         cull_mode: None,
                                         double_sided: true,
                                         perceptual_roughness: 0.9,
                                         reflectance: 0.1,
                                         ..default()
            });
            let mut rng_seed = custom_npc.texture_stem.len() as u64;
            let spawn_x = scene.spawn[0] + (next_rand(&mut rng_seed) - 0.5) * 4.0;
            let spawn_z = scene.spawn[2] + (next_rand(&mut rng_seed) - 0.5) * 4.0;
            parent.spawn((
                Mesh3d(meshes.add(Rectangle::new(w, h))),
                          MeshMaterial3d(material),
                          Transform::from_xyz(spawn_x, scene.spawn[1] + h * 0.5, spawn_z),
                          FlatBillboard,
                          InteriorNpc {
                              face_record: 0,
                              flat_archive: 0,
                              flat_record: 0,
                              size: Vec2::new(w, h),
                          },
                          CustomNpcName { name: custom_npc.name.clone() },
            ));
        }
        let cx = (scene.bounds_min[0] + scene.bounds_max[0]) * 0.5;
        let cz = (scene.bounds_min[2] + scene.bounds_max[2]) * 0.5;
        parent.spawn((
            PointLight {
                color: Color::srgb(1.0, 0.86, 0.62),
                      intensity: 600_000.0,
                      range: 40.0,
                      shadows_enabled: false,
                      ..default()
            },
            Transform::from_xyz(cx, scene.spawn[1] + 2.3, cz),
        ));
    })
    .id();
    let spawn = Vec3::from(scene.spawn);
    let exit = scene.exit.map(Vec3::from).unwrap_or(spawn);
    let look_dir = if scene.exit.is_some() {
        Vec3::new(spawn.x - exit.x, 0.0, spawn.z - exit.z)
    } else {
        let cx = (scene.bounds_min[0] + scene.bounds_max[0]) * 0.5;
        let cz = (scene.bounds_min[2] + scene.bounds_max[2]) * 0.5;
        Vec3::new(cx - spawn.x, 0.0, cz - spawn.z)
    };
    let people_count = scene.people.len();
    let saved = Active {
        root,
        origin: origin_world,
        saved_translation: tf.translation,
        saved_feet_y: p.feet_y,
        saved_yaw: p.yaw,
        saved_origin: (origin.east, origin.north),
        kind,
        building: building_id,
        scene,
    };
    p.feet_y = origin_world.y + spawn.y;
    p.vel_y = 0.0;
    p.on_ground = true;
    p.fly = false;
    if look_dir.length_squared() > 0.01 {
        let norm = look_dir.normalize();
        p.yaw = (-norm.x).atan2(-norm.z);
    }
    tf.translation = Vec3::new(
        origin_world.x + spawn.x,
        p.feet_y + EYE_HEIGHT,
        origin_world.z + spawn.z,
    );
    clear.0 = Color::BLACK;
    state.active = Some(saved);
    let what = match kind {
        PoiKind::Shop => "negozio",
        PoiKind::Guild => "gilda",
        _ => "taverna",
    };
    let custom_count = custom_npcs.npcs.iter().filter(|n| !state.is_recruited_custom(&n.name)).count();
    if is_home {
        println!("Entrato in {name} ({what}, sub {sub}): casa NPC ({people_count} flat + {custom_count} custom).");
    } else {
        println!("Entrato in {name} ({what}, sub {sub}): {people_count} flat.");
    }
}

pub fn interior_move(
    dialog: Res<DialogState>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<InteriorState>,
    mut player: Query<(&mut Player, &mut Transform)>,
) {
    if dialog.open { return; }
    let Some(active) = state.active.as_ref() else { return; };
    let Ok((mut p, mut tf)) = player.get_single_mut() else { return; };
    let dt = time.delta_secs().min(0.05);
    let (scene, o) = (&active.scene, active.origin);
    let forward = Vec3::new(-p.yaw.sin(), 0.0, -p.yaw.cos());
    let right = Vec3::new(p.yaw.cos(), 0.0, -p.yaw.sin());
    let axis = |pos: KeyCode, neg: KeyCode| keys.pressed(pos) as i32 as f32 - keys.pressed(neg) as i32 as f32;
    let wish = (forward * axis(KeyCode::KeyW, KeyCode::KeyS) + right * axis(KeyCode::KeyD, KeyCode::KeyA)).normalize_or_zero();
    let running = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let speed = WALK_SPEED * if running { RUN_MULTIPLIER } else { 1.0 };
    let total = speed * dt;
    let steps = (total / 0.1).ceil().max(1.0) as i32;
    let step = wish * (total / steps as f32);
    let mut pos = tf.translation;
    if step != Vec3::ZERO {
        for _ in 0..steps {
            let feet = p.feet_y - o.y;
            for cand in [step, Vec3::new(step.x, 0.0, 0.0), Vec3::new(0.0, 0.0, step.z)] {
                if cand == Vec3::ZERO { continue; }
                let n = pos + cand;
                if !scene.blocked(n.x - o.x, n.z - o.z, feet, RADIUS) { pos = n; break; }
            }
        }
    }
    let (lx, lz) = (pos.x - o.x, pos.z - o.z);
    let feet_local = p.feet_y - o.y;
    let floor = scene.floor_at(lx, lz, feet_local, STEP_UP);
    if keys.just_pressed(KeyCode::Space) && p.on_ground { p.vel_y = JUMP_SPEED; p.on_ground = false; }
    match floor {
        Some(f) if p.vel_y <= 0.0 && feet_local - f <= SNAP_DOWN => {
            p.feet_y = o.y + f; p.vel_y = 0.0; p.on_ground = true;
        }
        _ => {
            p.vel_y -= GRAVITY * dt; p.feet_y += p.vel_y * dt; p.on_ground = false;
            if let Some(f) = floor {
                if p.feet_y - o.y <= f { p.feet_y = o.y + f; p.vel_y = 0.0; p.on_ground = true; }
            }
        }
    }
    if p.feet_y - o.y < scene.bounds_min[1] - 5.0 {
        pos = Vec3::new(o.x + scene.spawn[0], 0.0, o.z + scene.spawn[2]);
        p.feet_y = o.y + scene.spawn[1]; p.vel_y = 0.0; p.on_ground = true;
    }
    pos.y = p.feet_y + EYE_HEIGHT;
    tf.translation = pos;
}

pub fn face_flats(
    player: Query<&Player>,
    mut flats: Query<&mut Transform, (With<FlatBillboard>, Without<crate::tiles::NpcWalker>)>,
) {
    let Ok(p) = player.get_single() else { return };
    let rot = Quat::from_rotation_y(p.yaw);
    for mut tf in &mut flats { tf.rotation = rot; }
}

pub fn interior_toggle_doors(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<InteriorState>,
    player: Query<(&Player, &Transform)>,
) {
    if !keys.just_pressed(KeyCode::KeyE) { return; }
    let Some(active) = state.active.as_mut() else { return; };
    let Ok((p, tf)) = player.get_single() else { return };
    let o = active.origin;
    let (lx, lz) = (tf.translation.x - o.x, tf.translation.z - o.z);
    let feet = p.feet_y - o.y;
    if let Some(i) = active.scene.toggle_nearest_door(lx, lz, feet, DOOR_REACH) {
        let open = active.scene.door_open[i];
        println!("Porta {i} {}.", if open { "aperta" } else { "chiusa" });
    }
}

pub fn animate_doors(
    time: Res<Time>,
    state: Res<InteriorState>,
    mut doors: Query<(&mut RoomDoor, &mut Transform)>,
) {
    let Some(active) = state.active.as_ref() else { return; };
    let step = time.delta_secs() / DOOR_OPEN_SECONDS;
    for (mut d, mut tf) in &mut doors {
        let open = active.scene.door_open.get(d.index).copied().unwrap_or(false);
        d.progress = if open { (d.progress + step).min(1.0) } else { (d.progress - step).max(0.0) };
        let eased = d.progress * d.progress * (3.0 - 2.0 * d.progress);
        tf.rotation = Quat::from_rotation_y(d.closed_y - DOOR_OPEN_SIGN * eased * std::f32::consts::FRAC_PI_2);
    }
}
