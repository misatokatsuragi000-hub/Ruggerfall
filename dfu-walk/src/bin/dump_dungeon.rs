//! Dump e test dei dungeon.
//! Uso: cargo run --release --bin dump_dungeon -- <ARENA2> [regione] [nome_dungeon]
//! Esempio: cargo run --release --bin dump_dungeon -- /path/to/arena2 17 "Privateer's Hold"

use std::path::PathBuf;
use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::dungeon_builder::build_dungeon;
use daggerfall_data::dungeon_layout::DungeonData;
use daggerfall_data::find_file_ci;
use daggerfall_data::maps::MapsReader;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("Uso: dump_dungeon <ARENA2> [regione] [nome]"));
    let region: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(17);
    let target_name = args.next().unwrap_or_else(|| "Privateer's Hold".to_string());

    let maps_buf = std::fs::read(find_file_ci(&dir, "MAPS.BSA").expect("MAPS.BSA"))?;
    let blocks_buf = std::fs::read(find_file_ci(&dir, "BLOCKS.BSA").expect("BLOCKS.BSA"))?;
    let arch_buf = std::fs::read(find_file_ci(&dir, "ARCH3D.BSA").expect("ARCH3D.BSA"))?;

    let maps_bsa = BsaArchive::parse(&maps_buf)?;
    let blocks_bsa = BsaArchive::parse(&blocks_buf)?;
    let arch_bsa = BsaArchive::parse(&arch_buf)?;

    let reader = MapsReader::new(&maps_bsa);
    let location = reader.find_location(region, &target_name)
        .expect("Località non trovata");

    println!("=== Dati Località ===");
    println!("Nome: {}, Region: {}", location.name, location.region_index);

    // In Daggerfall la combinazione (world_x, world_y) o LocationRecordElement.LocationId è l'ID
    // Per trovarlo con certezza cerchiamo il LocationId grezzo nel record MAPPITEM
    let loc_id = location.world_x as u32 & 0xFFFF; // Fallback euristico o id esatto

    println!("Ricerca dungeon in MAPDITEM.{:03}...", region);
    // Proviamo con gli ID candidati
    let dungeon_data = (0..65535u32).find_map(|cand_id| {
        DungeonData::read_from_maps(&maps_bsa, region, cand_id)
    });

    let Some(dungeon) = dungeon_data else {
        println!("Nessun dungeon associato trovato in questa provincia.");
        return Ok(());
    };

    println!("\n=== Layout Dungeon Trovato ===");
    println!("Numero Blocchi RDB: {}", dungeon.blocks.len());
    for (i, b) in dungeon.blocks.iter().enumerate() {
        println!("  [{:2}] Blocco: {:<12} Grid: ({:>2}, {:>2}) | Starting: {}",
            i, b.block_name, b.x, b.z, b.is_starting_block);
    }

    println!("\nCostruzione mesh 3D del Dungeon...");
    let built = build_dungeon(&dungeon, &blocks_bsa, &arch_bsa)
        .expect("Costruzione mesh fallita");

    println!("Vertici generati: {}", built.mesh.vertices.len());
    println!("Luci presenti: {}", built.lights.len());
    println!("Elementi flat: {}", built.flats.len());
    println!("Spawn entrata: {:?}", built.spawn.enter_pos);
    println!("Punto d'uscita: {:?}", built.spawn.exit_pos);

    let out_obj = "dungeon_dump.obj";
    built.mesh.export_obj(out_obj)?;
    println!("\nEsportato con successo in '{}'!", out_obj);

    Ok(())
}