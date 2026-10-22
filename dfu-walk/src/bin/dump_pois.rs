//! Verifica la lettura di BuildingData: per ogni tipo di blocco RMB (prefisso del nome)
//! mostra quali BuildingType compaiono, e quanti blocchi contengono quello atteso.
//! Se le percentuali sono alte (taverne in TVRN, armaioli in ARMR...) il layout e'
//! giusto. Uso: cargo run --release --bin dump_pois -- <cartella ARENA2>

use std::collections::BTreeMap;
use std::path::PathBuf;

use daggerfall_data::blocks::RmbBlock;
use daggerfall_data::bsa::BsaFile;
use daggerfall_data::city_map::building_type_name;
use daggerfall_data::find_file_ci;

fn expected(prefix: &str) -> Option<u8> {
    Some(match prefix {
        "TVRN" => 15,
        "WEAP" => 13,
        "ARMR" => 2,
        "ALCH" => 0,
        "BANK" => 3,
        "BOOK" => 5,
        "CLOT" => 6,
        "FURN" => 7,
        "GEMS" => 8,
        "GENR" => 9,
        "LIBR" => 10,
        "PAWN" => 12,
        "TEMP" => 14,
        "MAGE" | "FIGH" | "THIE" => 11,
        _ => return None,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).map(PathBuf::from).expect("uso: dump_pois <ARENA2>");
    let blocks = BsaFile::open(find_file_ci(&dir, "BLOCKS.BSA").expect("BLOCKS.BSA non trovato"))?;

    // prefisso -> (blocchi, blocchi con il tipo atteso, tipi visti)
    let mut stats: BTreeMap<String, (usize, usize, BTreeMap<u8, usize>)> = BTreeMap::new();
    let mut shown_raw = 0;

    for rec in blocks.records() {
        let name = rec.name().to_string();
        if !name.ends_with(".RMB") || name.len() < 4 {
            continue;
        }
        let Ok(data) = blocks.block(rec) else { continue };
        let Ok(rmb) = RmbBlock::parse(data) else { continue };
        let prefix = name[..4].to_string();

        if prefix == "TVRN" && shown_raw < 2 && !rmb.buildings.is_empty() {
            shown_raw += 1;
            println!("{name}: primi 26 byte di BuildingData:");
            println!("  {:02x?}", &data[643..643 + 26]);
            println!("  -> {:?}", rmb.buildings[0]);
        }

        let e = stats.entry(prefix.clone()).or_default();
        e.0 += 1;
        for b in &rmb.buildings {
            *e.2.entry(b.building_type).or_default() += 1;
        }
        if let Some(want) = expected(&prefix) {
            if rmb.buildings.iter().any(|b| b.building_type == want) {
                e.1 += 1;
            }
        }
    }

    println!("\nprefisso  blocchi  con-atteso   tipi visti (tipo=n)");
    for (prefix, (n, ok, types)) in &stats {
        let exp = match expected(prefix) {
            Some(t) => format!("{:3}% ({}={})", *ok * 100 / (*n).max(1), t, building_type_name(t)),
            None => "-".to_string(),
        };
        let seen: Vec<String> = types.iter().map(|(t, c)| format!("{t}={c}")).collect();
        println!("{prefix:8} {n:7}   {exp:22} {}", seen.join(" "));
    }
    Ok(())
}
