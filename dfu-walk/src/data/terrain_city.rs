//! Generazione del tilemap urbano a partire dai dati RMB (BLOCKS.BSA).
//! Converte i blocchi definiti in BLOCKS.BSA in una griglia 128x128 di tile
//! compatibile con `tilemap.rs` (porting di TerrainHelper.SetLocationTiles di
//! Daggerfall Unity).

use super::blocks::RmbBlock;
use super::bsa::BsaArchive;
use super::maps::DFLocation;
use super::tilemap::UNSET_TILE;

/// Orientamento verticale della tilemap.
///
/// * `false` = Y verso NORD, come Daggerfall Unity: i blocchi restano in ordine
///   naturale e si inverte solo l'interno di ciascun blocco.
/// * `true`  = Y verso SUD (mondo specchiato): si inverte l'ordine delle righe di
///   blocchi e l'interno del blocco resta com'è.
///
/// Se la città appare specchiata nord/sud rispetto agli edifici, cambia questo valore.
const TILEMAP_Y_SOUTHWARD: bool = true;

/// Un tile RMB è valido se il record (6 bit bassi) è < 56. Il record 0 è acqua
/// (fossati, canali): va mantenuto. I valori > 55 indicano "blend con il terreno".
const MAX_CITY_RECORD: u8 = 56;

/// Area occupata dalla città nella griglia 128x128 del map pixel.
/// `max_*` sono esclusivi.
#[derive(Debug, Clone, Copy)]
pub struct CityTileRect {
    pub min_x: usize,
    pub min_y: usize,
    pub max_x: usize,
    pub max_y: usize,
}

impl CityTileRect {
    #[inline]
    pub fn contains_tile(&self, x: usize, y: usize) -> bool {
        x >= self.min_x && x < self.max_x && y >= self.min_y && y < self.max_y
    }
}

/// Assembla il `preset` di tile per una località cittadina.
///
/// Ritorna `(preset, rect)`: `preset` è l'array 128*128 con il byte tile
/// (record + bit rotate 0x40 / flip 0x80) oppure `UNSET_TILE` dove decide il
/// terreno naturale; `rect` è l'area della città per l'appiattimento dell'altezza.
pub fn assemble_city_tilemap(
    location: &DFLocation,
    blocks_bsa: &BsaArchive,
) -> (Vec<u8>, CityTileRect) {
    let mut preset = vec![UNSET_TILE; 128 * 128];

    let bw = location.width.min(8);
    let bh = location.height.min(8);

    // Origine in TILE (come DFU: (128 - w*16) / 2), non in blocchi: per larghezze
    // dispari la divisione in blocchi perderebbe mezzo blocco (8 tile).
    let ox = (128 - bw * 16) / 2;
    let oy = (128 - bh * 16) / 2;

    let rect = CityTileRect {
        min_x: ox,
        min_y: oy,
        max_x: ox + bw * 16,
        max_y: oy + bh * 16,
    };

    for by in 0..bh {
        for bx in 0..bw {
            // block_names è row-major da Nord-Ovest (vedi maps.rs: y * width + x)
            let block_idx = by * location.width + bx;
            let Some(block_name) = location.block_names.get(block_idx) else {
                continue;
            };
            let Some(rmb_raw) = blocks_bsa.get_by_name(block_name) else {
                continue;
            };
            let Ok(rmb) = RmbBlock::parse(rmb_raw) else {
                continue;
            };

            // UNA sola inversione di Y (vedi TILEMAP_Y_SOUTHWARD).
            let block_row = if TILEMAP_Y_SOUTHWARD { bh - 1 - by } else { by };

            for ty in 0..16 {
                for tx in 0..16 {
                    let src_row = if TILEMAP_Y_SOUTHWARD { ty } else { 15 - ty };

                    let gx = ox + bx * 16 + tx;
                    let gy = oy + block_row * 16 + ty;
                    if gx >= 128 || gy >= 128 {
                        continue;
                    }

                    // ground_tiles è riga per riga: [riga * 16 + colonna]
                    let raw_tile = rmb.ground_tiles[src_row * 16 + tx];

                    // DFU: TextureRecord < 56, incluso 0 (acqua).
                    // 0 non collide con UNSET_TILE (0xFF).
                    if (raw_tile & 0x3F) < MAX_CITY_RECORD {
                        preset[gy * 128 + gx] = raw_tile;
                    }
                }
            }
        }
    }
    for y in rect.min_y..rect.max_y {
        let row: Vec<String> = (rect.min_x..rect.max_x)
        .map(|x| format!("{:02X}", preset[y * 128 + x]))
        .collect();
        eprintln!("{}", row.join(" "));
    }
    (preset, rect)
}

// =============================================================================
// APPIATTIMENTO DEL TERRENO (HEIGHTMAP FLATTENING)
// =============================================================================

const EDGE_BLEND_SAMPLES: f32 = 8.0;

fn smoothstep01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Appiattisce l'heightmap sotto la città e sfuma verso il terreno naturale.
/// Ritorna l'altezza della piattaforma.
pub fn flatten_terrain_heights(
    heights: &mut [f32],
    dim: usize,
    rect: &CityTileRect,
    blend_samples: f32,
) -> f32 {
    if dim < 2 || heights.len() != dim * dim {
        return 0.0;
    }

    // Attenzione: questo è indicizzato per colonne (x * dim + y). Verifica che
    // coincida con HeightSamples::get(x, y); in caso contrario scambia gli operandi.
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
            let dx = if x < x0 {
                (x0 - x) as f32
            } else if x > x1 {
                (x - x1) as f32
            } else {
                0.0
            };
            let dy = if y < y0 {
                (y0 - y) as f32
            } else if y > y1 {
                (y - y1) as f32
            } else {
                0.0
            };

            let dist = (dx * dx + dy * dy).sqrt();
            let w_city = 1.0 - smoothstep01(dist / blend);
            if w_city <= 0.0 {
                continue;
            }

            let edge_dist = x
                .min(y)
                .min(dim.saturating_sub(1 + x))
                .min(dim.saturating_sub(1 + y)) as f32;
            let w_edge = smoothstep01(edge_dist / EDGE_BLEND_SAMPLES);

            let w_final = w_city * w_edge;
            let idx = at(x, y);
            heights[idx] = heights[idx] * (1.0 - w_final) + y_flat * w_final;
        }
    }

    y_flat
}
