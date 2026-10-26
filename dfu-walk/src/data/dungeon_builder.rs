//! Assemblatore della geometria 3D, collisioni e marcatori di un dungeon RDB.

use std::collections::HashMap;
use super::arch3d::Arch3dModel;
use super::bsa::BsaArchive;
use super::dungeon_layout::DungeonData;
use super::mesh_builder::WorldMesh;
use super::rdb::{RdbBlock, RdbResource, RDB_SIDE_METERS};

pub type Tri = [[f32; 3]; 3];

#[derive(Debug, Clone)]
pub struct DungeonDoor {
    pub model_id: u32,
    pub mesh: WorldMesh,
    pub pos: [f32; 3],
    pub yaw_deg: f32,
    pub center: [f32; 3],
}

pub struct BuiltDungeon {
    pub mesh: WorldMesh,
    pub spawn_pos: [f32; 3],
    pub exit_pos: [f32; 3],
    pub lights: Vec<([f32; 3], f32)>,
    pub flats: Vec<([f32; 3], u16, u8)>,
    pub doors: Vec<DungeonDoor>,
    pub floors: Vec<Tri>,
    pub walls: Vec<Tri>,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
}

fn plane_y(t: &Tri, x: f32, z: f32) -> Option<f32> {
    let (a, b, c) = (t[0], t[1], t[2]);
    let d = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
    if d.abs() < 1e-9 { return None; }
    let l1 = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / d;
    let l2 = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / d;
    let l3 = 1.0 - l1 - l2;
    const EPS: f32 = -1e-4;
    if l1 < EPS || l2 < EPS || l3 < EPS { return None; }
    Some(l1 * a[1] + l2 * b[1] + l3 * c[1])
}

fn dist_segment_xz(x: f32, z: f32, a: [f32; 3], b: [f32; 3]) -> f32 {
    let (abx, abz) = (b[0] - a[0], b[2] - a[2]);
    let (apx, apz) = (x - a[0], z - a[2]);
    let len2 = abx * abx + abz * abz;
    let t = if len2 < 1e-9 { 0.0 } else { ((apx * abx + apz * abz) / len2).clamp(0.0, 1.0) };
    let (cx, cz) = (a[0] + abx * t, a[2] + abz * t);
    ((x - cx).powi(2) + (z - cz).powi(2)).sqrt()
}

impl BuiltDungeon {
    pub fn floor_at(&self, x: f32, z: f32, ref_y: f32, step: f32) -> Option<f32> {
        let mut best: Option<f32> = None;
        for t in &self.floors {
            if let Some(y) = plane_y(t, x, z) {
                if y <= ref_y + step && best.map_or(true, |b| y > b) {
                    best = Some(y);
                }
            }
        }
        best
    }

    pub fn blocked(&self, x: f32, z: f32, feet_y: f32, radius: f32) -> bool {
        let (lo, hi) = (feet_y + 0.45, feet_y + 1.8);
        for t in &self.walls {
            let ymin = t[0][1].min(t[1][1]).min(t[2][1]);
            let ymax = t[0][1].max(t[1][1]).max(t[2][1]);
            if ymax < lo || ymin > hi { continue; }
            for k in 0..3 {
                if dist_segment_xz(x, z, t[k], t[(k + 1) % 3]) < radius {
                    return true;
                }
            }
        }
        false
    }
}

pub fn build_dungeon(
    dungeon: &DungeonData,
    blocks_bsa: &BsaArchive,
    arch3d_bsa: &BsaArchive,
) -> Option<BuiltDungeon> {
    let mut mesh = WorldMesh::new();
    let mut model_cache: HashMap<u32, Option<Arch3dModel>> = HashMap::new();
    let mut lights = Vec::new();
    let mut flats = Vec::new();
    let doors = Vec::new();

    let mut enter_marker: Option<[f32; 3]> = None;
    let mut exit_marker: Option<[f32; 3]> = None;

    for b_info in &dungeon.blocks {
        let Some(raw_rdb) = blocks_bsa.get_by_name(&b_info.block_name) else { continue };
        let Ok(rdb) = RdbBlock::parse(&b_info.block_name, raw_rdb) else { continue };

        let bx_offset = b_info.x as f32 * RDB_SIDE_METERS;
        let bz_offset = b_info.z as f32 * RDB_SIDE_METERS;

        for obj in &rdb.objects {
            let ox = bx_offset + (obj.x as f32 * 0.025);
            let oy = -(obj.y as f32 * 0.025);
            let oz = bz_offset + (obj.z as f32 * 0.025);

            match &obj.resource {
                RdbResource::Model(m_res) => {
                    let model_id = m_res.model_id;
                    if model_id == 0 { continue; }

                    if model_id == 70300 && b_info.is_starting_block {
                        exit_marker = Some([ox, oy, oz]);
                    }

                    let model = model_cache.entry(model_id).or_insert_with(|| {
                        arch3d_bsa.get_by_id(model_id).and_then(|r| Arch3dModel::parse(model_id, r).ok())
                    });

                    if let Some(m) = model.as_ref() {
                        mesh.add_instance(m, ox, oy, oz, m_res.y_rot_deg);
                    }
                }
                RdbResource::Flat(f_res) => {
                    if f_res.archive == 199 {
                        // Record 8 = Enter Marker, Record 10 = Start Marker
                        if (f_res.record == 8 || f_res.record == 10) && b_info.is_starting_block {
                            if enter_marker.is_none() || f_res.record == 8 {
                                enter_marker = Some([ox, oy, oz]);
                            }
                        }
                    } else {
                        flats.push(([ox, oy, oz], f_res.archive, f_res.record));
                    }
                }
                RdbResource::Light(l_res) => {
                    let radius_m = (l_res.radius as f32 * 0.025).max(6.0);
                    lights.push(([ox, oy, oz], radius_m));
                }
                RdbResource::None => {}
            }
        }
    }

    if mesh.vertices.is_empty() {
        return None;
    }

    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    for v in &mesh.vertices {
        for k in 0..3 {
            mn[k] = mn[k].min(v[k]);
            mx[k] = mx[k].max(v[k]);
        }
    }

    let mut floors = Vec::new();
    let mut walls = Vec::new();
    for sm in &mesh.submeshes {
        for ch in sm.indices.chunks_exact(3) {
            let t: Tri = [
                mesh.vertices[ch[0] as usize],
                mesh.vertices[ch[1] as usize],
                mesh.vertices[ch[2] as usize],
            ];
            let e1 = [t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]];
            let e2 = [t[2][0] - t[0][0], t[2][1] - t[0][1], t[2][2] - t[0][2]];
            let n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len < 1e-6 { continue; }
            if n[1] / len > 0.5 {
                floors.push(t);
            } else {
                walls.push(t);
            }
        }
    }

    let spawn_pos = enter_marker.unwrap_or([
        (mn[0] + mx[0]) * 0.5,
                                           mn[1] + 1.0,
                                           (mn[2] + mx[2]) * 0.5,
    ]);

    let exit_pos = exit_marker.unwrap_or(spawn_pos);

    Some(BuiltDungeon {
        mesh,
         spawn_pos,
         exit_pos,
         lights,
         flats,
         doors,
         floors,
         walls,
         bounds_min: mn,
         bounds_max: mx,
    })
}
