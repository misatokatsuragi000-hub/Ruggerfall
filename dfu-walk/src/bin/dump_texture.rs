//! Esporta i record di un archivio TEXTURE.nnn come PNG.
//!
//! Uso:
//!   dump_texture <ARENA2> <archive>              # tutti i record
//!   dump_texture <ARENA2> <archive> <record>     # un solo record
//!
//! Esempio:  cargo run --bin dump_texture -- /percorso/ARENA2 201
//! Produce i file in ./dumped_textures/201_<record>-<frame>.png

use daggerfall_data::{archive_file_name, find_file_ci, Palette, TextureFile};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(
        args.next()
            .ok_or("uso: dump_texture <ARENA2> <archive> [record]")?,
    );
    let archive: u32 = args.next().ok_or("manca <archive>")?.parse()?;
    let only_record: Option<usize> = args.next().map(|s| s.parse()).transpose()?;

    let tex_name = archive_file_name(archive);
    let tex_path = find_file_ci(&dir, &tex_name)
        .ok_or_else(|| format!("{tex_name} non trovato in {}", dir.display()))?;
    let tex = TextureFile::open(&tex_path)?;

    let pal_path = find_file_ci(&dir, "ART_PAL.COL")
        .ok_or("ART_PAL.COL non trovato in ARENA2")?;
    let palette = Palette::open(&pal_path)?;

    let out_dir = PathBuf::from("dumped_textures");
    std::fs::create_dir_all(&out_dir)?;

    let n = tex.record_count();
    println!("{tex_name}: {n} record, {} frame", 
        (0..n).map(|r| tex.frame_count(r).unwrap_or(0)).sum::<usize>());

    let mut written = 0usize;
    for r in 0..n {
        if let Some(only) = only_record {
            if r != only { continue; }
        }
        let frames = tex.frame_count(r).unwrap_or(0);
        for f in 0..frames {
            let img = match tex.frame(r, f) {
                Ok(i) => i,
                Err(e) => { eprintln!("record {r} frame {f}: {e}"); continue; }
            };
            if img.width == 0 || img.height == 0 { continue; }
            let rgba = img.to_rgba(&palette, Some(0));
            let name = out_dir.join(format!("{archive:03}_{r}-{f}.png"));
            image::save_buffer(
                &name,
                &rgba,
                img.width as u32,
                img.height as u32,
                image::ColorType::Rgba8,
            )?;
            println!("  {} ({}x{})", name.display(), img.width, img.height);
            written += 1;
        }
    }
    println!("Scritti {written} PNG in {}", out_dir.display());
    Ok(())
}