//! Utility per l'acqua: mappa di profondità usata da `tiles.rs`.
//!
//! Il rendering dell'acqua è gestito interamente da `ocean.rs` (pipeline FFT).
//! Questo modulo esiste solo per `depth_map` e per `OCEAN_ELEVATION`
//! (riesportato da `data::heightmap`).

use crate::data::heightmap::{HeightSamples, OCEAN_ELEVATION};

/// Genera i dati di una texture di profondità (129x129) per il map pixel.
/// Restituisce `Vec<u8>` (RGBA) per compatibilità con `TileResult`.
pub fn depth_map(samples: &HeightSamples) -> Vec<u8> {
    let dim = samples.dim;
    let mut data = vec![0u8; dim * dim * 4];
    for y in 0..dim {
        for x in 0..dim {
            let h = samples.height(x, y);
            let depth = (OCEAN_ELEVATION - h).max(0.0);
            let val = (depth / 32.0 * 255.0).clamp(0.0, 255.0) as u8;
            let idx = (y * dim + x) * 4;
            data[idx] = val;
            data[idx + 1] = 0;
            data[idx + 2] = 0;
            data[idx + 3] = 255;
        }
    }
    data
}
