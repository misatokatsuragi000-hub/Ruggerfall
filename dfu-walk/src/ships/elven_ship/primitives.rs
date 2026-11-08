//! Helper per costruire mesh Bevy direttamente in `WorldMesh`, invece di
//! passare per `Mesh`. `MeshBuilder` accumula vertici in un `HashMap`
//! per materiale, e `finish` produce la `BuiltShip` finale (una `Mesh` per
//! submesh + un `Collider::trimesh` per la fisica).
//!
//! ## Funzioni chiave
//!
//! - `Part`: triangoli con posizione/normale/uv, indici locali.
//! - `MeshBuilder`: accumula `Part` per `ElvenMaterial`.
//! - `tri()`: aggiunge un triangolo 3D.
//! - `quad()`: aggiunge un quadrilatero (2 triangoli).
//! - `cylinder()`, `cone()`, `sphere()`, `torus()`: primitive semplici.
//! - `tube(curve, radius, segs)`: estruso di un cerchio lungo una Catmull-Rom.
//! - `extrude_polygon(poly, depth)`: estrude un poligono 2D (porte, merli,
//!   scudi, gemme). La triangolazione usa ear-clipping semplice su poligoni
//!   convessi/semiconvessi; se serve un poligono concavo, triangola prima.
//! - `prism_frustum()`: per i tronchi di cono delle torri.

use bevy::math::Vec3;
use bevy::prelude::*;
use bevy_rapier3d::prelude::Collider;

use super::super::{AnimatedPart, BuiltShip, ElvenMaterial};
use crate::data::mesh_builder::{Submesh, WorldMesh};

#[derive(Default, Clone)]
pub struct Part {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

pub struct MeshBuilder {
    parts: std::collections::HashMap<ElvenMaterial, Part>,
    animated: Vec<AnimatedPart>,
    deck_height: f32,
    helm_local: Vec3,
}

impl MeshBuilder {
    pub fn new() -> Self {
        Self {
            parts: Default::default(),
            animated: Default::default(),
            deck_height: 3.3,
            helm_local: Vec3::new(0.42, 10.58, 0.0),
        }
    }

    pub fn part(&mut self, m: ElvenMaterial) -> &mut Part {
        self.parts.entry(m).or_default()
    }

    pub fn push_animated(&mut self, a: AnimatedPart) {
        self.animated.push(a);
    }

    pub fn finish(self, animated: Vec<AnimatedPart>) -> BuiltShip {
        let mut mesh = WorldMesh::new();
        let mut submeshes: Vec<(ElvenMaterial, Submesh)> = Vec::new();

        for (mat, part) in self.parts {
            if part.indices.is_empty() {
                continue;
            }
            let base = mesh.vertices.len() as u32;
            mesh.vertices.extend_from_slice(&part.positions);
            mesh.normals.extend_from_slice(&part.normals);
            mesh.uvs.extend_from_slice(&part.uvs);
            let idxs: Vec<u32> = part.indices.iter().map(|i| base + i).collect();
            submeshes.push((
                mat,
                Submesh {
                    texture_archive: 0,
                    texture_record: 0,
                    indices: idxs,
                },
            ));
        }

        // Collider: tutti i triangoli indipendentemente dal materiale.
        let mut vertices = Vec::with_capacity(mesh.vertices.len());
        let mut indices = Vec::with_capacity(mesh.vertices.len() / 2);
        for (_, sub) in &submeshes {
            for ch in sub.indices.chunks_exact(3) {
                indices.push([ch[0], ch[1], ch[2]]);
            }
        }
        for v in &mesh.vertices {
            vertices.push(Vec3::from(*v));
        }
        let collider = if !vertices.is_empty() && !indices.is_empty() {
            Some(Collider::trimesh(vertices, indices))
        } else {
            None
        };

        let mut all_animated = self.animated;
        all_animated.extend(animated);

        BuiltShip {
            submeshes,
            mesh,
            collider,
            deck_height: self.deck_height,
            helm_local: self.helm_local,
            animated: all_animated,
        }
    }
}

// =============================================================================
// Aggiunta di triangoli / quadrilateri
// =============================================================================

pub fn tri(part: &mut Part, a: Vec3, b: Vec3, c: Vec3, color: [f32; 4], uv_a: [f32; 2], uv_b: [f32; 2], uv_c: [f32; 2]) {
    let n = (b - a).cross(c - a).normalize_or_zero();
    let base = part.positions.len() as u32;
    part.positions.push(a.to_array());
    part.normals.push(n.to_array());
    part.uvs.push(uv_a);
    part.positions.push(b.to_array());
    part.normals.push(n.to_array());
    part.uvs.push(uv_b);
    part.positions.push(c.to_array());
    part.normals.push(n.to_array());
    part.uvs.push(uv_c);
    part.indices.extend_from_slice(&[base, base + 1, base + 2]);
    let _ = color; // riservato per tint per-vertex
}

pub fn quad(part: &mut Part, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
    tri(part, a, b, c, [1.0; 4], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]);
    tri(part, a, c, d, [1.0; 4], [0.0, 0.0], [1.0, 1.0], [0.0, 1.0]);
}

// =============================================================================
// Primitive (equivalente delle *Geometry di Three.js)
// =============================================================================

/// Cilindro verticale centrato su `center`, raggio `r_bot` in basso, `r_top` in
/// alto, altezza `h`. `sides` = numero di facce laterali. `caps = false` salta
/// le due basi (utile per le torri che vengono chiuse dal tetto).
pub fn cylinder(part: &mut Part, center: Vec3, r_bot: f32, r_top: f32, h: f32, sides: usize, caps: bool) {
    let hh = h * 0.5;
    for k in 0..sides {
        let a0 = (k as f32 / sides as f32) * std::f32::consts::TAU;
        let a1 = ((k + 1) as f32 / sides as f32) * std::f32::consts::TAU;
        let (s0, c0) = a0.sin_cos();
        let (s1, c1) = a1.sin_cos();
        let p00 = center + Vec3::new(c0 * r_bot, -hh, s0 * r_bot);
        let p01 = center + Vec3::new(c1 * r_bot, -hh, s1 * r_bot);
        let p10 = center + Vec3::new(c0 * r_top, hh, s0 * r_top);
        let p11 = center + Vec3::new(c1 * r_top, hh, s1 * r_top);
        quad(part, p00, p01, p11, p10);
    }
    if caps {
        let bot = center + Vec3::new(0.0, -hh, 0.0);
        let top = center + Vec3::new(0.0, hh, 0.0);
        for k in 0..sides {
            let a0 = (k as f32 / sides as f32) * std::f32::consts::TAU;
            let a1 = ((k + 1) as f32 / sides as f32) * std::f32::consts::TAU;
            let (s0, c0) = a0.sin_cos();
            let (s1, c1) = a1.sin_cos();
            let pb0 = center + Vec3::new(c0 * r_bot, -hh, s0 * r_bot);
            let pb1 = center + Vec3::new(c1 * r_bot, -hh, s1 * r_bot);
            tri(part, bot, pb1, pb0, [1.0; 4], [0.0, 0.0], [1.0, 0.0], [0.0, 1.0]);
            let pt0 = center + Vec3::new(c0 * r_top, hh, s0 * r_top);
            let pt1 = center + Vec3::new(c1 * r_top, hh, s1 * r_top);
            tri(part, top, pt0, pt1, [1.0; 4], [0.0, 0.0], [1.0, 0.0], [0.0, 1.0]);
        }
    }
}

/// Cono verticale con la punta verso +Y.
pub fn cone(part: &mut Part, base_center: Vec3, r: f32, h: f32, sides: usize) {
    let apex = base_center + Vec3::Y * h;
    for k in 0..sides {
        let a0 = (k as f32 / sides as f32) * std::f32::consts::TAU;
        let a1 = ((k + 1) as f32 / sides as f32) * std::f32::consts::TAU;
        let (s0, c0) = a0.sin_cos();
        let (s1, c1) = a1.sin_cos();
        let p0 = base_center + Vec3::new(c0 * r, 0.0, s0 * r);
        let p1 = base_center + Vec3::new(c1 * r, 0.0, s1 * r);
        tri(part, p0, p1, apex, [1.0; 4], [0.0, 1.0], [1.0, 1.0], [0.5, 0.0]);
        tri(part, base_center, p1, p0, [1.0; 4], [0.5, 0.5], [1.0, 0.0], [0.0, 0.0]);
    }
}

/// Sfera a basso numero di poligoni (solo latitudini/longitudini).
pub fn sphere(part: &mut Part, center: Vec3, r: f32, lat: usize, lon: usize) {
    for i in 0..=lat {
        let theta0 = std::f32::consts::PI * i as f32 / lat as f32;
        let theta1 = std::f32::consts::PI * (i + 1) as f32 / lat as f32;
        for j in 0..=lon {
            let phi0 = std::f32::consts::TAU * j as f32 / lon as f32;
            let phi1 = std::f32::consts::TAU * (j + 1) as f32 / lon as f32;
            let p = |theta: f32, phi: f32| {
                center
                    + Vec3::new(theta.sin() * phi.cos(), theta.cos(), theta.sin() * phi.sin()) * r
            };
            if i < lat && j < lon {
                let a = p(theta0, phi0);
                let b = p(theta1, phi0);
                let c = p(theta1, phi1);
                let d = p(theta0, phi1);
                quad(part, a, b, c, d);
            }
        }
    }
}

/// Toro orizzontale (asse di rotazione = Y), come `THREE.TorusGeometry`
/// ruotato di 90° su X. `r_inner` = raggio del tubo, `r_outer` = raggio del
/// cerchio grande (che Three chiama "radius"). `segs` = passi attorno al
/// cerchio grande, `tube_segs` = passi del tubo.
pub fn torus_horizontal(part: &mut Part, center: Vec3, r_inner: f32, r_outer: f32, segs: usize, tube_segs: usize) {
    for i in 0..segs {
        let u0 = (i as f32 / segs as f32) * std::f32::consts::TAU;
        let u1 = ((i + 1) as f32 / segs as f32) * std::f32::consts::TAU;
        for j in 0..tube_segs {
            let v0 = (j as f32 / tube_segs as f32) * std::f32::consts::TAU;
            let v1 = ((j + 1) as f32 / tube_segs as f32) * std::f32::consts::TAU;
            let p = |u: f32, v: f32| -> Vec3 {
                let rr = r_outer + r_inner * v.cos();
                center + Vec3::new(rr * u.cos(), r_inner * v.sin(), rr * u.sin())
            };
            quad(part, p(u0, v0), p(u1, v0), p(u1, v1), p(u0, v1));
        }
    }
}

/// Tubo lungo una curva Catmull-Rom. `pts` = punti di controllo, `r` = raggio,
/// `segs` = passi lungo la curva, `tube_segs` = passi attorno al cerchio.
pub fn tube(part: &mut Part, pts: &[Vec3], r: f32, segs: usize, tube_segs: usize) {
    if pts.len() < 2 { return; }
    // Campiona la curva a `segs+1` punti con Catmull-Rom uniforme.
    let sampled = sample_catmull_rom(pts, segs);
    for k in 0..segs {
        let p0 = sampled[k];
        let p1 = sampled[k + 1];
        let t = (p1 - p0).normalize_or_zero();
        if t == Vec3::ZERO { continue; }
        let helper = if t.y.abs() < 0.95 { Vec3::Y } else { Vec3::X };
        let n = t.cross(helper).normalize_or_zero();
        let b = n.cross(t).normalize_or_zero();
        for j in 0..tube_segs {
            let v0 = (j as f32 / tube_segs as f32) * std::f32::consts::TAU;
            let v1 = ((j + 1) as f32 / tube_segs as f32) * std::f32::consts::TAU;
            let ring = |c: Vec3, v: f32| c + (n * v.cos() + b * v.sin()) * r;
            quad(part, ring(p0, v0), ring(p1, v0), ring(p1, v1), ring(p0, v1));
        }
    }
}

fn sample_catmull_rom(pts: &[Vec3], segs: usize) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(segs + 1);
    let n = pts.len();
    for k in 0..=segs {
        let t = k as f32 / segs as f32 * (n - 1) as f32;
        let i = (t.floor() as usize).min(n - 2);
        let u = t - i as f32;
        let p0 = if i == 0 { pts[0] } else { pts[i - 1] };
        let p1 = pts[i];
        let p2 = pts[i + 1];
        let p3 = if i + 2 < n { pts[i + 2] } else { pts[n - 1] };
        let p = 0.5 * ((2.0 * p1) + (-p0 + p2) * u + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * u * u + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * u * u * u);
        out.push(p);
    }
    out
}

/// Estrude un poligono 2D (piano XY, normale +Z) di profondità `depth` lungo
/// Z. `poly` deve essere **semplice** (nessuna auto-intersezione) e in senso
/// antiorario. Triangolazione a ventaglio (convessa) o ear-clipping
/// (concava).
pub fn extrude_polygon(part: &mut Part, poly: &[[f32; 2]], depth: f32) {
    if poly.len() < 3 { return; }
    let z0 = -depth * 0.5;
    let z1 = depth * 0.5;
    let tris = triangulate_polygon(poly);
    // Fronte (+Z).
    for [a, b, c] in &tris {
        let pa = Vec3::new(poly[*a][0], poly[*a][1], z1);
        let pb = Vec3::new(poly[*b][0], poly[*b][1], z1);
        let pc = Vec3::new(poly[*c][0], poly[*c][1], z1);
        tri(part, pa, pb, pc, [1.0; 4], poly[*a], poly[*b], poly[*c]);
    }
    // Retro (-Z), invertito.
    for [a, b, c] in &tris {
        let pa = Vec3::new(poly[*a][0], poly[*a][1], z0);
        let pb = Vec3::new(poly[*b][0], poly[*b][1], z0);
        let pc = Vec3::new(poly[*c][0], poly[*c][1], z0);
        tri(part, pa, pc, pb, [1.0; 4], poly[*a], poly[*c], poly[*b]);
    }
    // Lati.
    for k in 0..poly.len() {
        let a = poly[k];
        let b = poly[(k + 1) % poly.len()];
        let p0 = Vec3::new(a[0], a[1], z0);
        let p1 = Vec3::new(b[0], b[1], z0);
        let p2 = Vec3::new(b[0], b[1], z1);
        let p3 = Vec3::new(a[0], a[1], z1);
        quad(part, p0, p1, p2, p3);
    }
}

fn triangulate_polygon(poly: &[[f32; 2]]) -> Vec<[usize; 3]> {
    // Ear clipping. Funziona per poligoni semplici (convessi e concavi).
    let n = poly.len();
    if n < 3 { return vec![]; }
    let mut idx: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(n - 2);
    let is_convex = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]) > 0.0;
    let point_in_tri = |p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
        let d1 = (p[0] - b[0]) * (a[1] - b[1]) - (a[0] - b[0]) * (p[1] - b[1]);
        let d2 = (p[0] - c[0]) * (b[1] - c[1]) - (b[0] - c[0]) * (p[1] - c[1]);
        let d3 = (p[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (p[1] - a[1]);
        let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(neg && pos)
    };
    let mut guard = 0;
    while idx.len() > 3 && guard < idx.len() * 3 {
        guard += 1;
        let m = idx.len();
        let mut clipped = false;
        for k in 0..m {
            let i0 = idx[(k + m - 1) % m];
            let i1 = idx[k];
            let i2 = idx[(k + 1) % m];
            if !is_convex(poly[i0], poly[i1], poly[i2]) { continue; }
            if idx.iter().any(|&j| j != i0 && j != i1 && j != i2 && point_in_tri(poly[j], poly[i0], poly[i1], poly[i2])) {
                continue;
            }
            out.push([i0, i1, i2]);
            idx.remove(k);
            clipped = true;
            break;
        }
        if !clipped { break; }
    }
    if idx.len() == 3 {
        out.push([idx[0], idx[1], idx[2]]);
    }
    out
}