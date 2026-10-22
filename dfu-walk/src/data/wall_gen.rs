//! Generatore procedurale di mura, porte monumentali, muretti cimiteriali e monumenti in pietra.

use super::building_gen::{BuildingMesh, Footprint, MeshPart, Rng};

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
    Gate,
}

pub fn classify_wall(fp: &Footprint) -> Option<WallKind> {
    let (short, long) = if fp.width < fp.depth { (fp.width, fp.depth) } else { (fp.depth, fp.width) };
    if short < WALL_MIN_THICKNESS || fp.height < WALL_MIN_HEIGHT {
        return None;
    }
    if long / short >= TOWER_MAX_ASPECT {
        Some(WallKind::Segment)
    } else {
        Some(WallKind::Tower)
    }
}

// ---------------------------------------------------------------- geometria

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

pub fn generate_wall(fp: &Footprint, kind: WallKind, seed: u32, texture: Option<(u16, u8)>) -> BuildingMesh {
    let mut rng = Rng::new(seed);
    let mut out = BuildingMesh::default();
    let (hw, hd) = (fp.width * 0.5, fp.depth * 0.5);
    let stone = shade(&mut rng);
    let textured = texture.is_some();
    let wall_col = if textured { [1.0, 1.0, 1.0, 1.0] } else { rgba(stone, 1.0) };
    let trim_col = rgba(stone, 0.8);
    let slit_col = [0.05, 0.05, 0.06, 1.0];
    let [wall, _roof, trim, opening] = &mut out.parts;

    match kind {
        WallKind::Segment => {
            let h = fp.height.clamp(WALL_MIN_HEIGHT, WALL_MAX_HEIGHT);
            let along_x = fp.width >= fp.depth;
            let (hl, hs) = if along_x { (hw, hd) } else { (hd, hw) };
            push_aabb(wall, [-hw, -SKIRT, -hd], [hw, h, hd], wall_col);

            // Merli e zoccolo solo per mura alte; i muretti cimiteriali restano puliti
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

            // Parapetto per torri vere; le lapidi/obelischi/cripte basse restano monumentali
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
        WallKind::Gate => {
            let h = fp.height.clamp(WALL_MIN_HEIGHT, WALL_MAX_HEIGHT);
            let along_x = fp.width >= fp.depth;
            let (hl, hs) = if along_x { (hw, hd) } else { (hd, hw) };

            let arch_w = (hl * 0.7).clamp(6.0, 9.0);
            let haw = arch_w * 0.5;
            let arch_h = (h * 0.65).clamp(4.5, 7.5);

            let tower_thick = hs + 0.7;
            let tower_h = h + 1.4;
            let tower_len = 2.6f32;

            if along_x {
                push_aabb(wall, [-hl, -SKIRT, -hs], [-haw, h, hs], wall_col);
                push_aabb(wall, [haw, -SKIRT, -hs], [hl, h, hs], wall_col);
                push_aabb(wall, [-haw, arch_h, -hs], [haw, h, hs], wall_col);

                push_aabb(wall, [-haw - tower_len, -SKIRT, -tower_thick], [-haw, tower_h, tower_thick], wall_col);
                push_aabb(wall, [haw, -SKIRT, -tower_thick], [haw + tower_len, tower_h, tower_thick], wall_col);

                push_aabb(wall, [-haw, arch_h - 1.2, -hs], [-haw + 1.2, arch_h, hs], wall_col);
                push_aabb(wall, [haw - 1.2, arch_h - 1.2, -hs], [haw, arch_h, hs], wall_col);

                merlon_row(wall, true, hs, hl, h, wall_col);
                merlon_row(wall, true, -hs, hl, h, wall_col);

                push_aabb(wall, [-haw - tower_len, tower_h, tower_thick - 0.5], [-haw, tower_h + MERLON_H, tower_thick], wall_col);
                push_aabb(wall, [-haw - tower_len, tower_h, -tower_thick], [-haw, tower_h + MERLON_H, -tower_thick + 0.5], wall_col);
                push_aabb(wall, [haw, tower_h, tower_thick - 0.5], [haw + tower_len, tower_h + MERLON_H, tower_thick], wall_col);
                push_aabb(wall, [haw, tower_h, -tower_thick], [haw + tower_len, tower_h + MERLON_H, -tower_thick + 0.5], wall_col);

                let y0 = arch_h * 0.7;
                let y1 = y0 + 1.2;
                let inside_l = [-haw - tower_len * 0.5, y0, 0.0];
                let inside_r = [haw + tower_len * 0.5, y0, 0.0];
                for s in [1.0f32, -1.0] {
                    let z = s * (tower_thick + 0.03);
                    let xl = -haw - tower_len * 0.5;
                    push_face(opening, &[[xl - 0.25, y0, z], [xl + 0.25, y0, z], [xl + 0.25, y1, z], [xl - 0.25, y1, z]], slit_col, inside_l);
                    let xr = haw + tower_len * 0.5;
                    push_face(opening, &[[xr - 0.25, y0, z], [xr + 0.25, y0, z], [xr + 0.25, y1, z], [xr - 0.25, y1, z]], slit_col, inside_r);
                }
            } else {
                push_aabb(wall, [-hs, -SKIRT, -hl], [hs, h, -haw], wall_col);
                push_aabb(wall, [-hs, -SKIRT, haw], [hs, h, hl], wall_col);
                push_aabb(wall, [-hs, arch_h, -haw], [hs, h, haw], wall_col);

                push_aabb(wall, [-tower_thick, -SKIRT, -haw - tower_len], [tower_thick, tower_h, -haw], wall_col);
                push_aabb(wall, [-tower_thick, -SKIRT, haw], [tower_thick, tower_h, haw + tower_len], wall_col);

                push_aabb(wall, [-hs, arch_h - 1.2, -haw], [hs, arch_h, -haw + 1.2], wall_col);
                push_aabb(wall, [-hs, arch_h - 1.2, haw - 1.2], [hs, arch_h, haw], wall_col);

                merlon_row(wall, false, hs, hl, h, wall_col);
                merlon_row(wall, false, -hs, hl, h, wall_col);

                push_aabb(wall, [tower_thick - 0.5, tower_h, -haw - tower_len], [tower_thick, tower_h + MERLON_H, -haw], wall_col);
                push_aabb(wall, [-tower_thick, tower_h, -haw - tower_len], [-tower_thick + 0.5, tower_h + MERLON_H, -haw], wall_col);
                push_aabb(wall, [tower_thick - 0.5, tower_h, haw], [tower_thick, tower_h + MERLON_H, haw + tower_len], wall_col);
                push_aabb(wall, [-tower_thick, tower_h, haw], [-tower_thick + 0.5, tower_h + MERLON_H, haw + tower_len], wall_col);

                let y0 = arch_h * 0.7;
                let y1 = y0 + 1.2;
                let inside_l = [0.0, y0, -haw - tower_len * 0.5];
                let inside_r = [0.0, y0, haw + tower_len * 0.5];
                for s in [1.0f32, -1.0] {
                    let x = s * (tower_thick + 0.03);
                    let zl = -haw - tower_len * 0.5;
                    push_face(opening, &[[x, y0, zl - 0.25], [x, y0, zl + 0.25], [x, y1, zl + 0.25], [x, y1, zl - 0.25]], slit_col, inside_l);
                    let zr = haw + tower_len * 0.5;
                    push_face(opening, &[[x, y0, zr - 0.25], [x, y0, zr + 0.25], [x, y1, zr + 0.25], [x, y1, zr - 0.25]], slit_col, inside_r);
                }
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
