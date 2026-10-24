//! Erba procedurale: ciuffi generati solo dove il terreno e' davvero verde.
//!
//! Guarda l'immagine della tile di terreno usata (TEXTURE.302, 303, ...) e ne ricava
//! una mappa 16x16 di "verde". Un ciuffo viene piazzato solo dove quella mappa e' verde:
//!   - d'estate: erba sulle tile erbose e sulle transizioni erba/terra (solo sulla parte verde);
//!   - d'inverno: le tile con la neve non sono verdi, quindi niente erba;
//!   - deserto, spiaggia, roccia, acqua: niente erba.
//!
//! Tasto G: attiva/disattiva l'erba.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use bevy::pbr::NotShadowCaster;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::primitives::Aabb;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};

use crate::data::terrain::TILE_COUNT;
use crate::data::terrain_city::CityTileRect;
use crate::data::terrain_mesh::{terrain_height, tile_uv_transform, CELL_SIZE};
use crate::data::{archive_file_name, find_file_ci, load_tile_array, HeightSamples, TextureFile};
use crate::world::GameData;

// ---------------------------------------------------------------------------
// Parametri
// ---------------------------------------------------------------------------

/// Celle per lato di una patch (128 deve essere divisibile per questo valore).
const PATCH_CELLS: usize = 8;
const PATCHES_PER_SIDE: usize = 128 / PATCH_CELLS;
const PATCH_M: f32 = PATCH_CELLS as f32 * CELL_SIZE;

/// Risoluzione della mappa di "verde" di ogni tile.
const MASK_RES: usize = 16;

/// Patch distrutte solo oltre `range + HYSTERESIS`, cosi' non si rigenerano
/// continuamente camminando al limite.
const HYSTERESIS: f32 = 25.0;

/// Margine (in celle da 6,4 m) intorno al rettangolo di una città/località dove
/// non cresce erba: copre mura, porte e strade d'ingresso.
const CITY_MARGIN_CELLS: usize = 2;

const MAX_PENDING_PATCHES: usize = 6;
const MAX_SPAWNS_PER_FRAME: usize = 3;

/// Pendenza massima (m di dislivello per m) oltre la quale non cresce erba.
const MAX_SLOPE: f32 = 1.2;

#[derive(Resource)]
pub struct GrassSettings {
    pub enabled: bool,
    /// Distanza massima dell'erba dal giocatore (m).
    pub range: f32,
    /// Ciuffi candidati per lato di cella (tufts^2 per cella).
    pub tufts_per_axis: usize,
}

impl Default for GrassSettings {
    fn default() -> Self {
        Self { enabled: true, range: 500.0, tufts_per_axis: 10 }
    }
}

#[derive(Resource)]
pub struct GrassAssets {
    material: Handle<StandardMaterial>,
}

pub fn setup_grass(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    let material = materials.add(StandardMaterial {
        base_color: Color::WHITE, // i colori effettivi provengono dai colori di vertice
        perceptual_roughness: 0.95,
        reflectance: 0.05,
        cull_mode: None,
        ..default()
    });
    commands.insert_resource(GrassAssets { material });
}

pub fn grass_toggle(keys: Res<ButtonInput<KeyCode>>, mut settings: ResMut<GrassSettings>) {
    if keys.just_pressed(KeyCode::KeyG) {
        settings.enabled = !settings.enabled;
        println!("Erba: {}", if settings.enabled { "attiva" } else { "disattivata" });
    }
}

// ---------------------------------------------------------------------------
// Mappa di "verde" delle tile
// ---------------------------------------------------------------------------

pub struct GrassMask {
    weight: Vec<u8>,
    color: Vec<[u8; 3]>,
    tile_max: Vec<u8>,
}

/// Riconosce i pixel di erba tollerando i verdi desaturati/olivastri originali di Daggerfall.
fn is_grass_pixel(c: [u8; 3]) -> bool {
    let (r, g, b) = (c[0] as i32, c[1] as i32, c[2] as i32);
    g >= r && g >= b + 4 && g >= 20
}

impl GrassMask {
    fn build(data: &GameData, archive: i32) -> Option<Self> {
        let path = find_file_ci(&data.dir, &archive_file_name(archive as u32))?;
        let tex = TextureFile::open(&path).ok()?;
        let arr = load_tile_array(&tex).ok()?;
        let (w, h) = (arr.width, arr.height);
        if w < MASK_RES || h < MASK_RES {
            return None;
        }
        let cells = MASK_RES * MASK_RES;
        let mut weight = vec![0u8; arr.layers * cells];
        let mut color = vec![[0u8; 3]; arr.layers * cells];
        let mut tile_max = vec![0u8; arr.layers];

        for t in 0..arr.layers {
            let layer = arr.layer(t);
            for my in 0..MASK_RES {
                let (y0, y1) = (my * h / MASK_RES, (my + 1) * h / MASK_RES);
                for mx in 0..MASK_RES {
                    let (x0, x1) = (mx * w / MASK_RES, (mx + 1) * w / MASK_RES);
                    let (mut n, mut green) = (0u32, 0u32);
                    let (mut sum_green, mut sum_all) = ([0u32; 3], [0u32; 3]);
                    for y in y0..y1 {
                        for x in x0..x1 {
                            let c = data.palette.color(layer[y * w + x]);
                            n += 1;
                            for k in 0..3 {
                                sum_all[k] += c[k] as u32;
                            }
                            if is_grass_pixel(c) {
                                green += 1;
                                for k in 0..3 {
                                    sum_green[k] += c[k] as u32;
                                }
                            }
                        }
                    }
                    if n == 0 {
                        continue;
                    }
                    let i = t * cells + my * MASK_RES + mx;
                    weight[i] = (green * 255 / n) as u8;
                    let (sum, div) = if green > 0 { (sum_green, green) } else { (sum_all, n) };
                    color[i] = [(sum[0] / div) as u8, (sum[1] / div) as u8, (sum[2] / div) as u8];
                    tile_max[t] = tile_max[t].max(weight[i]);
                }
            }
        }

        let green_tiles: Vec<usize> = (0..arr.layers)
        .filter(|&t| {
            let s: u32 = weight[t * cells..(t + 1) * cells].iter().map(|&v| v as u32).sum();
            s / cells as u32 > 80
        })
        .collect();
        info!("Erba: archivio terreno {archive}, tile in maggioranza verdi: {green_tiles:?}");
        Some(Self { weight, color, tile_max })
    }

    fn tile_has_grass(&self, tile: usize) -> bool {
        self.tile_max.get(tile).is_some_and(|&m| m > 30)
    }

    fn sample(&self, tile: usize, u: f32, v: f32) -> (f32, [u8; 3]) {
        let ix = ((u * MASK_RES as f32) as usize).min(MASK_RES - 1);
        let iy = (((1.0 - v) * MASK_RES as f32) as usize).min(MASK_RES - 1);
        let i = tile * MASK_RES * MASK_RES + iy * MASK_RES + ix;
        (self.weight[i] as f32 / 255.0, self.color[i])
    }
}

pub fn grass_mask(data: &GameData, archive: i32) -> Option<Arc<GrassMask>> {
    static CACHE: OnceLock<Mutex<HashMap<i32, Option<Arc<GrassMask>>>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().ok()?;
    cache
    .entry(archive)
    .or_insert_with(|| GrassMask::build(data, archive).map(Arc::new))
    .clone()
}

// ---------------------------------------------------------------------------
// Dati per chunk e stato delle patch
// ---------------------------------------------------------------------------

enum PatchState {
    Building(Task<Option<PatchMesh>>),
    Spawned(Entity),
    Empty,
}

#[derive(Component)]
pub struct GrassSource {
    mx: i32,
    my: i32,
    samples: Arc<HeightSamples>,
    tilemap: Arc<Vec<u8>>,
    mask: Arc<GrassMask>,
    city_rect: Option<CityTileRect>,
    patches: HashMap<(usize, usize), PatchState>,
}

impl GrassSource {
    pub fn new(
        mx: i32,
        my: i32,
        samples: Arc<HeightSamples>,
        tilemap: Arc<Vec<u8>>,
        mask: Arc<GrassMask>,
        city_rect: Option<CityTileRect>,
    ) -> Self {
        Self { mx, my, samples, tilemap, mask, city_rect, patches: HashMap::new() }
    }
}

// ---------------------------------------------------------------------------
// Generazione della mesh di una patch (thread asincrono)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct PatchMesh {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[f32; 4]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    min: Vec3,
    max: Vec3,
}

struct PatchJob {
    seed: u64,
    px: usize,
    py: usize,
    tufts: usize,
    samples: Arc<HeightSamples>,
    tilemap: Arc<Vec<u8>>,
    mask: Arc<GrassMask>,
    city_rect: Option<CityTileRect>,
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        let mut r = Self(if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed });
        for _ in 0..3 {
            r.next();
        }
        r
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn f32(&mut self) -> f32 {
        (self.next() & 0xFF_FFFF) as f32 / 16_777_215.0
    }
    fn range(&mut self, min: f32, max: f32) -> f32 {
        min + self.f32() * (max - min)
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn linear_color(c: [f32; 3]) -> [f32; 4] {
    let f = |x: f32| {
        let x = x.clamp(0.0, 1.0);
        if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
    };
    [f(c[0]), f(c[1]), f(c[2]), 1.0]
}

impl PatchMesh {
    fn blade(
        &mut self,
        base: Vec3,
        facing: f32,
        width: f32,
        height: f32,
        lean: f32,
        color: [f32; 3],
    ) {
        let side = Vec3::new(facing.cos(), 0.0, facing.sin());
        let fwd = Vec3::new(-facing.sin(), 0.0, facing.cos());
        let up = Vec3::Y;

        let mid = base + up * (height * 0.5) + fwd * (lean * 0.25);
        let tip = base + up * height + fwd * lean;
        let verts = [
            (base - side * (width * 0.5), 0.0, 0.40),
            (base + side * (width * 0.5), 0.0, 0.40),
            (mid - side * (width * 0.36), 0.5, 0.80),
            (mid + side * (width * 0.36), 0.5, 0.80),
            (tip, 1.0, 1.12),
        ];
        let n = (up * 0.9 + fwd * 0.3).normalize();

        let first = self.positions.len() as u32;
        for (p, v, shade) in verts {
            self.min = self.min.min(p);
            self.max = self.max.max(p);
            self.positions.push(p.to_array());
            self.normals.push(n.to_array());
            self.uvs.push([0.5, v]);
            let warm = if v >= 1.0 { 0.03 } else { 0.0 };
            self.colors.push(linear_color([
                color[0] * shade + warm,
                color[1] * shade + warm * 0.6,
                color[2] * shade,
            ]));
        }
        self.indices
        .extend_from_slice(&[first, first + 1, first + 3, first, first + 3, first + 2, first + 2, first + 3, first + 4]);
    }
}

fn build_patch(job: PatchJob) -> Option<PatchMesh> {
    let mut out = PatchMesh { min: Vec3::splat(f32::MAX), max: Vec3::splat(f32::MIN), ..default() };
    let samples: &HeightSamples = &job.samples;
    let tufts = job.tufts.max(1);

    for cy in job.py * PATCH_CELLS..(job.py + 1) * PATCH_CELLS {
        for cx in job.px * PATCH_CELLS..(job.px + 1) * PATCH_CELLS {
            // niente erba dentro la città (strade, piazze, edifici, mura) e intorno
            if let Some(r) = job.city_rect {
                let m = CITY_MARGIN_CELLS;
                if cx + m >= r.min_x && cx <= r.max_x + m && cy + m >= r.min_y && cy <= r.max_y + m {
                    continue;
                }
            }
            let value = job.tilemap[cx + cy * 128];
            let tile = ((value >> 2) as usize).min(TILE_COUNT - 1);
            if !job.mask.tile_has_grass(tile) {
                continue;
            }

            let mut rng = Rng::new(job.seed ^ ((cx as u64) << 9) ^ ((cy as u64) << 24) ^ 0xA5A5_5A5A);
            for sy in 0..tufts {
                for sx in 0..tufts {
                    let vx = (sx as f32 + rng.f32()) / tufts as f32;
                    let vy = (sy as f32 + rng.f32()) / tufts as f32;
                    let keep_roll = rng.f32();
                    let facing0 = rng.f32() * std::f32::consts::TAU;
                    let tuft_height = rng.range(0.35, 1.0);
                    let tint = rng.range(0.85, 1.2);
                    let blades = 6 + (rng.f32() * 4.0) as usize; // 4..=6

                    let (u, v) = tile_uv_transform(value, vx, vy);
                    let (w, col) = job.mask.sample(tile, u, v);
                    if keep_roll >= smoothstep(0.15, 0.60, w) {
                        continue;
                    }

                    let (gx, gy) = (cx as f32 + vx, cy as f32 + vy);
                    let d = 0.2;
                    let dh_dx = terrain_height(samples, gx + d, gy) - terrain_height(samples, gx - d, gy);
                    let dh_dy = terrain_height(samples, gx, gy + d) - terrain_height(samples, gx, gy - d);
                    let slope = dh_dx.abs().max(dh_dy.abs()) / (2.0 * d * CELL_SIZE);
                    if slope > MAX_SLOPE {
                        continue;
                    }

                    let y = terrain_height(samples, gx, gy) - 0.05;
                    let origin = Vec3::new(gx * CELL_SIZE, y, -(gy * CELL_SIZE));

                    let h = tuft_height * (0.55 + 0.45 * w);
                    let base_color = [
                        col[0] as f32 / 255.0 * tint,
                        col[1] as f32 / 255.0 * tint,
                        col[2] as f32 / 255.0 * tint,
                    ];
                    for i in 0..blades {
                        let a = rng.f32() * std::f32::consts::TAU;
                        let r = rng.range(0.0, 0.30);
                        let pos = origin + Vec3::new(a.cos() * r, 0.0, a.sin() * r);
                        let facing = facing0 + i as f32 * 1.7 + rng.range(-0.4, 0.4);
                        let blade_h = h * rng.range(0.7, 1.1);
                        let lean = blade_h * rng.range(0.10, 0.40);
                        out.blade(pos, facing, rng.range(0.08, 0.13), blade_h, lean, base_color);
                    }
                }
            }
        }
    }

    (!out.positions.is_empty()).then_some(out)
}

// ---------------------------------------------------------------------------
// Sistema: crea e distrugge le patch intorno al giocatore
// ---------------------------------------------------------------------------

fn despawn_all(commands: &mut Commands, src: &mut GrassSource) {
    for (_, st) in src.patches.drain() {
        if let PatchState::Spawned(e) = st {
            commands.entity(e).despawn_recursive();
        }
    }
}

fn patch_distance(lx: f32, ln: f32, px: usize, py: usize) -> f32 {
    let (x0, y0) = (px as f32 * PATCH_M, py as f32 * PATCH_M);
    let dx = (x0 - lx).max(lx - (x0 + PATCH_M)).max(0.0);
    let dy = (y0 - ln).max(ln - (y0 + PATCH_M)).max(0.0);
    dx.hypot(dy)
}

#[allow(clippy::too_many_arguments)]
pub fn update_grass(
    mut commands: Commands,
    settings: Res<GrassSettings>,
    assets: Option<Res<GrassAssets>>,
    player: Query<&Transform, With<crate::player::Player>>,
    mut chunks: Query<(Entity, &mut GrassSource, &Transform)>,
                    mut meshes: ResMut<Assets<Mesh>>,
) {
    let Some(assets) = assets else { return };

    if !settings.enabled {
        for (_, mut src, _) in &mut chunks {
            if !src.patches.is_empty() {
                despawn_all(&mut commands, &mut src);
            }
        }
        return;
    }
    let Ok(player_tf) = player.get_single() else { return };
    let player_pos = player_tf.translation;
    let keep_dist = settings.range + HYSTERESIS;

    let mut pending: usize = chunks
    .iter()
    .map(|(_, s, _)| s.patches.values().filter(|p| matches!(p, PatchState::Building(_))).count())
    .sum();
    let mut spawned_now = 0usize;
    let mut wanted: Vec<(f32, Entity, (usize, usize))> = Vec::new();

    for (chunk, mut src, chunk_tf) in &mut chunks {
        let local = player_pos - chunk_tf.translation;
        let (lx, ln) = (local.x, -local.z);

        // 1. Despawn delle patch troppo lontane
        let far: Vec<(usize, usize)> = src
        .patches
        .keys()
        .filter(|&&(px, py)| patch_distance(lx, ln, px, py) > keep_dist)
        .copied()
        .collect();
        for key in far {
            if let Some(PatchState::Spawned(e)) = src.patches.remove(&key) {
                commands.entity(e).despawn_recursive();
            }
        }

        // 2. Mesh completate: spawn nel mondo
        let mut done: Vec<((usize, usize), Option<PatchMesh>)> = Vec::new();
        for (key, st) in src.patches.iter_mut() {
            if spawned_now + done.len() >= MAX_SPAWNS_PER_FRAME {
                break;
            }
            if let PatchState::Building(task) = st {
                if let Some(result) = block_on(future::poll_once(task)) {
                    done.push((*key, result));
                }
            }
        }
        for (key, result) in done {
            pending = pending.saturating_sub(1);
            spawned_now += 1;
            let Some(pm) = result else {
                src.patches.insert(key, PatchState::Empty);
                continue;
            };
            let aabb = Aabb::from_min_max(pm.min, pm.max);
            let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, pm.positions);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, pm.normals);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, pm.uvs);
            mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, pm.colors);
            mesh.insert_indices(Indices::U32(pm.indices));
            let e = commands
            .spawn((
                Mesh3d(meshes.add(mesh)),
                    MeshMaterial3d(assets.material.clone()),
                    Transform::IDENTITY,
                    aabb,
                    NotShadowCaster,
            ))
            .id();
            commands.entity(chunk).add_child(e);
            src.patches.insert(key, PatchState::Spawned(e));
        }

        // 3. Identificazione delle patch mancanti nel raggio
        let r = settings.range;
        let to_idx = |v: f32| ((v / PATCH_M).floor() as i32).clamp(0, PATCHES_PER_SIDE as i32 - 1) as usize;
        let (x_lo, x_hi) = (to_idx(lx - r), to_idx(lx + r));
        let (y_lo, y_hi) = (to_idx(ln - r), to_idx(ln + r));

        if lx + r < 0.0 || ln + r < 0.0 || lx - r > PATCHES_PER_SIDE as f32 * PATCH_M || ln - r > PATCHES_PER_SIDE as f32 * PATCH_M {
            continue;
        }
        for py in y_lo..=y_hi {
            for px in x_lo..=x_hi {
                if src.patches.contains_key(&(px, py)) {
                    continue;
                }
                let d = patch_distance(lx, ln, px, py);
                if d <= r {
                    wanted.push((d, chunk, (px, py)));
                }
            }
        }
    }

    wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
    let pool = AsyncComputeTaskPool::get();
    for (_, chunk, key) in wanted {
        if pending >= MAX_PENDING_PATCHES {
            break;
        }
        let Ok((_, mut src, _)) = chunks.get_mut(chunk) else { continue };
        let job = PatchJob {
            seed: ((src.mx as u64) << 32) ^ (src.my as u64 & 0xFFFF_FFFF) ^ 0x6A09_E667_F3BC_C908,
            px: key.0,
            py: key.1,
            tufts: settings.tufts_per_axis,
            samples: Arc::clone(&src.samples),
            tilemap: Arc::clone(&src.tilemap),
            mask: Arc::clone(&src.mask),
            city_rect: src.city_rect,
        };
        let task = pool.spawn(async move { build_patch(job) });
        src.patches.insert(key, PatchState::Building(task));
        pending += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_green_pixels_count_as_grass() {
        assert!(is_grass_pixel([70, 98, 40])); // erba
        assert!(is_grass_pixel([40, 60, 30])); // erba scura (palude)
        assert!(!is_grass_pixel([235, 240, 250])); // neve
        assert!(!is_grass_pixel([110, 84, 56])); // terra
        assert!(!is_grass_pixel([120, 120, 118])); // roccia
        assert!(!is_grass_pixel([190, 170, 110])); // sabbia
    }

    #[test]
    fn smoothstep_is_clamped() {
        assert_eq!(smoothstep(0.3, 0.75, 0.0), 0.0);
        assert_eq!(smoothstep(0.3, 0.75, 1.0), 1.0);
        let mid = smoothstep(0.3, 0.75, 0.525);
        assert!((mid - 0.5).abs() < 1e-3);
    }

    #[test]
    fn patch_distance_is_zero_inside() {
        assert_eq!(patch_distance(10.0, 10.0, 0, 0), 0.0);
        let d = patch_distance(PATCH_M + 3.0, 0.0, 0, 0);
        assert!((d - 3.0).abs() < 1e-4);
    }

    #[test]
    fn blade_adds_five_vertices_and_nine_indices() {
        let mut m = PatchMesh { min: Vec3::splat(f32::MAX), max: Vec3::splat(f32::MIN), ..default() };
        m.blade(Vec3::ZERO, 0.0, 0.1, 0.5, 0.1, [0.3, 0.5, 0.2]);
        assert_eq!(m.positions.len(), 5);
        assert_eq!(m.indices.len(), 9);
        assert!(m.indices.iter().all(|&i| i < 5));
        assert!((m.max.y - 0.5).abs() < 1e-5);
    }
}
