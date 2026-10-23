//! Verifica gli offset di world_x / world_y nel LocationRecordElement (71 byte).
//! Uso: cargo run --bin dump_loc_raw -- <arena2_dir> <region> <idx>
use daggerfall_data::bsa::BsaArchive;
use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = env::args().collect();
    let dir = PathBuf::from(a.get(1).expect("arena2_dir"));
    let region: usize = a.get(2).expect("region").parse()?;
    let idx: usize = a.get(3).expect("idx").parse()?;

    let path = ["MAPS.BSA", "maps.bsa"].iter().map(|n| dir.join(n)).find(|p| p.exists())
        .expect("MAPS.BSA non trovato");
    let buf = fs::read(path)?;
    let bsa = BsaArchive::parse(&buf)?;

    let pitem = bsa.get_by_name(&format!("MAPPITEM.{:03}", region)).expect("MAPPITEM");
    let names = bsa.get_by_name(&format!("MAPNAMES.{:03}", region)).expect("MAPNAMES");
    let total = u32::from_le_bytes(names[0..4].try_into()?) as usize;
    assert!(idx < total, "idx fuori range ({} location)", total);

    let rel = u32::from_le_bytes(pitem[idx * 4..idx * 4 + 4].try_into()?) as usize;
    let data = &pitem[total * 4 + rel..];
    let pre = u32::from_le_bytes(data[0..4].try_into()?) as usize;
    let p = 4 + pre * 6;

    println!("prerecord_count = {}, LocationRecordElement @ {}", pre, p);
    for (i, chunk) in data[p..p + 120].chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{:02X}", b)).collect();
        println!("+{:03}: {}", i * 16, hex.join(" "));
    }

    println!("\nCandidati (i32 LE) per X/Y, offset allineati e non:");
    for off in [4usize, 7, 8, 11, 12, 15, 16, 19] {
        let v = i32::from_le_bytes(data[p + off..p + off + 4].try_into()?);
        let ok = v.abs() <= 4096 * 100;
        println!("  +{:02} -> {:>12} {}", off, v, if ok { "(plausibile)" } else { "" });
    }
    println!("\nConfronta con una citta' nota (es. Daggerfall = regione 17) e scegli la coppia (X, Y) coerente.");
    Ok(())
}
