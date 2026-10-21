use std::{collections::BTreeMap, env, path::Path};
use dfmod::ModArchive;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Uso: dump_textures <file.dfmod> [filtro_testuale]");
        std::process::exit(1);
    }

    let archive = ModArchive::open(Path::new(&args[1]))?;
    let filter = args.get(2).map(|s| s.to_ascii_lowercase());

    println!("=== File: {} ===", args[1]);
    println!("Totale asset nel container: {}", archive.assets().len());

    // Statistiche classi
    let mut by_class: BTreeMap<i32, usize> = BTreeMap::new();
    for asset in archive.assets() {
        *by_class.entry(asset.class_id).or_default() += 1;
    }
    println!("\nRiepilogo classi:");
    for (cid, count) in &by_class {
        let desc = match *cid {
            21 => "Material (.mat)",
            28 => "Texture2D",
            49 => "TextAsset",
            187 => "Texture2DArray",
            _ => "Altro",
        };
        println!("  class {:3} ({:16}): {} asset", cid, desc, count);
    }

    let textures: Vec<_> = archive.texture_names().collect();
    println!("\nTexture2D estratte da dfmod: {}", textures.len());
    let mut printed = 0;
    for name in textures {
        if let Some(ref f) = filter {
            if !name.to_ascii_lowercase().contains(f) {
                continue;
            }
        }
        if let Ok(Some(tex)) = archive.texture(name) {
            println!("  {}: {}x{} {} mips={}", name, tex.width, tex.height, tex.format.name(), tex.mip_count);
        } else {
            println!("  {name}");
        }
        printed += 1;
        if filter.is_none() && printed >= 25 {
            println!("  ... [altre omesse, passa un filtro come secondo argomento per cercare]");
            break;
        }
    }

    let arrays: Vec<_> = archive.texture_array_names().collect();
    if !arrays.is_empty() {
        println!("\nTexture2DArray trovati: {}", arrays.len());
        for name in arrays {
            println!("  array: {name}");
        }
    }

    if archive.texture_names().next().is_none() && archive.texture_array_names().next().is_none() {
        println!("\nPrimi 20 asset nel file (percorso grezzo):");
        for a in archive.assets().iter().take(20) {
            println!("  class {:3}  {}", a.class_id, a.path);
        }
    }

    Ok(())
}
