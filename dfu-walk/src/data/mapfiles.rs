//! Località del mondo base di Daggerfall lette dai file `MAPTABLE.0xx`,
//! `MAPNAMES.0xx` e `MAPPITEM.0xx` (la forma estratta/decompressa di MAPS.BSA,
//! come quella usata da Daggerfall Unity).
//!
//! - `MAPTABLE.0xx`: tabella di record da 17 byte per i 256 map pixel del
//!   gruppo `xx`: [`MapId u32, X u8, Y u8, LocationType u8, Flags u8, Region
//!   u8, RmbBlockCount u8`] (X/Y sono le coordinate *locali* nel map pixel).
//! - `MAPNAMES.0xx`: un nome ASCII da 32 byte (`\0`-terminato) per ogni voce
//!   della tabella dello stesso gruppo.
//! - `MAPPITEM.0xx`: il record completo della location (header da 71 byte con
//!   le coordinate del centro in unità interne, nome da 32 byte, blocchi RMB
//!   come indici numerici 4+2+2).
//!
//! Nota: la copia di MAPS.BSA in circolazione in alcuni archivi non è un BSA
//! valido; questi file laterali restano la via più affidabile per ottenere le
//! città del mondo base.

use crate::error::{format_err, Result};
use crate::places::SettlementKind;
use crate::reader::Cursor;
use std::path::Path;

pub const MAP_PIXELS_WIDE: i32 = 1000;
pub const MAP_PIXELS_HIGH: i32 = 500;
/// Map pixel per ogni file `MAPTABLE/MAPNAMES/MAPPITEM.0xx`.
pub const PIXELS_PER_FILE: usize = 256;
pub const NUM_MAP_FILES: usize = 4;
/// Unità interne di una distanza: 128 = un map pixel (come DFU).
pub const INTERNAL_UNITS_PER_MAP_PIXEL: f32 = 128.0;

/// Dimensione fissa di un record di `MAPTABLE.0xx`.
const MAP_RECORD_SIZE: usize = 17;
/// Header fisso di un record di `MAPPITEM.0xx`.
const BLOCK_HEADER_SIZE: usize = 71;
/// Nome interno + unknowns che seguono l'header nei record MAPPITEM.
const BLOCK_NAME_SIZE: usize = 32;

/// Codici di `LocationType` di Mappy/DFU rilevanti per gli insediamenti.
pub mod location_type {
    pub const CITY: u8 = 1;
    pub const VILLAGE: u8 = 2;
    pub const DUNGEON: u8 = 3;
    pub const MANOR: u8 = 4;
    pub const PRISON: u8 = 5;
    pub const TAVERN: u8 = 6;
    pub const SHOP: u8 = 7;
    pub const FARM: u8 = 8;
    pub const RUINS: u8 = 9;
    pub const SENSATIONAL: u8 = 10;
    pub const DREAM_ISLAND: u8 = 11;
}

/// Una località del mondo (città, villaggio, maniero, dungeon...) letta da
/// `MAPTABLE.0xx` + `MAPNAMES.0xx`.
#[derive(Debug, Clone)]
pub struct WorldLocation {
    /// Resource ID della location (indice globale delle tabelle, usato anche
    /// come identificatore nelle save e nei teletrasporti).
    pub map_id: u32,
    /// Nome della località (da `MAPNAMES.0xx`).
    pub name: String,
    /// Colonna del map pixel (0..999).
    pub x: i32,
    /// Riga del map pixel (0..499, dall'alto).
    pub y: i32,
    /// Coordinate locali dentro il map pixel (0..255), dal bordo ovest/nord.
    pub local_x: u8,
    pub local_y: u8,
    /// Codice `LocationType` (vedi [`location_type`]).
    pub location_type: u8,
    pub flags: u8,
    /// Indice della regione (0..15).
    pub region: u8,
    /// Numero di blocchi RMB della location.
    pub rmb_block_count: u8,
}

impl WorldLocation {
    /// Tipo di insediamento dedotto da `LocationType` (con fallback sul nome,
    /// utile dove il tipo non basta, es. "City X" vs town).
    pub fn kind(&self) -> SettlementKind {
        match self.location_type {
            location_type::CITY => SettlementKind::from_name(&format!("City {}", self.name)),
            location_type::VILLAGE => SettlementKind::from_name(&format!("Village {}", self.name)),
            location_type::MANOR | location_type::PRISON | location_type::DREAM_ISLAND => {
                SettlementKind::Manor
            }
            location_type::FARM => SettlementKind::Farm,
            location_type::RUINS => SettlementKind::Ruins,
            _ => SettlementKind::Other,
        }
    }

    /// True se è una città o un villaggio (gli insediamenti "veri" da segnare
    /// sulla mappa).
    pub fn is_settlement(&self) -> bool {
        matches!(
            self.location_type,
            location_type::CITY | location_type::VILLAGE
        )
    }

    /// Posizione centrale della location in map pixel float (per disegnare il
    /// puntino sulla mappa 1000x500): il baricentro dei suoi record, oppure il
    /// map pixel + offset locale se non ci sono record MAPPITEM.
    pub fn center_position(&self) -> (f32, f32) {
        (
            self.x as f32 + (self.local_x as f32 + 0.5) / 256.0,
            self.y as f32 + (self.local_y as f32 + 0.5) / 256.0,
        )
    }
}

fn read_fixed_string(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_string()
}

/// I nomi delle 16 regioni di Iliac (blocco REGION del mondo base), per
/// etichettare i puntini sulla mappa.
pub const REGION_NAMES: [&str; 16] = [
    "Dwynnen",
    "Wayout Island",
    "The Isle of Balfell",
    "Mourningwood",
    "Mormunt Isle",
    "Phrygius Grove",
    "Isle of Falcar",
    "Turos Vitri",
    "Dragontail Mount",
    "Kirkwold Crag",
    "Camargan Crest",
    "Glenpoint Bluffs",
    "Shalgor Stone",
    "Orsinial Aldrock",
    "Banderbard",
    "Haemingwold",
];

pub fn region_name(index: u8) -> Option<&'static str> {
    REGION_NAMES.get(index as usize).copied()
}

/// Tutte le località del mondo base, unendo i 4 gruppi di file.
///
/// Cerca `<cartella>/MAPTABLE.000` ... `.003` e `<cartella>/MAPNAMES.000` ...
/// `.003` (nomi case-insensitive). I file possono stare anche direttamente
/// dentro un `MAPS.BSA` integro (in quel caso usa `WorldPlaces`); qui si
/// assumono file estratti, ad esempio da un archivio dfmod/BSA decompresso.
pub fn load_world_locations(dir: impl AsRef<Path>) -> Result<Vec<WorldLocation>> {
    let dir = dir.as_ref();
    let mut out = Vec::new();
    for file_index in 0..NUM_MAP_FILES {
        let table_name = format!("MAPTABLE.{file_index:03}");
        let names_name = format!("MAPNAMES.{file_index:03}");
        let Some(table_path) = crate::find_file_ci(dir, &table_name) else {
            continue; // gruppo assente: si salta senza errore
        };
        let names_path = crate::find_file_ci(dir, &names_name)
            .ok_or_else(|| crate::Error::Format(format!("manca {names_name} accanto a {table_name}")))?;
        let table = std::fs::read(&table_path)?;
        let names = std::fs::read(&names_path)?;
        out.extend(parse_map_table(&table, &names, file_index)?);
    }
    if out.is_empty() {
        return format_err(format!(
            "nessun file MAPTABLE.0xx trovato in {}",
            dir.display()
        ));
    }
    Ok(out)
}

/// Legge un paio `MAPTABLE.0xx` + `MAPNAMES.0xx` (buffer grezzi) per il gruppo
/// `file_index` (0..3). Ogni riga della tabella è un record da 17 byte; i nomi
/// sono stringhe da 32 byte nell'ordine delle righe.
pub fn parse_map_table(table: &[u8], names: &[u8], file_index: usize) -> Result<Vec<WorldLocation>> {
    let row_count = table.len() / MAP_RECORD_SIZE;
    if row_count == 0 {
        return Ok(Vec::new());
    }
    if names.len() < row_count * BLOCK_NAME_SIZE {
        return format_err(format!(
            "MAPNAMES.{file_index:03}: {row_count} record nella tabella ma solo {} nomi",
            names.len() / BLOCK_NAME_SIZE
        ));
    }
    let base_pixel = (file_index * PIXELS_PER_FILE) as i32;
    let mut out = Vec::with_capacity(row_count);
    for i in 0..row_count {
        let mut c = Cursor::at(table, (i * MAP_RECORD_SIZE) as i64)?;
        let map_id = c.u32()?;
        let local_x = c.u8()?;
        let local_y = c.u8()?;
        let location_type = c.u8()?;
        let flags = c.u8()?;
        let region = c.u8()?;
        let rmb_block_count = c.u8()?;
        // X/Y locali 0..255 dentro il map pixel; il map pixel globale deriva
        // dall'indice della tabella (gruppi da 256 pixel in ordine di riga).
        let pixel = base_pixel + i as i32;
        let gx = pixel % MAP_PIXELS_WIDE;
        let gy = pixel / MAP_PIXELS_WIDE;
        let name = read_fixed_string(
            Cursor::at(names, (i * BLOCK_NAME_SIZE) as i64)?.take(BLOCK_NAME_SIZE)?,
        );
        out.push(WorldLocation {
            map_id,
            name,
            x: gx,
            y: gy,
            local_x,
            local_y,
            location_type,
            flags,
            region,
            rmb_block_count,
        });
    }
    Ok(out)
}

/// Un blocco RMB di una location, composto alla maniera dell'engine vanilla:
/// `PREFIX(4) + ARCH_CHARS(2) + NUMBER(2) + ".RMB"` (es. index 41 → WALL,
/// char 'A','A', number 2 → `WALLAA02.RMB`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RmbBlockName {
    /// Prefisso dell'edificio (TVRN, GENR, RESI, BOOK, WALL, ...).
    pub prefix: String,
    /// Lettere architettura/clima (AA, AL, AM, AS, BL, ...).
    pub chars: String,
    /// Variante numerica a 2 cifre.
    pub number: u8,
}

impl RmbBlockName {
    /// Nome file completo, es. `WALLAA02.RMB`.
    pub fn file_name(&self) -> String {
        format!("{}{}{:02}.RMB", self.prefix, self.chars, self.number)
    }
}

/// Record di `MAPPITEM.0xx` decodato al livello minimo utile: coordinate del
/// centro della location (header da 71 byte) e nome interno.
#[derive(Debug, Clone)]
pub struct LocationBlock {
    pub name: String,
    /// Centro X/Y in unità interne (128 unità = 1 map pixel).
    pub center_x: i32,
    pub center_y: i32,
    pub width: u8,
    pub height: u8,
}

impl LocationBlock {
    pub fn center_map_position(&self) -> (f32, f32) {
        (
            self.center_x as f32 / INTERNAL_UNITS_PER_MAP_PIXEL,
            self.center_y as f32 / INTERNAL_UNITS_PER_MAP_PIXEL,
        )
    }
}

/// Analizza il blocco `location_index` di un buffer `MAPPITEM.0xx` già
/// decompresso. La struttura (da Mappy/Daggerfall):
/// - lista iniziale di offset u32 (uno per location presente nel file),
/// - ogni record: `PreRecordCount u32` + `N*6` byte, header da 71 byte
///   (coordinate mondo), nome da 32 byte, 9 byte sconosciuti,
///   `PostRecordCount u16` + `M*26` byte, poi 5 byte + altro nome da 32 byte
///   + 4 + 4 byte, `BlockWidth u8` + `BlockHeight u8`, 7 byte, e infine per
///   ogni blocco tre array da `width*height` byte: Index, Number, Char.
pub fn parse_location_block(item: &[u8], location_index: usize) -> Result<LocationBlock> {
    let mut offsets_c = Cursor::at(item, 0)?;
    let mut offsets = Vec::new();
    loop {
        let v = offsets_c.u32()?;
        if v == 0 || v as usize >= item.len() {
            break;
        }
        offsets.push(v as usize);
    }
    let start = *offsets
        .get(location_index)
        .ok_or_else(|| crate::Error::Format("MAPPITEM: indice location fuori lista".into()))?;
    let mut c = Cursor::at(item, start as i64)?;
    let pre_count = c.u32()? as usize;
    c.take(pre_count * 6)?;
    // Header fisso da 71 byte: i primi due u32 sono il centro della location
    // in unità interne (X est / Y nord rispetto all'angolo del map pixel).
    let center_x = c.i32()?;
    let center_y = c.i32()?;
    c.take(BLOCK_HEADER_SIZE - 8)?;
    let name = read_fixed_string(c.take(BLOCK_NAME_SIZE)?);
    c.take(9)?;
    let post_count = c.u16()? as usize;
    c.take(post_count * 26)?;
    c.take(5 + BLOCK_NAME_SIZE + 4 + 4)?;
    let width = c.u8()?;
    let height = c.u8()?;
    Ok(LocationBlock {
        name,
        center_x,
        center_y,
        width,
        height,
    })
}

/// Estrae i tre array di indici dei blocchi RMB (`Index`, `Number`, `Char`) da
/// un record `MAPPITEM` a partire dall'offset dei dati blocchi calcolato da
/// [`parse_location_block`]-like. Ritorna i nomi file composti alla maniera
/// vanilla (PREFIX + CHAR + NUMBER + .RMB).
pub fn block_names_from_record(record: &[u8], blocks_offset: usize, count: usize) -> Result<Vec<RmbBlockName>> {
    let mut idx = Cursor::at(record, blocks_offset as i64)?;
    let mut numbers = Cursor::at(record, (blocks_offset + count) as i64)?;
    let mut chars = Cursor::at(record, (blocks_offset + 2 * count) as i64)?;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let index = idx.u8()?;
        let number = numbers.u8()?;
        let letter = chars.u8()?;
        out.push(RmbBlockName {
            prefix: block_prefix(index).to_string(),
            chars: String::from(char::from(letter)),
            number,
        });
    }
    Ok(out)
}

/// Tabella dei prefissi edificio vanilla (index 0..46), allineata alla
/// `blockFileIndex` di Daggerfall Unity (Interware/BlockFileTable):
/// 0=TVRN taverna, 1=GENR emporio, 2=RESI residenza, 7=BOOK libreria,
/// 19=MANS maniero, 27=DUN dungeon, 46=WALL mura cittadine...
pub fn block_prefix(index: u8) -> &'static str {
    const PREFIXES: [&str; 47] = [
        "TVRN", "GENR", "RESI", "BLDG", "PLaz", "NGTN", "MAGE", "BOOK", "TEMP", "SMTH",
        "TMB1", "TMB2", "TMB3", "TMB4", "FRMN", "ARMC", "ALT1", "ALT2", "BKRP", "MANS",
        "DUNC", "GRDC", "HUT", "LVLT", "MONR", "MGSC", "NOBL", "OFFL", "OFFR", "PTEM",
        "RSDP", "SHIP", "SLAU", "STDL", "TAV2", "TRBN", "VAMP", "WZRD", "DUN", "GRVE",
        "HGBS", "JUEG", "DWN", "CIT", "Silo", "WALL", "GATE",
    ];
    PREFIXES.get(index as usize).copied().unwrap_or("RESI")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_map_table_base() {
        // Un record: map_id 42, locale (130, 77), city, flags 0, regione 5,
        // 9 blocchi. Nome "Anvil" padded a 32.
        let mut table = vec![0u8; MAP_RECORD_SIZE];
        table[0..4].copy_from_slice(&42u32.to_le_bytes());
        table[4] = 130;
        table[5] = 77;
        table[6] = location_type::CITY;
        table[8] = 5;
        table[9] = 9;
        let mut names = vec![0u8; 32];
        names[..5].copy_from_slice(b"Anvil");
        let locs = parse_map_table(&table, &names, 0).unwrap();
        assert_eq!(locs.len(), 1);
        let a = &locs[0];
        assert_eq!((a.map_id, a.name.as_str()), (42, "Anvil"));
        assert_eq!((a.x, a.y), (0, 0));
        assert_eq!((a.local_x, a.local_y), (130, 77));
        assert!(a.is_settlement());
        assert_eq!(a.kind(), SettlementKind::City);
        let (cx, cy) = a.center_position();
        assert!((cx - 0.5115).abs() < 1e-3 && (cy - 0.3047).abs() < 1e-3);
    }

    #[test]
    fn rifiuta_names_troppo_corti() {
        let table = vec![0u8; MAP_RECORD_SIZE * 2];
        let names = vec![b'A'; 32]; // solo 1 nome per 2 record
        assert!(parse_map_table(&table, &names, 0).is_err());
    }

    #[test]
    fn compone_nome_rmb() {
        let b = RmbBlockName {
            prefix: "WALL".into(),
            chars: "AA".into(),
            number: 2,
        };
        assert_eq!(b.file_name(), "WALLAA02.RMB");
        assert_eq!(block_prefix(41), "WALL");
        assert_eq!(block_prefix(0), "TVRN");
    }
}
