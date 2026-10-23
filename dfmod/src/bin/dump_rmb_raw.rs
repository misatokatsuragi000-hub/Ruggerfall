//! Stampa i byte grezzi di un blocco RMB per ricostruirne il formato.
//! Uso: cargo run --release --bin dump_rmb_raw -- <ARENA2> [NOME.RMB]
//! Senza nome usa TVRNAL06.RMB (se esiste) e poi il blocco RMB più piccolo.

use std::path::PathBuf;

use daggerfall_data::bsa::BsaFile;
use daggerfall_data::find_file_ci;

fn hexdump(data: &[u8], from: usize, len: usize) {
    let end = (from + len).min(data.len());
    let mut o = from;
    while o < end {
        let row = &data[o..(o + 16).min(end)];
        let hex: Vec<String> = row.iter().map(|b| format!("{b:02X}")).collect();
        println!("{o:5}: {}", hex.join(" "));
        o += 16;
    }
}

fn show(name: &str, data: &[u8]) {
    println!("\n===== {name}: {} byte =====", data.len());
    println!("--- primi 400 byte ---");
    hexdump(data, 0, 400);
    println!("--- ultimi 96 byte ---");
    hexdump(data, data.len().saturating_sub(96), 96);
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().map(PathBuf::from).expect("uso: dump_rmb_raw <ARENA2> [NOME.RMB]");
    let blocks = BsaFile::open(find_file_ci(&dir, "BLOCKS.BSA").expect("BLOCKS.BSA"))?;

    if let Some(name) = args.next() {
        let d = blocks.get(&name).expect("blocco non trovato");
        show(&name.to_ascii_uppercase(), d);
        return Ok(());
    }
    if let Some(d) = blocks.get("TVRNAL06.RMB") {
        show("TVRNAL06.RMB", d);
    }
    if let Some(r) = blocks
        .records()
        .iter()
        .filter(|r| r.name().ends_with(".RMB"))
        .min_by_key(|r| r.size())
    {
        show(r.name(), blocks.block(r)?);
    }
    Ok(())
}
