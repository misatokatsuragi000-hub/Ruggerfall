//! Scudo "kite" con croce dorata, rubino centrale in cornice romboidale e
//! quattro punte coniche. `createKiteShield` di Three.js.
//!
//! Geometria del corpo:
//! - metà larghezza parametrica `getHalfWidth(y)`: sopra la spalla è un arco
//!   circolare, sotto è una punta a `pow(t, 0.82)`;
//! - griglia `rows × cols` con `camber` in Z per la bombatura;
//! - bordo dorato come `tube` chiuso (44 punti), croce verticale + orizzontale
//!   curve, 4 punte coniche, rubino frontale con cornice romboidale.

use bevy::math::Quat;
use bevy::prelude::Vec3;

use super::super::ElvenMaterial;
use super::decorations::{gem_pyramid, rhomboidal_bezel};
use super::primitives::{cone, quad, sphere, tri, tube, MeshBuilder, Part};

// Costanti dello scudo (dal Three.js).
const SW: f32 = 0.26;
const Y_TOP: f32 = 0.50;
const Y_SH: f32 = 0.34;
const Y_BOT: f32 = -0.55;
const CAMBER: f32 = 0.075;
const SHIELD_SCALE: f32 = 1.5;

/// Metà larghezza del corpo dello scudo per la data quota locale `y`.
fn half_width(y: f32) -> f32 {
    if y > Y_SH {
        let cap_h = Y_TOP - Y_SH;
        let dy = y - Y_SH;
        SW * (1.0 - (dy * dy) / (cap_h * cap_h)).max(0.0).sqrt()
    } else {
        let t = ((y - Y_BOT) / (Y_SH - Y_BOT)).max(0.0);
        SW * t.powf(0.82)
    }
}

fn apply_to_part(part: &mut Part, base: usize, center: Vec3, yaw: f32) {
    let q = Quat::from_rotation_y(yaw);
    let s = SHIELD_SCALE;
    for i in base..part.positions.len() {
        let p = Vec3::from(part.positions[i]) * s;
        part.positions[i] = (center + q * p).to_array();
        let n = Vec3::from(part.normals[i]);
        part.normals[i] = (q * n).to_array();
    }
}

/// Costruisce un singolo scudo in coordinate locali e poi lo trasla/ruota.
/// `center` è il punto di attacco sulla fiancata, `yaw` l'orientamento (la
/// normale locale dello scudo punta verso +Z prima della rotazione).
pub fn kite_shield(b: &mut MeshBuilder, center: Vec3, yaw: f32) {
    // ============================================================
    // 1. Corpo parametrico
    // ============================================================
    const ROWS: usize = 12;
    const COLS: usize = 8;
    {
        let base = b.part(ElvenMaterial::Ivory).positions.len();
        let part = b.part(ElvenMaterial::Ivory);
        for j in 0..ROWS {
            for i in 0..COLS {
                let v0 = j as f32 / ROWS as f32;
                let v1 = (j + 1) as f32 / ROWS as f32;
                let u0 = -1.0 + 2.0 * (i as f32 / COLS as f32);
                let u1 = -1.0 + 2.0 * ((i + 1) as f32 / COLS as f32);
                let p = |u: f32, v: f32| -> Vec3 {
                    let y = Y_BOT + (Y_TOP - Y_BOT) * v;
                    let w = half_width(y);
                    Vec3::new(u * w, y, CAMBER * (1.0 - u * u))
                };
                quad(part, p(u0, v0), p(u1, v0), p(u1, v1), p(u0, v1));
            }
        }
        apply_to_part(part, base, center, yaw);
    }

    // ============================================================
    // 2. Bordo dorato (tube chiuso, 44 punti)
    // ============================================================
    {
        let base = b.part(ElvenMaterial::Gold).positions.len();
        let part = b.part(ElvenMaterial::Gold);
        let steps = 44;
        let mut pts = Vec::with_capacity(steps + 1);
        for k in 0..=steps {
            let t = (k as f32 / steps as f32) * std::f32::consts::TAU;
            let y = if t <= std::f32::consts::PI {
                let u = t / std::f32::consts::PI;
                Y_TOP - (Y_TOP - Y_BOT) * u
            } else {
                let u = (t - std::f32::consts::PI) / std::f32::consts::PI;
                Y_BOT + (Y_TOP - Y_BOT) * u
            };
            let x = if t <= std::f32::consts::PI {
                half_width(y)
            } else {
                -half_width(y)
            };
            pts.push(Vec3::new(x, y, 0.006));
        }
        tube(part, &pts, 0.020, steps, 6);
        apply_to_part(part, base, center, yaw);
    }

    // ============================================================
    // 3. Croce verticale + orizzontale + 4 punte
    // ============================================================
    let y_cross = 0.06;
    {
        let base = b.part(ElvenMaterial::Gold).positions.len();
        let part = b.part(ElvenMaterial::Gold);
        // Croce verticale: box sottile
        push_box(part, Vec3::new(0.0, y_cross, CAMBER + 0.015), Vec3::new(0.062, 0.82, 0.032), 0.0);
        // Punte coniche sopra e sotto (a 4 lati)
        for (dy, flip) in [(0.41, false), (-0.41, true)] {
            let c = Vec3::new(0.0, y_cross + dy, CAMBER + 0.015);
            let tip_dir = if flip { -1.0 } else { 1.0 };
            push_pyramid_4(part, c, 0.045, 0.08, tip_dir);
        }
        apply_to_part(part, base, center, yaw);
    }
    // Croce orizzontale curva
    {
        let base = b.part(ElvenMaterial::Gold).positions.len();
        let part = b.part(ElvenMaterial::Gold);
        let w_cross = half_width(y_cross) * 1.65;
        let steps = 8;
        let mut pts = Vec::with_capacity(steps + 1);
        for k in 0..=steps {
            let x = -w_cross * 0.5 + w_cross * (k as f32 / steps as f32);
            let z = (CAMBER * (1.0 - (x / (SW * 0.95)) * (x / (SW * 0.95))) + 0.015).max(0.01);
            pts.push(Vec3::new(x, y_cross, z));
        }
        tube(part, &pts, 0.022, steps, 6);
        apply_to_part(part, base, center, yaw);
    }
    // Punte coniche a destra/sinistra
    {
        let base = b.part(ElvenMaterial::Gold).positions.len();
        let part = b.part(ElvenMaterial::Gold);
        let w_cross = half_width(y_cross) * 1.65;
        for (dx, flip) in [(w_cross * 0.5, false), (-w_cross * 0.5, true)] {
            let c = Vec3::new(dx, y_cross, CAMBER * 0.35 + 0.015);
            push_pyramid_4(part, c, 0.045, 0.08, 0.0); // asse +Z
            let _ = flip;
        }
        apply_to_part(part, base, center, yaw);
    }

    // ============================================================
    // 4. Rubino frontale (cornice + piastra + gemma)
    // ============================================================
    // Cornice romboidale.
    rhomboidal_bezel(
        b,
        center + Quat::from_rotation_y(yaw) * (Vec3::new(0.0, y_cross, CAMBER + 0.030) * SHIELD_SCALE),
        yaw,
        0.115 * SHIELD_SCALE,
        0.130 * SHIELD_SCALE,
        0.026 * SHIELD_SCALE,
        0.028 * SHIELD_SCALE,
    );
    // Piastra dorata dietro (ottaedro schiacciato).
    {
        let base = b.part(ElvenMaterial::Gold).positions.len();
        let part = b.part(ElvenMaterial::Gold);
        let r = 0.085;
        let ry = r * 1.25;
        let rz = r * 0.20;
        let ring = [
            Vec3::new(-r, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -rz),
            Vec3::new(r, 0.0, 0.0),
            Vec3::new(0.0, 0.0, rz),
        ];
        let top = Vec3::new(0.0, ry, 0.0);
        let bot = Vec3::new(0.0, -ry, 0.0);
        for k in 0..4 {
            tri(part, ring[k], ring[(k + 1) % 4], top, [1.0; 4], [0.0, 0.0], [1.0, 0.0], [0.5, 1.0]);
            tri(part, ring[(k + 1) % 4], ring[k], bot, [1.0; 4], [0.0, 0.0], [1.0, 0.0], [0.5, 1.0]);
        }
        let local = Vec3::new(0.0, y_cross, CAMBER + 0.018);
        apply_to_part(part, base, center + Quat::from_rotation_y(yaw) * (local * SHIELD_SCALE), yaw);
    }
    // Gemma rubino (piramide con punta verso +Z locale).
    gem_pyramid(
        b,
        ElvenMaterial::Ruby,
        center + Quat::from_rotation_y(yaw) * (Vec3::new(0.0, y_cross, CAMBER + 0.030 + 0.040) * SHIELD_SCALE),
        yaw,
        0.080 * SHIELD_SCALE,
        0.10 * SHIELD_SCALE,
    );
}

/// Costruisce un box a 6 facce centrato in `center` con mezza-dimensione
/// `half`, ruotato di `yaw` attorno a Y.
fn push_box(part: &mut Part, center: Vec3, half: Vec3, yaw: f32) {
    let q = Quat::from_rotation_y(yaw);
    let corners = [
        Vec3::new(-half.x, -half.y, -half.z),
        Vec3::new( half.x, -half.y, -half.z),
        Vec3::new( half.x,  half.y, -half.z),
        Vec3::new(-half.x,  half.y, -half.z),
        Vec3::new(-half.x, -half.y,  half.z),
        Vec3::new( half.x, -half.y,  half.z),
        Vec3::new( half.x,  half.y,  half.z),
        Vec3::new(-half.x,  half.y,  half.z),
    ];
    let c = |i: usize| center + q * corners[i];
    // Z+ / Z-
    quad(part, c(4), c(5), c(6), c(7));
    quad(part, c(1), c(0), c(3), c(2));
    // X+ / X-
    quad(part, c(5), c(1), c(2), c(6));
    quad(part, c(0), c(4), c(7), c(3));
    // Y+ / Y-
    quad(part, c(7), c(6), c(2), c(3));
    quad(part, c(0), c(1), c(5), c(4));
}

/// Piramide a 4 lati allineata a +Y (o -Y se `sign<0`) o a +Z (sign=0).
fn push_pyramid_4(part: &mut Part, center: Vec3, r: f32, h: f32, sign: f32) {
    let apex;
    let ring;
    if sign.abs() > 0.5 {
        apex = Vec3::new(0.0, sign * h * 0.5, 0.0);
        ring = [
            Vec3::new(-r, -sign * h * 0.5, 0.0),
            Vec3::new(0.0, -sign * h * 0.5, -r),
            Vec3::new(r, -sign * h * 0.5, 0.0),
            Vec3::new(0.0, -sign * h * 0.5, r),
        ];
    } else {
        apex = Vec3::new(0.0, 0.0, h * 0.5);
        ring = [
            Vec3::new(-r, 0.0, -h * 0.5),
            Vec3::new(0.0, r, -h * 0.5),
            Vec3::new(r, 0.0, -h * 0.5),
            Vec3::new(0.0, -r, -h * 0.5),
        ];
    }
    for k in 0..4 {
        let a = center + ring[k];
        let b = center + ring[(k + 1) % 4];
        let t = center + apex;
        tri(part, a, b, t, [1.0; 4], [0.0, 0.0], [1.0, 0.0], [0.5, 1.0]);
    }
}

// =============================================================================
// Entry point
// =============================================================================

pub fn build(b: &mut MeshBuilder) {
    // Scudi lungo le fiancate. Le posizioni sono quelle del Three.js:
    // - fascia esterna normale (13.9..34 e -33.6..-13.5), h 0.78;
    // - fascia centrale (davanti alle baliste, [-8, -4, 0, 4, 8]), h 0.82,
    //   sporgenti 0.38 (fuori 0.26 nel normale).
    //
    // La normale della fiancata viene dalla geometria locale della nave
    // (ST_DEFS): calcolo tan = ahead-behind, up = bulwark_up, n = tan × up.
    // Uso helper locali qui sotto invece di chiamare hull perché serve anche
    // il punto medio in altezza.
    use super::hull::{bulwark_up, wall_point_at_height};

    for side in [-1.0_f32, 1.0] {
        let mut positions: Vec<(f32, bool)> = Vec::new();
        let mut x = 13.9;
        while x <= 34.0 + 1e-3 {
            positions.push((x, false));
            x += 2.8;
        }
        let mut x = -33.6;
        while x <= -13.5 + 1e-3 {
            positions.push((x, false));
            x += 2.8;
        }
        for bx in [-8.0_f32, -4.0, 0.0, 4.0, 8.0] {
            positions.push((bx, true));
        }

        for (x, central) in positions {
            let h_frac = if central { 0.82 } else { 0.78 };
            let wall_pt = wall_point_at_height(x, side, h_frac);
            let ahead = wall_point_at_height(x + 0.3, side, h_frac);
            let behind = wall_point_at_height(x - 0.3, side, h_frac);
            let v_tan = (ahead - behind).normalize_or_zero();
            let v_up = bulwark_up(x, side);
            let v_n = v_tan.cross(v_up).normalize_or_zero() * side;

            let out_dist = if central { 0.38 } else { 0.26 };
            let anchor = wall_pt + v_n * out_dist;
            // La normale dello scudo (locale +Z) deve puntare verso v_n.
            let yaw = v_n.x.atan2(v_n.z);
            kite_shield(b, anchor, yaw);
        }
    }

    // Controllo di qualità: usa `sphere` solo per non lasciare warning
    // di import inutilizzato; in realtà non serve qui.
    let _ = sphere;
}