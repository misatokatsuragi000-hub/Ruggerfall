//! Perché tanti oggetti RMB risultano "modello mancante"?
//! Distingue: id assente da ARCH3D / record troppo corto o troncato / zero vertici,
//! e per gli id assenti prova mappature alternative dell'id grezzo.
//! Uso: cargo run --release --bin dump_missing -- <cartella ARENA2>

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use daggerfall_data::arch3d::Arch3dModel;
use daggerfall_data::blocks::RmbBlock;
use daggerfall_data::bsa::{BsaArchive, BsaDirectory, BsaFile};
use daggerfall_data::find_file_ci;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).map(PathBuf::from).expect("uso: dump_missing <ARENA2>");
    let blocks = BsaFile::open(find_file_ci(&dir, "BLOCKS.BSA").expect("BLOCKS.BSA"))?;
    let arch_buf = std::fs::read(find_file_ci(&dir, "ARCH3D.BSA").expect("ARCH3D.BSA"))?;
    let arch = BsaArchive::parse(&arch_buf)?;

    let BsaDirectory::Numeric(ids) = &arch.directory else {
        println!("ARCH3D.BSA ha directory con NOMI, non numerica: get_by_id non può funzionare!");
        return Ok(());
    };
    let (mn, mx) = (ids.keys().min().unwrap(), ids.keys().max().unwrap());
    println!("ARCH3D: {} record, id da {mn} a {mx}", ids.len());

    // (raw_id, model_id) -> usi
    let mut uses: HashMap<(u32, u32), usize> = HashMap::new();
    for rec in blocks.records() {
        if !rec.name().ends_with(".RMB") { continue; }
        let Ok(data) = blocks.block(rec) else { continue };
        let Ok(b) = RmbBlock::parse(data) else { continue };
        for o in &b.objects_3d {
            *uses.entry((o.raw_id, o.model_id)).or_default() += 1;
        }
    }

    let mut by_status: BTreeMap<&str, usize> = BTreeMap::new();
    let mut missing: Vec<(usize, u32, u32)> = Vec::new();
    let mut alt_hits: BTreeMap<&str, usize> = BTreeMap::new();
    for (&(raw, id), &n) in &uses {
        let status = match arch.get_by_id(id) {
            None => "id assente in ARCH3D",
            Some(d) if d.len() < 32 => "record < 32 byte",
            Some(d) => match Arch3dModel::parse(id, d) {
                Err(_) => "parse fallito (header/vertici)",
                Ok(m) if m.vertices.is_empty() => "zero vertici",
                Ok(_) => "ok",
            },
        };
        *by_status.entry(status).or_default() += n;
        if status == "id assente in ARCH3D" {
            missing.push((n, raw, id));
            let (id1, id2) = (raw & 0xFFFF, (raw >> 16) & 0xFF);
            for (name, cand) in [
                ("raw intero", raw),
                ("solo id1", id1),
                ("id1*100", id1 * 100),
                ("id1 + id2", id1 + id2),
                ("id1*100 + (raw>>24)", id1 * 100 + (raw >> 24)),
                ("raw & 0xFFFFFF", raw & 0xFF_FFFF),
            ] {
                if ids.contains_key(&cand) { *alt_hits.entry(name).or_default() += n; }
            }
        }
    }
    println!("\nEsito per uso:");
    for (s, n) in &by_status { println!("  {n:7}  {s}"); }
    println!("\nPer gli id assenti, quanti usi TROVEREBBERO un record con una mappatura alternativa:");
    for (s, n) in &alt_hits { println!("  {n:7}  {s}"); }

    missing.sort_by(|a, b| b.0.cmp(&a.0));
    println!("\nPiù usati tra gli assenti (usi, raw esadecimale, id1, id2, byte3, id usato):");
    for (n, raw, id) in missing.iter().take(30) {
        println!("{n:6}  0x{raw:08X}  id1={:5} id2={:3} b3={:3}  -> {id}", raw & 0xFFFF, (raw >> 16) & 0xFF, raw >> 24);
    }
    Ok(())
}
