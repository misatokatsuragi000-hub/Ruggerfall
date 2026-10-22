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
/// Layout (da Daggerfall Unity): NameSeed u16 @0, 16 byte nulli, FactionId u16 @18,
/// Sector i16 @20, LocationId u16 @22, BuildingType u8 @24, Quality u8 @25.
/// Verificalo con `dump_pois`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RmbBuildingData {
    pub name_seed: u16,
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

#[derive(Debug, Clone)]
pub struct RmbFlatObject {
    pub texture_archive: u16,
    pub texture_record: u8,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub block: Option<u8>,
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
    }
}

fn parse_flat(d: &[u8], p: usize, block: Option<u8>) -> RmbFlatObject {
    let tex = u16::from_le_bytes([d[p + 12], d[p + 13]]);
    RmbFlatObject {
        texture_archive: tex >> 7,
        texture_record: (tex & 0x7F) as u8,
        x: i32_at(d, p),
        y: i32_at(d, p + 4),
        z: i32_at(d, p + 8),
        block,
    }
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
        let mut pos = RMB_HEADER_LEN;

        for i in 0..block_count {
            let size = u32::from_le_bytes(
                data[OFF_BLOCK_SIZES + i * 4..OFF_BLOCK_SIZES + i * 4 + 4].try_into().unwrap(),
            ) as usize;
            let end = pos.checked_add(size).ok_or("BlockSize assurda")?;
            if end > data.len() {
                return Err("Block oltre la fine del record");
            }
            // Solo l'esterno (primo BlockData); l'interno viene saltato.
            let q = pos;
            if q + BLOCKDATA_HEADER > end {
                return Err("BlockData piu' corto della sua intestazione");
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
                return Err("Esterno del Block oltre la sua dimensione");
            }
            let mut p = q + BLOCKDATA_HEADER;
            for _ in 0..nm {
                objects_3d.push(parse_model(data, p, Some(i as u8)));
                p += MODEL_SIZE;
            }
            for _ in 0..nf {
                flats.push(parse_flat(data, p, Some(i as u8)));
                p += FLAT_SIZE;
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

        Ok(Self { ground_tiles, ground_decor, subrecords, buildings, objects_3d, flats, bytes_used: pos })
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
        rec[18..20].copy_from_slice(&0x0150u16.to_le_bytes());
        rec[20..22].copy_from_slice(&(-3i16).to_le_bytes());
        rec[22..24].copy_from_slice(&0x0042u16.to_le_bytes());
        rec[24] = 15;
        rec[25] = 7;
        let b = parse_building_data(&rec);
        assert_eq!(
            b,
            RmbBuildingData { name_seed: 0x1234, faction_id: 0x150, sector: -3, location_id: 0x42, building_type: 15, quality: 7 }
        );
    }
}
