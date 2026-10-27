//! Parser per i blocchi dungeon RDB (BLOCKS.BSA), porting fedele da
//! `BlocksFile.cs` di Daggerfall Unity.
//!
//! ## Convenzione di rotazione
//!
//! DFU legge le rotazioni come **negative**: `-XRotation / Div`,
//! `-YRotation / Div`, `-ZRotation / Div`. La convenzione originale di
//! Daggerfall è oraria vista dall'alto, mentre `Quat::from_rotation_*(+ang)`
//! di Bevy è antioraria (regola della mano destra). Senza il segno meno, ogni
//! oggetto RDB risulta ruotato nel verso opposto: l'effetto visibile è che
//! l'ingresso del dungeon sembra ruotato di 180° e i blocchi non si allineano.

use std::str::FromStr;

pub const RDB_SIDE_UNITS: f32 = 2048.0;
pub const RDB_SIDE_METERS: f32 = RDB_SIDE_UNITS * 0.025; // 51.2 metri per blocco
pub const RDB_DA_TO_DEG: f32 = 360.0 / 2048.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RdbResourceType {
    Model = 1,
    Light = 2,
    Flat = 3,
    Unknown,
}

impl From<u8> for RdbResourceType {
    fn from(v: u8) -> Self {
        match v {
            1 => RdbResourceType::Model,
            2 => RdbResourceType::Light,
            3 => RdbResourceType::Flat,
            _ => RdbResourceType::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RdbModelRef {
    pub model_id: u32,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct RdbModelResource {
    /// Rotazione X in gradi, già negata (convenzione DFU).
    pub x_rot_deg: f32,
    /// Rotazione Y in gradi, già negata (convenzione DFU).
    pub y_rot_deg: f32,
    /// Rotazione Z in gradi, già negata (convenzione DFU).
    pub z_rot_deg: f32,
    pub model_index: u16,
    pub model_id: u32,
    pub action_offset: i32,
}

#[derive(Debug, Clone)]
pub struct RdbFlatResource {
    pub archive: u16,
    pub record: u8,
    pub faction_or_mobile_id: u16,
    pub flags: u16,
}

#[derive(Debug, Clone)]
pub struct RdbLightResource {
    pub radius: u16,
}

#[derive(Debug, Clone)]
pub enum RdbResource {
    Model(RdbModelResource),
    Flat(RdbFlatResource),
    Light(RdbLightResource),
    None,
}

#[derive(Debug, Clone)]
pub struct RdbObject {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub resource: RdbResource,
}

#[derive(Debug, Clone)]
pub struct RdbBlock {
    pub name: String,
    pub model_refs: Vec<RdbModelRef>,
    pub objects: Vec<RdbObject>,
}

fn read_c_str(data: &[u8], len: usize) -> String {
    let slice = &data[..len.min(data.len())];
    let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
    String::from_utf8_lossy(&slice[..end]).trim().to_string()
}

impl RdbBlock {
    pub fn parse(name: &str, data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < 20 {
            return Err("Dati RDB troppo brevi per l'header");
        }

        let width = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
        let height = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
        let root_offset = u32::from_le_bytes(data[12..16].try_into().unwrap()) as usize;

        // 1. ModelReferenceList: 750 record da 8 byte (offset 20..6020)
        let mut p = 20;
        let mut model_refs = Vec::with_capacity(750);
        for _ in 0..750 {
            if p + 8 > data.len() {
                return Err("Buffer RDB terminato durante la ModelReferenceList");
            }
            let id_str = read_c_str(&data[p..p + 5], 5);
            let desc_str = read_c_str(&data[p + 5..p + 8], 3);
            let model_id = u32::from_str(&id_str).unwrap_or(0);
            model_refs.push(RdbModelRef {
                model_id,
                description: desc_str,
            });
            p += 8;
        }

        // 2. ObjectRootList (Width * Height offset i32)
        if root_offset >= data.len() {
            return Err("ObjectRootOffset oltre la fine del file RDB");
        }

        let total_roots = width * height;
        let roots_end = root_offset + total_roots * 4;
        if roots_end > data.len() {
            return Err("ObjectRootList oltre la fine del file RDB");
        }

        let mut root_offsets = Vec::with_capacity(total_roots);
        for i in 0..total_roots {
            let off = i32::from_le_bytes(
                data[root_offset + i * 4..root_offset + i * 4 + 4]
                .try_into()
                .unwrap(),
            );
            if off >= 0 {
                root_offsets.push(off as usize);
            }
        }

        // 3. Traversal della linked list per ciascuna radice
        let mut objects = Vec::new();
        for mut obj_pos in root_offsets {
            let mut visited = 0;
            while obj_pos + 25 <= data.len() && visited < 5000 {
                visited += 1;
                let next = i32::from_le_bytes(data[obj_pos..obj_pos + 4].try_into().unwrap());
                let x = i32::from_le_bytes(data[obj_pos + 8..obj_pos + 12].try_into().unwrap());
                let y = i32::from_le_bytes(data[obj_pos + 12..obj_pos + 16].try_into().unwrap());
                let z = i32::from_le_bytes(data[obj_pos + 16..obj_pos + 20].try_into().unwrap());
                let r_type = RdbResourceType::from(data[obj_pos + 20]);
                let res_offset =
                u32::from_le_bytes(data[obj_pos + 21..obj_pos + 25].try_into().unwrap()) as usize;

                let resource = if res_offset < data.len() {
                    match r_type {
                        RdbResourceType::Model => {
                            if res_offset + 23 <= data.len() {
                                let xr = i32::from_le_bytes(
                                    data[res_offset..res_offset + 4].try_into().unwrap(),
                                );
                                let yr = i32::from_le_bytes(
                                    data[res_offset + 4..res_offset + 8].try_into().unwrap(),
                                );
                                let zr = i32::from_le_bytes(
                                    data[res_offset + 8..res_offset + 12].try_into().unwrap(),
                                );
                                let m_idx = u16::from_le_bytes(
                                    data[res_offset + 12..res_offset + 14].try_into().unwrap(),
                                );
                                let act_off = i32::from_le_bytes(
                                    data[res_offset + 19..res_offset + 23].try_into().unwrap(),
                                );

                                let model_id = model_refs
                                .get(m_idx as usize)
                                .map(|m| m.model_id)
                                .unwrap_or(0);

                                // FIX: le rotazioni sono NEGATIVE, come in DFU
                                // (`-XRotation / Div`, `-YRotation / Div`,
                                // `-ZRotation / Div`). Senza il segno meno, il
                                // dungeon risulta ruotato di 180° e i blocchi
                                // non si allineano tra loro.
                                RdbResource::Model(RdbModelResource {
                                    x_rot_deg: -((xr as f32) * RDB_DA_TO_DEG),
                                                   y_rot_deg: -((yr as f32) * RDB_DA_TO_DEG),
                                                   z_rot_deg: -((zr as f32) * RDB_DA_TO_DEG),
                                                   model_index: m_idx,
                                                   model_id,
                                                   action_offset: act_off,
                                })
                            } else {
                                RdbResource::None
                            }
                        }
                        RdbResourceType::Flat => {
                            if res_offset + 6 <= data.len() {
                                let bitfield = u16::from_le_bytes(
                                    data[res_offset..res_offset + 2].try_into().unwrap(),
                                );
                                let flags = u16::from_le_bytes(
                                    data[res_offset + 2..res_offset + 4].try_into().unwrap(),
                                );
                                let mag = data[res_offset + 4];
                                let snd = data[res_offset + 5];
                                let fact = u16::from_le_bytes([mag, snd]);

                                RdbResource::Flat(RdbFlatResource {
                                    archive: bitfield >> 7,
                                    record: (bitfield & 0x7F) as u8,
                                                  faction_or_mobile_id: fact,
                                                  flags,
                                })
                            } else {
                                RdbResource::None
                            }
                        }
                        RdbResourceType::Light => {
                            if res_offset + 10 <= data.len() {
                                let radius = u16::from_le_bytes(
                                    data[res_offset + 8..res_offset + 10].try_into().unwrap(),
                                );
                                RdbResource::Light(RdbLightResource { radius })
                            } else {
                                RdbResource::None
                            }
                        }
                        _ => RdbResource::None,
                    }
                } else {
                    RdbResource::None
                };

                objects.push(RdbObject { x, y, z, resource });

                if next < 0 {
                    break;
                }
                obj_pos = next as usize;
            }
        }

        Ok(RdbBlock {
            name: name.to_string(),
           model_refs,
           objects,
        })
    }
}
