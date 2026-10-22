//! Parser dei blocchi RMB di BLOCKS.BSA, secondo la specifica UESP
//! ("Daggerfall Mod:BLOCKS.BSA/Map Block Records").
//!
//! Layout di un record RMB:
//!   intestazione FISSA di 6776 byte
//!       0      3 byte  BlockCount, ModelCount, FlatCount
//!       3    640 byte  32 x BlockPosition (20 byte: unk1, unk2, X, Z, rotazione)
//!     643    832 byte  32 x BuildingData (26 byte)
//!    1475    264 byte  Section2 (BlockPtr[32], BlockSize[32], ModelPtr, FlatPtr)
//!    1739    512 byte  GroundData (256 texture + 256 decorazioni)
//!    2251   4096 byte  automap
//!    6347    429 byte  nomi file
//!   poi: BlockCount x Block (dimensione da BlockSize[i]; Exterior + Interior),
//!        ModelCount x Model (66 byte), FlatCount x Flat (17 byte).
//!
//! Posizioni: gli oggetti dentro un Block sono relativi al sotto-record
//! (X, Z, rotazione di `subrecords[i]`); quelli "misc" sono relativi al blocco.

pub const RMB_HEADER_LEN: usize = 6776;
const OFF_BLOCK_POS: usize = 3;
const BLOCK_POS_SIZE: usize = 20;
const OFF_BLOCK_SIZES: usize = 1475 + 128;
const OFF_GROUND: usize = 1739;
const MODEL_SIZE: usize = 66;
const FLAT_SIZE: usize = 17;
const SECTION3_SIZE: usize = 16;
const PERSON_SIZE: usize = 17;
const DOOR_SIZE: usize = 19;
const BLOCKDATA_HEADER: usize = 17;
const OFF_BUILDING_DATA: usize = 643;
const BUILDING_DATA_SIZE: usize = 26;

/// Se true, Z dei sotto-record e z degli oggetti "misc" sono distanze dal bordo NORD del
/// blocco (n = 4096 - |z|); se false dal bordo SUD (n = |z|). Se gli edifici risultano
/// specchiati nord/sud rispetto alle strade, inverti questo valore.
pub const Z_FROM_NORTH_EDGE: bool = true;
/// Segno della rotazione (+1 = antioraria vista dall'alto, come `Quat::from_rotation_y`
/// con z = -nord). Se gli edifici a 90/270 gradi sono girati al contrario, metti -1.
pub const ROTATION_SIGN: f32 = 1.0;
const BLOCK_UNITS: f32 = 4096.0;

/// Angoli Daggerfall: 2048 = 360 gradi.
pub const DA_TO_DEG: f32 = 360.0 / 2048.0;

/// Dati di un edificio (26 byte), uno per sotto-record: l'edificio `i` e' il sotto-record `i`.
/// Layout (da Daggerfall Unity): NameSeed u16 @0, ServiceTimeLimit u32 @2, 12 byte nulli, FactionId u16 @18,
/// Sector i16 @20, LocationId u16 @22, BuildingType u8 @24, Quality u8 @25.
/// Verificalo con `dump_pois`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RmbBuildingData {
    pub name_seed: u16,
    pub service_time_limit: u32,
    pub faction_id: u16,
    pub sector: i16,
    pub location_id: u16,
    pub building_type: u8,
    pub quality: u8,
}

pub fn parse_building_data(rec: &[u8]) -> RmbBuildingData {
    debug_assert!(rec.len() >= BUILDING_DATA_SIZE);
    RmbBuildingData {
        name_seed: u16::from_le_bytes([rec[0], rec[1]]),
        service_time_limit: u32::from_le_bytes([rec[2], rec[3], rec[4], rec[5]]),
        faction_id: u16::from_le_bytes([rec[18], rec[19]]),
        sector: i16::from_le_bytes([rec[20], rec[21]]),
        location_id: u16::from_le_bytes([rec[22], rec[23]]),
        building_type: rec[24],
        quality: rec[25],
    }
}

#[derive(Debug, Clone)]
pub struct RmbSubRecord {
    pub x: i32,
    pub z: i32,
    pub rotation_deg: f32,
}

#[derive(Debug, Clone)]
pub struct RmbObject3D {
    /// primi 4 byte del record, per diagnostica
    pub raw_id: u32,
    /// id in ARCH3D.BSA = id1 * 100 + id2
    pub model_id: u32,
    /// byte 3 del ModelId ("Unknown1" nella specifica)
    pub object_type: u8,
    /// Point2: posizione di piazzamento (grezza, nel sistema del sotto-record o del blocco)
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Point1: quasi sempre zero (forse un pivot)
    pub pivot: [i32; 3],
    pub rotation_y_deg: f32,
    /// `Some(i)` = esterno del sotto-record `i`; `None` = lista misc del blocco
    pub block: Option<u8>,
    /// ancora X del sotto-record (unita' Daggerfall), gia' applicata da `placed_models`
    pub block_x: i32,
    /// ancora Z "grezza" del sotto-record (unita' Daggerfall)
    pub block_z: i32,
}

/// Modello gia' collocato nel blocco: x verso est e `n` verso nord, in unita' Daggerfall
/// dall'angolo sud-ovest del blocco (0..4096); `rotation_deg` antioraria vista dall'alto.
#[derive(Debug, Clone, Copy)]
pub struct PlacedModel {
    pub model_id: u32,
    pub object_type: u8,
    pub x: f32,
    pub n: f32,
    pub y: i32,
    pub rotation_deg: f32,
    /// true = esterno di un sotto-record (edifici e loro accessori); false = lista misc
    pub is_building: bool,
    /// indice del sotto-record (= indice in `RmbBlock::buildings`); `None` per la lista misc
    pub subrecord: Option<u8>,
}

/// Porta d'ingresso di un edificio gia' collocata nel blocco (unita' Daggerfall
/// dall'angolo sud-ovest, x est / n nord), con la rotazione del sotto-record.
#[derive(Debug, Clone, Copy)]
pub struct PlacedDoor {
    pub x: f32,
    pub n: f32,
    pub rotation_deg: f32,
    /// indice del sotto-record = indice in `RmbBlock::interiors` quando presente
    pub subrecord: u8,
}

#[derive(Debug, Clone)]
pub struct RmbFlatObject {
    pub texture_archive: u16,
    pub texture_record: u8,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// rotazione (angoli Daggerfall * 360/2048) della bitmap; per le flats interne
    /// e' il campo "Unknown" del record di 17 byte
    pub rotation_deg: f32,
    pub width: i16,
    pub height: i16,
    pub block: Option<u8>,
}

/// Trigger/marker della sezione 3 (16 byte): quattro i32.
#[derive(Debug, Clone, Copy, Default)]
pub struct RmbSection3 {
    pub a: i32,
    pub b: i32,
    pub c: i32,
    pub d: i32,
}

/// NPC di un interno (17 byte).
#[derive(Debug, Clone, Copy)]
pub struct RmbPerson {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// orientamento (angoli Daggerfall convertiti in gradi)
    pub direction_deg: f32,
    pub texture_archive: u16,
    pub texture_record: u8,
}

/// Porta interattiva (19 byte), presente negli interni (e raramente all'esterno).
#[derive(Debug, Clone)]
pub struct RmbDoor {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// angolo Daggerfall della normale della porta
    pub rotation_da: i16,
    pub texture_archive: u16,
    pub texture_record: u8,
    /// grado di chiusura (0 = spalancata)
    pub closed_amount: u8,
    /// direzione di apertura
    pub open_direction: u8,
    /// policy: 0 = passa, 1 = con scatto, 2 = chiusa, 3 = chiusa a chiave
    pub door_policy: u8,
}

impl RmbDoor {
    pub fn rotation_deg(&self) -> f32 {
        self.rotation_da as f32 * DA_TO_DEG
    }
}

/// Uno dei due BlockData di un sotto-record: `ext == true` = esterno dell'edificio,
/// `ext == false` = INTERNO (la stanza visitabile).
#[derive(Debug, Clone)]
pub struct BlockData {
    pub objects_3d: Vec<RmbObject3D>,
    pub flats: Vec<RmbFlatObject>,
    pub section3: Vec<RmbSection3>,
    pub people: Vec<RmbPerson>,
    pub doors: Vec<RmbDoor>,
    /// byte consumati dal BlockData
    pub bytes_used: usize,
}

/// Interno di un edificio: il secondo BlockData del sotto-record `subrecord`.
#[derive(Debug, Clone)]
pub struct InteriorBlock {
    pub subrecord: u8,
    pub data: BlockData,
}

#[derive(Debug, Clone)]
pub struct RmbBlock {
    /// 16x16 byte: bit 0-5 indice texture, bit 6-7 rotazione
    pub ground_tiles: [u8; 16 * 16],
    pub ground_decor: [u8; 16 * 16],
    pub subrecords: Vec<RmbSubRecord>,
    /// dati degli edifici, `buildings[i]` descrive `subrecords[i]`
    pub buildings: Vec<RmbBuildingData>,
    pub objects_3d: Vec<RmbObject3D>,
    pub flats: Vec<RmbFlatObject>,
    /// uno per sotto-record: l'INTERNO (secondo BlockData) di quell'edificio
    pub interiors: Vec<InteriorBlock>,
    /// porte del blocco stesso (di solito una per ogni ingresso di edificio)
    pub exterior_doors: Vec<(u8, RmbDoor)>,
    /// byte consumati dal parsing (per controllo: dovrebbe essere = lunghezza del record)
    pub bytes_used: usize,
}

fn i32_at(d: &[u8], p: usize) -> i32 {
    i32::from_le_bytes(d[p..p + 4].try_into().unwrap())
}

fn parse_model(d: &[u8], p: usize, block: Option<u8>) -> RmbObject3D {
    let raw_id = u32::from_le_bytes(d[p..p + 4].try_into().unwrap());
    let id1 = u16::from_le_bytes([d[p], d[p + 1]]) as u32;
    let id2 = d[p + 2] as u32;
    RmbObject3D {
        raw_id,
        model_id: id1 * 100 + id2,
        object_type: d[p + 3],
        pivot: [i32_at(d, p + 24), i32_at(d, p + 28), i32_at(d, p + 32)],
        x: i32_at(d, p + 36),
        y: i32_at(d, p + 40),
        z: i32_at(d, p + 44),
        rotation_y_deg: i16::from_le_bytes([d[p + 52], d[p + 53]]) as f32 * DA_TO_DEG,
        block,
        block_x: 0,
        block_z: 0,
    }
}

/// Record flat di 17 byte (layout verificato su BLOCKS.BSA intero, 109k flats):
/// X i32 @0, Y i32 @4, Z i32 @8, "Unknown" u16 @12 (valori tipo 25492/26893: NON e'
/// una rotazione ne' una dimensione), Height u8 @14 (0/4/8/12/16/20/255...),
/// TextureFileId u16 @15 (bit 0-8 record, bit 9-15 archivio).
/// Le flats degli interni usano quasi solo gli archivi bitmap 0/3/34/35 (e 98):
/// sono quadri/segnali/piastre di pavimento, NON modelli ARCH3D (gli archivi 199+
/// non compaiono mai qui). La rotazione della parete di supporto non e' nel record:
/// viene stimata da `interior.rs` comeFacing verso il centro della stanza.
fn parse_flat(d: &[u8], p: usize, block: Option<u8>) -> RmbFlatObject {
    let tex = u16::from_le_bytes([d[p + 15], d[p + 16]]);
    RmbFlatObject {
        texture_archive: (tex >> 7) as u16,
        texture_record: (tex & 0x7F) as u8,
        x: i32_at(d, p),
        y: i32_at(d, p + 4),
        z: i32_at(d, p + 8),
        rotation_deg: 0.0,
        width: 0,
        height: d[p + 14] as i16,
        block,
    }
}

/// Record person di 17 byte (verificato): X i32 @0, Y i32 @4, Z i32 @8,
/// byte @12 (0..146, ignoto), TextureFileId u16 @13 (record 91/92 = volti,
/// archivio = tex>>7, quasi sempre 0), byte @15 (0..3), byte @16 (0..45, volto?).
fn parse_person(d: &[u8], p: usize) -> RmbPerson {
    let tex = u16::from_le_bytes([d[p + 13], d[p + 14]]);
    RmbPerson {
        x: i32_at(d, p),
        y: i32_at(d, p + 4),
        z: i32_at(d, p + 8),
        direction_deg: 0.0,
        texture_archive: (tex >> 7) as u16,
        texture_record: (tex & 0x7F) as u8,
    }
}

fn parse_door(d: &[u8], p: usize) -> RmbDoor {
    let tex = u16::from_le_bytes([d[p + 14], d[p + 15]]);
    RmbDoor {
        x: i32_at(d, p),
        y: i32_at(d, p + 4),
        z: i32_at(d, p + 8),
        rotation_da: i16::from_le_bytes([d[p + 12], d[p + 13]]),
        texture_archive: tex >> 7,
        texture_record: (tex & 0x7F) as u8,
        closed_amount: d[p + 16],
        open_direction: d[p + 17],
        door_policy: d[p + 18],
    }
}

/// Legge un BlockData (17 byte di intestazione + le 5 liste) a partire da `q`.
fn parse_block_data(data: &[u8], q: usize, end: usize, block: Option<u8>) -> Result<BlockData, &'static str> {
    if q + BLOCKDATA_HEADER > end || end > data.len() {
        return Err("BlockData oltre la fine del record");
    }
    let nm = data[q] as usize;
    let nf = data[q + 1] as usize;
    let ns = data[q + 2] as usize;
    let np = data[q + 3] as usize;
    let nd = data[q + 4] as usize;
    let total = BLOCKDATA_HEADER
        + nm * MODEL_SIZE
        + nf * FLAT_SIZE
        + ns * SECTION3_SIZE
        + np * PERSON_SIZE
        + nd * DOOR_SIZE;
    if q + total > end {
        return Err("BlockData piu' grande dello spazio disponibile");
    }
    let mut p = q + BLOCKDATA_HEADER;
    let mut out = BlockData {
        objects_3d: Vec::with_capacity(nm),
        flats: Vec::with_capacity(nf),
        section3: Vec::with_capacity(ns),
        people: Vec::with_capacity(np),
        doors: Vec::with_capacity(nd),
        bytes_used: total,
    };
    for _ in 0..nm {
        out.objects_3d.push(parse_model(data, p, block));
        p += MODEL_SIZE;
    }
    for _ in 0..nf {
        out.flats.push(parse_flat(data, p, block));
        p += FLAT_SIZE;
    }
    for _ in 0..ns {
        out.section3.push(RmbSection3 {
            a: i32_at(data, p),
            b: i32_at(data, p + 4),
            c: i32_at(data, p + 8),
            d: i32_at(data, p + 12),
        });
        p += SECTION3_SIZE;
    }
    for _ in 0..np {
        out.people.push(parse_person(data, p));
        p += PERSON_SIZE;
    }
    for _ in 0..nd {
        out.doors.push(parse_door(data, p));
        p += DOOR_SIZE;
    }
    Ok(out)
}

impl RmbBlock {
    pub fn parse(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < RMB_HEADER_LEN {
            return Err("Record RMB piu' corto dell'intestazione (6776 byte)");
        }
        let block_count = data[0] as usize;
        let model_count = data[1] as usize;
        let flat_count = data[2] as usize;
        if block_count > 32 {
            return Err("BlockCount > 32");
        }

        let mut ground_tiles = [0u8; 256];
        ground_tiles.copy_from_slice(&data[OFF_GROUND..OFF_GROUND + 256]);
        let mut ground_decor = [0u8; 256];
        ground_decor.copy_from_slice(&data[OFF_GROUND + 256..OFF_GROUND + 512]);

        let mut subrecords = Vec::with_capacity(block_count);
        for i in 0..block_count {
            let p = OFF_BLOCK_POS + i * BLOCK_POS_SIZE;
            subrecords.push(RmbSubRecord {
                x: i32_at(data, p + 8),
                z: i32_at(data, p + 12),
                rotation_deg: i32_at(data, p + 16) as f32 * DA_TO_DEG,
            });
        }

        let mut buildings = Vec::with_capacity(block_count);
        for i in 0..block_count {
            let p = OFF_BUILDING_DATA + i * BUILDING_DATA_SIZE;
            buildings.push(parse_building_data(&data[p..p + BUILDING_DATA_SIZE]));
        }

        let mut objects_3d = Vec::new();
        let mut flats = Vec::new();
        let mut interiors = Vec::with_capacity(block_count);
        let mut exterior_doors: Vec<(u8, RmbDoor)> = Vec::new();
        let mut pos = RMB_HEADER_LEN;

        for i in 0..block_count {
            let size = u32::from_le_bytes(
                data[OFF_BLOCK_SIZES + i * 4..OFF_BLOCK_SIZES + i * 4 + 4].try_into().unwrap(),
            ) as usize;
            let end = pos.checked_add(size).ok_or("BlockSize assurda")?;
            if end > data.len() {
                return Err("Block oltre la fine del record");
            }
            // Ogni subrecord contiene DUE BlockData consecutivi: esterno e interno.
            let ext = parse_block_data(data, pos, end, Some(i as u8))?;
            let q_int = pos + ext.bytes_used;
            let int = match parse_block_data(data, q_int, end, Some(i as u8)) {
                Ok(bd) => Some(bd),
                // Alcuni archivi hanno 1 byte di padding o un interno assente: non
                // e' fatale, l'edificio semplicemente non ha interni visitabili.
                Err(_) => None,
            };
            objects_3d.extend(ext.objects_3d);
            flats.extend(ext.flats);
            exterior_doors.extend(ext.doors.into_iter().map(|d| (i as u8, d)));
            if let Some(bd) = int {
                interiors.push(InteriorBlock { subrecord: i as u8, data: bd });
            }
            pos = end;
        }

        if pos + model_count * MODEL_SIZE + flat_count * FLAT_SIZE > data.len() {
            return Err("Liste misc oltre la fine del record");
        }
        for _ in 0..model_count {
            objects_3d.push(parse_model(data, pos, None));
            pos += MODEL_SIZE;
        }
        for _ in 0..flat_count {
            flats.push(parse_flat(data, pos, None));
            pos += FLAT_SIZE;
        }

        Ok(Self { ground_tiles, ground_decor, subrecords, buildings, objects_3d, flats, interiors, exterior_doors, bytes_used: pos })
    }

    /// Porte d'ingresso degli edifici gia' collocate nel blocco (stesso sistema di
    /// `placed_models`: x verso est, n verso nord, unita' Daggerfall dall'angolo
    /// sud-ovest). Se il BlockData esterno del sotto-record ha porte sue, si usa la
    /// prima; altrimenti l'ingresso canonico e' l'ancora del sotto-record.
    pub fn placed_exterior_doors(&self) -> Vec<PlacedDoor> {
        let mut out = Vec::new();
        for (i, s) in self.subrecords.iter().enumerate() {
            let anchor_n = if Z_FROM_NORTH_EDGE { BLOCK_UNITS - s.z as f32 } else { s.z as f32 };
            let phi = (s.rotation_deg * ROTATION_SIGN).to_radians();
            let (sn, cs) = phi.sin_cos();
            // scostamento locale della porta rispetto all'ancora, ruotato col subrecord
            let place = |lx: f32, ln: f32| PlacedDoor {
                x: s.x as f32 + lx * cs - ln * sn,
                n: anchor_n + lx * sn + ln * cs,
                rotation_deg: s.rotation_deg * ROTATION_SIGN,
                subrecord: i as u8,
            };
            if let Some((_, d)) = self.exterior_doors.iter().find(|(sb, _)| *sb == i as u8) {
                let ln = if Z_FROM_NORTH_EDGE { d.z as f32 } else { -(d.z as f32) };
                out.push(place(d.x as f32, ln));
            } else {
                out.push(place(0.0, 0.0));
            }
        }
        out
    }

    /// Posizioni nel blocco di tutti gli oggetti 3D. Gli oggetti dei sotto-record sono
    /// ruotati e traslati con X, Z e rotazione del sotto-record.
    pub fn placed_models(&self) -> Vec<PlacedModel> {
        // z "grezza" (negativa o positiva) -> verso nord
        let n_of = |z_raw: f32| if Z_FROM_NORTH_EDGE { BLOCK_UNITS + z_raw } else { -z_raw };
        self.objects_3d
            .iter()
            .map(|o| match o.block {
                None => PlacedModel {
                    model_id: o.model_id,
                    object_type: o.object_type,
                    x: o.x as f32,
                    n: n_of(o.z as f32),
                    y: o.y,
                    rotation_deg: o.rotation_y_deg * ROTATION_SIGN,
                    is_building: false,
                    subrecord: None,
                },
                Some(i) => {
                    let s = &self.subrecords[i as usize];
                    let anchor_n = if Z_FROM_NORTH_EDGE { BLOCK_UNITS - s.z as f32 } else { s.z as f32 };
                    let phi = (s.rotation_deg * ROTATION_SIGN).to_radians();
                    let (sn, cs) = phi.sin_cos();
                    // scostamento locale (di solito 0 o pochi punti): ruotato col sotto-record
                    let lx = o.x as f32;
                    let ln = if Z_FROM_NORTH_EDGE { o.z as f32 } else { -(o.z as f32) };
                    PlacedModel {
                        model_id: o.model_id,
                        object_type: o.object_type,
                        x: s.x as f32 + lx * cs - ln * sn,
                        n: anchor_n + lx * sn + ln * cs,
                        y: o.y,
                        rotation_deg: (s.rotation_deg + o.rotation_y_deg) * ROTATION_SIGN,
                        is_building: true,
                        subrecord: Some(i),
                    }
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn building_data_layout() {
        let mut rec = [0u8; 26];
        rec[0..2].copy_from_slice(&0x1234u16.to_le_bytes());
        rec[2..6].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        rec[18..20].copy_from_slice(&0x0150u16.to_le_bytes());
        rec[20..22].copy_from_slice(&(-3i16).to_le_bytes());
        rec[22..24].copy_from_slice(&0x0042u16.to_le_bytes());
        rec[24] = 15;
        rec[25] = 7;
        let b = parse_building_data(&rec);
        assert_eq!(
            b,
            RmbBuildingData { name_seed: 0x1234, service_time_limit: 0xDEADBEEF, faction_id: 0x150, sector: -3, location_id: 0x42, building_type: 15, quality: 7 }
        );
    }
}


