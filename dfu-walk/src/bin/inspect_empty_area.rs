use std::path::PathBuf;
use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::blocks::RmbBlock;
use daggerfall_data::maps::MapsReader;
use daggerfall_data::find_file_ci;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("uso: inspect_empty_area <ARENA2>"));
    let maps_buf = std::fs::read(find_file_ci(&dir, "MAPS.BSA").unwrap())?;
    let blocks_buf = std::fs::read(find_file_ci(&dir, "BLOCKS.BSA").unwrap())?;
    let maps = BsaArchive::parse(&maps_buf)?;
    let blocks = BsaArchive::parse(&blocks_buf)?;
    let loc = MapsReader::new(&maps).find_location(17, "DAGGERFALL").unwrap();

    // Ispeziona CUSTAA08 e GRVEAL27 senza NESSUN filtro
    for target in ["CUSTAA08.RMB", "GRVEAL27.RMB"] {
        println!("\n=== TUTTI GLI OGGETTI IN {} ===", target);
        if let Some(raw) = blocks.get_by_name(target) {
            if let Ok(rmb) = RmbBlock::parse(raw) {
                println!("Subrecords: {}, Objects_3D: {}", rmb.subrecords.len(), rmb.objects_3d.len());
                for (i, o) in rmb.objects_3d.iter().enumerate() {
                    println!("   [{:2}] ID: {:5}, block: {:?}, pos: ({:5}, {:5}, {:5}), rot: {:.1}°",
                        i, o.model_id, o.block, o.x, o.y, o.z, o.rotation_y_deg);
                }
            }
        }
    }

    // Cerca se ci sono mura (modelli 444, 445, 446) dentro blocchi NON-WALL in tutta la città
    println!("\n=== MURA O TORRI INTERNE ALLA CITTA' ===");
    for by in 0..loc.height {
        for bx in 0..loc.width {
            let name = &loc.block_names[by * loc.width + bx];
            if !name.starts_with("WALL") {
                if let Some(raw) = blocks.get_by_name(name) {
                    if let Ok(rmb) = RmbBlock::parse(raw) {
                        for o in &rmb.objects_3d {
                            if [444, 445, 446, 744, 746, 750].contains(&o.model_id) || o.model_id >= 41200 && o.model_id <= 41225 {
                                println!("   Blocco [{},{}] {}: Modello ID = {}, pos = ({}, {})",
                                    bx, by, name, o.model_id, o.x, o.z);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
