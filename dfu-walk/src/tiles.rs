//! Caricamento e streaming dei map pixel: terreno, città, vegetazione,
//! flat esterni (decorazioni) e NPC animati.
use crate::mod_textures::ModTextures;
use crate::player::Player;
use crate::terrain_material::{TerrainExtension, TerrainMaterial};
use crate::trees::{spawn_trees_for_chunk, NaturePool};
use crate::wall_textures::WallTextures;
use crate::world::*;
use bevy::pbr::ExtendedMaterial;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use crate::data::blocks::RmbBlock;
use crate::data::bsa::BsaArchive;
use crate::data::building_gen::{MaterialKind, MeshPart, MATERIAL_COUNT, DF_UNIT, BLOCK_SIZE_M};
use crate::data::city_buildings::build_city_buildings;
use crate::data::terrain_city::{assemble_city_tilemap, flatten_terrain_heights, CityTileRect};
use crate::data::terrain_mesh::{build_terrain_mesh, terrain_height, TerrainMeshData, CELL_SIZE};
use crate::data::{
    archive_file_name, find_file_ci, TextureFile, world_climate_settings, ClimateBase,
    encode_for_shader, generate_default_samples, generate_default_tilemap,
    ground_archive, HeightSamples,
};
use std::collections::HashMap;
use std::sync::Arc;

const MAX_PENDING: usize = 4;
const MAX_SPAWNS_PER_FRAME: usize = 2;
const DEFAULT_CLIMATE: i32 = 231;

/// Archivi texture dei passanti (da DFU `MobilePersonBillboard`).
/// UN archivio = UNA persona. Record 0-4: camminata (sud, sud-ovest, ovest,
/// nord-ovest, nord), ciascuno con più frame. Record 5: idle.
/// Le direzioni verso est si ottengono specchiando i record 3, 2 e 1.
const PERSON_ARCHIVES: [u16; 24] = [
    381, 382, 383, 384, // maschi Redguard
    385, 386, 391, 394, // maschi Breton
    387, 388, 389, 390, // maschi Nord
    392, 393, 451, 452, // femmine Nord
    395, 396, 397, 398, // femmine Redguard
    453, 454, 455, 456, // femmine Breton
];

/// Per ognuna degli 8 orientamenti rispetto alla camera: (record, specchiato).
/// Indice 0 = la persona cammina verso la camera, 4 = di spalle (tabella DFU `MoveAnims`).
const ORIENTATIONS: [(usize, bool); 8] = [
    (0, false), (1, false), (2, false), (3, false),
    (4, false), (3, true), (2, true), (1, true),
];
const IDLE_RECORD: u8 = 5;
const NPC_MAX_FRAMES: usize = 32;

const NPC_FRAME_SECONDS: f32 = 0.25; // 4 fps come DFU (MoveAnimSpeed)
const NPC_SPEED: f32 = 1.3; // m/s, come DFU movementSpeed
const NPC_STEP: f32 = 4.0; // lunghezza di un passo "di griglia" in metri
const NPC_IDLE_DISTANCE: f32 = 2.5; // si fermano se il giocatore è fermo qui vicino
const NPC_RANDOM_TURN_CHANCE: f32 = 0.025; // probabilità di cambiare direzione a ogni passo

const CARDINALS: [Vec2; 4] = [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::NEG_Y];

/// Sprite di una persona: materiali già pronti per ogni record/frame.
pub struct NpcSpriteSet {
    walk: [Vec<Handle<StandardMaterial>>; 5],
    walk_size: [Vec2; 5],
    idle: Vec<Handle<StandardMaterial>>,
    idle_size: Vec2,
}

/// Carica tutti i frame di un record con UNA sola apertura del file.
fn load_record(
    file: &TextureFile, record: usize, data: &GameData,
    images: &mut Assets<Image>, materials: &mut Assets<StandardMaterial>,
) -> Option<(Vec<Handle<StandardMaterial>>, Vec2)> {
    let rec = file.record(record)?;
    let frame_count = file.frame_count(record)?.min(NPC_MAX_FRAMES);
    // Fattore di scala del record (diviso 256 come in DFU).
    let (sx, sy) = (rec.scale_x as f32 / 256.0, rec.scale_y as f32 / 256.0);
    let mut handles = Vec::new();
    let mut size = Vec2::ONE;
    for frame in 0..frame_count {
        let Ok(img) = file.frame(record, frame) else { break };
        if img.width == 0 || img.height == 0 { break; }
        if frame == 0 {
            size = Vec2::new(img.width as f32 * (1.0 + sx), img.height as f32 * (1.0 + sy)) * DF_UNIT;
        }
        let mut image = Image::new(
            Extent3d { width: img.width as u32, height: img.height as u32, depth_or_array_layers: 1 },
            TextureDimension::D2,
            img.to_rgba(&data.palette, Some(0)),
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        image.sampler = bevy::image::ImageSampler::nearest();
        handles.push(materials.add(StandardMaterial {
            base_color_texture: Some(images.add(image)),
            alpha_mode: AlphaMode::Mask(0.5),
            cull_mode: None,
            double_sided: true,
            perceptual_roughness: 0.9,
            reflectance: 0.1,
            ..default()
        }));
    }
    (!handles.is_empty()).then_some((handles, size))
}

fn build_npc_sprites(
    archive: u16, data: &GameData,
    images: &mut Assets<Image>, materials: &mut Assets<StandardMaterial>,
) -> Option<NpcSpriteSet> {
    let path = find_file_ci(&data.dir, &archive_file_name(archive as u32))?;
    let file = TextureFile::open(&path).ok()?;
    let mut walk: [Vec<Handle<StandardMaterial>>; 5] = Default::default();
    let mut walk_size = [Vec2::ONE; 5];
    for rec in 0..5 {
        let (frames, size) = load_record(&file, rec, data, images, materials)?;
        walk[rec] = frames;
        walk_size[rec] = size;
    }
    let (idle, idle_size) = load_record(&file, IDLE_RECORD as usize, data, images, materials)
        .unwrap_or_else(|| (vec![walk[0][0].clone()], walk_size[0]));
    Some(NpcSpriteSet { walk, walk_size, idle, idle_size })
}

fn next_rand(state: &mut u64) -> f32 {
    if *state == 0 { *state = 0x9E37_79B9_7F4A_7C15; }
    *state ^= *state << 13; *state ^= *state >> 7; *state ^= *state << 17;
    ((*state >> 11) & 0xFF_FFFF) as f32 / 16_777_215.0
}

/// Sceglie il prossimo bersaglio sulla griglia cardinale (N/S/E/O), restando nei limiti.
/// Di norma continua dritto; cambia direzione con piccola probabilità o se è bloccato.
fn pick_target(pos: Vec2, heading: &mut Vec2, bounds: (Vec2, Vec2), rng: &mut u64, force_new: bool) -> Vec2 {
    let inside = |p: Vec2| p.x >= bounds.0.x && p.x <= bounds.1.x && p.y >= bounds.0.y && p.y <= bounds.1.y;
    let keep = !force_new && inside(pos + *heading * NPC_STEP) && next_rand(rng) >= NPC_RANDOM_TURN_CHANCE;
    if !keep {
        let first = (next_rand(rng) * 4.0) as usize % 4;
        for k in 0..4 {
            let d = CARDINALS[(first + k) % 4];
            if inside(pos + d * NPC_STEP) { *heading = d; break; }
        }
    }
    let target = pos + *heading * NPC_STEP;
    if inside(target) { target } else { pos }
}

/// NPC che cammina su griglia cardinale con animazioni direzionali (come DFU).
#[derive(Component)]
pub struct NpcWalker {
    sprites: Arc<NpcSpriteSet>,
    heading: Vec2,
    target: Vec2,
    frame: usize,
    frame_timer: f32,
    /// (record, specchiato, frame) attualmente mostrato; None = da assegnare.
    shown: Option<(usize, bool, usize)>,
    rng: u64,
    bounds: (Vec2, Vec2),
    samples: Arc<HeightSamples>,
}

#[derive(Component)]
pub struct TerrainTile {
    pub mx: i32,
    pub my: i32,
}

// ---------------------------------------------------------------------------
// Nemici selvatici (fuori dalle città)
// ---------------------------------------------------------------------------

const ENEMIES_PER_TILE: usize = 4; // nemici per map pixel selvaggio
const ENEMY_NOTICE_DISTANCE: f32 = 25.0; // distanza a cui notano il giocatore (m)
const ENEMY_LOSE_DISTANCE: f32 = 60.0; // distanza a cui smettono di inseguire (m)
const ENEMY_MELEE_RANGE: f32 = 2.0; // distanza di attacco (m)
const ENEMY_HOVER: f32 = 1.5; // quota extra dei nemici volanti (m)

// Velocità animazioni in fotogrammi/secondo (EnemyBasics di DFU).
const ENEMY_MOVE_FPS: f32 = 6.0;
const ENEMY_FLY_FPS: f32 = 10.0;
const ENEMY_ATTACK_FPS: f32 = 10.0;
const ENEMY_IDLE_FPS: f32 = 4.0;

/// Record di un archivio nemico (EnemyBasics): 0-4 movimento, 5-9 attacco,
/// 10-14 colpito, 15-19 idle (solo se `has_idle`). Come per i passanti, i 5 record
/// coprono gli angoli sud..nord e le direzioni a est si ottengono specchiando.
const ENEMY_ATTACK_FIRST: usize = 5;
const ENEMY_IDLE_FIRST: usize = 15;

struct EnemySpecies {
    /// Archivio texture (= `MaleTexture` in EnemyBasics di DFU).
    archive: u16,
    has_idle: bool,
    flying: bool,
    /// Velocità in m/s: valori stimati, DFU le legge da MONSTER.BSA.
    wander_speed: f32,
    chase_speed: f32,
}

static ENEMY_SPECIES: [EnemySpecies; 12] = [
    EnemySpecies { archive: 257, has_idle: true,  flying: false, wander_speed: 1.2, chase_speed: 3.0 }, // 0 Spriggan
    EnemySpecies { archive: 259, has_idle: true,  flying: false, wander_speed: 1.5, chase_speed: 4.5 }, // 1 Orso grizzly
    EnemySpecies { archive: 260, has_idle: true,  flying: false, wander_speed: 1.8, chase_speed: 5.5 }, // 2 Tigre dai denti a sciabola
    EnemySpecies { archive: 261, has_idle: true,  flying: false, wander_speed: 1.4, chase_speed: 3.5 }, // 3 Ragno
    EnemySpecies { archive: 262, has_idle: true,  flying: false, wander_speed: 1.2, chase_speed: 3.0 }, // 4 Orco
    EnemySpecies { archive: 263, has_idle: true,  flying: false, wander_speed: 2.0, chase_speed: 5.5 }, // 5 Centauro
    EnemySpecies { archive: 265, has_idle: true,  flying: false, wander_speed: 1.0, chase_speed: 2.5 }, // 6 Ninfa
    EnemySpecies { archive: 267, has_idle: true,  flying: false, wander_speed: 1.3, chase_speed: 3.2 }, // 7 Sergente orco
    EnemySpecies { archive: 268, has_idle: false, flying: true,  wander_speed: 1.8, chase_speed: 4.5 }, // 8 Arpia
    EnemySpecies { archive: 271, has_idle: true,  flying: false, wander_speed: 1.0, chase_speed: 2.5 }, // 9 Gigante
    EnemySpecies { archive: 275, has_idle: false, flying: false, wander_speed: 1.2, chase_speed: 3.0 }, // 10 Scorpione gigante
    EnemySpecies { archive: 276, has_idle: true,  flying: false, wander_speed: 1.2, chase_speed: 3.0 }, // 11 Sciamano orco
];

/// Specie possibili all'aperto per clima. Elenco approssimato (animali, mostri e orchi
/// degli incontri nelle terre selvagge): le tabelle esatte di FALL.EXE non sono leggibili da qui.
fn enemy_table(climate: ClimateBase) -> &'static [usize] {
    match climate {
        ClimateBase::Temperate => &[1, 2, 3, 4, 7, 5, 0, 6, 9],
        ClimateBase::Mountain => &[1, 2, 4, 7, 11, 9, 8],
        ClimateBase::Desert => &[10, 3, 2, 4, 8, 5],
        ClimateBase::Swamp => &[3, 10, 0, 6, 4, 8],
    }
}

type RecordSet = ([Vec<Handle<StandardMaterial>>; 5], [Vec2; 5]);

/// Carica 5 record consecutivi (le 5 direzioni di una stessa animazione).
fn load_record_set(
    file: &TextureFile, first: usize, data: &GameData,
    images: &mut Assets<Image>, materials: &mut Assets<StandardMaterial>,
) -> Option<RecordSet> {
    let mut frames: [Vec<Handle<StandardMaterial>>; 5] = Default::default();
    let mut sizes = [Vec2::ONE; 5];
    for i in 0..5 {
        let (f, s) = load_record(file, first + i, data, images, materials)?;
        frames[i] = f;
        sizes[i] = s;
    }
    Some((frames, sizes))
}

pub struct EnemySpriteSet {
    walk: RecordSet,
    attack: RecordSet,
    idle: Option<RecordSet>,
}

fn build_enemy_sprites(
    archive: u16, has_idle: bool, data: &GameData,
    images: &mut Assets<Image>, materials: &mut Assets<StandardMaterial>,
) -> Option<EnemySpriteSet> {
    let path = find_file_ci(&data.dir, &archive_file_name(archive as u32))?;
    let file = TextureFile::open(&path).ok()?;
    let walk = load_record_set(&file, 0, data, images, materials)?;
    let attack = load_record_set(&file, ENEMY_ATTACK_FIRST, data, images, materials)
        .unwrap_or_else(|| walk.clone());
    let idle = if has_idle { load_record_set(&file, ENEMY_IDLE_FIRST, data, images, materials) } else { None };
    Some(EnemySpriteSet { walk, attack, idle })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EnemyMode {
    Idle,
    Walk,
    Chase,
    Attack,
}

/// Nemico selvatico: vaga, nota il giocatore, lo insegue e attacca.
#[derive(Component)]
pub struct Enemy {
    sprites: Arc<EnemySpriteSet>,
    species: usize,
    mode: EnemyMode,
    heading: Vec2,
    timer: f32,
    frame: usize,
    frame_timer: f32,
    /// 0 = camminata, 1 = attacco, 2 = idle
    kind: u8,
    /// (tipo, record, specchiato, frame) attualmente mostrato; None = da assegnare.
    shown: Option<(u8, usize, bool, usize)>,
    rng: u64,
    bounds: (Vec2, Vec2),
    samples: Arc<HeightSamples>,
    water: Arc<Vec<bool>>,
}

fn is_water(water: &[bool], p: Vec2) -> bool {
    let cx = ((p.x / CELL_SIZE) as i32).clamp(0, 127) as usize;
    let cy = ((-p.y / CELL_SIZE) as i32).clamp(0, 127) as usize;
    water.get(cy * 128 + cx).copied().unwrap_or(false)
}

pub struct TileResult {
    archive: i32,
    mesh: TerrainMeshData,
    encoded_tilemap: Vec<u8>,
    samples: Arc<HeightSamples>,
    city_parts: Option<[MeshPart; MATERIAL_COUNT]>,
    city_textured: Vec<((u16, u8), MeshPart)>,
    city_rect: Option<CityTileRect>,
    city_buildings: Vec<crate::data::city_map::MapBuilding>,
    /// Decorazioni statiche: (x, z, archive, record)
    exterior_flats: Vec<(f32, f32, u16, u8)>,
    /// NPC: (x, z, archivio persona, limiti di passeggiata)
    exterior_npcs: Vec<(f32, f32, u16, (Vec2, Vec2))>,
    /// Nemici selvatici: (x, z, indice in ENEMY_SPECIES)
    exterior_enemies: Vec<(f32, f32, usize)>,
    /// Celle d'acqua della tilemap (128×128, indice y*128+x)
    water: Vec<bool>,
    /// Per l'erba: tilemap nel formato dello shader e mappa di "verde" delle tile
    grass_tilemap: Arc<Vec<u8>>,
    grass_mask: Option<Arc<crate::grass::GrassMask>>,
}

#[derive(Resource)]
pub struct Streaming {
    pub radius: i32,
    pub tile_px: (usize, usize),
    loaded: HashMap<(i32, i32), Entity>,
    pending: HashMap<(i32, i32), Task<Option<TileResult>>>,
}

impl Streaming {
    pub fn new(radius: i32, tile_px: (usize, usize)) -> Self {
        Self { radius, tile_px, loaded: HashMap::new(), pending: HashMap::new() }
    }
    pub fn loaded_count(&self) -> usize {
        self.loaded.len()
    }
}

pub fn climate_of(data: &GameData, mx: i32, my: i32) -> i32 {
    data.climate.map_pixel_value(mx, my).map(i32::from).unwrap_or(DEFAULT_CLIMATE)
}

fn to_bevy_part(part: MeshPart) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, part.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, part.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, part.uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, part.colors);
    mesh.insert_indices(Indices::U32(part.indices));
    mesh
}

fn city_material(kind: MaterialKind) -> StandardMaterial {
    let (roughness, double_sided) = match kind {
        MaterialKind::Roof => (0.9, true),
        MaterialKind::Opening => (0.4, false),
        _ => (0.9, false),
    };
    StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: roughness,
        reflectance: 0.1,
        double_sided,
        cull_mode: if double_sided { None } else { Some(bevy::render::render_resource::Face::Back) },
        ..default()
    }
}

fn to_bevy_mesh(d: TerrainMeshData) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    let (mut min_x, mut max_x) = (f32::MAX, f32::MIN);
    let (mut min_z, mut max_z) = (f32::MAX, f32::MIN);
    for p in &d.positions {
        min_x = min_x.min(p[0]); max_x = max_x.max(p[0]);
        min_z = min_z.min(p[2]); max_z = max_z.max(p[2]);
    }
    let range_x = (max_x - min_x).max(1.0);
    let range_z = (max_z - min_z).max(1.0);
    let normalized_uvs: Vec<[f32; 2]> = d.positions.iter()
    .map(|p| [(p[0] - min_x) / range_x, (p[2] - min_z) / range_z]).collect();
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, d.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, d.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, normalized_uvs);
    mesh.insert_indices(Indices::U32(d.indices));
    mesh
}

fn build_tile(data: &GameData, tile_px: (usize, usize), mx: i32, my: i32) -> Option<TileResult> {
    let mut samples = generate_default_samples(&data.woods, mx, my).ok()?;
    let mut city_parts = None;
    let mut city_textured = Vec::new();
    let mut city_rect_opt = None;
    let mut tile_preset = None;
    let mut exterior_flats: Vec<(f32, f32, u16, u8)> = Vec::new();
    let mut exterior_npcs: Vec<(f32, f32, u16, (Vec2, Vec2))> = Vec::new();

    if let Some(loc) = data.locations.get(&(mx, my)) {
        let blocks_bsa = BsaArchive::parse(&data.blocks_buf).ok()?;
        let arch3d_bsa = BsaArchive::parse(&data.arch3d_buf).ok()?;
        let (preset, city_rect) = assemble_city_tilemap(loc, &blocks_bsa);
        city_rect_opt = Some(city_rect);
        tile_preset = Some(preset);
        flatten_terrain_heights(&mut samples.data, samples.dim, &city_rect, 12.0);

        let ground = |x: f32, z: f32| terrain_height(&samples, x / CELL_SIZE, -z / CELL_SIZE);
        let (builder, stats) = build_city_buildings(loc, &blocks_bsa, &arch3d_bsa, &ground);
        info!("{}: {} oggetti, {} edifici, {} scartati, {} modelli mancanti",
              loc.name, stats.objects, stats.built, stats.skipped, stats.missing_models);
        if !builder.is_empty() {
            let (parts, textured) = builder.into_all();
            city_parts = Some(parts);
            city_textured = textured;
        }

        let bw = loc.width.min(8);
        let bh = loc.height.min(8);
        let start_bx = (8 - bw) / 2;
        let start_by = (8 - bh) / 2;

        // --- Decorazioni stradali dai record misc dei blocchi RMB ---
        for by in 0..bh {
            for bx in 0..bw {
                let Some(name) = loc.block_names.get(by * loc.width + bx) else { continue };
                let Some(raw) = blocks_bsa.get_by_name(name) else { continue };
                let Ok(rmb) = RmbBlock::parse(raw) else { continue };

                let bx0 = (start_bx + bx) as f32 * BLOCK_SIZE_M;
                let bz0 = (start_by + by) as f32 * BLOCK_SIZE_M;

                for flat in &rmb.flats {
                    if flat.block.is_some() { continue; }
                    if flat.texture_archive == 199 { continue; }
                    let fx = bx0 + flat.x as f32 * DF_UNIT;
                    let fz = -(bz0 + (flat.z as f32 + 4096.0) * DF_UNIT);
                    exterior_flats.push((fx, fz, flat.texture_archive, flat.texture_record));
                }
            }
        }

        // --- NPC procedurali: FREQUENZA MOLTO ALTA ---
        if let Some(rect) = city_rect_opt.as_ref() {
            let bounds = (
                Vec2::new(rect.min_x as f32 * CELL_SIZE, -((rect.max_y as f32 + 1.0) * CELL_SIZE)),
                          Vec2::new((rect.max_x as f32 + 1.0) * CELL_SIZE, -(rect.min_y as f32 * CELL_SIZE)),
            );
            let mut seed: u64 = ((mx as u64) << 32) ^ (my as u64) ^ 0x517C_C1B7_2722_0A95;
            if seed == 0 { seed = 0x9E37_79B9_7F4A_7C15; }

            let npc_count = 50 + bw as usize * bh as usize * 5;
            for _ in 0..npc_count {
                let x = bounds.0.x + ((seed & 0xFFFFFF) as f32 / 16_777_215.0) * (bounds.1.x - bounds.0.x);
                seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17;
                let z = bounds.0.y + ((seed & 0xFFFFFF) as f32 / 16_777_215.0) * (bounds.1.y - bounds.0.y);
                seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17;

                let archive = PERSON_ARCHIVES[((seed & 0xFFFFFF) as usize) % PERSON_ARCHIVES.len()];
                seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17;

                exterior_npcs.push((x, z, archive, bounds));
            }
            info!("  {} decorazioni + {} NPC in {}", exterior_flats.len(), exterior_npcs.len(), loc.name);
        }

        let map = crate::data::city_map::collect_city_map(loc, &blocks_bsa, &arch3d_bsa);
        let city_buildings = map.buildings;
        // keep the value for returning below - if any later code needs it, it is stored here
        let _ = city_buildings;
    }

    let tilemap = generate_default_tilemap(&samples, mx, my, tile_preset.as_deref()).ok()?;
    let encoded = encode_for_shader(&tilemap, true);
    let mesh = build_terrain_mesh(&samples, &encoded, tile_px);
    let archive = ground_archive(climate_of(data, mx, my), data.winter);
    let grass_mask = crate::grass::grass_mask(data, archive);
    let grass_tilemap = Arc::new(encoded.clone());

    // --- Nemici selvatici: celle di terra, fuori dalla città ---
    let water: Vec<bool> = tilemap.iter().map(|&t| t == 0xFF || (t & 0x3F) == 0).collect();
    let mut exterior_enemies: Vec<(f32, f32, usize)> = Vec::new();
    {
        let table = enemy_table(world_climate_settings(climate_of(data, mx, my)).climate_type);
        let mut seed: u64 = ((mx as u64) << 32) ^ ((my as u64) & 0xFFFF_FFFF) ^ 0xA3B1_95D7_4F6E_2C81;
        for _ in 0..ENEMIES_PER_TILE {
            for _attempt in 0..16 {
                let cx = (next_rand(&mut seed) * 120.0) as usize + 4;
                let cy = (next_rand(&mut seed) * 120.0) as usize + 4;
                let species = table[(next_rand(&mut seed) * table.len() as f32) as usize % table.len()];
                if water[cy * 128 + cx] { continue; }
                // lontano dalla città (margine di 6 celle)
                let near_city = city_rect_opt.as_ref().is_some_and(|r| {
                    cx + 6 >= r.min_x && cx <= r.max_x + 6 && cy + 6 >= r.min_y && cy <= r.max_y + 6
                });
                if near_city { continue; }
                exterior_enemies.push(((cx as f32 + 0.5) * CELL_SIZE, -((cy as f32 + 0.5) * CELL_SIZE), species));
                break;
            }
        }
    }

    let city_buildings = if let Some(loc) = data.locations.get(&(mx, my)) {
        let blocks_bsa = BsaArchive::parse(&data.blocks_buf).ok()?;
        let arch3d_bsa = BsaArchive::parse(&data.arch3d_buf).ok()?;
        let map = crate::data::city_map::collect_city_map(loc, &blocks_bsa, &arch3d_bsa);
        map.buildings
    } else {
        Vec::new()
    };

    Some(TileResult { archive, mesh, encoded_tilemap: encoded, samples: Arc::new(samples),
        city_parts, city_textured, city_rect: city_rect_opt, city_buildings, exterior_flats, exterior_npcs,
        exterior_enemies, water, grass_tilemap, grass_mask })
}

pub fn rebase_origin(
    mut origin: ResMut<Origin>,
    mut player: Query<&mut Transform, (With<Player>, Without<TerrainTile>)>,
                     mut tiles: Query<(&TerrainTile, &mut Transform), Without<Player>>,
) {
    let Ok(mut pt) = player.get_single_mut() else { return };
    let (east, north) = absolute(&origin, pt.translation);
    let side = crate::data::terrain_mesh::MAP_PIXEL_SIZE as f64;
    let (pe, pn) = ((east / side).floor() as i32, (north / side).floor() as i32);
    if (pe - origin.east).abs() < 2 && (pn - origin.north).abs() < 2 { return; }
    let (de, dn) = (pe - origin.east, pn - origin.north);
    origin.east = pe; origin.north = pn;
    pt.translation.x -= de as f32 * crate::data::terrain_mesh::MAP_PIXEL_SIZE;
    pt.translation.z += dn as f32 * crate::data::terrain_mesh::MAP_PIXEL_SIZE;
    for (tile, mut tf) in &mut tiles { tf.translation = tile_translation(&origin, tile.mx, tile.my); }
}

pub fn stream_tiles(
    mut commands: Commands, mut streaming: ResMut<Streaming>, mut field: ResMut<HeightField>,
    mut colliders: ResMut<BuildingColliders>, data: Res<GameData>, origin: Res<Origin>,
    player: Query<&Transform, With<Player>>,
) {
    let Ok(tf) = player.get_single() else { return };
    let (pmx, pmy) = map_pixel_of(&origin, tf.translation);
    let r = streaming.radius;
    let keep = r + 1;
    let near = |(mx, my): (i32, i32), d: i32| (mx - pmx).abs() <= d && (my - pmy).abs() <= d;

    let far: Vec<_> = streaming.loaded.keys().filter(|k| !near(**k, keep)).copied().collect();
    for key in far {
        if let Some(e) = streaming.loaded.remove(&key) { commands.entity(e).despawn_recursive(); }
        field.tiles.remove(&key);
        colliders.chunks.remove(&key);
    }
    streaming.pending.retain(|k, _| near(*k, keep));

    let mut wanted = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            let key = (pmx + dx, pmy + dy);
            if in_map(key.0, key.1) && !streaming.loaded.contains_key(&key) && !streaming.pending.contains_key(&key) {
                wanted.push(key);
            }
        }
    }
    wanted.sort_by_key(|(mx, my)| (mx - pmx).pow(2) + (my - pmy).pow(2));

    let pool = AsyncComputeTaskPool::get();
    for key in wanted {
        if streaming.pending.len() >= MAX_PENDING { break; }
        let (data, tile_px) = (data.clone(), streaming.tile_px);
        let task = pool.spawn(async move { build_tile(&data, tile_px, key.0, key.1) });
        streaming.pending.insert(key, task);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn collect_tiles(
    mut commands: Commands, mut streaming: ResMut<Streaming>, mut field: ResMut<HeightField>,
    mut colliders: ResMut<BuildingColliders>, data: Res<GameData>, origin: Res<Origin>,
    mut mods: ResMut<ModTextures>, mut nature_pool: ResMut<NaturePool>, mut meshes: ResMut<Assets<Mesh>>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>, mut standard_materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>, mut wall_textures: ResMut<WallTextures>,
    mut npc_sprites: Local<HashMap<u16, Option<Arc<NpcSpriteSet>>>>,
    mut enemy_sprites: Local<HashMap<u16, Option<Arc<EnemySpriteSet>>>>,
) {
    let finished: Vec<_> = streaming.pending.iter_mut()
    .filter_map(|(key, task)| block_on(future::poll_once(task)).map(|result| (*key, result)))
    .take(MAX_SPAWNS_PER_FRAME).collect();

    for (key, result) in finished {
        streaming.pending.remove(&key);
        let Some(tile) = result else { continue };

        let tile_array_handle = match mods.terrain_texture_array(tile.archive as u32, &mut images) {
            Some(h) => h,
            None => { warn!("Array terreno {} non trovato nelle mod", tile.archive); continue; }
        };
        let tile_water = Arc::new(tile.water.clone());
        let tilemap_image = Image::new(
            Extent3d { width: 128, height: 128, depth_or_array_layers: 1 },
            TextureDimension::D2, tile.encoded_tilemap, TextureFormat::R8Unorm, RenderAssetUsages::RENDER_WORLD,
        );
        let tilemap_handle = images.add(tilemap_image);
        let material = terrain_materials.add(ExtendedMaterial {
            base: StandardMaterial { perceptual_roughness: 1.0, reflectance: 0.1, ..default() },
                                             extension: TerrainExtension { tile_array: tile_array_handle, tilemap: tilemap_handle },
        });

        let chunk_entity = commands.spawn((
            Mesh3d(meshes.add(to_bevy_mesh(tile.mesh))), MeshMaterial3d(material),
                                           Transform::from_translation(tile_translation(&origin, key.0, key.1)),
                                           TerrainTile { mx: key.0, my: key.1 },
        )).id();

        if let Some(mask) = tile.grass_mask.clone() {
            commands.entity(chunk_entity).insert(crate::grass::GrassSource::new(
                key.0, key.1, Arc::clone(&tile.samples),
                Arc::clone(&tile.grass_tilemap), mask, tile.city_rect,
            ));
        }

        if let Some(parts) = tile.city_parts {
            commands.entity(chunk_entity).with_children(|parent| {
                for (kind, part) in MaterialKind::ALL.iter().zip(parts) {
                    if part.is_empty() { continue; }
                    let mat = standard_materials.add(city_material(*kind));
                    parent.spawn((Mesh3d(meshes.add(to_bevy_part(part))), MeshMaterial3d(mat), Transform::IDENTITY));
                }
            });
        }

        let textured: Vec<_> = tile.city_textured.into_iter().map(|((archive, record), part)| {
            let mat = wall_textures.material(archive, record, climate_of(&data, key.0, key.1),
                                             &data, &mut mods, &mut images, &mut standard_materials);
            (mat, part)
        }).collect();
        if !textured.is_empty() {
            commands.entity(chunk_entity).with_children(|parent| {
                for (mat, part) in textured {
                    parent.spawn((Mesh3d(meshes.add(to_bevy_part(part))), MeshMaterial3d(mat), Transform::IDENTITY));
                }
            });
        }

        // --- Decorazioni stradali (billboard statici) ---
        for (fx, fz, archive, record) in &tile.exterior_flats {
            let Some((mat, w_px, h_px)) = wall_textures.flat_material(*archive, *record, &data, &mut images, &mut standard_materials) else { continue };
            let (w, h) = (w_px * DF_UNIT, h_px * DF_UNIT);
            let gx = (fx / CELL_SIZE).clamp(0.0, 128.0);
            let gy = (-fz / CELL_SIZE).clamp(0.0, 128.0);
            let y = terrain_height(&tile.samples, gx, gy);
            commands.entity(chunk_entity).with_children(|parent| {
                parent.spawn((
                    Mesh3d(meshes.add(Rectangle::new(w, h))), MeshMaterial3d(mat),
                              Transform::from_xyz(*fx, y + h * 0.5, *fz),
                              crate::interior::FlatBillboard,
                ));
            });
        }

        // --- NPC animati (passanti DFU: un archivio = una persona) ---
        let mut spawned = 0usize;
        let npc_quad = meshes.add(Rectangle::new(1.0, 1.0));
        for (fx, fz, archive, bounds) in &tile.exterior_npcs {
            let sprites = npc_sprites.entry(*archive)
                .or_insert_with(|| build_npc_sprites(*archive, &data, &mut images, &mut standard_materials).map(Arc::new))
                .clone();
            let Some(sprites) = sprites else { continue };

            let mut rng = ((fx.to_bits() as u64) << 32) ^ (fz.to_bits() as u64) ^ 0x9E37_79B9_7F4A_7C15;
            let mut heading = CARDINALS[(next_rand(&mut rng) * 4.0) as usize % 4];
            let pos = Vec2::new(*fx, *fz);
            let target = pick_target(pos, &mut heading, *bounds, &mut rng, true);
            let frame_timer = next_rand(&mut rng) * NPC_FRAME_SECONDS;
            let frame = (next_rand(&mut rng) * sprites.walk[0].len() as f32) as usize;

            let size = sprites.walk_size[0];
            let gx = (fx / CELL_SIZE).clamp(0.0, 128.0);
            let gy = (-fz / CELL_SIZE).clamp(0.0, 128.0);
            let y = terrain_height(&tile.samples, gx, gy);

            let walker = NpcWalker {
                sprites: Arc::clone(&sprites), heading, target, frame, frame_timer,
                shown: None, rng, bounds: *bounds, samples: Arc::clone(&tile.samples),
            };

            commands.entity(chunk_entity).with_children(|parent| {
                parent.spawn((
                    Mesh3d(npc_quad.clone()),
                    MeshMaterial3d(sprites.walk[0][0].clone()),
                    Transform::from_xyz(*fx, y + size.y * 0.5, *fz).with_scale(size.extend(1.0)),
                    crate::interior::FlatBillboard,
                    walker,
                ));
            });
            spawned += 1;
        }
        if spawned > 0 { debug!("  {} NPC animati nel tile ({},{})", spawned, key.0, key.1); }

        // --- Nemici selvatici ---
        let mut enemies_spawned = 0usize;
        for (fx, fz, species) in &tile.exterior_enemies {
            let sp = &ENEMY_SPECIES[*species];
            let sprites = enemy_sprites.entry(sp.archive)
                .or_insert_with(|| build_enemy_sprites(sp.archive, sp.has_idle, &data, &mut images, &mut standard_materials).map(Arc::new))
                .clone();
            let Some(sprites) = sprites else { continue };

            let mut rng = ((fx.to_bits() as u64) << 32) ^ (fz.to_bits() as u64) ^ 0x5DEE_CE66_D1CE_4E5B;
            let angle = next_rand(&mut rng) * std::f32::consts::TAU;
            let heading = Vec2::new(angle.sin(), angle.cos());
            let timer = 1.0 + next_rand(&mut rng) * 4.0;
            let frame = (next_rand(&mut rng) * sprites.walk.0[0].len() as f32) as usize;

            let size = sprites.walk.1[0];
            let gx = (fx / CELL_SIZE).clamp(0.0, 128.0);
            let gy = (-fz / CELL_SIZE).clamp(0.0, 128.0);
            let y = terrain_height(&tile.samples, gx, gy) + if sp.flying { ENEMY_HOVER } else { 0.0 };

            let enemy = Enemy {
                sprites: Arc::clone(&sprites), species: *species, mode: EnemyMode::Idle,
                heading, timer, frame, frame_timer: 0.0, kind: 0, shown: None, rng,
                bounds: (Vec2::new(2.0 * CELL_SIZE, -126.0 * CELL_SIZE), Vec2::new(126.0 * CELL_SIZE, -2.0 * CELL_SIZE)),
                samples: Arc::clone(&tile.samples), water: Arc::clone(&tile_water),
            };

            commands.entity(chunk_entity).with_children(|parent| {
                parent.spawn((
                    Mesh3d(npc_quad.clone()),
                    MeshMaterial3d(sprites.walk.0[0][0].clone()),
                    Transform::from_xyz(*fx, y + size.y * 0.5, *fz).with_scale(size.extend(1.0)),
                    enemy,
                ));
            });
            enemies_spawned += 1;
        }
        if enemies_spawned > 0 { debug!("  {} nemici nel tile ({},{})", enemies_spawned, key.0, key.1); }

        spawn_trees_for_chunk(&mut commands, chunk_entity, key.0, key.1, climate_of(&data, key.0, key.1),
                              data.winter, &tile.samples, &data, &mut mods, &mut nature_pool,
                              &mut images, &mut meshes, &mut standard_materials, tile.city_rect);

        colliders.chunks.insert(key, tile.city_buildings);
        streaming.loaded.insert(key, chunk_entity);
        field.tiles.insert(key, tile.samples);
    }
}

/// Indice di orientamento (0-7) della persona rispetto alla camera, come DFU
/// `UpdateOrientation`: 0 = cammina verso la camera, 4 = di spalle, 1-3 = verso la
/// sinistra dello schermo, 5-7 = verso destra (record specchiati).
fn orientation_index(heading: Vec2, to_camera: Vec2) -> usize {
    let dir = to_camera.normalize_or_zero();
    if dir == Vec2::ZERO { return 0; }
    let right = Vec2::new(dir.y, -dir.x); // destra della camera (guarda lungo -dir)
    let angle = (-heading.dot(right)).atan2(heading.dot(dir));
    ((angle / std::f32::consts::FRAC_PI_4).round() as i32).rem_euclid(8) as usize
}

pub fn update_npcs(
    time: Res<Time>,
    player: Query<&Transform, (With<Player>, Without<NpcWalker>)>,
    mut npcs: Query<(&mut NpcWalker, &mut Transform, &GlobalTransform, &mut MeshMaterial3d<StandardMaterial>)>,
    mut last_player_pos: Local<Option<Vec3>>,
) {
    let real_dt = time.delta_secs();
    let dt = real_dt.min(0.1);
    let Ok(player_tf) = player.get_single() else { return };
    let player_pos = player_tf.translation;
    let player_still = last_player_pos.is_some_and(|p| p.distance(player_pos) < 0.1 * real_dt.max(1e-4));
    *last_player_pos = Some(player_pos);

    let cam_forward = player_tf.forward();
    let cam_forward_xz = Vec2::new(cam_forward.x, cam_forward.z).normalize_or_zero();
    let yaw = (cam_forward_xz != Vec2::ZERO).then(|| (-cam_forward_xz.x).atan2(-cam_forward_xz.y));

    for (mut npc, mut tf, gtf, mut material) in &mut npcs {
        let npc = &mut *npc;
        // Posizioni in spazio mondo per distanza/orientamento (il Transform dell'NPC è locale al tile).
        let world = gtf.translation();
        let to_player = Vec2::new(player_pos.x - world.x, player_pos.z - world.z);
        let idle = player_still && to_player.length() < NPC_IDLE_DISTANCE;

        if !idle {
            // --- Movimento verso il bersaglio sulla griglia cardinale ---
            let pos = tf.translation.xz();
            let to_target = npc.target - pos;
            let step = NPC_SPEED * dt;
            let new_pos = if to_target.length() > step {
                pos + to_target.normalize() * step
            } else {
                npc.target
            };
            tf.translation.x = new_pos.x;
            tf.translation.z = new_pos.y;
            if (npc.target - new_pos).length() < 0.1 {
                npc.target = pick_target(new_pos, &mut npc.heading, npc.bounds, &mut npc.rng, false);
            }

            // --- Avanzamento frame ---
            npc.frame_timer += dt;
            if npc.frame_timer >= NPC_FRAME_SECONDS {
                npc.frame_timer -= NPC_FRAME_SECONDS;
                npc.frame = (npc.frame + 1) % npc.sprites.walk[0].len();
            }
        }

        // --- Scelta dello sprite ---
        let (record, flip, frame, handle, size) = if idle {
            (5, false, 0, &npc.sprites.idle[0], npc.sprites.idle_size)
        } else {
            let (record, flip) = ORIENTATIONS[orientation_index(npc.heading, to_player)];
            let frames = &npc.sprites.walk[record];
            let frame = npc.frame % frames.len();
            (record, flip, frame, &frames[frame], npc.sprites.walk_size[record])
        };
        if npc.shown != Some((record, flip, frame)) {
            *material = MeshMaterial3d(handle.clone());
            npc.shown = Some((record, flip, frame));
        }

        // --- Dimensione per record, specchio e altezza sul terreno ---
        tf.scale = Vec3::new(if flip { -size.x } else { size.x }, size.y, 1.0);
        let gx = (tf.translation.x / CELL_SIZE).clamp(0.0, 128.0);
        let gy = (-tf.translation.z / CELL_SIZE).clamp(0.0, 128.0);
        tf.translation.y = terrain_height(&npc.samples, gx, gy) + size.y * 0.5;

        // --- Billboard: il piano guarda la camera (come DFU, lungo la vista) ---
        if let Some(yaw) = yaw {
            tf.rotation = Quat::from_rotation_y(yaw);
        }
    }
}

pub fn update_enemies(
    time: Res<Time>,
    player: Query<&Transform, (With<Player>, Without<Enemy>)>,
    mut enemies: Query<(&mut Enemy, &mut Transform, &GlobalTransform, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    let dt = time.delta_secs().min(0.1);
    let Ok(player_tf) = player.get_single() else { return };
    let player_pos = player_tf.translation;

    let cam_forward = player_tf.forward();
    let cam_forward_xz = Vec2::new(cam_forward.x, cam_forward.z).normalize_or_zero();
    let yaw = (cam_forward_xz != Vec2::ZERO).then(|| (-cam_forward_xz.x).atan2(-cam_forward_xz.y));

    for (mut enemy, mut tf, gtf, mut material) in &mut enemies {
        let e = &mut *enemy;
        let sp = &ENEMY_SPECIES[e.species];

        // Posizioni in spazio mondo (il Transform dell'entità è locale al tile).
        let world = gtf.translation();
        let to_player = Vec2::new(player_pos.x - world.x, player_pos.z - world.z);
        let dist = to_player.length();

        // --- Transizioni di stato ---
        let engaged = matches!(e.mode, EnemyMode::Chase | EnemyMode::Attack);
        let aware = dist < ENEMY_NOTICE_DISTANCE || (engaged && dist < ENEMY_LOSE_DISTANCE);
        let new_mode = if aware {
            if dist <= ENEMY_MELEE_RANGE { EnemyMode::Attack } else { EnemyMode::Chase }
        } else if engaged {
            EnemyMode::Idle
        } else {
            e.mode
        };
        if new_mode != e.mode {
            if !aware { e.timer = 1.0 + next_rand(&mut e.rng) * 2.0; }
            e.mode = new_mode;
        }

        match e.mode {
            EnemyMode::Chase | EnemyMode::Attack => {
                if dist > 0.01 { e.heading = to_player / dist; }
                // TODO danno: quando in Attack applicare il danno al giocatore
                // (nel progetto non esiste ancora un sistema di salute).
            }
            EnemyMode::Idle | EnemyMode::Walk => {
                e.timer -= dt;
                if e.timer <= 0.0 {
                    if e.mode == EnemyMode::Idle {
                        let angle = next_rand(&mut e.rng) * std::f32::consts::TAU;
                        e.heading = Vec2::new(angle.sin(), angle.cos());
                        e.mode = EnemyMode::Walk;
                        e.timer = 2.0 + next_rand(&mut e.rng) * 3.0;
                    } else {
                        e.mode = EnemyMode::Idle;
                        e.timer = 2.0 + next_rand(&mut e.rng) * 4.0;
                    }
                }
            }
        }

        // --- Movimento (senza uscire dal tile né entrare in acqua) ---
        let speed = match e.mode {
            EnemyMode::Chase => sp.chase_speed,
            EnemyMode::Walk => sp.wander_speed,
            _ => 0.0,
        };
        if speed > 0.0 {
            let pos = tf.translation.xz();
            let next = pos + e.heading * speed * dt;
            let (min, max) = e.bounds;
            let inside = next.x >= min.x && next.x <= max.x && next.y >= min.y && next.y <= max.y;
            if inside && !is_water(&e.water, next) {
                tf.translation.x = next.x;
                tf.translation.z = next.y;
            } else if e.mode == EnemyMode::Walk {
                // bloccato: si volta verso il centro del tile
                e.heading = ((min + max) * 0.5 - pos).normalize_or_zero();
                e.timer = e.timer.min(1.0);
            }
        }

        // --- Animazione ---
        let has_idle_anim = e.sprites.idle.is_some();
        let (kind, fps) = match e.mode {
            EnemyMode::Attack => (1u8, ENEMY_ATTACK_FPS),
            EnemyMode::Chase | EnemyMode::Walk => (0u8, if sp.flying { ENEMY_FLY_FPS } else { ENEMY_MOVE_FPS }),
            EnemyMode::Idle if has_idle_anim => (2u8, ENEMY_IDLE_FPS),
            // senza animazione idle DFU riusa quella di movimento, ferma sul primo frame
            EnemyMode::Idle => (0u8, ENEMY_MOVE_FPS),
        };
        let hold = e.mode == EnemyMode::Idle && !has_idle_anim;
        if kind != e.kind {
            e.kind = kind;
            e.frame = 0;
            e.frame_timer = 0.0;
        }
        if !hold {
            e.frame_timer += dt;
            let period = 1.0 / fps;
            while e.frame_timer >= period {
                e.frame_timer -= period;
                e.frame = (e.frame + 1) % 4096;
            }
        }

        // --- Scelta dello sprite ---
        let (offset, flip) = ORIENTATIONS[orientation_index(e.heading, to_player)];
        let set: &RecordSet = match kind {
            1 => &e.sprites.attack,
            2 => e.sprites.idle.as_ref().unwrap_or(&e.sprites.walk),
            _ => &e.sprites.walk,
        };
        let frames = &set.0[offset];
        let frame = if hold { 0 } else { e.frame % frames.len() };
        let size = set.1[offset];
        if e.shown != Some((kind, offset, flip, frame)) {
            *material = MeshMaterial3d(frames[frame].clone());
            e.shown = Some((kind, offset, flip, frame));
        }

        // --- Dimensione per record, specchio e altezza sul terreno ---
        tf.scale = Vec3::new(if flip { -size.x } else { size.x }, size.y, 1.0);
        let gx = (tf.translation.x / CELL_SIZE).clamp(0.0, 128.0);
        let gy = (-tf.translation.z / CELL_SIZE).clamp(0.0, 128.0);
        let hover = if sp.flying { ENEMY_HOVER } else { 0.0 };
        tf.translation.y = terrain_height(&e.samples, gx, gy) + size.y * 0.5 + hover;

        // --- Billboard verso la camera ---
        if let Some(yaw) = yaw {
            tf.rotation = Quat::from_rotation_y(yaw);
        }
    }
}

























