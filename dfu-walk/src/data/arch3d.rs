//! Lettura dei modelli di ARCH3D.BSA con estrazione automatica di pareti, tetti, porte e finestre.

pub const MODEL_TO_METERS: f32 = 0.025 / 256.0;

const HEADER_LEN: usize = 64;
const PLANE_HEADER_LEN: usize = 8;
const PLANE_POINT_LEN: usize = 8;
const MAX_POINTS: usize = 200_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildingTextures {
    pub wall: Option<(u16, u8)>,
    pub roof: Option<(u16, u8)>,
    pub door: Option<(u16, u8)>,
    pub window: Option<(u16, u8)>,
}

#[derive(Debug, Clone)]
pub struct Arch3dFace {
    pub texture_archive: u16,
    pub texture_record: u8,
    pub vertex_indices: Vec<u32>,
    pub uvs: Vec<[f32; 2]>,
}

#[derive(Debug, Clone)]
pub struct Arch3dModel {
    pub id: u32,
    pub vertices: Vec<[f32; 3]>,
    pub faces: Vec<Arch3dFace>,
}

#[derive(Debug, Clone, Copy)]
pub struct ParseInfo {
    pub version: [u8; 4],
    pub point_count: usize,
    pub plane_count: usize,
    pub header_point_offset: i32,
    pub header_plane_offset: i32,
    pub point_start: usize,
    pub plane_start: usize,
    pub planes_from_header: bool,
    pub points_from_header: bool,
    pub radius_header: f32,
    pub radius_computed: f32,
}

fn i32_at(d: &[u8], p: usize) -> i32 {
    i32::from_le_bytes(d[p..p + 4].try_into().unwrap())
}

fn i16_at(d: &[u8], p: usize) -> i16 {
    i16::from_le_bytes(d[p..p + 2].try_into().unwrap())
}

/// UV grezza: intero a 16 bit con segno, in 1/16 di pixel. Non va tagliata a 12 bit: le pareti
/// larghe hanno valori come 0x0800, 0x1000, 0x1800 (2, 4, 6 ripetizioni di una texture da 64 px)
/// che il taglio a 12 bit azzererebbe o cambierebbe di segno.
fn uv12(raw: i16) -> f32 {
    raw as f32
}

struct RawPlane {
    texture: u16,
    points: Vec<(u32, i16, i16)>,
}

fn offset_divisor(version: &[u8; 4]) -> i64 {
    if version == b"v2.5" { 4 } else { 12 }
}

fn walk_planes(
    data: &[u8],
    start: usize,
    plane_count: usize,
    point_count: usize,
    version: &[u8; 4],
) -> Option<Vec<RawPlane>> {
    let div = offset_divisor(version);
    let mut p = start;
    let mut planes = Vec::with_capacity(plane_count);
    for _ in 0..plane_count {
        if p.checked_add(PLANE_HEADER_LEN)? > data.len() {
            return None;
        }
        let n = data[p] as usize;
        let texture = u16::from_le_bytes([data[p + 2], data[p + 3]]);
        if n < 3 {
            return None;
        }
        let end = p + PLANE_HEADER_LEN + n * PLANE_POINT_LEN;
        if end > data.len() {
            return None;
        }
        let mut points = Vec::with_capacity(n);
        let mut q = p + PLANE_HEADER_LEN;
        for _ in 0..n {
            let off = i32_at(data, q) as i64;
            if off < 0 || off % div != 0 || off / div >= point_count as i64 {
                return None;
            }
            points.push(((off / div) as u32, i16_at(data, q + 4), i16_at(data, q + 6)));
            q += PLANE_POINT_LEN;
        }
        let mut distinct: Vec<u32> = points.iter().map(|p| p.0).collect();
        distinct.sort_unstable();
        distinct.dedup();
        if distinct.len() < 3 {
            return None;
        }
        planes.push(RawPlane { texture, points });
        p = end;
    }
    Some(planes)
}

fn search_planes(
    data: &[u8],
    from: usize,
    plane_count: usize,
    point_count: usize,
    version: &[u8; 4],
) -> Option<(usize, Vec<RawPlane>)> {
    let last = data.len().min(from.saturating_add(8192));
    (from..last).find_map(|s| walk_planes(data, s, plane_count, point_count, version).map(|p| (s, p)))
}

fn read_points(data: &[u8], start: usize, count: usize) -> Option<Vec<[i32; 3]>> {
    let end = start.checked_add(count.checked_mul(12)?)?;
    if end > data.len() {
        return None;
    }
    Some(
        (0..count)
            .map(|i| {
                let p = start + i * 12;
                [i32_at(data, p), i32_at(data, p + 4), i32_at(data, p + 8)]
            })
            .collect(),
    )
}

fn max_radius(points: &[[i32; 3]]) -> f32 {
    points
        .iter()
        .map(|p| ((p[0] as f64).powi(2) + (p[1] as f64).powi(2) + (p[2] as f64).powi(2)).sqrt() as f32)
        .fold(0.0, f32::max)
}

fn plane_uvs(raw: &[(u32, i16, i16)], positions: &[[f32; 3]]) -> Vec<[f32; 2]> {
    let n = raw.len();
    let mut uv: Vec<[f32; 2]> = raw.iter().map(|&(_, u, v)| [uv12(u), uv12(v)]).collect();
    if n >= 2 {
        uv[1][0] += uv[0][0];
        uv[1][1] += uv[0][1];
    }
    if n >= 3 {
        uv[2][0] += uv[1][0];
        uv[2][1] += uv[1][1];
    }
    if n > 4 {
        let pos = |i: usize| positions.get(raw[i].0 as usize).copied();
        let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        for k in [2usize, 3] {
            let (Some(p0), Some(p1), Some(pk)) = (pos(0), pos(1), pos(k)) else { continue };
            let (e1, e2) = (sub(p1, p0), sub(pk, p0));
            let (g11, g12, g22) = (dot(e1, e1), dot(e1, e2), dot(e2, e2));
            let det = g11 * g22 - g12 * g12;
            if det.abs() <= 1e-9 * (g11 * g22).max(1e-30) {
                continue;
            }
            let (du1, dv1) = (uv[1][0] - uv[0][0], uv[1][1] - uv[0][1]);
            let (du2, dv2) = (uv[k][0] - uv[0][0], uv[k][1] - uv[0][1]);
            for i in 4..n {
                let Some(pi) = pos(i) else { continue };
                let d = sub(pi, p0);
                let (b1, b2) = (dot(d, e1), dot(d, e2));
                let alpha = (b1 * g22 - b2 * g12) / det;
                let beta = (g11 * b2 - g12 * b1) / det;
                uv[i] = [uv[0][0] + alpha * du1 + beta * du2, uv[0][1] + alpha * dv1 + beta * dv2];
            }
            break;
        }
    }
    uv
}

impl Arch3dModel {
    pub fn parse(id: u32, data: &[u8]) -> Result<Self, &'static str> {
        Self::parse_with_info(id, data).map(|(m, _)| m)
    }

    pub fn parse_with_info(id: u32, data: &[u8]) -> Result<(Self, ParseInfo), &'static str> {
        if data.len() < HEADER_LEN {
            return Err("Dati modello ARCH3D troppo brevi");
        }
        let version: [u8; 4] = data[0..4].try_into().unwrap();
        let point_count = i32_at(data, 4);
        let plane_count = i32_at(data, 8);
        if point_count < 3 || plane_count < 1 || point_count as usize > MAX_POINTS || plane_count as usize > MAX_POINTS {
            return Err("PointCount o PlaneCount non validi");
        }
        let (point_count, plane_count) = (point_count as usize, plane_count as usize);
        let radius_header = i32_at(data, 12) as f32;
        let header_point_offset = i32_at(data, 48);
        let header_plane_offset = i32_at(data, 60);

        let from_header = (header_plane_offset >= HEADER_LEN as i32)
            .then(|| header_plane_offset as usize)
            .and_then(|s| walk_planes(data, s, plane_count, point_count, &version).map(|p| (s, p)));
        let planes_from_header = from_header.is_some();
        let (plane_start, planes) = match from_header {
            Some(found) => found,
            None => search_planes(data, HEADER_LEN, plane_count, point_count, &version)
                .ok_or("Piani ARCH3D non validi")?,
        };

        let mut candidates: Vec<usize> = Vec::new();
        if header_point_offset >= 0 {
            candidates.push(header_point_offset as usize);
        }
        for c in [HEADER_LEN, 32] {
            if !candidates.contains(&c) {
                candidates.push(c);
            }
        }
        let mut best: Option<(f32, usize, Vec<[i32; 3]>)> = None;
        for &c in &candidates {
            let Some(points) = read_points(data, c, point_count) else { continue };
            let err = (max_radius(&points) - radius_header).abs() / radius_header.abs().max(1.0);
            if best.as_ref().map_or(true, |(e, _, _)| err < *e - 1e-6) {
                best = Some((err, c, points));
            }
            if err < 0.01 {
                break;
            }
        }
        let (_, point_start, raw_points) = best.ok_or("Punti ARCH3D oltre fine record")?;
        let radius_computed = max_radius(&raw_points);

        let vertices: Vec<[f32; 3]> = raw_points
            .iter()
            .map(|p| [p[0] as f32 * MODEL_TO_METERS, -(p[1] as f32) * MODEL_TO_METERS, p[2] as f32 * MODEL_TO_METERS])
            .collect();
        let raw_pos: Vec<[f32; 3]> = raw_points.iter().map(|p| [p[0] as f32, p[1] as f32, p[2] as f32]).collect();

        // Diagnostica UV: con DF_UV_DEBUG=1 stampa i piani piu' larghi di 3 m
        if std::env::var("DF_UV_DEBUG").is_ok() {
            for (fi, pl) in planes.iter().enumerate() {
                let n = pl.points.len();
                let pos = |i: usize| raw_pos[pl.points[i].0 as usize];
                let dist = |a: [f32; 3], b: [f32; 3]| {
                    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt() * MODEL_TO_METERS
                };
                let widest = (0..n).map(|i| dist(pos(i), pos((i + 1) % n))).fold(0.0, f32::max);
                if widest > 3.0 {
                    let raw: Vec<String> = pl
                        .points
                        .iter()
                        .take(4)
                        .map(|&(_, u, v)| format!("{:04X}/{:04X}", u as u16, v as u16))
                        .collect();
                    let uv = plane_uvs(&pl.points, &raw_pos);
                    println!(
                        "UV dbg modello {id} piano {fi}: tex {}/{} n={n} larghezza max {widest:.2} m, raw u/v {raw:?}, uv {:?}",
                        pl.texture >> 7,
                        pl.texture & 0x7F,
                        &uv[..uv.len().min(4)]
                    );
                }
            }
        }

        let faces = planes
            .iter()
            .map(|pl| {
                let uv = plane_uvs(&pl.points, &raw_pos);
                Arch3dFace {
                    texture_archive: pl.texture >> 7,
                    texture_record: (pl.texture & 0x7F) as u8,
                    vertex_indices: pl.points.iter().map(|p| p.0).collect(),
                    uvs: uv.into_iter().map(|[u, v]| [u / 16.0, v / 16.0]).collect(),
                }
            })
            .collect();

        let info = ParseInfo {
            version,
            point_count,
            plane_count,
            header_point_offset,
            header_plane_offset,
            point_start,
            plane_start,
            planes_from_header,
            points_from_header: header_point_offset >= 0 && point_start == header_point_offset as usize,
            radius_header,
            radius_computed,
        };
        Ok((Arch3dModel { id, vertices, faces }, info))
    }

    pub fn dominant_texture(&self) -> Option<(u16, u8)> {
        self.building_textures().wall
    }

    /// Riconosce ed estrae le texture separate per pareti, tetto, porte e finestre
    pub fn building_textures(&self) -> BuildingTextures {
        let mut sloped_areas: Vec<((u16, u8), f32)> = Vec::new();
        let mut vertical_areas: Vec<((u16, u8), f32)> = Vec::new();
        let mut door: Option<(u16, u8)> = None;
        let mut window: Option<(u16, u8)> = None;

        for f in &self.faces {
            if f.vertex_indices.len() < 3 {
                continue;
            }
            let key = (f.texture_archive, f.texture_record);
            let v0 = self.vertices.get(f.vertex_indices[0] as usize).copied().unwrap_or([0.0; 3]);
            let v1 = self.vertices.get(f.vertex_indices[1] as usize).copied().unwrap_or([0.0; 3]);
            let v2 = self.vertices.get(f.vertex_indices[2] as usize).copied().unwrap_or([0.0; 3]);
            let e1 = [v1[0]-v0[0], v1[1]-v0[1], v1[2]-v0[2]];
            let e2 = [v2[0]-v0[0], v2[1]-v0[1], v2[2]-v0[2]];
            let ny = e1[2]*e2[0] - e1[0]*e2[2];
            let area = 0.5 * ((e1[1]*e2[2]-e1[2]*e2[1]).powi(2) + ny.powi(2) + (e1[0]*e2[1]-e1[1]*e2[0]).powi(2)).sqrt();

            let arch_base = f.texture_archive % 100;
            // Porta
            if arch_base == 74 {
                door = Some(key);
            }
            // Finestra
            if (arch_base == 71 || f.texture_record == 3 || f.texture_record == 4) && area <= 18.0 {
                if window.is_none() {
                    window = Some(key);
                }
            }

            // Tetti: riconosce solo gli archivi di copertura originali (69 e varianti)
            if arch_base == 69 || (ny.abs() > 0.15 && (f.texture_archive >= 60 && f.texture_archive <= 70 || f.texture_archive >= 160 && f.texture_archive <= 170)) {
                match sloped_areas.iter_mut().find(|(k, _)| *k == key) {
                    Some(entry) => entry.1 += area,
                    None => sloped_areas.push((key, area)),
                }
            } else if ny.abs() <= 0.10 {
                match vertical_areas.iter_mut().find(|(k, _)| *k == key) {
                    Some(entry) => entry.1 += area,
                    None => vertical_areas.push((key, area)),
                }
            }
        }

        let wall = vertical_areas
            .into_iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(k, _)| k);

        let raw_roof = sloped_areas
            .into_iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(k, _)| k);

        // Riporta il tetto all'archivio base 69 così apply_climate assegna le tegole corrette
        let roof = raw_roof.map(|(a, r)| if a % 100 == 69 { (69, r) } else { (a, r) });

        BuildingTextures {
            wall,
            roof: roof.or(Some((69, 0))),        // base 69 -> climi temperati 169 (tegole rosse)
            door: door.or(Some((74, 0))),       // porta in legno originale
            window: window.or(Some((171, 4))),  // vetri piombati ambrati
        }
    }
}

pub fn decode_face_texture(archive: u16, record: u8) -> (u16, u8) {
    (archive, record)
}
