use crate::data::maps::DFLocation;
use crate::world::MAP_HEIGHT_PIXELS;
use image::GenericImageView;
use std::path::Path;

/// Struttura di supporto per la lettura del file txt
struct CityConfig {
    name: String,
    map_pixel_x: i32,
    map_pixel_y: i32,
    image_path: String,
    grid_size: usize,
}

/// Legge il file di testo e genera le DFLocation custom.
pub fn load_custom_cities(dir: &Path) -> Vec<DFLocation> {
    let mut out = Vec::new();
    let config_path = dir.join("custom_city.txt");

    let Ok(content) = std::fs::read_to_string(&config_path) else {
        return out;
    };

    let mut cfg = CityConfig {
        name: "Custom City".to_string(),
        map_pixel_x: 0,
        map_pixel_y: 0,
        image_path: "planimetria.png".to_string(),
        grid_size: 8,
    };

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        let value = value.trim();

        match key {
            "name" => cfg.name = value.to_string(),
            "map_pixel_x" => cfg.map_pixel_x = value.parse().unwrap_or(0),
            "map_pixel_y" => cfg.map_pixel_y = value.parse().unwrap_or(0),
            "image" => cfg.image_path = value.to_string(),
            "grid_size" => cfg.grid_size = value.parse().unwrap_or(8),
            _ => {}
        }
    }

    let img_path = dir.join(&cfg.image_path);
    let Ok(img) = image::open(&img_path) else {
        println!("ATTENZIONE: Impossibile caricare l'immagine custom {}", img_path.display());
        return out;
    };

    let (w, h) = img.dimensions();
    let mut block_names = Vec::with_capacity(cfg.grid_size * cfg.grid_size);

    println!("Generazione città custom '{}' da {}x{} pixel...", cfg.name, w, h);

    for y in 0..cfg.grid_size {
        for x in 0..cfg.grid_size {
            // Campiona l'immagine nella posizione corrispondente alla griglia
            let px_x = ((x as f32 / cfg.grid_size as f32) * w as f32) as u32;
            let px_y = ((y as f32 / cfg.grid_size as f32) * h as f32) as u32;
            
            let pixel = img.get_pixel(px_x, px_y);
            let (r, g, b, a) = (pixel[0], pixel[1], pixel[2], pixel[3]);

            // Se il pixel è trasparente o verde scuro, lo trattiamo come strada/vuoto
            let block = if a < 128 {
                "EMPTY".to_string()
            } else {
                match (r, g, b) {
                    // ROSSO: Casa (Residenza)
                    (150..=255, 0..=80, 0..=80) => "RESIAA01.RMB".to_string(),
                    // BLU: Taverna
                    (0..=80, 0..=80, 150..=255) => "TVRNAA01.RMB".to_string(),
                    // GIALLO: Gilda dei Maghi
                    (150..=255, 150..=255, 0..=80) => "MAGEAA01.RMB".to_string(),
                    // GRIGIO SCURO: Mura
                    (40..=90, 40..=90, 40..=90) => "WALLAA01.RMB".to_string(),
                    // VIOLA: Tempio
                    (150..=255, 0..=80, 150..=255) => "TEMPAA01.RMB".to_string(),
                    // CIANO: Negozio
                    (0..=80, 150..=255, 150..=255) => "GENRAA01.RMB".to_string(),
                    // MARRONE: Palazzo / Castello
                    (100..=150, 50..=100, 0..=50) => "PALA00I0.RMB".to_string(),
                    // Verde o altri colori -> Strada/Vuoto
                    _ => "EMPTY".to_string(),
                }
            };
            block_names.push(block);
        }
    }

    // Crea la DFLocation
    let loc = DFLocation {
        name: cfg.name,
        region_index: 0,
        location_id: 9999, // ID fittizio
        world_x: cfg.map_pixel_x * 32768,
        // Bevy ha Y invertito rispetto a Daggerfall
        world_y: (MAP_HEIGHT_PIXELS - 1 - cfg.map_pixel_y) * 32768,
        width: cfg.grid_size,
        height: cfg.grid_size,
        block_names,
        letter1_for_rmb_name: None,
    };

    out.push(loc);
    out
}