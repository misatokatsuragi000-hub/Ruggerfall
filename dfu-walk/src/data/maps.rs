use crate::bsa::BsaArchive;

const RMB_PREFIXES: [&str; 45] = [
    "TVRN", "GENR", "RESI", "WEAP", "ARMR", "ALCH", "BANK", "BOOK", "CLOT", "FURN",
"GEMS", "LIBR", "PAWN", "TEMP", "TEMP", "PALA", "FARM", "DUNG", "CAST", "MANR",
"SHRI", "RUIN", "SHCK", "GRVE", "FILL", "KRAV", "KDRA", "KOWL", "KMOO", "KCAN",
"KPAS", "KROS", "KNOT", "WARP", "MONU", "DRAG", "CUST", "MAGE", "THIE", "FIGH",
"TEMP", "WALL", "MARK", "SHIP", "WITC"
];

pub fn decode_rmb_name(index: u8, number: u8, char_byte: u8) -> Option<String> {
    let prefix = RMB_PREFIXES.get(index as usize)?;

    if *prefix == "MAGE" {
        return Some(format!("MAGEAA{:02}.RMB", number));
    }

    if *prefix == "TEMP" {
        if number == 0 {
            return Some("TEMPAA00.RMB".to_string());
        } else if (1..=8).contains(&number) {
            let letter = (b'A' + (number - 1)) as char;
            return Some(format!("TEMPAA{}0.RMB", letter));
        }
    }

    if *prefix == "THIE" {
        let size_str = match char_byte {
            0x2F => "AL",
            0x4F => "AM",
            0x6F => "AS",
            _ => "AL",
        };
        return Some(format!("THIE{}00.RMB", size_str));
    }

    let char_str = match char_byte {
        0x00..=0x07 | 0x8F => "AA",
        0x0F | 0x11 | 0x13 | 0x15 | 0x1F | 0x9F => "AA",
        0x2F => "AL",
        0x3F => "BL",
        0x4F => "AM",
        0x5F => "BM",
        0x6F => "AS",
        0x7F => "BS",
        _ => "AA",
    };

    Some(format!("{}{}{:02}.RMB", prefix, char_str, number))
}

#[derive(Debug, Clone)]
pub struct DFLocation {
    pub name: String,
    pub region_index: usize,
    pub world_x: i32,
    pub world_y: i32,
    pub width: usize,
    pub height: usize,
    pub block_names: Vec<String>,
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

        // Estrazione nome
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
            pitem_raw[ptr_in_offsets..ptr_in_offsets + 4].try_into().ok()?
        ) as usize;

        let loc_offset = offset_table_size + rel_offset;
        if loc_offset + 128 > pitem_raw.len() {
            return None;
        }
        let data = &pitem_raw[loc_offset..];

        let prerecord_count = u32::from_le_bytes(data[0..4].try_into().ok()?) as usize;
        let mut p = 4 + prerecord_count * 6;

        if p + 71 > data.len() {
            return None;
        }
        let world_x = i32::from_le_bytes(data[p + 7..p + 11].try_into().ok()?);
        let world_y = i32::from_le_bytes(data[p + 15..p + 19].try_into().ok()?);
        p += 71 + 32 + 9;

        if p + 2 > data.len() {
            return None;
        }
        let postrecord_count = u16::from_le_bytes(data[p..p + 2].try_into().ok()?) as usize;
        p += 2 + 5 + postrecord_count * 26 + 32 + 4 + 4;

        if p + 2 > data.len() {
            return None;
        }
        let width = data[p] as usize;
        let height = data[p + 1] as usize;
        p += 2 + 7;

        if width == 0 || height == 0 || width > 8 || height > 8 {
            return None;
        }

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
                let blk = decode_rmb_name(block_indices[i], block_numbers[i], block_chars[i])
                .unwrap_or_else(|| "EMPTY.RMB".to_string());
                block_names.push(blk);
            }
        }

        Some(DFLocation {
            name: loc_name,
            region_index,
            world_x,
            world_y,
            width,
            height,
            block_names,
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
            if p + 32 > names_raw.len() { break; }
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
