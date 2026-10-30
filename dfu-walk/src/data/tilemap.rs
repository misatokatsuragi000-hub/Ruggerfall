//! Tilemap del terreno: quale tile (0..55) va in ognuna delle 128x128 celle di un
//! map pixel. Porting di DefaultTerrainTexturing (TerrainTexturing.cs) e della
//! conversione finale di TerrainHelper.UpdateTileMapDataJob.

use super::error::{format_err, Result};
use super::heightmap::{HeightSamples, NoiseFn, BEACH_ELEVATION, MAX_TERRAIN_HEIGHT, OCEAN_ELEVATION};
use super::perlin::perlin_noise;

pub const TILEMAP_DIM: usize = 128;
pub const TILE_DATA_DIM: usize = TILEMAP_DIM + 1;
pub const ROTATE_BIT: u8 = 0x40;
pub const FLIP_BIT: u8 = 0x80;
pub const UNSET_TILE: u8 = 0xFF;

const MAX_WORLD_TILE_COORD_Z: i32 = 64000;
const SEED: i32 = 417028;

const WATER: u8 = 0;
const DIRT: u8 = 1;
const GRASS: u8 = 2;
const STONE: u8 = 3;

pub type BeachJitterFn = fn(u32) -> f32;

fn wang_hash(mut n: u32) -> u32 {
    n = (n ^ 61) ^ (n >> 16);
    n = n.wrapping_mul(9);
    n ^= n >> 4;
    n = n.wrapping_mul(0x27d4_eb2d);
    n ^= n >> 15;
    n
}

fn next_state(state: &mut u32) -> u32 {
    let t = *state;
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    t
}

pub fn unity_beach_jitter(index: u32) -> f32 {
    let mut state = wang_hash(index.wrapping_add(62)) ^ 0xD251_1F53;
    next_state(&mut state);
    let bits = 0x3f80_0000u32 | (next_state(&mut state) >> 9);
    let unit = f32::from_bits(bits) - 1.0;
    unit * 3.0 + -1.5
}

fn weight_noise(noise: NoiseFn, x: f32, y: f32) -> f32 {
    let (mut frequency, mut amplitude) = (0.05f32, 0.9f32);
    let mut value = 0.0f32;
    for _ in 0..3 {
        value += noise(SEED as f32 + x * frequency, SEED as f32 + y * frequency) * amplitude;
        frequency *= 2.0;
        amplitude *= 0.4;
    }
    value.clamp(-1.0, 1.0)
}

fn weighted_record(weight: f32) -> u8 {
    if weight < 0.5 {
        DIRT
    } else if weight > 0.95 {
        STONE
    } else {
        GRASS
    }
}

fn make_lookup(index: i32, rotate: bool, flip: bool) -> u8 {
    assert!(index <= 55, "indice di tile fuori range (0-55)");
    let mut v = index;
    if rotate {
        v += 64;
    }
    if flip {
        v += 128;
    }
    v as u8
}

fn add_lookup_range(
    table: &mut [u8; 64],
    base_start: i32,
    base_end: i32,
    shape_start: i32,
    saddle: i32,
    reverse: bool,
    offset: usize,
) {
    let m = make_lookup;
    let s = shape_start;
    let rows: [u8; 16] = if reverse {
        [
            m(base_start, false, false),
            m(s + 2, true, true),
            m(s + 2, false, false),
            m(s + 1, true, true),
            m(s + 2, false, true),
            m(s + 1, false, true),
            m(saddle, true, false),
            m(s, true, true),
            m(s + 2, true, false),
            m(saddle, false, false),
            m(s + 1, false, false),
            m(s, false, false),
            m(s + 1, true, false),
            m(s, false, true),
            m(s, true, false),
            m(base_end, false, false),
        ]
    } else {
        [
            m(base_start, false, false),
            m(s, true, false),
            m(s, false, true),
            m(s + 1, true, false),
            m(s, false, false),
            m(s + 1, false, false),
            m(saddle, false, false),
            m(s + 2, true, false),
            m(s, true, true),
            m(saddle, true, false),
            m(s + 1, false, true),
            m(s + 2, false, true),
            m(s + 1, true, true),
            m(s + 2, false, false),
            m(s + 2, true, true),
            m(base_end, false, false),
        ]
    };
    table[offset..offset + 16].copy_from_slice(&rows);
}

pub fn lookup_table() -> [u8; 64] {
    let mut t = [0u8; 64];
    add_lookup_range(&mut t, 0, 1, 5, 48, false, 0);
    add_lookup_range(&mut t, 2, 1, 10, 51, true, 16);
    add_lookup_range(&mut t, 2, 3, 15, 53, false, 32);
    add_lookup_range(&mut t, 3, 3, 15, 53, true, 48);
    t
}

fn generate_tile_data(
    samples: &HeightSamples,
    map_pixel_x: i32,
    map_pixel_y: i32,
    noise: NoiseFn,
    jitter: BeachJitterFn,
) -> Vec<u8> {
    let hdim = samples.dim;
    let mut tile_data = vec![0u8; TILE_DATA_DIM * TILE_DATA_DIM];
    for (index, out) in tile_data.iter_mut().enumerate() {
        let x = index % TILE_DATA_DIM;
        let y = index / TILE_DATA_DIM;

        let hx = (hdim as f32 * (x as f32 / TILE_DATA_DIM as f32)).clamp(0.0, (hdim - 1) as f32) as usize;
        let hy = (hdim as f32 * (y as f32 / TILE_DATA_DIM as f32)).clamp(0.0, (hdim - 1) as f32) as usize;
        let height = samples.get(hx, hy) * MAX_TERRAIN_HEIGHT;

        if height <= OCEAN_ELEVATION {
            *out = WATER;
            continue;
        }
        if height <= BEACH_ELEVATION + jitter(index as u32) {
            *out = DIRT;
            continue;
        }

        let latitude = map_pixel_x * TILEMAP_DIM as i32 + x as i32;
        let longitude = MAX_WORLD_TILE_COORD_Z - map_pixel_y * TILEMAP_DIM as i32 + y as i32;
        *out = weighted_record(weight_noise(noise, latitude as f32, longitude as f32));
    }
    tile_data
}

pub fn generate_tilemap(
    samples: &HeightSamples,
    map_pixel_x: i32,
    map_pixel_y: i32,
    preset: Option<&[u8]>,
    noise: NoiseFn,
    jitter: BeachJitterFn,
    march: bool,
) -> Result<Vec<u8>> {
    let mut tilemap = match preset {
        Some(p) if p.len() == TILEMAP_DIM * TILEMAP_DIM => p.to_vec(),
        Some(p) => return format_err(format!("preset di {} celle, attese {}", p.len(), TILEMAP_DIM * TILEMAP_DIM)),
        None => vec![UNSET_TILE; TILEMAP_DIM * TILEMAP_DIM],
    };
    let tile_data = generate_tile_data(samples, map_pixel_x, map_pixel_y, noise, jitter);
    let lookup = lookup_table();

    for (index, cell) in tilemap.iter_mut().enumerate() {
        if *cell != UNSET_TILE {
            continue;
        }
        let x = index % TILEMAP_DIM;
        let y = index / TILEMAP_DIM;
        let td = x + y * TILE_DATA_DIM;
        if march {
            let b0 = tile_data[td] as usize;
            let b1 = tile_data[td + 1] as usize;
            let b2 = tile_data[td + TILE_DATA_DIM] as usize;
            let b3 = tile_data[td + TILE_DATA_DIM + 1] as usize;
            let shape = (b0 & 1) | ((b1 & 1) << 1) | ((b2 & 1) << 2) | ((b3 & 1) << 3);
            let ring = (b0 + b1 + b2 + b3) >> 2;
            *cell = lookup[shape | (ring << 4)];
        } else {
            *cell = tile_data[td];
        }
    }
    Ok(tilemap)
}

pub fn generate_default_tilemap(
    samples: &HeightSamples,
    map_pixel_x: i32,
    map_pixel_y: i32,
    preset: Option<&[u8]>,
) -> Result<Vec<u8>> {
    generate_tilemap(samples, map_pixel_x, map_pixel_y, preset, perlin_noise, unity_beach_jitter, true)
}

pub fn encode_for_shader(tilemap: &[u8], convert_water: bool) -> Vec<u8> {
    tilemap
    .iter()
    .map(|&tile| {
        if convert_water && tile == UNSET_TILE {
            return 0;
        }
        let mut record = (tile & 0x3F) as u32 * 4;
        if tile & ROTATE_BIT != 0 {
            record += 1;
        }
        if tile & FLIP_BIT != 0 {
            record += 2;
        }
        (record & 0xFF) as u8
    })
    .collect()
}
