//! Mostra i blocchi di una localita' e, per ciascun oggetto di un blocco WALL o piu' alto
//! di 15 m, le dimensioni e come viene classificato (casa / muro / torre / scartato).
//! Serve a capire cosa e' finito dove se qualche pezzo delle mura risulta sbagliato.
//! Uso: cargo run --release --bin dump_wall_blocks -- <ARENA2> [regione=17] [NOME=DAGGERFALL]

use std::path::PathBuf;

use daggerfall_data::arch3d::{decode_face_texture, Arch3dModel};
use daggerfall_data::blocks::RmbBlock;
use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::building_gen::Footprint;
use daggerfall_data::find_file_ci;
use daggerfall_data::maps::MapsReader;
use daggerfall_data::wall_gen::classify_wall;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("uso: dump_wall_blocks <ARENA2> [regione] [nome]"));
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
    println!("{} ({}x{} blocchi)", loc.name, loc.width, loc.height);

    for by in 0..loc.height {
        for bx in 0..loc.width {
            let bname = &loc.block_names[by * loc.width + bx];
            let is_wall = bname.to_ascii_uppercase().starts_with("WALL");
            let Some(raw) = blocks.get_by_name(bname) else {
                println!("[{bx},{by}] {bname}: NON TROVATO in BLOCKS.BSA");
                continue;
            };
            let Ok(rmb) = RmbBlock::parse(raw) else {
                println!("[{bx},{by}] {bname}: parse fallito");
                continue;
            };
            let models = rmb.placed_models();
            let n_b = models.iter().filter(|m| m.is_building).count();
            println!("[{bx},{by}] {bname}{}: {n_b} oggetti di sotto-record", if is_wall { "  <-- MURA" } else { "" });

            for m in models.iter().filter(|m| m.is_building) {
                let model = arch.get_by_id(m.model_id).and_then(|r| Arch3dModel::parse(m.model_id, r).ok());
                let tex = model.as_ref().and_then(|a| a.dominant_texture());
                let raw = model.as_ref().and_then(|a| a.faces.first().map(|f| (f.texture_archive, f.texture_record)));
                let fp = model.as_ref().and_then(Footprint::from_model);
                let Some(fp) = fp else {
                    if is_wall {
                        println!("      id {:6}  modello mancante", m.model_id);
                    }
                    continue;
                };
                if !is_wall && fp.height < 15.0 {
                    continue;
                }
                println!(
                    "      id {:6}  {:5.1} x {:5.1} x {:5.1} m   casa={:?}  muro={:?}",
                    m.model_id,
                    fp.width,
                    fp.depth,
                    fp.height,
                    fp.classify(),
                    classify_wall(&fp)
                );
                if is_wall {
                    let first = raw.map(|(a, r)| decode_face_texture(a, r));
                    println!("            texture dominante {:?}   (prima faccia: grezzo {:?} -> {:?})", tex, raw, first);
                }
            }
        }
    }
    Ok(())
}
