use super::bsa::BsaArchive;

const RMB_PREFIXES: [&str; 45] = [
    "TVRN", "GENR", "RESI", "WEAP", "ARMR", "ALCH", "BANK", "BOOK", "CLOT", "FURN",
"GEMS", "LIBR", "PAWN", "TEMP", "TEMP", "PALA", "FARM", "DUNG", "CAST", "MANR",
"SHRI", "RUIN", "SHCK", "GRVE", "FILL", "KRAV", "KDRA", "KOWL", "KMOO", "KCAN",
"KFLA", "KHOR", "KROS", "KWHE", "KSCA", "KHAW", "MAGE", "THIE", "DARK", "FIGH",
"CUST", "WALL", "MARK", "SHIP", "WITC",
];

const LETTER2_ARRAY: [char; 4] = ['A', 'L', 'M', 'S'];

/// Prima lettera usata se `letter1_for_rmb_name` non è ancora letto dal record e il
/// bit 0x10 è acceso: riproduce la mappa originale (B per 0x3F/0x5F/0x7F, altrimenti A).
fn legacy_letter1(char_byte: u8) -> char {
    if matches!(char_byte, 0x3F | 0x5F | 0x7F) { 'B' } else { 'A' }
}

/// Porting di `resolveRmbBlockName` da Daggerfall Unity, con fallback legacy per i
/// byte non standard per garantire che i nomi coincidano con quelli presenti in BLOCKS.BSA.
pub fn decode_rmb_name(
    index: u8,
    number: u8,
    char_byte: u8,
    letter1_for_rmb_name: Option<u8>,
) -> Option<String> {
    let prefix = RMB_PREFIXES.get(index as usize)?;

    // Letter 1
    let letter1 = if char_byte & 0x10 != 0 {
        match letter1_for_rmb_name {
            Some(c) if c != 0 => c as char,
            _ => legacy_letter1(char_byte),
        }
    } else {
        'A'
    };

    // Letter 2: bit 5-6 del byte, con salvaguardia sui vecchi codici Daggerfall
    let letter2 = if matches!(char_byte, 0x0F | 0x1F | 0x8F | 0x9F | 0x11 | 0x13 | 0x15) {
        'A'
    } else {
        LETTER2_ARRAY[(char_byte.wrapping_mul(2) >> 6) as usize]
    };

    // Numero del blocco
    let numbers = if index == 13 || index == 14 {
        // Templi: lettera dal nibble basso + numero senza padding (es. TEMPAAA0.RMB)
        format!("{}{}", (b'A' + (char_byte & 0x0F)) as char, number)
    } else {
        format!("{:02}", number)
    };

    Some(format!("{}{}{}{}.RMB", prefix, letter1, letter2, numbers))
}

#[derive(Debug, Clone)]
pub struct DFLocation {
    pub name: String,
    pub region_index: usize,
    pub location_id: u32,
    pub world_x: i32,
    pub world_y: i32,
    pub width: usize,
    pub height: usize,
    pub block_names: Vec<String>,
    /// `letter1ForRMBName` letto da ExteriorData (codice ASCII)
    pub letter1_for_rmb_name: Option<u8>,
}

pub struct MapsReader<'a> {
    archive: &'a BsaArchive<'a>,
}

impl<'a> MapsReader<'a> {
    pub fn new(archive: &'a BsaArchive<'a>) -> Self {
        Self { archive }
    }

    /// Legge direttamente una località dato l'indice della regione e l'indice della location
    pub fn get_location(&self, region_index: usize, loc_idx: usize) -> Option<DFLocation> {
        let names_file = format!("MAPNAMES.{:03}", region_index);
        let pitem_file = format!("MAPPITEM.{:03}", region_index);

        let names_raw = self.archive.get_by_name(&names_file)?;
        let pitem_raw = self.archive.get_by_name(&pitem_file)?;

        if names_raw.len() < 4 || pitem_raw.len() < 4 {
            return None;
        }

        let total_locations = u32::from_le_bytes(names_raw[0..4].try_into().ok()?) as usize;
        if loc_idx >= total_locations {
            return None;
        }

        // Estrazione nome località (32 byte null-terminated)
        let name_p = 4 + loc_idx * 32;
        if name_p + 32 > names_raw.len() {
            return None;
        }
        let chunk = &names_raw[name_p..name_p + 32];
        let len = chunk.iter().position(|&b| b == 0).unwrap_or(32);
        let loc_name = String::from_utf8_lossy(&chunk[..len]).trim().to_string();

        let offset_table_size = total_locations * 4;
        let ptr_in_offsets = loc_idx * 4;
        if ptr_in_offsets + 4 > pitem_raw.len() {
            return None;
        }

        let rel_offset = u32::from_le_bytes(
            pitem_raw[ptr_in_offsets..ptr_in_offsets + 4].try_into().ok()?,
        ) as usize;

        let loc_offset = offset_table_size + rel_offset;
        if loc_offset + 128 > pitem_raw.len() {
            return None;
        }
        let data = &pitem_raw[loc_offset..];

        // DoorCount (u32 @0) * 6 byte per porta
        let prerecord_count = u32::from_le_bytes(data[0..4].try_into().ok()?) as usize;
        let mut p = 4 + prerecord_count * 6;

        if p + 71 > data.len() {
            return None;
        }

        // LocationRecordElementHeader:
        // +7..+11: X (world_x)
        // +15..+19: Y (world_y)
        // +33..+35: LocationId (u16)
        let world_x = i32::from_le_bytes(data[p + 7..p + 11].try_into().ok()?);
        let world_y = i32::from_le_bytes(data[p + 15..p + 19].try_into().ok()?);
        let location_id = u16::from_le_bytes(data[p + 33..p + 35].try_into().ok()?) as u32;

        p += 71 + 32 + 9;

        if p + 2 > data.len() {
            return None;
        }
        // BuildingCount (u16) * 26 byte per edificio
        let postrecord_count = u16::from_le_bytes(data[p..p + 2].try_into().ok()?) as usize;
        p += 2 + 5 + postrecord_count * 26 + 32 + 4 + 4;

        if p + 2 > data.len() {
            return None;
        }
        let width = data[p] as usize;
        let height = data[p + 1] as usize;

        if width == 0 || height == 0 || width > 8 || height > 8 {
            return None;
        }

        // ExteriorData:
        // +0: Width, +1: Height
        // +2..+6: Unknown2 (4 byte)
        // +6: Letter1ForRMBName (1 byte)
        let letter1_for_rmb_name = data.get(p + 6).copied();

        p += 2 + 7;

        if p + 192 > data.len() {
            return None;
        }
        let block_indices = &data[p..p + 64];
        let block_numbers = &data[p + 64..p + 128];
        let block_chars = &data[p + 128..p + 192];

        let mut block_names = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let i = y * width + x;
                let blk = decode_rmb_name(
                    block_indices[i],
                    block_numbers[i],
                    block_chars[i],
                    letter1_for_rmb_name,
                )
                .unwrap_or_else(|| "EMPTY.RMB".to_string());
                block_names.push(blk);
            }
        }

        Some(DFLocation {
            name: loc_name,
            region_index,
            location_id,
            world_x,
            world_y,
            width,
            height,
            block_names,
            letter1_for_rmb_name,
        })
    }

    pub fn find_location(&self, region_index: usize, location_name: &str) -> Option<DFLocation> {
        let target = location_name.trim().to_ascii_uppercase();
        let names_file = format!("MAPNAMES.{:03}", region_index);
        let names_raw = self.archive.get_by_name(&names_file)?;
        if names_raw.len() < 4 {
            return None;
        }

        let total_locations = u32::from_le_bytes(names_raw[0..4].try_into().ok()?) as usize;
        for i in 0..total_locations {
            let p = 4 + i * 32;
            if p + 32 > names_raw.len() {
                break;
            }
            let chunk = &names_raw[p..p + 32];
            let len = chunk.iter().position(|&b| b == 0).unwrap_or(32);
            let name = String::from_utf8_lossy(&chunk[..len]).trim().to_ascii_uppercase();
            if name == target {
                return self.get_location(region_index, i);
            }
        }
        None
    }
}
