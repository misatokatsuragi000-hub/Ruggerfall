//! Tilemap del terreno: quale tile (0..55) va in ognuna delle 128x128 celle di un
//! map pixel. Porting di DefaultTerrainTexturing (TerrainTexturing.cs) e della
//! conversione finale di TerrainHelper.UpdateTileMapDataJob.
//!
//! Funzionamento: per ogni punto di una griglia 129x129 si sceglie un materiale
//! (acqua, terra, erba, pietra) da quota e rumore; poi, per ogni cella, un
//! "marching squares" guarda i quattro angoli e sceglie la tile di transizione
//! (con rotazione e specchiatura) dalla tabella di lookup.
//!
//! I valori della tilemap sono: indice di tile 0..55 | `ROTATE_BIT` | `FLIP_BIT`.
//! Il valore 0xFF segna una cella di una località che sarebbe acqua (zero) e che
//! non va sovrascritta; `encode_for_shader` lo riconverte in acqua.

use crate::error::{format_err, Result};
use crate::heightmap::{HeightSamples, NoiseFn, BEACH_ELEVATION, MAX_TERRAIN_HEIGHT, OCEAN_ELEVATION};
use crate::perlin::perlin_noise;

pub const TILEMAP_DIM: usize = 128;
pub const TILE_DATA_DIM: usize = TILEMAP_DIM + 1;
pub const ROTATE_BIT: u8 = 0x40;
pub const FLIP_BIT: u8 = 0x80;
const MAX_WORLD_TILE_COORD_Z: i32 = 64000;
/// Stesso seme per tutti i map pixel, così le tile sono continue fra l'uno e l'altro.
const SEED: i32 = 417028;

const WATER: u8 = 0;
const DIRT: u8 = 1;
const GRASS: u8 = 2;
const STONE: u8 = 3;

/// Variazione casuale (±1,5) della linea di spiaggia, per indice di cella.
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

/// `Unity.Mathematics.Random.CreateFromIndex(index).NextFloat(-1.5f, 1.5f)`
/// come lo ricordo dalla libreria (hash di Wang + xorshift). Non verificato
/// contro Unity: influisce solo su quali celle al limite della spiaggia
/// diventano terra o erba, quindi si può sostituire.
pub fn unity_beach_jitter(index: u32) -> f32 {
    let mut state = wang_hash(index.wrapping_add(62)) ^ 0xD251_1F53;
    next_state(&mut state); // il costruttore avanza già di uno stato
    let bits = 0x3f80_0000u32 | (next_state(&mut state) >> 9);
    let unit = f32::from_bits(bits) - 1.0;
    unit * 3.0 + -1.5
}

fn weight_noise(noise: NoiseFn, x: f32, y: f32) -> f32 {
    // GetNoise(worldX, worldY, 0.05, 0.9, 0.4, 3, seed) con clamp in [-1, 1].
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
        // alto > basso
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
        // basso > alto
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

/// Tabella a 64 voci: indice = forma (4 bit) | anello << 4.
pub fn lookup_table() -> [u8; 64] {
    let mut t = [0u8; 64];
    add_lookup_range(&mut t, 0, 1, 5, 48, false, 0);
    add_lookup_range(&mut t, 2, 1, 10, 51, true, 16);
    add_lookup_range(&mut t, 2, 3, 15, 53, false, 32);
    add_lookup_range(&mut t, 3, 3, 15, 53, true, 48);
    t
}

/// Materiale di ognuno dei 129x129 punti (indice = x + y * 129).
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

        // Campione di altezza per acqua e spiaggia (stessa aritmetica f32 di DFU).
        let hx = (hdim as f32 * (x as f32 / TILE_DATA_DIM as f32)).clamp(0.0, (hdim - 1) as f32) as usize;
        let hy = (hdim as f32 * (y as f32 / TILE_DATA_DIM as f32)).clamp(0.0, (hdim - 1) as f32) as usize;
        let height = samples.get(hx, hy) * MAX_TERRAIN_HEIGHT;

        if height <= OCEAN_ELEVATION {
            *out = WATER;
            continue;
        }
        // Spiaggia, con un po' di casualità perché la linea non sia troppo regolare.
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

/// Tilemap 128x128 (indice = x + y * 128). `preset` contiene le celle già
/// fissate da una località (0 = libera); `march` abilita le transizioni.
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
        None => vec![0u8; TILEMAP_DIM * TILEMAP_DIM],
    };
    let tile_data = generate_tile_data(samples, map_pixel_x, map_pixel_y, noise, jitter);
    let lookup = lookup_table();

    for (index, cell) in tilemap.iter_mut().enumerate() {
        // Le celle già impostate da una località non si toccano.
        if *cell != 0 {
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

/// Tilemap con rumore e variazione di spiaggia predefiniti.
pub fn generate_default_tilemap(
    samples: &HeightSamples,
    map_pixel_x: i32,
    map_pixel_y: i32,
    preset: Option<&[u8]>,
) -> Result<Vec<u8>> {
    generate_tilemap(samples, map_pixel_x, map_pixel_y, preset, perlin_noise, unity_beach_jitter, true)
}

/// Formato letto dallo shader del terreno: da [flip, rotate, indice a 6 bit] a
/// `(indice << 2) | rotate | flip << 1` (UpdateTileMapDataJob).
pub fn encode_for_shader(tilemap: &[u8], convert_water: bool) -> Vec<u8> {
    tilemap
        .iter()
        .map(|&tile| {
            if convert_water && tile == 0xFF {
                return 0;
            }
            let mut record = tile as u32 * 4;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heightmap::HeightSamples;

    fn flat(height_units: f32) -> HeightSamples {
        HeightSamples {
            dim: 129,
            data: vec![height_units / MAX_TERRAIN_HEIGHT; 129 * 129],
        }
    }

    // Rumori costanti come fn pointer: uno per valore.
    fn n030(_: f32, _: f32) -> f32 {
        0.3
    }
    fn n050(_: f32, _: f32) -> f32 {
        0.5
    }
    fn n090(_: f32, _: f32) -> f32 {
        0.9
    }
    fn no_jitter(_: u32) -> f32 {
        0.0
    }

    #[test]
    fn lookup_table_matches_the_dfu_source() {
        let t = lookup_table();
        // acqua > terra (offset 0)
        assert_eq!(t[0], 0);
        assert_eq!(t[1], 5 + 64);
        assert_eq!(t[2], 5 + 128);
        assert_eq!(t[6], 48);
        assert_eq!(t[8], 5 + 64 + 128);
        assert_eq!(t[9], 48 + 64);
        assert_eq!(t[10], 6 + 128);
        assert_eq!(t[15], 1);
        // terra > erba (offset 16, invertito)
        assert_eq!(t[16], 2);
        assert_eq!(t[22], 51 + 64);
        assert_eq!(t[31], 1);
        // erba > pietra (offset 32) e pietra (offset 48)
        assert_eq!(t[32], 2);
        assert_eq!(t[47], 3);
        assert_eq!(t[63], 3);
        assert!(t.iter().all(|&v| (v & 63) <= 55));
    }

    #[test]
    fn ocean_everywhere_is_water() {
        let tm = generate_tilemap(&flat(0.0), 100, 200, None, n050, no_jitter, true).unwrap();
        assert_eq!(tm.len(), 128 * 128);
        assert!(tm.iter().all(|&t| t == 0));
    }

    #[test]
    fn beach_band_is_dirt() {
        // 30 sta fra l'oceano (27,2) e la spiaggia (40).
        let tm = generate_tilemap(&flat(30.0), 100, 200, None, n050, no_jitter, true).unwrap();
        assert!(tm.iter().all(|&t| t == 1));
    }

    #[test]
    fn noise_weight_picks_dirt_grass_or_stone() {
        // peso = rumore * (0.9 + 0.36 + 0.144) = rumore * 1.404
        let dirt = generate_tilemap(&flat(500.0), 5, 5, None, n030, no_jitter, true).unwrap();
        assert!(dirt.iter().all(|&t| t == 1)); // 0.42 < 0.5
        let grass = generate_tilemap(&flat(500.0), 5, 5, None, n050, no_jitter, true).unwrap();
        assert!(grass.iter().all(|&t| t == 2)); // 0.70
        let stone = generate_tilemap(&flat(500.0), 5, 5, None, n090, no_jitter, true).unwrap();
        assert!(stone.iter().all(|&t| t == 3)); // 1.26 > 0.95
    }

    #[test]
    fn coast_uses_the_transition_tiles() {
        // colonne x < 64 acqua, x >= 64 terra (altezza 30): la cella 63 è una transizione.
        let mut s = flat(0.0);
        for x in 64..129 {
            for y in 0..129 {
                s.data[y + x * 129] = 30.0 / MAX_TERRAIN_HEIGHT;
            }
        }
        let tm = generate_tilemap(&s, 0, 0, None, n050, no_jitter, true).unwrap();
        for y in 0..128 {
            assert_eq!(tm[62 + y * 128], 0, "acqua a x=62");
            assert_eq!(tm[63 + y * 128], 6 + 128, "bordo a x=63 (forma 10, anello 0)");
            assert_eq!(tm[64 + y * 128], 1, "terra a x=64");
        }
    }

    #[test]
    fn location_tiles_are_preserved() {
        let mut preset = vec![0u8; 128 * 128];
        preset[5 + 7 * 128] = 0xFF; // cella di località che sarebbe acqua
        preset[6 + 7 * 128] = ROTATE_BIT | 10;
        let tm = generate_tilemap(&flat(500.0), 5, 5, Some(&preset), n050, no_jitter, true).unwrap();
        assert_eq!(tm[5 + 7 * 128], 0xFF);
        assert_eq!(tm[6 + 7 * 128], ROTATE_BIT | 10);
        assert_eq!(tm[7 + 7 * 128], 2);
        assert!(generate_tilemap(&flat(500.0), 5, 5, Some(&[0u8; 10]), n050, no_jitter, true).is_err());
    }

    #[test]
    fn without_march_raw_materials_are_used() {
        let tm = generate_tilemap(&flat(500.0), 5, 5, None, n090, no_jitter, false).unwrap();
        assert!(tm.iter().all(|&t| t == 3));
    }

    #[test]
    fn shader_encoding() {
        let enc = encode_for_shader(&[0, 10, ROTATE_BIT | 10, FLIP_BIT | 10, ROTATE_BIT | FLIP_BIT | 55, 0xFF], true);
        assert_eq!(enc, vec![0, 40, 41, 42, 55 * 4 + 3, 0]);
        // senza conversione dell'acqua il valore 0xFF segue la formula normale
        assert_eq!(encode_for_shader(&[0xFF], false), vec![255]);
    }

    #[test]
    fn beach_jitter_is_deterministic_and_centered() {
        assert_eq!(unity_beach_jitter(1234), unity_beach_jitter(1234));
        let (mut sum, mut min, mut max) = (0.0f64, f32::MAX, f32::MIN);
        for i in 0..20000u32 {
            let j = unity_beach_jitter(i);
            sum += j as f64;
            min = min.min(j);
            max = max.max(j);
        }
        assert!(min >= -1.5 && max <= 1.5, "{min} {max}");
        assert!((sum / 20000.0).abs() < 0.1, "media {}", sum / 20000.0);
        assert!(max - min > 2.0);
    }
}
