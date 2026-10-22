//! Genera gli edifici procedurali di una località e li esporta in OBJ.
//! Uso: cargo run --release --bin dump_buildings -- <ARENA2> [regione=17] [NOME=DAGGERFALL]

use std::path::PathBuf;

use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::building_gen::MaterialKind;
use daggerfall_data::city_buildings::build_city_buildings;
use daggerfall_data::find_file_ci;
use daggerfall_data::maps::MapsReader;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("uso: dump_buildings <ARENA2> [regione] [nome]"));
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

    let loc = MapsReader::new(&maps)
        .find_location(region, &name)
        .ok_or("località non trovata")?;
    println!("{} ({}x{} blocchi)", loc.name, loc.width, loc.height);

    let (city, stats) = build_city_buildings(&loc, &blocks, &arch, &|_, _| 0.0);
    println!(
        "oggetti {}, edifici {}, scartati {}, modelli mancanti {}",
        stats.objects, stats.built, stats.skipped, stats.missing_models
    );
    for (m, part) in MaterialKind::ALL.iter().zip(city.parts().iter()) {
        println!("  {:8} {:7} triangoli", m.name(), part.triangle_count());
    }
    let out = format!("city_{}.obj", loc.name.to_ascii_lowercase().replace(' ', "_"));
    city.export_obj(&out)?;
    println!("scritto {out}");
    Ok(())
}
