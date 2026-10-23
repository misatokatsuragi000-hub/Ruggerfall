//! Strumento di debug: genera la tilemap di un map pixel e la salva come PNG,
//! colorando ogni cella con il colore medio della sua tile (rotazione e
//! specchiatura sono ignorate, servono solo a controllare la distribuzione).
//!
//! Uso: dump_tilemap <cartella ARENA2> <X> <Y> [--winter] [--scale N] --out file.png

use daggerfall_data::{
    archive_file_name, find_file_ci, generate_default_samples, generate_default_tilemap,
    ground_archive, load_tile_array, png, PakFile, Palette, TextureFile, WoodsFile,
};
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("errore: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().ok_or("manca la cartella ARENA2")?;
    let x: i32 = args.next().ok_or("manca X")?.parse()?;
    let y: i32 = args.next().ok_or("manca Y")?.parse()?;
    let (mut winter, mut scale, mut out) = (false, 4usize, None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--winter" => winter = true,
            "--scale" => scale = args.next().ok_or("--scale vuole un valore")?.parse()?,
            "--out" => out = Some(args.next().ok_or("--out vuole un file")?),
            other => return Err(format!("opzione sconosciuta: {other}").into()),
        }
    }
    let out = out.ok_or("manca --out file.png")?;

    let woods = WoodsFile::open(find_file_ci(&dir, "WOODS.WLD").ok_or("WOODS.WLD non trovato")?)?;
    let pak = PakFile::open(find_file_ci(&dir, "CLIMATE.PAK").ok_or("CLIMATE.PAK non trovato")?)?;
    let palette = Palette::open(find_file_ci(&dir, "ART_PAL.COL").ok_or("ART_PAL.COL non trovato")?)?;

    let climate = pak.map_pixel_value(x, y).ok_or("map pixel fuori mappa")? as i32;
    let archive = ground_archive(climate, winter);
    let name = archive_file_name(archive as u32);
    println!("map pixel ({x}, {y}): clima {climate} -> {name}");
    let tiles = load_tile_array(&TextureFile::open(
        find_file_ci(&dir, &name).ok_or_else(|| format!("{name} non trovato"))?,
    )?)?;

    // Colore medio di ogni tile.
    let lut = palette.to_rgba(None);
    let avg: Vec<[u8; 3]> = (0..tiles.layers)
        .map(|t| {
            let l = tiles.layer(t);
            let mut sum = [0u64; 3];
            for &i in l {
                for c in 0..3 {
                    sum[c] += lut[i as usize][c] as u64;
                }
            }
            let n = l.len().max(1) as u64;
            [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]
        })
        .collect();

    let samples = generate_default_samples(&woods, x, y)?;
    let tilemap = generate_default_tilemap(&samples, x, y, None)?;

    let mut histogram = [0usize; 56];
    let side = 128 * scale;
    let mut rgba = vec![255u8; side * side * 4];
    for ty in 0..128 {
        for tx in 0..128 {
            let idx = (tilemap[tx + ty * 128] & 63) as usize;
            histogram[idx.min(55)] += 1;
            let c = avg[idx.min(55)];
            for dy in 0..scale {
                for dx in 0..scale {
                    let p = ((ty * scale + dy) * side + tx * scale + dx) * 4;
                    rgba[p..p + 3].copy_from_slice(&c);
                }
            }
        }
    }
    std::fs::write(&out, png::encode_rgba(side as u32, side as u32, &rgba))?;
    println!("tilemap salvata in {out}");
    let mut used: Vec<(usize, usize)> = histogram.iter().copied().enumerate().filter(|&(_, n)| n > 0).collect();
    used.sort_by(|a, b| b.1.cmp(&a.1));
    for (tile, n) in used.iter().take(8) {
        println!("  tile {tile:2}: {n} celle");
    }
    Ok(())
}
