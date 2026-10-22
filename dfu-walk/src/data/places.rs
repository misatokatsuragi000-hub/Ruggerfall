//! Località di Daggerfall (città, villaggi, dungeon) da MAPS.BSA.
//! Porting di MapsFile.cs / CityTable / DungeonTable di Daggerfall Unity (MIT).
//!
//! MAPS.BSA contiene un blocco `REGION00NN` per ogni regione (15), un blocco
//! `CITY00NN` e uno `VILL00NN` per ogni regione (nomi storici, non usati dal
//! gioco), e un blocco `DUNGEON0NN` per ogni regione. Le località generate dal
//! motore stanno nei blocchi con resource ID = map pixel (`x + y * 1001`) il cui
//! nome inizia con "CITY" o "VILL": ogni blocco è una tabella di record da 174
//! byte (nome 16, X e Y del map pixel come i16, ...); la stessa tabella include
//! anche gli insediamenti minori (manieri, fattorie, rovine), distinguibili dal
//! prefisso del nome. I dungeon stanno in tabelle da 20 byte nello stesso map
//! pixel.

use super::bsa::BsaFile;
use super::error::{format_err, Result};
use super::reader::Cursor;

pub const MAP_PIXELS_WIDE: i32 = 1001;
const CITY_RECORD_SIZE: usize = 174;
const DUNGEON_RECORD_SIZE: usize = 20;

/// Tipo di insediamento dedotto dal nome del record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementKind {
    City,
    Town,
    Village,
    Hamlet,
    Manor,
    Farm,
    Ruins,
    Other,
}

impl SettlementKind {
    pub fn from_name(name: &str) -> Self {
        // Come DFU: prima si scarta "ruins", poi maniero/fattoria, poi le
        // dimensioni dell'insediamento.
        let n = name.to_ascii_lowercase();
        if n.contains("ruin") {
            return SettlementKind::Ruins;
        }
        if n.contains("manor") || n.contains("tower") {
            return SettlementKind::Manor;
        }
        if n.contains("farm") {
            return SettlementKind::Farm;
        }
        match n.as_str() {
            s if s.starts_with("city ") => SettlementKind::City,
            s if s.starts_with("town ") => SettlementKind::Town,
            s if s.starts_with("village ") => SettlementKind::Village,
            s if s.starts_with("hamlet ") => SettlementKind::Hamlet,
            _ => SettlementKind::Other,
        }
    }

    /// Città, town, village o hamlet: gli insediamenti "veri" del gioco.
    pub fn is_settlement(&self) -> bool {
        matches!(
            self,
            SettlementKind::City | SettlementKind::Town | SettlementKind::Village | SettlementKind::Hamlet
        )
    }
}

/// Un record della tabella delle città/villaggi (o dei dungeon) di un map pixel.
#[derive(Debug, Clone)]
pub struct MapRecord {
    pub name: String,
    /// X del map pixel (0..1000).
    pub x: i16,
    /// Y del map pixel (0..499, dall'alto).
    pub y: i16,
    /// Record originale: i 174 byte della tabella città (vuoto per i dungeon).
    pub raw: Vec<u8>,
}

impl MapRecord {
    pub fn kind(&self) -> SettlementKind {
        SettlementKind::from_name(&self.name)
    }
}

/// Nome di una regione (blocchi REGION00NN, 15 nella versione standard).
#[derive(Debug, Clone)]
pub struct RegionName {
    pub index: u32,
    pub name: String,
}

pub struct WorldPlaces {
    bsa: BsaFile,
}

fn read_fixed_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

/// Legge una tabella di record da `record_size`, con conteggio u16 iniziale
/// (come MakeCityTable/MakeDungeonTable di DFU).
fn read_table(block: &[u8], record_size: usize) -> Result<Vec<MapRecord>> {
    let mut c = Cursor::at(block, 0)?;
    let count = c.u16()? as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let start = 2 + i * record_size;
        let mut r = Cursor::at(block, start as i64)?;
        let name = read_fixed_string(r.take(16)?);
        let x = r.i16()?;
        let y = r.i16()?;
        let raw = if record_size == CITY_RECORD_SIZE {
            block[start..start + record_size].to_vec()
        } else {
            Vec::new()
        };
        out.push(MapRecord { name, x, y, raw });
    }
    Ok(out)
}

impl WorldPlaces {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let bsa = BsaFile::open(path)?;
        if !bsa.records().iter().any(|r| r.prefix() == "REGION") {
            return format_err("non sembra un MAPS.BSA (nessun blocco REGION)");
        }
        Ok(Self { bsa })
    }

    pub fn bsa(&self) -> &BsaFile {
        &self.bsa
    }

    /// Nomi delle regioni nell'ordine dell'header.
    pub fn regions(&self) -> Result<Vec<RegionName>> {
        let mut out = Vec::new();
        for (name, block) in self.bsa.blocks_with_prefix("REGION")? {
            // RECORDS: conteggio u16, poi nomi da 16 byte.
            let mut c = Cursor::at(&block, 0)?;
            let count = c.u16()? as usize;
            for i in 0..count {
                let idx = name
                    .trim_start_matches(|ch: char| ch.is_ascii_alphabetic())
                    .parse::<u32>()
                    .unwrap_or(i as u32);
                let region = read_fixed_string(c.take(16)?);
                out.push(RegionName {
                    index: idx,
                    name: region,
                });
            }
        }
        Ok(out)
    }

    /// Tutte le località generate (città, villaggi, dungeon) dai blocchi con
    /// resource ID = map pixel. `prefix` filtrà per tipo ("CITY", "VILL",
    /// "DUNGEON").
    fn records_from_resource_ids(&self, prefix: &str, record_size: usize) -> Result<Vec<MapRecord>> {
        let up = prefix.to_ascii_uppercase();
        let mut out = Vec::new();
        for r in self.bsa.records() {
            if r.name.to_ascii_uppercase().starts_with(&up) && r.resource_id > 0 {
                let block = self.bsa.block(r)?;
                if block.len() >= 2 {
                    out.extend(read_table(block, record_size)?);
                }
            }
        }
        Ok(out)
    }

    /// Insediamenti (dai blocchi CITY/VILL per map pixel): città, towns,
    /// villages, hamlets più i minori (manieri, fattorie, rovine...).
    pub fn settlements(&self) -> Result<Vec<MapRecord>> {
        let mut all = self.records_from_resource_ids("CITY", CITY_RECORD_SIZE)?;
        all.extend(self.records_from_resource_ids("VILL", CITY_RECORD_SIZE)?);
        Ok(all)
    }

    /// Solo gli insediamenti principali (città/town/village/hamlet).
    pub fn towns(&self) -> Result<Vec<MapRecord>> {
        Ok(self
            .settlements()?
            .into_iter()
            .filter(|r| r.kind().is_settlement())
            .collect())
    }

    pub fn dungeons(&self) -> Result<Vec<MapRecord>> {
        self.records_from_resource_ids("DUNGEON", DUNGEON_RECORD_SIZE)
    }

    /// Località (insediamenti + dungeon) che cadono nel rect di map pixel
    /// `(min_x, min_y)..=(max_x, max_y)`, utili per popolare una zona.
    pub fn near_rect(
        &self,
        min_x: i32,
        min_y: i32,
        max_x: i32,
        max_y: i32,
    ) -> Result<Vec<(MapRecord, bool)>> {
        let inside = |r: &MapRecord| {
            r.x as i32 >= min_x
                && r.x as i32 <= max_x
                && r.y as i32 >= min_y
                && r.y as i32 <= max_y
        };
        let mut out = Vec::new();
        for s in self.settlements()? {
            if inside(&s) {
                out.push((s, false));
            }
        }
        for d in self.dungeons()? {
            if inside(&d) {
                out.push((d, true));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn city_block(entries: &[(&str, i16, i16)]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (name, x, y) in entries {
            let mut rec = vec![0u8; CITY_RECORD_SIZE];
            let nb = name.as_bytes();
            rec[..nb.len()].copy_from_slice(nb);
            rec[16..18].copy_from_slice(&x.to_le_bytes());
            rec[18..20].copy_from_slice(&y.to_le_bytes());
            data.extend_from_slice(&rec);
        }
        data
    }

    #[test]
    fn classifica_insediamenti() {
        assert_eq!(SettlementKind::from_name("City Wayrest"), SettlementKind::City);
        assert_eq!(
            SettlementKind::from_name("Village Dragontooth Isle"),
            SettlementKind::Village
        );
        assert_eq!(
            SettlementKind::from_name("Ruins of The Caorchom Farmstead"),
            SettlementKind::Ruins
        );
        assert_eq!(SettlementKind::from_name("Caarcun Manor"), SettlementKind::Manor);
        assert!(SettlementKind::Village.is_settlement());
        assert!(!SettlementKind::Ruins.is_settlement());
    }

    #[test]
    fn legge_tabella_citta() {
        // Blocco finto: header BSA con un REGION e un blocco città per map pixel 5.
        let mut records: Vec<(&str, u32, Vec<u8>)> = Vec::new();
        let mut region = vec![1u8, 0];
        let mut rn = [b' '; 16];
        rn[..8].copy_from_slice(b"Sentinel");
        region.extend_from_slice(&rn);
        records.push(("REGION0000", 0, region));
        records.push((
            "CITY0005",
            5,
            city_block(&[("City Sentinel", 5, 90), ("Hamlet Tiny", 6, 91)]),
        ));
        // Ricostruisce l'header a mano (offset contigui).
        let count = records.len() as u32;
        let mut data = Vec::new();
        data.extend_from_slice(&count.to_le_bytes());
        data.extend_from_slice(&0x010000F8u32.to_le_bytes());
        let mut offset = 8 + records.len() * 24;
        for (name, rid, blk) in &records {
            let mut n = [b' '; 12];
            let nb = name.as_bytes();
            n[..nb.len()].copy_from_slice(nb);
            data.extend_from_slice(&rid.to_le_bytes());
            data.extend_from_slice(&(offset as u32).to_le_bytes());
            data.extend_from_slice(&(blk.len() as u32).to_le_bytes());
            data.extend_from_slice(&n);
            offset += blk.len();
        }
        for (_, _, blk) in &records {
            data.extend_from_slice(blk);
        }

        let places = WorldPlaces {
            bsa: BsaFile::from_bytes(data).unwrap(),
        };
        let regions = places.regions().unwrap();
        assert_eq!(regions[0].name, "Sentinel");
        let towns = places.towns().unwrap();
        assert_eq!(towns.len(), 2);
        assert_eq!(towns[0].name, "City Sentinel");
        assert_eq!((towns[0].x, towns[0].y), (5, 90));
        let near = places.near_rect(0, 0, 5, 90).unwrap();
        assert_eq!(near.len(), 1);
        assert_eq!(near[0].0.name, "City Sentinel");
    }
}
