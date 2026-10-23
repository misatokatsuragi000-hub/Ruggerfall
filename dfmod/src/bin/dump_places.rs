//! Strumento di debug per le località del mondo: legge MAPS.BSA (archivio BSA)
//! e stampa regioni, città/villaggi e dungeon con il loro map pixel.
//!
//! Uso: dump_places <MAPS.BSA> [--towns-only] [--near X Y RAGGIO] [--csv file.csv]

use daggerfall_data::{SettlementKind, WorldPlaces};
use std::process::ExitCode;

fn kind_label(k: SettlementKind) -> &'static str {
    match k {
        SettlementKind::City => "città",
        SettlementKind::Town => "town",
        SettlementKind::Village => "villaggio",
        SettlementKind::Hamlet => "hamlet",
        SettlementKind::Manor => "maniero",
        SettlementKind::Farm => "fattoria",
        SettlementKind::Ruins => "rovine",
        SettlementKind::Other => "altro",
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("errore: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().ok_or("manca il percorso di MAPS.BSA")?;
    let mut towns_only = false;
    let mut near: Option<(i32, i32, i32)> = None;
    let mut csv: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--towns-only" => towns_only = true,
            "--near" => {
                let x: i32 = args.next().ok_or("--near vuole X Y RAGGIO")?.parse()?;
                let y: i32 = args.next().ok_or("--near vuole X Y RAGGIO")?.parse()?;
                let r: i32 = args.next().ok_or("--near vuole X Y RAGGIO")?.parse()?;
                near = Some((x, y, r));
            }
            "--csv" => csv = Some(args.next().ok_or("--csv vuole un file")?),
            other => return Err(format!("opzione sconosciuta: {other}").into()),
        }
    }

    let places = WorldPlaces::open(&path)?;

    let regions = places.regions()?;
    println!("regioni ({}):", regions.len());
    for r in &regions {
        println!("  {:>2} {}", r.index, r.name);
    }

    let mut settlements = if towns_only { places.towns()? } else { places.settlements()? };
    let mut dungeons = places.dungeons()?;
    if let Some((x, y, radius)) = near {
        let inside = |r: &daggerfall_data::MapRecord| {
            (r.x as i32 - x).abs() <= radius && (r.y as i32 - y).abs() <= radius
        };
        settlements.retain(inside);
        dungeons.retain(inside);
        println!("\nlocalità entro {radius} map pixel da ({x}, {y}):");
    }

    // Conteggi per tipo.
    let mut counts: Vec<(&'static str, usize)> = Vec::new();
    for k in [
        SettlementKind::City,
        SettlementKind::Town,
        SettlementKind::Village,
        SettlementKind::Hamlet,
        SettlementKind::Manor,
        SettlementKind::Farm,
        SettlementKind::Ruins,
        SettlementKind::Other,
    ] {
        let n = settlements.iter().filter(|r| r.kind() == k).count();
        if n > 0 {
            counts.push((kind_label(k), n));
        }
    }
    println!(
        "\ninsediamenti: {} ({}), dungeon: {}",
        settlements.len(),
        counts
            .iter()
            .map(|(l, n)| format!("{l}: {n}"))
            .collect::<Vec<_>>()
            .join(", "),
        dungeons.len()
    );

    println!("\n{:40} {:8} {:4} {:4}", "NOME", "TIPO", "X", "Y");
    for r in &settlements {
        println!("{:40} {:8} {:4} {:4}", r.name, kind_label(r.kind()), r.x, r.y);
    }
    println!("\ndungeon:");
    for d in &dungeons {
        println!("{:40} {:8} {:4} {:4}", d.name, "dungeon", d.x, d.y);
    }

    if let Some(file) = csv {
        let mut out = String::from("nome,tipo,x,y\n");
        for r in &settlements {
            out.push_str(&format!("{},{},{},{}\n", r.name, kind_label(r.kind()), r.x, r.y));
        }
        for d in &dungeons {
            out.push_str(&format!("{},dungeon,{},{}\n", d.name, d.x, d.y));
        }
        std::fs::write(&file, out)?;
        println!("CSV salvato in {file}");
    }
    Ok(())
}
