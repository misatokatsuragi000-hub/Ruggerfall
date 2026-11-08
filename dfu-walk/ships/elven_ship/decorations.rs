//! Porte, gemme, merli, parapetti, finestre, dormer e torri del veliero
//! elfico. Raccoglie in un unico modulo le funzioni che in Three.js erano
//! sparse (`createExtrudedElvenDoor`, `createRhomboidalBezel`,
//! `createCastleWindow`, `createDormerWindow`, `placeDormerOnRoof`,
//! `createClassicMerlon`, `createParapetWithDenseMerlons`, e i gruppi
//! `pCent`, `citadel`, `foreMastBase`, `mastList`).
//!
//! ## Trasformazione locale → mondo
//!
//! `MeshBuilder` accumula vertici già in coordinate mondo. Le funzioni di
//! questo modulo costruiscono i pezzi in coordinate locali (porta centrata
//! sull'origine, merlo alla base, finestra piatta...) e poi applicano una
//! traslazione + rotazione Y tramite `apply_transform`. Stesso schema del
//! `Group.position` + `Group.rotation.y` di Three.js.

use bevy::math::{Quat, Vec3};

use super::super::ElvenMaterial;
use super::hull::{bulwark_up, wall_point_at_height, StDef, ST_DEFS};
use super::primitives::{
    cone, cylinder, extrude_polygon, quad, sphere, torus_horizontal, tri, tube, MeshBuilder, Part,
};

// =============================================================================
// Trasformazione locale → mondo
// =============================================================================

fn apply_transform(part: &mut Part, base: usize, center: Vec3, yaw: f32) {
    let q = Quat::from_rotation_y(yaw);
    for i in base..part.positions.len() {
        let p = Vec3::from(part.positions[i]);
        part.positions[i] = (center + q * p).to_array();
        let n = Vec3::from(part.normals[i]);
        part.normals[i] = (q * n).to_array();
    }
}

// =============================================================================
// Entry point
// =============================================================================

pub fn build(b: &mut MeshBuilder) {
    build_towers(b);
    build_doors_on_ship(b);
    build_windows_on_ship(b);
}

// =============================================================================
// PORTE (`createExtrudedElvenDoor`)
// =============================================================================

/// Costruisce il poligono di una porta ad arco (5+ punti). `w` larghezza,
/// `h` altezza, l'arco parte al 65% dell'altezza.
fn door_polygon(w: f32, h: f32) -> Vec<[f32; 2]> {
    let hw = w * 0.5;
    let spring = h * 0.65;
    let mut poly = Vec::with_capacity(16);
    poly.push([-hw, 0.0]);
    poly.push([hw, 0.0]);
    poly.push([hw, spring]);
    // Bezier quadratico da (hw, spring) a (0, h) con controllo (hw, h).
    for i in 1..=6 {
        let t = i as f32 / 6.0 * 0.5; // primo ramo: t in (0, 0.5]
        let t = t + 0.0; // 0..=0.5
        let u = t * 2.0; // 0..=1
        let b0 = (1.0 - u) * (1.0 - u);
        let b1 = 2.0 * (1.0 - u) * u;
        let b2 = u * u;
        let x = b0 * hw + b1 * hw + b2 * 0.0;
        let y = b0 * spring + b1 * h + b2 * h;
        poly.push([x, y]);
    }
    // Secondo ramo: da (0, h) a (-hw, spring), controllo (-hw, h).
    for i in 1..=6 {
        let t = i as f32 / 6.0;
        let b0 = (1.0 - t) * (1.0 - t);
        let b1 = 2.0 * (1.0 - t) * t;
        let b2 = t * t;
        let x = b0 * 0.0 + b1 * -hw + b2 * -hw;
        let y = b0 * h + b1 * h + b2 * spring;
        poly.push([x, y]);
    }
    poly
}

fn door_frame_polygon(w: f32, h: f32) -> Vec<[f32; 2]> {
    let fw = w * 1.25;
    let fh = h * 1.12;
    door_polygon(fw, fh)
}

/// Cornice dorata (arco) lungo il bordo esterno della porta. Cinque punti del
/// bordo, `tube` con raggio 0.045, come `TubeGeometry` di Three.js.
fn door_frame_ribbon(part: &mut Part, w: f32, h: f32, depth: f32) {
    let hw = w * 0.5;
    let spring = h * 0.65;
    let pts = vec![
        Vec3::new(-hw, 0.0, depth * 1.15),
        Vec3::new(-hw, spring, depth * 1.15),
        Vec3::new(0.0, h, depth * 1.15),
        Vec3::new(hw, spring, depth * 1.15),
        Vec3::new(hw, 0.0, depth * 1.15),
    ];
    tube(part, &pts, 0.045, 20, 6);
}

/// Gemma in cima alla porta (cono a 4 lati, punta verso -Y).
fn door_gem(part: &mut Part, w: f32, h: f32, depth: f32) {
    let r = w * 0.18;
    let hh = h * 0.28;
    let cx = 0.0;
    let cy = h * 0.62;
    let cz = depth * 1.2;
    // Punta verso il basso (nel Three.js: rotation.z = π).
    let apex = Vec3::new(cx, cy - hh * 0.5, cz);
    let base = [
        Vec3::new(cx - r, cy + hh * 0.5, cz),
        Vec3::new(cx + r, cy + hh * 0.5, cz),
        Vec3::new(cx, cy + hh * 0.5, cz - r),
        Vec3::new(cx, cy + hh * 0.5, cz + r),
    ];
    for k in 0..4 {
        let a = base[k];
        let b = base[(k + 1) % 4];
        tri(part, a, b, apex, [1.0; 4], [0.0, 0.0], [1.0, 0.0], [0.5, 1.0]);
    }
}

/// Maniglia: piccolo anello dorato sul lato destro della porta.
fn door_handle(part: &mut Part, w: f32, h: f32, depth: f32) {
    let cx = w * 0.22;
    let cy = h * 0.35;
    let cz = depth * 1.2;
    let r_outer = w * 0.11;
    let r_tube = 0.025;
    // Toro verticale (piano XY), costruito a mano.
    let segs = 12;
    let tube_segs = 6;
    for i in 0..segs {
        let u0 = (i as f32 / segs as f32) * std::f32::consts::TAU;
        let u1 = ((i + 1) as f32 / segs as f32) * std::f32::consts::TAU;
        for j in 0..tube_segs {
            let v0 = (j as f32 / tube_segs as f32) * std::f32::consts::TAU;
            let v1 = ((j + 1) as f32 / tube_segs as f32) * std::f32::consts::TAU;
            let p = |u: f32, v: f32| -> Vec3 {
                let rr = r_outer + r_tube * v.cos();
                Vec3::new(cx + rr * u.cos(), cy + rr * u.sin(), cz + r_tube * v.sin())
            };
            quad(part, p(u0, v0), p(u1, v0), p(u1, v1), p(u0, v1));
        }
    }
}

/// Porta elfica completa (`createExtrudedElvenDoor`). `center` = posizione
/// del cardine in coordinate mondo della nave, `yaw` = rotazione attorno a Y.
pub fn elven_door(
    b: &mut MeshBuilder,
    center: Vec3,
    yaw: f32,
    width: f32,
    height: f32,
    depth: f32,
    is_blue: bool,
) {
    let w = width;
    let h = height;
    let d = depth;

    // 1. Cornice esterna (oro).
    {
        let base = b.part(ElvenMaterial::Gold).positions.len();
        let frame = door_frame_polygon(w, h);
        extrude_polygon(b.part(ElvenMaterial::Gold), &frame, d * 0.75);
        apply_transform(b.part(ElvenMaterial::Gold), base, center, yaw);
    }

    // 2. Battente (legno o blu).
    {
        let mat = if is_blue { ElvenMaterial::BlueDoor } else { ElvenMaterial::DoorWood };
        let base = b.part(mat).positions.len();
        let poly = door_polygon(w, h);
        extrude_polygon(b.part(mat), &poly, d);
        apply_transform(b.part(mat), base, center + Vec3::new(0.0, 0.0, d * 0.15), yaw);
    }

    // 3. Bordo dorato del battente (tube).
    {
        let base = b.part(ElvenMaterial::Gold).positions.len();
        door_frame_ribbon(b.part(ElvenMaterial::Gold), w, h, d);
        apply_transform(b.part(ElvenMaterial::Gold), base, center, yaw);
    }

    // 4. Gemma in cima (solo cornice).
    {
        let base = b.part(ElvenMaterial::Gold).positions.len();
        door_gem(b.part(ElvenMaterial::Gold), w, h, d);
        apply_transform(b.part(ElvenMaterial::Gold), base, center, yaw);
    }

    // 5. Maniglia.
    {
        let base = b.part(ElvenMaterial::Gold).positions.len();
        door_handle(b.part(ElvenMaterial::Gold), w, h, d);
        apply_transform(b.part(ElvenMaterial::Gold), base, center, yaw);
    }
}

// =============================================================================
// GEMME (`createRhomboidalBezel`)
// =============================================================================

/// Cornice dorata a rombo (`createRhomboidalBezel`): quattro trapezi tra il
/// rombo esterno (`ox`, `oy`) e quello interno (`ox - thickness`,
/// `oy - thickness`), estrusi in `depth`.
pub fn rhomboidal_bezel(b: &mut MeshBuilder, center: Vec3, yaw: f32, ox: f32, oy: f32, thickness: f32, depth: f32) {
    let ix = (ox - thickness).max(0.001);
    let iy = (oy - thickness).max(0.001);
    let base = b.part(ElvenMaterial::Gold).positions.len();
    let part = b.part(ElvenMaterial::Gold);

    let outer = [Vec3::new(0.0, oy, 0.0), Vec3::new(ox, 0.0, 0.0), Vec3::new(0.0, -oy, 0.0), Vec3::new(-ox, 0.0, 0.0)];
    let inner = [Vec3::new(0.0, iy, 0.0), Vec3::new(ix, 0.0, 0.0), Vec3::new(0.0, -iy, 0.0), Vec3::new(-ix, 0.0, 0.0)];

    // Fronte (rombo) + retro (rombo) + 4 trapezi spessi.
    let z0 = -depth * 0.5;
    let z1 = depth * 0.5;
    for k in 0..4 {
        let a = outer[k];
        let b2 = outer[(k + 1) % 4];
        let c = inner[(k + 1) % 4];
        let d = inner[k];
        // Fronte
        quad(part, Vec3::new(a.x, a.y, z1), Vec3::new(b2.x, b2.y, z1), Vec3::new(c.x, c.y, z1), Vec3::new(d.x, d.y, z1));
        // Retro
        quad(part, Vec3::new(a.x, a.y, z0), Vec3::new(d.x, d.y, z0), Vec3::new(c.x, c.y, z0), Vec3::new(b2.x, b2.y, z0));
        // Bordo esterno
        quad(part, Vec3::new(a.x, a.y, z0), Vec3::new(b2.x, b2.y, z0), Vec3::new(b2.x, b2.y, z1), Vec3::new(a.x, a.y, z1));
        // Bordo interno
        quad(part, Vec3::new(d.x, d.y, z1), Vec3::new(c.x, c.y, z1), Vec3::new(c.x, c.y, z0), Vec3::new(d.x, d.y, z0));
    }
    apply_transform(part, base, center, yaw);
}

/// Gemma a piramide romboidale: 4 triangoli verso la punta (sul +Z).
pub fn gem_pyramid(b: &mut MeshBuilder, mat: ElvenMaterial, center: Vec3, yaw: f32, r: f32, h: f32) {
    let base = b.part(mat).positions.len();
    let part = b.part(mat);
    let tip = Vec3::new(0.0, 0.0, h);
    let ring = [
        Vec3::new(-r, 0.0, 0.0),
        Vec3::new(0.0, r, 0.0),
        Vec3::new(r, 0.0, 0.0),
        Vec3::new(0.0, -r, 0.0),
    ];
    for k in 0..4 {
        tri(part, ring[k], ring[(k + 1) % 4], tip, [1.0; 4], [0.0, 0.0], [1.0, 0.0], [0.5, 1.0]);
    }
    apply_transform(part, base, center, yaw);
}

// =============================================================================
// MERLI E PARAPETTI (`createClassicMerlon`, `createParapetWithDenseMerlons`)
// =============================================================================

/// Profilo pentagonale di un merlo, in coordinate locali con base a y=0.
fn merlon_polygon(w: f32, h: f32) -> Vec<[f32; 2]> {
    let h = h * 1.10;
    let mut base_w = w;
    let max_ratio = 0.72;
    if h > max_ratio * base_w {
        base_w = h / max_ratio;
    }
    let hs = 1.5 * h - 0.433 * base_w;
    let xs = base_w / 2.0 - hs / 1.732;
    vec![
        [-base_w * 0.5, 0.0],
        [ base_w * 0.5, 0.0],
        [ xs, hs],
        [ 0.0, h],
        [-xs, hs],
    ]
}

pub fn classic_merlon(b: &mut MeshBuilder, center: Vec3, yaw: f32, w: f32, h: f32, depth: f32) {
    // Merlo in avorio.
    {
        let base = b.part(ElvenMaterial::Ivory).positions.len();
        let poly = merlon_polygon(w, h);
        extrude_polygon(b.part(ElvenMaterial::Ivory), &poly, depth);
        apply_transform(b.part(ElvenMaterial::Ivory), base, center, yaw);
    }

    // Topazio: cornice + piastra + gemma.
    let topaz_r = w * 0.16;
    let gem_z = depth / 2.0 - 0.004;
    let gem_y = h * 0.40;

    rhomboidal_bezel(
        b,
        center + Vec3::new(0.0, gem_y, gem_z),
                     yaw,
                     topaz_r * 1.45,
                     topaz_r * 1.80,
                     topaz_r * 0.35,
                     depth * 0.35,
    );

    // Piastra dorata dietro (ottaedro schiacciato).
    {
        let base = b.part(ElvenMaterial::Gold).positions.len();
        let part = b.part(ElvenMaterial::Gold);
        let r = topaz_r * 1.15;
        let ry = r * 1.35;
        let rz = r * 0.20;
        // Ottaedro = due piramidi (base ottagonale, apice sopra e sotto).
        let ring = [
            Vec3::new(-r, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -rz),
            Vec3::new(r, 0.0, 0.0),
            Vec3::new(0.0, 0.0, rz),
        ];
        let top = Vec3::new(0.0, ry, 0.0);
        let bot = Vec3::new(0.0, -ry, 0.0);
        for k in 0..4 {
            let a = ring[k];
            let b2 = ring[(k + 1) % 4];
            tri(part, a, b2, top, [1.0; 4], [0.0, 0.0], [1.0, 0.0], [0.5, 1.0]);
            tri(part, b2, a, bot, [1.0; 4], [0.0, 0.0], [1.0, 0.0], [0.5, 1.0]);
        }
        apply_transform(part, base, center + Vec3::new(0.0, gem_y, gem_z - depth * 0.15), yaw);
    }

    // Gemma frontale (topazio, punta verso +Z locale = +n del merlo).
    let gem_h = topaz_r * 1.2;
    gem_pyramid(
        b,
        ElvenMaterial::Topaz,
        center + Vec3::new(0.0, gem_y, gem_z + gem_h * 0.4),
                yaw,
                topaz_r * 1.02,
                gem_h,
    );
}

pub fn parapet_with_dense_merlons(
    b: &mut MeshBuilder,
    center: Vec3,
    radius: f32,
    count: usize,
    parapet_height: f32,
    merlon_w: f32,
    merlon_h: f32,
) {
    let ph = parapet_height;
    // Pavimento.
    cylinder(b.part(ElvenMaterial::Ivory), center + Vec3::new(0.0, 0.04, 0.0), radius * 0.99, radius * 0.99, 0.08, count * 2, true);
    // Parapetto (cilindro senza caps).
    cylinder(b.part(ElvenMaterial::Ivory), center + Vec3::new(0.0, ph * 0.5, 0.0), radius * 0.98, radius, ph, count * 2, false);
    // Cornice dorata al bordo superiore del parapetto.
    torus_horizontal(b.part(ElvenMaterial::Gold), center + Vec3::new(0.0, ph, 0.0), 0.035, radius, count * 2, 8);
    // Base rail.
    torus_horizontal(b.part(ElvenMaterial::Gold), center + Vec3::new(0.0, 0.03, 0.0), 0.03, radius * 0.98, count * 2, 8);
    // Merli.
    for i in 0..count {
        let a = (i as f32 / count as f32) * std::f32::consts::TAU;
        let pos = center + Vec3::new(a.cos() * radius, ph, a.sin() * radius);
        classic_merlon(b, pos, -a + std::f32::consts::FRAC_PI_2, merlon_w, merlon_h, 0.06);
    }
}

// =============================================================================
// FINESTRE (`createCastleWindow`, `createDormerWindow`)
// =============================================================================

/// Poligono del vetro di una finestra appuntita (5 punti).
fn castle_window_glass(w: f32, h: f32) -> Vec<[f32; 2]> {
    let half = w * 0.5;
    let shoulder = -h * 0.5 + h * 0.6;
    vec![
        [-half, -h * 0.5],
        [ half, -h * 0.5],
        [ half, shoulder],
        [ 0.0,  h * 0.5],
        [-half, shoulder],
    ]
}

pub fn castle_window(b: &mut MeshBuilder, center: Vec3, yaw: f32, w: f32, h: f32) {
    // Cornice (avorio, più grande).
    {
        let base = b.part(ElvenMaterial::Ivory).positions.len();
        // FIX: `castle_window_glass` restituisce già `Vec<[f32; 2]>`. Il
        // precedente `.iter().map(|[x, y]| [x, y]).collect()` produceva
        // `Vec<[&f32; 2]>` (x, y sono riferimenti) e non compilava.
        let poly = castle_window_glass(w + 0.10, h + 0.10);
        extrude_polygon(b.part(ElvenMaterial::Ivory), &poly, 0.06);
        apply_transform(b.part(ElvenMaterial::Ivory), base, center, yaw);
    }
    // Vetro.
    {
        let base = b.part(ElvenMaterial::DarkWindow).positions.len();
        let poly = castle_window_glass(w, h);
        extrude_polygon(b.part(ElvenMaterial::DarkWindow), &poly, 0.02);
        apply_transform(
            b.part(ElvenMaterial::DarkWindow),
                        base,
                        center + Vec3::new(0.0, 0.0, 0.024),
                        yaw,
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn add_castle_windows(
    b: &mut MeshBuilder,
    center: Vec3,
    r_avg: f32,
    facets: usize,
    angles_deg: &[f32],
    win_w: f32,
    win_h: f32,
) {
    let apothem = r_avg * (std::f32::consts::PI / facets as f32).cos();
    for deg in angles_deg {
        let phi = deg.to_radians();
        let pos = center + Vec3::new(apothem * phi.cos(), 0.0, apothem * phi.sin());
        // Nel Three.js la finestra è ruotata di `π/2 - phi` attorno a Y.
        castle_window(b, pos, std::f32::consts::FRAC_PI_2 - phi, win_w, win_h);
    }
}

/// Tettoia con vetro (dormer). Costruisce un canopy a due falde blu e un
/// vetro triangolare sottostante, come `createDormerWindow`.
pub fn dormer_window(b: &mut MeshBuilder, center: Vec3, yaw: f32, scale: f32) {
    let s = scale.max(0.05);
    let w = 0.44 * s;
    let t_roof = 0.11 * s;
    let z_back = -0.75 * s;
    let z_front = 0.24 * s;
    let win_h = 0.40 * s;
    let win_y_top = win_h;
    let y_ridge = win_y_top + 0.02 * s;

    // Vetro triangolare.
    {
        let base = b.part(ElvenMaterial::DarkWindow).positions.len();
        let w_half = w * 0.5 + 0.08 * s;
        let poly = vec![
            [-w_half, 0.0],
            [ w_half, 0.0],
            [ 0.0, win_h],
        ];
        extrude_polygon(b.part(ElvenMaterial::DarkWindow), &poly, 0.02);
        apply_transform(b.part(ElvenMaterial::DarkWindow), base, center, yaw);
    }

    // Canopy blu (due falde).
    {
        let base = b.part(ElvenMaterial::BlueRoof).positions.len();
        let part = b.part(ElvenMaterial::BlueRoof);
        let w_half = w * 0.5 + 0.08 * s;
        let left = Vec3::new(-w_half - t_roof, 0.0, z_front * 0.85);
        let right = Vec3::new(w_half + t_roof, 0.0, z_front * 0.85);
        let ridge_f = Vec3::new(0.0, y_ridge + t_roof, z_front);
        let ridge_b = Vec3::new(0.0, y_ridge + t_roof, z_back);
        let left_b = Vec3::new(-w_half - t_roof, 0.0, z_back);
        let right_b = Vec3::new(w_half + t_roof, 0.0, z_back);
        // Fronte
        tri(part, left, ridge_f, right, [1.0; 4], [0.0, 1.0], [0.5, 0.0], [1.0, 1.0]);
        // Retro
        quad(part, right, ridge_f, ridge_b, right_b);
        quad(part, left, left_b, ridge_b, ridge_f);
        // Lato sinistro
        tri(part, left, right_b, ridge_b, [1.0; 4], [0.0, 1.0], [0.5, 0.0], [1.0, 1.0]);
        // Lato destro
        tri(part, right, ridge_b, right_b, [1.0; 4], [0.0, 1.0], [0.5, 0.0], [1.0, 1.0]);
        apply_transform(part, base, center, yaw);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn place_dormer_on_roof(
    b: &mut MeshBuilder,
    base_radius: f32,
    _roof_height: f32,
    y_base: f32,
    u_frac: f32,
    angle: f32,
    scale: f32,
    facet_count: usize,
) {
    let win_y = y_base + _roof_height * u_frac;
    let mut eff = base_radius * (1.0 - u_frac);
    if facet_count >= 3 {
        eff *= (std::f32::consts::PI / facet_count as f32).cos();
    }
    let pos = Vec3::new(angle.cos() * eff, win_y, angle.sin() * eff);
    // Nel Three.js: dormer.rotateY(-angle + π/2).
    dormer_window(b, pos, -angle + std::f32::consts::FRAC_PI_2, scale);
}

// =============================================================================
// TORRI (`mastList`, `pCent`, `citadel`, `foreMastBase`)
// =============================================================================

struct MastSpec {
    x: f32,
    y_b: f32,
    h: f32,
    hex_h: f32,
    r_hex: f32,
    col_r_top: f32,
    col_r_bot: f32,
}

const MAST_LIST: [MastSpec; 3] = [
    MastSpec { x: 19.0, y_b: 3.5, h: 22.0, hex_h: 5.6, r_hex: 2.1, col_r_top: 0.50, col_r_bot: 0.82 },
MastSpec { x:  0.0, y_b: 5.0, h: 32.0, hex_h: 7.2, r_hex: 2.5, col_r_top: 0.55, col_r_bot: 0.90 },
MastSpec { x: -18.0, y_b: 9.2, h: 21.0, hex_h: 5.2, r_hex: 1.9, col_r_top: 0.46, col_r_bot: 0.78 },
];

fn build_towers(b: &mut MeshBuilder) {
    // 1. Castello centrale (pCent).
    {
        let center = Vec3::new(0.0, 2.8, 0.0);
        cylinder(b.part(ElvenMaterial::Ivory), center + Vec3::new(0.0, 1.0, 0.0), 2.9, 2.5, 2.0, 8, true);
        cone(b.part(ElvenMaterial::BlueRoof), center + Vec3::new(0.0, 2.0, 0.0), 3.0, 2.2, 8);
        for i in 0..4 {
            let ang = (i as f32 / 4.0) * std::f32::consts::TAU + std::f32::consts::FRAC_PI_4;
            place_dormer_on_roof(b, 3.0, 2.2, center.y + 2.0, 0.26, ang, 0.864, 8);
        }
        elven_door(b, center + Vec3::new(2.45, 0.05, 0.0), std::f32::consts::FRAC_PI_2, 0.9, 1.5, 0.44, true);
        elven_door(b, center + Vec3::new(-2.45, 0.05, 0.0), -std::f32::consts::FRAC_PI_2, 0.9, 1.5, 0.44, true);
    }

    // 2. Cittadella di mezzana (citadel).
    {
        let center = Vec3::new(-18.0, 4.2, 0.0);
        cylinder(b.part(ElvenMaterial::Ivory), center + Vec3::new(0.0, 1.2, 0.0), 3.1, 2.5, 2.4, 8, true);
        cone(b.part(ElvenMaterial::BlueRoof), center + Vec3::new(0.0, 2.4, 0.0), 3.3, 1.5, 8);
        for i in 0..4 {
            let ang = (i as f32 / 4.0) * std::f32::consts::TAU + std::f32::consts::FRAC_PI_4;
            place_dormer_on_roof(b, 3.3, 1.5, center.y + 2.4, 0.26, ang, 0.816, 8);
        }
        // Torre superiore (r 1.6, h 2.0, 12 lati) + terrazza merlata.
        cylinder(b.part(ElvenMaterial::Ivory), center + Vec3::new(0.0, 4.4, 0.0), 1.6, 1.6, 2.0, 12, true);
        // Terrazza (floor + merli): centro a (0, 5.2, 0) locale.
        parapet_with_dense_merlons(b, center + Vec3::new(0.0, 5.2, 0.0), 2.02, 20, 0.32, 0.20, 0.28);
        elven_door(b, center + Vec3::new(2.48, 0.05, 0.0), std::f32::consts::FRAC_PI_2, 0.85, 1.4, 0.42, true);
        elven_door(b, center + Vec3::new(1.58, 3.45, 0.0), std::f32::consts::FRAC_PI_2, 0.65, 1.1, 0.16, false);
        elven_door(b, center + Vec3::new(-1.58, 4.4, 0.0), -std::f32::consts::FRAC_PI_2, 0.6, 1.05, 0.16, false);
        // Finestre (2 anelli).
        add_castle_windows(b, center + Vec3::new(0.0, 1.2, 0.0), 2.8, 8,
                           &[67.5, 112.5, 157.5, 202.5, 247.5, 292.5], 0.32, 1.40);
        add_castle_windows(b, center + Vec3::new(0.0, 4.4, 0.0), 1.6, 12,
                           &[45.0, 75.0, 105.0, 135.0, 225.0, 255.0, 285.0, 315.0], 0.20, 0.90);
    }

    // 3. Base dell'albero di trinchetto (foreMastBase).
    {
        let center = Vec3::new(19.0, 3.4, 0.0);
        cylinder(b.part(ElvenMaterial::Ivory), center + Vec3::new(0.0, 0.7, 0.0), 1.7, 1.5, 1.4, 8, true);
        cone(b.part(ElvenMaterial::BlueRoof), center + Vec3::new(0.0, 1.4, 0.0), 1.8, 1.1, 8);
        elven_door(b, center + Vec3::new(-1.46, 0.05, 0.0), -std::f32::consts::FRAC_PI_2, 0.7, 1.15, 0.4, true);
    }

    // 4. Tre alberi a torretta.
    for (idx, m) in MAST_LIST.iter().enumerate() {
        build_mast_tower(b, m, idx);
    }
}

fn build_mast_tower(b: &mut MeshBuilder, m: &MastSpec, idx: usize) {
    let base = Vec3::new(m.x, m.y_b, 0.0);

    // Colonna.
    cylinder(b.part(ElvenMaterial::Ivory), base + Vec3::new(0.0, m.h * 0.5, 0.0), m.col_r_bot, m.col_r_top, m.h, 20, true);

    // Anelli dorati a 1/3 e 2/3.
    for frac in [1.0 / 3.0, 2.0 / 3.0] {
        let y = m.h * frac;
        let r = m.col_r_bot + (m.col_r_top - m.col_r_bot) * frac;
        cylinder(b.part(ElvenMaterial::Gold), base + Vec3::new(0.0, y, 0.0), r * 1.32, r * 1.32, 0.55, 20, true);
        torus_horizontal(b.part(ElvenMaterial::Gold), base + Vec3::new(0.0, y, 0.0), 0.10, r * 1.32, 24, 8);
    }

    // Base ring + floor.
    cylinder(b.part(ElvenMaterial::Gold), base + Vec3::new(0.0, m.h, 0.0), m.r_hex * 0.75, m.r_hex * 1.05, 0.45, 24, true);
    cylinder(b.part(ElvenMaterial::Ivory), base + Vec3::new(0.0, m.h + 0.05, 0.0), m.r_hex, m.r_hex, 0.10, 24, true);

    // Cabina esagonale (r 94-98% di r_hex, h 1.35).
    let cabin_h = 1.35;
    cylinder(b.part(ElvenMaterial::Ivory), base + Vec3::new(0.0, m.h + 0.05 + cabin_h * 0.5, 0.0), m.r_hex * 0.98, m.r_hex * 0.94, cabin_h, 6, true);

    // Sei finestre della cabina.
    for k in 0..6 {
        let ang = (k as f32 / 6.0) * std::f32::consts::TAU;
        let r = m.r_hex * 0.95 * (std::f32::consts::PI / 6.0).cos();
        let pos = base + Vec3::new(ang.cos() * r, m.h + 0.05 + cabin_h * 0.52, ang.sin() * r);
        castle_window(b, pos, -ang + std::f32::consts::FRAC_PI_2, m.r_hex * 0.46, cabin_h * 0.65);
    }

    // Tetto a 6 lati (piramide esagonale).
    let hex_y = m.h + 0.05 + cabin_h;
    cone(b.part(ElvenMaterial::BlueRoof), base + Vec3::new(0.0, hex_y, 0.0), m.r_hex, m.hex_h, 6);

    // 6 dormer sul tetto.
    let scale = if idx == 1 { 1.14 } else { 1.02 };
    for i in 0..6 {
        let ang = (i as f32 / 6.0) * std::f32::consts::TAU;
        place_dormer_on_roof(b, m.r_hex, m.hex_h, hex_y, 0.30, ang, scale, 6);
    }

    // Guglia in cima.
    finial(b, base + Vec3::new(0.0, hex_y + m.hex_h, 0.0), 0.22, 1.5);
}

/// Guglia dorata in cima all'albero (`createPointedSpireFinial`).
fn finial(b: &mut MeshBuilder, center: Vec3, r: f32, h: f32) {
    // Anello di base.
    cylinder(b.part(ElvenMaterial::Gold), center + Vec3::new(0.0, h * 0.06, 0.0), r * 1.3, r * 1.15, h * 0.12, 10, true);
    // Ago.
    cone(b.part(ElvenMaterial::Gold), center + Vec3::new(0.0, h * 0.12, 0.0), r, h * 0.88, 10);
}

// =============================================================================
// Porte e finestre "sparse" sulla nave
// =============================================================================

fn build_doors_on_ship(_b: &mut MeshBuilder) {
    // Porte già posizionate dalle torri in `build_towers` (pCent, citadel,
    // foreMastBase). Qui non serve altro, ma il punto di ingresso resta
    // esplicito per eventuali aggiunte future (es. porte sui fianchi).
}

fn build_windows_on_ship(b: &mut MeshBuilder) {
    // Le finestre della murata (lati `-1` e `+1`) sono distribuite lungo X
    // dalle stazioni `ST_DEFS`. Per ognuna delle 4 fasce "libere" (fuori dal
    // blocco centrale [-12.5, 12.5]) mettiamo una finestra ogni 2.8 m, come
    // in Three.js.
    for side in [-1.0_f32, 1.0] {
        let mut x = -35.0;
        while x <= 36.0 {
            if !(x >= -12.5 && x <= 12.5) {
                let p = wall_point_at_height(x, side, 0.48);
                let ahead = wall_point_at_height(x + 0.3, side, 0.48);
                let behind = wall_point_at_height(x - 0.3, side, 0.48);
                let v_tan = (ahead - behind).normalize_or_zero();
                let v_up = bulwark_up(x, side);
                let v_n = v_tan.cross(v_up).normalize_or_zero() * side;
                // Z locale della finestra = normale verso l'esterno.
                let yaw = v_n.x.atan2(v_n.z);
                // Una per lato (dentro + fuori coincidono sul bordo della
                // fiancata): basta mettere la finestra al punto della murata.
                castle_window(b, p + v_n * 0.012, yaw, 0.55, 0.80);
            }
            x += 2.8;
        }
    }
}
