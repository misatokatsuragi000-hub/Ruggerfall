//! Altezze del terreno di un map pixel. Porting di DefaultTerrainSampler.cs
//! (versione 1), con gli stessi coefficienti e la stessa interpolazione cubica.

use crate::error::Result;
use crate::perlin::perlin_noise;
use crate::woods::WoodsFile;

pub const DEFAULT_HEIGHTMAP_DIM: usize = 129;
pub const MAX_TERRAIN_HEIGHT: f32 = 1539.0;
pub const BASE_HEIGHT_SCALE: f32 = 8.0;
pub const NOISE_MAP_SCALE: f32 = 4.0;
pub const EXTRA_NOISE_SCALE: f32 = 10.0;
pub const OCEAN_ELEVATION: f32 = 3.4 * BASE_HEIGHT_SCALE;
pub const BEACH_ELEVATION: f32 = 5.0 * BASE_HEIGHT_SCALE;
const MAX_MAP_PIXEL_Y: i32 = 500;

/// Funzione di rumore 2D in [0, 1], sostituibile (vedi `perlin`).
pub type NoiseFn = fn(f32, f32) -> f32;

/// Campioni 0..1 (altezza / `MAX_TERRAIN_HEIGHT`) di `dim` x `dim` punti.
/// Come in DFU il campione (riga `y`, colonna `x`) sta in `data[y + x * dim]`:
/// è la `x` a variare più lentamente.
#[derive(Clone, Debug)]
pub struct HeightSamples {
    pub dim: usize,
    pub data: Vec<f32>,
}

impl HeightSamples {
    pub fn get(&self, x: usize, y: usize) -> f32 {
        self.data[y + x * self.dim]
    }

    /// Altezza in unità di DFU (campione * `MAX_TERRAIN_HEIGHT`).
    pub fn height(&self, x: usize, y: usize) -> f32 {
        self.get(x, y) * MAX_TERRAIN_HEIGHT
    }
}

pub fn cubic_interpolator(v0: f32, v1: f32, v2: f32, v3: f32, t: f32) -> f32 {
    let a = (v3 - v2) - (v0 - v1);
    let b = (v0 - v1) - a;
    let c = v2 - v0;
    let d = v1;
    a * (t * t * t) + b * (t * t) + c * t + d
}

#[allow(clippy::too_many_arguments)]
pub fn get_noise(
    noise: NoiseFn,
    x: i32,
    y: i32,
    mut frequency: f32,
    mut amplitude: f32,
    persistance: f32,
    octaves: u32,
    seed: i32,
) -> f32 {
    let mut value = 0.0f32;
    for _ in 0..octaves {
        value += noise(seed as f32 + x as f32 * frequency, seed as f32 + y as f32 * frequency) * amplitude;
        frequency *= 2.0;
        amplitude *= persistance;
    }
    value.clamp(0.0, 1.0)
}

pub fn generate_samples(
    woods: &WoodsFile,
    map_pixel_x: i32,
    map_pixel_y: i32,
    dim: usize,
    noise: NoiseFn,
) -> Result<HeightSamples> {
    let div = (dim - 1) as f32 / 3.0;

    // 4x4 altezze attorno al map pixel (base) e 9x9 altezze di dettaglio (rumore).
    let shm = woods.height_values_range_1dim(map_pixel_x - 2, map_pixel_y - 2, 4);
    let lhm = woods.large_height_values_range(map_pixel_x - 1, map_pixel_y, 3)?;
    let s = |x: usize, y: usize| shm[x + y * 4] as f32;
    let l = |x: usize, y: usize| lhm.get(x, y) as f32;

    let mut data = vec![0.0f32; dim * dim];
    for (index, out) in data.iter_mut().enumerate() {
        let x = index / dim;
        let y = index % dim;

        let rx = x as f32 / div;
        let ry = y as f32 / div;
        let ix = rx.floor() as usize;
        let iy = ry.floor() as usize;
        let sfracx = x as f32 / (dim - 1) as f32;
        let sfracy = y as f32 / (dim - 1) as f32;
        let fracx = (x as f32 - ix as f32 * div) / div;
        let fracy = (y as f32 - iy as f32 * div) / div;

        // Base: interpolazione bicubica della mappa piccola.
        let x1 = cubic_interpolator(s(0, 3), s(1, 3), s(2, 3), s(3, 3), sfracx);
        let x2 = cubic_interpolator(s(0, 2), s(1, 2), s(2, 2), s(3, 2), sfracx);
        let x3 = cubic_interpolator(s(0, 1), s(1, 1), s(2, 1), s(3, 1), sfracx);
        let x4 = cubic_interpolator(s(0, 0), s(1, 0), s(2, 0), s(3, 0), sfracx);
        let base = cubic_interpolator(x1, x2, x3, x4, sfracy);
        let mut scaled = base * BASE_HEIGHT_SCALE;

        // Maschera di rumore: interpolazione bicubica della mappa di dettaglio.
        let row = |j: usize| {
            cubic_interpolator(
                l(ix, iy + j),
                l(ix + 1, iy + j),
                l(ix + 2, iy + j),
                l(ix + 3, iy + j),
                fracx,
            )
        };
        let detail = cubic_interpolator(row(0), row(1), row(2), row(3), fracy);
        scaled += detail * NOISE_MAP_SCALE;

        // Microrilievo a livello del suolo.
        let noisex = map_pixel_x * (dim as i32 - 1) + x as i32;
        let noisey = (MAX_MAP_PIXEL_Y - map_pixel_y) * (dim as i32 - 1) + y as i32;
        let low = get_noise(noise, noisex, noisey, 0.3, 0.5, 0.5, 1, 0);
        let high = get_noise(noise, noisex, noisey, 0.9, 0.5, 0.5, 1, 0);
        scaled += (low * high) * EXTRA_NOISE_SCALE;

        if scaled < OCEAN_ELEVATION {
            scaled = OCEAN_ELEVATION;
        }
        *out = (scaled / MAX_TERRAIN_HEIGHT).clamp(0.0, 1.0);
    }
    Ok(HeightSamples { dim, data })
}

/// Come `generate_samples` con la risoluzione e il rumore predefiniti.
pub fn generate_default_samples(woods: &WoodsFile, map_pixel_x: i32, map_pixel_y: i32) -> Result<HeightSamples> {
    generate_samples(woods, map_pixel_x, map_pixel_y, DEFAULT_HEIGHTMAP_DIM, perlin_noise)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::woods::test_support::build_woods_filled;

    fn no_noise(_: f32, _: f32) -> f32 {
        0.0
    }

    #[test]
    fn flat_zero_world_is_clamped_to_ocean() {
        let w = WoodsFile::from_bytes("WOODS.WLD", build_woods_filled(|_, _| 0, &[], 0)).unwrap();
        let s = generate_default_samples(&w, 100, 200).unwrap();
        assert_eq!(s.data.len(), 129 * 129);
        let ocean = OCEAN_ELEVATION / MAX_TERRAIN_HEIGHT;
        assert!(s.data.iter().all(|&v| (v - ocean).abs() < 1e-6));
    }

    #[test]
    fn constant_world_without_noise_is_exact() {
        // base 100*8 + dettaglio 50*4 = 1000
        let w = WoodsFile::from_bytes("WOODS.WLD", build_woods_filled(|_, _| 100, &[], 50)).unwrap();
        let s = generate_samples(&w, 300, 250, 129, no_noise).unwrap();
        for &v in &s.data {
            assert!((v * MAX_TERRAIN_HEIGHT - 1000.0).abs() < 0.01);
        }
    }

    #[test]
    fn microrelief_is_bounded() {
        let w = WoodsFile::from_bytes("WOODS.WLD", build_woods_filled(|_, _| 100, &[], 50)).unwrap();
        let s = generate_default_samples(&w, 300, 250).unwrap();
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for &v in &s.data {
            let h = v * MAX_TERRAIN_HEIGHT;
            assert!((999.99..=1002.51).contains(&h), "{h}");
            lo = lo.min(h);
            hi = hi.max(h);
        }
        assert!(hi - lo > 0.1, "il microrilievo deve variare");
    }

    #[test]
    fn x_axis_spans_pixel_minus_one_to_pixel() {
        // altezza = x/2: il pixel 99 vale 49, il pixel 100 vale 50.
        let w = WoodsFile::from_bytes("WOODS.WLD", build_woods_filled(|x, _| (x / 2) as u8, &[], 0)).unwrap();
        let s = generate_samples(&w, 100, 250, 129, no_noise).unwrap();
        assert!((s.height(0, 64) - 49.0 * 8.0).abs() < 0.01);
        assert!((s.height(128, 64) - 50.0 * 8.0).abs() < 0.01);
        assert!(s.height(64, 64) > s.height(0, 64) && s.height(64, 64) < s.height(128, 64));
    }

    #[test]
    fn cubic_interpolator_hits_the_middle_points() {
        assert_eq!(cubic_interpolator(1.0, 2.0, 5.0, 9.0, 0.0), 2.0);
        assert_eq!(cubic_interpolator(1.0, 2.0, 5.0, 9.0, 1.0), 5.0);
    }
}
