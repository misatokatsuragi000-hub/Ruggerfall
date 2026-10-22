//! Atlante RGBA delle 56 tile del terreno (8 colonne x 7 righe) con mipmap
//! limitate, pensato per un materiale standard senza shader personalizzati.
//!
//! Le tile mantengono l'orientamento del file (riga 0 = in alto). I mipmap sono
//! al massimo 4 livelli, così i bordi delle tile non si mescolano.

use crate::palette::Palette;
use crate::terrain::{TileArray, TILE_COUNT};

pub const ATLAS_COLS: usize = 8;
pub const ATLAS_ROWS: usize = 7;
const MAX_MIP_LEVELS: u32 = 4;

pub struct Atlas {
    pub tile_w: usize,
    pub tile_h: usize,
    pub width: usize,
    pub height: usize,
    /// RGBA8; il livello 0 è l'immagine intera, gli altri sono dimezzati.
    pub levels: Vec<Vec<u8>>,
}

impl Atlas {
    pub fn mip_count(&self) -> u32 {
        self.levels.len() as u32
    }

    /// Tutti i livelli uno dopo l'altro, come li vuole una texture con mipmap.
    pub fn mip_data(&self) -> Vec<u8> {
        self.levels.concat()
    }

    /// Posizione in pixel dell'angolo in alto a sinistra di una tile.
    pub fn tile_origin(&self, tile: usize) -> (usize, usize) {
        ((tile % ATLAS_COLS) * self.tile_w, (tile / ATLAS_COLS) * self.tile_h)
    }
}

/// `target_tile` forza la dimensione delle tile (ridimensionamento "nearest"),
/// utile se archivi diversi non hanno la stessa risoluzione.
pub fn build_atlas(tiles: &TileArray, palette: &Palette, target_tile: Option<(usize, usize)>) -> Atlas {
    let (tile_w, tile_h) = target_tile.unwrap_or((tiles.width, tiles.height));
    let (width, height) = (tile_w * ATLAS_COLS, tile_h * ATLAS_ROWS);
    let lut = palette.to_rgba(None);

    let mut base = vec![0u8; width * height * 4];
    for t in 0..TILE_COUNT.min(tiles.layers) {
        let layer = tiles.layer(t);
        let (ox, oy) = ((t % ATLAS_COLS) * tile_w, (t / ATLAS_COLS) * tile_h);
        for y in 0..tile_h {
            let sy = y * tiles.height / tile_h;
            for x in 0..tile_w {
                let sx = x * tiles.width / tile_w;
                let c = lut[layer[sx + sy * tiles.width] as usize];
                let p = ((oy + y) * width + ox + x) * 4;
                base[p..p + 4].copy_from_slice(&c);
            }
        }
    }

    let extra = MAX_MIP_LEVELS
        .saturating_sub(1)
        .min(tile_w.trailing_zeros())
        .min(tile_h.trailing_zeros());
    let mut levels = vec![base];
    let (mut w, mut h) = (width, height);
    for _ in 0..extra {
        let prev = levels.last().unwrap();
        let (nw, nh) = (w / 2, h / 2);
        let mut next = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let at = |dx: usize, dy: usize| prev[((y * 2 + dy) * w + x * 2 + dx) * 4 + c] as u32;
                    next[(y * nw + x) * 4 + c] = ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8;
                }
            }
        }
        levels.push(next);
        w = nw;
        h = nh;
    }

    Atlas {
        tile_w,
        tile_h,
        width,
        height,
        levels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        let mut p = vec![0u8; 768];
        for i in 0..256 {
            p[i * 3] = i as u8;
            p[i * 3 + 1] = 255 - i as u8;
            p[i * 3 + 2] = 7;
        }
        Palette::from_bytes("PAL.PAL", &p).unwrap()
    }

    /// 56 tile `w`x`h` in cui ogni pixel vale `tile` (cioè indice = numero di tile).
    fn tiles(w: usize, h: usize) -> TileArray {
        let mut indices = Vec::new();
        for t in 0..TILE_COUNT {
            indices.extend(std::iter::repeat(t as u8).take(w * h));
        }
        TileArray {
            width: w,
            height: h,
            layers: TILE_COUNT,
            indices,
        }
    }

    #[test]
    fn tiles_are_placed_in_an_8_by_7_grid() {
        let a = build_atlas(&tiles(8, 8), &palette(), None);
        assert_eq!((a.width, a.height), (64, 56));
        let px = |x: usize, y: usize| {
            let p = (y * a.width + x) * 4;
            [a.levels[0][p], a.levels[0][p + 1], a.levels[0][p + 2], a.levels[0][p + 3]]
        };
        assert_eq!(px(0, 0), [0, 255, 7, 255]); // tile 0
        assert_eq!(px(8, 0), [1, 254, 7, 255]); // tile 1: seconda colonna
        assert_eq!(px(0, 8), [8, 247, 7, 255]); // tile 8: seconda riga
        assert_eq!(px(63, 55), [55, 200, 7, 255]); // tile 55: ultimo pixel
        assert_eq!(a.tile_origin(9), (8, 8));
    }

    #[test]
    fn mip_chain_has_four_levels_for_64px_tiles() {
        let a = build_atlas(&tiles(64, 64), &palette(), None);
        assert_eq!(a.mip_count(), 4);
        assert_eq!((a.width, a.height), (512, 448));
        let sizes: Vec<usize> = a.levels.iter().map(|l| l.len()).collect();
        assert_eq!(sizes, vec![512 * 448 * 4, 256 * 224 * 4, 128 * 112 * 4, 64 * 56 * 4]);
        assert_eq!(a.mip_data().len(), sizes.iter().sum::<usize>());
    }

    #[test]
    fn odd_tile_sizes_get_fewer_mips() {
        assert_eq!(build_atlas(&tiles(6, 6), &palette(), None).mip_count(), 2);
        assert_eq!(build_atlas(&tiles(5, 5), &palette(), None).mip_count(), 1);
    }

    #[test]
    fn mips_average_inside_each_tile_and_tiles_can_be_resized() {
        let a = build_atlas(&tiles(8, 8), &palette(), None);
        // a livello 1 la tile 3 occupa 4x4 pixel e resta uniforme
        let w1 = a.width / 2;
        let p = ((0 * w1) + 3 * 4) * 4;
        assert_eq!(&a.levels[1][p..p + 3], &[3, 252, 7]);
        // ridimensionata a 4x4 (nearest)
        let r = build_atlas(&tiles(8, 8), &palette(), Some((4, 4)));
        assert_eq!((r.tile_w, r.width, r.height), (4, 32, 28));
    }
}
