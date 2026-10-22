use std::collections::HashMap;
use crate::arch3d::Arch3dModel;

#[derive(Debug, Clone)]
pub struct Submesh {
    pub texture_archive: u16,
    pub texture_record: u8,
    pub indices: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct WorldMesh {
    pub vertices: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub submeshes: Vec<Submesh>,
}

impl WorldMesh {
    pub fn new() -> Self {
        Self {
            vertices: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            submeshes: Vec::new(),
        }
    }

    /// Istanzia un modello ARCH3D nel mondo con attributi vertici 1:1 e normali calcolate
    pub fn add_instance(
        &mut self,
        model: &Arch3dModel,
        pos_x: f32,
        pos_y: f32,
        pos_z: f32,
        rotation_y_deg: f32,
    ) {
        let rad = rotation_y_deg.to_radians();
        let cos_a = rad.cos();
        let sin_a = rad.sin();

        // Trasformazione preliminare dei vertici del modello nello spazio del chunk
        let world_verts: Vec<[f32; 3]> = model
        .vertices
        .iter()
        .map(|v| {
            let rx = v[0] * cos_a - v[2] * sin_a;
            let rz = v[0] * sin_a + v[2] * cos_a;
            [rx + pos_x, v[1] + pos_y, rz + pos_z]
        })
        .collect();

        let mut grouped: HashMap<(u16, u8), Vec<u32>> = HashMap::new();

        for face in &model.faces {
            let n_pts = face.vertex_indices.len();
            if n_pts < 3 {
                continue;
            }

            let key = (face.texture_archive, face.texture_record);
            let indices = grouped.entry(key).or_default();

            // Triangolazione a ventaglio con orientamento Counter-Clockwise per Bevy
            for k in 1..n_pts - 1 {
                let idx0 = face.vertex_indices[0] as usize;
                let idx1 = face.vertex_indices[k] as usize;
                let idx2 = face.vertex_indices[k + 1] as usize;

                if idx0 >= world_verts.len() || idx1 >= world_verts.len() || idx2 >= world_verts.len() {
                    continue;
                }

                let p0 = world_verts[idx0];
                let p1 = world_verts[idx1];
                let p2 = world_verts[idx2];

                // Calcolo normale geometrica della faccia (Cross product su triangolo CCW p0, p2, p1)
                let v01 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
                let v02 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
                let nx = v01[1] * v02[2] - v01[2] * v02[1];
                let ny = v01[2] * v02[0] - v01[0] * v02[2];
                let nz = v01[0] * v02[1] - v01[1] * v02[0];
                let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-5);
                let normal = [nx / len, ny / len, nz / len];

                let uv0 = face.uvs.get(0).copied().unwrap_or([0.0, 0.0]);
                let uv1 = face.uvs.get(k).copied().unwrap_or([1.0, 0.0]);
                let uv2 = face.uvs.get(k + 1).copied().unwrap_or([0.0, 1.0]);

                let start_idx = self.vertices.len() as u32;

                // Vertice 0
                self.vertices.push(p0);
                self.normals.push(normal);
                self.uvs.push(uv0);

                // Vertice 1 (p2 per CCW)
                self.vertices.push(p2);
                self.normals.push(normal);
                self.uvs.push(uv2);

                // Vertice 2 (p1 per CCW)
                self.vertices.push(p1);
                self.normals.push(normal);
                self.uvs.push(uv1);

                indices.push(start_idx);
                indices.push(start_idx + 1);
                indices.push(start_idx + 2);
            }
        }

        for ((archive, record), idxs) in grouped {
            if let Some(existing) = self.submeshes.iter_mut().find(|s| s.texture_archive == archive && s.texture_record == record) {
                existing.indices.extend(idxs);
            } else {
                self.submeshes.push(Submesh {
                    texture_archive: archive,
                    texture_record: record,
                    indices: idxs,
                });
            }
        }
    }

    pub fn export_obj(&self, obj_path: &str) -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::File::create(obj_path)?;

        writeln!(f, "# Exported Daggerfall World Mesh")?;
        writeln!(f, "# Vertices: {}, Submeshes: {}", self.vertices.len(), self.submeshes.len())?;

        for v in &self.vertices {
            writeln!(f, "v {:.4} {:.4} {:.4}", v[0], v[1], v[2])?;
        }

        for vn in &self.normals {
            writeln!(f, "vn {:.4} {:.4} {:.4}", vn[0], vn[1], vn[2])?;
        }

        for uv in &self.uvs {
            writeln!(f, "vt {:.4} {:.4}", uv[0], uv[1])?;
        }

        for sub in &self.submeshes {
            writeln!(f, "g tex_{}_{}", sub.texture_archive, sub.texture_record)?;
            writeln!(f, "usemtl tex_{}_{}", sub.texture_archive, sub.texture_record)?;

            for chunk in sub.indices.chunks_exact(3) {
                let i0 = chunk[0] + 1;
                let i1 = chunk[1] + 1;
                let i2 = chunk[2] + 1;
                writeln!(f, "f {}/{}/{} {}/{}/{} {}/{}/{}", i0, i0, i0, i1, i1, i1, i2, i2, i2)?;
            }
        }

        Ok(())
    }
}
