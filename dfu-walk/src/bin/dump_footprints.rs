//! Statistiche sugli ingombri degli oggetti 3D di tutti i blocchi RMB:
//! serve a tarare le soglie di `building_gen` (MIN_SIDE, MAX_SIDE, ...).
//! Uso: cargo run --release --bin dump_footprints -- <cartella ARENA2>

use std::collections::HashMap;
use std::path::PathBuf;

use daggerfall_data::arch3d::Arch3dModel;
use daggerfall_data::blocks::RmbBlock;
use daggerfall_data::bsa::{BsaArchive, BsaFile};
use daggerfall_data::building_gen::Footprint;
use daggerfall_data::find_file_ci;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).map(PathBuf::from).expect("uso: dump_footprints <ARENA2>");
    let blocks_path = find_file_ci(&dir, "BLOCKS.BSA").expect("BLOCKS.BSA non trovato");
    let arch_path = find_file_ci(&dir, "ARCH3D.BSA").expect("ARCH3D.BSA non trovato");

    let blocks = BsaFile::open(blocks_path)?;
    let arch_buf = std::fs::read(arch_path)?;
    let arch = BsaArchive::parse(&arch_buf)?;

    let mut uses: HashMap<u32, usize> = HashMap::new();
    let mut rmb_total = 0usize;
    let mut rmb_failed = 0usize;
    for rec in blocks.records() {
        if !rec.name().ends_with(".RMB") {
            continue;
        }
        rmb_total += 1;
        let Ok(data) = blocks.block(rec) else { rmb_failed += 1; continue };
        match RmbBlock::parse(data) {
            Ok(b) => {
                for o in &b.objects_3d {
                    *uses.entry(o.model_id).or_default() += 1;
                }
            }
            Err(_) => rmb_failed += 1,
        }
    }
    println!("blocchi RMB: {rmb_total} (non letti: {rmb_failed}), modelli distinti usati: {}", uses.len());

    let mut rows = Vec::new();
    let mut reasons: HashMap<&'static str, usize> = HashMap::new();
    let (mut kept, mut missing, mut off_cx, mut off_cz, mut n_off) = (0usize, 0usize, 0.0f32, 0.0f32, 0usize);
    for (&id, &count) in &uses {
        let fp = arch
            .get_by_id(id)
            .and_then(|raw| Arch3dModel::parse(id, raw).ok())
            .and_then(|m| Footprint::from_model(&m));
        let Some(fp) = fp else { missing += count; continue };
        off_cx += fp.cx.abs();
        off_cz += fp.cz.abs();
        n_off += 1;
        let verdict = fp.evaluate();
        match verdict {
            Ok(_) => kept += count,
            Err(r) => *reasons.entry(r).or_default() += count,
        }
        rows.push((count, id, fp, verdict));
    }
    println!("oggetti: tenuti come edificio {kept}, modello mancante {missing}");
    for (r, n) in &reasons {
        println!("  scartati ({r}): {n}");
    }
    if n_off > 0 {
        println!(
            "scostamento medio del centro dall'origine del modello: |cx|={:.2} m, |cz|={:.2} m",
            off_cx / n_off as f32,
            off_cz / n_off as f32
        );
    }

    rows.sort_by(|a, b| b.0.cmp(&a.0));
    println!("\nPiù usati (usi, id, larghezza x profondità x altezza, esito):");
    for (count, id, fp, v) in rows.iter().take(60) {
        println!("{count:6} {id:6}  {:5.1} x {:5.1} x {:5.1}  {:?}", fp.width, fp.depth, fp.height, v);
    }
    Ok(())
}
