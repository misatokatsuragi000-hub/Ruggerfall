use std::path::PathBuf;
use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::blocks::RmbBlock;
use daggerfall_data::arch3d::Arch3dModel;
use daggerfall_data::building_gen::Footprint;
use daggerfall_data::maps::MapsReader;
use daggerfall_data::find_file_ci;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("uso: inspect_daggerfall_grid <ARENA2>"));
    let maps_buf = std::fs::read(find_file_ci(&dir, "MAPS.BSA").unwrap())?;
    let blocks_buf = std::fs::read(find_file_ci(&dir, "BLOCKS.BSA").unwrap())?;
    let arch_buf = std::fs::read(find_file_ci(&dir, "ARCH3D.BSA").unwrap())?;
    let maps = BsaArchive::parse(&maps_buf)?;
    let blocks = BsaArchive::parse(&blocks_buf)?;
    let arch = BsaArchive::parse(&arch_buf)?;
    let loc = MapsReader::new(&maps).find_location(17, "DAGGERFALL").unwrap();

    println!("=== GRIGLIA DI DAGGERFALL (8x8 BLOCCHI) ===");
    for by in 0..loc.height {
        let mut row = String::new();
        for bx in 0..loc.width {
            let name = &loc.block_names[by * loc.width + bx];
            row.push_str(&format!("{:-12} ", name));
        }
        println!("riga by={}: {}", by, row);
    }

    println!("\n=== ISPEZIONE BLOCCHI SPECIALI E GRANDI EDIFICI ===");
    for by in 0..loc.height {
        for bx in 0..loc.width {
            let name = &loc.block_names[by * loc.width + bx];
            if !name.starts_with("WALL") && (name.contains("CUST") || name.contains("PAL") || name.contains("CAST") || name.contains("MANR") || name.contains("TEMP") || bx <= 2 && by <= 2 || bx >= 5 && by >= 5) {
                if let Some(raw) = blocks.get_by_name(name) {
                    if let Ok(rmb) = RmbBlock::parse(raw) {
                        println!("\nBlocco [bx={}, by={}] {}:", bx, by, name);
                        for pm in rmb.placed_models() {
                            let fp_str = if let Some(m_raw) = arch.get_by_id(pm.model_id) {
                                if let Ok(m) = Arch3dModel::parse(pm.model_id, m_raw) {
                                    if let Some(fp) = Footprint::from_model(&m) {
                                        format!("{:.1}x{:.1}x{:.1}m -> {:?}", fp.width, fp.depth, fp.height, fp.classify())
                                    } else {
                                        "Footprint fallita".into()
                                    }
                                } else {
                                    "Parse Arch3d fallito".into()
                                }
                            } else {
                                "Modello NON in Arch3d".into()
                            };
                            if pm.is_building || pm.model_id >= 40000 || pm.model_id < 1000 {
                                println!("   id {:6} (is_building: {}) pos: ({:.1}, {:.1}) | {}",
                                    pm.model_id, pm.is_building, pm.x, pm.n, fp_str);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
