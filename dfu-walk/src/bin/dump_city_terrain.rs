use std::env;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;
use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::maps::MapsReader;
use daggerfall_data::terrain_city::{assemble_city_tilemap, flatten_terrain_heights};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arena2_dir = env::args()
    .nth(1)
    .map(PathBuf::from)
    .unwrap_or_else(|| PathBuf::from("/mnt/1CE16DBE48849731/Daggerfall/arena2"));

    let mut maps_buf = Vec::new();
    File::open(arena2_dir.join("MAPS.BSA"))?.read_to_end(&mut maps_buf)?;
    let maps_bsa = BsaArchive::parse(&maps_buf)?;

    let mut blocks_buf = Vec::new();
    File::open(arena2_dir.join("BLOCKS.BSA"))?.read_to_end(&mut blocks_buf)?;
    let blocks_bsa = BsaArchive::parse(&blocks_buf)?;

    let maps_reader = MapsReader::new(&maps_bsa);
    let location = maps_reader.find_location(17, "DAGGERFALL")
    .expect("Location Daggerfall non trovata");

    println!("=== Test Integrazione Città-Terreno ===");
    println!("Città: {}, Blocchi: {}x{}", location.name, location.width, location.height);

    // 1. Assemblaggio preset tilemap
    let (preset, city_rect) = assemble_city_tilemap(&location, &blocks_bsa);
    println!("Preset generato: {} tile, Bounding Box: [{}, {}] -> [{}, {}]",
             preset.len(), city_rect.min_x, city_rect.min_y, city_rect.max_x, city_rect.max_y);

    // 2. Simulazione griglia altezze naturale collinare (128x128 campioni con pendenza e colline)
    let dim = 128;
    let mut heights = vec![0.0f32; dim * dim];
    for y in 0..dim {
        for x in 0..dim {
            let hill = ((x as f32 * 0.1).sin() + (y as f32 * 0.1).cos()) * 15.0;
            let slope = (x as f32 + y as f32) * 0.5;
            heights[y * dim + x] = 100.0 + hill + slope;
        }
    }

    let h_min_before = heights.iter().cloned().fold(f32::INFINITY, f32::min);
    let h_max_before = heights.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    println!("Altezze naturali prima del livellamento: min = {:.2}m, max = {:.2}m", h_min_before, h_max_before);

    // 3. Esecuzione livellamento terreno
    flatten_terrain_heights(&mut heights, dim, &city_rect, 6.0);

    // Verifica altezza nel centro e nei 4 angoli interni della città
    let c_center = heights[64 * dim + 64];
    let c_p1 = heights[city_rect.min_y * dim + city_rect.min_x];
    let c_p2 = heights[(city_rect.max_y - 1) * dim + (city_rect.max_x - 1)];

    println!("Altezze verificate dentro la città:");
    println!("  Centro (64, 64): {:.4}m", c_center);
    println!("  Angolo min ({}, {}): {:.4}m", city_rect.min_x, city_rect.min_y, c_p1);
    println!("  Angolo max ({}, {}): {:.4}m", city_rect.max_x - 1, city_rect.max_y - 1, c_p2);
    assert!((c_center - c_p1).abs() < 1e-4 && (c_center - c_p2).abs() < 1e-4,
            "L'area urbana non è perfettamente piatta!");
    println!("-> Il terreno urbano è perfettamente piatto a quota costante!");

    // 4. Test scatter alberi procedurali
    let total_random_trees = 500;
    let mut trees_spawned = 0;
    let mut trees_culled_in_city = 0;

    for i in 0..total_random_trees {
        // Coordinate pseudo-casuali nel chunk
        let tx = (i * 37) % 128;
        let ty = (i * 73) % 128;

        if city_rect.contains_tile(tx, ty) {
            trees_culled_in_city += 1; // Spento dentro la città!
        } else {
            trees_spawned += 1;
        }
    }

    println!("\nTest maschera scatter alberi:");
    println!("  Alberi naturali generati fuori città: {}", trees_spawned);
    println!("  Alberi soppressi dentro la città: {} (sostituiti dai flat dei blocchi RMB)", trees_culled_in_city);

    Ok(())
}
