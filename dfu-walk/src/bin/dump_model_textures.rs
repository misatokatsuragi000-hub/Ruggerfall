//! Verifica la lettura dei modelli ARCH3D (piani, texture, punti):
//!  - come sono stati letti i record (offset dell'intestazione o ricerca);
//!  - se il raggio calcolato dai punti coincide con il campo Radius dell'intestazione;
//!  - per le facce, quante puntano a una texture che esiste in ARENA2;
//!  - la texture dominante di ogni modello delle mura, con l'esito.
//! Uso: cargo run --release --bin dump_model_textures -- <ARENA2> [regione=17] [NOME=DAGGERFALL]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use daggerfall_data::arch3d::Arch3dModel;
use daggerfall_data::blocks::RmbBlock;
use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::climate::{apply_climate, ClimateBase, Season};
use daggerfall_data::maps::MapsReader;
use daggerfall_data::{archive_file_name, find_file_ci, TextureFile};

fn record_count(cache: &mut HashMap<u32, Option<usize>>, dir: &Path, archive: u32) -> Option<usize> {
    *cache.entry(archive).or_insert_with(|| {
        let path = find_file_ci(dir, &archive_file_name(archive))?;
        TextureFile::open(&path).ok().map(|f| f.record_count())
    })
}

/// (esiste cosi' com'e', archivio dopo il clima temperato/estate, esiste dopo il clima)
fn check(cache: &mut HashMap<u32, Option<usize>>, dir: &Path, archive: u32, record: u32) -> (bool, u32, bool) {
    let direct = record_count(cache, dir, archive).map_or(false, |n| (record as usize) < n);
    let swapped = apply_climate(archive as i32, record as i32, ClimateBase::Temperate, Season::Summer).max(0) as u32;
    let after = record_count(cache, dir, swapped).map_or(false, |n| (record as usize) < n);
    (direct, swapped, after)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("uso: dump_model_textures <ARENA2> [regione] [nome]"));
    let region: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(17);
    let name = args.next().unwrap_or_else(|| "DAGGERFALL".to_string());

    let read = |n: &str| -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        Ok(std::fs::read(find_file_ci(&dir, n).ok_or_else(|| format!("{n} non trovato"))?)?)
    };
    let maps_buf = read("MAPS.BSA")?;
    let blocks_buf = read("BLOCKS.BSA")?;
    let arch_buf = read("ARCH3D.BSA")?;
    let maps = BsaArchive::parse(&maps_buf)?;
    let blocks = BsaArchive::parse(&blocks_buf)?;
    let arch = BsaArchive::parse(&arch_buf)?;
    let loc = MapsReader::new(&maps).find_location(region, &name).ok_or("località non trovata")?;

    // modelli della localita': (id, vengono dai blocchi WALL?)
    let mut models: Vec<(u32, bool)> = Vec::new();
    for bname in &loc.block_names {
        let is_wall = bname.to_ascii_uppercase().starts_with("WALL");
        let Some(raw) = blocks.get_by_name(bname) else { continue };
        let Ok(rmb) = RmbBlock::parse(raw) else { continue };
        for m in rmb.placed_models().iter().filter(|m| m.is_building) {
            match models.iter_mut().find(|(id, _)| *id == m.model_id) {
                Some(e) => e.1 |= is_wall,
                None => models.push((m.model_id, is_wall)),
            }
        }
    }
    println!("{}: {} modelli distinti ({} nei blocchi WALL)", loc.name, models.len(), models.iter().filter(|m| m.1).count());

    let mut cache: HashMap<u32, Option<usize>> = HashMap::new();
    let (mut total, mut parsed, mut failed) = (0usize, 0usize, 0usize);
    let (mut planes_hdr, mut points_hdr) = (0usize, 0usize);
    let (mut radius_ok, mut radius_bad) = (0usize, 0usize);
    let mut point_starts: HashMap<usize, usize> = HashMap::new();
    let mut versions: HashMap<String, usize> = HashMap::new();
    let (mut faces_ok, mut faces_total) = (0usize, 0usize);
    let mut bad_radius_list: Vec<String> = Vec::new();

    for (id, _) in &models {
        let Some(data) = arch.get_by_id(*id) else { continue };
        total += 1;
        match Arch3dModel::parse_with_info(*id, data) {
            Err(e) => {
                failed += 1;
                println!("  modello {id}: ERRORE {e}");
            }
            Ok((m, info)) => {
                parsed += 1;
                planes_hdr += info.planes_from_header as usize;
                points_hdr += info.points_from_header as usize;
                *point_starts.entry(info.point_start).or_default() += 1;
                *versions.entry(String::from_utf8_lossy(&info.version).into_owned()).or_default() += 1;
                let err = (info.radius_computed - info.radius_header).abs() / info.radius_header.abs().max(1.0);
                if err < 0.01 {
                    radius_ok += 1;
                } else {
                    radius_bad += 1;
                    if bad_radius_list.len() < 8 {
                        bad_radius_list.push(format!(
                            "{id}: Radius {} calcolato {:.0} (punti a {}, header {})",
                            info.radius_header, info.radius_computed, info.point_start, info.header_point_offset
                        ));
                    }
                }
                for f in &m.faces {
                    let (direct, _, after) = check(&mut cache, &dir, f.texture_archive as u32, f.texture_record as u32);
                    faces_total += 1;
                    faces_ok += (direct || after) as usize;
                }
            }
        }
    }
    println!("\nmodelli letti: {parsed}/{total} (errori {failed})");
    println!("piani trovati dove dice l'intestazione: {planes_hdr}/{parsed}");
    println!("punti letti dove dice l'intestazione:   {points_hdr}/{parsed}");
    println!("inizio dei punti usato (offset -> modelli): {point_starts:?}");
    println!("versioni: {versions:?}");
    println!("Radius dell'intestazione = raggio calcolato: {radius_ok} si', {radius_bad} no");
    for l in &bad_radius_list {
        println!("    {l}");
    }
    println!("facce con texture esistente: {faces_ok}/{faces_total}");

    println!("\nModelli delle mura (texture dominante):");
    for (id, _) in models.iter().filter(|m| m.1) {
        let Some(data) = arch.get_by_id(*id) else { continue };
        let Ok(m) = Arch3dModel::parse(*id, data) else { continue };
        match m.dominant_texture() {
            Some((a, r)) => {
                let (direct, swapped, after) = check(&mut cache, &dir, a as u32, r as u32);
                println!(
                    "  modello {id:6}: archivio {a:3} record {r:3}   esiste: {}   dopo il clima -> {swapped}: {}",
                    if direct { "si" } else { "NO" },
                    if after { "si" } else { "NO" }
                );
            }
            None => println!("  modello {id:6}: nessuna faccia"),
        }
    }
    Ok(())
}
