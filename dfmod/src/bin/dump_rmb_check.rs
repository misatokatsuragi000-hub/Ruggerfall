//! Verifica del nuovo parser RMB su tutti i 920 blocchi.
//! Uso: cargo run --release --bin dump_rmb_check -- <ARENA2> [NOME.RMB]

use std::collections::BTreeMap;
use std::path::PathBuf;

use daggerfall_data::arch3d::Arch3dModel;
use daggerfall_data::blocks::RmbBlock;
use daggerfall_data::bsa::{BsaArchive, BsaFile};
use daggerfall_data::building_gen::Footprint;
use daggerfall_data::find_file_ci;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().map(PathBuf::from).expect("uso: dump_rmb_check <ARENA2> [BLOCCO.RMB]");
    let focus = args.next().unwrap_or_else(|| "TVRNAL06.RMB".to_string()).to_ascii_uppercase();
    let blocks = BsaFile::open(find_file_ci(&dir, "BLOCKS.BSA").expect("BLOCKS.BSA"))?;
    let arch_buf = std::fs::read(find_file_ci(&dir, "ARCH3D.BSA").expect("ARCH3D.BSA"))?;
    let arch = BsaArchive::parse(&arch_buf)?;

    let (mut ok, mut err, mut exact) = (0usize, 0usize, 0usize);
    let mut diffs: BTreeMap<i64, usize> = BTreeMap::new();
    let mut errs: BTreeMap<&str, usize> = BTreeMap::new();
    let (mut in_block, mut misc) = (0usize, 0usize);
    let (mut miss_block, mut miss_misc) = (0usize, 0usize);
    let mut missing_ids: BTreeMap<u32, usize> = BTreeMap::new();
    let mut p1_nonzero = 0usize;
    let mut rot_hist: BTreeMap<i32, usize> = BTreeMap::new();
    // minimi/massimi di x,z per origine
    let mut rng_block = [i32::MAX, i32::MIN, i32::MAX, i32::MIN];
    let mut rng_misc = rng_block;
    let mut rng_flat = rng_block;

    for rec in blocks.records() {
        if !rec.name().ends_with(".RMB") { continue; }
        let Ok(data) = blocks.block(rec) else { continue };
        let b = match RmbBlock::parse(data) {
            Ok(b) => b,
            Err(e) => { err += 1; *errs.entry(e).or_default() += 1; continue }
        };
        ok += 1;
        let d = data.len() as i64 - b.bytes_used as i64;
        if d == 0 { exact += 1; }
        *diffs.entry(d).or_default() += 1;
        for s in &b.subrecords { *rot_hist.entry(s.rotation_deg.round() as i32).or_default() += 1; }
        for o in &b.objects_3d {
            let missing = arch.get_by_id(o.model_id).is_none();
            if missing { *missing_ids.entry(o.model_id).or_default() += 1; }
            if o.pivot != [0, 0, 0] { p1_nonzero += 1; }
            let r = if o.block.is_some() { in_block += 1; if missing { miss_block += 1; } &mut rng_block }
                    else { misc += 1; if missing { miss_misc += 1; } &mut rng_misc };
            r[0] = r[0].min(o.x); r[1] = r[1].max(o.x); r[2] = r[2].min(o.z); r[3] = r[3].max(o.z);
        }
        for f in &b.flats {
            let r = &mut rng_flat;
            r[0] = r[0].min(f.x); r[1] = r[1].max(f.x); r[2] = r[2].min(f.z); r[3] = r[3].max(f.z);
        }
    }

    println!("blocchi letti: {ok}, errori: {err}, consumo esatto (bytes_used == lunghezza): {exact}");
    for (e, n) in &errs { println!("  errore \"{e}\": {n}");}
    println!("differenza (lunghezza - consumati) -> numero di blocchi:");
    for (d, n) in diffs.iter().take(8) { println!("  {d:6} -> {n}"); }
    println!("\noggetti 3D: dentro i Block {in_block} (mancanti in ARCH3D: {miss_block}), liste misc {misc} (mancanti: {miss_misc})");
    println!("oggetti con Point1 != 0: {p1_nonzero}");
    println!("x,z oggetti nei Block : x [{}, {}]  z [{}, {}]", rng_block[0], rng_block[1], rng_block[2], rng_block[3]);
    println!("x,z oggetti misc      : x [{}, {}]  z [{}, {}]", rng_misc[0], rng_misc[1], rng_misc[2], rng_misc[3]);
    println!("x,z flat              : x [{}, {}]  z [{}, {}]", rng_flat[0], rng_flat[1], rng_flat[2], rng_flat[3]);
    println!("rotazioni dei sotto-record (gradi -> numero): {rot_hist:?}");
    let mut miss: Vec<_> = missing_ids.into_iter().collect();
    miss.sort_by(|a, b| b.1.cmp(&a.1));
    println!("id ancora mancanti (primi 15): {:?}", &miss[..miss.len().min(15)]);

    // dettaglio di un blocco
    let Some(data) = blocks.get(&focus) else { println!("\n{focus} non trovato"); return Ok(()) };
    let b = RmbBlock::parse(data)?;
    println!("\n===== {focus}: {} sotto-record =====", b.subrecords.len());
    for (i, s) in b.subrecords.iter().enumerate() {
        println!("sotto-record {i:2}: X={:5} Z={:5} rot={:6.1}", s.x, s.z, s.rotation_deg);
        for o in b.objects_3d.iter().filter(|o| o.block == Some(i as u8)) {
            let fp = arch.get_by_id(o.model_id).and_then(|r| Arch3dModel::parse(o.model_id, r).ok())
                .and_then(|m| Footprint::from_model(&m));
            let fps = fp.map(|f| format!("ingombro {:.1}x{:.1}x{:.1} m centro ({:.1},{:.1})", f.width, f.depth, f.height, f.cx, f.cz))
                .unwrap_or_else(|| "modello assente".into());
            println!("    id {:6} tipo {:3} p2=({:5},{:5},{:5}) rot={:6.1}  {fps}", o.model_id, o.object_type, o.x, o.y, o.z, o.rotation_y_deg);
        }
    }
    println!("--- liste misc ---");
    for o in b.objects_3d.iter().filter(|o| o.block.is_none()).take(25) {
        println!("    id {:6} tipo {:3} p2=({:5},{:5},{:5}) rot={:6.1}", o.model_id, o.object_type, o.x, o.y, o.z, o.rotation_y_deg);
    }
    Ok(())
}
