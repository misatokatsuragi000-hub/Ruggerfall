//! Scafo del veliero elfico: le 19 stazioni `stDefs` del file Three.js
//! generano lower hull, deck (3 strisce: left/center/right), mid rail,
//! bulwark, top rail, stem trim. Tutte parti statiche.

use bevy::math::Vec3;

use super::super::ElvenMaterial;
use super::primitives::{quad, tube, MeshBuilder, Part};

/// Stazione dello scafo: `x` = asse longitudinale (prua = +x), `y_k` = chiglia,
/// `y_mid` = cinta, `y_top` = murata, `w_m` = semilarghezza alla cinta.
#[derive(Clone, Copy)]
pub struct StDef {
    pub x: f32,
    pub y_k: f32,
    pub y_mid: f32,
    pub y_top: f32,
    pub w_m: f32,
}

pub const ST_DEFS: [StDef; 19] = [
    StDef { x: -41.5, y_k: 16.5, y_mid: 22.0, y_top: 27.5, w_m: 0.25 },
    StDef { x: -41.0, y_k: 10.5, y_mid: 16.0, y_top: 21.5, w_m: 0.7 },
    StDef { x: -39.0, y_k:  5.5, y_mid: 10.8, y_top: 16.0, w_m: 1.4 },
    StDef { x: -35.0, y_k:  2.6, y_mid:  7.2, y_top: 12.0, w_m: 2.3 },
    StDef { x: -28.0, y_k:  1.2, y_mid:  5.1, y_top:  9.6, w_m: 3.5 },
    StDef { x: -20.0, y_k:  0.4, y_mid:  4.1, y_top:  7.8, w_m: 4.5 },
    StDef { x: -15.5, y_k:  0.1, y_mid:  3.6, y_top:  6.8, w_m: 5.0 },
    StDef { x: -12.5, y_k:  0.0, y_mid:  3.3, y_top:  6.5, w_m: 5.3 },
    StDef { x: -10.5, y_k: -0.1, y_mid:  3.1, y_top:  4.4, w_m: 5.5 },
    StDef { x:  -8.0, y_k: -0.2, y_mid:  2.9, y_top:  3.5, w_m: 5.6 },
    StDef { x:   0.0, y_k: -0.4, y_mid:  2.7, y_top:  3.3, w_m: 5.8 },
    StDef { x:   8.0, y_k: -0.2, y_mid:  2.9, y_top:  3.5, w_m: 5.6 },
    StDef { x:  10.5, y_k: -0.1, y_mid:  3.1, y_top:  4.4, w_m: 5.4 },
    StDef { x:  12.5, y_k:  0.0, y_mid:  3.3, y_top:  5.4, w_m: 5.1 },
    StDef { x:  18.0, y_k:  0.0, y_mid:  3.4, y_top:  5.4, w_m: 4.6 },
    StDef { x:  26.0, y_k:  0.0, y_mid:  3.4, y_top:  5.4, w_m: 3.8 },
    StDef { x:  34.0, y_k:  0.0, y_mid:  3.4, y_top:  5.4, w_m: 2.5 },
    StDef { x:  40.0, y_k:  0.0, y_mid:  3.4, y_top:  5.4, w_m: 0.9 },
    StDef { x:  42.5, y_k:  0.0, y_mid:  3.4, y_top:  5.4, w_m: 0.0 },
];

pub fn build(b: &mut MeshBuilder) {
    lower_hull(b);
    deck(b);
    mid_rail(b);
    bulwark(b);
    top_rail(b);
    stem_trim(b);
}

/// 4 slice verticali per ogni stazione (chiglia, 32%, 72%, cinta) × 2 lati.
fn lower_hull(b: &mut MeshBuilder) {
    let part = b.part(ElvenMaterial::Ivory);
    const SLICES: usize = 4;
    for s in 0..ST_DEFS.len() - 1 {
        let a = ST_DEFS[s];
        let c = ST_DEFS[s + 1];
        for side in [-1.0_f32, 1.0] {
            for k in 0..SLICES - 1 {
                let u0 = k as f32 / (SLICES - 1) as f32;
                let u1 = (k + 1) as f32 / (SLICES - 1) as f32;
                let p = |st: StDef, u: f32| -> Vec3 {
                    // profilo laterale: y_k + (y_mid - y_k)*u^0.8
                    let dy = st.y_mid - st.y_k;
                    Vec3::new(st.x, st.y_k + dy * u.powf(0.8), side * st.w_m * u)
                };
                let p0 = p(a, u0);
                let p1 = p(c, u0);
                let p2 = p(c, u1);
                let p3 = p(a, u1);
                if side < 0.0 {
                    quad(part, p0, p3, p2, p1);
                } else {
                    quad(part, p0, p1, p2, p3);
                }
            }
        }
    }
}

/// Deck: 3 curve lungo X (left / center / right) unite da strisce di quad.
fn deck(b: &mut MeshBuilder) {
    let part = b.part(ElvenMaterial::DeckWood);
    const STEPS: usize = 60;
    let mut left = Vec::with_capacity(STEPS + 1);
    let mut center = Vec::with_capacity(STEPS + 1);
    let mut right = Vec::with_capacity(STEPS + 1);
    // Campiona le 3 curve con Catmull-Rom per (x → y, z).
    for k in 0..=STEPS {
        let t = k as f32 / STEPS as f32 * (ST_DEFS.len() - 1) as f32;
        let i = (t.floor() as usize).min(ST_DEFS.len() - 2);
        let u = t - i as f32;
        let a = ST_DEFS[i];
        let c = ST_DEFS[i + 1];
        let y = a.y_mid + (c.y_mid - a.y_mid) * u;
        let w = a.w_m + (c.w_m - a.w_m) * u;
        let x = a.x + (c.x - a.x) * u;
        center.push(Vec3::new(x, y + if w > 0.1 { 0.1 } else { 0.0 }, 0.0));
        left.push(Vec3::new(x, y, -w));
        right.push(Vec3::new(x, y, w));
    }
    for k in 0..STEPS {
        // left → center
        quad(part, left[k], left[k + 1], center[k + 1], center[k]);
        // center → right
        quad(part, center[k], center[k + 1], right[k + 1], right[k]);
    }
}

fn mid_rail(b: &mut MeshBuilder) {
    let part = b.part(ElvenMaterial::Gold);
    for side in [-1.0_f32, 1.0] {
        let pts: Vec<Vec3> = ST_DEFS
            .iter()
            .map(|st| Vec3::new(st.x, st.y_mid, side * st.w_m))
            .collect();
        tube(part, &pts, 0.16, 80, 8);
    }
}

fn bulwark(b: &mut MeshBuilder) {
    let part = b.part(ElvenMaterial::Ivory);
    for s in 0..ST_DEFS.len() - 1 {
        let a = ST_DEFS[s];
        let c = ST_DEFS[s + 1];
        for side in [-1.0_f32, 1.0] {
            let p0 = Vec3::new(a.x, a.y_mid, side * a.w_m);
            let p1 = Vec3::new(c.x, c.y_mid, side * c.w_m);
            let p2 = Vec3::new(c.x, c.y_top, side * (c.w_m * 0.96));
            let p3 = Vec3::new(a.x, a.y_top, side * (a.w_m * 0.96));
            if side < 0.0 {
                quad(part, p0, p3, p2, p1);
            } else {
                quad(part, p0, p1, p2, p3);
            }
        }
    }
}

fn top_rail(b: &mut MeshBuilder) {
    let part = b.part(ElvenMaterial::Gold);
    for side in [-1.0_f32, 1.0] {
        let pts: Vec<Vec3> = ST_DEFS
            .iter()
            .map(|st| Vec3::new(st.x, st.y_top + 0.08, side * st.w_m * 0.96))
            .collect();
        tube(part, &pts, 0.16, 80, 8);
    }
}

fn stem_trim(b: &mut MeshBuilder) {
    let part = b.part(ElvenMaterial::Gold);
    let pts = [Vec3::new(42.5, 0.0, 0.0), Vec3::new(42.5, 5.48, 0.0)];
    tube(part, &pts, 0.16, 8, 8);
}

// =============================================================================
// Utility per le altre parti: interpolazione stDefs a una certa altezza
// =============================================================================

pub fn wall_point_at_height(x_pos: f32, side: f32, h_frac: f32) -> Vec3 {
    let mut a = ST_DEFS[0];
    let mut c = ST_DEFS[1];
    for i in 0..ST_DEFS.len() - 1 {
        if x_pos >= ST_DEFS[i].x && x_pos <= ST_DEFS[i + 1].x {
            a = ST_DEFS[i];
            c = ST_DEFS[i + 1];
            break;
        }
    }
    let t = ((x_pos - a.x) / (c.x - a.x)).clamp(0.0, 1.0);
    let y_mid = a.y_mid + (c.y_mid - a.y_mid) * t;
    let y_top = a.y_top + (c.y_top - a.y_top) * t;
    let w = a.w_m + (c.w_m - a.w_m) * t;
    Vec3::new(x_pos, y_mid + (y_top - y_mid) * h_frac, side * w * (1.0 - 0.04 * h_frac))
}

pub fn bulwark_up(x_pos: f32, side: f32) -> Vec3 {
    let mut a = ST_DEFS[0];
    let mut c = ST_DEFS[1];
    for i in 0..ST_DEFS.len() - 1 {
        if x_pos >= ST_DEFS[i].x && x_pos <= ST_DEFS[i + 1].x {
            a = ST_DEFS[i];
            c = ST_DEFS[i + 1];
            break;
        }
    }
    let t = ((x_pos - a.x) / (c.x - a.x)).clamp(0.0, 1.0);
    let y_mid = a.y_mid + (c.y_mid - a.y_mid) * t;
    let y_top = a.y_top + (c.y_top - a.y_top) * t;
    let w = a.w_m + (c.w_m - a.w_m) * t;
    Vec3::new(0.0, y_top - y_mid, side * (w * 0.96 - w)).normalize_or_zero()
}