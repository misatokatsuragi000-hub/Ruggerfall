//! Lettore dei file TEXTURE.nnn di Daggerfall.
//! Porting di TextureFile.cs (Daggerfall Unity, MIT): stessa logica di
//! decodifica, ma l'output resta in indici di palette a 8 bit (R8) invece di
//! essere convertito in RGBA. Le righe sono nell'ordine del file (prima riga =
//! in alto): DFU le capovolge solo perché le texture di Unity hanno l'origine
//! in basso, e qui non serve.

use super::error::{format_err, Error, Result};
use super::reader::Cursor;
use super::palette::Palette;
use std::path::Path;

const SOLID_SIZE: usize = 32;
const RECORD_TABLE_POS: usize = 26;
const RECORD_TABLE_ENTRY_LEN: usize = 20;
const UNSUPPORTED: [&str; 3] = ["TEXTURE.215", "TEXTURE.217", "TEXTURE.436"];
const ROW_IS_RLE: u16 = 0x8000;

/// Nome del file di un archivio, es. 112 -> "TEXTURE.112".
pub fn archive_file_name(index: u32) -> String {
    format!("TEXTURE.{index:03}")
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Compression {
    Uncompressed,
    RleCompressed,
    ImageRle,
    RecordRle,
    Unknown(u16),
}

impl Compression {
    fn from_raw(v: u16) -> Self {
        match v {
            0x0000 => Compression::Uncompressed,
            0x0002 => Compression::RleCompressed,
            0x0108 => Compression::ImageRle,
            0x1108 => Compression::RecordRle,
            other => Compression::Unknown(other),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Solid {
    None,
    ColoursA,
    ColoursB,
}

#[derive(Clone, Debug)]
pub struct Record {
    pub offset_x: i16,
    pub offset_y: i16,
    pub width: usize,
    pub height: usize,
    pub compression: Compression,
    pub record_size: u32,
    pub data_offset: u32,
    pub is_normal: bool,
    pub frame_count: u16,
    pub scale_x: i16,
    pub scale_y: i16,
    position: usize,
}

/// Immagine a indici di palette (un byte per pixel).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedImage {
    pub width: usize,
    pub height: usize,
    pub indices: Vec<u8>,
}

impl IndexedImage {
    /// Converte in RGBA8 con la palette data. `transparent` è l'indice con alpha 0.
    pub fn to_rgba(&self, palette: &Palette, transparent: Option<u8>) -> Vec<u8> {
        let lut = palette.to_rgba(transparent);
        let mut out = Vec::with_capacity(self.indices.len() * 4);
        for &i in &self.indices {
            out.extend_from_slice(&lut[i as usize]);
        }
        out
    }
}

/// Scrittura con controllo dei limiti: DFU andrebbe in eccezione, noi restituiamo un errore.
fn put(out: &mut [u8], dst: &mut usize, v: u8) -> Result<()> {
    match out.get_mut(*dst) {
        Some(slot) => {
            *slot = v;
            *dst += 1;
            Ok(())
        }
        None => format_err("i dati decodificati superano la dimensione del record"),
    }
}

pub struct TextureFile {
    data: Vec<u8>,
    solid: Solid,
    records: Vec<Record>,
}

impl TextureFile {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let data = std::fs::read(path)?;
        Self::from_bytes(&name, data)
    }

    pub fn from_bytes(file_name: &str, data: Vec<u8>) -> Result<Self> {
        let upper = file_name.to_ascii_uppercase();
        if !upper.starts_with("TEXTURE.") {
            return format_err(format!("{file_name}: non è un file TEXTURE.nnn"));
        }
        if UNSUPPORTED.contains(&upper.as_str()) {
            return format_err(format!("{file_name}: archivio non supportato"));
        }
        let solid = match upper.as_str() {
            "TEXTURE.000" => Solid::ColoursA,
            "TEXTURE.001" => Solid::ColoursB,
            _ => Solid::None,
        };

        let mut c = Cursor::at(&data, 0)?;
        let record_count = c.i16()?;
        if record_count < 0 {
            return format_err("numero di record negativo");
        }
        let record_count = record_count as usize;

        // Tabella degli header dei record: 20 byte ciascuno, parte da 26.
        let mut positions = Vec::with_capacity(record_count);
        for r in 0..record_count {
            let base = RECORD_TABLE_POS + r * RECORD_TABLE_ENTRY_LEN;
            let mut c = Cursor::at(&data, base as i64)?;
            let _type1 = c.i16()?;
            let record_position = c.i32()?;
            positions.push(record_position);
        }

        let mut records = Vec::with_capacity(record_count);
        for &pos in &positions {
            let mut c = Cursor::at(&data, pos as i64)?;
            let offset_x = c.i16()?;
            let offset_y = c.i16()?;
            let width = c.i16()?.max(0) as usize;
            let height = c.i16()?.max(0) as usize;
            let compression = Compression::from_raw(c.u16()?);
            let record_size = c.i32()? as u32;
            let data_offset = c.i32()? as u32;
            let is_normal = c.i16()? != 0;
            let frame_count = c.u16()?;
            let _unknown = c.i16()?;
            let scale_x = c.i16()?;
            let scale_y = c.i16()?;
            let (width, height) = if solid == Solid::None {
                (width, height)
            } else {
                (SOLID_SIZE, SOLID_SIZE)
            };
            records.push(Record {
                offset_x,
                offset_y,
                width,
                height,
                compression,
                record_size,
                data_offset,
                is_normal,
                frame_count,
                scale_x,
                scale_y,
                position: pos as usize,
            });
        }

        Ok(Self {
            data,
            solid,
            records,
        })
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    pub fn record(&self, record: usize) -> Option<&Record> {
        self.records.get(record)
    }

    /// Numero di frame; gli archivi a colore pieno hanno sempre 1 frame.
    pub fn frame_count(&self, record: usize) -> Option<usize> {
        let rec = self.records.get(record)?;
        Some(if self.solid != Solid::None {
            1
        } else {
            rec.frame_count as usize
        })
    }

    pub fn size(&self, record: usize) -> Option<(usize, usize)> {
        self.records.get(record).map(|r| (r.width, r.height))
    }

    /// Decodifica un frame in indici di palette.
    pub fn frame(&self, record: usize, frame: usize) -> Result<IndexedImage> {
        let rec = self
            .records
            .get(record)
            .ok_or_else(|| Error::Format(format!("record {record} inesistente")))?;
        let frames = self.frame_count(record).unwrap_or(0);
        if frame >= frames {
            return format_err(format!(
                "frame {frame} inesistente (il record {record} ne ha {frames})"
            ));
        }

        if self.solid != Solid::None {
            let index = match self.solid {
                Solid::ColoursA => record as u8,
                _ => (128 + record) as u8,
            };
            return Ok(IndexedImage {
                width: SOLID_SIZE,
                height: SOLID_SIZE,
                indices: vec![index; SOLID_SIZE * SOLID_SIZE],
            });
        }

        let indices = match rec.compression {
            Compression::RecordRle | Compression::ImageRle => self.read_rle(rec, frame)?,
            _ => self.read_plain(rec, frame)?,
        };
        Ok(IndexedImage {
            width: rec.width,
            height: rec.height,
            indices,
        })
    }

    /// Record non compressi e `RleCompressed` (ReadImage in DFU).
    fn read_plain(&self, rec: &Record, frame: usize) -> Result<Vec<u8>> {
        let (w, h) = (rec.width, rec.height);
        let mut out = vec![0u8; w * h];
        let base = rec.position as i64 + rec.data_offset as i64;

        if rec.frame_count == 1 {
            // Le righe sono a passo fisso di 256 byte, qualunque sia la larghezza.
            let mut src = base;
            for y in 0..h {
                let mut c = Cursor::at(&self.data, src)?;
                out[y * w..(y + 1) * w].copy_from_slice(c.take(w)?);
                src += 256;
            }
        } else if rec.frame_count > 1 {
            let mut c = Cursor::at(&self.data, base)?;
            let mut offsets = Vec::with_capacity(rec.frame_count as usize);
            for _ in 0..rec.frame_count {
                offsets.push(c.i32()?);
            }
            let mut c = Cursor::at(&self.data, base + offsets[frame] as i64)?;
            let cx = c.i16()? as i32;
            let cy = c.i16()? as i32;

            // Per ogni riga: coppie (pixel trasparenti da saltare, run di pixel).
            let mut dst = 0usize;
            for _ in 0..cy {
                let mut x = 0i32;
                while x < cx {
                    let skip = c.u8()? as i32;
                    for _ in 0..skip {
                        put(&mut out, &mut dst, 0)?;
                    }
                    x += skip;
                    let run = c.u8()? as i32;
                    for _ in 0..run {
                        let p = c.u8()?;
                        put(&mut out, &mut dst, p)?;
                    }
                    x += run;
                }
            }
        } else {
            return format_err("record senza frame");
        }
        Ok(out)
    }

    /// Record `RecordRle` e `ImageRle` (ReadRle in DFU).
    fn read_rle(&self, rec: &Record, frame: usize) -> Result<Vec<u8>> {
        let (w, h) = (rec.width, rec.height);
        let mut out = vec![0u8; w * h];

        // Tabella degli header di riga di questo frame: 4 byte per riga.
        let table = rec.position as i64 + rec.data_offset as i64 + (h * frame * 4) as i64;
        let mut c = Cursor::at(&self.data, table)?;
        let mut rows = Vec::with_capacity(h);
        for _ in 0..h {
            let row_offset = c.i16()?;
            let encoding = c.u16()?;
            rows.push((row_offset, encoding));
        }

        let mut dst = 0usize;
        for (row_offset, encoding) in rows {
            let mut c = Cursor::at(&self.data, rec.position as i64 + row_offset as i64)?;
            if encoding == ROW_IS_RLE {
                let row_width = c.u16()? as usize;
                let mut row_pos = 0usize;
                loop {
                    let probe = c.i16()?;
                    if probe < 0 {
                        // Conteggio negativo: ripeti un byte N volte.
                        let n = (-(probe as i32)) as usize;
                        let px = c.u8()?;
                        for _ in 0..n {
                            put(&mut out, &mut dst, px)?;
                        }
                        row_pos += n;
                    } else if probe > 0 {
                        // Conteggio positivo: N byte letterali.
                        let n = probe as usize;
                        for &b in c.take(n)? {
                            put(&mut out, &mut dst, b)?;
                        }
                        row_pos += n;
                    }
                    if row_pos >= row_width {
                        break;
                    }
                }
            } else {
                for &b in c.take(w)? {
                    put(&mut out, &mut dst, b)?;
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{build_file, i16le, record_header};

    fn uncompressed_file() -> Vec<u8> {
        let mut rec = record_header(4, 3, 0x0000, 1);
        let mut payload = vec![0u8; 256 * 2 + 4];
        payload[0..4].copy_from_slice(&[1, 2, 3, 4]);
        payload[256..260].copy_from_slice(&[5, 6, 7, 8]);
        payload[512..516].copy_from_slice(&[9, 10, 11, 12]);
        rec.extend(payload);
        build_file(&[rec])
    }

    fn rle_compressed_file() -> Vec<u8> {
        let mut rec = record_header(4, 2, 0x0002, 2);
        let mut p = Vec::new();
        p.extend_from_slice(&8i32.to_le_bytes()); // offset frame 0
        p.extend_from_slice(&24i32.to_le_bytes()); // offset frame 1
        // frame 0
        p.extend_from_slice(&i16le(4));
        p.extend_from_slice(&i16le(2));
        p.extend_from_slice(&[1, 2, 7, 8, 1, 0]); // riga 0 -> [0,7,8,0]
        p.extend_from_slice(&[0, 4, 1, 2, 3, 4]); // riga 1 -> [1,2,3,4]
        // frame 1
        p.extend_from_slice(&i16le(4));
        p.extend_from_slice(&i16le(2));
        p.extend_from_slice(&[4, 0]); // riga 0 -> [0,0,0,0]
        p.extend_from_slice(&[2, 2, 9, 9]); // riga 1 -> [0,0,9,9]
        rec.extend(p);
        build_file(&[rec])
    }

    fn record_rle_file() -> Vec<u8> {
        let mut rec = record_header(4, 2, 0x1108, 2);
        let mut p = Vec::new();
        // tabelle di riga: frame 0 (righe a 44 e 52), frame 1 (righe a 56 e 60)
        for (off, enc) in [(44i16, 0x8000u16), (52, 0), (56, 0), (60, 0)] {
            p.extend_from_slice(&i16le(off));
            p.extend_from_slice(&enc.to_le_bytes());
        }
        // frame 0, riga 0 (RLE): larghezza 4, "-3 x 5", "1 letterale: 9"
        p.extend_from_slice(&4u16.to_le_bytes());
        p.extend_from_slice(&i16le(-3));
        p.push(5);
        p.extend_from_slice(&i16le(1));
        p.push(9);
        // frame 0, riga 1 (copia)
        p.extend_from_slice(&[1, 2, 3, 4]);
        // frame 1, righe 0 e 1 (copia)
        p.extend_from_slice(&[9, 9, 9, 9]);
        p.extend_from_slice(&[8, 7, 6, 5]);
        rec.extend(p);
        build_file(&[rec])
    }

    #[test]
    fn uncompressed_single_frame_uses_256_byte_stride() {
        let f = TextureFile::from_bytes("TEXTURE.100", uncompressed_file()).unwrap();
        let img = f.frame(0, 0).unwrap();
        assert_eq!((img.width, img.height), (4, 3));
        assert_eq!(img.indices, (1..=12).collect::<Vec<u8>>());
    }

    #[test]
    fn rle_compressed_multi_frame_skip_and_run() {
        let f = TextureFile::from_bytes("TEXTURE.101", rle_compressed_file()).unwrap();
        assert_eq!(f.frame_count(0), Some(2));
        assert_eq!(f.frame(0, 0).unwrap().indices, vec![0, 7, 8, 0, 1, 2, 3, 4]);
        assert_eq!(f.frame(0, 1).unwrap().indices, vec![0, 0, 0, 0, 0, 0, 9, 9]);
    }

    #[test]
    fn record_rle_rows_and_frames() {
        let f = TextureFile::from_bytes("TEXTURE.102", record_rle_file()).unwrap();
        assert_eq!(f.frame(0, 0).unwrap().indices, vec![5, 5, 5, 9, 1, 2, 3, 4]);
        assert_eq!(f.frame(0, 1).unwrap().indices, vec![9, 9, 9, 9, 8, 7, 6, 5]);
    }

    #[test]
    fn solid_archives_generate_flat_colour() {
        let recs: Vec<Vec<u8>> = (0..6).map(|_| record_header(0, 0, 0, 0)).collect();
        let a = TextureFile::from_bytes("TEXTURE.000", build_file(&recs)).unwrap();
        let img = a.frame(5, 0).unwrap();
        assert_eq!((img.width, img.height), (32, 32));
        assert!(img.indices.iter().all(|&i| i == 5));
        let b = TextureFile::from_bytes("texture.001", build_file(&recs)).unwrap();
        assert!(b.frame(3, 0).unwrap().indices.iter().all(|&i| i == 131));
        assert_eq!(b.frame_count(3), Some(1));
    }

    #[test]
    fn invalid_names_and_indices_are_rejected() {
        assert!(TextureFile::from_bytes("TEXTURE.215", uncompressed_file()).is_err());
        assert!(TextureFile::from_bytes("ARCH3D.BSA", uncompressed_file()).is_err());
        let f = TextureFile::from_bytes("TEXTURE.100", uncompressed_file()).unwrap();
        assert!(f.frame(1, 0).is_err());
        assert!(f.frame(0, 1).is_err());
    }

    #[test]
    fn truncated_files_never_panic() {
        for full in [uncompressed_file(), rle_compressed_file(), record_rle_file()] {
            for len in 0..full.len() {
                let cut = full[..len].to_vec();
                if let Ok(f) = TextureFile::from_bytes("TEXTURE.100", cut) {
                    for fr in 0..2 {
                        let _ = f.frame(0, fr);
                    }
                }
            }
        }
    }

    #[test]
    fn indexed_image_converts_through_palette() {
        let f = TextureFile::from_bytes("TEXTURE.100", uncompressed_file()).unwrap();
        let mut pal = vec![0u8; 768];
        pal[3..6].copy_from_slice(&[10, 20, 30]); // indice 1
        let pal = Palette::from_bytes("PAL.PAL", &pal).unwrap();
        let rgba = f.frame(0, 0).unwrap().to_rgba(&pal, Some(0));
        assert_eq!(rgba.len(), 12 * 4);
        assert_eq!(&rgba[0..4], &[10, 20, 30, 255]); // primo pixel: indice 1
    }
}
