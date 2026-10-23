//! Tile del terreno: le 56 immagini di un archivio "ground" (es. TEXTURE.302)
//! impilate come layer di un array. Porting di
//! TextureReader.GetTerrainTextureArray (solo albedo).
//!
//! In DFU l'array è un Texture2DArray ARGB32 con wrap Clamp e anisotropia 8; qui
//! resta in indici R8 (stessa mappa di palette delle altre texture) e queste
//! impostazioni di campionamento vanno decise lato renderer.

use super::error::{format_err, Result};
use super::texture::TextureFile;

pub const TILE_COUNT: usize = 56;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileArray {
    pub width: usize,
    pub height: usize,
    pub layers: usize,
    /// `layers` immagini consecutive di `width * height` indici ciascuna.
    pub indices: Vec<u8>,
}

impl TileArray {
    pub fn layer(&self, index: usize) -> &[u8] {
        let n = self.width * self.height;
        &self.indices[index * n..(index + 1) * n]
    }
}

/// DFU rifiuta (restituisce null) gli archivi che non hanno esattamente 56
/// record; qui è un errore. Come in DFU, dimensioni e passo vengono dal record 0
/// e tutti i layer devono avere la stessa dimensione.
pub fn load_tile_array(tex: &TextureFile) -> Result<TileArray> {
    if tex.record_count() != TILE_COUNT {
        return format_err(format!(
            "l'archivio ha {} record, per il terreno ne servono {TILE_COUNT}",
            tex.record_count()
        ));
    }
    let (width, height) = tex.size(0).unwrap_or((0, 0));
    let mut indices = Vec::with_capacity(width * height * TILE_COUNT);
    for record in 0..TILE_COUNT {
        let img = tex.frame(record, 0)?;
        if (img.width, img.height) != (width, height) {
            return format_err(format!(
                "la tile {record} è {}x{}, attesa {width}x{height}",
                img.width, img.height
            ));
        }
        indices.extend_from_slice(&img.indices);
    }
    Ok(TileArray {
        width,
        height,
        layers: TILE_COUNT,
        indices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{build_file, record_header};

    fn tile_record(i: u8, w: i16, h: i16) -> Vec<u8> {
        let mut rec = record_header(w, h, 0x0000, 1);
        let mut payload = vec![0u8; 256 * (h as usize - 1) + w as usize];
        for y in 0..h as usize {
            for x in 0..w as usize {
                payload[y * 256 + x] = i.wrapping_add((y * w as usize + x) as u8);
            }
        }
        rec.extend(payload);
        rec
    }

    #[test]
    fn stacks_56_layers() {
        let recs: Vec<_> = (0..56u8).map(|i| tile_record(i, 2, 2)).collect();
        let tex = TextureFile::from_bytes("TEXTURE.302", build_file(&recs)).unwrap();
        let arr = load_tile_array(&tex).unwrap();
        assert_eq!((arr.width, arr.height, arr.layers), (2, 2, 56));
        assert_eq!(arr.indices.len(), 2 * 2 * 56);
        assert_eq!(arr.layer(5), &[5, 6, 7, 8]);
        assert_eq!(arr.layer(55), &[55, 56, 57, 58]);
    }

    #[test]
    fn rejects_wrong_count_and_mixed_sizes() {
        let recs: Vec<_> = (0..55u8).map(|i| tile_record(i, 2, 2)).collect();
        let tex = TextureFile::from_bytes("TEXTURE.302", build_file(&recs)).unwrap();
        assert!(load_tile_array(&tex).is_err());

        let mut recs: Vec<_> = (0..56u8).map(|i| tile_record(i, 2, 2)).collect();
        recs[3] = tile_record(3, 3, 2);
        let tex = TextureFile::from_bytes("TEXTURE.302", build_file(&recs)).unwrap();
        assert!(load_tile_array(&tex).is_err());
    }
}
