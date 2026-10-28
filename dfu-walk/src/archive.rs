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

/// Texture assegnata a una proprieta' di un materiale (`_MainTex`, `_BumpMap`, ...).
#[derive(Debug, Clone)]
pub struct MaterialTexture {
    /// 0 = nello stesso file; altrimenti la texture sta in un altro bundle (non risolvibile qui).
    pub file_id: i64,
    pub path_id: i64,
    pub scale: [f32; 2],
    pub offset: [f32; 2],
}

/// Un `Material` Unity ridotto alle proprieta' utili: texture, numeri e colori.
#[derive(Debug, Clone, Default)]
pub struct ModMaterial {
    pub name: String,
    /// nome proprieta' (`_MainTex`, `_BumpMap`, `_EmissionMap`, `_MetallicGlossMap`) -> texture
    pub textures: HashMap<String, MaterialTexture>,
    /// `_Metallic`, `_Glossiness`, `_Cutoff`, ...
    pub floats: HashMap<String, f32>,
    /// `_Color`, `_EmissionColor`, ... come [r, g, b, a]
    pub colors: HashMap<String, [f32; 4]>,
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
    /// lower-case file stem -> index into `assets` (Material entries, class 21).
    materials: HashMap<String, usize>,
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
        let mut materials = HashMap::new();
        for (i, a) in assets.iter().enumerate() {
            if a.class_id == class_id::TEXTURE_2D {
                textures.insert(file_stem_lower(&a.path), i);
            } else if a.class_id == class_id::TEXTURE_2D_ARRAY {
                texture_arrays.insert(file_stem_lower(&a.path), i);
            } else if a.class_id == class_id::MATERIAL {
                materials.insert(file_stem_lower(&a.path), i);
            }
        }
        Ok(Self { bundle, files, assets, textures, texture_arrays, materials })
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

    /// Nomi (stem minuscolo) dei materiali Unity del contenitore.
    pub fn material_names(&self) -> impl Iterator<Item = &str> {
        self.materials.keys().map(String::as_str)
    }

    /// Legge un materiale per nome (es. `"083_5-0"`).
    pub fn material(&self, name: &str) -> Result<Option<ModMaterial>> {
        let key = file_stem_lower(name);
        let Some(&i) = self.materials.get(&key) else { return Ok(None) };
        let entry = &self.assets[i];
        let v = self.read_entry(entry)?;
        Ok(Some(parse_material(&v, &key, entry.file_index)))
    }

    /// Decodifica la `Texture2D` assegnata a `property` (es. `"_MainTex"`) di un materiale.
    /// Le texture dei materiali sono oggetti del bundle che non compaiono in `texture_names()`.
    pub fn material_texture(&self, mat: &ModMaterial, property: &str) -> Result<Option<Texture2D>> {
        let Some(t) = mat.textures.get(property) else { return Ok(None) };
        if t.file_id != 0 || t.path_id == 0 {
            return Ok(None);
        }
        let (ni, sf) = &self.files[mat.file_index];
        let Some(obj) = sf.object(t.path_id) else { return Ok(None) };
        if obj.class_id != class_id::TEXTURE_2D {
            return Ok(None);
        }
        let node = &self.bundle.nodes()[*ni];
        let v = sf.read_object(&self.bundle, node, obj)?;
        Texture2D::from_value(&v, &self.bundle).map(Some)
    }

    /// Carica direttamente l'array del terreno di Daggerfall dato l'archivio (es. `302`, `002`, `102`).
    pub fn terrain_array(&self, archive: u32) -> Result<Option<Texture2DArray>> {
        let key = format!("{archive:03}-texarray");
        self.texture_array(&key)
    }
}

fn parse_material(v: &Value, name: &str, file_index: usize) -> ModMaterial {
    let mut m = ModMaterial { name: name.to_owned(), file_index, ..Default::default() };
    let Some(props) = v.get("m_SavedProperties") else { return m };
    let num = |v: Option<&Value>| v.and_then(Value::as_f64).unwrap_or(0.0) as f32;

    if let Some(list) = props.get("m_TexEnvs").and_then(Value::as_array) {
        for e in list {
            let (Some(key), Some(second)) = (e.get("first").and_then(Value::as_str), e.get("second")) else {
                continue;
            };
            let tex = second.get("m_Texture");
            let id = |f: &str| tex.and_then(|t| t.get(f)).and_then(Value::as_i64).unwrap_or(0);
            let vec2 = |f: &str, default: [f32; 2]| {
                second.get(f).map_or(default, |s| [num(s.get("x")), num(s.get("y"))])
            };
            m.textures.insert(
                key.to_owned(),
                MaterialTexture {
                    file_id: id("m_FileID"),
                    path_id: id("m_PathID"),
                    scale: vec2("m_Scale", [1.0, 1.0]),
                    offset: vec2("m_Offset", [0.0, 0.0]),
                },
            );
        }
    }
    if let Some(list) = props.get("m_Floats").and_then(Value::as_array) {
        for e in list {
            if let (Some(key), Some(val)) = (e.get("first").and_then(Value::as_str), e.get("second")) {
                m.floats.insert(key.to_owned(), num(Some(val)));
            }
        }
    }
    if let Some(list) = props.get("m_Colors").and_then(Value::as_array) {
        for e in list {
            if let (Some(key), Some(c)) = (e.get("first").and_then(Value::as_str), e.get("second")) {
                m.colors.insert(
                    key.to_owned(),
                    [num(c.get("r")), num(c.get("g")), num(c.get("b")), num(c.get("a"))],
                );
            }
        }
    }
    m
}

fn file_stem_lower(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    let stem = match file.rsplit_once('.') {
        Some((s, ext)) if !s.is_empty() && ext.len() <= 5 => s,
        _ => file,
    };
    stem.to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(fields: Vec<(&str, Value)>) -> Value {
        Value::Struct(fields.into_iter().map(|(n, v)| (n.to_owned(), v)).collect())
    }
    fn pair(first: &str, second: Value) -> Value {
        st(vec![("first", Value::Str(first.into())), ("second", second)])
    }

    #[test]
    fn material_properties_are_parsed() {
        let tex = st(vec![
            ("m_Texture", st(vec![("m_FileID", Value::Int(0)), ("m_PathID", Value::Int(-1234))])),
            ("m_Scale", st(vec![("x", Value::Float(2.0)), ("y", Value::Float(3.0))])),
            ("m_Offset", st(vec![("x", Value::Float(0.0)), ("y", Value::Float(0.5))])),
        ]);
        let color = st(vec![
            ("r", Value::Float(1.0)), ("g", Value::Float(0.5)), ("b", Value::Float(0.25)), ("a", Value::Float(1.0)),
        ]);
        let v = st(vec![(
            "m_SavedProperties",
            st(vec![
                ("m_TexEnvs", Value::Array(vec![pair("_MainTex", tex)])),
                ("m_Floats", Value::Array(vec![pair("_Metallic", Value::Float(0.25)), pair("_Glossiness", Value::Float(0.5))])),
                ("m_Colors", Value::Array(vec![pair("_Color", color)])),
            ]),
        )]);
        let m = parse_material(&v, "083_5-0", 0);
        let t = &m.textures["_MainTex"];
        assert_eq!((t.file_id, t.path_id), (0, -1234));
        assert_eq!(t.scale, [2.0, 3.0]);
        assert_eq!(t.offset, [0.0, 0.5]);
        assert_eq!(m.floats["_Metallic"], 0.25);
        assert_eq!(m.colors["_Color"], [1.0, 0.5, 0.25, 1.0]);
    }

    #[test]
    fn material_without_properties_is_empty() {
        let m = parse_material(&st(vec![]), "x", 0);
        assert!(m.textures.is_empty() && m.floats.is_empty() && m.colors.is_empty());
    }
}
