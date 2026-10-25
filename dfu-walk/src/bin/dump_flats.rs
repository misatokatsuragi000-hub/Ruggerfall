//! Stampa TUTTE le voci di FLATS.CFG in una tabella ordinata per flatID,
//! così si può confrontare il numero che appare nel nome dei file della mod
//! (es. TFAC00I0.RCI_411-0.png) con il flatID calcolato come in DFU:
//!   flatID = (archive << 7) | record
//!
//! Uso: cargo run --release --bin dump_flats -- <ARENA2> [filtro_face|filtro_archive]
//! Esempi:
//!   dump_flats /path/arena2              -> tabella completa
//!   dump_flats /path/arena2 411          -> solo le voci con faceIndex=411
//!   dump_flats /path/arena2 381          -> solo archive=381
use std::path::PathBuf;
use daggerfall_data::find_file_ci;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mnt/1CE16DBE48849731/Daggerfall/arena2"));
    let filter: Option<u32> = args.next().and_then(|s| s.parse().ok());

    let path = find_file_ci(&dir, "FLATS.CFG").ok_or("FLATS.CFG non trovato")?;
    let bytes = std::fs::read(&path)?;
    println!("File: {}", path.display());
    println!("Dimensione: {} byte", bytes.len());

    // DFU usa Encoding.UTF8.GetString (mai errori), replichiamo con from_utf8_lossy.
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);

    // Estrai tutte le voci in blocchi da 6 righe (identico a FlatsFile.cs).
    #[derive(Clone)]
    struct V {
        archive: u16,
        record: u8,
        flat_id: u32,
        face: i32,
        caption: String,
        gender: String,
        unk2: String,
        unk3: String,
    }
    let mut entries: Vec<V> = Vec::new();
    let mut rejected = 0usize;
    let mut block: Vec<&str> = Vec::new();

    for line in text.lines().map(str::trim).chain(std::iter::once("")) {
        if line.is_empty() {
            if !block.is_empty() {
                if block.len() >= 6 {
                    let mut parts = block[0].split_whitespace();
                    let a: Option<u16> = parts.next().and_then(|s| s.parse().ok());
                    let r: Option<u8> = parts.next().and_then(|s| s.parse().ok());
                    let f: Option<i32> = block[5].parse().ok();
                    if let (Some(a), Some(r), Some(f)) = (a, r, f) {
                        let flat_id = ((a as u32) << 7) | (r as u32 & 0x7F);
                        entries.push(V {
                            archive: a,
                            record: r,
                            flat_id,
                            face: f,
                            caption: block.get(1).copied().unwrap_or("").to_string(),
                            gender: block.get(2).copied().unwrap_or("").to_string(),
                            unk2: block.get(3).copied().unwrap_or("").to_string(),
                            unk3: block.get(4).copied().unwrap_or("").to_string(),
                        });
                    } else {
                        rejected += 1;
                    }
                } else {
                    rejected += 1;
                }
                block.clear();
            }
            continue;
        }
        block.push(line);
        if block.len() > 6 {
            block.clear();
            rejected += 1;
        }
    }

    if entries.is_empty() {
        println!("NESSUNA VOCE VALIDA (lette {} righe, {} scartati)", text.lines().count(), rejected);
        return Ok(());
    }

    // Ordinamento per flatID (equivalente a (archive, record)).
    entries.sort_by_key(|v| v.flat_id);

    // Filtro opzionale: se il valore corrisponde a un faceIndex o a un archive, filtra.
    let mut shown = 0usize;
    let mut skipped_by_filter = 0usize;
    if let Some(f) = filter {
        // Prima controlla se è un face valido
        let n_by_face = entries.iter().filter(|v| v.face == f as i32).count();
        let n_by_archive = entries.iter().filter(|v| v.archive == f as u16).count();
        let n_by_flat = entries.iter().filter(|v| v.flat_id == f).count();
        println!(
            "Filtro {f}: {} voci con faceIndex={f}, {} con archive={f}, {} con flatID={f}",
            n_by_face, n_by_archive, n_by_flat
        );
        println!();
    }

    println!(
        "{:>5} {:>6} {:>7} {:>5} {:>4} {:>4}  {:22}  {:>5}",
        "arch", "record", "flatID", "face", "gen", "unk", "caption", "match?"
    );
    println!("{}", "-".repeat(90));

    for v in &entries {
        if let Some(f) = filter {
            if v.face != f as i32 && v.archive != f as u16 && v.flat_id != f {
                skipped_by_filter += 1;
                continue;
            }
        }
        // Colonnina "match?": se il faceIndex è uguale al flatID (utile per
        // capire se la mod numera i ritratti col flatID).
        let match_mark = if v.face >= 0 && v.face as u32 == v.flat_id {
            " =flat"
        } else {
            ""
        };
        println!(
            "{:>5} {:>6} {:>7} {:>5} {:>4} {:>4}  {:22}  {}",
            v.archive, v.record, v.flat_id, v.face, v.gender, v.unk2, v.caption, match_mark
        );
        shown += 1;
    }

    println!();
    println!(
        "Totale: {} voci valide, {} scartate, {} mostrate ({} filtrate)",
        entries.len(),
        rejected,
        shown,
        skipped_by_filter
    );

    // Riepilogo per archivio (utile per capire quali archivi di "persone" sono coperti).
    let mut by_archive: std::collections::BTreeMap<u16, usize> = Default::default();
    for v in &entries {
        *by_archive.entry(v.archive).or_default() += 1;
    }
    println!("\nArchivi presenti in FLATS.CFG (archive: n.record):");
    let mut line = String::new();
    for (a, n) in &by_archive {
        line.push_str(&format!("{a}:{n}  "));
        if line.len() > 100 {
            println!("  {line}");
            line.clear();
        }
    }
    if !line.is_empty() {
        println!("  {line}");
    }

    // Statistica: quanti faceIndex sono "vicini al flatID"?
    let same_count = entries
        .iter()
        .filter(|v| v.face >= 0 && v.face as u32 == v.flat_id)
        .count();
    println!(
        "\nVoci in cui faceIndex == flatID: {} (su {})",
        same_count,
        entries.len()
    );

    Ok(())
}