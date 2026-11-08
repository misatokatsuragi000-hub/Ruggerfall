//! Template degli oggetti.
//!
//! ## Da dove vengono i dati
//!
//! Daggerfall Unity **non legge `FALL.EXE` a runtime**: carica un dump JSON
//! (`ItemTemplates.json`) che contiene la stessa tabella già digerita, con i
//! campi `playerTextureArchive/Record` e `worldTextureArchive/Record` per
//! ogni template. È esattamente quello che serve a `icon_location_full` per
//! risolvere l'icona corretta.
//!
//! Questo modulo prova in ordine:
//!   1. `assets/ItemTemplates.json` (dump di DFU, consigliato);
//!   2. `<ARENA2>/FALL.EXE` (parser euristico, opzionale, se hai l'exe).
//!
//! Se nessuno dei due funziona, `ItemTemplates` resta vuoto: la catena
//! `icon_location_full` ricade su paperdoll/procedurale e il gioco funziona
//! come prima.
//!
//! ## Formato del JSON di DFU
//!
//! Array di oggetti con campi tra cui:
//! - `index: u32` (obbligatorio);
//! - `playerTextureArchive: u32`, `playerTextureRecord: u32`;
//! - `worldTextureArchive: u32`, `worldTextureRecord: u32`.
//!
//! Regola di `ItemHelper.GetItemImage`:
//! - se `playerTextureArchive == 0 && playerTextureRecord == 0` → usa
//!   `worldTextureArchive/Record`;
//! - altrimenti usa `playerTextureArchive/Record`.
//!
//! Alcuni template hanno `worldArchive == 0` (es. armi, che non hanno
//! world texture): restano senza fallback world e il chiamante userà
//! paperdoll o procedurale.
//!
//! ## Patch anti-file-sbagliato
//!
//! Prima del fix, `load_from_dfu_json` accettava qualunque array JSON con
//! campo `index`, anche `Artifacts.json` o `MagicItems.json` (che hanno la
//! stessa forma ma **non** i campi texture). Il risultato era una mappa
//! "piena" ma inutile: tutte le entry con `player`/`world` a `None`,
//! `icon()` che ritorna `None` per tutti, e `load_auto` che ritorna `Some`
//! impedendo il fallback a FALL.EXE e al procedurale.
//!
//! Ora:
//! - i template **senza né player né world texture** vengono **scartati**
//!   (non entrano nella mappa);
//! - se la mappa finale è **vuota**, si logga un warning chiaro e si
//!   ritorna `None` (così `load_auto` prova FALL.EXE e, se fallisce
//!   anche quello, la catena ricade sul procedurale);
//! - se la mappa è **non vuota** ma alcuni template sono stati scartati,
//!   si logga un warning con il conteggio (non blocca, ma aiuta a capire
//!   se il file è quello giusto).
//!
//! Con questa patch:
//! - **file giusto** (`ItemTemplates.json`): 200+ template con texture,
//!   nessuno scartato, `info!` con il conteggio;
//! - **file sbagliato** (`Artifacts.json` / `MagicItems.json`): tutti
//!   scartati, `warn!` chiaro, `None` → `load_auto` prova FALL.EXE o
//!   ritorna `None` → `icon_location_full` usa paperdoll/procedurale
//!   senza finta mappa.

use bevy::prelude::*;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// Struttura minima di un template, solo i campi che servono all'icona.
///
/// Il JSON di DFU usa camelCase (`playerTextureArchive`, ecc.). Serde con
/// `rename_all = "camelCase"` mappa `playerTextureArchive` su
/// `player_texture_archive`. I campi extra del JSON (`name`, `baseWeight`,
/// `hitPoints`, …) vengono ignorati: non abbiamo `deny_unknown_fields`, quindi
/// un dump reale (che ha molti più campi) deserializza senza errori.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawItemTemplate {
    index: u32,
    #[serde(default)]
    player_texture_archive: u32,
    #[serde(default)]
    player_texture_record: u32,
    #[serde(default)]
    world_texture_archive: u32,
    #[serde(default)]
    world_texture_record: u32,
}

/// Icona di un template: `player` da inventario/equipaggiamento, `world`
/// fallback per gli oggetti che non hanno una texture "player" dedicata
/// (gemme, ingredienti, monete, …).
#[derive(Clone, Copy, Debug, Default)]
pub struct ItemTemplateIcon {
    /// `playerTextureArchive/Record`: icona da inventario/equipaggiamento.
    pub player: Option<(u32, usize)>,
    /// `worldTextureArchive/Record`: fallback DFU quando player è (0, 0).
    pub world: Option<(u32, usize)>,
}

impl ItemTemplateIcon {
    /// Regola di `ItemHelper.GetItemImage`: player, altrimenti world.
    pub fn icon(self) -> Option<(u32, usize)> {
        self.player.or(self.world)
    }
}

#[derive(Resource, Default)]
pub struct ItemTemplates {
    map: HashMap<u32, ItemTemplateIcon>,
}

impl ItemTemplates {
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn icon(&self, id: u32) -> Option<ItemTemplateIcon> {
        self.map.get(&id).copied()
    }

    /// Carica da un file `ItemTemplates.json` di DFU. Se il file non esiste,
    /// non è JSON valido, o **non contiene nessun template con texture**,
    /// restituisce `None`.
    ///
    /// I template senza né `player` né `world` texture vengono scartati e
    /// contati. Se la mappa finale è vuota, `warn!` + `None` (così `load_auto`
    /// prova FALL.EXE o ricade sul procedurale). Se la mappa è non vuota ma
    /// alcuni template sono stati scartati, `warn!` con il conteggio (non
    /// blocca: il file è probabilmente giusto ma ha qualche entry anomala).
    pub fn load_from_dfu_json(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let raw: Vec<RawItemTemplate> = serde_json::from_str(&text).ok()?;
        let mut map = HashMap::with_capacity(raw.len());
        let mut scartati_senza_texture = 0usize;
        for t in raw {
            let player = if t.player_texture_archive != 0 {
                Some((t.player_texture_archive, t.player_texture_record as usize))
            } else {
                None
            };
            let world = if t.world_texture_archive != 0 {
                Some((t.world_texture_archive, t.world_texture_record as usize))
            } else {
                None
            };
            // Un template senza né player né world texture non serve alla
            // catena delle icone: è probabilmente un artefatto o un magic
            // item preso dal file sbagliato (`Artifacts.json` invece di
            // `ItemTemplates.json`).
            if player.is_none() && world.is_none() {
                scartati_senza_texture += 1;
                continue;
            }
            map.insert(t.index, ItemTemplateIcon { player, world });
        }
        if map.is_empty() {
            warn!(
                "item_templates: `{}` non contiene template con texture ({} scartati): \
probabilmente è `Artifacts.json` o `MagicItems.json`, non `ItemTemplates.json`",
path.display(),
                  scartati_senza_texture,
            );
            return None;
        }
        if scartati_senza_texture > 0 {
            warn!(
                "item_templates: {} template senza texture scartati da {}",
                scartati_senza_texture,
                path.display(),
            );
        }
        info!("item_templates: {} template caricati da {}", map.len(), path.display());
        Some(Self { map })
    }

    /// Prova prima `ItemTemplates.json`, poi `FALL.EXE` (parser euristico).
    /// Non c'è bisogno di avere l'exe se il JSON è presente.
    pub fn load_auto(data_dir: &Path, assets_json: &Path) -> Option<Self> {
        // 1. JSON di DFU in `assets/ItemTemplates.json`
        if let Some(t) = Self::load_from_dfu_json(assets_json) {
            return Some(t);
        }
        // 2. Parser euristico di FALL.EXE (solo se presente)
        if let Some(path) = crate::data::find_file_ci(data_dir, "FALL.EXE") {
            if let Some(t) = Self::load(&path) {
                return Some(t);
            }
            warn!("FALL.EXE trovato ma tabella template non localizzata: uso i fallback");
        }
        warn!(
            "Nessun `{}` e nessun FALL.EXE: icone via paperdoll/procedurale",
            assets_json.display()
        );
        None
    }

    /// Legge FALL.EXE e localizza la tabella template (fallback).
    /// `None` se non trova nulla: i fallback restano attivi.
    ///
    /// **Non toccato dalla patch anti-file-sbagliato**: FALL.EXE non è un
    /// JSON e non può essere confuso con `Artifacts.json`. Il controllo
    /// `map.len() < 64` resta: se il parser euristico trova meno di 64
    /// template, la tabella non è stata localizzata correttamente e si
    /// ritorna `None`.
    pub fn load(fall_exe: &Path) -> Option<Self> {
        let bytes = std::fs::read(fall_exe).ok()?;
        let (off, stride) = Self::find_table(&bytes)?;
        let (wa, _wr, pa, _pr) = Self::find_fields(&bytes, off, stride)?;

        let mut map = HashMap::new();
        let mut k: u32 = 0;
        loop {
            let rec = off + (k as usize) * stride;
            if rec + stride > bytes.len() {
                break;
            }
            let id = u16_at(&bytes, rec)? as u32;
            if id != k {
                break;
            }
            let read = |o: usize| -> Option<(u32, usize)> {
                let a = u16_at(&bytes, rec + o)? as u32;
                let r = u16_at(&bytes, rec + o + 2)? as usize;
                if a == 0 {
                    None
                } else {
                    Some((a, r))
                }
            };
            map.insert(id, ItemTemplateIcon { player: read(pa), world: read(wa) });
            k += 1;
            if k > 4096 {
                break;
            }
        }
        if map.len() < 64 {
            return None;
        }
        info!("item_templates: {} template letti da FALL.EXE", map.len());
        Some(Self { map })
    }

    fn find_table(bytes: &[u8]) -> Option<(usize, usize)> {
        for stride in (24..=48).step_by(2) {
            let mut off = 0usize;
            while off + stride * 49 <= bytes.len() {
                if u16_at(bytes, off) == Some(0)
                    && u16_at(bytes, off + stride) == Some(1)
                    && u16_at(bytes, off + 2 * stride) == Some(2)
                    {
                        let full = (0..=48usize).all(|k| u16_at(bytes, off + k * stride) == Some(k as u16));
                        if full {
                            return Some((off, stride));
                        }
                    }
                    off += 2;
            }
        }
        None
    }

    fn find_fields(bytes: &[u8], off: usize, stride: usize) -> Option<(usize, usize, usize, usize)> {
        const KNOWN_ARCHIVES: [u32; 13] = [
            0, 204, 205, 216, 217, 233, 234, 246, 247, 250, 251, 432, 433,
        ];
        let mut best: Option<(usize, usize, usize, usize, u32)> = None;
        for pa in (6..stride.saturating_sub(2)).step_by(2) {
            let wa = pa.saturating_sub(4);
            let mut hits = 0u32;
            for k in 0..200usize {
                let rec = off + k * stride;
                if rec + stride > bytes.len() {
                    break;
                }
                let pa_v = u16_at(bytes, rec + pa)? as u32;
                let wa_v = u16_at(bytes, rec + wa)? as u32;
                let pr_v = u16_at(bytes, rec + pa + 2)?;
                let wr_v = u16_at(bytes, rec + wa + 2)?;
                let ok_pa = KNOWN_ARCHIVES.contains(&pa_v);
                let ok_wa = wa_v == 0 || KNOWN_ARCHIVES.contains(&wa_v);
                if ok_pa && ok_wa && pr_v < 512 && wr_v < 512 {
                    hits += 1;
                }
            }
            if hits >= 140 && best.map_or(true, |b| hits > b.4) {
                best = Some((wa, wa + 2, pa, pa + 2, hits));
            }
        }
        best.map(|(wa, wr, pa, pr, _)| (wa, wr, pa, pr))
    }
}

fn u16_at(bytes: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*bytes.get(off)?, *bytes.get(off + 1)?]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(index: u32, pa: u32, pr: u32, wa: u32, wr: u32) -> String {
        format!(
            r#"{{"index":{index},"playerTextureArchive":{pa},"playerTextureRecord":{pr},"worldTextureArchive":{wa},"worldTextureRecord":{wr}}}"#
        )
    }

    fn write_json(entries: &[String]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "item_templates_test_{}_{}",
            std::process::id(),
                                                    entries.len(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ItemTemplates.json");
        std::fs::write(&path, format!("[{}]", entries.join(","))).unwrap();
        path
    }

    #[test]
    fn good_file_loads_and_keeps_player_and_world() {
        let path = write_json(&[
            raw(0, 0, 0, 254, 0),           // solo world (gemma)
        raw(1, 216, 4, 0, 0),           // solo player (arma)
        raw(2, 247, 3, 254, 1),         // entrambi
        ]);
        let t = ItemTemplates::load_from_dfu_json(&path).expect("doveva caricare");
        assert_eq!(t.len(), 3);
        assert_eq!(t.icon(0).unwrap().icon(), Some((254, 0)));
        assert_eq!(t.icon(0).unwrap().player, None);
        assert_eq!(t.icon(1).unwrap().icon(), Some((216, 4)));
        assert_eq!(t.icon(1).unwrap().world, None);
        let both = t.icon(2).unwrap();
        assert_eq!(both.player, Some((247, 3)));
        assert_eq!(both.world, Some((254, 1)));
        assert_eq!(both.icon(), Some((247, 3))); // player vince
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn wrong_file_like_artifacts_returns_none() {
        // `Artifacts.json` ha `index` ma non i campi texture.
        let path = write_json(&[
            r#"{"index":4,"name":"The Masque of Clavicus","type":"ArtifactClass1"}"#.to_string(),
                              r#"{"index":66,"name":"The Mehrunes Razor","type":"ArtifactClass1"}"#.to_string(),
        ]);
        assert!(ItemTemplates::load_from_dfu_json(&path).is_none());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn mixed_file_loads_valid_and_skips_invalid() {
        // File "giusto" ma con qualche entry anomala: si carica, si scartano
        // i template senza texture.
        let path = write_json(&[
            raw(0, 0, 0, 254, 0),           // valido
                              r#"{"index":1,"name":"Anomalia"}"#.to_string(), // scartato
                              raw(2, 216, 4, 0, 0),           // valido
        ]);
        let t = ItemTemplates::load_from_dfu_json(&path).expect("doveva caricare");
        assert_eq!(t.len(), 2);
        assert!(t.icon(0).is_some());
        assert!(t.icon(1).is_none()); // scartato
        assert!(t.icon(2).is_some());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn empty_json_array_returns_none() {
        let dir = std::env::temp_dir().join(format!("item_templates_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ItemTemplates.json");
        std::fs::write(&path, "[]").unwrap();
        assert!(ItemTemplates::load_from_dfu_json(&path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_returns_none() {
        let path = std::env::temp_dir().join("questo_file_non_esiste_mai.json");
        assert!(ItemTemplates::load_from_dfu_json(&path).is_none());
    }

    #[test]
    fn extra_fields_are_ignored() {
        // Il dump reale ha `name`, `baseWeight`, `hitPoints`, `variants`, ecc.:
        // devono essere ignorati senza far fallire la deserializzazione.
        let path = write_json(&[r#"{"index":0,"name":"Ruby","baseWeight":0.25,"hitPoints":1000,"worldTextureArchive":254,"worldTextureRecord":0,"playerTextureArchive":0,"playerTextureRecord":0}"#.to_string()]);
        let t = ItemTemplates::load_from_dfu_json(&path).expect("doveva caricare");
        assert_eq!(t.icon(0).unwrap().icon(), Some((254, 0)));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
