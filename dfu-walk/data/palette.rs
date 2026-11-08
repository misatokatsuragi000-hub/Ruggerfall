//! Palette a 256 colori (.PAL da 768 byte, .COL da 776 byte con header di 8).
//! Porting di DFPalette.cs.

use super::error::{format_err, Result};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Palette {
    rgb: [[u8; 3]; 256],
}

impl Palette {
    /// `file_name` serve solo per riconoscere MAP.PAL, i cui valori (a 6 bit)
    /// vengono moltiplicati per 4 come in DFU.
    pub fn from_bytes(file_name: &str, bytes: &[u8]) -> Result<Self> {
        let header = match bytes.len() {
            768 => 0,
            776 => 8,
            n => return format_err(format!("palette di {n} byte (attesi 768 o 776)")),
        };
        let mut rgb = [[0u8; 3]; 256];
        for (i, c) in rgb.iter_mut().enumerate() {
            for (k, v) in c.iter_mut().enumerate() {
                *v = bytes[header + i * 3 + k];
            }
        }
        if file_name.eq_ignore_ascii_case("MAP.PAL") {
            for c in rgb.iter_mut() {
                for v in c.iter_mut() {
                    *v = v.wrapping_mul(4);
                }
            }
        }
        Ok(Self { rgb })
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let bytes = std::fs::read(path)?;
        Self::from_bytes(&name, &bytes)
    }

    pub fn color(&self, index: u8) -> [u8; 3] {
        self.rgb[index as usize]
    }

    /// Tabella RGBA da 256 voci, pronta per essere caricata come texture 256x1
    /// e usata nello shader come lookup. `transparent` è l'indice con alpha 0.
    pub fn to_rgba(&self, transparent: Option<u8>) -> [[u8; 4]; 256] {
        let mut out = [[0u8; 4]; 256];
        for (i, o) in out.iter_mut().enumerate() {
            let c = self.rgb[i];
            let a = if transparent == Some(i as u8) { 0 } else { 255 };
            *o = [c[0], c[1], c[2], a];
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(len_header: usize) -> Vec<u8> {
        let mut v = vec![0xEE; len_header];
        for i in 0..256usize {
            v.extend_from_slice(&[i as u8, (i + 1) as u8, (i + 2) as u8]);
        }
        v
    }

    #[test]
    fn pal_768_and_col_776_give_same_colors() {
        let a = Palette::from_bytes("PAL.PAL", &sample(0)).unwrap();
        let b = Palette::from_bytes("ART_PAL.COL", &sample(8)).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.color(5), [5, 6, 7]);
        assert_eq!(a.color(255), [255, 0, 1]);
    }

    #[test]
    fn map_pal_is_multiplied_by_four_with_wrapping() {
        let p = Palette::from_bytes("map.pal", &sample(0)).unwrap();
        assert_eq!(p.color(5), [20, 24, 28]);
        assert_eq!(p.color(100), [144, 148, 152]); // 400 mod 256 = 144
    }

    #[test]
    fn wrong_length_is_rejected() {
        assert!(Palette::from_bytes("X.PAL", &[0u8; 700]).is_err());
    }

    #[test]
    fn rgba_table_marks_transparent_index() {
        let p = Palette::from_bytes("PAL.PAL", &sample(0)).unwrap();
        let t = p.to_rgba(Some(0));
        assert_eq!(t[0][3], 0);
        assert_eq!(t[1][3], 255);
        assert_eq!(p.to_rgba(None)[0][3], 255);
    }
}
