//! Catalogo degli artefatti di Daggerfall (da `MagicItems.json`, dump di DFU).
//!
//! Gli artefatti non usano il template base per l'icona: hanno una texture
//! dedicata in `TEXTURE.432` (player maschile) o `TEXTURE.433` (femminile).
//! Il record è determinato dal `param` del primo enchantment
//! `SpecialArtifactEffect` di ogni artefatto, tramite la tabella
//! `artifactTextureIndexMappings` di `ItemHelper.cs` (DFU, MIT).
use bevy::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// `artifactTextureIndexMappings` di `ItemHelper.cs` (indice = `ArtifactsSubTypes`).
const ARTIFACT_TEXTURE_MAPPINGS: [u32; 23] = [
    12, 13, 10, 8, 19, 16, 25, 18, 21, 2, 24, 26, 0, 15, 3, 9, 23, 17, 7, 1, 22, 20, 5,
];
const ARTIFACT_ARCHIVE_MALE: u32 = 432;
const ARTIFACT_ARCHIVE_FEMALE: u32 = 433;

#[derive(Clone, Debug)]
pub struct MagicItemDef {
    pub param: u8,
    pub name: String,
    pub texture_record: u32,
}

impl MagicItemDef {
    /// Archivio della texture per un dato sesso. In DFU l'icona dell'artefatto
    /// è la stessa per paperdoll e inventario.
    pub fn texture_archive(&self, female: bool) -> u32 {
        if female { ARTIFACT_ARCHIVE_FEMALE } else { ARTIFACT_ARCHIVE_MALE }
    }
    pub fn texture_for(&self, female: bool) -> (u32, u32) {
        (self.texture_archive(female), self.texture_record)
    }
}

#[derive(Resource, Default)]
pub struct MagicItemsCatalog {
    by_param: HashMap<u8, MagicItemDef>,
}

#[derive(Debug, Deserialize)]
struct RawEnchantment {
    #[serde(rename = "type")]
    ty: String,
    param: i32,
}

#[derive(Debug, Deserialize)]
struct RawMagicItem {
    #[serde(default)]
    name: String,
    #[serde(default)]
    enchantments: Vec<RawEnchantment>,
}

impl MagicItemsCatalog {
    /// Carica da `MagicItems.json` o `Artifacts.json`.
    pub fn load_auto(assets_dir: &Path) -> Option<Self> {
        for name in ["MagicItems.json", "Artifacts.json"] {
            let path = assets_dir.join(name);
            if let Some(c) = Self::from_json(&path) {
                info!("magic_items: {} artefatti caricati da {}", c.by_param.len(), path.display());
                return Some(c);
            }
        }
        warn!("magic_items: né MagicItems.json né Artifacts.json in {}", assets_dir.display());
        None
    }

    pub fn from_json(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let raw: Vec<RawMagicItem> = serde_json::from_str(&text).ok()?;
        let mut by_param = HashMap::new();
        for item in raw {
            let param = item.enchantments.iter()
                .find(|e| e.ty == "SpecialArtifactEffect")
                .map(|e| e.param)
                .filter(|&p| (0..23).contains(&p));
            let Some(param) = param else { continue };
            let record = ARTIFACT_TEXTURE_MAPPINGS[param as usize];
            by_param.insert(param as u8, MagicItemDef {
                param: param as u8,
                name: item.name,
                texture_record: record,
            });
        }
        (!by_param.is_empty()).then_some(Self { by_param })
    }

    pub fn get(&self, param: u8) -> Option<&MagicItemDef> {
        self.by_param.get(&param)
    }

    pub fn len(&self) -> usize { self.by_param.len() }
    pub fn is_empty(&self) -> bool { self.by_param.is_empty() }
}