use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

const RMB_PREFIXES: [&str; 45] = [
    "TVRN", "GENR", "RESI", "WEAP", "ARMR", "ALCH", "BANK", "BOOK", "CLOT", "FURN", // 0..9
"GEMS", "LIBR", "PAWN", "TEMP", "TEMP", "PALA", "FARM", "DUNG", "CAST", "MANR", // 10..19
"SHRI", "RUIN", "SHCK", "GRVE", "FILL", "KRAV", "KDRA", "KOWL", "KMOO", "KCAN", // 20..29
"KPAS", "KROS", "KNOT", "WARP", "MONU", "DRAG", "CUST", "MAGE", "THIE", "FIGH", // 30..39 (FIGH a 4 lettere!)
"TEMP", "WALL", "MARK", "SHIP", "WITC"                                          // 40..44
];

fn decode_rmb_name(index: u8, number: u8, char_byte: u8) -> Option<String> {
    let prefix = RMB_PREFIXES.get(index as usize)?;

    // Gestione speciale MAGE: usa sempre AA
    if *prefix == "MAGE" {
        return Some(format!("MAGEAA{:02}.RMB", number));
    }

    // Gestione speciale Templi: multi-blocco con lettere A0..H0
    if *prefix == "TEMP" {
        if number == 0 {
            return Some("TEMPAA00.RMB".to_string());
        } else if (1..=8).contains(&number) {
            let letter = (b'A' + (number - 1)) as char;
            return Some(format!("TEMPAA{}0.RMB", letter));
        }
    }

    // Gestione speciale THIE: solo variante 00
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

struct BsaArchive<'a> {
    data: &'a [u8],
    entries: HashMap<String, (usize, usize)>,
}

impl<'a> BsaArchive<'a> {
    fn parse(data: &'a [u8]) -> Result<Self, &'static str> {
        let count = u16::from_le_bytes([data[0], data[1]]) as usize;
        let dir_type = u16::from_le_bytes([data[2], data[3]]);

        if dir_type != 0x0100 {
            return Err("Directory BSA non di tipo 0x0100");
        }

        let entry_size = 18;
        let dir_bytes_len = count * entry_size;
        let dir_offset = data.len().checked_sub(dir_bytes_len).ok_or("Offset directory errato")?;

        let mut entries = HashMap::with_capacity(count);
        let mut current_offset = 4usize;

        for i in 0..count {
            let p = dir_offset + i * entry_size;
            let name_raw = &data[p..p + 14];
            let rec_size = u32::from_le_bytes(data[p + 14..p + 18].try_into().unwrap()) as usize;

            let name_len = name_raw.iter().position(|&b| b == 0).unwrap_or(14);
            let name = String::from_utf8_lossy(&name_raw[..name_len]).trim().to_ascii_uppercase();

            entries.insert(name, (current_offset, rec_size));
            current_offset += rec_size;
        }

        Ok(Self { data, entries })
    }

    fn get(&self, name: &str) -> Option<&'a [u8]> {
        let (offset, size) = *self.entries.get(&name.to_ascii_uppercase())?;
        Some(&self.data[offset..offset + size])
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arena2_dir = env::args()
    .nth(1)
    .map(PathBuf::from)
    .unwrap_or_else(|| PathBuf::from("/mnt/1CE16DBE48849731/Daggerfall/arena2"));

    // Carica MAPS.BSA
    let maps_path = arena2_dir.join("MAPS.BSA");
    let mut maps_buf = Vec::new();
    File::open(&maps_path)?.read_to_end(&mut maps_buf)?;
    let maps_bsa = BsaArchive::parse(&maps_buf)?;

    // Carica BLOCKS.BSA
    let blocks_path = arena2_dir.join("BLOCKS.BSA");
    let mut blocks_buf = Vec::new();
    File::open(&blocks_path)?.read_to_end(&mut blocks_buf)?;
    let blocks_bsa = BsaArchive::parse(&blocks_buf)?;

    println!("=== Varianti presenti in BLOCKS.BSA per i blocchi speciali ===");
    let mut special_matches = Vec::new();
    for name in blocks_bsa.entries.keys() {
        if name.starts_with("MAG")
            || name.starts_with("THI")
            || name.starts_with("FIG")
            || name.starts_with("TEM")
            {
                special_matches.push(name.as_str());
            }
    }
    special_matches.sort();
    for name in special_matches {
        println!("  {}", name);
    }

    println!("\n=== Daggerfall Capital City Layout ===");
    let mappitem_raw = maps_bsa.get("MAPPITEM.017").unwrap();
    let mapnames_raw = maps_bsa.get("MAPNAMES.017").unwrap();

    let total_locations = u32::from_le_bytes(mapnames_raw[0..4].try_into().unwrap()) as usize;
    let target_idx = 1231; // Daggerfall
    let offset_table_size = total_locations * 4;
    let ptr_in_offsets = target_idx * 4;
    let record_rel_offset = u32::from_le_bytes(
        mappitem_raw[ptr_in_offsets..ptr_in_offsets + 4].try_into().unwrap()
    ) as usize;

    let loc_offset = offset_table_size + record_rel_offset;
    let data = &mappitem_raw[loc_offset..];

    let prerecord_count = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
    let mut p = 4 + prerecord_count * 6;
    p += 71; // Header
    p += 32; // Name
    p += 9;  // Unknowns

    let postrecord_count = u16::from_le_bytes(data[p..p + 2].try_into().unwrap()) as usize;
    p += 2 + 5 + postrecord_count * 26 + 32 + 4 + 4;

    let block_width = data[p] as usize;
    let block_height = data[p + 1] as usize;
    p += 2 + 7;

    let block_indices = &data[p..p + 64];
    let block_numbers = &data[p + 64..p + 128];
    let block_chars = &data[p + 128..p + 192];

    println!("Dimensioni griglia: {}x{} blocchi RMB\n", block_width, block_height);

    for y in 0..block_height {
        let mut row_str = String::new();
        for x in 0..block_width {
            let i = y * block_width + x;
            let blk_name = decode_rmb_name(block_indices[i], block_numbers[i], block_chars[i])
            .unwrap_or_else(|| "??????.RMB".to_string());

            let exists = if blocks_bsa.get(&blk_name).is_some() { "OK" } else { "NOT FOUND" };
            row_str.push_str(&format!("[{}: {}] ", blk_name, exists));
        }
        println!("Riga {}: {}", y, row_str);
    }

    Ok(())
}
