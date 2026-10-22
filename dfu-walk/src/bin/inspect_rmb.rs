use std::env;
use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::arch3d::Arch3dModel;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arena2_dir = env::args()
    .nth(1)
    .map(PathBuf::from)
    .unwrap_or_else(|| PathBuf::from("/mnt/1CE16DBE48849731/Daggerfall/arena2"));

    let blocks_path = arena2_dir.join("BLOCKS.BSA");
    let mut blocks_data = Vec::new();
    File::open(&blocks_path)?.read_to_end(&mut blocks_data)?;
    let blocks_bsa = BsaArchive::parse(&blocks_data)?;

    let arch_path = arena2_dir.join("ARCH3D.BSA");
    let mut arch_data = Vec::new();
    File::open(&arch_path)?.read_to_end(&mut arch_data)?;
    let arch_bsa = BsaArchive::parse(&arch_data)?;

    // Esportiamo la taverna TVRNAL06.RMB
    println!("=== Esportazione Modello Taverna 43049 in .OBJ ===");
    if let Some(model_raw) = arch_bsa.get_by_id(43049) {
        let model = Arch3dModel::parse(43049, model_raw)?;
        let obj_name = "taverna_43049.obj";
        let mut f = File::create(obj_name)?;

        writeln!(f, "# Daggerfall Exterior Building 43049")?;
        for v in &model.vertices {
            writeln!(f, "v {:.4} {:.4} {:.4}", v[0], v[1], v[2])?;
        }
        for face in &model.faces {
            for uv in &face.uvs {
                writeln!(f, "vt {:.4} {:.4}", uv[0], uv[1])?;
            }
        }

        let mut uv_off = 1;
        for face in &model.faces {
            for k in 1..face.vertex_indices.len().saturating_sub(1) {
                let v1 = face.vertex_indices[0] + 1;
                let v2 = face.vertex_indices[k] + 1;
                let v3 = face.vertex_indices[k + 1] + 1;
                let vt1 = uv_off;
                let vt2 = uv_off + k;
                let vt3 = uv_off + k + 1;
                writeln!(f, "f {}/{} {}/{} {}/{}", v1, vt1, v2, vt2, v3, vt3)?;
            }
            uv_off += face.vertex_indices.len();
        }
        println!("File salvato: {}", obj_name);
    }

    Ok(())
}
