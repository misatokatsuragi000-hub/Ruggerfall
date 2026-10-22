//! Verifica degli interni delle taverne di Daggerfall: per ogni sotto-record con
//! BuildingType 15 stampa modelli, flat, porte (grezze) e quello che ne esce
//! (ingombro, punto di partenza, porta d'uscita, triangoli di pavimento e pareti).
//!
//! Uso: dump_interior <cartella ARENA2>

use daggerfall_data::blocks::RmbBlock;
use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::find_file_ci;
use daggerfall_data::interior::build_interior;
use daggerfall_data::maps::MapsReader;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("uso: dump_interior <ARENA2>"));
    let maps_buf = std::fs::read(find_file_ci(&dir, "MAPS.BSA").unwrap())?;
    let blocks_buf = std::fs::read(find_file_ci(&dir, "BLOCKS.BSA").unwrap())?;
    let arch_buf = std::fs::read(find_file_ci(&dir, "ARCH3D.BSA").unwrap())?;
    let maps = BsaArchive::parse(&maps_buf)?;
    let blocks = BsaArchive::parse(&blocks_buf)?;
    let arch = BsaArchive::parse(&arch_buf)?;
    let loc = MapsReader::new(&maps).find_location(17, "DAGGERFALL").unwrap();

    println!("Taverne (BuildingType 15) di Daggerfall:");
    let mut found = 0;
    for by in 0..loc.height {
        for bx in 0..loc.width {
            let name = &loc.block_names[by * loc.width + bx];
            let Some(raw) = blocks.get_by_name(name) else { continue };
            let Ok(rmb) = RmbBlock::parse(raw) else { continue };
            for (i, b) in rmb.buildings.iter().enumerate() {
                if b.building_type != 15 {
                    continue;
                }
                found += 1;
                println!("\n== blocco {name} (bx={bx}, by={by}) sotto-record {i}, qualita' {}", b.quality);
                let Some(it) = rmb.interiors.get(i) else { continue };
                println!("   interno: {} modelli, {} flat, {} porte", it.objects_3d.len(), it.flats.len(), it.doors.len());
                for d in &it.doors {
                    println!("   porta grezza: x={} y={} z={}", d.x, d.y, d.z);
                }
                match build_interior(&rmb, i, &arch) {
                    Some(s) => {
                        println!(
                            "   ingombro: x {:.1}..{:.1}  y {:.1}..{:.1}  z {:.1}..{:.1}",
                            s.bounds_min[0], s.bounds_max[0], s.bounds_min[1], s.bounds_max[1], s.bounds_min[2], s.bounds_max[2]
                        );
                        println!("   partenza (piedi): {:?}  porta d'uscita: {:?}", s.spawn, s.exit);
                        println!("   triangoli: {} pavimento, {} pareti; flat disegnati: {}", s.floor_count(), s.wall_count(), s.flats.len());
                    }
                    None => println!("   nessun modello 3D nell'interno"),
                }
            }
        }
    }
    if found == 0 {
        println!("nessuna taverna trovata");
    }
    Ok(())
}
