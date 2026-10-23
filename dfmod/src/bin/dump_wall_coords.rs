use std::path::PathBuf;
use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::blocks::RmbBlock;
use daggerfall_data::maps::MapsReader;
use daggerfall_data::find_file_ci;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("uso: dump_wall_coords <ARENA2>"));
    let maps_buf = std::fs::read(find_file_ci(&dir, "MAPS.BSA").expect("MAPS.BSA mancante"))?;
    let blocks_buf = std::fs::read(find_file_ci(&dir, "BLOCKS.BSA").expect("BLOCKS.BSA mancante"))?;
    let maps = BsaArchive::parse(&maps_buf)?;
    let blocks = BsaArchive::parse(&blocks_buf)?;

    let loc = MapsReader::new(&maps).find_location(17, "DAGGERFALL").expect("località DAGGERFALL non trovata");
    println!("Città: {} (griglia {}x{} blocchi)", loc.name, loc.width, loc.height);

    for by in 0..loc.height {
        for bx in 0..loc.width {
            let bname = &loc.block_names[by * loc.width + bx];
            if bname.to_ascii_uppercase().starts_with("WALL") {
                let raw = blocks.get_by_name(bname).expect("blocco non trovato");
                let rmb = RmbBlock::parse(raw).expect("parse fallito");
                println!("\nBLOCCO MURA [colonna bx={}, riga by={}] {}", bx, by, bname);
                for (i, s) in rmb.subrecords.iter().enumerate() {
                    println!("   Subrecord[{}]: x={}, z={}, rot={:.1}°", i, s.x, s.z, s.rotation_deg);
                }
                for o in &rmb.objects_3d {
                    if [444, 445, 446].contains(&o.model_id) {
                        println!("   -> Modello Mura {}: block={:?}, x={}, y={}, z={}, rot={:.1}°",
                            o.model_id, o.block, o.x, o.y, o.z, o.rotation_y_deg);
                    }
                }
            }
        }
    }
    Ok(())
}
