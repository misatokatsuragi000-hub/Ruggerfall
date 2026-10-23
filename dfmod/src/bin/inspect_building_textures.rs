use std::path::PathBuf;
use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::arch3d::Arch3dModel;
use daggerfall_data::find_file_ci;
use std::collections::HashMap;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("uso: inspect_building_textures <ARENA2>"));
    let arch_buf = std::fs::read(find_file_ci(&dir, "ARCH3D.BSA").expect("ARCH3D.BSA mancante"))?;
    let arch = BsaArchive::parse(&arch_buf)?;

    // Modelli tipici di case e cottage: 709, 125, 200, 658, 43502
    for id in [709, 125, 200, 658, 43502] {
        if let Some(raw) = arch.get_by_id(id) {
            if let Ok(m) = Arch3dModel::parse(id, raw) {
                println!("\n=== Modello {} (punti: {}, facce: {}) ===", id, m.vertices.len(), m.faces.len());
                let mut tex_counts: HashMap<(u16, u8), (usize, f32, f32)> = HashMap::new(); // (count, avg_ny, total_area)
                for f in &m.faces {
                    if f.vertex_indices.len() < 3 { continue; }
                    let key = (f.texture_archive, f.texture_record);
                    let v0 = m.vertices[f.vertex_indices[0] as usize];
                    let v1 = m.vertices[f.vertex_indices[1] as usize];
                    let v2 = m.vertices[f.vertex_indices[2] as usize];
                    let e1 = [v1[0]-v0[0], v1[1]-v0[1], v1[2]-v0[2]];
                    let e2 = [v2[0]-v0[0], v2[1]-v0[1], v2[2]-v0[2]];
                    let ny = e1[2]*e2[0] - e1[0]*e2[2]; // componente Y della normale grezza
                    let area = 0.5 * ((e1[1]*e2[2]-e1[2]*e2[1]).powi(2) + ny.powi(2) + (e1[0]*e2[1]-e1[1]*e2[0]).powi(2)).sqrt();
                    let entry = tex_counts.entry(key).or_insert((0, 0.0, 0.0));
                    entry.0 += 1;
                    entry.1 += ny;
                    entry.2 += area;
                }
                for ((archive, record), (count, avg_ny, area)) in tex_counts {
                    println!("   Texture (archivio: {:3}, record: {:2}) -> facce: {:3}, area: {:6.1} m², inclinazione Y: {:+.2}",
                        archive, record, count, area, if area > 0.0 { avg_ny / count as f32 } else { 0.0 });
                }
            }
        }
    }
    Ok(())
}
