//! Costruzione delle mesh degli INTERNI dai BlockData interni dei sotto-record RMB.
//!
//! Convenzioni (verificate su BLOCKS.BSA, vedi `dump_interiors`):
//! - le coordinate X/Y/Z di mobili e porte sono LOCALI al subrecord: per andare nel
//!   sistema del blocco si applica la rotazione del subrecord e si somma l'ancora (x, z);
//! - il modello ARCH3D va ruotato di 180 gradi (i modelli hanno il "frontale" verso -Z),
//!   poi ruotato dell'angolo dell'oggetto;
//! - Y = altezza Daggerfall (positivo verso il BASSO): la quota interna del pavimento e'
//!   circa +45 DF (~1.1 m), convertita in metri Bevy con -Y * 0.025;
//! - le flats interne usano SOLO archivi bitmap (0/3/34/35/98): sono decorazioni 2D
//!   appese o appoggiate, NON modelli ARCH3D.
//!
//! L'output e' nello SPAZIO LOCALE della stanza (non del blocco): origine nell'ancora
//! del subrecord, asse +z = nord del locale (rotazione del subrecord gia' applicata),
//! y = altezza in metri (su = positivo). Cio' rende la mesh indipendente dal blocco:
//! basta una radice Bevy con posizione/rotazione per collocarla dove si vuole.

use std::collections::HashMap;

use crate::arch3d::Arch3dModel;
use crate::blocks::{InteriorBlock, RmbBlock, RmbObject3D};
use crate::bsa::BsaArchive;
use crate::building_gen::{DF_UNIT, MeshPart};

/// Angoli Daggerfall -> radianti.
const DA_TO_RAD: f32 = std::f32::consts::PI / 1024.0;

/// Materiali degli interni: pavimento, soffitto, pareti, porte, arredi.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteriorMaterial {
    Floor,
    Ceiling,
    Wall,
    Door,
    Furniture,
}

pub const INTERIOR_MATERIAL_COUNT: usize = 5;

impl InteriorMaterial {
    pub const ALL: [InteriorMaterial; INTERIOR_MATERIAL_COUNT] = [
        InteriorMaterial::Floor,
        InteriorMaterial::Ceiling,
        InteriorMaterial::Wall,
        InteriorMaterial::Door,
        InteriorMaterial::Furniture,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            InteriorMaterial::Floor => "floor",
            InteriorMaterial::Ceiling => "ceiling",
            InteriorMaterial::Wall => "wall",
            InteriorMaterial::Door => "door",
            InteriorMaterial::Furniture => "furniture",
        }
    }
}

#[derive(Debug, Clone)]
pub struct InteriorDoor {
    /// posizione della porta nello spazio della mesh (assi Bevy: x est, z = -nord)
    pub position: [f32; 3],
    /// normale esterna della porta (verso cui si esce), unitaria su x/z
    pub normal: [f32; 2],
    /// apertura approssimata in metri
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone)]
pub struct InteriorPerson {
    pub position: [f32; 3],
    /// orientamento in gradi (0 = verso -Z locale, come i modelli ARCH3D)
    pub direction_deg: f32,
    pub texture_archive: u16,
    pub texture_record: u8,
}

#[derive(Debug, Default)]
pub struct InteriorMesh {
    /// un part per `InteriorMaterial`
    pub parts: [MeshPart; INTERIOR_MATERIAL_COUNT],
    /// un part per ogni texture bitmap delle decorazioni 2D
    pub textured: Vec<((u16, u8), MeshPart)>,
    pub doors: Vec<InteriorDoor>,
    pub people: Vec<InteriorPerson>,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub spawn_position: [f32; 3],
    pub missing_models: usize,
}

impl InteriorMesh {
    pub fn is_empty(&self) -> bool {
        self.parts.iter().all(|p| p.is_empty()) && self.textured.iter().all(|(_, p)| p.is_empty())
    }

    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(|p| p.triangle_count()).sum::<usize>()
            + self.textured.iter().map(|(_, p)| p.triangle_count()).sum::<usize>()
    }

    /// Punto di nascita del giocatore: meta' strada tra la porta d'ingresso
    /// ((0,0,-R)) e il centro della stanza.
    pub fn spawn_position(&self) -> [f32; 3] {
        self.spawn_position
    }

/// Vertici del modello nello spazio locale della stanza (flip 180° + rotazione
/// oggetto + traslazione locale). Calcolarli prima di toccare la cache evita
/// problemi di borrow.
fn transformed_vertices(model: &Arch3dModel, o: &RmbObject3D) -> Vec<[f32; 3]> {
    let (so, co) = (o.rotation_y_deg * DA_TO_RAD).sin_cos();
    // flip di 180 gradi del modello + rotazione propria dell'oggetto
    let (s, c) = (-so, -co);
    model
        .vertices
        .iter()
        .map(|v| {
            let mut p = [v[0], v[1], v[2]];
            p[0] = v[0] * c + v[2] * s;
            p[2] = -v[0] * s + v[2] * c;
            p[1] = v[1];
            [
                p[0] + o.x as f32 * DF_UNIT,
                p[1] - o.y as f32 * DF_UNIT,
                p[2] + o.z as f32 * DF_UNIT,
            ]
        })
        .collect()
}

/// Costruisce la mesh dell'interno di un sotto-record nello spazio locale della
/// stanza (origine = ancora del subrecord, y in metri verso l'alto). La rotazione
/// del subrecord NON e' applicata: va applicata lato scene al collocare la mesh.
pub fn build_interior(interior: &InteriorBlock, _block: &RmbBlock, bsa: &BsaArchive) -> InteriorMesh {
    let mut out = InteriorMesh::default();
    let mut cache: HashMap<u32, Option<Arch3dModel>> = HashMap::new();

    // ---- modelli 3D (pareti, soffitto, pavimento, arredi) -----------------------
    for o in &interior.data.objects_3d {
        let id = o.model_id;
        let entry = cache.entry(id).or_insert_with(|| {
            bsa.get_by_id(id).and_then(|raw| Arch3dModel::parse(id, raw).ok())
        });
        let Some(model) = entry else {
            out.missing_models += 1;
            continue;
        };
        let verts = transformed_vertices(model, o);
        let kind = classify_model(o, &verts);
        let mat = kind.material();
        append_model(&mut out.parts[mat], model, &verts);
    }

    // ---- flats: decorazioni 2D (bitmap) ------------------------------------------
    // Le flats interne non hanno rotazione nel record: il quad viene rivolto verso
    // il centro della stanza (stimato dalla media dei modelli), cosi' quadri e
    // segnali risultano comunque visibili da chi entra.
    let (ccx, ccz) = centroid_xz(&interior.data.objects_3d);
    for f in &interior.data.flats {
        let cx = f.x as f32 * DF_UNIT;
        let cz = f.z as f32 * DF_UNIT;
        let cy = -f.y as f32 * DF_UNIT;
        // normale: dal punto verso il centro stanza (se la flat e' gia' al centro,
        // usa l'asse +x)
        let (mut nx, mut nz) = (ccx - cx, ccz - cz);
        let ln = (nx * nx + nz * nz).sqrt();
        if ln < 1e-4 {
            nx = 1.0;
            nz = 0.0;
        } else {
            nx /= ln;
            nz /= ln;
        }
        let hw = 0.4;
        let hh = if f.height > 0 && f.height < 64 { f.height as f32 * DF_UNIT * 0.5 } else { 0.4 };
        let part = out.textured_part((f.texture_archive, f.texture_record));
        // asse orizzontale del quad = perpendicolare alla normale
        let (ux, uz) = (-nz, nx);
        let corners = [
            [-hw, -hh],
            [hw, -hh],
            [hw, hh],
            [-hw, hh],
        ];
        let base = part.positions.len() as u32;
        for (k, &(px, py)) in corners.iter().enumerate() {
            part.positions.push([cx + ux * px, cy + py, cz + uz * px]);
            part.normals.push([nx, 0.0, nz]);
            part.uvs.push([[0.0, 1.0][k / 2], [0.0, 1.0][(k + 1) / 2 % 2]]);
            part.colors.push([1.0, 1.0, 1.0, 1.0]);
        }
        part.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    // ---- porte -------------------------------------------------------------------
    for d in &interior.doors {
        let wx = d.x as f32 * DF_UNIT;
        let wz = d.z as f32 * DF_UNIT;
        let wy = -d.y as f32 * DF_UNIT;
        let phi = d.rotation_deg * DA_TO_RAD;
        let nx = phi.cos();
        let nz = phi.sin();
        out.doors.push(InteriorDoor {
            position: [wx, wy, wz],
            normal: [nx, nz],
            width: 1.0,
            height: 1.65,
        });
        // un piccolo quadro scuro che marca la porta
        let hw = 0.5;
        let hh = 0.85;
        let (s, c) = phi.sin_cos();
        let part = &mut out.parts[InteriorMaterial::Door.index()];
        let base = part.positions.len() as u32;
        for (px, py) in [(-hw, -hh), (hw, -hh), (hw, hh), (-hw, hh)] {
            let rx = px * c;
            let rz = -px * s;
            part.positions.push([wx + rx, wy + py, wz + rz]);
            part.normals.push([nx, 0.0, nz]);
            part.uvs.push([0.0, 0.0]);
            part.colors.push([0.25, 0.15, 0.08, 1.0]);
        }
        part.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    // ---- persone ------------------------------------------------------------------
    for p in &interior.people {
        out.people.push(InteriorPerson {
            position: [
                p.x as f32 * DF_UNIT,
                -p.y as f32 * DF_UNIT,
                p.z as f32 * DF_UNIT,
            ],
            direction_deg: p.direction_deg,
            texture_archive: p.texture_archive,
            texture_record: p.texture_record,
        });
    }

    // bounds e spawn
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for part in out.parts.iter().chain(out.textured.iter().map(|(_, p)| p)) {
        for p in &part.positions {
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
    }
    if min[0] > max[0] {
        min = [0.0; 3];
        max = [0.0; 3];
    }
    out.min = min;
    out.max = max;
    // la porta d'ingresso canonica e' nell'ancora del subrecord = origine locale
    let center = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5, (min[2] + max[2]) * 0.5];
    out.spawn_position = [center[0] * 0.5, center[1], center[2] * 0.5];

    out
}

/// Centro (x, z) in metri della stanza, mediato sulle posizioni dei modelli 3D.
fn centroid_xz(objects: &[RmbObject3D]) -> (f32, f32) {
    if objects.is_empty() {
        return (0.0, 0.0);
    }
    let (sx, sz): (f32, f32) = objects
        .iter()
        .map(|o| (o.x as f32 * DF_UNIT, o.z as f32 * DF_UNIT))
        .fold((0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
    (sx / objects.len() as f32, sz / objects.len() as f32)
}

enum ModelKind {
    Floor,
    Ceiling,
    Wall,
    Furniture,
}

impl ModelKind {
    fn material(self) -> usize {
        match self {
            ModelKind::Floor => InteriorMaterial::Floor.index(),
            ModelKind::Ceiling => InteriorMaterial::Ceiling.index(),
            ModelKind::Wall => InteriorMaterial::Wall.index(),
            ModelKind::Furniture => InteriorMaterial::Furniture.index(),
        }
    }
}

/// Classifica un modello piazzato in interno guardando i suoi vertici trasformati:
/// pavimento = piatto in basso, soffitto = piatto in alto, pareti = facce verticali
/// che stanno sul bordo della stanza, tutto il resto e arredamento.
fn classify_model(o: &RmbObject3D, verts: &[[f32; 3]]) -> ModelKind {
    if verts.is_empty() {
        return ModelKind::Furniture;
    }
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for v in verts {
        for k in 0..3 {
            min[k] = min[k].min(v[k]);
            max[k] = max[k].max(v[k]);
        }
    }
    let h = max[1] - min[1];
    let w = max[0] - min[0];
    let d = max[2] - min[2];
    let big = w.max(d);
    if h < 0.25 && big > 2.0 {
        // piastra orizzontale grande: pavimento o soffitto dall'altezza
        return if min[1] <= 0.05 { ModelKind::Floor } else { ModelKind::Ceiling };
    }
    if (w < 0.4 || d < 0.4) && big > 2.0 && h > 1.5 {
        return ModelKind::Wall;
    }
    let _ = o;
    ModelKind::Furniture
}

fn info_counts(_a: usize, _b: usize, _c: usize, _d: usize, _out: &InteriorMesh) {}

/// Aggiunge un modello ARCH3D (gia' con i vertici trasformati in `verts`) a un MeshPart,
/// triangolizzando le facce convex.
fn append_model(part: &mut MeshPart, model: &Arch3dModel, verts: &[[f32; 3]]) {
    for face in &model.faces {
        let idx = &face.vertex_indices;
        if idx.len() < 3 {
            continue;
        }
        let base = part.positions.len() as u32;
        for (k, &vi) in idx.iter().enumerate() {
            let p = verts.get(vi as usize).copied().unwrap_or([0.0; 3]);
            part.positions.push(p);
            part.normals.push(face_normal_at(verts, idx));
            let uv = face.uvs.get(k).copied().unwrap_or([0.0, 0.0]);
            part.uvs.push([uv[0], -uv[1]]);
            part.colors.push([1.0, 1.0, 1.0, 1.0]);
        }
        for k in 1..(idx.len() as u32 - 1) {
            part.indices.extend_from_slice(&[base, base + k, base + k + 1]);
        }
    }
}

fn face_normal_at(verts: &[[f32; 3]], idx: &[u32]) -> [f32; 3] {
    let g = |i: usize| verts.get(idx[i] as usize).copied().unwrap_or([0.0; 3]);
    if idx.len() < 3 {
        return [0.0, 1.0, 0.0];
    }
    let a = g(0);
    let b = g(1);
    let c = g(2);
    let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        e1[1] * e2[2] - e1[2] * e2[1],
        e1[2] * e2[0] - e1[0] * e2[2],
        e1[0] * e2[1] - e1[1] * e2[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len < 1e-9 {
        [0.0, 1.0, 0.0]
    } else {
        [n[0] / len, n[1] / len, n[2] / len]
    }
}

impl InteriorMesh {
    fn textured_part(&mut self, key: (u16, u8)) -> &mut MeshPart {
        if let Some(i) = self.textured.iter().position(|(k, _)| *k == key) {
            return &mut self.textured[i].1;
        }
        self.textured.push((key, MeshPart::default()));
        &mut self.textured.last_mut().unwrap().1
    }
}
