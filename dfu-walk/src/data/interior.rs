//! Interni degli edifici (taverne, gilde, negozi): modelli e flat del secondo BlockData di ogni sotto-record
//! RMB, piu' la geometria usata per far camminare il giocatore (pavimenti e pareti).
//!
//! Sistema locale dell'interno: metri, x est, y su, z come nelle coordinate grezze Daggerfall
//! (la stessa convenzione delle citta': `z_mondo = -nord`).

use std::collections::HashMap;
use super::arch3d::Arch3dModel;
use super::blocks::RmbBlock;
use super::bsa::BsaArchive;
use super::building_gen::DF_UNIT;
use super::mesh_builder::WorldMesh;

/// Archivio dei flat "editor" (marcatori di quest, nemici...): non si disegnano.
const EDITOR_FLATS_ARCHIVE: u16 = 199;

/// Marcatori di ingresso fra i flat "editor" (archivio 199), come in Daggerfall Unity
/// (`InteriorMarkerTypes`): Enter = 8. Il 4 (Rest) e' accettato come ripiego perche' a volte i
/// disegnatori l'hanno usato al posto dell'8.
const ENTER_MARKER_RECORDS: [u8; 2] = [8, 4];

/// Un marcatore piu' lontano di cosi' dalla porta d'ingresso e' quasi certamente fuori posto.
const MAX_MARKER_DISTANCE: f32 = 12.0;

/// Tipo di oggetto 3D che in Daggerfall Unity indica arredi e props.
const PROP_OBJECT_TYPE: u8 = 3;

/// Se true, gli arredi (tipo 3) si posizionano come in Daggerfall Unity: Y non negata e modello
/// appoggiato sul suo punto piu' basso.
const PROPS_LIKE_DFU: bool = true;

pub type Tri = [[f32; 3]; 3];

/// Porta di una stanza: modello separato dal resto, che ruota attorno alla sua origine (la cerniera).
#[derive(Debug, Clone)]
pub struct InteriorDoor {
    pub model_id: u32,
    pub mesh: WorldMesh,
    pub pos: [f32; 3],
    pub yaw_deg: f32,
    pub center: [f32; 3],
    tris: Vec<Tri>,
}

/// Riconosce i veri pannelli porta: battente sottile (max 18 cm, esclude categoricamente gli armadi),
/// altezza da porta e texture porta (archivio 74).
fn looks_like_door(m: &Arch3dModel) -> bool {
    // Un battente porta ha poche facce (max 12), gli armadi ne hanno molte di più
    if m.faces.is_empty() || m.faces.len() > 12 {
        return false;
    }
    if !m.faces.iter().any(|f| f.texture_archive % 100 == 74) {
        return false;
    }
    let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in &m.vertices {
        for k in 0..3 {
            mn[k] = mn[k].min(v[k]);
            mx[k] = mx[k].max(v[k]);
        }
    }
    let (dx, dy, dz) = (mx[0] - mn[0], mx[1] - mn[1], mx[2] - mn[2]);
    let (w, t) = (dx.max(dz), dx.min(dz));

    // Lo spessore t deve essere <= 18 cm: un armadio è profondo 45-60 cm e viene escluso!
    (0.6..=1.8).contains(&w) && (1.8..=3.0).contains(&dy) && t <= 0.18
}

/// Faccia con texture di porta (archivio 74) di un modello dell'interno, in coordinate della scena.
#[derive(Debug, Clone, Copy)]
struct DoorFace {
    center: [f32; 3],
    /// Normale orizzontale (y = 0), senza un verso affidabile: serve solo come asse.
    normal: [f32; 3],
    /// Quota del bordo inferiore.
    bottom: f32,
}

/// Raccoglie le facce-porta di un modello posizionato come `WorldMesh::add_instance`.
/// In Daggerfall Unity le porte di un interno sono proprio queste facce (`ModelData.Doors`).
fn collect_door_faces(m: &Arch3dModel, px: f32, py: f32, pz: f32, rot_deg: f32, out: &mut Vec<DoorFace>) {
    let (sin_a, cos_a) = rot_deg.to_radians().sin_cos();
    let place = |v: [f32; 3]| -> [f32; 3] {
        [v[0] * cos_a - v[2] * sin_a + px, v[1] + py, v[0] * sin_a + v[2] * cos_a + pz]
    };
    for f in &m.faces {
        if f.texture_archive % 100 != 74 || f.vertex_indices.len() < 3 {
            continue;
        }
        let pts: Vec<[f32; 3]> = f
            .vertex_indices
            .iter()
            .filter_map(|&i| m.vertices.get(i as usize).copied())
            .map(place)
            .collect();
        if pts.len() < 3 {
            continue;
        }
        let n = pts.len() as f32;
        let center = [
            pts.iter().map(|p| p[0]).sum::<f32>() / n,
            pts.iter().map(|p| p[1]).sum::<f32>() / n,
            pts.iter().map(|p| p[2]).sum::<f32>() / n,
        ];
        let bottom = pts.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
        let top = pts.iter().map(|p| p[1]).fold(f32::MIN, f32::max);
        // altezza da porta
        if !(1.5..=3.5).contains(&(top - bottom)) {
            continue;
        }
        let e1 = [pts[1][0] - pts[0][0], pts[1][1] - pts[0][1], pts[1][2] - pts[0][2]];
        let e2 = [pts[2][0] - pts[0][0], pts[2][1] - pts[0][1], pts[2][2] - pts[0][2]];
        let cross = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let len = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
        let horizontal = (cross[0] * cross[0] + cross[2] * cross[2]).sqrt();
        // la porta deve essere verticale
        if len < 1e-6 || horizontal / len < 0.7 {
            continue;
        }
        // lo stesso battente puo' comparire due volte (due facce): si tiene una sola
        if out.iter().any(|d| {
            (d.center[0] - center[0]).hypot(d.center[2] - center[2]) < 0.3 && (d.center[1] - center[1]).abs() < 0.5
        }) {
            continue;
        }
        out.push(DoorFace { center, normal: [cross[0] / horizontal, 0.0, cross[2] / horizontal], bottom });
    }
}

/// La porta d'ingresso: quella piu' vicina al livello del suolo e, fra queste, la piu' lontana
/// dal centro dell'edificio (`DaggerfallInterior.FindLowestOuterInteriorDoor` di DFU). Le porte
/// delle stanze interne stanno piu' al centro, quella d'ingresso sul perimetro.
fn pick_entrance_door(doors: &[DoorFace], center_xz: [f32; 2]) -> Option<usize> {
    let lowest = doors.iter().map(|d| d.bottom).fold(f32::MAX, f32::min);
    if doors.is_empty() {
        return None;
    }
    let dist2 = |d: &DoorFace| (d.center[0] - center_xz[0]).powi(2) + (d.center[2] - center_xz[1]).powi(2);
    doors
        .iter()
        .enumerate()
        .filter(|(_, d)| d.bottom <= lowest + 1.0)
        .max_by(|a, b| dist2(a.1).partial_cmp(&dist2(b.1)).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
}

/// Il marcatore piu' vicino a `to` entro `max_dist` (distanza 3D, come in Daggerfall Unity).
fn closest_marker(markers: &[[f32; 3]], to: [f32; 3], max_dist: f32) -> Option<[f32; 3]> {
    let dist = |m: &[f32; 3]| ((m[0] - to[0]).powi(2) + (m[1] - to[1]).powi(2) + (m[2] - to[2]).powi(2)).sqrt();
    markers
        .iter()
        .copied()
        .filter(|m| dist(m) <= max_dist)
        .min_by(|a, b| dist(a).partial_cmp(&dist(b)).unwrap_or(std::cmp::Ordering::Equal))
}

fn mesh_tris(mesh: &WorldMesh) -> Vec<Tri> {
    let mut out = Vec::new();
    for sm in &mesh.submeshes {
        for ch in sm.indices.chunks_exact(3) {
            out.push([
                mesh.vertices[ch[0] as usize],
                mesh.vertices[ch[1] as usize],
                mesh.vertices[ch[2] as usize],
            ]);
        }
    }
    out
}

fn tris_block(tris: &[Tri], x: f32, z: f32, lo: f32, hi: f32, radius: f32) -> bool {
    for t in tris {
        let ymin = t[0][1].min(t[1][1]).min(t[2][1]);
        let ymax = t[0][1].max(t[1][1]).max(t[2][1]);
        if ymax < lo || ymin > hi {
            continue;
        }
        for k in 0..3 {
            if dist_segment_xz(x, z, t[k], t[(k + 1) % 3]) < radius {
                return true;
            }
        }
    }
    false
}

#[derive(Debug, Clone, Copy)]
pub struct InteriorFlat {
    pub archive: u16,
    pub record: u8,
    pub pos: [f32; 3],
}

#[derive(Debug, Clone)]
pub struct InteriorScene {
    pub mesh: WorldMesh,
    pub flats: Vec<InteriorFlat>,
    pub people: Vec<InteriorFlat>,
    pub spawn: [f32; 3],
    pub exit: Option<[f32; 3]>,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub doors: Vec<InteriorDoor>,
    pub door_open: Vec<bool>,
    pub floors: Vec<Tri>,
    pub walls: Vec<Tri>,
}

fn plane_y(t: &Tri, x: f32, z: f32) -> Option<f32> {
    let (a, b, c) = (t[0], t[1], t[2]);
    let d = (b[2] - c[2]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[2] - c[2]);
    if d.abs() < 1e-9 {
        return None;
    }
    let l1 = ((b[2] - c[2]) * (x - c[0]) + (c[0] - b[0]) * (z - c[2])) / d;
    let l2 = ((c[2] - a[2]) * (x - c[0]) + (a[0] - c[0]) * (z - c[2])) / d;
    let l3 = 1.0 - l1 - l2;
    const EPS: f32 = -1e-4;
    if l1 < EPS || l2 < EPS || l3 < EPS {
        return None;
    }
    Some(l1 * a[1] + l2 * b[1] + l3 * c[1])
}

fn dist_segment_xz(x: f32, z: f32, a: [f32; 3], b: [f32; 3]) -> f32 {
    let (abx, abz) = (b[0] - a[0], b[2] - a[2]);
    let (apx, apz) = (x - a[0], z - a[2]);
    let len2 = abx * abx + abz * abz;
    let t = if len2 < 1e-9 {
        0.0
    } else {
        ((apx * abx + apz * abz) / len2).clamp(0.0, 1.0)
    };
    let (cx, cz) = (a[0] + abx * t, a[2] + abz * t);
    ((x - cx).powi(2) + (z - cz).powi(2)).sqrt()
}

impl InteriorScene {
    pub fn floor_count(&self) -> usize {
        self.floors.len()
    }
    pub fn wall_count(&self) -> usize {
        self.walls.len()
    }

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

    /// Quota del pavimento in (x, z) più vicina a `target_y` entro `max_dist`.
    /// Evita di agganciare soffitti o tetti.
    pub fn floor_closest_to(&self, x: f32, z: f32, target_y: f32, max_dist: f32) -> Option<f32> {
        let mut best: Option<(f32, f32)> = None;
        for t in &self.floors {
            if let Some(y) = plane_y(t, x, z) {
                let dist = (y - target_y).abs();
                if dist <= max_dist && best.map_or(true, |(b_dist, _)| dist < b_dist) {
                    best = Some((dist, y));
                }
            }
        }
        best.map(|(_, y)| y)
    }

    pub fn blocked(&self, x: f32, z: f32, feet_y: f32, radius: f32) -> bool {
        let (lo, hi) = (feet_y + 0.45, feet_y + 1.6);
        if tris_block(&self.walls, x, z, lo, hi, radius) {
            return true;
        }
        for (d, &open) in self.doors.iter().zip(&self.door_open) {
            if !open && tris_block(&d.tris, x, z, lo, hi, radius) {
                return true;
            }
        }
        false
    }

    pub fn toggle_nearest_door(&mut self, x: f32, z: f32, feet_y: f32, reach: f32) -> Option<usize> {
        let mut best: Option<(f32, usize)> = None;
        for (i, d) in self.doors.iter().enumerate() {
            if (d.center[1] - feet_y).abs() > 3.0 {
                continue;
            }
            let mut dist = f32::MAX;
            for t in &d.tris {
                for k in 0..3 {
                    dist = dist.min(dist_segment_xz(x, z, t[k], t[(k + 1) % 3]));
                }
            }
            if dist <= reach && best.map_or(true, |(b, _)| dist < b) {
                best = Some((dist, i));
            }
        }
        let (_, i) = best?;
        self.door_open[i] = !self.door_open[i];
        Some(i)
    }
}

/// Posizione di apparizione all'ingresso e posizione della porta d'ingresso (usata per l'uscita).
fn find_entrance_spawn(
    scene: &InteriorScene,
    doors: &[DoorFace],
    markers: &[[f32; 3]],
    cx: f32,
    cz: f32,
) -> Option<([f32; 3], [f32; 3])> {
    let door = &doors[pick_entrance_door(doors, [cx, cz])?];
    let c = door.center;

    // 1. Marcatore d'ingresso piu' vicino alla porta (se occupato da un arredo, subito intorno).
    if let Some(m) = closest_marker(markers, c, MAX_MARKER_DISTANCE) {
        for radius in [0.0f32, 0.4, 0.8, 1.2] {
            let steps = if radius == 0.0 { 1 } else { 8 };
            for k in 0..steps {
                let a = k as f32 / steps as f32 * std::f32::consts::TAU;
                let (x, z) = (m[0] + a.cos() * radius, m[2] + a.sin() * radius);
                if let Some(floor_y) = scene.floor_closest_to(x, z, m[1], 1.5) {
                    if !scene.blocked(x, z, floor_y, 0.35) {
                        return Some(([x, floor_y, z], c));
                    }
                }
            }
        }
    }

    // 2. Davanti alla porta, dal lato dell'interno (quello con il pavimento).
    let n = [door.normal[0], door.normal[2]];
    let toward_center = n[0] * (cx - c[0]) + n[1] * (cz - c[2]) >= 0.0;
    let sign = if toward_center { 1.0 } else { -1.0 };
    for side in [sign, -sign] {
        for dist in [1.0f32, 1.4, 1.8, 2.3] {
            let (x, z) = (c[0] + n[0] * side * dist, c[2] + n[1] * side * dist);
            if let Some(floor_y) = scene.floor_closest_to(x, z, door.bottom, 1.0) {
                if !scene.blocked(x, z, floor_y, 0.35) {
                    return Some(([x, floor_y, z], c));
                }
            }
        }
    }
    None
}

pub fn build_interior(rmb: &RmbBlock, sub: usize, arch3d: &BsaArchive) -> Option<InteriorScene> {
    let it = rmb.interiors.get(sub)?;
    if it.objects_3d.is_empty() {
        return None;
    }
    let mut cache: HashMap<u32, Option<Arch3dModel>> = HashMap::new();
    let mut mesh = WorldMesh::new();
    let mut doors: Vec<InteriorDoor> = Vec::new();
    let mut door_faces: Vec<DoorFace> = Vec::new();

    for o in &it.objects_3d {
        let model = cache.entry(o.model_id).or_insert_with(|| {
            arch3d
            .get_by_id(o.model_id)
            .and_then(|raw| Arch3dModel::parse(o.model_id, raw).ok())
        });
        let Some(m) = model.as_ref() else { continue };
        let (x, z) = (o.x as f32 * DF_UNIT, o.z as f32 * DF_UNIT);
        let y = if PROPS_LIKE_DFU && o.object_type == PROP_OBJECT_TYPE {
            let bottom = m.vertices.iter().map(|v| v[1]).fold(f32::MAX, f32::min);
            o.y as f32 * DF_UNIT - bottom
        } else {
            -(o.y as f32) * DF_UNIT
        };

        collect_door_faces(m, x, y, z, o.rotation_y_deg, &mut door_faces);

        if looks_like_door(m) {
            let mut local = WorldMesh::new();
            local.add_instance(m, 0.0, 0.0, 0.0, 0.0);
            let mut placed = WorldMesh::new();
            placed.add_instance(m, x, y, z, o.rotation_y_deg);
            let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in &placed.vertices {
                for k in 0..3 {
                    mn[k] = mn[k].min(v[k]);
                    mx[k] = mx[k].max(v[k]);
                }
            }
            doors.push(InteriorDoor {
                model_id: o.model_id,
                mesh: local,
                pos: [x, y, z],
                yaw_deg: o.rotation_y_deg,
                center: [
                    (mn[0] + mx[0]) * 0.5,
                       (mn[1] + mx[1]) * 0.5,
                       (mn[2] + mx[2]) * 0.5,
                ],
                tris: mesh_tris(&placed),
            });
            continue;
        }
        mesh.add_instance(m, x, y, z, o.rotation_y_deg);
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
            if len < 1e-6 {
                continue;
            }
            // Solo superfici con normale Y rivolta verso l'alto (n[1] / len > 0.5)
            // I soffitti (n[1] < 0) vengono esclusi
            if n[1] / len > 0.5 {
                floors.push(t);
            } else {
                walls.push(t);
            }
        }
    }

    // Marcatori d'ingresso (flat editor 199:8 e 199:4): si usano per far apparire il giocatore.
    let enter_markers: Vec<[f32; 3]> = it
        .flats
        .iter()
        .filter(|f| f.texture_archive == EDITOR_FLATS_ARCHIVE && ENTER_MARKER_RECORDS.contains(&f.texture_record))
        .map(|f| [f.x as f32 * DF_UNIT, -(f.y as f32) * DF_UNIT, f.z as f32 * DF_UNIT])
        .collect();

    let flats = it
    .flats
    .iter()
    .filter(|f| f.texture_archive != EDITOR_FLATS_ARCHIVE)
    .map(|f| InteriorFlat {
        archive: f.texture_archive,
         record: f.texture_record,
         pos: [f.x as f32 * DF_UNIT, -(f.y as f32) * DF_UNIT, f.z as f32 * DF_UNIT],
    })
    .collect();

    let people = it
    .people
    .iter()
    .filter(|f| f.texture_archive != EDITOR_FLATS_ARCHIVE)
    .map(|f| InteriorFlat {
        archive: f.texture_archive,
         record: f.texture_record,
         pos: [f.x as f32 * DF_UNIT, -(f.y as f32) * DF_UNIT, f.z as f32 * DF_UNIT],
    })
    .collect();

    let door_open = vec![false; doors.len()];
    let mut scene = InteriorScene {
        mesh,
        flats,
        people,
        spawn: [0.0; 3],
        exit: None,
        bounds_min: mn,
        bounds_max: mx,
        doors,
        door_open,
        floors,
        walls,
    };

    let (cx, cz) = ((mn[0] + mx[0]) * 0.5, (mn[2] + mx[2]) * 0.5);

    // Come Daggerfall Unity: porta d'ingresso = la piu' bassa e piu' esterna; il giocatore appare
    // sul marcatore d'ingresso piu' vicino ad essa o, in mancanza, davanti alla porta.
    if let Some((spawn, exit)) = find_entrance_spawn(&scene, &door_faces, &enter_markers, cx, cz) {
        scene.spawn = spawn;
        scene.exit = Some(exit);
        return Some(scene);
    }

    // Ripiego (nessuna faccia-porta nei modelli): euristica sulle porte dei record.
    // 1. Raccoglie le vere porte d'ingresso (it.doors)
    let mut candidate_doors: Vec<[f32; 3]> = it
    .doors
    .iter()
    .map(|d| [d.x as f32 * DF_UNIT, -(d.y as f32) * DF_UNIT, d.z as f32 * DF_UNIT])
    .filter(|d| {
        d[0] >= mn[0] - 2.0
        && d[0] <= mx[0] + 2.0
        && d[2] >= mn[2] - 2.0
        && d[2] <= mx[2] + 2.0
    })
    .collect();

    // 2. Se it.doors è vuoto, cerca tra i battenti porta (scene.doors) SOLO quelli
    // posti lungo le pareti perimetrali esterne dell'edificio (distanza <= 2.2m dal bordo esterno)
    if candidate_doors.is_empty() {
        for d in &scene.doors {
            let dist_to_perimeter = (d.pos[0] - mn[0])
            .min(mx[0] - d.pos[0])
            .min(d.pos[2] - mn[2])
            .min(mx[2] - d.pos[2]);

            // Se è vicina al muro perimetrale e al piano terra, è la porta d'ingresso/uscita!
            if dist_to_perimeter <= 2.2 && d.pos[1] <= mn[1] + 2.5 {
                candidate_doors.push(d.pos);
            }
        }
    }

    // Se ancora vuoto, prendi qualsiasi porta disponibile
    if candidate_doors.is_empty() {
        for d in &scene.doors {
            if d.pos[1] <= mn[1] + 2.5 {
                candidate_doors.push(d.pos);
            }
        }
    }

    let mut best_spawn: Option<([f32; 3], [f32; 3])> = None;

    // Cerca attorno alla porta vera un punto sul pavimento calpestabile libero da pareti
    for &d in &candidate_doors {
        let (to_cx, to_cz) = (cx - d[0], cz - d[2]);
        let base_angle = to_cz.atan2(to_cx);

        for dist in [1.2f32, 1.5, 1.8, 1.0, 2.2] {
            for angle_offset_deg in [0i32, 25, -25, 50, -50, 75, -75, 100, -100, 135, -135, 180] {
                let rad = base_angle + (angle_offset_deg as f32).to_radians();
                let sx = d[0] + rad.cos() * dist;
                let sz = d[2] + rad.sin() * dist;

                if let Some(floor_y) = scene.floor_closest_to(sx, sz, d[1], 1.2) {
                    if !scene.blocked(sx, sz, floor_y, 0.35) && !scene.blocked(sx, sz, floor_y + 0.8, 0.35) {
                        best_spawn = Some(([sx, floor_y, sz], d));
                        break;
                    }
                }
            }
            if best_spawn.is_some() {
                break;
            }
        }
        if best_spawn.is_some() {
            break;
        }
    }

    // Fallback sicuro se tutte le porte falliscono: piano terra
    let (spawn_pos, exit_door) = match best_spawn {
        Some((sp, dp)) => (sp, Some(dp)),
        None => {
            let mut sorted_floors: Vec<&Tri> = scene.floors.iter().collect();
            sorted_floors.sort_by(|a, b| {
                let ya = (a[0][1] + a[1][1] + a[2][1]) / 3.0;
                let yb = (b[0][1] + b[1][1] + b[2][1]) / 3.0;
                ya.partial_cmp(&yb).unwrap_or(std::cmp::Ordering::Equal)
            });

            let mut fallback: Option<[f32; 3]> = None;
            for t in sorted_floors {
                let centroid = [
                    (t[0][0] + t[1][0] + t[2][0]) / 3.0,
                    (t[0][1] + t[1][1] + t[2][1]) / 3.0,
                    (t[0][2] + t[1][2] + t[2][2]) / 3.0,
                ];
                if let Some(floor_y) = scene.floor_closest_to(centroid[0], centroid[2], centroid[1], 0.5) {
                    if !scene.blocked(centroid[0], centroid[2], floor_y, 0.35) {
                        fallback = Some([centroid[0], floor_y, centroid[2]]);
                        break;
                    }
                }
            }
            let sp = fallback.unwrap_or([cx, mn[1] + 0.1, cz]);
            (sp, candidate_doors.first().copied())
        }
    };

    scene.spawn = spawn_pos;
    scene.exit = exit_door;
    Some(scene)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene_with(floors: Vec<Tri>, walls: Vec<Tri>) -> InteriorScene {
        InteriorScene {
            mesh: WorldMesh::new(),
            flats: Vec::new(),
            people: Vec::new(),
            spawn: [0.0; 3],
            exit: None,
            bounds_min: [0.0; 3],
            bounds_max: [0.0; 3],
            doors: Vec::new(),
            door_open: Vec::new(),
            floors,
            walls,
        }
    }

    #[test]
    fn floor_height_and_step() {
        let low: Vec<Tri> = vec![
            [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 0.0, 4.0]],
            [[0.0, 0.0, 0.0], [4.0, 0.0, 4.0], [0.0, 0.0, 4.0]],
        ];
        let step: Tri = [[1.0, 0.4, 1.0], [3.0, 0.4, 1.0], [2.0, 0.4, 3.0]];
        let s = scene_with([low, vec![step]].concat(), Vec::new());
        assert_eq!(s.floor_at(0.5, 3.0, 0.0, 0.6), Some(0.0));
        let on_step = s.floor_at(2.0, 1.8, 0.0, 0.6).unwrap();
        assert!((on_step - 0.4).abs() < 1e-4);
        assert_eq!(s.floor_at(10.0, 10.0, 0.0, 0.6), None);
    }

    fn door(x: f32, z: f32, bottom: f32) -> DoorFace {
        DoorFace { center: [x, bottom + 1.1, z], normal: [1.0, 0.0, 0.0], bottom }
    }

    #[test]
    fn entrance_is_the_lowest_and_outermost_door() {
        // centro dell'edificio in (0, 0): una porta interna vicina, una d'ingresso sul perimetro,
        // una al piano di sopra molto esterna ma in alto.
        let doors = [door(1.0, 0.0, 0.0), door(8.0, 0.0, 0.0), door(9.0, 0.0, 3.0)];
        assert_eq!(pick_entrance_door(&doors, [0.0, 0.0]), Some(1));
        assert_eq!(pick_entrance_door(&[], [0.0, 0.0]), None);
    }

    #[test]
    fn closest_enter_marker_to_the_door_is_chosen() {
        let markers = [[2.0, 0.0, 0.0], [7.0, 0.0, 0.5], [-3.0, 0.0, 4.0]];
        let near_door = [8.0, 1.1, 0.0];
        assert_eq!(closest_marker(&markers, near_door, 12.0), Some([7.0, 0.0, 0.5]));
        // un marcatore troppo lontano e' ignorato
        assert_eq!(closest_marker(&[[-30.0, 0.0, 0.0]], near_door, 12.0), None);
    }

    #[test]
    fn spawn_uses_marker_next_to_entrance_door() {
        // pavimento 20x20 e nessun muro: il marcatore e' libero e si usa cosi' com'e'
        let floor: Vec<Tri> = vec![
            [[-10.0, 0.0, -10.0], [10.0, 0.0, -10.0], [10.0, 0.0, 10.0]],
            [[-10.0, 0.0, -10.0], [10.0, 0.0, 10.0], [-10.0, 0.0, 10.0]],
        ];
        let s = scene_with(floor, Vec::new());
        let doors = [door(1.0, 0.0, 0.0), door(8.0, 0.0, 0.0)];
        let (spawn, exit) = find_entrance_spawn(&s, &doors, &[[7.0, 0.0, 0.5]], 0.0, 0.0).unwrap();
        assert_eq!(spawn, [7.0, 0.0, 0.5]);
        assert_eq!(exit, doors[1].center);
        // senza marcatori: davanti alla porta, dal lato del centro (x < 8)
        let (spawn, _) = find_entrance_spawn(&s, &doors, &[], 0.0, 0.0).unwrap();
        assert!(spawn[0] < 8.0 && (8.0 - spawn[0]) >= 1.0 - 1e-4);
    }

    #[test]
    fn wall_blocks_between_knee_and_head() {
        let wall: Vec<Tri> = vec![
            [[0.0, 0.0, 2.0], [4.0, 0.0, 2.0], [4.0, 3.0, 2.0]],
            [[0.0, 0.0, 2.0], [4.0, 3.0, 2.0], [0.0, 3.0, 2.0]],
        ];
        let s = scene_with(Vec::new(), wall);
        assert!(s.blocked(2.0, 1.8, 0.0, 0.3));
        assert!(!s.blocked(2.0, 1.0, 0.0, 0.3));
        let low_wall: Vec<Tri> = vec![[[0.0, 0.0, 2.0], [4.0, 0.0, 2.0], [4.0, 0.3, 2.0]]];
        let s2 = scene_with(Vec::new(), low_wall);
        assert!(!s2.blocked(2.0, 2.0, 0.0, 0.3));
    }
}
