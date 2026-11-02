//! Parser dei blocchi RMB di BLOCKS.BSA, secondo la specifica UESP.
//!
//! ## Fix 1: `n_of` per i misc objects
//!
//! I misc objects (`o.block = None`) sono quegli `objects_3d` che non
//! appartengono a nessun subrecord. La formula originale
//!
//! ```ignore
//! if z_raw < 0.0 { BLOCK_UNITS + z_raw } else { BLOCK_UNITS - z_raw }
//! ```
//!
//! è **simmetrica**: `z_raw = +500` e `z_raw = -500` danno entrambi
//! `4096 - 500 = 3596`. Due pali posti simmetricamente rispetto al centro
//! del blocco finiscono nello **stesso punto**, spostati di decine di metri.
//!
//! Il fix usa `z_raw.abs()` (default) oppure una delle altre interpretazioni,
//! selezionabili con `DF_MISC_Z_MODE`:
//!
//! - `abs` (default): `z.abs()` — simmetrico ma "non perde" il segno
//! - `center`: `z + BLOCK_UNITS/2` — z relativo al centro del blocco
//! - `flip`: `BLOCK_UNITS - z` — z = distanza dal nord, invertita
//! - `raw`: `z` — z già "distanza dal sud"
//! - `orig`: formula originale (simmetrica, bug)
//!
//! ## Fix 2: `rotation_sign` configurabile
//!
//! `ROTATION_SIGN = -1.0` è quello che serve alle case (che sono a posto
//! e non vogliamo rompere). Ma se alcuni pali sembrano "ruotati nel verso
//! opposto" rispetto all'edificio, si può provare `DF_ROTATION_SIGN=1`.
//! Il default resta `-1.0` (comportamento attuale preservato).
//!
//! La variabile viene letta una volta sola con `OnceLock`, quindi non c'è
//! overhead per ogni `placed_models`.

use std::sync::OnceLock;

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

pub const Z_FROM_NORTH_EDGE: bool = true;
pub const ROTATION_SIGN: f32 = -1.0;
const BLOCK_UNITS: f32 = 4096.0;
pub const DA_TO_DEG: f32 = 360.0 / 2048.0;

/// Segno di rotazione attivo. Di default `ROTATION_SIGN` (-1.0), ma può
/// essere sovrascritto con `DF_ROTATION_SIGN` (es. `DF_ROTATION_SIGN=1`).
///
/// Viene letto **una volta sola** (`OnceLock`), quindi non c'è overhead
/// per ogni `placed_models` (che è chiamato migliaia di volte per città).
///
/// Se le case si rompono con `DF_ROTATION_SIGN=1`, il segno corretto per
/// il tuo asset è `-1.0` (default) e il problema dei pali è altrove.
/// Verso della componente Z dell'offset locale degli oggetti dentro un sottoblocco (edificio):
/// `DF_SUBREC_LZ_SIGN=-1` la specchia. Le case hanno offset (0,0), quindi non lo mostrano; pali,
/// travi e insegne si': se finiscono sul lato sbagliato dell'edificio, provare -1.
pub fn subrec_lz_sign() -> f32 {
    static V: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("DF_SUBREC_LZ_SIGN").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0))
}

pub fn rotation_sign() -> f32 {
    static SIGN: OnceLock<f32> = OnceLock::new();
    *SIGN.get_or_init(|| {
        std::env::var("DF_ROTATION_SIGN")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(ROTATION_SIGN)
    })
}

/// Modalità di calcolo della `n` (distanza dal bordo sud) per i misc objects
/// (`o.block = None`). Vedi il commento in testa al file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MiscZMode {
    /// `z.abs()` — simmetrico ma "esplicito". **Default** (Fix 1).
    Abs,
    /// `z + BLOCK_UNITS/2` — z relativo al centro del blocco.
    Center,
    /// `BLOCK_UNITS - z` — z = distanza dal nord, senza abs.
    Flip,
    /// `z` — z già "distanza dal sud".
    Raw,
    /// Formula originale: `if z < 0 { BLOCK_UNITS + z } else { BLOCK_UNITS - z }`.
    /// Simmetrica, è il bug che Fix 1 corregge.
    Orig,
}

/// Modalità attiva per `n_of` dei misc objects.
///
/// Letta da `DF_MISC_Z_MODE` una volta sola (`OnceLock`), con default `Abs`.
/// Valori accettati: `abs` (default), `center`, `flip`, `raw`, `orig`.
fn misc_z_mode() -> MiscZMode {
    static MODE: OnceLock<MiscZMode> = OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("DF_MISC_Z_MODE").ok().as_deref() {
        Some("center") => MiscZMode::Center,
                      Some("flip") => MiscZMode::Flip,
                      Some("raw") => MiscZMode::Raw,
                      Some("orig") => MiscZMode::Orig,
                      _ => MiscZMode::Abs,
    })
}

fn n_of_misc(z_raw: f32, mode: MiscZMode) -> f32 {
    match mode {
        MiscZMode::Abs => z_raw.abs(),
        MiscZMode::Center => z_raw + BLOCK_UNITS * 0.5,
        MiscZMode::Flip => {
            if Z_FROM_NORTH_EDGE {
                BLOCK_UNITS - z_raw
            } else {
                z_raw
            }
        }
        MiscZMode::Raw => z_raw,
        MiscZMode::Orig => {
            if Z_FROM_NORTH_EDGE {
                if z_raw < 0.0 {
                    BLOCK_UNITS + z_raw
                } else {
                    BLOCK_UNITS - z_raw
                }
            } else {
                z_raw.abs()
            }
        }
    }
}

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RmbDoor {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

#[derive(Debug, Clone, Default)]
pub struct RmbInterior {
    pub objects_3d: Vec<RmbObject3D>,
    pub flats: Vec<RmbFlatObject>,
    pub people: Vec<RmbFlatObject>,
    pub doors: Vec<RmbDoor>,
}

#[derive(Debug, Clone)]
pub struct RmbSubRecord {
    pub x: i32,
    pub z: i32,
    pub rotation_deg: f32,
}

#[derive(Debug, Clone)]
pub struct RmbObject3D {
    pub raw_id: u32,
    pub model_id: u32,
    pub object_type: u8,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub pivot: [i32; 3],
    pub rotation_y_deg: f32,
    /// Rotazione attorno a X (inclinazione), stesso formato di `rotation_y_deg` (letta a +48).
    pub rotation_x_deg: f32,
    /// Rotazione attorno a Z (inclinazione) (letta a +56).
    pub rotation_z_deg: f32,
    /// Byte grezzi 46..66 del record come 10 i16, per diagnosticare i campi non ancora decodificati.
    pub tail: [i16; 10],
    pub block: Option<u8>,
}

#[derive(Debug, Clone, Copy)]
pub struct PlacedModel {
    pub model_id: u32,
    pub object_type: u8,
    pub x: f32,
    pub n: f32,
    pub y: i32,
    pub rotation_deg: f32,
    /// Inclinazioni (gradi, stesso segno di `rotation_deg`): travi, insegne e pali orizzontali.
    pub rotation_x_deg: f32,
    pub rotation_z_deg: f32,
    pub tail: [i16; 10],
    pub is_building: bool,
    pub subrecord: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct RmbFlatObject {
    pub texture_archive: u16,
    pub texture_record: u8,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub faction_id: i16,
    pub flags: u8,
    pub block: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct RmbBlock {
    pub ground_tiles: [u8; 16 * 16],
    pub ground_decor: [u8; 16 * 16],
    pub subrecords: Vec<RmbSubRecord>,
    pub buildings: Vec<RmbBuildingData>,
    pub objects_3d: Vec<RmbObject3D>,
    pub flats: Vec<RmbFlatObject>,
    pub interiors: Vec<RmbInterior>,
    pub exterior_people: Vec<RmbFlatObject>,
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
        rotation_x_deg: i16::from_le_bytes([d[p + 48], d[p + 49]]) as f32 * DA_TO_DEG,
        rotation_z_deg: i16::from_le_bytes([d[p + 56], d[p + 57]]) as f32 * DA_TO_DEG,
        tail: std::array::from_fn(|k| i16::from_le_bytes([d[p + 46 + k * 2], d[p + 47 + k * 2]])),
        block,
    }
}

fn parse_interior(d: &[u8], q: usize, end: usize, block: u8) -> RmbInterior {
    let mut out = RmbInterior::default();
    if q + BLOCKDATA_HEADER > end {
        return out;
    }
    let nm = d[q] as usize;
    let nf = d[q + 1] as usize;
    let ns = d[q + 2] as usize;
    let np = d[q + 3] as usize;
    let nd = d[q + 4] as usize;
    let total = BLOCKDATA_HEADER
    + nm * MODEL_SIZE
    + nf * FLAT_SIZE
    + ns * SECTION3_SIZE
    + np * PERSON_SIZE
    + nd * DOOR_SIZE;
    if q + total > end {
        return out;
    }
    let mut p = q + BLOCKDATA_HEADER;
    for _ in 0..nm {
        out.objects_3d.push(parse_model(d, p, Some(block)));
        p += MODEL_SIZE;
    }
    for _ in 0..nf {
        out.flats.push(parse_flat(d, p, Some(block)));
        p += FLAT_SIZE;
    }
    p += ns * SECTION3_SIZE;
    for _ in 0..np {
        out.people.push(parse_flat(d, p, Some(block)));
        p += PERSON_SIZE;
    }
    for _ in 0..nd {
        out.doors.push(RmbDoor {
            x: i32_at(d, p),
                       y: i32_at(d, p + 4),
                       z: i32_at(d, p + 8),
        });
        p += DOOR_SIZE;
    }
    out
}

fn parse_flat(d: &[u8], p: usize, block: Option<u8>) -> RmbFlatObject {
    let tex = u16::from_le_bytes([d[p + 12], d[p + 13]]);
    let faction_id = i16::from_le_bytes([d[p + 14], d[p + 15]]);
    let flags = d[p + 16];
    RmbFlatObject {
        texture_archive: tex >> 7,
        texture_record: (tex & 0x7F) as u8,
        x: i32_at(d, p),
        y: i32_at(d, p + 4),
        z: i32_at(d, p + 8),
        faction_id,
        flags,
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
        let mut interiors = Vec::with_capacity(block_count);
        let mut exterior_people = Vec::new();
        let mut pos = RMB_HEADER_LEN;

        for i in 0..block_count {
            let size = u32::from_le_bytes(
                data[OFF_BLOCK_SIZES + i * 4..OFF_BLOCK_SIZES + i * 4 + 4].try_into().unwrap(),
            ) as usize;
            let end = pos.checked_add(size).ok_or("BlockSize assurda")?;
            if end > data.len() {
                return Err("Block oltre la fine del record");
            }
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
            p += ns * SECTION3_SIZE;
            for _ in 0..np {
                exterior_people.push(parse_flat(data, p, Some(i as u8)));
                p += PERSON_SIZE;
            }
            interiors.push(parse_interior(data, q + total, end, i as u8));
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

        Ok(Self {
            ground_tiles,
            ground_decor,
            subrecords,
            buildings,
            objects_3d,
            flats,
            interiors,
            exterior_people,
            bytes_used: pos,
        })
    }

    /// Posizioni nel blocco di tutti gli oggetti 3D.
    ///
    /// Vedi `n_of_misc` e `rotation_sign` per il comportamento configurabile.
    pub fn placed_models(&self) -> Vec<PlacedModel> {
        let sign = rotation_sign();
        let z_mode = misc_z_mode();

        // Per i misc objects, `n_of_misc` sceglie come interpretare `o.z`.
        // Il default (`Abs`) è il Fix 1.
        let n_of = |z_raw: f32| -> f32 { n_of_misc(z_raw, z_mode) };

        self.objects_3d
        .iter()
        .map(|o| match o.block {
            None => PlacedModel {
                model_id: o.model_id,
                object_type: o.object_type,
                x: o.x as f32,
                n: n_of(o.z as f32),
             y: o.y,
             rotation_deg: o.rotation_y_deg * sign,
             rotation_x_deg: o.rotation_x_deg * sign,
             rotation_z_deg: o.rotation_z_deg * sign,
             tail: o.tail,
             is_building: false,
             subrecord: None,
            },
            Some(i) => {
                let s = &self.subrecords[i as usize];
                let anchor_n = if Z_FROM_NORTH_EDGE {
                    BLOCK_UNITS - (s.z as f32).abs()
                } else {
                    (s.z as f32).abs()
                };
                let phi = (s.rotation_deg * sign).to_radians();
                let (sn, cs) = phi.sin_cos();
                let lx = o.x as f32;
                let ln = (if Z_FROM_NORTH_EDGE { o.z as f32 } else { -(o.z as f32) }) * subrec_lz_sign();
                PlacedModel {
                    model_id: o.model_id,
                    object_type: o.object_type,
                    x: s.x as f32 + lx * cs - ln * sn,
                    n: anchor_n + lx * sn + ln * cs,
                    y: o.y,
                    rotation_deg: (s.rotation_deg + o.rotation_y_deg) * sign,
             rotation_x_deg: o.rotation_x_deg * sign,
             rotation_z_deg: o.rotation_z_deg * sign,
             tail: o.tail,
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
    fn misc_z_modes_are_distinct() {
        // Verifica che le modalità producano valori diversi per z=+500 e z=-500,
        // tranne `Orig` che è proprio il bug da correggere.
        let pos = 500.0f32;
        let neg = -500.0f32;

        // `Abs`: simmetrico ma esplicito (Fix 1).
        assert_eq!(n_of_misc(pos, MiscZMode::Abs), 500.0);
        assert_eq!(n_of_misc(neg, MiscZMode::Abs), 500.0);

        // `Center`: non simmetrico, z relativo al centro.
        assert_eq!(n_of_misc(pos, MiscZMode::Center), 500.0 + 2048.0);
        assert_eq!(n_of_misc(neg, MiscZMode::Center), -500.0 + 2048.0);

        // `Flip`: non simmetrico, z = distanza dal nord.
        assert_eq!(n_of_misc(pos, MiscZMode::Flip), 4096.0 - 500.0);
        assert_eq!(n_of_misc(neg, MiscZMode::Flip), 4096.0 - (-500.0));

        // `Raw`: non simmetrico, z = distanza dal sud.
        assert_eq!(n_of_misc(pos, MiscZMode::Raw), 500.0);
        assert_eq!(n_of_misc(neg, MiscZMode::Raw), -500.0);

        // `Orig`: simmetrico (il bug).
        assert_eq!(n_of_misc(pos, MiscZMode::Orig), 4096.0 - 500.0);
        assert_eq!(n_of_misc(neg, MiscZMode::Orig), 4096.0 + (-500.0));
        // I due valori sono identici: è il bug che Fix 1 corregge con Abs.
        assert_eq!(
            n_of_misc(pos, MiscZMode::Orig),
                   n_of_misc(neg, MiscZMode::Orig)
        );
    }
}
