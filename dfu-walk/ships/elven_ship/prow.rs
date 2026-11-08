//! Prua del veliero elfico: rostro a stella (mesh custom 9 vertici, 24
//! indici) + spine dorate + polena femminile. `createProw` del Three.js.

use bevy::math::Vec3;
use bevy::prelude::*;

use super::super::ElvenMaterial;
use super::primitives::{cylinder, quad, sphere, tube, MeshBuilder, Part};

const PROW_ORIGIN: Vec3 = Vec3::new(42.5, 0.0, 0.0);

pub fn build(b: &mut MeshBuilder) {
    rostro(b);
    polena(b);
}

// =============================================================================
// ROSTRO (ramGeo)
// =============================================================================

fn rostro(b: &mut MeshBuilder) {
    let base = b.part(ElvenMaterial::Gold).positions.len();
    let part = b.part(ElvenMaterial::Gold);

    // 9 vertici locali del rostro (dal Three.js).
    let v: [Vec3; 9] = [
        Vec3::new(15.0, 1.2,  0.00),
        Vec3::new( 7.5, 2.4,  0.00),
        Vec3::new( 7.5, 1.2,  0.55),
        Vec3::new( 7.5, 0.1,  0.00),
        Vec3::new( 7.5, 1.2, -0.55),
        Vec3::new( 0.0, 3.4,  0.00),
        Vec3::new( 0.0, 1.7,  0.70),
        Vec3::new( 0.0, 0.0,  0.00),
        Vec3::new( 0.0, 1.7, -0.70),
    ];
    // Facce del rostro (24 terne, dal Three.js).
    let faces: [[usize; 3]; 24] = [
        [0, 1, 2], [0, 2, 3], [0, 3, 4], [0, 4, 1],
        [1, 5, 6], [1, 6, 2], [2, 6, 7], [2, 7, 3],
        [3, 7, 8], [3, 8, 4], [4, 8, 5], [4, 5, 1],
        [5, 6, 7], [5, 7, 8],
        [0, 1, 5], [0, 5, 7], [0, 7, 3], [0, 3, 5],
        [1, 2, 6], [1, 6, 5], [2, 3, 7], [2, 7, 6],
        [3, 4, 8], [3, 8, 7],
    ];
    for [a, c, d] in faces {
        push_tri_v(part, v[a] + PROW_ORIGIN, v[c] + PROW_ORIGIN, v[d] + PROW_ORIGIN);
    }
    let _ = base;

    // Spine dorsali (tube).
    tube(
        b.part(ElvenMaterial::Gold),
        &[
            PROW_ORIGIN + Vec3::new(0.0, 3.4, 0.0),
            PROW_ORIGIN + Vec3::new(7.5, 2.4, 0.0),
            PROW_ORIGIN + Vec3::new(15.0, 1.2, 0.0),
        ],
        0.10, 16, 6,
    );
    tube(
        b.part(ElvenMaterial::Gold),
        &[
            PROW_ORIGIN + Vec3::new(0.0, 0.0, 0.0),
            PROW_ORIGIN + Vec3::new(7.5, 0.1, 0.0),
            PROW_ORIGIN + Vec3::new(15.0, 1.2, 0.0),
        ],
        0.09, 16, 6,
    );
}

fn push_tri_v(part: &mut Part, a: Vec3, b: Vec3, c: Vec3) {
    use super::primitives::tri;
    tri(part, a, b, c, [1.0; 4], [0.0, 0.0], [1.0, 0.0], [0.5, 1.0]);
}

// =============================================================================
// POLENA (figurehead)
// =============================================================================

const FIG_ORIGIN: Vec3 = Vec3::new(0.55, 4.45, 0.0);
const FIG_ROT_Z: f32 = -0.26;

fn polena(b: &mut MeshBuilder) {
    // Trasformazione: ogni vertice locale della polena viene ruotato di
    // FIG_ROT_Z attorno a Z, poi traslato a PROW_ORIGIN + FIG_ORIGIN.
    let anchor = PROW_ORIGIN + FIG_ORIGIN;

    // 1. Drappeggio (tube 3 punti).
    tube(
        b.part(ElvenMaterial::Gold),
        &local_to_world(&[
            Vec3::new(-1.0, -1.8, 0.0),
            Vec3::new(-0.2, -0.9, 0.0),
            Vec3::new(0.3, 0.0, 0.0),
        ], anchor),
        0.38, 16, 10,
    );

    // 2. Fianchi (sfera schiacciata 1.0, 1.15, 0.85).
    sphere_scaled(
        b.part(ElvenMaterial::Gold),
        anchor + rot_z(Vec3::new(0.32, 0.0, 0.0)),
        0.42, Vec3::new(1.0, 1.15, 0.85), 12, 12,
    );

    // 3. Torso (cilindro ruotato -0.35 su Z).
    cylinder_oriented(
        b.part(ElvenMaterial::Gold),
        anchor + rot_z(Vec3::new(0.55, 0.55, 0.0)),
        0.28, 0.36, 0.9, 12,
        Quat::from_rotation_z(FIG_ROT_Z - 0.35),
    );

    // 4. Seni (due sfere).
    for z_side in [-0.15_f32, 0.15] {
        sphere(
            b.part(ElvenMaterial::Gold),
            anchor + rot_z(Vec3::new(0.78, 0.65, z_side)),
            0.18, 10, 10,
        );
    }

    // 5. Collo (cilindro).
    cylinder_oriented(
        b.part(ElvenMaterial::Gold),
        anchor + rot_z(Vec3::new(0.78, 1.05, 0.0)),
        0.18, 0.16, 0.35, 10,
        Quat::from_rotation_z(FIG_ROT_Z - 0.2),
    );

    // 6. Testa (sfera scalata 1.15, 1.12, 0.95).
    sphere_scaled(
        b.part(ElvenMaterial::Gold),
        anchor + rot_z(Vec3::new(0.9, 1.25, 0.0)),
        0.26, Vec3::new(1.15, 1.12, 0.95), 14, 14,
    );

    // 7. Chioma (sfera scalata 1.2, 0.95, 1.15).
    sphere_scaled(
        b.part(ElvenMaterial::Gold),
        anchor + rot_z(Vec3::new(0.78, 1.34, 0.0)),
        0.30, Vec3::new(1.2, 0.95, 1.15), 12, 12,
    );

    // 8. Nove ciuffi di capelli (tube su 6 o 5 punti ciascuno).
    let tresses: [[(f32, f32, f32); 6]; 9] = [
        [(0.75, 1.42, 0.0), (0.35, 1.62, 0.0), (-0.25, 1.32, 0.0), (-0.95, 1.68, 0.0), (-1.75, 1.22, 0.0), (-2.65, 1.45, 0.0)],
        [(0.72, 1.38, 0.18), (0.30, 1.58, 0.32), (-0.35, 1.25, 0.15), (-1.10, 1.55, 0.38), (-1.95, 1.12, 0.20), (-2.55, 1.35, 0.10)],
        [(0.72, 1.38, -0.18), (0.30, 1.58, -0.32), (-0.35, 1.25, -0.15), (-1.10, 1.55, -0.38), (-1.95, 1.12, -0.20), (-2.55, 1.35, -0.10)],
        [(0.65, 1.25, 0.25), (0.15, 1.40, 0.48), (-0.45, 1.05, 0.28), (-1.25, 1.35, 0.52), (-2.15, 0.85, 0.32), (-2.70, 1.10, 0.22)],
        [(0.65, 1.25, -0.25), (0.15, 1.40, -0.48), (-0.45, 1.05, -0.28), (-1.25, 1.35, -0.52), (-2.15, 0.85, -0.32), (-2.70, 1.10, -0.22)],
        [(0.60, 1.10, 0.15), (0.00, 1.22, 0.32), (-0.65, 0.85, 0.18), (-1.40, 1.15, 0.34), (-2.20, 0.72, 0.18), (0.0, 0.0, 0.0)],
        [(0.60, 1.10, -0.15), (0.00, 1.22, -0.32), (-0.65, 0.85, -0.18), (-1.40, 1.15, -0.34), (-2.20, 0.72, -0.18), (0.0, 0.0, 0.0)],
        [(0.68, 1.30, 0.32), (0.20, 1.52, 0.62), (-0.40, 1.15, 0.45), (-1.15, 1.48, 0.72), (-1.90, 1.02, 0.55), (-2.50, 1.28, 0.40)],
        [(0.68, 1.30, -0.32), (0.20, 1.52, -0.62), (-0.40, 1.15, -0.45), (-1.15, 1.48, -0.72), (-1.90, 1.02, -0.55), (-2.50, 1.28, -0.40)],
    ];
    for (idx, pts) in tresses.iter().enumerate() {
        // 6° e 7° hanno l'ultimo punto a (0,0,0): in Three.js i due array
        // hanno 5 punti. Se l'ultimo è zero lo salto.
        let real_pts: Vec<Vec3> = pts.iter()
            .filter(|p| !(p.0 == 0.0 && p.1 == 0.0 && p.2 == 0.0))
            .map(|(x, y, z)| local_pt(Vec3::new(*x, *y, *z), anchor))
            .collect();
        let r = if idx == 0 { 0.14 } else { 0.105 };
        tube(b.part(ElvenMaterial::Gold), &real_pts, r, 24, 7);
    }

    // 9. Braccia (2 tube su 4 punti).
    for side in [-1.0_f32, 1.0] {
        let pts = [
            Vec3::new(0.6, 0.88, side * 0.32),
            Vec3::new(0.0, 0.72, side * 0.62),
            Vec3::new(-0.9, 0.50, side * 0.78),
            Vec3::new(-1.9, 0.25, side * 0.65),
        ];
        let world: Vec<Vec3> = pts.iter().map(|p| local_pt(*p, anchor)).collect();
        tube(b.part(ElvenMaterial::Gold), &world, 0.095, 18, 8);
    }
}

fn local_pt(p: Vec3, anchor: Vec3) -> Vec3 {
    anchor + rot_z(p)
}

fn rot_z(p: Vec3) -> Vec3 {
    let c = FIG_ROT_Z.cos();
    let s = FIG_ROT_Z.sin();
    Vec3::new(p.x * c - p.y * s, p.x * s + p.y * c, p.z)
}

fn local_to_world(pts: &[Vec3], anchor: Vec3) -> Vec<Vec3> {
    pts.iter().map(|p| anchor + rot_z(*p)).collect()
}

fn cylinder_oriented(part: &mut Part, center: Vec3, r_bot: f32, r_top: f32, h: f32, sides: usize, q: Quat) {
    let base = part.positions.len();
    // `cylinder` costruisce con centro a ZERO: traslo in (0, 0, 0), poi
    // applico q e centro.
    cylinder(part, Vec3::ZERO, r_bot, r_top, h, sides, true);
    for i in base..part.positions.len() {
        let p = Vec3::from(part.positions[i]);
        part.positions[i] = (center + q * p).to_array();
        let n = Vec3::from(part.normals[i]);
        part.normals[i] = (q * n).to_array();
    }
}

fn sphere_scaled(part: &mut Part, center: Vec3, r: f32, scale: Vec3, lat: usize, lon: usize) {
    let base = part.positions.len();
    sphere(part, Vec3::ZERO, r, lat, lon);
    for i in base..part.positions.len() {
        let p = Vec3::from(part.positions[i]);
        part.positions[i] = (center + p * scale).to_array();
    }
}

// `quad` e `cyl` possono servire altrove: import mirati per non lasciare warning.
#[allow(unused_imports)]
use super::primitives::{cylinder as _cyl, quad as _quad};