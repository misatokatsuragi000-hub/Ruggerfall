//! High-level access to a `.dfmod`: asset listing by container path, text/binary assets, textures.

use std::collections::HashMap;
use std::path::Path;

use crate::bundle::Bundle;
use crate::error::{Error, Result};
use crate::key::TextureKey;
use crate::serialized::{class_id, SerializedFile, Value};
use crate::texture::{Texture2D, Texture2DArray};

/// One entry of the bundle's `AssetBundle.m_Container` (asset path -> object).
#[derive(Debug, Clone)]
pub struct AssetEntry {
    /// Lower-case path as stored by Unity, e.g. `assets/game/mods/foo/textures/301_0-0.png`.
    pub path: String,
    pub path_id: i64,
    pub class_id: i32,
    file_index: usize,
}

pub struct ModArchive {
    bundle: Bundle,
    /// (index of the node in the bundle, parsed serialized file)
    files: Vec<(usize, SerializedFile)>,
    assets: Vec<AssetEntry>,
    /// lower-case file stem -> index into `assets` (Texture2D entries only; last one wins).
    textures: HashMap<String, usize>,
    /// lower-case file stem -> index into `assets` (Texture2DArray entries, class 187).
    texture_arrays: HashMap<String, usize>,
}

impl ModArchive {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_bundle(Bundle::open(path)?)
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        Self::from_bundle(Bundle::from_bytes(bytes)?)
    }

    pub fn from_bundle(bundle: Bundle) -> Result<Self> {
        let mut files = Vec::new();
        for (ni, node) in bundle.nodes().iter().enumerate() {
            if node.is_serialized_file() {
                files.push((ni, SerializedFile::parse(&bundle, node)?));
            }
        }

        let mut assets = Vec::new();
        for (fi, (ni, sf)) in files.iter().enumerate() {
            let node = &bundle.nodes()[*ni];
            for ab in sf.objects_of_class(class_id::ASSET_BUNDLE) {
                let v = sf.read_object(&bundle, node, ab)?;
                let Some(container) = v.get("m_Container").and_then(Value::as_array) else {
                    continue;
                };
                for e in container {
                    let Some(path) = e.get("first").and_then(Value::as_str) else { continue };
                    let Some(asset) = e.get("second").and_then(|s| s.get("asset")) else { continue };
                    let file_id = asset.get("m_FileID").and_then(Value::as_i64).unwrap_or(0);
                    let Some(path_id) = asset.get("m_PathID").and_then(Value::as_i64) else { continue };
                    if file_id != 0 {
                        continue; // lives in another file (dependency bundle)
                    }
                    if let Some(obj) = sf.object(path_id) {
                        assets.push(AssetEntry {
                            path: path.to_owned(),
                            path_id,
                            class_id: obj.class_id,
                            file_index: fi,
                        });
                    }
                }
            }
        }

        let mut textures = HashMap::new();
        let mut texture_arrays = HashMap::new();
        for (i, a) in assets.iter().enumerate() {
            if a.class_id == class_id::TEXTURE_2D {
                textures.insert(file_stem_lower(&a.path), i);
            } else if a.class_id == class_id::TEXTURE_2D_ARRAY {
                texture_arrays.insert(file_stem_lower(&a.path), i);
            }
        }
        Ok(Self { bundle, files, assets, textures, texture_arrays })
    }

    pub fn bundle(&self) -> &Bundle {
        &self.bundle
    }

    pub fn assets(&self) -> &[AssetEntry] {
        &self.assets
    }

    pub fn serialized_files(&self) -> impl Iterator<Item = &SerializedFile> {
        self.files.iter().map(|(_, sf)| sf)
    }

    fn find(&self, path: &str) -> Option<&AssetEntry> {
        let p = path.to_ascii_lowercase();
        self.assets.iter().find(|a| a.path == p)
    }

    fn read_entry(&self, a: &AssetEntry) -> Result<Value> {
        let (ni, sf) = &self.files[a.file_index];
        let node = &self.bundle.nodes()[*ni];
        let obj = sf.object(a.path_id).ok_or(Error::ObjectNotFound(a.path_id))?;
        sf.read_object(&self.bundle, node, obj)
    }

    pub fn read_asset(&self, path: &str) -> Result<Option<Value>> {
        self.find(path).map(|a| self.read_entry(a)).transpose()
    }

    pub fn text_asset(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let Some(v) = self.read_asset(path)? else { return Ok(None) };
        Ok(v.get("m_Script").and_then(Value::as_bytes).map(<[u8]>::to_vec))
    }

    pub fn texture_names(&self) -> impl Iterator<Item = &str> {
        self.textures.keys().map(String::as_str)
    }

    pub fn texture_keys(&self) -> impl Iterator<Item = (TextureKey, &str)> {
        self.texture_names().filter_map(|n| TextureKey::parse(n).map(|k| (k, n)))
    }

    pub fn texture(&self, name: &str) -> Result<Option<Texture2D>> {
        let key = file_stem_lower(name);
        let Some(&i) = self.textures.get(&key) else { return Ok(None) };
        let v = self.read_entry(&self.assets[i])?;
        Texture2D::from_value(&v, &self.bundle).map(Some)
    }

    /// Elenco dei nomi degli array di texture (es. `302-texarray`).
    pub fn texture_array_names(&self) -> impl Iterator<Item = &str> {
        self.texture_arrays.keys().map(String::as_str)
    }

    /// Carica un `Texture2DArray` per nome (es. `"302-texarray"`).
    pub fn texture_array(&self, name: &str) -> Result<Option<Texture2DArray>> {
        let key = file_stem_lower(name);
        let Some(&i) = self.texture_arrays.get(&key) else { return Ok(None) };
        let v = self.read_entry(&self.assets[i])?;
        Texture2DArray::from_value(&v, &self.bundle).map(Some)
    }

    /// Carica direttamente l'array del terreno di Daggerfall dato l'archivio (es. `302`, `002`, `102`).
    pub fn terrain_array(&self, archive: u32) -> Result<Option<Texture2DArray>> {
        let key = format!("{archive:03}-texarray");
        self.texture_array(&key)
    }
}

fn file_stem_lower(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = match file.rsplit_once('.') {
        Some((s, ext)) if !s.is_empty() && ext.len() <= 5 => s,
        _ => file,
    };
    stem.to_ascii_lowercase()
}
