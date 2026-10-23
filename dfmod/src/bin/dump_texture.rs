//! Strumento di debug: elenca i record di un archivio TEXTURE.nnn, ne stampa un
//! CRC32 degli indici (da confrontare con DFU) e, a richiesta, salva i PNG.
//!
//! Uso: dump_texture <cartella ARENA2> <archivio> [--record N] [--frame N]
//!                   [--out cartella] [--alpha0]

use daggerfall_data::{archive_file_name, find_file_ci, png, Palette, TextureFile};
use std::path::PathBuf;
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
    let archive: u32 = args.next().ok_or("manca il numero di archivio")?.parse()?;

    let (mut only_record, mut only_frame) = (None, None);
    let mut out_dir: Option<PathBuf> = None;
    let mut alpha0 = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--record" => only_record = Some(args.next().ok_or("--record vuole un valore")?.parse::<usize>()?),
            "--frame" => only_frame = Some(args.next().ok_or("--frame vuole un valore")?.parse::<usize>()?),
            "--out" => out_dir = Some(PathBuf::from(args.next().ok_or("--out vuole un valore")?)),
            "--alpha0" => alpha0 = true,
            other => return Err(format!("opzione sconosciuta: {other}").into()),
        }
    }

    let name = archive_file_name(archive);
    let path = find_file_ci(&dir, &name).ok_or_else(|| format!("{name} non trovato in {dir}"))?;
    let pal_path = find_file_ci(&dir, "ART_PAL.COL").ok_or("ART_PAL.COL non trovato")?;
    let tex = TextureFile::open(&path)?;
    let palette = Palette::open(&pal_path)?;

    if let Some(d) = &out_dir {
        std::fs::create_dir_all(d)?;
    }

    for record in 0..tex.record_count() {
        if only_record.is_some_and(|r| r != record) {
            continue;
        }
        let rec = tex.record(record).unwrap();
        let frames = tex.frame_count(record).unwrap_or(0);
        println!(
            "record {record}: {}x{} frame={frames} compressione={:?} scala=({},{}) offset=({},{})",
            rec.width, rec.height, rec.compression, rec.scale_x, rec.scale_y, rec.offset_x, rec.offset_y
        );
        for frame in 0..frames {
            if only_frame.map_or(frame != 0, |f| f != frame) {
                continue;
            }
            match tex.frame(record, frame) {
                Ok(img) => {
                    println!("  frame {frame}: crc32(indici)={:08x}", png::crc32(&img.indices));
                    if let Some(d) = &out_dir {
                        let rgba = img.to_rgba(&palette, alpha0.then_some(0));
                        let bytes = png::encode_rgba(img.width as u32, img.height as u32, &rgba);
                        let file = d.join(format!("{name}_{record:03}_{frame}.png"));
                        std::fs::write(file, bytes)?;
                    }
                }
                Err(e) => println!("  frame {frame}: ERRORE {e}"),
            }
        }
    }
    Ok(())
}
