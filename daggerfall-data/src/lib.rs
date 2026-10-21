//! Lettori dei formati di dati di Daggerfall, portati da Daggerfall Unity (MIT).
//! Vedi il file NOTICE per l'attribuzione.

pub mod atlas;
pub mod climate;
pub mod error;
pub mod heightmap;
pub mod pak;
pub mod palette;
pub mod perlin;
pub mod png;
pub mod sky;
mod reader;
pub mod terrain;
pub mod terrain_mesh;
pub mod tilemap;
pub mod texture;
pub mod woods;

#[cfg(test)]
mod test_support;

use std::path::{Path, PathBuf};

pub use error::{Error, Result};
pub use climate::{apply_climate, ground_archive, world_climate_settings, ClimateBase, Season};
pub use heightmap::{generate_default_samples, generate_samples, HeightSamples};
pub use pak::PakFile;
pub use palette::Palette;
pub use terrain::{load_tile_array, TileArray};
pub use tilemap::{encode_for_shader, generate_default_tilemap, generate_tilemap, lookup_table};
pub use texture::{archive_file_name, IndexedImage, TextureFile};
pub use woods::{ByteGrid, WoodsFile};

/// Cerca un file in una cartella ignorando maiuscole/minuscole: su Linux gli
/// ARENA2 possono avere nomi minuscoli (texture.112) o maiuscoli (TEXTURE.112).
pub fn find_file_ci(dir: impl AsRef<Path>, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let file_name = e.file_name();
        file_name
            .to_string_lossy()
            .eq_ignore_ascii_case(name)
            .then(|| e.path())
    })
}
