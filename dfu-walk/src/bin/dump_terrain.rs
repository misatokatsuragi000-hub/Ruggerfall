//! Strumento di debug: risolve l'archivio delle tile del terreno per un clima
//! mondiale (223..=232) e una stagione, carica le 56 tile e salva un foglio
//! contatti PNG (8 colonne x 7 righe).
//!
//! Uso: dump_terrain <cartella ARENA2> <clima 223-232> [--winter] [--out file.png]

use daggerfall_data::{
    archive_file_name, find_file_ci, ground_archive, load_tile_array, png, Palette, TextureFile,
};
use std::process::ExitCode;

const COLS: usize = 8;

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
    let climate: i32 = args.next().ok_or("manca il clima (223-232)")?.parse()?;
    let mut winter = false;
    let mut out: Option<String> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--winter" => winter = true,
            "--out" => out = Some(args.next().ok_or("--out vuole un valore")?),
            other => return Err(format!("opzione sconosciuta: {other}").into()),
        }
    }

    let archive = ground_archive(climate, winter);
    let name = archive_file_name(archive as u32);
    println!("clima {climate}, inverno={winter} -> {name}");

    let path = find_file_ci(&dir, &name).ok_or_else(|| format!("{name} non trovato in {dir}"))?;
    let palette = Palette::open(find_file_ci(&dir, "ART_PAL.COL").ok_or("ART_PAL.COL non trovato")?)?;
    let arr = load_tile_array(&TextureFile::open(path)?)?;
    println!("{} layer di {}x{}", arr.layers, arr.width, arr.height);
    for i in 0..arr.layers {
        println!("  tile {i:2}: crc32(indici)={:08x}", png::crc32(arr.layer(i)));
    }

    if let Some(file) = out {
        let rows = arr.layers.div_ceil(COLS);
        let (sw, sh) = (COLS * arr.width, rows * arr.height);
        let lut = palette.to_rgba(None);
        let mut rgba = vec![0u8; sw * sh * 4];
        for t in 0..arr.layers {
            let (ox, oy) = ((t % COLS) * arr.width, (t / COLS) * arr.height);
            for (i, &idx) in arr.layer(t).iter().enumerate() {
                let (x, y) = (ox + i % arr.width, oy + i / arr.width);
                rgba[(y * sw + x) * 4..][..4].copy_from_slice(&lut[idx as usize]);
            }
        }
        std::fs::write(&file, png::encode_rgba(sw as u32, sh as u32, &rgba))?;
        println!("foglio contatti salvato in {file}");
    }
    Ok(())
}
