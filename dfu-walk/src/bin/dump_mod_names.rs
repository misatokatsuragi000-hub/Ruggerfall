//! Stampa tutti i nomi delle texture di un file .dfmod su stdout (una per riga).
//! Uso: cargo run --release --bin dump_mod_names -- <file.dfmod> [filtro]

use std::path::Path;
use dfmod::ModArchive;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Uso: cargo run --release --bin dump_mod_names -- <file.dfmod> [filtro]");
        std::process::exit(1);
    }

    let mod_path = Path::new(&args[1]);
    let filter = args.get(2).map(|s| s.to_lowercase());

    let archive = ModArchive::open(mod_path)?;

    for name in archive.texture_names() {
        // Applica filtro opzionale se fornito da riga comando
        if let Some(ref f) = filter {
            if !name.to_lowercase().contains(f.as_str()) {
                continue;
            }
        }

        // Stampa SOLO il nome grezzo, niente intestazioni o log
        println!("{}", name);
    }

    Ok(())
}
