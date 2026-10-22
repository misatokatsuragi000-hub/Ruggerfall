//! Generatore procedurale di edifici per le città con texture originali per facciate, tetti, porte e finestre.

use super::arch3d::{Arch3dModel, BuildingTextures};

pub const DF_UNIT: f32 = 0.025;
pub const BLOCK_SIZE_M: f32 = 4096.0 * DF_UNIT;

pub const MIN_SIDE: f32 = 3.0;
pub const MAX_SIDE: f32 = 75.0;
pub const MAX_ASPECT: f32 = 3.5;
pub const MIN_HEIGHT: f32 = 2.4;
pub const HOUSE_AREA: f32 = 120.0;
pub const HALL_AREA: f32 = 400.0;

pub const BUILDING_TEXTURE_METERS: f32 = 3.2;
pub const ROOF_TEXTURE_METERS: f32 = 3.5;

const SKIRT: f32 = 0.3;
const EAVE: f32 = 0.4;
const MIN_WALL_HEIGHT: f32 = 2.6;
const FLOOR_HEIGHT: f32 = 3.0;

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
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

fn newell_normal(pts: &[V3]) -> Option<V3> {
    let mut n = [0.0f32; 3];
    let len = pts.len();
    for i in 0..len {
        let a = pts[i];
        let b = pts[(i + 1) % len];
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l < 1e-8 {
        None
    } else {
        Some([n[0] / l, n[1] / l, n[2] / l])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialKind {
    Wall,
    Roof,
    Trim,
    Opening,
}

pub const MATERIAL_COUNT: usize = 4;

impl MaterialKind {
    pub const ALL: [MaterialKind; MATERIAL_COUNT] = [
        MaterialKind::Wall,
        MaterialKind::Roof,
        MaterialKind::Trim,
        MaterialKind::Opening,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            MaterialKind::Wall => "wall",
            MaterialKind::Roof => "roof",
            MaterialKind::Trim => "trim",
            MaterialKind::Opening => "opening",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    PosX,
    NegX,
    PosZ,
    NegZ,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::PosX, Side::NegX, Side::PosZ, Side::NegZ];

    pub fn normal(self) -> [f32; 2] {
        match self {
            Side::PosX => [1.0, 0.0],
            Side::NegX => [-1.0, 0.0],
            Side::PosZ => [0.0, 1.0],
            Side::NegZ => [0.0, -1.0],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildingKind {
    Cottage,
    House,
    Hall,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RoofKind {
    Gable,
    Hip,
    Flat,
}

#[derive(Clone, Copy, Debug)]
pub struct Footprint {
    pub cx: f32,
    pub cz: f32,
    pub width: f32,
    pub depth: f32,
    pub height: f32,
}

impl Footprint {
    pub fn from_model(model: &Arch3dModel) -> Option<Self> {
        let mut it = model.vertices.iter();
        let first = *it.next()?;
        let (mut min, mut max) = (first, first);
        for v in it {
            for k in 0..3 {
                min[k] = min[k].min(v[k]);
                max[k] = max[k].max(v[k]);
            }
        }
        if !min.iter().chain(max.iter()).all(|c| c.is_finite()) {
            return None;
        }
        Some(Footprint {
            cx: (min[0] + max[0]) * 0.5,
             cz: -(min[2] + max[2]) * 0.5,
             width: max[0] - min[0],
             depth: max[2] - min[2],
             height: max[1] - min[1].min(0.0),
        })
    }

    pub fn evaluate(&self) -> Result<BuildingKind, &'static str> {
        let (short, long) = if self.width < self.depth {
            (self.width, self.depth)
        } else {
            (self.depth, self.width)
        };
        if short < MIN_SIDE {
            return Err("lato corto < MIN_SIDE");
        }
        if long > MAX_SIDE {
            return Err("lato lungo > MAX_SIDE");
        }
        if long / short > MAX_ASPECT {
            return Err("troppo allungato");
        }
        if self.height < MIN_HEIGHT {
            return Err("troppo basso");
        }
        let area = self.width * self.depth;
        Ok(if area >= HALL_AREA {
            BuildingKind::Hall
        } else if area >= HOUSE_AREA || self.height >= 9.0 {
            BuildingKind::House
        } else {
            BuildingKind::Cottage
        })
    }

    pub fn classify(&self) -> Option<BuildingKind> {
        self.evaluate().ok()
    }
}

pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        let s = seed.wrapping_mul(2_654_435_761) ^ 0xA5A5_A5A5;
        Rng(if s == 0 { 1 } else { s })
    }
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    pub fn f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.0
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f32()
    }
    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }
    fn pick(&mut self, n: usize) -> usize {
        ((self.f32() * n as f32) as usize).min(n - 1)
    }
}

pub fn seed_for(model_id: u32, x: i32, z: i32) -> u32 {
    let mut h = model_id.wrapping_mul(0x9E37_79B1)
    ^ (x as u32).wrapping_mul(0x85EB_CA6B)
    ^ (z as u32).wrapping_mul(0xC2B2_AE35);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^ (h >> 12)
}

#[derive(Clone, Debug, Default)]
pub struct MeshPart {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
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

impl MeshPart {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    fn push_face(&mut self, pts: &[V3], color: [f32; 4], inside: V3) {
        let len = pts.len();
        debug_assert!(len == 3 || len == 4);

        let raw = newell_normal(pts)
        .or_else(|| face_normal(pts[0], pts[1], pts[2]))
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

        let base = self.positions.len() as u32;
        let order: Vec<usize> = if flip { (0..len).rev().collect() } else { (0..len).collect() };
        for &i in &order {
            self.positions.push(pts[i]);
            self.normals.push(n);
            self.uvs.push(planar_uv(pts[i], n));
            self.colors.push(color);
        }
        for k in 1..(len as u32 - 1) {
            self.indices.extend_from_slice(&[base, base + k, base + k + 1]);
        }
    }
}

fn push_quad_uv(part: &mut MeshPart, pts: &[V3; 4], uvs: &[[f32; 2]; 4], inside: V3) {
    let raw = newell_normal(pts)
    .or_else(|| face_normal(pts[0], pts[1], pts[2]))
    .or_else(|| face_normal(pts[1], pts[2], pts[3]));
    let Some(raw) = raw else { return };
    let centroid = [
        (pts[0][0] + pts[1][0] + pts[2][0] + pts[3][0]) * 0.25,
        (pts[0][1] + pts[1][1] + pts[2][1] + pts[3][1]) * 0.25,
        (pts[0][2] + pts[1][2] + pts[2][2] + pts[3][2]) * 0.25,
    ];
    let flip = dot(raw, sub(centroid, inside)) < 0.0;
    let n = if flip { [-raw[0], -raw[1], -raw[2]] } else { raw };
    let base = part.positions.len() as u32;

    let order = if flip { [3, 2, 1, 0] } else { [0, 1, 2, 3] };
    for &i in &order {
        part.positions.push(pts[i]);
        part.normals.push(n);
        part.uvs.push(uvs[i]);
        part.colors.push([1.0, 1.0, 1.0, 1.0]);
    }
    part.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

#[derive(Clone, Debug, Default)]
pub struct BuildingMesh {
    pub parts: [MeshPart; MATERIAL_COUNT],
    pub wall_texture: Option<(u16, u8)>,
    pub roof_texture: Option<(u16, u8)>,
    pub door_texture: Option<(u16, u8)>,
    pub window_texture: Option<(u16, u8)>,
    pub doors: MeshPart,
    pub windows: MeshPart,
}

impl BuildingMesh {
    pub fn part(&self, m: MaterialKind) -> &MeshPart {
        &self.parts[m.index()]
    }
    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(|p| p.triangle_count()).sum::<usize>()
        + self.doors.triangle_count()
        + self.windows.triangle_count()
    }
}

fn push_hex(part: &mut MeshPart, c: [V3; 8], color: [f32; 4]) {
    let mut inside = [0.0f32; 3];
    for p in &c {
        for k in 0..3 {
            inside[k] += p[k] / 8.0;
        }
    }
    const FACES: [[usize; 4]; 6] = [
        [0, 2, 6, 4],
        [1, 3, 7, 5],
        [0, 1, 5, 4],
        [2, 3, 7, 6],
        [0, 1, 3, 2],
        [4, 5, 7, 6],
    ];
    for f in FACES {
        part.push_face(&[c[f[0]], c[f[1]], c[f[2]], c[f[3]]], color, inside);
    }
}

struct Facade {
    c: V3,
    u: V3,
    n: V3,
    len: f32,
}

impl Facade {
    fn new(side: Side, hw: f32, hd: f32) -> Self {
        let n2 = side.normal();
        let n = [n2[0], 0.0, n2[1]];
        let u = [n[2], 0.0, -n[0]];
        Facade {
            c: [n[0] * hw, 0.0, n[2] * hd],
            u,
            n,
            len: if n[0] != 0.0 { 2.0 * hd } else { 2.0 * hw },
        }
    }

    fn p(&self, s: f32, y: f32, o: f32) -> V3 {
        [
            self.c[0] + self.u[0] * s + self.n[0] * o,
            self.c[1] + y,
            self.c[2] + self.u[2] * s + self.n[2] * o,
        ]
    }

    fn push_box(&self, part: &mut MeshPart, s: (f32, f32), y: (f32, f32), o: (f32, f32), color: [f32; 4]) {
        let mut c = [[0.0f32; 3]; 8];
        for (i, corner) in c.iter_mut().enumerate() {
            let sv = if i & 1 == 0 { s.0 } else { s.1 };
            let yv = if i & 2 == 0 { y.0 } else { y.1 };
            let ov = if i & 4 == 0 { o.0 } else { o.1 };
            *corner = self.p(sv, yv, ov);
        }
        push_hex(part, c, color);
    }
}

const PLASTER: [[f32; 3]; 5] = [
    [0.80, 0.74, 0.62],
[0.74, 0.66, 0.52],
[0.70, 0.62, 0.50],
[0.82, 0.78, 0.70],
[0.66, 0.55, 0.42],
];
const STONE: [[f32; 3]; 3] = [[0.50, 0.49, 0.47], [0.58, 0.56, 0.52], [0.44, 0.43, 0.42]];
const ROOFS: [[f32; 3]; 4] = [
    [0.46, 0.20, 0.13],
[0.30, 0.21, 0.15],
[0.27, 0.28, 0.31],
[0.52, 0.38, 0.22],
];
const WOOD: [f32; 3] = [0.20, 0.13, 0.08];
const CHIMNEY: [f32; 3] = [0.38, 0.36, 0.34];

fn jitter(rng: &mut Rng, c: [f32; 3]) -> [f32; 4] {
    let mut out = [1.0f32; 4];
    for k in 0..3 {
        out[k] = (c[k] + rng.range(-0.04, 0.04)).clamp(0.0, 1.0);
    }
    out
}

fn rgba(c: [f32; 3]) -> [f32; 4] {
    [c[0], c[1], c[2], 1.0]
}

pub fn generate_building(
    fp: &Footprint,
    kind: BuildingKind,
    front: Side,
    seed: u32,
    textures: BuildingTextures,
) -> BuildingMesh {
    let mut rng = Rng::new(seed);
    let mut out = BuildingMesh::default();

    out.wall_texture = textures.wall;
    out.roof_texture = textures.roof;
    out.door_texture = textures.door;
    out.window_texture = textures.window;

    let hw = fp.width * 0.5;
    let hd = fp.depth * 0.5;
    let ridge_along_x = fp.width >= fp.depth;
    let (hl, hs) = if ridge_along_x { (hw, hd) } else { (hd, hw) };

    let mut roof_h = (hs * 0.7).clamp(1.2, 4.5);
    let mut wall_h = fp.height - roof_h;
    if wall_h < MIN_WALL_HEIGHT {
        roof_h = (fp.height - MIN_WALL_HEIGHT).max(0.0);
        wall_h = fp.height - roof_h;
    }
    let roof_kind = if roof_h < 0.8 {
        RoofKind::Flat
    } else if kind == BuildingKind::Hall {
        RoofKind::Hip
    } else if rng.chance(0.6) {
        RoofKind::Gable
    } else {
        RoofKind::Hip
    };
    if roof_kind == RoofKind::Flat {
        wall_h = fp.height;
        roof_h = 0.0;
    }
    let top = wall_h + roof_h;
    let floors = ((wall_h / FLOOR_HEIGHT).round() as i32).max(1);
    let floor_h = wall_h / floors as f32;

    let wall_inside = [0.0, wall_h * 0.5, 0.0];
    let roof_inside = [0.0, wall_h - 0.1, 0.0];

    let stone = kind == BuildingKind::Hall || rng.chance(0.25);
    let wall_col = if textures.wall.is_some() {
        [1.0, 1.0, 1.0, 1.0]
    } else if stone {
        let i = rng.pick(STONE.len());
        jitter(&mut rng, STONE[i])
    } else {
        let i = rng.pick(PLASTER.len());
        jitter(&mut rng, PLASTER[i])
    };

    let roof_col = if textures.roof.is_some() {
        [1.0, 1.0, 1.0, 1.0]
    } else {
        let i = rng.pick(ROOFS.len());
        jitter(&mut rng, ROOFS[i])
    };

    let trim_col = rgba(WOOD);
    let [wall, roof, trim, _opening] = &mut out.parts;

    for side in Side::ALL {
        let f = Facade::new(side, hw, hd);
        let l = f.len * 0.5;
        wall.push_face(
            &[f.p(-l, -SKIRT, 0.0), f.p(l, -SKIRT, 0.0), f.p(l, wall_h, 0.0), f.p(-l, wall_h, 0.0)],
                       wall_col,
                       wall_inside,
        );

        let is_gable_end = roof_kind == RoofKind::Gable
        && if ridge_along_x {
            matches!(side, Side::PosX | Side::NegX)
        } else {
            matches!(side, Side::PosZ | Side::NegZ)
        };
        if is_gable_end {
            wall.push_face(
                &[f.p(-l, wall_h, 0.0), f.p(l, wall_h, 0.0), f.p(0.0, top, 0.0)],
                           wall_col,
                           wall_inside,
            );
        }

        for k in 1..floors {
            let y = k as f32 * floor_h;
            f.push_box(trim, (-l - 0.1, l + 0.1), (y - 0.12, y + 0.12), (-0.02, 0.1), trim_col);
        }

        if f.len >= 4.5 {
            let n = (((f.len - 2.0) / 3.2).floor() as i32).clamp(0, 6);
            for i in 0..n {
                let s = ((i as f32 + 0.5) / n as f32 - 0.5) * f.len;
                for k in 0..floors {
                    if side == front && k == 0 && s.abs() < 1.5 {
                        continue;
                    }
                    if rng.chance(0.08) {
                        continue;
                    }
                    let yc = k as f32 * floor_h + floor_h * 0.58;
                    let (y0, y1) = (yc - 0.55, yc + 0.55);

                    let win_pts = [
                        f.p(s - 0.45, y0, 0.03),
                        f.p(s + 0.45, y0, 0.03),
                        f.p(s + 0.45, y1, 0.03),
                        f.p(s - 0.45, y1, 0.03),
                    ];
                    let win_uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
                    push_quad_uv(&mut out.windows, &win_pts, &win_uvs, wall_inside);

                    f.push_box(trim, (s - 0.6, s + 0.6), (y0 - 0.14, y0), (-0.02, 0.14), trim_col);
                }
            }
        }

        if side == front {
            let door_pts = [
                f.p(-0.55, 0.0, 0.03),
                f.p(0.55, 0.0, 0.03),
                f.p(0.55, 2.2, 0.03),
                f.p(-0.55, 2.2, 0.03),
            ];
            let door_uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
            push_quad_uv(&mut out.doors, &door_pts, &door_uvs, wall_inside);

            f.push_box(trim, (-0.7, -0.55), (0.0, 2.4), (-0.02, 0.1), trim_col);
            f.push_box(trim, (0.55, 0.7), (0.0, 2.4), (-0.02, 0.1), trim_col);
            f.push_box(trim, (-0.7, 0.7), (2.2, 2.4), (-0.02, 0.1), trim_col);
            f.push_box(trim, (-0.9, 0.9), (-0.2, 0.15), (-0.02, 0.55), rgba(CHIMNEY));
        }
    }

    let map = |a: f32, y: f32, b: f32| -> V3 {
        if ridge_along_x { [a, y, b] } else { [b, y, a] }
    };
    let slope = if hs > 0.0 { roof_h / hs } else { 0.0 };
    let eave_y = wall_h - EAVE * slope;
    let (ea, eb) = (hl + EAVE, hs + EAVE);

    match roof_kind {
        RoofKind::Flat => {
            let y = top + 0.05;
            roof.push_face(
                &[[-hw, y, -hd], [hw, y, -hd], [hw, y, hd], [-hw, y, hd]],
                roof_col,
                roof_inside,
            );
        }
        RoofKind::Gable => {
            roof.push_face(
                &[map(-ea, eave_y, eb), map(ea, eave_y, eb), map(ea, top, 0.0), map(-ea, top, 0.0)],
                           roof_col,
                           roof_inside,
            );
            roof.push_face(
                &[map(ea, eave_y, -eb), map(-ea, eave_y, -eb), map(-ea, top, 0.0), map(ea, top, 0.0)],
                           roof_col,
                           roof_inside,
            );
        }
        RoofKind::Hip => {
            let rl = (hl - hs).max(0.0);
            let (rp, rm) = (map(rl, top, 0.0), map(-rl, top, 0.0));
            if rl < 0.01 {
                roof.push_face(&[map(-ea, eave_y, eb), map(ea, eave_y, eb), rp], roof_col, roof_inside);
                roof.push_face(&[map(ea, eave_y, -eb), map(-ea, eave_y, -eb), rp], roof_col, roof_inside);
            } else {
                roof.push_face(&[map(-ea, eave_y, eb), map(ea, eave_y, eb), rp, rm], roof_col, roof_inside);
                roof.push_face(&[map(ea, eave_y, -eb), map(-ea, eave_y, -eb), rm, rp], roof_col, roof_inside);
            }
            // Lato est
            roof.push_face(&[map(ea, eave_y, eb), map(ea, eave_y, -eb), rp], roof_col, roof_inside);
            // Lato ovest — prima aveva due vertici identici e la faccia veniva scartata.
            // Ora il secondo vertice è map(-ea, eave_y, eb), che chiude correttamente il triangolo.
            roof.push_face(&[map(-ea, eave_y, -eb), map(-ea, eave_y, eb), rm], roof_col, roof_inside);
        }
    }

    if kind != BuildingKind::Hall && roof_kind != RoofKind::Flat && rng.chance(0.5) {
        let reach = if roof_kind == RoofKind::Gable { hl } else { (hl - hs).max(0.0) } * 0.6;
        if reach > 0.8 {
            let a = if rng.chance(0.5) { reach } else { -reach };
            let c = map(a, 0.0, 0.0);
            push_hex(
                trim,
                [
                    [c[0] - 0.45, wall_h, c[2] - 0.45],
                     [c[0] + 0.45, wall_h, c[2] - 0.45],
                     [c[0] - 0.45, top + 0.9, c[2] - 0.45],
                     [c[0] + 0.45, top + 0.9, c[2] - 0.45],
                     [c[0] - 0.45, wall_h, c[2] + 0.45],
                     [c[0] + 0.45, wall_h, c[2] + 0.45],
                     [c[0] - 0.45, top + 0.9, c[2] + 0.45],
                     [c[0] + 0.45, top + 0.9, c[2] + 0.45],
                ],
                rgba(CHIMNEY),
            );
        }
    }

    if textures.wall.is_some() {
        for uv in wall.uvs.iter_mut() {
            *uv = [uv[0] / BUILDING_TEXTURE_METERS, -uv[1] / BUILDING_TEXTURE_METERS];
        }
    }

    if textures.roof.is_some() {
        for uv in roof.uvs.iter_mut() {
            if ridge_along_x {
                *uv = [uv[0] / ROOF_TEXTURE_METERS, -uv[1] / ROOF_TEXTURE_METERS];
            } else {
                *uv = [uv[1] / ROOF_TEXTURE_METERS, -uv[0] / ROOF_TEXTURE_METERS];
            }
        }
    }

    out
}

pub fn choose_front_side(center: [f32; 2], half: [f32; 2], rot_deg: f32, bounds: [f32; 4]) -> Side {
    let (s, c) = rot_deg.to_radians().sin_cos();
    let mut best = (Side::PosZ, f32::MAX);
    for side in Side::ALL {
        let n = side.normal();
        let wx = n[0] * c + n[1] * s;
        let wz = -n[0] * s + n[1] * c;
        let mut t = f32::MAX;
        if wx > 1e-4 {
            t = t.min((bounds[2] - center[0]) / wx);
        } else if wx < -1e-4 {
            t = t.min((bounds[0] - center[0]) / wx);
        }
        if wz > 1e-4 {
            t = t.min((bounds[3] - center[1]) / wz);
        } else if wz < -1e-4 {
            t = t.min((bounds[1] - center[1]) / wz);
        }
        let reach = if n[0] != 0.0 { half[0] } else { half[1] };
        let gap = t - reach;
        if gap < best.1 {
            best = (side, gap);
        }
    }
    best.0
}

#[derive(Default)]
pub struct CityBuilder {
    parts: [MeshPart; MATERIAL_COUNT],
    textured: Vec<((u16, u8), MeshPart)>,
}

fn append_transformed(dst: &mut MeshPart, src: &MeshPart, offset: [f32; 3], s: f32, c: f32, position: [f32; 3]) {
    let base = dst.positions.len() as u32;
    for i in 0..src.positions.len() {
        let p = src.positions[i];
        let q = [p[0] + offset[0], p[1] + offset[1], p[2] + offset[2]];
        dst.positions.push([
            q[0] * c + q[2] * s + position[0],
            q[1] + position[1],
            -q[0] * s + q[2] * c + position[2],
        ]);
        let n = src.normals[i];
        dst.normals.push([n[0] * c + n[2] * s, n[1], -n[0] * s + n[2] * c]);
        dst.uvs.push(src.uvs[i]);
        dst.colors.push(src.colors[i]);
    }
    dst.indices.extend(src.indices.iter().map(|i| i + base));
}

impl CityBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.parts.iter().all(|p| p.is_empty()) && self.textured.iter().all(|(_, p)| p.is_empty())
    }

    pub fn parts(&self) -> &[MeshPart; MATERIAL_COUNT] {
        &self.parts
    }

    pub fn textured(&self) -> &[((u16, u8), MeshPart)] {
        &self.textured
    }

    pub fn into_parts(self) -> [MeshPart; MATERIAL_COUNT] {
        self.parts
    }

    pub fn into_all(self) -> ([MeshPart; MATERIAL_COUNT], Vec<((u16, u8), MeshPart)>) {
        (self.parts, self.textured)
    }

    fn textured_part(&mut self, key: (u16, u8)) -> &mut MeshPart {
        if let Some(i) = self.textured.iter().position(|(k, _)| *k == key) {
            return &mut self.textured[i].1;
        }
        self.textured.push((key, MeshPart::default()));
        &mut self.textured.last_mut().unwrap().1
    }

    pub fn add(&mut self, b: &BuildingMesh, offset: [f32; 3], rot_deg: f32, position: [f32; 3]) {
        let (s, c) = rot_deg.to_radians().sin_cos();

        let wall = &b.parts[MaterialKind::Wall.index()];
        if !wall.is_empty() {
            let dst = match b.wall_texture {
                Some(key) => self.textured_part(key),
                None => &mut self.parts[MaterialKind::Wall.index()],
            };
            append_transformed(dst, wall, offset, s, c, position);
        }

        let roof = &b.parts[MaterialKind::Roof.index()];
        if !roof.is_empty() {
            let dst = match b.roof_texture {
                Some(key) => self.textured_part(key),
                None => &mut self.parts[MaterialKind::Roof.index()],
            };
            append_transformed(dst, roof, offset, s, c, position);
        }

        let trim = &b.parts[MaterialKind::Trim.index()];
        if !trim.is_empty() {
            append_transformed(&mut self.parts[MaterialKind::Trim.index()], trim, offset, s, c, position);
        }

        if !b.doors.is_empty() {
            let dst = match b.door_texture {
                Some(key) => self.textured_part(key),
                None => &mut self.parts[MaterialKind::Opening.index()],
            };
            append_transformed(dst, &b.doors, offset, s, c, position);
        }

        if !b.windows.is_empty() {
            let dst = match b.window_texture {
                Some(key) => self.textured_part(key),
                None => &mut self.parts[MaterialKind::Opening.index()],
            };
            append_transformed(dst, &b.windows, offset, s, c, position);
        }
    }
}
