//! CLIMATE.PAK e POLITIC.PAK: mappe del mondo 1001x500 compresse a run.
//! Porting di PakFile.cs e di MapsFile.GetClimateIndex/GetPoliticIndex.

use crate::error::{format_err, Result};
use crate::reader::Cursor;
use std::path::Path;

pub const PAK_WIDTH: usize = 1001;
pub const PAK_HEIGHT: usize = 500;

pub struct PakFile {
    buffer: Vec<u8>,
}

impl PakFile {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self::from_bytes(&name, &std::fs::read(path)?)
    }

    pub fn from_bytes(file_name: &str, data: &[u8]) -> Result<Self> {
        let upper = file_name.to_ascii_uppercase();
        if !upper.ends_with("CLIMATE.PAK") && !upper.ends_with("POLITIC.PAK") {
            return format_err(format!("{file_name}: non è CLIMATE.PAK o POLITIC.PAK"));
        }

        // 500 offset u32 all'inizio, uno per riga; ogni riga è una sequenza di
        // run (conteggio u16, valore u8) fino a coprire 1001 pixel.
        let mut offsets = Cursor::at(data, 0)?;
        let mut buffer = vec![0u8; PAK_WIDTH * PAK_HEIGHT];
        for row in 0..PAK_HEIGHT {
            let offset = offsets.u32()?;
            let mut c = Cursor::at(data, offset as i64)?;
            let base = PAK_WIDTH * row;
            let mut row_pos = 0usize;
            while row_pos < PAK_WIDTH {
                let count = c.u16()?;
                let value = c.u8()?;
                for _ in 0..count {
                    // Un run può sforare nella riga successiva (verrà riscritta),
                    // ma non oltre la fine del buffer.
                    match buffer.get_mut(base + row_pos) {
                        Some(slot) => *slot = value,
                        None => return format_err("run oltre la fine della mappa"),
                    }
                    row_pos += 1;
                }
            }
        }
        Ok(Self { buffer })
    }

    pub fn buffer(&self) -> &[u8] {
        &self.buffer
    }

    /// Valore grezzo; `None` fuori dai limiti (DFU restituisce -1).
    pub fn get_value(&self, x: i32, y: i32) -> Option<u8> {
        if x < 0 || x >= PAK_WIDTH as i32 || y < 0 || y >= PAK_HEIGHT as i32 {
            return None;
        }
        Some(self.buffer[y as usize * PAK_WIDTH + x as usize])
    }

    /// Valore per un map pixel (0..1000, 0..500): DFU aggiunge 1 alla X per
    /// allinearlo alla mappa delle altezze.
    pub fn map_pixel_value(&self, map_pixel_x: i32, map_pixel_y: i32) -> Option<u8> {
        self.get_value(map_pixel_x + 1, map_pixel_y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_pak() -> Vec<u8> {
        let mut file = vec![0u8; PAK_HEIGHT * 4];
        let mut rows = Vec::new();
        for r in 0..PAK_HEIGHT {
            let offset = (PAK_HEIGHT * 4 + rows.len()) as u32;
            file[r * 4..r * 4 + 4].copy_from_slice(&offset.to_le_bytes());
            rows.extend_from_slice(&500u16.to_le_bytes());
            rows.push((r % 256) as u8);
            rows.extend_from_slice(&501u16.to_le_bytes());
            rows.push(7);
        }
        file.extend(rows);
        file
    }

    #[test]
    fn decodes_runs_and_applies_x_offset() {
        let pak = PakFile::from_bytes("CLIMATE.PAK", &build_pak()).unwrap();
        assert_eq!(pak.get_value(0, 3), Some(3));
        assert_eq!(pak.get_value(499, 3), Some(3));
        assert_eq!(pak.get_value(500, 3), Some(7));
        assert_eq!(pak.get_value(1000, 499), Some(7));
        assert_eq!(pak.get_value(1001, 0), None);
        assert_eq!(pak.get_value(0, 500), None);
        assert_eq!(pak.get_value(-1, 0), None);
        // map pixel 499 -> colonna 500 (+1)
        assert_eq!(pak.map_pixel_value(499, 3), Some(7));
        assert_eq!(pak.map_pixel_value(498, 3), Some(3));
    }

    #[test]
    fn rejects_bad_names_and_truncated_files() {
        assert!(PakFile::from_bytes("OTHER.PAK", &build_pak()).is_err());
        let full = build_pak();
        for len in [0usize, 10, 1999, 2000, 2500, full.len() - 1] {
            assert!(PakFile::from_bytes("politic.pak", &full[..len]).is_err());
        }
    }
}
