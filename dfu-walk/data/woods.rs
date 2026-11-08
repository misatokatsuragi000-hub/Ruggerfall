//! WOODS.WLD: mappa delle altezze del mondo (1000x500) e dati di dettaglio 5x5
//! per ogni map pixel. Porting di WoodsFile.cs.

use super::error::{format_err, Result};
use super::reader::Cursor;
use std::path::Path;

pub const MAP_WIDTH: usize = 1000;
pub const MAP_HEIGHT: usize = 500;
const MAP_LEN: usize = MAP_WIDTH * MAP_HEIGHT;
const HEADER_FIELDS_AFTER_FIRST: usize = 28;

/// Griglia quadrata di byte; l'elemento (x, y) sta in `data[x + y * side]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByteGrid {
    pub side: usize,
    pub data: Vec<u8>,
}

impl ByteGrid {
    pub fn get(&self, x: usize, y: usize) -> u8 {
        self.data[x + y * self.side]
    }
}

pub struct WoodsFile {
    data: Vec<u8>,
    offsets: Vec<u32>,
    height_map: Vec<u8>,
}

impl WoodsFile {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self::from_bytes(&name, std::fs::read(path)?)
    }

    pub fn from_bytes(file_name: &str, data: Vec<u8>) -> Result<Self> {
        if !file_name.to_ascii_uppercase().ends_with("WOODS.WLD") {
            return format_err(format!("{file_name}: non è WOODS.WLD"));
        }

        // Header: 8 u32 + 28 u32 nulli; poi la tabella degli offset (un u32 per pixel).
        let mut c = Cursor::at(&data, 0)?;
        let _offset_size = c.u32()?;
        let width = c.u32()?;
        let height = c.u32()?;
        let _null1 = c.u32()?;
        let _data_section1 = c.u32()?;
        let _unknown1 = c.u32()?;
        let _unknown2 = c.u32()?;
        let height_map_offset = c.u32()?;
        for _ in 0..HEADER_FIELDS_AFTER_FIRST {
            c.u32()?;
        }
        if width as usize * height as usize != MAP_LEN {
            return format_err("WOODS.WLD: width*height diverso da 1000*500");
        }
        let mut offsets = Vec::with_capacity(MAP_LEN);
        for _ in 0..MAP_LEN {
            offsets.push(c.u32()?);
        }

        let mut c = Cursor::at(&data, height_map_offset as i64)?;
        let height_map = c.take(MAP_LEN)?.to_vec();

        Ok(Self {
            data,
            offsets,
            height_map,
        })
    }

    pub fn height_map(&self) -> &[u8] {
        &self.height_map
    }

    /// Altezza di un map pixel; le coordinate fuori mappa vengono bloccate sul bordo.
    pub fn height_value(&self, map_pixel_x: i32, map_pixel_y: i32) -> u8 {
        let x = map_pixel_x.clamp(0, MAP_WIDTH as i32 - 1) as usize;
        let y = map_pixel_y.clamp(0, MAP_HEIGHT as i32 - 1) as usize;
        self.height_map[y * MAP_WIDTH + x]
    }

    /// Blocco `dim` x `dim` di altezze, elemento (x, y) in `[x + y * dim]`.
    pub fn height_values_range_1dim(&self, map_pixel_x: i32, map_pixel_y: i32, dim: usize) -> Vec<u8> {
        let mut out = vec![0u8; dim * dim];
        for y in 0..dim {
            for x in 0..dim {
                out[x + y * dim] = self.height_value(map_pixel_x + x as i32, map_pixel_y + y as i32);
            }
        }
        out
    }

    /// Dettaglio 5x5 di un map pixel: `[x][y]`, letto con y esterno e x interno
    /// a partire da `offset + 22`.
    pub fn large_map_data(&self, map_pixel_x: i32, map_pixel_y: i32) -> Result<[[u8; 5]; 5]> {
        let x = map_pixel_x.clamp(0, MAP_WIDTH as i32 - 1) as usize;
        let y = map_pixel_y.clamp(0, MAP_HEIGHT as i32 - 1) as usize;
        let pos = self.offsets[y * MAP_WIDTH + x] as i64 + 22;
        let mut c = Cursor::at(&self.data, pos)?;
        let mut out = [[0u8; 5]; 5];
        for y in 0..5 {
            for x in 0..5 {
                out[x][y] = c.u8()?;
            }
        }
        Ok(out)
    }

    /// Altezze di dettaglio per `dim` x `dim` map pixel (3x3 campioni ciascuno),
    /// come `GetLargeHeightMapValuesRange`: la Y dei map pixel scende e i 5x5
    /// vengono capovolti in verticale.
    pub fn large_height_values_range(&self, map_pixel_x: i32, map_pixel_y: i32, dim: usize) -> Result<ByteGrid> {
        const OFFSET: usize = 1;
        const LEN: usize = 3;
        let side = dim * LEN;
        let mut data = vec![0u8; side * side];
        for y in 0..dim {
            for x in 0..dim {
                let src = self.large_map_data(map_pixel_x + x as i32, map_pixel_y - y as i32)?;
                let (start_x, start_y) = (x * LEN, y * LEN);
                for iy in OFFSET..OFFSET + LEN {
                    for ix in OFFSET..OFFSET + LEN {
                        data[(start_x + ix - OFFSET) + (start_y + iy - OFFSET) * side] = src[ix][4 - iy];
                    }
                }
            }
        }
        Ok(ByteGrid { side, data })
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::collections::HashMap;

    /// WOODS.WLD sintetico: altezza = f(x, y); i blocchi 5x5 dei pixel indicati
    /// contengono `10*y + x`, gli altri sono a zero.
    pub(crate) fn build_woods(height: impl Fn(usize, usize) -> u8, detail: &[(usize, usize)]) -> Vec<u8> {
        build_woods_filled(height, detail, 0)
    }

    /// Come `build_woods`, ma il blocco di dettaglio comune a tutti gli altri pixel
    /// è riempito con `fill`.
    pub(crate) fn build_woods_filled(
        height: impl Fn(usize, usize) -> u8,
        detail: &[(usize, usize)],
        fill: u8,
    ) -> Vec<u8> {
        let header_len = 4 * (8 + HEADER_FIELDS_AFTER_FIRST);
        let table_len = 4 * MAP_LEN;
        let zero_block = header_len + table_len;
        let mut blocks: Vec<u8> = vec![0u8; 22];
        blocks.extend_from_slice(&[fill; 25]);
        let mut block_pos: HashMap<(usize, usize), usize> = HashMap::new();
        for &(px, py) in detail {
            block_pos.insert((px, py), zero_block + blocks.len());
            blocks.extend_from_slice(&[0u8; 22]);
            for y in 0..5u8 {
                for x in 0..5u8 {
                    blocks.push(10 * y + x);
                }
            }
        }
        let height_map_offset = zero_block + blocks.len();

        let mut f = Vec::new();
        let fields = [0u32, 1000, 500, 0, 0, 0, 0, height_map_offset as u32];
        for v in fields {
            f.extend_from_slice(&v.to_le_bytes());
        }
        f.extend_from_slice(&vec![0u8; 4 * HEADER_FIELDS_AFTER_FIRST]);
        for y in 0..MAP_HEIGHT {
            for x in 0..MAP_WIDTH {
                let off = block_pos.get(&(x, y)).copied().unwrap_or(zero_block);
                f.extend_from_slice(&(off as u32).to_le_bytes());
            }
        }
        f.extend(blocks);
        for y in 0..MAP_HEIGHT {
            for x in 0..MAP_WIDTH {
                f.push(height(x, y));
            }
        }
        f
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::build_woods;
    use super::*;

    #[test]
    fn heights_are_read_and_clamped() {
        let f = WoodsFile::from_bytes("WOODS.WLD", build_woods(|x, y| ((x + 2 * y) % 251) as u8, &[])).unwrap();
        assert_eq!(f.height_value(10, 20), 50);
        assert_eq!(f.height_value(-5, -5), f.height_value(0, 0));
        assert_eq!(f.height_value(5000, 5000), f.height_value(999, 499));
        let block = f.height_values_range_1dim(3, 4, 2);
        assert_eq!(block, vec![11, 12, 13, 14]); // (3,4)=11 (4,4)=12 (3,5)=13 (4,5)=14
    }

    #[test]
    fn detail_block_is_read_with_x_inner_order() {
        let f = WoodsFile::from_bytes("woods.wld", build_woods(|_, _| 0, &[(100, 200)])).unwrap();
        let d = f.large_map_data(100, 200).unwrap();
        assert_eq!(d[3][2], 23); // 10*y + x con x=3, y=2
        assert_eq!(d[0][4], 40);
        assert_eq!(f.large_map_data(0, 0).unwrap(), [[0u8; 5]; 5]);
    }

    #[test]
    fn large_range_flips_vertically_and_walks_pixels_downwards() {
        let f = WoodsFile::from_bytes("WOODS.WLD", build_woods(|_, _| 0, &[(10, 10), (11, 10), (10, 9)])).unwrap();
        let g = f.large_height_values_range(10, 10, 1).unwrap();
        assert_eq!(g.side, 3);
        // dst[x][y] = src[x+1][3-y] con src[x][y] = 10*y + x
        assert_eq!(g.get(0, 0), 31);
        assert_eq!(g.get(2, 0), 33);
        assert_eq!(g.get(0, 2), 11);
        // con dim = 2 il secondo blocco in X è il pixel 11, quello in Y è il pixel 9
        let g2 = f.large_height_values_range(10, 10, 2).unwrap();
        assert_eq!(g2.side, 6);
        assert_eq!(g2.get(3, 0), 31);
        assert_eq!(g2.get(0, 3), 31);
    }

    #[test]
    fn invalid_files_are_rejected() {
        assert!(WoodsFile::from_bytes("MAPS.BSA", vec![0u8; 200]).is_err());
        assert!(WoodsFile::from_bytes("WOODS.WLD", vec![0u8; 200]).is_err());
        let good = build_woods(|_, _| 1, &[]);
        assert!(WoodsFile::from_bytes("WOODS.WLD", good[..good.len() - 1].to_vec()).is_err());
    }
}
