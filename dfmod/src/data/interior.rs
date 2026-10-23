//! Interni degli edifici (taverne): modelli e flat del secondo BlockData di ogni sotto-record
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

/// Tipo di oggetto 3D che in Daggerfall Unity indica arredi e props.
const PROP_OBJECT_TYPE: u8 = 3;
/// Se true, gli arredi (tipo 3) si posizionano come in Daggerfall Unity: Y non negata e modello
/// appoggiato sul suo punto piu' basso. Mettilo a false per tornare al comportamento precedente.
const PROPS_LIKE_DFU: bool = true;

pub type Tri = [[f32; 3]; 3];

/// Porta di una stanza: modello separato dal resto, che ruota attorno alla sua origine (la cerniera).
#[derive(Debug, Clone)]
pub struct InteriorDoor {
    pub model_id: u32,
    /// geometria nel sistema del modello (origine = cerniera), senza rotazione
    pub mesh: WorldMesh,
    /// posizione della cerniera nel sistema locale dell'interno (metri)
    pub pos: [f32; 3],
    /// rotazione a porta chiusa: stessa convenzione di `WorldMesh::add_instance`
    pub yaw_deg: f32,
    /// centro della porta a riposo (sistema locale), per capire a che altezza sta
    pub center: [f32; 3],
    /// triangoli a porta chiusa (sistema locale), per le collisioni e per la distanza
    tris: Vec<Tri>,
}

/// Riconosce i modelli-porta: pochi piani, texture di porta (archivio 74 e varianti climatiche)
/// e dimensioni da porta (stretti, alti 1.5-3.4 m, larghi 0.6-2.2 m).
fn looks_like_door(m: &Arch3dModel) -> bool {
    if m.faces.is_empty() || m.faces.len() > 24 {
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
    (0.6..=2.2).contains(&w) && (1.5..=3.4).contains(&dy) && t <= 0.5
}

fn mesh_tris(mesh: &WorldMesh) -> Vec<Tri> {
    let mut out = Vec::new();
    for sm in &mesh.submeshes {
        for ch in sm.indices.chunks_exact(3) {
            out.push([mesh.vertices[ch[0] as usize], mesh.vertices[ch[1] as usize], mesh.vertices[ch[2] as usize]]);
        }
    }
    out
}

/// true se un cilindro di raggio `radius` in (x, z) tocca uno dei triangoli tra `lo` e `hi` in quota.
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

/// Sprite (flat) dell'interno, con la base al centro in `pos` (metri, sistema locale).
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
    /// dove compaiono i piedi del giocatore (sistema locale)
    pub spawn: [f32; 3],
    /// porta d'uscita, se il record delle porte e' plausibile
    pub exit: Option<[f32; 3]>,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// porte delle stanze, apribili con E
    pub doors: Vec<InteriorDoor>,
    /// stato di ogni porta (true = aperta)
    pub door_open: Vec<bool>,
    floors: Vec<Tri>,
    walls: Vec<Tri>,
}

/// Altezza del piano del triangolo in (x, z), se il punto cade nella sua proiezione.
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

/// Distanza orizzontale dal punto (x, z) al segmento a-b proiettato sul piano.
fn dist_segment_xz(x: f32, z: f32, a: [f32; 3], b: [f32; 3]) -> f32 {
    let (abx, abz) = (b[0] - a[0], b[2] - a[2]);
    let (apx, apz) = (x - a[0], z - a[2]);
    let len2 = abx * abx + abz * abz;
    let t = if len2 < 1e-9 { 0.0 } else { ((apx * abx + apz * abz) / len2).clamp(0.0, 1.0) };
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

    /// Quota del pavimento in (x, z): la piu' alta tra le superfici quasi orizzontali che non
    /// stanno sopra `ref_y + step`. Cosi' scale e gradini (alzate basse) si salgono da soli.
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

    /// true se un cilindro di raggio `radius` in (x, z), con i piedi a `feet_y`, tocca una parete.
    /// Si considerano le pareti tra il ginocchio e la testa.
    pub fn blocked(&self, x: f32, z: f32, feet_y: f32, radius: f32) -> bool {
        let (lo, hi) = (feet_y + 0.45, feet_y + 1.6);
        if tris_block(&self.walls, x, z, lo, hi, radius) {
            return true;
        }
        // le porte aperte non bloccano
        for (d, &open) in self.doors.iter().zip(&self.door_open) {
            if !open && tris_block(&d.tris, x, z, lo, hi, radius) {
                return true;
            }
        }
        false
    }

    /// Apre/chiude la porta piu' vicina a (x, z) entro `reach` metri. Restituisce il suo indice.
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

/// Costruisce l'interno del sotto-record `sub` del blocco. `None` se non ha modelli 3D.
pub fn build_interior(rmb: &RmbBlock, sub: usize, arch3d: &BsaArchive) -> Option<InteriorScene> {
    let it = rmb.interiors.get(sub)?;
    if it.objects_3d.is_empty() {
        return None;
    }

    let debug_doors = std::env::var("DF_DOOR_DEBUG").is_ok();
    let mut cache: HashMap<u32, Option<Arch3dModel>> = HashMap::new();
    let mut mesh = WorldMesh::new();
    let mut doors: Vec<InteriorDoor> = Vec::new();
    for o in &it.objects_3d {
        let model = cache.entry(o.model_id).or_insert_with(|| {
            arch3d.get_by_id(o.model_id).and_then(|raw| Arch3dModel::parse(o.model_id, raw).ok())
        });
        let Some(m) = model.as_ref() else { continue };
        let (x, z) = (o.x as f32 * DF_UNIT, o.z as f32 * DF_UNIT);
        let y = if PROPS_LIKE_DFU && o.object_type == PROP_OBJECT_TYPE {
            // arredi: la Y non si nega e il modello poggia sul suo punto piu' basso
            let bottom = m.vertices.iter().map(|v| v[1]).fold(f32::MAX, f32::min);
            o.y as f32 * DF_UNIT - bottom
        } else {
            -(o.y as f32) * DF_UNIT
        };
        let is_door = looks_like_door(m);
        if debug_doors {
            let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
            for v in &m.vertices {
                for k in 0..3 {
                    mn[k] = mn[k].min(v[k]);
                    mx[k] = mx[k].max(v[k]);
                }
            }
            let mut archives: Vec<u16> = m.faces.iter().map(|f| f.texture_archive).collect();
            archives.sort_unstable();
            archives.dedup();
            println!(
                "DOOR dbg modello {} tipo {} piani {} dim {:.2}x{:.2}x{:.2} m archivi {:?} -> {}",
                o.model_id,
                o.object_type,
                m.faces.len(),
                mx[0] - mn[0],
                mx[1] - mn[1],
                mx[2] - mn[2],
                archives,
                if is_door { "PORTA" } else { "-" }
            );
        }
        if is_door {
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
                center: [(mn[0] + mx[0]) * 0.5, (mn[1] + mx[1]) * 0.5, (mn[2] + mx[2]) * 0.5],
                tris: mesh_tris(&placed),
            });
            continue;
        }
        // Le coordinate z restano quelle grezze (come in Daggerfall Unity), quindi la rotazione
        // e' quella di DFU: x' = x cos - z sin, z' = x sin + z cos, che e' `add_instance(+angolo)`.
        // Il segno invertito delle citta' (CityBuilder) vale solo perche' li' z = -nord.
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
            // il segno della normale non conta: si classifica per inclinazione
            if (n[1] / len).abs() > 0.5 {
                floors.push(t);
            } else {
                walls.push(t);
            }
        }
    }

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

    if debug_doors || !doors.is_empty() {
        println!("Interno: {} porte di stanza riconosciute.", doors.len());
    }
    let door_open = vec![false; doors.len()];
    let mut scene = InteriorScene {
        mesh,
        flats,
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
    // porta valida solo se cade dentro (o appena fuori) dall'ingombro dei modelli
    let door = it
        .doors
        .first()
        .map(|d| [d.x as f32 * DF_UNIT, -(d.y as f32) * DF_UNIT, d.z as f32 * DF_UNIT])
        .filter(|d| d[0] >= mn[0] - 2.0 && d[0] <= mx[0] + 2.0 && d[2] >= mn[2] - 2.0 && d[2] <= mx[2] + 2.0);

    let (sx, sz, ref_y) = match door {
        Some(d) => {
            let (dx, dz) = (cx - d[0], cz - d[2]);
            let l = (dx * dx + dz * dz).sqrt().max(1e-3);
            (d[0] + dx / l * 1.5, d[2] + dz / l * 1.5, d[1] + 1.0)
        }
        None => (cx, cz, mn[1] + 1.0),
    };
    let floor = scene
        .floor_at(sx, sz, ref_y, 1.0)
        .or_else(|| scene.floor_at(cx, cz, mn[1] + 1.0, 1.0))
        .unwrap_or(mn[1]);
    scene.spawn = [sx, floor, sz];
    scene.exit = door;
    Some(scene)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene_with(floors: Vec<Tri>, walls: Vec<Tri>) -> InteriorScene {
        InteriorScene {
            mesh: WorldMesh::new(),
            flats: Vec::new(),
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
        // due triangoli che coprono il quadrato 0..4 a quota 0 e 0.4 (gradino)
        let low: Vec<Tri> = vec![
            [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 0.0, 4.0]],
            [[0.0, 0.0, 0.0], [4.0, 0.0, 4.0], [0.0, 0.0, 4.0]],
        ];
        let step: Tri = [[1.0, 0.4, 1.0], [3.0, 0.4, 1.0], [2.0, 0.4, 3.0]];
        let s = scene_with([low, vec![step]].concat(), Vec::new());
        assert_eq!(s.floor_at(0.5, 3.0, 0.0, 0.6), Some(0.0));
        let on_step = s.floor_at(2.0, 1.8, 0.0, 0.6).unwrap();
        assert!((on_step - 0.4).abs() < 1e-4);
        // fuori dal pavimento non c'e' nulla
        assert_eq!(s.floor_at(10.0, 10.0, 0.0, 0.6), None);
    }

    #[test]
    fn wall_blocks_between_knee_and_head() {
        // parete verticale sul piano z = 2, da x = 0 a 4, alta 3 m
        let wall: Vec<Tri> = vec![
            [[0.0, 0.0, 2.0], [4.0, 0.0, 2.0], [4.0, 3.0, 2.0]],
            [[0.0, 0.0, 2.0], [4.0, 3.0, 2.0], [0.0, 3.0, 2.0]],
        ];
        let s = scene_with(Vec::new(), wall);
        assert!(s.blocked(2.0, 1.8, 0.0, 0.3));
        assert!(!s.blocked(2.0, 1.0, 0.0, 0.3));
        // un gradino basso (sotto il ginocchio) non blocca
        let low_wall: Vec<Tri> = vec![[[0.0, 0.0, 2.0], [4.0, 0.0, 2.0], [4.0, 0.3, 2.0]]];
        let s2 = scene_with(Vec::new(), low_wall);
        assert!(!s2.blocked(2.0, 2.0, 0.0, 0.3));
    }
}
