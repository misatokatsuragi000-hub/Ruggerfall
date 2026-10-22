//! Verifica del piazzamento degli edifici con gli ingombri LETTI CON IL NUOVO PARSER.
//! Per ogni combinazione di (Z dal bordo nord/sud) x (segno della rotazione) x (lati
//! scambiati o no) conta, su tutti i blocchi RMB tranne i WALL, quante coppie di edifici
//! si sovrappongono e quanti edifici escono dal blocco. La combinazione giusta ha quasi zero.
//! Uso: cargo run --release --bin dump_layout_check -- <ARENA2>

use std::collections::HashMap;
use std::path::PathBuf;

use daggerfall_data::arch3d::Arch3dModel;
use daggerfall_data::blocks::RmbBlock;
use daggerfall_data::bsa::{BsaArchive, BsaFile};
use daggerfall_data::building_gen::{Footprint, BLOCK_SIZE_M, DF_UNIT};
use daggerfall_data::find_file_ci;

type P = [f32; 2];

/// Rettangolo con centro e semi-lati, ruotato come `CityBuilder::add` (x' = x c + z s, z' = -x s + z c).
fn rect(center: P, half: P, rot_deg: f32, shrink: f32) -> [P; 4] {
    let (s, c) = rot_deg.to_radians().sin_cos();
    let (hx, hz) = (half[0] * shrink, half[1] * shrink);
    [(-hx, -hz), (hx, -hz), (hx, hz), (-hx, hz)].map(|(x, z)| [center[0] + x * c + z * s, center[1] - x * s + z * c])
}

fn overlap(a: &[P; 4], b: &[P; 4]) -> bool {
    for poly in [a, b] {
        for i in 0..4 {
            let (p, q) = (poly[i], poly[(i + 1) % 4]);
            let n = [q[1] - p[1], -(q[0] - p[0])];
            let proj = |r: &[P; 4]| {
                let v: Vec<f32> = r.iter().map(|k| k[0] * n[0] + k[1] * n[1]).collect();
                (v.iter().cloned().fold(f32::MAX, f32::min), v.iter().cloned().fold(f32::MIN, f32::max))
            };
            let ((a0, a1), (b0, b1)) = (proj(a), proj(b));
            if a1 <= b0 || b1 <= a0 {
                return false;
            }
        }
    }
    true
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::args().nth(1).map(PathBuf::from).expect("uso: dump_layout_check <ARENA2>");
    let blocks = BsaFile::open(find_file_ci(&dir, "BLOCKS.BSA").expect("BLOCKS.BSA"))?;
    let arch_buf = std::fs::read(find_file_ci(&dir, "ARCH3D.BSA").expect("ARCH3D.BSA"))?;
    let arch = BsaArchive::parse(&arch_buf)?;

    let mut fps: HashMap<u32, Option<Footprint>> = HashMap::new();
    let mut parsed: Vec<RmbBlock> = Vec::new();
    for rec in blocks.records() {
        let name = rec.name().to_ascii_uppercase();
        if !name.ends_with(".RMB") || name.starts_with("WALL") {
            continue;
        }
        if let Ok(b) = blocks.block(rec).map_err(|_| ()).and_then(|d| RmbBlock::parse(d).map_err(|_| ())) {
            parsed.push(b);
        }
    }

    // (sovrapposizioni, fuori dal blocco, edifici) per combinazione
    let mut rows: Vec<(usize, usize, usize, bool, f32, bool)> = Vec::new();
    for zflip in [true, false] {
        for rsign in [1.0f32, -1.0] {
            for swap in [false, true] {
                let (mut overlaps, mut outside, mut total) = (0usize, 0usize, 0usize);
                for b in &parsed {
                    let mut rects: Vec<[P; 4]> = Vec::new();
                    for o in &b.objects_3d {
                        let Some(i) = o.block else { continue };
                        let fp = *fps.entry(o.model_id).or_insert_with(|| {
                            arch.get_by_id(o.model_id)
                                .and_then(|r| Arch3dModel::parse(o.model_id, r).ok())
                                .and_then(|m| Footprint::from_model(&m))
                        });
                        let Some(mut fp) = fp else { continue };
                        // solo edifici veri: scarta pali, lampioni, insegne
                        if fp.width.min(fp.depth) < 3.0 || fp.height < 3.0 {
                            continue;
                        }
                        if swap {
                            fp = Footprint { width: fp.depth, depth: fp.width, ..fp };
                        }
                        let s = &b.subrecords[i as usize];
                        let n = if zflip { 4096.0 - s.z as f32 } else { s.z as f32 };
                        let (px, pz) = (s.x as f32 * DF_UNIT, -(n * DF_UNIT));
                        let rot = s.rotation_deg * rsign;
                        let (sn, cs) = rot.to_radians().sin_cos();
                        let center = [px + fp.cx * cs + fp.cz * sn, pz - fp.cx * sn + fp.cz * cs];
                        rects.push(rect(center, [fp.width * 0.5, fp.depth * 0.5], rot, 1.0));
                    }
                    total += rects.len();
                    for r in &rects {
                        let bad = r.iter().any(|p| p[0] < -1.0 || p[0] > BLOCK_SIZE_M + 1.0 || p[1] > 1.0 || p[1] < -BLOCK_SIZE_M - 1.0);
                        outside += bad as usize;
                    }
                    let shrunk: Vec<[P; 4]> = rects
                        .iter()
                        .map(|r| {
                            let c = [(r[0][0] + r[2][0]) * 0.5, (r[0][1] + r[2][1]) * 0.5];
                            r.map(|p| [c[0] + (p[0] - c[0]) * 0.95, c[1] + (p[1] - c[1]) * 0.95])
                        })
                        .collect();
                    for i in 0..shrunk.len() {
                        for j in i + 1..shrunk.len() {
                            overlaps += overlap(&shrunk[i], &shrunk[j]) as usize;
                        }
                    }
                }
                rows.push((overlaps, outside, total, zflip, rsign, swap));
            }
        }
    }
    println!("blocchi (non WALL): {}", parsed.len());
    rows.sort_by_key(|r| r.0 + r.1);
    println!("\n{:>12} {:>12} {:>9}   Z dal nord  rotazione  lati scambiati", "sovrapposiz.", "fuori blocco", "edifici");
    for (ov, out, tot, zflip, rsign, swap) in rows {
        println!("{ov:12} {out:12} {tot:9}   {:10}  {rsign:+9.0}  {swap}", zflip);
    }
    println!("\nLa riga in cima e' la combinazione migliore. Nel codice attuale: Z dal nord = true, rotazione +1, lati scambiati = false.");
    Ok(())
}
