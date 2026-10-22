use std::path::PathBuf;
use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::blocks::RmbBlock;
use daggerfall_data::maps::MapsReader;
use daggerfall_data::find_file_ci;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("uso: find_palace <ARENA2>"));
    let maps_buf = std::fs::read(find_file_ci(&dir, "MAPS.BSA").unwrap())?;
    let blocks_buf = std::fs::read(find_file_ci(&dir, "BLOCKS.BSA").unwrap())?;
    let maps = BsaArchive::parse(&maps_buf)?;
    let blocks = BsaArchive::parse(&blocks_buf)?;
    let loc = MapsReader::new(&maps).find_location(17, "DAGGERFALL").unwrap();

    println!("Ricerca del Palazzo Reale (BuildingType == 16) a Daggerfall:");
    let mut found = false;
    for by in 0..loc.height {
        for bx in 0..loc.width {
            let name = &loc.block_names[by * loc.width + bx];
            if let Some(raw) = blocks.get_by_name(name) {
                if let Ok(rmb) = RmbBlock::parse(raw) {
                    for (i, b) in rmb.buildings.iter().enumerate() {
                        if b.building_type == 16 {
                            println!("\n TROVATO PALAZZO REALE in [colonna bx={}, riga by={}] Blocco {}", bx, by, name);
                            println!("   Sotto-record {}: FactionId={}, Quality={}", i, b.faction_id, b.quality);
                            if let Some(s) = rmb.subrecords.get(i) {
                                println!("   Posizione nel blocco: x={}, z={}, rot={:.1}°", s.x, s.z, s.rotation_deg);
                            }
                            for pm in rmb.placed_models().iter().filter(|m| m.subrecord == Some(i as u8)) {
                                println!("   -> Modello 3D castello: ID = {}, pos: ({:.1}, {:.1})", pm.model_id, pm.x, pm.n);
                            }
                            found = true;
                        }
                    }
                }
            }
        }
    }
    if !found {
        println!("Nessun edificio con BuildingType == 16 nei sotto-record.");
    }
    Ok(())
}
