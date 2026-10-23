use std::collections::HashMap;
use std::env;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::maps::MapsReader;
use daggerfall_data::blocks::RmbBlock;
use daggerfall_data::arch3d::Arch3dModel;
use daggerfall_data::mesh_builder::WorldMesh;

fn read_arena2_file(base_dir: &Path, filename: &str) -> Result<Vec<u8>, std::io::Error> {
    let upper = base_dir.join(filename.to_ascii_uppercase());
    let lower = base_dir.join(filename.to_ascii_lowercase());

    let target = if upper.exists() { upper } else { lower };
    let mut file = File::open(&target)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arena2_dir = env::args()
    .nth(1)
    .map(PathBuf::from)
    .unwrap_or_else(|| PathBuf::from("/mnt/1CE16DBE48849731/Daggerfall/arena2"));

    println!("=== Caricamento BSA per Generazione Mesh ===");

    let maps_buf = read_arena2_file(&arena2_dir, "MAPS.BSA")?;
    let maps_bsa = BsaArchive::parse(&maps_buf)?;

    let blocks_buf = read_arena2_file(&arena2_dir, "BLOCKS.BSA")?;
    let blocks_bsa = BsaArchive::parse(&blocks_buf)?;

    let arch3d_buf = read_arena2_file(&arena2_dir, "ARCH3D.BSA")?;
    let arch3d_bsa = BsaArchive::parse(&arch3d_buf)?;

    let maps_reader = MapsReader::new(&maps_bsa);
    let location = maps_reader.find_location(17, "DAGGERFALL")
    .expect("Location Daggerfall non trovata");

    println!("Location: {}, Dimensioni: {}x{} blocchi", location.name, location.width, location.height);

    // Cache locale dei modelli 3D caricati
    let mut model_cache: HashMap<u32, Arch3dModel> = HashMap::new();

    // Scegliamo un blocco urbano centrale ricco di edifici (es. Blocco 50: TVRNAL06.RMB)
    let target_block_idx = 50;
    let block_name = &location.block_names[target_block_idx];
    println!("\nAssemblaggio mesh 3D per blocco [{}]: {}", target_block_idx, block_name);

    let rmb_raw = blocks_bsa.get_by_name(block_name).expect("Blocco non trovato");
    let rmb = RmbBlock::parse(rmb_raw)?;

    println!("Oggetti 3D nel blocco: {}", rmb.objects_3d.len());

    let mut world_mesh = WorldMesh::new();
    let mut loaded_models = 0;

    for obj in &rmb.objects_3d {
        // Carica il modello da ARCH3D.BSA se non è già in cache
        if !model_cache.contains_key(&obj.model_id) {
            if let Some(m_raw) = arch3d_bsa.get_by_id(obj.model_id) {
                if let Ok(m) = Arch3dModel::parse(obj.model_id, m_raw) {
                    model_cache.insert(obj.model_id, m);
                }
            }
        }

        if let Some(model) = model_cache.get(&obj.model_id) {
            // Conversione unità DFU -> metri
            let pos_x = obj.x as f32 / 39.37;
            let pos_y = -obj.y as f32 / 39.37;
            let pos_z = obj.z as f32 / 39.37;

            world_mesh.add_instance(model, pos_x, pos_y, pos_z, obj.rotation_y_deg);
            loaded_models += 1;
        }
    }

    println!("Istanziati {} oggetti 3D nella scena!", loaded_models);
    println!("Totale Vertici generati: {}", world_mesh.vertices.len());
    println!("Totale Submesh (per texture): {}", world_mesh.submeshes.len());

    let output_obj = "city_block_tavern.obj";
    world_mesh.export_obj(output_obj)?;
    println!("\nScena 3D esportata con successo in: {}", output_obj);

    Ok(())
}
