//! Dump diagnostico per una località: per ogni blocco della griglia stampa
//! quanti oggetti 3D contiene, quanti modelli sono trovati in ARCH3D, quanti
//! vengono parsati con successo, e (in fondo) elenca i prefissi noti
//! (MAGE/FIGH/THIE/DARK/GUIL/TEMP/PALA/CAST/GRVE) presenti in BLOCKS.BSA,
//! con il dettaglio del blocco che dovrebbe essere la gilda dei maghi:
//! sotto-record (posizioni), posizioni finali (via `placed_models`), building
//! data con marcatura dei `building_type == 11`.
//!
//! Applica lo stesso fixup dei nomi che fa `main.rs` (SearchAlternateRMBName
//! di DFU: prefix 4 + suffix 6, ultimo match), così mostra i nomi
//! effettivamente usati dal gioco.
//!
//! Uso: dump_guild_block <cartella ARENA2> [regione=17] [nome=DAGGERFALL]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use daggerfall_data::arch3d::Arch3dModel;
use daggerfall_data::blocks::RmbBlock;
use daggerfall_data::bsa::{BsaArchive, BsaDirectory};
use daggerfall_data::find_file_ci;
use daggerfall_data::maps::{DFLocation, MapsReader};

// =============================================================================
// COSTANTI (copia dei valori in `blocks.rs`, che sono privati)
// =============================================================================

/// Lato di un blocco RMB in unità DF (RMBDimension in DFU).
const BLOCK_UNITS: f32 = 4096.0;

// =============================================================================
// FIXUP DEI NOMI (copia di quella in `src/main.rs`)
// =============================================================================

fn fix_missing_block_names(
    locations: &mut HashMap<(i32, i32), DFLocation>,
                           blocks_bsa: &BsaArchive,
) -> (usize, usize) {
    let all_names: HashSet<&str> = match &blocks_bsa.directory {
        BsaDirectory::Named(map) => map.keys().map(String::as_str).collect(),
        _ => return (0, 0),
    };
    let mut sorted: Vec<&str> = all_names.iter().copied().collect();
    sorted.sort();

    let mut fixed = 0usize;
    let mut unfixed = 0usize;
    for loc in locations.values_mut() {
        for name in loc.block_names.iter_mut() {
            if all_names.contains(name.as_str()) {
                continue;
            }
            // DFU: prefix primi 4 char, suffix ultimi 6 char (include ".RMB").
            if name.len() < 6 {
                unfixed += 1;
                continue;
            }
            let prefix = &name[..4];
            let suffix = &name[name.len() - 6..];

            let mut found = String::new();
            for real in &sorted {
                if real.starts_with(prefix) && real.ends_with(suffix) {
                    found = real.to_string(); // sovrascrive: L'ULTIMO match (come DFU)
                }
            }

            if !found.is_empty() {
                eprintln!("fixup [{}]: {} -> {}", loc.name, name, found);
                *name = found;
                fixed += 1;
            } else {
                eprintln!("fixup [{}]: {} NON correggibile", loc.name, name);
                unfixed += 1;
            }
        }
    }
    (fixed, unfixed)
}

// =============================================================================
// MAIN
// =============================================================================

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(
        args.next()
        .expect("uso: dump_guild_block <ARENA2> [regione=17] [nome=DAGGERFALL]"),
    );
    let region: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(17);
    let name = args.next().unwrap_or_else(|| "DAGGERFALL".to_string());

    let read = |n: &str| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        Ok(std::fs::read(
            find_file_ci(&dir, n).ok_or_else(|| format!("{n} non trovato"))?,
        )?)
    };
    let maps_buf = read("MAPS.BSA")?;
    let blocks_buf = read("BLOCKS.BSA")?;
    let arch_buf = read("ARCH3D.BSA")?;
    let maps = BsaArchive::parse(&maps_buf)?;
    let blocks = BsaArchive::parse(&blocks_buf)?;
    let arch = BsaArchive::parse(&arch_buf)?;

    // -------------------------------------------------------------------------
    // 1. Prefissi noti presenti in BLOCKS.BSA
    // -------------------------------------------------------------------------
    println!("=== Prefissi in BLOCKS.BSA (MAGE/FIGH/THIE/DARK/GUIL/TEMP/PALA/CAST/GRVE) ===");
    let all_names: Vec<&str> = match &blocks.directory {
        BsaDirectory::Named(map) => {
            let mut v: Vec<&str> = map.keys().map(String::as_str).collect();
            v.sort();
            v
        }
        _ => Vec::new(),
    };
    let mut found_prefixes: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for n in &all_names {
        if !n.ends_with(".RMB") {
            continue;
        }
        for prefix in ["MAGE", "FIGH", "THIE", "DARK", "GUIL", "TEMP", "PALA", "CAST", "GRVE"] {
            if n.starts_with(prefix) {
                found_prefixes.entry(prefix).or_default().push(n);
                break;
            }
        }
    }
    if found_prefixes.is_empty() {
        println!("  NESSUNO. Il fixup non ha nulla su cui lavorare.");
    } else {
        for (prefix, names) in &found_prefixes {
            let head: Vec<&str> = names.iter().take(8).copied().collect();
            println!("  {}: {} varianti {:?}", prefix, names.len(), head);
        }
    }

    // -------------------------------------------------------------------------
    // 2. Costruisci la mappa delle località e applica il fixup (come main.rs)
    // -------------------------------------------------------------------------
    let maps_reader = MapsReader::new(&maps);
    let mut locations: HashMap<(i32, i32), DFLocation> = HashMap::new();
    for r in 0..62 {
        let names_file = format!("MAPNAMES.{:03}", r);
        if let Some(names_raw) = maps.get_by_name(&names_file) {
            if names_raw.len() < 4 {
                continue;
            }
            let count =
            u32::from_le_bytes(names_raw[0..4].try_into().unwrap_or_default()) as usize;
            for i in 0..count {
                if let Some(l) = maps_reader.get_location(r, i) {
                    let mx = (l.world_x / 32768) as i32;
                    let my = (499) - ((l.world_y / 32768) as i32);
                    locations.insert((mx, my), l);
                }
            }
        }
    }
    let (fixed, unfixed) = fix_missing_block_names(&mut locations, &blocks);
    if fixed + unfixed > 0 {
        eprintln!(
            "\n=== Fixup applicato: {} corretti, {} non correggibili ===\n",
            fixed, unfixed
        );
    }

    // -------------------------------------------------------------------------
    // 3. Trova la località richiesta nella mappa fixata
    // -------------------------------------------------------------------------
    let base_loc = maps_reader
    .find_location(region, &name)
    .ok_or("località non trovata")?;
    let mx = (base_loc.world_x / 32768) as i32;
    let my = (499) - ((base_loc.world_y / 32768) as i32);
    let loc = locations
    .get(&(mx, my))
    .cloned()
    .ok_or("località non trovata dopo il fixup")?;

    // -------------------------------------------------------------------------
    // 4. Tabella dei blocchi della località
    // -------------------------------------------------------------------------
    println!("=== {} ({}x{}) ===", loc.name, loc.width, loc.height);
    println!(
        "{:<16} {:>4} {:>6} {:>6} {:>6}  note",
        "blocco", "obj", "found", "parsed", "failed"
    );

    let mut total_objects = 0;
    let mut total_found = 0;
    let mut total_parsed = 0;
    let mut total_failed = 0;
    let mut total_blocks = 0;
    let mut total_blocks_empty = 0;
    let mut total_blocks_missing = 0;
    let mut total_blocks_bad = 0;

    for by in 0..loc.height {
        for bx in 0..loc.width {
            total_blocks += 1;
            let block_name = &loc.block_names[by * loc.width + bx];
            let Some(raw) = blocks.get_by_name(block_name) else {
                println!(
                    "[{:2},{:2}] {:<14} BLOCCO NON IN BLOCKS.BSA",
                    bx, by, block_name
                );
                total_blocks_missing += 1;
                continue;
            };
            let rmb = match RmbBlock::parse(raw) {
                Ok(r) => r,
                Err(e) => {
                    println!("[{:2},{:2}] {:<14} PARSE FALLITO: {}", bx, by, block_name, e);
                    total_blocks_bad += 1;
                    continue;
                }
            };

            if rmb.objects_3d.is_empty() {
                println!("[{:2},{:2}] {:<14} VUOTO (0 oggetti 3D)", bx, by, block_name);
                total_blocks_empty += 1;
                continue;
            }

            let mut found = 0;
            let mut parsed = 0;
            let mut failed = 0;
            let mut first_fail: Option<u32> = None;
            let mut first_missing: Option<u32> = None;

            for obj in &rmb.objects_3d {
                total_objects += 1;
                let Some(model_raw) = arch.get_by_id(obj.model_id) else {
                    if first_missing.is_none() {
                        first_missing = Some(obj.model_id);
                    }
                    continue;
                };
                found += 1;
                match Arch3dModel::parse(obj.model_id, model_raw) {
                    Ok(_) => parsed += 1,
                    Err(_) => {
                        failed += 1;
                        if first_fail.is_none() {
                            first_fail = Some(obj.model_id);
                        }
                    }
                }
            }
            total_found += found;
            total_parsed += parsed;
            total_failed += failed;

            let mut notes = String::new();
            if failed > 0 {
                notes.push_str(&format!(" PARSE FAIL id={}", first_fail.unwrap()));
            }
            if found == 0 {
                notes.push_str(&format!(
                    " NESSUN MODELLO IN ARCH3D (primo: {})",
                                        first_missing.unwrap_or(0)
                ));
            }

            let flag = if !notes.is_empty() { "  ←" } else { "" };
            println!(
                "[{:2},{:2}] {:<14} {:>4} {:>6} {:>6} {:>6}{}{}",
                bx,
                by,
                block_name,
                rmb.objects_3d.len(),
                     found,
                     parsed,
                     failed,
                     flag,
                     notes
            );
        }
    }

    println!(
        "\nTotale: {} blocchi ({} vuoti, {} mancanti in BLOCKS.BSA, {} con parse fallito)",
             total_blocks, total_blocks_empty, total_blocks_missing, total_blocks_bad
    );
    println!(
        "Oggetti: {} totali, {} trovati in ARCH3D, {} parsati, {} falliti",
        total_objects, total_found, total_parsed, total_failed
    );

    // -------------------------------------------------------------------------
    // 5. Dettaglio dei blocchi chiave (gilda e cimitero)
    // -------------------------------------------------------------------------
    // Per Daggerfall: [3,1] = MAGE (gilda dei maghi, o area), [1,2] = CUST
    // (blocco custom, probabile gilda), [2,2] = GRVE (cimitero).
    // Per altre località: primo MAGE trovato + primo CUST + primo GRVE.
    let mut targets: Vec<(usize, usize, &'static str)> = Vec::new();
    if loc.name.eq_ignore_ascii_case("DAGGERFALL") {
        targets.push((3, 1, "MAGE (atteso: gilda dei maghi)"));
        targets.push((1, 2, "CUST (blocco custom, possibile gilda)"));
        targets.push((2, 2, "GRVE (cimitero)"));
    } else {
        let mut mage: Option<(usize, usize)> = None;
        let mut cust: Option<(usize, usize)> = None;
        let mut grve: Option<(usize, usize)> = None;
        'outer: for by in 0..loc.height {
            for bx in 0..loc.width {
                let n = &loc.block_names[by * loc.width + bx];
                if mage.is_none() && n.starts_with("MAGE") {
                    mage = Some((bx, by));
                }
                if cust.is_none() && n.starts_with("CUST") {
                    cust = Some((bx, by));
                }
                if grve.is_none() && n.starts_with("GRVE") {
                    grve = Some((bx, by));
                }
                if mage.is_some() && cust.is_some() && grve.is_some() {
                    break 'outer;
                }
            }
        }
        if let Some((bx, by)) = mage {
            targets.push((bx, by, "MAGE"));
        }
        if let Some((bx, by)) = cust {
            targets.push((bx, by, "CUST"));
        }
        if let Some((bx, by)) = grve {
            targets.push((bx, by, "GRVE"));
        }
    }

    for (bx, by, label) in targets {
        let target_name = &loc.block_names[by * loc.width + bx];
        println!(
            "\n=== Dettaglio blocco [{},{}] ({} — {}): ===",
                 bx, by, target_name, label
        );
        let Some(raw) = blocks.get_by_name(target_name) else {
            println!("  Blocco {} NON in BLOCKS.BSA (fixup non riuscito)", target_name);
            continue;
        };
        let rmb = match RmbBlock::parse(raw) {
            Ok(r) => r,
            Err(e) => {
                println!("  PARSE FALLITO: {}", e);
                continue;
            }
        };

        println!(
            "  Sotto-record: {}, oggetti 3D: {}, flat: {}, buildings: {}",
            rmb.subrecords.len(),
                 rmb.objects_3d.len(),
                 rmb.flats.len(),
                 rmb.buildings.len()
        );

        // Sotto-record: posizioni nel blocco (x, z grezzi + n calcolato)
        println!("\n  Sotto-record (posizione nel blocco):");
        for (i, s) in rmb.subrecords.iter().enumerate() {
            let n = if daggerfall_data::blocks::Z_FROM_NORTH_EDGE {
                BLOCK_UNITS - s.z as f32
            } else {
                s.z as f32
            };
            println!(
                "  [{:2}] x={:5} z={:5} -> (x={:5.0}, n={:5.0}) rot={:6.1}",
                     i, s.x, s.z, s.x, n, s.rotation_deg
            );
        }

        // Posizioni finali calcolate da placed_models()
        let placed = rmb.placed_models();
        println!("\n  Posizioni finali (via placed_models, primi 40):");
        for (i, pm) in placed.iter().enumerate().take(40) {
            println!(
                "  [{:2}] model={:6} block={:?} -> world_x={:8.1} world_n={:8.1} rot={:6.1}",
                i, pm.model_id, pm.subrecord, pm.x, pm.n, pm.rotation_deg
            );
        }
        if placed.len() > 40 {
            println!("  ... e altri {} oggetti", placed.len() - 40);
        }

        // Building data, con marcatura delle gilde (type 11)
        println!("\n  Building data:");
        for (i, b) in rmb.buildings.iter().enumerate() {
            let is_guild = if b.building_type == 11 { " ← GILDA" } else { "" };
            let is_temple = if b.building_type == 14 { " ← TEMPIO" } else { "" };
            let is_tavern = if b.building_type == 15 { " ← TAVERNA" } else { "" };
            let is_palace = if b.building_type == 16 { " ← PALAZZO" } else { "" };
            println!(
                "  [{:2}] type={:3} quality={:3} faction_id={:5} sector={:3} location_id={:5}{}{}{}{}",
                i, b.building_type, b.quality, b.faction_id, b.sector, b.location_id,
                is_guild, is_temple, is_tavern, is_palace
            );
        }
    }

    Ok(())
}
