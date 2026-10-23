//! Generatore procedurale di mura, porte monumentali, archi di trionfo e torri in pietra.

use super::building_gen::{BuildingMesh, Footprint, MeshPart, Rng};
use std::f32::consts::PI;

pub const WALL_MIN_HEIGHT: f32 = 1.2;
pub const WALL_MAX_HEIGHT: f32 = 16.0;
pub const TOWER_MIN_HEIGHT: f32 = 1.2;
pub const TOWER_MAX_HEIGHT: f32 = 22.0;
pub const TOWER_MAX_ASPECT: f32 = 1.8;
pub const TOWER_MIN_SIDE: f32 = 2.0;
pub const WALL_MIN_THICKNESS: f32 = 0.7;

pub const WALL_TEXTURE_METERS: f32 = 6.4;

const SKIRT: f32 = 0.3;
const MERLON_H: f32 = 1.3;

pub const ROTATE_WALLS_90: bool = false;

pub const FORCE_TRIUMPHAL_ARCH: bool = false;

pub fn wall_footprint(fp: &Footprint) -> Footprint {
    if ROTATE_WALLS_90 {
        Footprint { width: fp.depth, depth: fp.width, ..*fp }
    } else {
        *fp
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WallKind {
    Segment,
    Tower,
    Gate, // Arco di Trionfo Monumentale
}

pub fn classify_wall(fp: &Footprint) -> Option<WallKind> {
    let (short, long) = if fp.width < fp.depth { (fp.width, fp.depth) } else { (fp.depth, fp.width) };
    if short < WALL_MIN_THICKNESS || fp.height < WALL_MIN_HEIGHT {
        return None;
    }
    if long / short >= TOWER_MAX_ASPECT {
        if FORCE_TRIUMPHAL_ARCH {
            Some(WallKind::Gate)
        } else {
            Some(WallKind::Segment)
        }
    } else {
        Some(WallKind::Tower)
    }
}

// =========================================================================
// GEOMETRIA E MATEMATICA VETTORIALE
// =========================================================================

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn face_normal(a: V3, b: V3, c: V3) -> Option<V3> {
    let n = cross(sub(b, a), sub(c, a));
    let len = dot(n, n).sqrt();
    if len < 1e-8 {
        None
    } else {
        Some([n[0] / len, n[1] / len, n[2] / len])
    }
}
fn planar_uv(p: V3, n: V3) -> [f32; 2] {
    let (ax, ay, az) = (n[0].abs(), n[1].abs(), n[2].abs());
    if ay >= ax && ay >= az {
        [p[0], p[2]]
    } else if ax >= az {
        [p[2], p[1]]
    } else {
        [p[0], p[1]]
    }
}

fn push_face(part: &mut MeshPart, pts: &[V3], color: [f32; 4], inside: V3) {
    let len = pts.len();
    let raw = face_normal(pts[0], pts[1], pts[2])
    .or_else(|| if len == 4 { face_normal(pts[1], pts[2], pts[3]) } else { None });
    let Some(raw) = raw else { return };
    let mut centroid = [0.0f32; 3];
    for p in pts {
        for k in 0..3 {
            centroid[k] += p[k] / len as f32;
        }
    }
    let flip = dot(raw, sub(centroid, inside)) < 0.0;
    let n = if flip { [-raw[0], -raw[1], -raw[2]] } else { raw };
    let base = part.positions.len() as u32;
    let order: Vec<usize> = if flip { (0..len).rev().collect() } else { (0..len).collect() };
    for &i in &order {
        part.positions.push(pts[i]);
        part.normals.push(n);
        part.uvs.push(planar_uv(pts[i], n));
        part.colors.push(color);
    }
    for k in 1..(len as u32 - 1) {
        part.indices.extend_from_slice(&[base, base + k, base + k + 1]);
    }
}

fn push_aabb(part: &mut MeshPart, min: V3, max: V3, color: [f32; 4]) {
    let mut c = [[0.0f32; 3]; 8];
    for (i, corner) in c.iter_mut().enumerate() {
        *corner = [
            if i & 1 == 0 { min[0] } else { max[0] },
                if i & 2 == 0 { min[1] } else { max[1] },
                    if i & 4 == 0 { min[2] } else { max[2] },
        ];
    }
    let mut inside = [0.0f32; 3];
    for p in &c {
        for k in 0..3 {
            inside[k] += p[k] / 8.0;
        }
    }
    const FACES: [[usize; 4]; 6] =
    [[0, 2, 6, 4], [1, 3, 7, 5], [0, 1, 5, 4], [2, 3, 7, 6], [0, 1, 3, 2], [4, 5, 7, 6]];
    for f in FACES {
        push_face(part, &[c[f[0]], c[f[1]], c[f[2]], c[f[3]]], color, inside);
    }
}

fn merlon_row(part: &mut MeshPart, along_x: bool, edge: f32, half_len: f32, y0: f32, color: [f32; 4]) {
    let len = half_len * 2.0;
    let n = ((len / 2.0).floor() as usize).clamp(1, 200);
    let pitch = len / n as f32;
    let ml = pitch * 0.55;
    let t = 0.5f32;
    let (b0, b1) = if edge >= 0.0 { (edge - t, edge) } else { (edge, edge + t) };
    for i in 0..n {
        let a = -half_len + (i as f32 + 0.5) * pitch;
        let (a0, a1) = (a - ml * 0.5, a + ml * 0.5);
        let (min, max) = if along_x {
            ([a0, y0, b0], [a1, y0 + MERLON_H, b1])
        } else {
            ([b0, y0, a0], [b1, y0 + MERLON_H, a1])
        };
        push_aabb(part, min, max, color);
    }
}

fn shade(rng: &mut Rng) -> [f32; 3] {
    const STONE: [[f32; 3]; 3] = [[0.50, 0.49, 0.47], [0.58, 0.56, 0.52], [0.44, 0.43, 0.42]];
    let base = STONE[((rng.f32() * 3.0) as usize).min(2)];
    let mut c = base;
    for k in 0..3 {
        c[k] = (c[k] + rng.range(-0.03, 0.03)).clamp(0.0, 1.0);
    }
    c
}

fn rgba(c: [f32; 3], k: f32) -> [f32; 4] {
    [c[0] * k, c[1] * k, c[2] * k, 1.0]
}

// =========================================================================
// GENERATORE PRINCIPALE
// =========================================================================

pub fn generate_wall(fp: &Footprint, mut kind: WallKind, seed: u32, texture: Option<(u16, u8)>) -> BuildingMesh {
    if FORCE_TRIUMPHAL_ARCH && kind == WallKind::Segment {
        kind = WallKind::Gate;
    }

    let mut rng = Rng::new(seed);
    let mut out = BuildingMesh::default();
    let (hw, hd) = (fp.width * 0.5, fp.depth * 0.5);
    let stone = shade(&mut rng);
    let textured = texture.is_some();
    let wall_col = if textured { [1.0, 1.0, 1.0, 1.0] } else { rgba(stone, 1.0) };
    let trim_col = rgba(stone, 0.85);
    let slit_col = [0.05, 0.05, 0.06, 1.0];
    let [wall, _roof, trim, opening] = &mut out.parts;

    match kind {
        WallKind::Segment => {
            let h = fp.height.clamp(WALL_MIN_HEIGHT, WALL_MAX_HEIGHT);
            let along_x = fp.width >= fp.depth;
            let (hl, hs) = if along_x { (hw, hd) } else { (hd, hw) };
            push_aabb(wall, [-hw, -SKIRT, -hd], [hw, h, hd], wall_col);

            if h >= 3.8 {
                if textured {
                    push_aabb(wall, [-hw - 0.3, -SKIRT, -hd - 0.3], [hw + 0.3, 1.0, hd + 0.3], wall_col);
                } else {
                    push_aabb(trim, [-hw - 0.3, -SKIRT, -hd - 0.3], [hw + 0.3, 1.0, hd + 0.3], trim_col);
                }
                merlon_row(wall, along_x, hs, hl, h, wall_col);
                merlon_row(wall, along_x, -hs, hl, h, wall_col);
            }
        }

        WallKind::Tower => {
            let h = fp.height.clamp(TOWER_MIN_HEIGHT, TOWER_MAX_HEIGHT);
            push_aabb(wall, [-hw, -SKIRT, -hd], [hw, h, hd], wall_col);

            if h >= 4.0 {
                if textured {
                    push_aabb(wall, [-hw - 0.4, h - 1.0, -hd - 0.4], [hw + 0.4, h, hd + 0.4], wall_col);
                } else {
                    push_aabb(trim, [-hw - 0.4, h - 1.0, -hd - 0.4], [hw + 0.4, h, hd + 0.4], trim_col);
                }
                for s in [1.0f32, -1.0] {
                    merlon_row(wall, true, s * (hd + 0.4), hw + 0.4, h, wall_col);
                    merlon_row(wall, false, s * (hw + 0.4), hd + 0.4, h, wall_col);
                }
            }
            if h >= 8.0 {
                let (y0, y1) = (h * 0.6, h * 0.6 + 1.2);
                let inside = [0.0, h * 0.5, 0.0];
                let o = 0.03;
                for s in [1.0f32, -1.0] {
                    let x = s * (hw + o);
                    push_face(opening, &[[x, y0, -0.2], [x, y0, 0.2], [x, y1, 0.2], [x, y1, -0.2]], slit_col, inside);
                    let z = s * (hd + o);
                    push_face(opening, &[[-0.2, y0, z], [0.2, y0, z], [0.2, y1, z], [-0.2, y1, z]], slit_col, inside);
                }
            }
        }

        // =====================================================================
        // ARCO DI TRIONFO MONUMENTALE (FORNICE IMPERIALE CONTINUO E SOLIDO)
        // =====================================================================
        WallKind::Gate => {
            let h = fp.height.clamp(9.0, WALL_MAX_HEIGHT);
            let along_x = fp.width >= fp.depth;
            let (hl, hs) = if along_x { (hw, hd) } else { (hd, hw) };

            let to_world = |u: f32, y: f32, v: f32| -> V3 {
                if along_x { [u, y, v] } else { [v, y, u] }
            };

            let mut push_ob = |part: &mut MeshPart, u0: f32, u1: f32, y0: f32, y1: f32, v0: f32, v1: f32, col: [f32; 4]| {
                let (min, max) = if along_x {
                    ([u0.min(u1), y0.min(y1), v0.min(v1)], [u0.max(u1), y0.max(y1), v0.max(v1)])
                } else {
                    ([v0.min(v1), y0.min(y1), u0.min(u1)], [v0.max(v1), y0.max(y1), u0.max(u1)])
                };
                push_aabb(part, min, max, col);
            };

            // Spessore aggettante dell'Arco
            let arch_hs = (hs + 0.85).max(2.4);

            // 1. FORNICE MAESTRO DA 14.4 METRI (Accoglie comodamente la strada da 12 metri)
            let haw = 7.2f32.min(hl * 0.75); // semilarghezza arco: 7.2m (luce totale 14.4m)
            let r_main = haw;
            let y_spring = (h * 0.40).clamp(4.0, 5.2);
            let y_apex = y_spring + r_main; // Colmo arco a tutto sesto (~11.4m - 12.4m)

            // Trabeazione e attico
            let y_entablature = y_apex + 0.8;
            let y_attic_top = y_entablature + 3.0;

            // Piloni massicci ai lati dell'arco
            let pylon_w = 4.2f32;
            let arch_hw = (haw + pylon_w).min(hl);

            // -----------------------------------------------------------------
            // 2. PILONI MONUMENTALI PIENI AL 100% (Zero buchi, da -SKIRT a trabeazione)
            // -----------------------------------------------------------------
            push_ob(wall, -arch_hw, -haw, -SKIRT, y_entablature, -arch_hs, arch_hs, wall_col);
            push_ob(wall, haw, arch_hw, -SKIRT, y_entablature, -arch_hs, arch_hs, wall_col);

            // -----------------------------------------------------------------
            // 3. VOLTA A BOTTE CONTINUA AD ARCO A TUTTO SESTO E PENNACCHI
            // -----------------------------------------------------------------
            let n_segs = 16;
            for k in 0..n_segs {
                let t0 = (k as f32 / n_segs as f32) * PI;
                let t1 = ((k + 1) as f32 / n_segs as f32) * PI;

                let u0 = -r_main * t0.cos();
                let y0 = y_spring + r_main * t0.sin();
                let u1 = -r_main * t1.cos();
                let y1 = y_spring + r_main * t1.sin();

                let u_mid = (u0 + u1) * 0.5;
                let y_mid = (y0 + y1) * 0.5;

                // 1. Intradosso della volta (soffitto curvo della galleria)
                let p0 = to_world(u0, y0, -arch_hs);
                let p1 = to_world(u1, y1, -arch_hs);
                let p2 = to_world(u1, y1, arch_hs);
                let p3 = to_world(u0, y0, arch_hs);

                // Controllo radiale perfetto: calcolato lungo la pietra per orientare correttamente ogni spicchio
                let ins_vault = to_world(u_mid * 1.15, y_spring + (y_mid - y_spring) * 1.15, 0.0);
                push_face(wall, &[p0, p1, p2, p3], wall_col, ins_vault);

                // 2. Pennacchio facciata sud (+V)
                let f0 = to_world(u0, y0, arch_hs);
                let f1 = to_world(u1, y1, arch_hs);
                let f2 = to_world(u1, y_entablature, arch_hs);
                let f3 = to_world(u0, y_entablature, arch_hs);
                let ins_front = to_world(u_mid, (y_mid + y_entablature) * 0.5, arch_hs - 1.0);
                push_face(wall, &[f0, f1, f2, f3], wall_col, ins_front);

                // 3. Pennacchio facciata nord (-V)
                let b0 = to_world(u0, y0, -arch_hs);
                let b1 = to_world(u0, y_entablature, -arch_hs);
                let b2 = to_world(u1, y_entablature, -arch_hs);
                let b3 = to_world(u1, y1, -arch_hs);
                let ins_back = to_world(u_mid, (y_mid + y_entablature) * 0.5, -arch_hs + 1.0);
                push_face(wall, &[b0, b1, b2, b3], wall_col, ins_back);
            }

            // Muratura piena sopra il colmo dell'arco fino alla trabeazione
            push_ob(wall, -haw, haw, y_apex, y_entablature, -arch_hs + 0.05, arch_hs - 0.05, wall_col);

            // -----------------------------------------------------------------
            // 4. SPALLETTE LATERALI DI RITORNO DELL'AGGETTO (Sigillano i fianchi)
            // -----------------------------------------------------------------
            push_ob(wall, -arch_hw, -arch_hw + 0.05, -SKIRT, y_entablature, hs, arch_hs, wall_col);
            push_ob(wall, -arch_hw, -arch_hw + 0.05, -SKIRT, y_entablature, -arch_hs, -hs, wall_col);

            push_ob(wall, arch_hw - 0.05, arch_hw, -SKIRT, y_entablature, hs, arch_hs, wall_col);
            push_ob(wall, arch_hw - 0.05, arch_hw, -SKIRT, y_entablature, -arch_hs, -hs, wall_col);

            // -----------------------------------------------------------------
            // 5. COLONNE COLOSSALI BINATE A RILIEVO (Facciata Nord e Sud)
            // -----------------------------------------------------------------
            let col_proj = 0.32f32;
            let col_w = 0.85f32;
            let col_u_offsets = [-haw - 1.4, -haw - 3.2, haw + 1.4, haw + 3.2];

            for &u_col in &col_u_offsets {
                if u_col.abs() <= arch_hw - 0.2 {
                    for &sign_v in &[-1.0f32, 1.0] {
                        let v_face = sign_v * arch_hs;
                        let v_outer = sign_v * (arch_hs + col_proj);

                        // Piedistallo
                        push_ob(wall, u_col - col_w * 0.6, u_col + col_w * 0.6, -SKIRT, 1.6, v_face, v_outer + sign_v * 0.08, trim_col);
                        // Fusto colonna
                        push_ob(wall, u_col - col_w * 0.5, u_col + col_w * 0.5, 1.6, y_entablature - 0.8, v_face, v_outer, wall_col);
                        // Capitello
                        push_ob(wall, u_col - col_w * 0.65, u_col + col_w * 0.65, y_entablature - 0.8, y_entablature, v_face, v_outer + sign_v * 0.12, trim_col);
                    }
                }
            }

            // -----------------------------------------------------------------
            // 6. TRABEAZIONE, ATTICO IMPERIALE E TIMPANO CLASSICO
            // -----------------------------------------------------------------
            // Trabeazione continua marcapiano
            push_ob(wall, -arch_hw - 0.35, arch_hw + 0.35, y_entablature, y_entablature + 0.70, -arch_hs - 0.40, arch_hs + 0.40, trim_col);

            // Attico imperiale
            push_ob(wall, -arch_hw, arch_hw, y_entablature + 0.70, y_attic_top, -arch_hs + 0.15, arch_hs - 0.15, wall_col);

            // Cornice superiore dell'attico
            push_ob(wall, -arch_hw - 0.25, arch_hw + 0.25, y_attic_top, y_attic_top + 0.35, -arch_hs - 0.15, arch_hs + 0.15, trim_col);

            // TIMPANO CLASSICO TRIANGOLARE AL CENTRO DELL'ATTICO
            let ped_w = (haw + 1.2).min(arch_hw);
            let ped_apex = y_attic_top + 1.8;

            for &sign_v in &[-1.0f32, 1.0] {
                let v_ped = sign_v * (arch_hs - 0.1);
                let p_left  = to_world(-ped_w, y_attic_top + 0.35, v_ped);
                let p_right = to_world(ped_w,  y_attic_top + 0.35, v_ped);
                let p_top   = to_world(0.0,    ped_apex,           v_ped);
                let ins_ped = to_world(0.0, y_attic_top, v_ped - sign_v * 0.5);

                if sign_v > 0.0 {
                    push_face(wall, &[p_left, p_right, p_top], trim_col, ins_ped);
                } else {
                    push_face(wall, &[p_left, p_top, p_right], trim_col, ins_ped);
                }
            }

            // Falde del timpano
            let ped_left_f  = to_world(-ped_w, y_attic_top + 0.35, arch_hs - 0.1);
            let ped_left_b  = to_world(-ped_w, y_attic_top + 0.35, -arch_hs + 0.1);
            let ped_top_f   = to_world(0.0, ped_apex, arch_hs - 0.1);
            let ped_top_b   = to_world(0.0, ped_apex, -arch_hs + 0.1);
            let ped_right_f = to_world(ped_w, y_attic_top + 0.35, arch_hs - 0.1);
            let ped_right_b = to_world(ped_w, y_attic_top + 0.35, -arch_hs + 0.1);

            let ins_roof = to_world(0.0, y_attic_top, 0.0);
            push_face(wall, &[ped_left_f, ped_left_b, ped_top_b, ped_top_f], trim_col, ins_roof);
            push_face(wall, &[ped_top_f, ped_top_b, ped_right_b, ped_right_f], trim_col, ins_roof);

            // -----------------------------------------------------------------
            // 7. RACCORDO CON LE MURA CITTADINE E MERLI
            // -----------------------------------------------------------------
            if hl > arch_hw {
                // Tratto di mura sinistro
                push_ob(wall, -hl, -arch_hw, -SKIRT, h, -hs, hs, wall_col);
                push_ob(wall, -hl - 0.25, -arch_hw, -SKIRT, 1.0, -hs - 0.25, hs + 0.25, trim_col);

                // Tratto di mura destro
                push_ob(wall, arch_hw, hl, -SKIRT, h, -hs, hs, wall_col);
                push_ob(wall, arch_hw, hl + 0.25, -SKIRT, 1.0, -hs - 0.25, hs + 0.25, trim_col);

                // Merli sulle ali laterali
                let mut push_wall_merlons = |u_start: f32, u_end: f32, edge: f32| {
                    let len = (u_end - u_start).abs();
                    let n = ((len / 2.0).floor() as usize).clamp(1, 100);
                    let pitch = len / n as f32;
                    let ml = pitch * 0.55;
                    let t = 0.5f32;
                    let (v0, v1) = if edge >= 0.0 { (edge - t, edge) } else { (edge, edge + t) };
                    for i in 0..n {
                        let u_c = u_start.min(u_end) + (i as f32 + 0.5) * pitch;
                        let u0 = u_c - ml * 0.5;
                        let u1 = u_c + ml * 0.5;
                        push_ob(wall, u0, u1, h, h + MERLON_H, v0, v1, wall_col);
                    }
                };

                push_wall_merlons(-hl, -arch_hw, hs);
                push_wall_merlons(-hl, -arch_hw, -hs);
                push_wall_merlons(arch_hw, hl, hs);
                push_wall_merlons(arch_hw, hl, -hs);
            }
        }
    }

    if textured {
        for uv in wall.uvs.iter_mut() {
            *uv = [uv[0] / WALL_TEXTURE_METERS, -uv[1] / WALL_TEXTURE_METERS];
        }
    }
    out.wall_texture = texture;
    out
}
