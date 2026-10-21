//! Strumento di debug per i dati del mondo: salva come PNG la mappa delle altezze
//! (WOODS.WLD), la mappa dei climi (CLIMATE.PAK) e i campioni di altezza di un
//! map pixel.
//!
//! Uso: dump_world <cartella ARENA2> [--heightmap file.png] [--climate file.png]
//!                  [--samples X Y file.png]

use daggerfall_data::{find_file_ci, generate_default_samples, png, PakFile, WoodsFile};
use std::process::ExitCode;

const W: usize = 1000;
const H: usize = 500;

const CLIMATE_COLORS: [(i32, &str, [u8; 3]); 10] = [
    (223, "oceano", [20, 40, 120]),
    (224, "deserto", [230, 200, 120]),
    (225, "deserto 2", [210, 170, 90]),
    (226, "montagna", [140, 140, 140]),
    (227, "foresta pluviale", [0, 120, 40]),
    (228, "palude", [90, 110, 60]),
    (229, "sub-tropicale", [200, 190, 60]),
    (230, "boschi di montagna", [80, 120, 100]),
    (231, "boschi", [60, 160, 60]),
    (232, "boschi infestati", [120, 60, 140]),
];

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("errore: {e}");
            ExitCode::FAILURE
        }
    }
}

fn gray_png(width: usize, height: usize, gray: impl Fn(usize, usize) -> u8) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(width * height * 4);
    for y in 0..height {
        for x in 0..width {
            let g = gray(x, y);
            rgba.extend_from_slice(&[g, g, g, 255]);
        }
    }
    png::encode_rgba(width as u32, height as u32, &rgba)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().ok_or("manca la cartella ARENA2")?;
    let (mut heightmap, mut climate, mut samples) = (None, None, None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--heightmap" => heightmap = Some(args.next().ok_or("--heightmap vuole un file")?),
            "--climate" => climate = Some(args.next().ok_or("--climate vuole un file")?),
            "--samples" => {
                let x: i32 = args.next().ok_or("--samples vuole X Y file")?.parse()?;
                let y: i32 = args.next().ok_or("--samples vuole X Y file")?.parse()?;
                let f = args.next().ok_or("--samples vuole X Y file")?;
                samples = Some((x, y, f));
            }
            other => return Err(format!("opzione sconosciuta: {other}").into()),
        }
    }

    if heightmap.is_some() || samples.is_some() {
        let path = find_file_ci(&dir, "WOODS.WLD").ok_or("WOODS.WLD non trovato")?;
        let woods = WoodsFile::open(path)?;
        let map = woods.height_map();
        let (min, max) = (map.iter().min().unwrap(), map.iter().max().unwrap());
        println!("WOODS.WLD: altezze da {min} a {max}");

        if let Some(file) = heightmap {
            std::fs::write(&file, gray_png(W, H, |x, y| map[y * W + x]))?;
            println!("mappa delle altezze salvata in {file}");
        }
        if let Some((x, y, file)) = samples {
            let s = generate_default_samples(&woods, x, y)?;
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for &v in &s.data {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            println!("map pixel ({x}, {y}): altezza {:.1} .. {:.1} (campioni 0..1: {lo:.4} .. {hi:.4})",
                lo * daggerfall_data::heightmap::MAX_TERRAIN_HEIGHT,
                hi * daggerfall_data::heightmap::MAX_TERRAIN_HEIGHT);
            let range = (hi - lo).max(f32::EPSILON);
            let d = s.dim;
            std::fs::write(&file, gray_png(d, d, |px, py| (((s.get(px, py) - lo) / range) * 255.0) as u8))?;
            println!("campioni salvati in {file} (contrasto normalizzato)");
        }
    }

    if let Some(file) = climate {
        let path = find_file_ci(&dir, "CLIMATE.PAK").ok_or("CLIMATE.PAK non trovato")?;
        let pak = PakFile::open(path)?;
        let mut rgba = Vec::with_capacity(W * H * 4);
        let mut counts = [0usize; 10];
        for y in 0..H {
            for x in 0..W {
                let v = pak.map_pixel_value(x as i32, y as i32).unwrap_or(0) as i32;
                match CLIMATE_COLORS.iter().position(|c| c.0 == v) {
                    Some(i) => {
                        counts[i] += 1;
                        let c = CLIMATE_COLORS[i].2;
                        rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
                    }
                    None => rgba.extend_from_slice(&[0, 0, 0, 255]),
                }
            }
        }
        std::fs::write(&file, png::encode_rgba(W as u32, H as u32, &rgba))?;
        println!("mappa dei climi salvata in {file}");
        for (i, c) in CLIMATE_COLORS.iter().enumerate() {
            println!("  {} {:<20} {} pixel", c.0, c.1, counts[i]);
        }
    }
    Ok(())
}
