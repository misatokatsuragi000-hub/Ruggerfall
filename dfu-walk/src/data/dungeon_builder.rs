//! Assemblatore della geometria 3D, collisioni e porte apribili di un dungeon RDB.
//!
//! ## Riconoscimento delle porte (porting di `RDBLayout.IsActionDoor` di DFU)
//!
//! In Daggerfall una porta è un modello RDB la cui `Description` nel
//! `ModelReferenceList` è una di: `"DOR"`, `"DDR"`, `"NEW"`, `"CAV"`. Il modello
//! `72100` (redBrick) ha `"DOR"` ma è solo decorativo e va escluso.
//!
//! ## Angolo di apertura
//!
//! DFU usa `OpenAngle = -90°` fisso, perché i modelli delle porte sono orientati
//! in modo che la cerniera stia all'origine e il pannello si estenda sempre dallo
//! stesso lato. Nel nostro codice la mesh locale ha i vertici originali, quindi
//! per le porte con cerniera "dall'altra parte" (tipiche delle doppie ante `DDR`)
//! il verso va invertito. `compute_open_angle` guarda il baricentro del modello
//! rispetto alla cerniera e restituisce `-90°` o `+90°` di conseguenza.
//!
//! ## Nemici
//!
//! I nemici nei dungeon non sono "oggetti" del blocco: sono i flat dell'archivio
//! 199 (editor marker) con `FactionID != 0`. Il byte basso del faction id è il
//! `MobileID` del mostro (0..=42 per i mostri, 128..=146 per gli umanoidi). Il
//! parser estrae questi flat in `BuiltDungeon::enemies`, senza scartarli.

use std::collections::HashMap;
use bevy::prelude::{Quat, Vec3};
use super::arch3d::Arch3dModel;
use super::bsa::BsaArchive;
use super::dungeon_layout::DungeonData;
use super::mesh_builder::WorldMesh;
use super::rdb::{RdbBlock, RdbResource, RDB_SIDE_METERS};

pub type Tri = [[f32; 3]; 3];

/// Modello "redBrick door": ha `desc == "DOR"` ma non è una porta apribile
/// (fonte: `RDBLayout.cs` di DFU).
const RED_BRICK_DOOR_MODEL_ID: u32 = 72100;

/// Altezza massima di un ostacolo che il giocatore può superare camminando.
/// Tenere allineato a `STEP_UP` in `dungeon.rs`.
pub const STEP_HEIGHT: f32 = 0.8;

/// Riconosce una porta in stile DFU: descrizione del ModelReferenceList.
fn is_dungeon_door(desc: &str, model_id: u32) -> bool {
    if model_id == RED_BRICK_DOOR_MODEL_ID {
        return false;
    }
    matches!(desc, "DOR" | "DDR" | "NEW" | "CAV")
}

/// Calcola l'angolo di apertura in base a dove si trova il pannello rispetto
/// alla cerniera (= origine del modello). Se il pannello si estende in +X
/// (baricentro X > 0), il default `-90°` di DFU lo fa aprire in un verso;
/// se si estende in -X, il verso va invertito (`+90°`). Stessa logica per Z
/// per le porte "lunghe in Z".
fn compute_open_angle(m: &Arch3dModel) -> f32 {
    if m.vertices.is_empty() {
        return -90.0;
    }
    let mut mn = [f32::MAX; 3];
    let mut mx = [f32::MIN; 3];
    let mut sum = Vec3::ZERO;
    for v in &m.vertices {
        let p = Vec3::from(*v);
        sum += p;
        for k in 0..3 {
            mn[k] = mn[k].min(p[k]);
            mx[k] = mx[k].max(p[k]);
        }
    }
    let centroid = sum / m.vertices.len() as f32;
    let (dx, dz) = (mx[0] - mn[0], mx[2] - mn[2]);

    if dx >= dz {
        if centroid.x.abs() > 0.05 {
            if centroid.x > 0.0 { -90.0 } else { 90.0 }
        } else {
            -90.0
        }
    } else {
        if centroid.z.abs() > 0.05 {
            if centroid.z > 0.0 { -90.0 } else { 90.0 }
        } else {
            -90.0
        }
    }
}

/// Bounds verticali locali in metri (già scalati da `Arch3dModel::parse`).
fn local_y_range(m: &Arch3dModel) -> (f32, f32) {
    let mut mn = f32::MAX;
    let mut mx = f32::MIN;
    for v in &m.vertices {
        mn = mn.min(v[1]);
        mx = mx.max(v[1]);
    }
    if mn > mx { (0.0, 2.4) } else { (mn, mx) }
}

#[derive(Debug, Clone)]
pub struct DungeonDoor {
    pub model_id: u32,
    /// Mesh in spazio locale (cerniera = origine del modello).
    pub mesh: WorldMesh,
    /// Cerniera in coordinate mondo del dungeon.
    pub pos: [f32; 3],
    /// Rotazione a porta chiusa (gradi).
    pub closed_yaw: f32,
    /// Centro geometrico in coordinate mondo, per il toggle E e la collisione.
    pub center: [f32; 3],
    /// Angolo di apertura in gradi: `-90°` di default DFU, `+90°` per le porte
    /// con cerniera "dall'altra parte".
    pub open_angle_deg: f32,
}

/// Nemico da generare a runtime: posizione locale del dungeon + MobileID del
/// mostro. La posizione è nello stesso sistema delle città: `x` est, `y` su,
/// `z` = -nord (come tutto il resto del progetto).
#[derive(Debug, Clone)]
pub struct DungeonEnemySpawn {
    pub pos: [f32; 3],
    pub mobile_id: u8,
}

/// Mappa MobileID → (archivio, record) della texture del mostro.
///
/// ATTENZIONE: questa è una tabella "best effort" basata sugli archivi che il
/// progetto già usa in `tiles.rs::ENEMY_SPECIES`. I valori esatti DFU stanno in
/// `EnemyBasics.cs::GetMobileTextureArchive`. Se vedi un topo al posto di uno
/// scheletro (o viceversa), correggi il valore qui.
fn mobile_texture(mobile_id: u8) -> Option<(u16, u8)> {
    let entry = match mobile_id {
        // Mostri comuni (0..=42)
        0 => (257, 0),   // Rat
        1 => (259, 0),   // Giant Rat
        2 => (260, 0),   // Imp
        3 => (261, 0),   // Spriggan
        4 => (262, 0),   // Giant Bat
        5 => (263, 0),   // Gargoyle
        6 => (265, 0),   // Ice Atronach
        7 => (267, 0),   // Orc
        8 => (268, 0),   // Skeletal Warrior
        9 => (271, 0),   // Ghost
        10 => (275, 0),  // Mummy
        11 => (276, 0),  // Giant Scorpion
        12 => (265, 0),  // Spider
        13 => (271, 0),  // Wraith
        14 => (265, 0),  // Fire Atronach
        15 => (267, 0),  // Zombie
        16 => (259, 0),  // Centaur
        17 => (262, 0),  // Harpy
        18 => (271, 0),  // Lich
        19 => (268, 0),  // Werewolf
        20 => (259, 0),  // Nymph
        21 => (267, 0),  // Slaughterfish
        22 => (267, 0),  // Orc Sergeant
        23 => (265, 0),  // Giant Spider
        24 => (265, 0),  // Iron Atronach
        25 => (263, 0),  // Giant
        26 => (271, 0),  // Daedra Lord
        27 => (262, 0),  // Dragonling
        // Umanoidi (128..=146): usano le persone
        128..=146 => (334, mobile_id.saturating_sub(128)),
        _ => return None,
    };
    Some(entry)
}

/// Espone la tabella al resto del crate.
pub fn mobile_texture_public(mobile_id: u8) -> Option<(u16, u8)> {
    mobile_texture(mobile_id)
}

pub struct BuiltDungeon {
    pub mesh: WorldMesh,
    pub spawn_pos: [f32; 3],
    pub exit_pos: [f32; 3],
    pub lights: Vec<([f32; 3], f32)>,
    pub flats: Vec<([f32; 3], u16, u8)>,
    pub doors: Vec<DungeonDoor>,
    pub door_open: Vec<bool>,
    pub floors: Vec<Tri>,
    pub walls: Vec<Tri>,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// Nemici individuati dai flat editor 199 con FactionID != 0.
    pub enemies: Vec<DungeonEnemySpawn>,
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
            if ymax <= feet_y + STEP_HEIGHT { continue; }
            if ymax < lo || ymin > hi { continue; }
            for k in 0..3 {
                if dist_segment_xz(x, z, t[k], t[(k + 1) % 3]) < radius {
                    return true;
                }
            }
        }
        for (d, &open) in self.doors.iter().zip(&self.door_open) {
            if open { continue; }
            if (d.center[1] - feet_y).abs() > 2.2 { continue; }
            let dist_xz = (d.center[0] - x).hypot(d.center[2] - z);
            if dist_xz < radius + 0.45 {
                return true;
            }
        }
        false
    }

    pub fn toggle_nearest_door(
        &mut self,
        x: f32,
        z: f32,
        feet_y: f32,
        reach: f32,
    ) -> Option<usize> {
        let mut best: Option<(f32, usize)> = None;
        for (i, d) in self.doors.iter().enumerate() {
            if (d.center[1] - feet_y).abs() > 2.5 { continue; }
            let dist = (d.center[0] - x).hypot(d.center[2] - z);
            if dist <= reach && best.map_or(true, |(b, _)| dist < b) {
                best = Some((dist, i));
            }
        }
        let (_, i) = best?;
        self.door_open[i] = !self.door_open[i];
        Some(i)
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
    let mut enemies: Vec<DungeonEnemySpawn> = Vec::new();
    let mut doors = Vec::new();

    let mut enter_marker: Option<[f32; 3]> = None;
    let mut exit_marker: Option<[f32; 3]> = None;

    let mut desc_counts: HashMap<String, (usize, u32)> = HashMap::new();

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

                    let desc = rdb
                    .model_refs
                    .get(m_res.model_index as usize)
                    .map(|r| r.description.as_str())
                    .unwrap_or("");
                    let entry = desc_counts
                    .entry(desc.to_string())
                    .or_insert((0, model_id));
                    entry.0 += 1;

                    let model = model_cache.entry(model_id).or_insert_with(|| {
                        arch3d_bsa
                        .get_by_id(model_id)
                        .and_then(|r| Arch3dModel::parse(model_id, r).ok())
                    });
                    let Some(m) = model.as_ref() else { continue };

                    let rot = Quat::from_rotation_z(m_res.z_rot_deg.to_radians())
                    * Quat::from_rotation_x(m_res.x_rot_deg.to_radians())
                    * Quat::from_rotation_y(m_res.y_rot_deg.to_radians());

                    if is_dungeon_door(desc, model_id) {
                        let open_angle = compute_open_angle(m);
                        let (mn_y, mx_y) = local_y_range(m);
                        let center_y = oy + (mn_y + mx_y) * 0.5;

                        let mut door_mesh = WorldMesh::new();
                        door_mesh.add_instance(m, 0.0, 0.0, 0.0, 0.0);

                        let mut sum = Vec3::ZERO;
                        for v in &m.vertices { sum += Vec3::from(*v); }
                        let centroid = sum / m.vertices.len().max(1) as f32;

                        println!(
                            "Porta: desc={:?} id={} pos={:?} closed_yaw={:.1} open_angle={:.1} baricentro_locale=({:.2},{:.2},{:.2})",
                                 desc, model_id, [ox, oy, oz], m_res.y_rot_deg, open_angle,
                                 centroid.x, centroid.y, centroid.z,
                        );

                        doors.push(DungeonDoor {
                            model_id,
                            mesh: door_mesh,
                            pos: [ox, oy, oz],
                            closed_yaw: m_res.y_rot_deg,
                            center: [ox, center_y, oz],
                            open_angle_deg: open_angle,
                        });
                    } else {
                        add_dungeon_instance(&mut mesh, m, [ox, oy, oz], rot);
                    }
                }
                RdbResource::Flat(f_res) => {
                    if f_res.archive == 199 {
                        // Editor markers: ingressi, uscite e spawn point dei nemici.
                        if (f_res.record == 8 || f_res.record == 10) && b_info.is_starting_block {
                            if enter_marker.is_none() || f_res.record == 8 {
                                enter_marker = Some([ox, oy, oz]);
                            }
                        } else if f_res.faction_or_mobile_id != 0 {
                            // Marker di spawn nemico: il byte basso del faction id
                            // è il MobileID del mostro.
                            let mobile_id = (f_res.faction_or_mobile_id & 0xFF) as u8;
                            enemies.push(DungeonEnemySpawn {
                                pos: [ox, oy, oz],
                                mobile_id,
                            });
                        }
                    } else {
                        flats.push(([ox, oy, oz], f_res.archive, f_res.record));
                    }
                }
                RdbResource::Light(l_res) => {
                    let radius_m = (l_res.radius as f32 * 0.025).max(7.0);
                    lights.push(([ox, oy, oz], radius_m));
                }
                RdbResource::None => {}
            }
        }
    }

    if mesh.vertices.is_empty() && doors.is_empty() {
        return None;
    }

    if !desc_counts.is_empty() {
        println!("Dungeon: descrizioni modello viste (desc: usi, id esempio):");
        let mut all: Vec<_> = desc_counts.into_iter().collect();
        all.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(&b.0)));
        for (desc, (count, example_id)) in &all {
            let marker = if is_dungeon_door(desc, *example_id) { "  <-- PORTA" } else { "" };
            println!("    desc={desc:?} usi={count} id_esempio={example_id}{marker}");
        }
    }
    println!("Porte riconosciute: {}", doors.len());
    println!("Nemici trovati: {}", enemies.len());

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
            let ny = n[1] / len;
            if ny > 0.15 {
                floors.push(t);
            } else if ny.abs() <= 0.15 {
                walls.push(t);
            }
        }
    }

    // Rimuove dalla collisione i triangoli dei muri che ostruiscono il passaggio
    // attraverso una porta.
    if !doors.is_empty() {
        const DOOR_CLEAR_RADIUS: f32 = 1.2;
        const DOOR_CLEAR_DOWN: f32 = 0.7;
        const DOOR_CLEAR_UP: f32 = 0.6;

        let before = walls.len();
        walls.retain(|t| {
            let e1 = [t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]];
            let e2 = [t[2][0] - t[0][0], t[2][1] - t[0][1], t[2][2] - t[0][2]];
            let n = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-5);
            if (n[1] / len).abs() > 0.5 {
                return true;
            }
            let cx = (t[0][0] + t[1][0] + t[2][0]) / 3.0;
            let cy = (t[0][1] + t[1][1] + t[2][1]) / 3.0;
            let cz = (t[0][2] + t[1][2] + t[2][2]) / 3.0;
            for d in &doors {
                let dx = cx - d.center[0];
                let dz = cz - d.center[2];
                if (dx * dx + dz * dz).sqrt() < DOOR_CLEAR_RADIUS
                    && cy >= d.center[1] - DOOR_CLEAR_DOWN
                    && cy <= d.center[1] + DOOR_CLEAR_UP
                    {
                        return false;
                    }
            }
            true
        });
        let removed = before - walls.len();
        if removed > 0 {
            println!(
                "Rimossi {} triangoli muri che ostruivano il passaggio delle porte",
                removed
            );
        }
    }

    let spawn_pos = enter_marker.unwrap_or([
        (mn[0] + mx[0]) * 0.5,
                                           mn[1] + 1.0,
                                           (mn[2] + mx[2]) * 0.5,
    ]);

    let exit_pos = exit_marker.unwrap_or(spawn_pos);
    let door_count = doors.len();

    Some(BuiltDungeon {
        mesh,
         spawn_pos,
         exit_pos,
         lights,
         flats,
         doors,
         door_open: vec![false; door_count],
         floors,
         walls,
         bounds_min: mn,
         bounds_max: mx,
         enemies,
    })
}

fn add_dungeon_instance(
    world_mesh: &mut WorldMesh,
    model: &Arch3dModel,
    pos: [f32; 3],
    rot: Quat,
) {
    let p_offset = Vec3::from(pos);
    let world_verts: Vec<[f32; 3]> = model
    .vertices
    .iter()
    .map(|v| (rot.mul_vec3(Vec3::from(*v)) + p_offset).to_array())
    .collect();

    let mut grouped: HashMap<(u16, u8), Vec<u32>> = HashMap::new();

    for face in &model.faces {
        let n_pts = face.vertex_indices.len();
        if n_pts < 3 { continue; }

        let key = (face.texture_archive, face.texture_record);
        let indices = grouped.entry(key).or_default();

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

            let v01 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
            let v02 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
            let nx = v01[1] * v02[2] - v01[2] * v02[1];
            let ny = v01[2] * v02[0] - v01[0] * v02[2];
            let nz = v01[0] * v02[1] - v01[1] * v02[0];
            let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-5);
            let normal = [nx / len, ny / len, nz / len];

            let uv0 = face.uvs.first().copied().unwrap_or([0.0, 0.0]);
            let uv1 = face.uvs.get(k).copied().unwrap_or([1.0, 0.0]);
            let uv2 = face.uvs.get(k + 1).copied().unwrap_or([0.0, 1.0]);

            let start_idx = world_mesh.vertices.len() as u32;

            world_mesh.vertices.push(p0);
            world_mesh.normals.push(normal);
            world_mesh.uvs.push(uv0);

            world_mesh.vertices.push(p2);
            world_mesh.normals.push(normal);
            world_mesh.uvs.push(uv2);

            world_mesh.vertices.push(p1);
            world_mesh.normals.push(normal);
            world_mesh.uvs.push(uv1);

            indices.push(start_idx);
            indices.push(start_idx + 1);
            indices.push(start_idx + 2);
        }
    }

    for ((archive, record), idxs) in grouped {
        if let Some(existing) = world_mesh
            .submeshes
            .iter_mut()
            .find(|s| s.texture_archive == archive && s.texture_record == record)
            {
                existing.indices.extend(idxs);
            } else {
                world_mesh.submeshes.push(super::mesh_builder::Submesh {
                    texture_archive: archive,
                    texture_record: record,
                    indices: idxs,
                });
            }
    }
}
