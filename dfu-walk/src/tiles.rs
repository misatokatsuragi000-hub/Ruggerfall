//! Caricamento e streaming dei map pixel: terreno, città, vegetazione,
//! flat esterni (decorazioni) e NPC animati con supporto texture HD (.dfmod).

use crate::mod_textures::ModTextures;
use crate::player::Player;
use crate::terrain_material::{TerrainExtension, TerrainMaterial};
use crate::trees::{spawn_trees_for_chunk, NaturePool};
use crate::wall_textures::WallTextures;
use crate::world::*;
use bevy::math::Affine2;
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
use dfmod::{MapKind, TextureKey};
use std::collections::HashMap;
use std::sync::Arc;

const MAX_PENDING: usize = 4;
const MAX_SPAWNS_PER_FRAME: usize = 2;
const DEFAULT_CLIMATE: i32 = 231;

/// Flat che in Daggerfall originale sono alberi ma il RMB decodifica come
/// "lampioni": li sostituiamo con cross-billboard 3D dell'archivio natura.
const FLAT_IDS_AS_TREES: &[(u16, u8)] = &[(210, 28)];

const PERSON_ARCHIVES: [u16; 24] = [
    381, 382, 383, 384,
385, 386, 391, 394,
387, 388, 389, 390,
392, 393, 451, 452,
395, 396, 397, 398,
453, 454, 455, 456,
];

const PERSON_FACE_BASE: [(u16, usize); 24] = [
    (381, 336), (382, 312), (383, 336), (384, 312),
    (385, 192), (386, 216), (391, 240), (394, 240),
    (387, 240), (388, 264), (389, 264), (390, 216),
    (392, 72), (393, 0), (451, 48), (452, 72),
    (395, 144), (396, 144), (397, 120), (398, 96),
    (453, 72), (454, 72), (455, 24), (456, 72),
];
const FACE_VARIANTS: usize = 24;

fn person_face_record(archive: u16, variant: usize) -> Option<usize> {
    PERSON_FACE_BASE
    .iter()
    .find(|(a, _)| *a == archive)
    .map(|(_, base)| base + variant % FACE_VARIANTS)
}

pub(crate) const ORIENTATIONS: [(usize, bool); 8] = [
    (0, false), (1, false), (2, false), (3, false),
    (4, false), (3, true), (2, true), (1, true),
];
const IDLE_RECORD: u8 = 5;
const NPC_MAX_FRAMES: usize = 32;

const NPC_FRAME_SECONDS: f32 = 0.25;
const NPC_SPEED: f32 = 1.3;
const NPC_STEP: f32 = 4.0;
const NPC_IDLE_DISTANCE: f32 = 2.5;
const NPC_RANDOM_TURN_CHANCE: f32 = 0.025;
const NPC_RADIUS: f32 = 0.35;

const CARDINALS: [Vec2; 4] = [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::NEG_Y];

pub struct NpcSpriteSet {
    walk: [Vec<Handle<StandardMaterial>>; 5],
    walk_size: [Vec2; 5],
    idle: Vec<Handle<StandardMaterial>>,
    idle_size: Vec2,
    portrait: Handle<Image>,
    portrait_size: Vec2,
}

fn load_record(
    archive: u16,
    file: &TextureFile,
    record: usize,
    data: &GameData,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<(Vec<Handle<StandardMaterial>>, Vec2)> {
    let rec = file.record(record)?;
    let frame_count = file.frame_count(record)?.min(NPC_MAX_FRAMES);
    let (sx, sy) = (rec.scale_x as f32 / 256.0, rec.scale_y as f32 / 256.0);
    let mut handles = Vec::new();
    let mut size = Vec2::ONE;

    for frame in 0..frame_count {
        let Ok(img) = file.frame(record, frame) else { break };
        if img.width == 0 || img.height == 0 {
            break;
        }
        if frame == 0 {
            size = Vec2::new(
                img.width as f32 * (1.0 + sx),
                             img.height as f32 * (1.0 + sy),
            ) * DF_UNIT;
        }

        let key = TextureKey {
            archive: archive as u32,
            record: record as u32,
            frame: frame as u32,
            kind: MapKind::Albedo,
        };

        let (image_handle, is_hd) = if let Some(hd_handle) = mods.image_by_key(&key, images) {
            (hd_handle, true)
        } else {
            let mut image = Image::new(
                Extent3d {
                    width: img.width as u32,
                    height: img.height as u32,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                img.to_rgba(&data.palette, Some(0)),
                                       TextureFormat::Rgba8UnormSrgb,
                                       RenderAssetUsages::default(),
            );
            image.sampler = bevy::image::ImageSampler::nearest();
            (images.add(image), false)
        };

        let uv_transform = if is_hd && mods.needs_v_flip(&image_handle) {
            Affine2::from_scale_angle_translation(
                Vec2::new(1.0, -1.0),
                                                  0.0,
                                                  Vec2::new(0.0, 1.0),
            )
        } else {
            Affine2::IDENTITY
        };

        handles.push(materials.add(StandardMaterial {
            base_color_texture: Some(image_handle),
                                   alpha_mode: AlphaMode::Mask(0.5),
                                   cull_mode: None,
                                   double_sided: true,
                                   perceptual_roughness: 0.9,
                                   reflectance: 0.1,
                                   uv_transform,
                                   ..default()
        }));
    }
    (!handles.is_empty()).then_some((handles, size))
}

fn build_npc_sprites(
    archive: u16,
    data: &GameData,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<NpcSpriteSet> {
    let path = find_file_ci(&data.dir, &archive_file_name(archive as u32))?;
    let file = TextureFile::open(&path).ok()?;
    let mut walk: [Vec<Handle<StandardMaterial>>; 5] = Default::default();
    let mut walk_size = [Vec2::ONE; 5];
    for rec in 0..5 {
        let (frames, size) = load_record(archive, &file, rec, data, mods, images, materials)?;
        walk[rec] = frames;
        walk_size[rec] = size;
    }
    let (idle, idle_size) =
    load_record(archive, &file, IDLE_RECORD as usize, data, mods, images, materials)
    .unwrap_or_else(|| (vec![walk[0][0].clone()], walk_size[0]));
    let portrait = materials
    .get(&walk[0][0])
    .and_then(|m| m.base_color_texture.clone())?;
    let portrait_size = mods
    .image_size(&portrait)
    .or_else(|| images.get(&portrait).map(|i| i.size_f32()))
    .unwrap_or(Vec2::new(64.0, 64.0));
    Some(NpcSpriteSet { walk, walk_size, idle, idle_size, portrait, portrait_size })
}

pub(crate) fn next_rand(state: &mut u64) -> f32 {
    if *state == 0 {
        *state = 0x9E37_79B9_7F4A_7C15;
    }
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    ((*state >> 11) & 0xFF_FFFF) as f32 / 16_777_215.0
}

fn pick_target(
    pos: Vec2,
    heading: &mut Vec2,
    bounds: (Vec2, Vec2),
               rng: &mut u64,
               force_new: bool,
                   blocked: &dyn Fn(Vec2) -> bool,
) -> Vec2 {
    let inside = |p: Vec2| {
        p.x >= bounds.0.x && p.x <= bounds.1.x && p.y >= bounds.0.y && p.y <= bounds.1.y
    };
    let free = |p: Vec2| inside(p) && !blocked(p) && !blocked((pos + p) * 0.5);
    let keep = !force_new
    && free(pos + *heading * NPC_STEP)
    && next_rand(rng) >= NPC_RANDOM_TURN_CHANCE;
    if !keep {
        let first = (next_rand(rng) * 4.0) as usize % 4;
        for k in 0..4 {
            let d = CARDINALS[(first + k) % 4];
            if free(pos + d * NPC_STEP) {
                *heading = d;
                break;
            }
        }
    }
    let target = pos + *heading * NPC_STEP;
    if free(target) { target } else { pos }
}

fn npc_blocked(colliders: Option<&Colliders>, samples: &HeightSamples, p: Vec2) -> bool {
    colliders.is_some_and(|c| {
        let ground = terrain_height(samples, p.x / CELL_SIZE, -p.y / CELL_SIZE);
        c.hits(p, ground, NPC_RADIUS + 0.2)
    })
}

// ---------------------------------------------------------------------------
// Collisioni statiche
// ---------------------------------------------------------------------------

const COLLISION_GRID: usize = 64;
const COLLIDER_MARGIN: f32 = 1.0;
const STEP_HEIGHT: f32 = 0.6;
const BODY_HEIGHT: f32 = 1.8;
const MIN_WALL_LEN: f32 = 0.05;

struct WallSegment {
    a: Vec2,
    b: Vec2,
    y_min: f32,
    y_max: f32,
}

struct SurfaceTri {
    v: [Vec3; 3],
}

impl SurfaceTri {
    fn height_at(&self, p: Vec2) -> Option<f32> {
        let [a, b, c] = self.v;
        let d = (b.z - c.z) * (a.x - c.x) + (c.x - b.x) * (a.z - c.z);
        if d.abs() < 1e-8 {
            return None;
        }
        let w0 = ((b.z - c.z) * (p.x - c.x) + (c.x - b.x) * (p.y - c.z)) / d;
        let w1 = ((c.z - a.z) * (p.x - c.x) + (a.x - c.x) * (p.y - c.z)) / d;
        let w2 = 1.0 - w0 - w1;
        const EPS: f32 = -1e-4;
        (w0 >= EPS && w1 >= EPS && w2 >= EPS).then(|| w0 * a.y + w1 * b.y + w2 * c.y)
    }
}

pub struct Colliders {
    segments: Vec<WallSegment>,
    grid: Vec<Vec<u32>>,
    surfaces: Vec<SurfaceTri>,
    surf_grid: Vec<Vec<u32>>,
    cell: f32,
}

#[derive(Component)]
pub struct TileCollision(pub Arc<Colliders>);

#[derive(Clone, Copy, Debug)]
pub struct WallHit {
    pub point: Vec2,
    pub normal: Vec2,
    pub distance: f32,
    pub y_min: f32,
    pub y_max: f32,
}

impl WallHit {
    pub fn ledge_height(&self, feet_y: f32) -> f32 {
        self.y_max - feet_y
    }
}

impl Colliders {
    fn from_parts<'a>(parts: impl Iterator<Item = (&'a MeshPart, bool, u8)>) -> Self {
        let mut index: HashMap<(i32, i32, i32, i32), usize> = HashMap::new();
        let mut segments: Vec<WallSegment> = Vec::new();
        let mut surfaces: Vec<SurfaceTri> = Vec::new();
        let q = |v: f32| (v * 20.0).round() as i32;

        for (part, as_walls, surface_mode) in parts {
            for tri in part.indices.chunks_exact(3) {
                let v = |i: u32| Vec3::from(part.positions[i as usize]);
                let (p0, p1, p2) = (v(tri[0]), v(tri[1]), v(tri[2]));
                let n = (p1 - p0).cross(p2 - p0);
                let len = n.length();
                if len < 1e-6 {
                    continue;
                }
                let ny = n.y / len;
                let walkable = match surface_mode {
                    1 => ny > 0.35,
                    2 => ny.abs() > 0.35,
                    _ => false,
                };
                if walkable {
                    surfaces.push(SurfaceTri { v: [p0, p1, p2] });
                    continue;
                }
                if !as_walls || ny.abs() > 0.35 {
                    continue;
                }
                let pts = [
                    Vec2::new(p0.x, p0.z),
                    Vec2::new(p1.x, p1.z),
                    Vec2::new(p2.x, p2.z),
                ];
                let mut best = (0usize, 1usize, 0.0f32);
                for (i, j) in [(0, 1), (1, 2), (0, 2)] {
                    let d = pts[i].distance_squared(pts[j]);
                    if d > best.2 {
                        best = (i, j, d);
                    }
                }
                if best.2.sqrt() < MIN_WALL_LEN {
                    continue;
                }
                let (mut a, mut b) = (pts[best.0], pts[best.1]);
                if (q(a.x), q(a.y)) > (q(b.x), q(b.y)) {
                    std::mem::swap(&mut a, &mut b);
                }
                let y_min = p0.y.min(p1.y).min(p2.y);
                let y_max = p0.y.max(p1.y).max(p2.y);
                let key = (q(a.x), q(a.y), q(b.x), q(b.y));
                match index.get(&key) {
                    Some(&i) => {
                        segments[i].y_min = segments[i].y_min.min(y_min);
                        segments[i].y_max = segments[i].y_max.max(y_max);
                    }
                    None => {
                        index.insert(key, segments.len());
                        segments.push(WallSegment { a, b, y_min, y_max });
                    }
                }
            }
        }

        let cell = 128.0 * CELL_SIZE / COLLISION_GRID as f32;
        let mut grid: Vec<Vec<u32>> = vec![Vec::new(); COLLISION_GRID * COLLISION_GRID];
        let to_cell =
        |v: f32| ((v / cell).floor() as i32).clamp(0, COLLISION_GRID as i32 - 1) as usize;
        for (i, s) in segments.iter().enumerate() {
            let (min_x, max_x) = (
                s.a.x.min(s.b.x) - COLLIDER_MARGIN,
                                  s.a.x.max(s.b.x) + COLLIDER_MARGIN,
            );
            let (min_z, max_z) = (
                -(s.a.y.max(s.b.y) + COLLIDER_MARGIN),
                                  -(s.a.y.min(s.b.y) - COLLIDER_MARGIN),
            );
            for iz in to_cell(min_z)..=to_cell(max_z) {
                for ix in to_cell(min_x)..=to_cell(max_x) {
                    grid[iz * COLLISION_GRID + ix].push(i as u32);
                }
            }
        }

        let mut surf_grid: Vec<Vec<u32>> = vec![Vec::new(); COLLISION_GRID * COLLISION_GRID];
        for (i, t) in surfaces.iter().enumerate() {
            let xs = t.v.map(|v| v.x);
            let zs = t.v.map(|v| -v.z);
            let (min_x, max_x) = (xs[0].min(xs[1]).min(xs[2]), xs[0].max(xs[1]).max(xs[2]));
            let (min_z, max_z) = (zs[0].min(zs[1]).min(zs[2]), zs[0].max(zs[1]).max(zs[2]));
            for iz in to_cell(min_z)..=to_cell(max_z) {
                for ix in to_cell(min_x)..=to_cell(max_x) {
                    surf_grid[iz * COLLISION_GRID + ix].push(i as u32);
                }
            }
        }
        Colliders { segments, grid, surfaces, surf_grid, cell }
    }

    fn cell_index(&self, p: Vec2) -> Option<usize> {
        let ix = (p.x / self.cell).floor() as i32;
        let iz = (-p.y / self.cell).floor() as i32;
        let n = COLLISION_GRID as i32;
        (ix >= 0 && iz >= 0 && ix < n && iz < n).then(|| iz as usize * COLLISION_GRID + ix as usize)
    }

    fn relevant(s: &WallSegment, feet_y: f32) -> bool {
        s.y_max > feet_y + STEP_HEIGHT && s.y_min < feet_y + BODY_HEIGHT
    }

    fn closest(s: &WallSegment, p: Vec2) -> Vec2 {
        let ab = s.b - s.a;
        let t = ((p - s.a).dot(ab) / ab.length_squared().max(1e-8)).clamp(0.0, 1.0);
        s.a + ab * t
    }

    pub fn hits(&self, p: Vec2, feet_y: f32, radius: f32) -> bool {
        let Some(ci) = self.cell_index(p) else { return false };
        self.grid[ci].iter().any(|&i| {
            let s = &self.segments[i as usize];
            Self::relevant(s, feet_y) && Self::closest(s, p).distance(p) < radius
        })
    }

    pub fn resolve(&self, mut p: Vec2, feet_y: f32, radius: f32) -> Vec2 {
        let Some(ci) = self.cell_index(p) else { return p };
        let list = &self.grid[ci];
        if list.is_empty() {
            return p;
        }
        for _ in 0..4 {
            let mut moved = false;
            for &i in list {
                let s = &self.segments[i as usize];
                if !Self::relevant(s, feet_y) {
                    continue;
                }
                let d = p - Self::closest(s, p);
                let dist = d.length();
                if dist < radius {
                    let ab = s.b - s.a;
                    let push = if dist > 1e-4 {
                        d / dist
                    } else {
                        Vec2::new(-ab.y, ab.x).normalize_or_zero()
                    };
                    p += push * (radius - dist);
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        p
    }

    pub fn probe(&self, p: Vec2, dir: Vec2, reach: f32, feet_y: f32) -> Option<WallHit> {
        let ci = self.cell_index(p)?;
        let reach = reach.min(COLLIDER_MARGIN);
        let cross = |a: Vec2, b: Vec2| a.x * b.y - a.y * b.x;
        let mut best: Option<WallHit> = None;
        for &i in &self.grid[ci] {
            let s = &self.segments[i as usize];
            if s.y_max <= feet_y || s.y_min >= feet_y + BODY_HEIGHT {
                continue;
            }
            let ab = s.b - s.a;
            let denom = cross(dir, ab);
            if denom.abs() < 1e-6 {
                continue;
            }
            let t = cross(s.a - p, ab) / denom;
            let u = cross(s.a - p, dir) / denom;
            if !(0.0..=reach).contains(&t) || !(0.0..=1.0).contains(&u) {
                continue;
            }
            if best.map_or(true, |b| t < b.distance) {
                let mut normal = Vec2::new(-ab.y, ab.x).normalize_or_zero();
                if normal.dot(dir) > 0.0 {
                    normal = -normal;
                }
                best = Some(WallHit {
                    point: p + dir * t,
                    normal,
                    distance: t,
                    y_min: s.y_min,
                    y_max: s.y_max,
                });
            }
        }
        best
    }

    pub fn surface_height(&self, p: Vec2, max_y: f32) -> Option<f32> {
        let ci = self.cell_index(p)?;
        self.surf_grid[ci]
        .iter()
        .filter_map(|&i| self.surfaces[i as usize].height_at(p))
        .filter(|&h| h <= max_y)
        .fold(None, |best: Option<f32>, h| {
            Some(best.map_or(h, |b| b.max(h)))
        })
    }

    pub fn inside_building(&self, p: Vec2, ground_y: f32) -> bool {
        let (lo, hi) = (ground_y + 0.5, ground_y + 1.5);
        let mut crossings = 0;
        for s in &self.segments {
            if s.y_max < lo || s.y_min > hi {
                continue;
            }
            if (s.a.y > p.y) != (s.b.y > p.y) {
                let x = s.a.x + (p.y - s.a.y) / (s.b.y - s.a.y) * (s.b.x - s.a.x);
                if x > p.x {
                    crossings += 1;
                }
            }
        }
        crossings % 2 == 1
    }
}

pub fn resolve_world<'a>(
    chunks: impl Iterator<Item = (&'a TileCollision, &'a Transform)>,
                         mut p: Vec2,
                         feet_y: f32,
                         radius: f32,
) -> Vec2 {
    for (collision, tf) in chunks {
        let origin = Vec2::new(tf.translation.x, tf.translation.z);
        p = collision.0.resolve(p - origin, feet_y, radius) + origin;
    }
    p
}

pub fn surface_height_world<'a>(
    chunks: impl Iterator<Item = (&'a TileCollision, &'a Transform)>,
                                p: Vec2,
                                max_y: f32,
) -> Option<f32> {
    chunks
    .filter_map(|(collision, tf)| {
        collision
        .0
        .surface_height(
            p - Vec2::new(tf.translation.x, tf.translation.z),
                        max_y - tf.translation.y,
        )
        .map(|h| h + tf.translation.y)
    })
    .fold(None, |best: Option<f32>, h| {
        Some(best.map_or(h, |b| b.max(h)))
    })
}

pub fn probe_world<'a>(
    chunks: impl Iterator<Item = (&'a TileCollision, &'a Transform)>,
                       p: Vec2,
                       dir: Vec2,
                       reach: f32,
                       feet_y: f32,
) -> Option<WallHit> {
    let mut best: Option<WallHit> = None;
    for (collision, tf) in chunks {
        let origin = Vec2::new(tf.translation.x, tf.translation.z);
        if let Some(mut hit) = collision.0.probe(p - origin, dir, reach, feet_y) {
            hit.point += origin;
            if best.map_or(true, |b| hit.distance < b.distance) {
                best = Some(hit);
            }
        }
    }
    best
}

#[derive(Component)]
pub struct NpcWalker {
    sprites: Arc<NpcSpriteSet>,
    heading: Vec2,
    target: Vec2,
    frame: usize,
    frame_timer: f32,
    shown: Option<(usize, bool, usize)>,
    rng: u64,
    bounds: (Vec2, Vec2),
    samples: Arc<HeightSamples>,
    colliders: Option<Arc<Colliders>>,
    face_record: Option<usize>,
}

impl NpcWalker {
    pub fn face_record(&self) -> Option<usize> {
        self.face_record
    }
    pub fn portrait(&self) -> (Handle<Image>, Vec2) {
        (self.sprites.portrait.clone(), self.sprites.portrait_size)
    }
}

#[derive(Component)]
pub struct TerrainTile {
    pub mx: i32,
    pub my: i32,
}

// ---------------------------------------------------------------------------
// Nemici selvatici
// ---------------------------------------------------------------------------

const ENEMIES_PER_TILE: usize = 4;
const ENEMY_NOTICE_DISTANCE: f32 = 25.0;
const ENEMY_LOSE_DISTANCE: f32 = 60.0;
const ENEMY_MELEE_RANGE: f32 = 2.0;
const ENEMY_HOVER: f32 = 1.5;

pub(crate) const ENEMY_MOVE_FPS: f32 = 6.0;
pub(crate) const ENEMY_FLY_FPS: f32 = 10.0;
pub(crate) const ENEMY_ATTACK_FPS: f32 = 10.0;
pub(crate) const ENEMY_IDLE_FPS: f32 = 4.0;

const ENEMY_ATTACK_FIRST: usize = 5;
const ENEMY_IDLE_FIRST: usize = 15;

struct EnemySpecies {
    archive: u16,
    has_idle: bool,
    flying: bool,
    wander_speed: f32,
    chase_speed: f32,
}

static ENEMY_SPECIES: [EnemySpecies; 12] = [
    EnemySpecies { archive: 257, has_idle: true, flying: false, wander_speed: 1.2, chase_speed: 3.0 },
EnemySpecies { archive: 259, has_idle: true, flying: false, wander_speed: 1.5, chase_speed: 4.5 },
EnemySpecies { archive: 260, has_idle: true, flying: false, wander_speed: 1.8, chase_speed: 5.5 },
EnemySpecies { archive: 261, has_idle: true, flying: false, wander_speed: 1.4, chase_speed: 3.5 },
EnemySpecies { archive: 262, has_idle: true, flying: false, wander_speed: 1.2, chase_speed: 3.0 },
EnemySpecies { archive: 263, has_idle: true, flying: false, wander_speed: 2.0, chase_speed: 5.5 },
EnemySpecies { archive: 265, has_idle: true, flying: false, wander_speed: 1.0, chase_speed: 2.5 },
EnemySpecies { archive: 267, has_idle: true, flying: false, wander_speed: 1.3, chase_speed: 3.2 },
EnemySpecies { archive: 268, has_idle: false, flying: true, wander_speed: 1.8, chase_speed: 4.5 },
EnemySpecies { archive: 271, has_idle: true, flying: false, wander_speed: 1.0, chase_speed: 2.5 },
EnemySpecies { archive: 275, has_idle: false, flying: false, wander_speed: 1.2, chase_speed: 3.0 },
EnemySpecies { archive: 276, has_idle: true, flying: false, wander_speed: 1.2, chase_speed: 3.0 },
];

fn enemy_table(climate: ClimateBase) -> &'static [usize] {
    match climate {
        ClimateBase::Temperate => &[1, 2, 3, 4, 7, 5, 0, 6, 9],
        ClimateBase::Mountain => &[1, 2, 4, 7, 11, 9, 8],
        ClimateBase::Desert => &[10, 3, 2, 4, 8, 5],
        ClimateBase::Swamp => &[3, 10, 0, 6, 4, 8],
    }
}

pub type RecordSet = ([Vec<Handle<StandardMaterial>>; 5], [Vec2; 5]);

fn load_record_set(
    archive: u16,
    file: &TextureFile,
    first: usize,
    data: &GameData,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<RecordSet> {
    let mut frames: [Vec<Handle<StandardMaterial>>; 5] = Default::default();
    let mut sizes = [Vec2::ONE; 5];
    for i in 0..5 {
        let (f, s) = load_record(archive, file, first + i, data, mods, images, materials)?;
        frames[i] = f;
        sizes[i] = s;
    }
    Some((frames, sizes))
}

pub struct EnemySpriteSet {
    pub walk: RecordSet,
    pub attack: RecordSet,
    pub idle: Option<RecordSet>,
}

pub fn build_enemy_sprites(
    archive: u16,
    has_idle: bool,
    data: &GameData,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<EnemySpriteSet> {
    let path = find_file_ci(&data.dir, &archive_file_name(archive as u32))?;
    let file = TextureFile::open(&path).ok()?;
    let walk = load_record_set(archive, &file, 0, data, mods, images, materials)?;
    let attack =
    load_record_set(archive, &file, ENEMY_ATTACK_FIRST, data, mods, images, materials)
    .unwrap_or_else(|| walk.clone());
    let idle = if has_idle {
        load_record_set(archive, &file, ENEMY_IDLE_FIRST, data, mods, images, materials)
    } else {
        None
    };
    Some(EnemySpriteSet { walk, attack, idle })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnemyMode {
    Idle,
    Walk,
    Chase,
    Attack,
}

#[derive(Component)]
pub struct Enemy {
    sprites: Arc<EnemySpriteSet>,
    species: usize,
    mode: EnemyMode,
    heading: Vec2,
    timer: f32,
    frame: usize,
    frame_timer: f32,
    kind: u8,
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
    exterior_flats: Vec<(f32, f32, u16, u8)>,
    exterior_npcs: Vec<(f32, f32, u16, (Vec2, Vec2))>,
    exterior_enemies: Vec<(f32, f32, usize)>,
    colliders: Option<Arc<Colliders>>,
    water: Vec<bool>,
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
    data.climate
    .map_pixel_value(mx, my)
    .map(i32::from)
    .unwrap_or(DEFAULT_CLIMATE)
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
        cull_mode: if double_sided {
            None
        } else {
            Some(bevy::render::render_resource::Face::Back)
        },
        ..default()
    }
}

fn to_bevy_mesh(d: TerrainMeshData) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    let (mut min_x, mut max_x) = (f32::MAX, f32::MIN);
    let (mut min_z, mut max_z) = (f32::MAX, f32::MIN);
    for p in &d.positions {
        min_x = min_x.min(p[0]);
        max_x = max_x.max(p[0]);
        min_z = min_z.min(p[2]);
        max_z = max_z.max(p[2]);
    }
    let range_x = (max_x - min_x).max(1.0);
    let range_z = (max_z - min_z).max(1.0);
    let normalized_uvs: Vec<[f32; 2]> = d
    .positions
    .iter()
    .map(|p| [(p[0] - min_x) / range_x, (p[2] - min_z) / range_z])
    .collect();
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
    let mut city_colliders: Option<Arc<Colliders>> = None;
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
        info!(
            "{}: {} oggetti, {} edifici, {} scartati, {} modelli mancanti",
            loc.name, stats.objects, stats.built, stats.skipped, stats.missing_models
        );
        if !builder.is_empty() {
            let (parts, textured) = builder.into_all();
            let (wall_i, roof_i, trim_i) = (
                MaterialKind::Wall.index(),
                                            MaterialKind::Roof.index(),
                                            MaterialKind::Trim.index(),
            );
            city_colliders = Some(Arc::new(Colliders::from_parts(
                parts
                .iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    if i == wall_i {
                        Some((p, true, 1u8))
                    } else if i == roof_i || i == trim_i {
                        Some((p, false, 2u8))
                    } else {
                        None
                    }
                })
                .chain(textured.iter().map(|(_, p)| (p, true, 2u8))),
            )));
            city_parts = Some(parts);
            city_textured = textured;
        }

        let bw = loc.width.min(8);
        let bh = loc.height.min(8);
        let start_bx = (8 - bw) / 2;
        let start_by = (8 - bh) / 2;

        for by in 0..bh {
            for bx in 0..bw {
                let Some(name) = loc.block_names.get(by * loc.width + bx) else { continue };
                let Some(raw) = blocks_bsa.get_by_name(name) else { continue };
                let Ok(rmb) = RmbBlock::parse(raw) else { continue };

                let bx0 = (start_bx + bx) as f32 * BLOCK_SIZE_M;
                let bz0 = (start_by + by) as f32 * BLOCK_SIZE_M;

                for flat in &rmb.flats {
                    if flat.block.is_some() {
                        continue;
                    }
                    if flat.texture_archive == 199 {
                        continue;
                    }
                    let fx = bx0 + flat.x as f32 * DF_UNIT;
                    let fz = -(bz0 + (flat.z as f32 + 4096.0) * DF_UNIT);
                    exterior_flats.push((fx, fz, flat.texture_archive, flat.texture_record));
                }
            }
        }

        if let Some(rect) = city_rect_opt.as_ref() {
            let bounds = (
                Vec2::new(
                    rect.min_x as f32 * CELL_SIZE,
                    -((rect.max_y as f32 + 1.0) * CELL_SIZE),
                ),
                Vec2::new(
                    (rect.max_x as f32 + 1.0) * CELL_SIZE,
                          -(rect.min_y as f32 * CELL_SIZE),
                ),
            );
            let mut seed: u64 = ((mx as u64) << 32) ^ (my as u64) ^ 0x517C_C1B7_2722_0A95;
            if seed == 0 {
                seed = 0x9E37_79B9_7F4A_7C15;
            }

            let npc_count = 50 + bw as usize * bh as usize * 5;
            let mut attempts = 0;
            while exterior_npcs.len() < npc_count && attempts < npc_count * 6 {
                attempts += 1;
                let x = bounds.0.x
                + ((seed & 0xFFFFFF) as f32 / 16_777_215.0) * (bounds.1.x - bounds.0.x);
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                let z = bounds.0.y
                + ((seed & 0xFFFFFF) as f32 / 16_777_215.0) * (bounds.1.y - bounds.0.y);
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;

                let archive =
                PERSON_ARCHIVES[((seed & 0xFFFFFF) as usize) % PERSON_ARCHIVES.len()];
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;

                if let Some(c) = city_colliders.as_deref() {
                    let p = Vec2::new(x, z);
                    let g = terrain_height(&samples, x / CELL_SIZE, -z / CELL_SIZE);
                    if c.inside_building(p, g) || c.hits(p, g, NPC_RADIUS + 0.5) {
                        continue;
                    }
                }
                exterior_npcs.push((x, z, archive, bounds));
            }
            info!(
                "  {} decorazioni + {} NPC in {}",
                exterior_flats.len(),
                  exterior_npcs.len(),
                  loc.name
            );
        }
    }

    let tilemap = generate_default_tilemap(&samples, mx, my, tile_preset.as_deref()).ok()?;
    let encoded = encode_for_shader(&tilemap, true);
    let mesh = build_terrain_mesh(&samples, &encoded, tile_px);
    let archive = ground_archive(climate_of(data, mx, my), data.winter);
    let grass_mask = crate::grass::grass_mask(data, archive);
    let grass_tilemap = Arc::new(encoded.clone());

    let water: Vec<bool> = tilemap.iter().map(|&t| t == 0xFF || (t & 0x3F) == 0).collect();
    let mut exterior_enemies: Vec<(f32, f32, usize)> = Vec::new();
    {
        let table = enemy_table(world_climate_settings(climate_of(data, mx, my)).climate_type);
        let mut seed: u64 =
        ((mx as u64) << 32) ^ ((my as u64) & 0xFFFF_FFFF) ^ 0xA3B1_95D7_4F6E_2C81;
        for _ in 0..ENEMIES_PER_TILE {
            for _attempt in 0..16 {
                let cx = (next_rand(&mut seed) * 120.0) as usize + 4;
                let cy = (next_rand(&mut seed) * 120.0) as usize + 4;
                let species =
                table[(next_rand(&mut seed) * table.len() as f32) as usize % table.len()];
                if water[cy * 128 + cx] {
                    continue;
                }
                let near_city = city_rect_opt.as_ref().is_some_and(|r| {
                    cx + 6 >= r.min_x && cx <= r.max_x + 6 && cy + 6 >= r.min_y && cy <= r.max_y + 6
                });
                if near_city {
                    continue;
                }
                exterior_enemies.push((
                    (cx as f32 + 0.5) * CELL_SIZE,
                                       -((cy as f32 + 0.5) * CELL_SIZE),
                                       species,
                ));
                break;
            }
        }
    }

    Some(TileResult {
        archive,
         mesh,
         encoded_tilemap: encoded,
         samples: Arc::new(samples),
         city_parts,
         city_textured,
         city_rect: city_rect_opt,
         exterior_flats,
         exterior_npcs,
         exterior_enemies,
         water,
         colliders: city_colliders,
         grass_tilemap,
         grass_mask,
    })
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
    if (pe - origin.east).abs() < 2 && (pn - origin.north).abs() < 2 {
        return;
    }
    let (de, dn) = (pe - origin.east, pn - origin.north);
    origin.east = pe;
    origin.north = pn;
    pt.translation.x -= de as f32 * crate::data::terrain_mesh::MAP_PIXEL_SIZE;
    pt.translation.z += dn as f32 * crate::data::terrain_mesh::MAP_PIXEL_SIZE;
    for (tile, mut tf) in &mut tiles {
        tf.translation = tile_translation(&origin, tile.mx, tile.my);
    }
}

pub fn stream_tiles(
    mut commands: Commands,
    mut streaming: ResMut<Streaming>,
    mut field: ResMut<HeightField>,
    data: Res<GameData>,
    origin: Res<Origin>,
    player: Query<&Transform, With<Player>>,
) {
    let Ok(tf) = player.get_single() else { return };
    let (pmx, pmy) = map_pixel_of(&origin, tf.translation);
    let r = streaming.radius;
    let keep = r + 1;
    let near = |(mx, my): (i32, i32), d: i32| (mx - pmx).abs() <= d && (my - pmy).abs() <= d;

    let far: Vec<_> = streaming
    .loaded
    .keys()
    .filter(|k| !near(**k, keep))
    .copied()
    .collect();
    for key in far {
        if let Some(e) = streaming.loaded.remove(&key) {
            commands.entity(e).despawn_recursive();
        }
        field.tiles.remove(&key);
    }
    streaming.pending.retain(|k, _| near(*k, keep));

    let mut wanted = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            let key = (pmx + dx, pmy + dy);
            if in_map(key.0, key.1)
                && !streaming.loaded.contains_key(&key)
                && !streaming.pending.contains_key(&key)
                {
                    wanted.push(key);
                }
        }
    }
    wanted.sort_by_key(|(mx, my)| (mx - pmx).pow(2) + (my - pmy).pow(2));

    let pool = AsyncComputeTaskPool::get();
    for key in wanted {
        if streaming.pending.len() >= MAX_PENDING {
            break;
        }
        let (data, tile_px) = (data.clone(), streaming.tile_px);
        let task = pool.spawn(async move { build_tile(&data, tile_px, key.0, key.1) });
        streaming.pending.insert(key, task);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn collect_tiles(
    mut commands: Commands,
    mut streaming: ResMut<Streaming>,
    mut field: ResMut<HeightField>,
    data: Res<GameData>,
    origin: Res<Origin>,
    mut mods: ResMut<ModTextures>,
    mut nature_pool: ResMut<NaturePool>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut standard_materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut wall_textures: ResMut<WallTextures>,
    mut npc_sprites: Local<HashMap<u16, Option<Arc<NpcSpriteSet>>>>,
    mut enemy_sprites: Local<HashMap<u16, Option<Arc<EnemySpriteSet>>>>,
) {
    let finished: Vec<_> = streaming
    .pending
    .iter_mut()
    .filter_map(|(key, task)| block_on(future::poll_once(task)).map(|result| (*key, result)))
    .take(MAX_SPAWNS_PER_FRAME)
    .collect();

    for (key, result) in finished {
        streaming.pending.remove(&key);
        let Some(tile) = result else { continue };

        let tile_array_handle = match mods.terrain_texture_array(tile.archive as u32, &mut images) {
            Some(h) => h,
            None => {
                warn!("Array terreno {} non trovato nelle mod", tile.archive);
                continue;
            }
        };
        let tile_water = Arc::new(tile.water.clone());
        let tilemap_image = Image::new(
            Extent3d {
                width: 128,
                height: 128,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            tile.encoded_tilemap,
            TextureFormat::R8Unorm,
            RenderAssetUsages::RENDER_WORLD,
        );
        let tilemap_handle = images.add(tilemap_image);
        let material = terrain_materials.add(ExtendedMaterial {
            base: StandardMaterial {
                perceptual_roughness: 1.0,
                reflectance: 0.1,
                ..default()
            },
            extension: TerrainExtension {
                tile_array: tile_array_handle,
                tilemap: tilemap_handle,
            },
        });

        let chunk_entity = commands
        .spawn((
            Mesh3d(meshes.add(to_bevy_mesh(tile.mesh))),
                MeshMaterial3d(material),
                Transform::from_translation(tile_translation(&origin, key.0, key.1)),
                TerrainTile { mx: key.0, my: key.1 },
        ))
        .id();

        if let Some(c) = &tile.colliders {
            commands
            .entity(chunk_entity)
            .insert(TileCollision(Arc::clone(c)));
        }

        if let Some(mask) = tile.grass_mask.clone() {
            commands.entity(chunk_entity).insert(crate::grass::GrassSource::new(
                key.0,
                key.1,
                Arc::clone(&tile.samples),
                                                                                Arc::clone(&tile.grass_tilemap),
                                                                                mask,
                                                                                tile.city_rect,
            ));
        }

        if let Some(parts) = tile.city_parts {
            commands.entity(chunk_entity).with_children(|parent| {
                for (kind, part) in MaterialKind::ALL.iter().zip(parts) {
                    if part.is_empty() {
                        continue;
                    }
                    let mat = standard_materials.add(city_material(*kind));
                    parent.spawn((
                        Mesh3d(meshes.add(to_bevy_part(part))),
                                  MeshMaterial3d(mat),
                                  Transform::IDENTITY,
                    ));
                }
            });
        }

        let textured: Vec<_> = tile
        .city_textured
        .into_iter()
        .map(|((archive, record), part)| {
            let mat = wall_textures.material(
                archive,
                record,
                climate_of(&data, key.0, key.1),
                                             &data,
                                             &mut mods,
                                             &mut images,
                                             &mut standard_materials,
            );
            let mut mesh = to_bevy_part(part);
            wall_textures.prepare_mesh(&mat, &mut mesh);
            (mat, mesh)
        })
        .collect();
        if !textured.is_empty() {
            commands.entity(chunk_entity).with_children(|parent| {
                for (mat, mesh) in textured {
                    parent.spawn((
                        Mesh3d(meshes.add(mesh)),
                                  MeshMaterial3d(mat),
                                  Transform::IDENTITY,
                    ));
                }
            });
        }

        // --- Flat esterni (decorazioni + alberi) ---
        //
        // La maggior parte dei flat sono rettangoli 2D che si girano verso la
        // camera. Ma alcuni ID, che nel DFU originale sono alberi, vengono
        // decodificati come "lampioni" e vanno sostituiti con la texture di un
        // albero dell'archivio natura per il clima corrente (cross-billboard 3D).
        let climate = climate_of(&data, key.0, key.1);
        let tree_archive = crate::trees::nature_archive_for_climate(climate, data.winter);

        nature_pool.get_or_load(
            tree_archive,
            &data,
            &mut mods,
            &mut images,
            &mut meshes,
            &mut standard_materials,
        );
        let tree_template: Option<crate::trees::TreeType> =
        (0..64u32).find_map(|r| nature_pool.tree_for_flat(tree_archive, r));

        // Niente mesh unitaria: ogni albero usa `tree.mesh`, che ha le UV già
        // coerenti con `needs_v_flip` della sua texture (fix "testa in giù").
        let mut trees_to_spawn: Vec<(Handle<Mesh>, Handle<StandardMaterial>, Vec3, Vec3)> = Vec::new();
        let mut rects_to_spawn: Vec<(Handle<Mesh>, Handle<StandardMaterial>, Vec3)> = Vec::new();
        let mut total_nature = 0usize;
        let mut total_decor = 0usize;

        for (fx, fz, archive, record) in &tile.exterior_flats {
            let gx = (fx / CELL_SIZE).clamp(0.0, 128.0);
            let gy = (-fz / CELL_SIZE).clamp(0.0, 128.0);
            let y = terrain_height(&tile.samples, gx, gy);

            let Some((mat_orig, w_px, h_px)) = wall_textures.flat_material(
                *archive,
                *record,
                &data,
                &mut mods,
                &mut images,
                &mut standard_materials,
            ) else {
                continue;
            };

            if FLAT_IDS_AS_TREES.contains(&(*archive, *record)) {
                // Questo specifico flat è in realtà un albero nel gioco originale.
                let Some(tree) = tree_template.as_ref() else {
                    continue;
                };
                // Alberi più grandi (era 3.2×6.5): 6×12 m, altezza di un albero vero.
                let (w_tree, h_tree) = (6.0_f32, 12.0_f32);
                // Scala uguale su X e Z: i due piani del cross billboard restano
                // simmetrici. Con Z=1 il piano B resterebbe 1 m → sembra piatto.
                let scale_xz = w_tree / tree.w_meters.max(1e-3);
                let scale_y = h_tree / tree.h_meters.max(1e-3);
                total_nature += 1;
                trees_to_spawn.push((
                    tree.mesh.clone(),
                                     tree.material.clone(),
                                     Vec3::new(*fx, y, *fz),
                                     Vec3::new(scale_xz, scale_y, scale_xz),
                ));
            } else {
                // Decorazione normale: rettangolo 2D con billboard.
                let (w, h) = (w_px * DF_UNIT, h_px * DF_UNIT);
                info!(
                    "FLAT pos=({:.1},{:.1}) archive={} record={} size_px={}x{}",
                      fx, fz, archive, record, w_px, h_px
                );
                let mesh = meshes.add(Rectangle::new(w, h));
                rects_to_spawn.push((mesh, mat_orig, Vec3::new(*fx, y + h * 0.5, *fz)));
                total_decor += 1;
            }
        }

        if total_nature + total_decor > 0 {
            info!(
                "Tile ({},{}): {} alberi 3D, {} decorazioni 2D",
                  key.0, key.1, total_nature, total_decor
            );
        }

        commands.entity(chunk_entity).with_children(|parent| {
            for (mesh, mat, pos, scale) in trees_to_spawn {
                parent.spawn((
                    Mesh3d(mesh),
                              MeshMaterial3d(mat),
                              Transform::from_translation(pos).with_scale(scale),
                ));
            }
            for (mesh, mat, pos) in rects_to_spawn {
                parent.spawn((
                    Mesh3d(mesh),
                              MeshMaterial3d(mat),
                              Transform::from_translation(pos),
                              crate::interior::FlatBillboard,
                ));
            }
        });
        // --- NPC animati con texture HD (.dfmod) ---
        let mut spawned = 0usize;
        let npc_quad = meshes.add(Rectangle::new(1.0, 1.0));
        for (fx, fz, archive, bounds) in &tile.exterior_npcs {
            let sprites = if let Some(cached) = npc_sprites.get(archive) {
                cached.clone()
            } else {
                let s = build_npc_sprites(
                    *archive,
                    &data,
                    &mut mods,
                    &mut images,
                    &mut standard_materials,
                )
                .map(Arc::new);
                npc_sprites.insert(*archive, s.clone());
                s
            };
            let Some(sprites) = sprites else { continue };

            let mut rng =
            ((fx.to_bits() as u64) << 32) ^ (fz.to_bits() as u64) ^ 0x9E37_79B9_7F4A_7C15;
            let mut heading = CARDINALS[(next_rand(&mut rng) * 4.0) as usize % 4];
            let pos = Vec2::new(*fx, *fz);
            let target = pick_target(
                pos,
                &mut heading,
                *bounds,
                &mut rng,
                true,
                &|p| npc_blocked(tile.colliders.as_deref(), &tile.samples, p),
            );
            let frame_timer = next_rand(&mut rng) * NPC_FRAME_SECONDS;
            let frame = (next_rand(&mut rng) * sprites.walk[0].len() as f32) as usize;

            let size = sprites.walk_size[0];
            let gx = (fx / CELL_SIZE).clamp(0.0, 128.0);
            let gy = (-fz / CELL_SIZE).clamp(0.0, 128.0);
            let y = terrain_height(&tile.samples, gx, gy);

            let mut face_rng =
            ((fx.to_bits() as u64) << 32) ^ (fz.to_bits() as u64) ^ 0xFACE_0000_0000_F00D;
            let face_variant = (next_rand(&mut face_rng) * FACE_VARIANTS as f32) as usize;
            let walker = NpcWalker {
                face_record: person_face_record(*archive, face_variant),
                sprites: Arc::clone(&sprites),
                heading,
                target,
                frame,
                frame_timer,
                shown: None,
                rng,
                bounds: *bounds,
                samples: Arc::clone(&tile.samples),
                colliders: tile.colliders.clone(),
            };

            commands.entity(chunk_entity).with_children(|parent| {
                parent.spawn((
                    Mesh3d(npc_quad.clone()),
                              MeshMaterial3d(sprites.walk[0][0].clone()),
                              Transform::from_xyz(*fx, y + size.y * 0.5, *fz)
                              .with_scale(size.extend(1.0)),
                              crate::interior::FlatBillboard,
                              walker,
                ));
            });
            spawned += 1;
        }
        if spawned > 0 {
            debug!("  {} NPC animati nel tile ({},{})", spawned, key.0, key.1);
        }

        // --- Nemici selvatici ---
        let mut enemies_spawned = 0usize;
        for (fx, fz, species) in &tile.exterior_enemies {
            let sp = &ENEMY_SPECIES[*species];
            let sprites = if let Some(cached) = enemy_sprites.get(&sp.archive) {
                cached.clone()
            } else {
                let s = build_enemy_sprites(
                    sp.archive,
                    sp.has_idle,
                    &data,
                    &mut mods,
                    &mut images,
                    &mut standard_materials,
                )
                .map(Arc::new);
                enemy_sprites.insert(sp.archive, s.clone());
                s
            };
            let Some(sprites) = sprites else { continue };

            let mut rng =
            ((fx.to_bits() as u64) << 32) ^ (fz.to_bits() as u64) ^ 0x5DEE_CE66_D1CE_4E5B;
            let angle = next_rand(&mut rng) * std::f32::consts::TAU;
            let heading = Vec2::new(angle.sin(), angle.cos());
            let timer = 1.0 + next_rand(&mut rng) * 4.0;
            let frame = (next_rand(&mut rng) * sprites.walk.0[0].len() as f32) as usize;

            let size = sprites.walk.1[0];
            let gx = (fx / CELL_SIZE).clamp(0.0, 128.0);
            let gy = (-fz / CELL_SIZE).clamp(0.0, 128.0);
            let y = terrain_height(&tile.samples, gx, gy)
            + if sp.flying { ENEMY_HOVER } else { 0.0 };

            let enemy = Enemy {
                sprites: Arc::clone(&sprites),
                species: *species,
                mode: EnemyMode::Idle,
                heading,
                timer,
                frame,
                frame_timer: 0.0,
                kind: 0,
                shown: None,
                rng,
                bounds: (
                    Vec2::new(2.0 * CELL_SIZE, -126.0 * CELL_SIZE),
                         Vec2::new(126.0 * CELL_SIZE, -2.0 * CELL_SIZE),
                ),
                samples: Arc::clone(&tile.samples),
                water: Arc::clone(&tile_water),
            };

            commands.entity(chunk_entity).with_children(|parent| {
                parent.spawn((
                    Mesh3d(npc_quad.clone()),
                              MeshMaterial3d(sprites.walk.0[0][0].clone()),
                              Transform::from_xyz(*fx, y + size.y * 0.5, *fz)
                              .with_scale(size.extend(1.0)),
                              enemy,
                ));
            });
            enemies_spawned += 1;
        }
        if enemies_spawned > 0 {
            debug!("  {} nemici nel tile ({},{})", enemies_spawned, key.0, key.1);
        }

        spawn_trees_for_chunk(
            &mut commands,
            chunk_entity,
            key.0,
            key.1,
            climate_of(&data, key.0, key.1),
                              data.winter,
                              &tile.samples,
                              &data,
                              &mut mods,
                              &mut nature_pool,
                              &mut images,
                              &mut meshes,
                              &mut standard_materials,
                              tile.city_rect,
        );

        streaming.loaded.insert(key, chunk_entity);
        field.tiles.insert(key, tile.samples);
    }
}

pub(crate) fn orientation_index(heading: Vec2, to_camera: Vec2) -> usize {
    let dir = to_camera.normalize_or_zero();
    if dir == Vec2::ZERO {
        return 0;
    }
    let right = Vec2::new(dir.y, -dir.x);
    let angle = (-heading.dot(right)).atan2(heading.dot(dir));
    ((angle / std::f32::consts::FRAC_PI_4).round() as i32).rem_euclid(8) as usize
}

pub fn update_npcs(
    time: Res<Time>,
    player: Query<&Transform, (With<Player>, Without<NpcWalker>)>,
                   mut npcs: Query<(
                       &mut NpcWalker,
                       &mut Transform,
                       &GlobalTransform,
                       &mut MeshMaterial3d<StandardMaterial>,
                   )>,
                   mut last_player_pos: Local<Option<Vec3>>,
) {
    let real_dt = time.delta_secs();
    let dt = real_dt.min(0.1);
    let Ok(player_tf) = player.get_single() else { return };
    let player_pos = player_tf.translation;
    let player_still =
    last_player_pos.is_some_and(|p| p.distance(player_pos) < 0.1 * real_dt.max(1e-4));
    *last_player_pos = Some(player_pos);

    let cam_forward = player_tf.forward();
    let cam_forward_xz = Vec2::new(cam_forward.x, cam_forward.z).normalize_or_zero();
    let yaw = (cam_forward_xz != Vec2::ZERO).then(|| (-cam_forward_xz.x).atan2(-cam_forward_xz.y));

    for (mut npc, mut tf, gtf, mut material) in &mut npcs {
        let npc = &mut *npc;
        let world = gtf.translation();
        let to_player = Vec2::new(player_pos.x - world.x, player_pos.z - world.z);
        let idle = player_still && to_player.length() < NPC_IDLE_DISTANCE;

        if !idle {
            let pos = tf.translation.xz();
            let to_target = npc.target - pos;
            let step = NPC_SPEED * dt;
            let mut new_pos = if to_target.length() > step {
                pos + to_target.normalize() * step
            } else {
                npc.target
            };

            let mut hit = false;
            if let Some(c) = npc.colliders.as_deref() {
                let feet =
                terrain_height(&npc.samples, new_pos.x / CELL_SIZE, -new_pos.y / CELL_SIZE);
                let fixed = c.resolve(new_pos, feet, NPC_RADIUS);
                hit = fixed.distance(new_pos) > 0.01;
                new_pos = fixed;
            }
            tf.translation.x = new_pos.x;
            tf.translation.z = new_pos.y;
            if hit || (npc.target - new_pos).length() < 0.1 {
                let blocked = |p: Vec2| npc_blocked(npc.colliders.as_deref(), &npc.samples, p);
                npc.target = pick_target(
                    new_pos,
                    &mut npc.heading,
                    npc.bounds,
                    &mut npc.rng,
                    hit,
                    &blocked,
                );
            }

            npc.frame_timer += dt;
            if npc.frame_timer >= NPC_FRAME_SECONDS {
                npc.frame_timer -= NPC_FRAME_SECONDS;
                npc.frame = (npc.frame + 1) % npc.sprites.walk[0].len();
            }
        }

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

        tf.scale = Vec3::new(if flip { -size.x } else { size.x }, size.y, 1.0);
        let gx = (tf.translation.x / CELL_SIZE).clamp(0.0, 128.0);
        let gy = (-tf.translation.z / CELL_SIZE).clamp(0.0, 128.0);
        tf.translation.y = terrain_height(&npc.samples, gx, gy) + size.y * 0.5;

        if let Some(yaw) = yaw {
            tf.rotation = Quat::from_rotation_y(yaw);
        }
    }
}

pub fn update_enemies(
    time: Res<Time>,
    player: Query<&Transform, (With<Player>, Without<Enemy>)>,
                      mut enemies: Query<(
                          &mut Enemy,
                          &mut Transform,
                          &GlobalTransform,
                          &mut MeshMaterial3d<StandardMaterial>,
                      )>,
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

        let world = gtf.translation();
        let to_player = Vec2::new(player_pos.x - world.x, player_pos.z - world.z);
        let dist = to_player.length();

        let engaged = matches!(e.mode, EnemyMode::Chase | EnemyMode::Attack);
        let aware = dist < ENEMY_NOTICE_DISTANCE || (engaged && dist < ENEMY_LOSE_DISTANCE);
        let new_mode = if aware {
            if dist <= ENEMY_MELEE_RANGE {
                EnemyMode::Attack
            } else {
                EnemyMode::Chase
            }
        } else if engaged {
            EnemyMode::Idle
        } else {
            e.mode
        };
        if new_mode != e.mode {
            if !aware {
                e.timer = 1.0 + next_rand(&mut e.rng) * 2.0;
            }
            e.mode = new_mode;
        }

        match e.mode {
            EnemyMode::Chase | EnemyMode::Attack => {
                if dist > 0.01 {
                    e.heading = to_player / dist;
                }
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
                e.heading = ((min + max) * 0.5 - pos).normalize_or_zero();
                e.timer = e.timer.min(1.0);
            }
        }

        let has_idle_anim = e.sprites.idle.is_some();
        let (kind, fps) = match e.mode {
            EnemyMode::Attack => (1u8, ENEMY_ATTACK_FPS),
            EnemyMode::Chase | EnemyMode::Walk => {
                (0u8, if sp.flying { ENEMY_FLY_FPS } else { ENEMY_MOVE_FPS })
            }
            EnemyMode::Idle if has_idle_anim => (2u8, ENEMY_IDLE_FPS),
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

        tf.scale = Vec3::new(if flip { -size.x } else { size.x }, size.y, 1.0);
        let gx = (tf.translation.x / CELL_SIZE).clamp(0.0, 128.0);
        let gy = (-tf.translation.z / CELL_SIZE).clamp(0.0, 128.0);
        let hover = if sp.flying { ENEMY_HOVER } else { 0.0 };
        tf.translation.y = terrain_height(&e.samples, gx, gy) + size.y * 0.5 + hover;

        if let Some(yaw) = yaw {
            tf.rotation = Quat::from_rotation_y(yaw);
        }
    }
}
