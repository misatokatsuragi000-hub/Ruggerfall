use crate::bsa::BsaArchive;
use crate::blocks::RmbBlock;
use crate::maps::DFLocation;

#[derive(Debug, Clone, Copy)]
pub struct CityTileRect {
    pub min_x: usize,
    pub min_y: usize,
    pub max_x: usize,
    pub max_y: usize,
}

impl CityTileRect {
    pub fn contains_tile(&self, x: usize, y: usize) -> bool {
        x >= self.min_x && x < self.max_x && y >= self.min_y && y < self.max_y
    }
}

pub fn assemble_city_tilemap(
    location: &DFLocation,
    blocks_bsa: &BsaArchive,
) -> (Vec<u8>, CityTileRect) {
    let mut preset = vec![0u8; 128 * 128];

    let start_block_x = (8 - location.width.min(8)) / 2;
    let start_block_y = (8 - location.height.min(8)) / 2;

    let rect = CityTileRect {
        min_x: start_block_x * 16,
        min_y: start_block_y * 16,
        max_x: (start_block_x + location.width.min(8)) * 16,
        max_y: (start_block_y + location.height.min(8)) * 16,
    };

    for by in 0..location.height.min(8) {
        for bx in 0..location.width.min(8) {
            let block_idx = by * location.width + bx;
            if block_idx >= location.block_names.len() { continue; }
            let block_name = &location.block_names[block_idx];

            if let Some(rmb_raw) = blocks_bsa.get_by_name(block_name) {
                if let Ok(rmb) = RmbBlock::parse(rmb_raw) {
                    for ty in 0..16 {
                        for tx in 0..16 {
                            let gx = (start_block_x + bx) * 16 + tx;
                            let gy = (start_block_y + by) * 16 + ty;
                            if gx < 128 && gy < 128 {
                                preset[gy * 128 + gx] = rmb.ground_tiles[ty * 16 + tx];
                            }
                        }
                    }
                }
            }
        }
    }

    (preset, rect)
}

/// Quanti campioni, dal bordo del chunk, restano sull'altezza naturale per
/// non spezzare la cucitura con i chunk vicini.
const EDGE_BLEND_SAMPLES: f32 = 8.0;

fn smoothstep01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Appiattisce il terreno sotto `rect` (in celle), con raccordo di `blend_samples`
/// campioni verso l'esterno. I bordi del chunk restano all'altezza naturale.
/// `heights` ha lo stesso layout di `HeightSamples::data`: indice `y + x * dim`
/// (x a est, y a nord). Ritorna la quota piatta (campione grezzo, non in metri).
pub fn flatten_terrain_heights(
    heights: &mut [f32],
    dim: usize,
    rect: &CityTileRect,
    blend_samples: f32,
) -> f32 {
    if dim < 2 || heights.len() != dim * dim {
        return 0.0;
    }
    let at = |x: usize, y: usize| y + x * dim;
    let (x0, x1) = (rect.min_x.min(dim - 1), rect.max_x.min(dim - 1));
    let (y0, y1) = (rect.min_y.min(dim - 1), rect.max_y.min(dim - 1));

    let mut sum = 0.0f32;
    let mut count = 0usize;
    for x in x0..=x1 {
        for y in y0..=y1 {
            sum += heights[at(x, y)];
            count += 1;
        }
    }
    let y_flat = if count > 0 { sum / count as f32 } else { 0.0 };
    let blend = blend_samples.max(1.0);

    for x in 0..dim {
        for y in 0..dim {
            let dx = if x < x0 { x0 - x } else if x > x1 { x - x1 } else { 0 } as f32;
            let dy = if y < y0 { y0 - y } else if y > y1 { y - y1 } else { 0 } as f32;
            let dist = (dx * dx + dy * dy).sqrt();
            let w_city = 1.0 - smoothstep01(dist / blend);
            if w_city <= 0.0 {
                continue;
            }
            let edge_dist = x.min(y).min(dim - 1 - x).min(dim - 1 - y) as f32;
            let w = w_city * smoothstep01(edge_dist / EDGE_BLEND_SAMPLES);
            let h = heights[at(x, y)];
            heights[at(x, y)] = h * (1.0 - w) + y_flat * w;
        }
    }
    y_flat
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_only_under_rect_and_keeps_chunk_edges() {
        let dim = 129;
        let mut h: Vec<f32> = (0..dim * dim).map(|i| (i % 7) as f32 * 0.01).collect();
        let original = h.clone();
        let rect = CityTileRect { min_x: 48, min_y: 48, max_x: 80, max_y: 80 };
        let y_flat = flatten_terrain_heights(&mut h, dim, &rect, 12.0);

        // dentro il rettangolo: quota costante
        for x in 48..=80 {
            for y in 48..=80 {
                assert!((h[y + x * dim] - y_flat).abs() < 1e-5);
            }
        }
        // lontano dal rettangolo e sui bordi: invariato
        assert_eq!(h[0], original[0]);
        assert_eq!(h[10 + 10 * dim], original[10 + 10 * dim]);
        assert_eq!(h[(dim - 1) + (dim - 1) * dim], original[(dim - 1) + (dim - 1) * dim]);
    }

    #[test]
    fn flatten_asymmetric_rect_uses_xy_layout() {
        let dim = 129;
        let mut h = vec![0.0f32; dim * dim];
        // rampa lungo x (indice x*dim): se x e y fossero scambiati il test fallisce
        for x in 0..dim {
            for y in 0..dim {
                h[y + x * dim] = x as f32 * 0.01;
            }
        }
        let rect = CityTileRect { min_x: 20, min_y: 60, max_x: 40, max_y: 70 };
        flatten_terrain_heights(&mut h, dim, &rect, 4.0);
        // dentro il rettangolo (x=25, y=65): appiattito alla media delle x 20..=40
        assert!((h[65 + 25 * dim] - 0.30).abs() < 1e-5);
        // stessa y ma x lontana dal rettangolo: ancora sulla rampa
        assert!((h[65 + 60 * dim] - 0.60).abs() < 1e-5);
        // stessa x ma y lontana dal rettangolo: ancora sulla rampa
        assert!((h[100 + 25 * dim] - 0.25).abs() < 1e-5);
    }
}
