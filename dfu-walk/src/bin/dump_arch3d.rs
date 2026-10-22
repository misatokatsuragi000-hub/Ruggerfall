use std::env;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arena2_dir = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/1CE16DBE48849731/Daggerfall/arena2"));

    let arch_path = arena2_dir.join("ARCH3D.BSA");
    let mut data = Vec::new();
    File::open(&arch_path)?.read_to_end(&mut data)?;

    let count = u16::from_le_bytes([data[0], data[1]]) as usize;
    let dir_type = u16::from_le_bytes([data[2], data[3]]);

    println!("ARCH3D.BSA:");
    println!("  Record count: {}", count);
    println!("  Directory type: 0x{:04X}", dir_type);

    if dir_type == 0x0100 {
        println!("  -> È una directory con NOMI ASCII!");
        let dir_offset = data.len() - (count * 18);
        for i in 0..count.min(20) {
            let p = dir_offset + i * 18;
            let name_raw = &data[p..p + 14];
            let len = name_raw.iter().position(|&b| b == 0).unwrap_or(14);
            println!("     Record [{}]: '{}'", i, String::from_utf8_lossy(&name_raw[..len]));
        }
    } else if dir_type == 0x0200 || dir_type == 0x0000 {
        println!("  -> È una directory NUMERICA!");
        let dir_offset = data.len() - (count * 8);
        for i in 0..count.min(20) {
            let p = dir_offset + i * 8;
            let id = u32::from_le_bytes(data[p..p + 4].try_into().unwrap());
            let sz = u32::from_le_bytes(data[p + 4..p + 8].try_into().unwrap());
            println!("     Record [{}]: ID = {}, Size = {} bytes", i, id, sz);
        }
    } else {
        println!("  -> Tipo directory inaspettato: 0x{:04X}", dir_type);
    }

    Ok(())
}