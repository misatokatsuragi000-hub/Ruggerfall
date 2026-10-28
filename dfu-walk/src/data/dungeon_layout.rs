//! Lettura del layout dungeon da MAPDITEM.nnn in MAPS.BSA.

use super::bsa::BsaArchive;

const RDB_BLOCK_LETTERS: [char; 6] = ['N', 'W', 'L', 'S', 'B', 'M'];

#[derive(Debug, Clone)]
pub struct DungeonBlockInfo {
    pub x: i8,
    pub z: i8,
    pub block_name: String,
    pub is_starting_block: bool,
}

#[derive(Debug, Clone)]
pub struct DungeonData {
    pub location_id: u32,
    pub blocks: Vec<DungeonBlockInfo>,
}

impl DungeonData {
    pub fn read_from_maps(
        maps_bsa: &BsaArchive,
        region_index: usize,
        location_id: u32,
    ) -> Option<Self> {
        let pitem_name = format!("MAPDITEM.{:03}", region_index);
        let data = maps_bsa.get_by_name(&pitem_name)?;

        if data.len() < 4 {
            return None;
        }

        let dungeon_count = u32::from_le_bytes(data[0..4].try_into().ok()?) as usize;
        let mut found_offset: Option<usize> = None;

        for i in 0..dungeon_count {
            let p = 4 + i * 8;
            if p + 8 > data.len() {
                break;
            }
            let offset = u32::from_le_bytes(data[p..p + 4].try_into().ok()?) as usize;
            let ext_loc_id = u16::from_le_bytes(data[p + 6..p + 8].try_into().ok()?) as u32;

            if ext_loc_id == location_id {
                found_offset = Some(offset);
                break;
            }
        }

        let offset = found_offset?;
        let rec_pos = 4 + dungeon_count * 8 + offset;
        if rec_pos + 120 > data.len() {
            return None;
        }

        let rec_data = &data[rec_pos..];

        // 1. Salta il LocationRecordElement:
        //    DoorCount (u32 @0) * 6 byte
        let door_count = u32::from_le_bytes(rec_data[0..4].try_into().ok()?) as usize;
        let mut p = 4 + door_count * 6;

        //    Header fisso da 71 byte + 32 byte nome + 9 byte unknowns
        p += 71 + 32 + 9;

        if p + 16 > rec_data.len() {
            return None;
        }

        // 2. DungeonHeader:
        //    NullValue1: u16 (2)
        //    Unknown1: u32 (4)
        //    Unknown2: u32 (4)
        //    BlockCount: u16 (2)
        //    Unknown3: [u8; 5] (5)
        let block_count = u16::from_le_bytes(rec_data[p + 10..p + 12].try_into().ok()?) as usize;
        p += 2 + 4 + 4 + 2 + 5;

        // 3. DungeonBlocks: ciascuno è 4 byte (X: i8, Z: i8, bitfield: u16)
        let mut blocks = Vec::with_capacity(block_count);
        for i in 0..block_count {
            let bp = p + i * 4;
            if bp + 4 > rec_data.len() {
                break;
            }
            let x = rec_data[bp] as i8;
            let z = rec_data[bp + 1] as i8;
            let bitfield = u16::from_le_bytes(rec_data[bp + 2..bp + 4].try_into().ok()?);

            let block_number = bitfield & 0x03FF;
            let is_starting_block = (bitfield & 0x0400) != 0;
            let letter_idx = (bitfield >> 11) as usize;
            let letter = if letter_idx < RDB_BLOCK_LETTERS.len() {
                RDB_BLOCK_LETTERS[letter_idx]
            } else {
                'N'
            };

            let block_name = format!("{}{:07}.RDB", letter, block_number);

            blocks.push(DungeonBlockInfo {
                x,
                z,
                block_name,
                is_starting_block,
            });
        }

        // Patch DFU: Orsinium fix
        if location_id == 50015 && blocks.len() > 13 {
            blocks[13].z = -2;
        }

        Some(DungeonData {
            location_id,
            blocks,
        })
    }
}
