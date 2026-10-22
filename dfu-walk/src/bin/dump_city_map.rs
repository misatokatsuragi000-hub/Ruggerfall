//! Scrive la mappa locale di una città in PNG (senza avviare il viewer).
//! Uso: cargo run --release --bin dump_city_map -- <ARENA2> [regione=17] [NOME=DAGGERFALL]

use std::path::PathBuf;

use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::city_map::{collect_city_map, image_size, render_city_map, PoiKind};
use daggerfall_data::find_file_ci;
use daggerfall_data::maps::MapsReader;
use daggerfall_data::png::encode_rgba;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("uso: dump_city_map <ARENA2> [regione] [nome]"));
    let region: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(17);
    let name = args.next().unwrap_or_else(|| "DAGGERFALL".to_string());

    let read = |n: &str| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        Ok(std::fs::read(find_file_ci(&dir, n).ok_or_else(|| format!("{n} non trovato"))?)?)
    };
    let maps_buf = read("MAPS.BSA")?;
    let blocks_buf = read("BLOCKS.BSA")?;
    let arch_buf = read("ARCH3D.BSA")?;
    let maps = BsaArchive::parse(&maps_buf)?;
    let blocks = BsaArchive::parse(&blocks_buf)?;
    let arch = BsaArchive::parse(&arch_buf)?;

    let loc = MapsReader::new(&maps).find_location(region, &name).ok_or("località non trovata")?;
    let map = collect_city_map(&loc, &blocks, &arch);
    println!("{}: {} edifici sulla mappa", map.name, map.buildings.len());
    for k in PoiKind::ALL {
        println!("  {:8} {}", k.label(), map.counts[k.index()]);
    }

    let (w, h) = image_size();
    let out = format!("city_map_{}.png", loc.name.to_ascii_lowercase().replace(' ', "_"));
    std::fs::write(&out, encode_rgba(w as u32, h as u32, &render_city_map(&map)))?;
    println!("scritto {out}");
    Ok(())
}
