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
    /// Piani scartati perche' degeneri (meno di 3 punti o meno di 3 vertici distinti).
    pub degenerate_planes: usize,
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

/// Percorre `plane_count` piani a partire da `start`. In modalita' rigida un piano con meno di 3
/// punti (o meno di 3 vertici distinti) invalida tutto il percorso: serve a riconoscere l'inizio
/// vero dei piani quando l'header non e' affidabile. In modalita' `lenient` quei piani vengono
/// saltati (e contati) ma la struttura (dimensioni, offset dei punti) resta validata: cosi' un
/// modello con un piano degenere non viene scartato e non si aggancia a un falso inizio.
fn walk_planes(
    data: &[u8],
    start: usize,
    plane_count: usize,
    point_count: usize,
    version: &[u8; 4],
    lenient: bool,
) -> Option<(Vec<RawPlane>, usize)> {
    let div = offset_divisor(version);
    let mut p = start;
    let mut skipped = 0usize;
    let mut planes = Vec::with_capacity(plane_count);
    for _ in 0..plane_count {
        if p.checked_add(PLANE_HEADER_LEN)? > data.len() {
            return None;
        }
        let n = data[p] as usize;
        let texture = u16::from_le_bytes([data[p + 2], data[p + 3]]);
        if n < 3 && !lenient {
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
        if n < 3 || distinct.len() < 3 {
            if !lenient {
                return None;
            }
            skipped += 1;
        } else {
            planes.push(RawPlane { texture, points });
        }
        p = end;
    }
    if planes.is_empty() {
        return None;
    }
    Some((planes, skipped))
}

fn search_planes(
    data: &[u8],
    from: usize,
    plane_count: usize,
    point_count: usize,
    version: &[u8; 4],
    lenient: bool,
) -> Option<(usize, Vec<RawPlane>, usize)> {
    let last = data.len().min(from.saturating_add(8192));
    (from..last).find_map(|s| {
        walk_planes(data, s, plane_count, point_count, version, lenient).map(|(p, skipped)| (s, p, skipped))
    })
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

impl Arch3dFace {
    /// Triangola la faccia con ear clipping, anche quando e' concava (siepi, cornici, croci...).
    ///
    /// Restituisce terne di indici LOCALI (posizione dentro la faccia), validi sia per
    /// `vertex_indices` che per `uvs`, con lo stesso senso di avvolgimento della faccia. Una
    /// triangolazione "a ventaglio" dal vertice 0 va bene solo per i poligoni convessi: sulle
    /// facce da 6-12 punti dei modelli di siepe produce triangoli fuori dal contorno e buchi.
    /// Se la faccia e' degenere (area nulla, vertici mancanti) ripiega sul ventaglio.
    pub fn triangulate(&self, vertices: &[[f32; 3]]) -> Vec<[usize; 3]> {
        let n = self.vertex_indices.len();
        let fan = || (1..n.saturating_sub(1)).map(|i| [0, i, i + 1]).collect::<Vec<_>>();
        if n < 3 {
            return Vec::new();
        }
        if n == 3 {
            return vec![[0, 1, 2]];
        }
        let Some(pts) = self
            .vertex_indices
            .iter()
            .map(|&i| vertices.get(i as usize).copied())
            .collect::<Option<Vec<[f32; 3]>>>()
        else {
            return fan();
        };

        // Normale di Newell e proiezione sul piano dominante (si scarta l'asse piu' "ortogonale").
        let mut nrm = [0.0f64; 3];
        for i in 0..n {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            nrm[0] += (a[1] as f64 - b[1] as f64) * (a[2] as f64 + b[2] as f64);
            nrm[1] += (a[2] as f64 - b[2] as f64) * (a[0] as f64 + b[0] as f64);
            nrm[2] += (a[0] as f64 - b[0] as f64) * (a[1] as f64 + b[1] as f64);
        }
        let abs = [nrm[0].abs(), nrm[1].abs(), nrm[2].abs()];
        let drop = if abs[0] >= abs[1] && abs[0] >= abs[2] { 0 } else if abs[1] >= abs[2] { 1 } else { 2 };
        if abs[drop] < 1e-12 {
            return fan();
        }
        let (ua, va) = match drop {
            0 => (1, 2),
            1 => (2, 0),
            _ => (0, 1),
        };
        let p2: Vec<[f64; 2]> = pts.iter().map(|p| [p[ua] as f64, p[va] as f64]).collect();
        // Con (ua, va) ciclici, il verso del poligono nel piano 2D e' il segno della componente scartata.
        let orient = if nrm[drop] >= 0.0 { 1.0 } else { -1.0 };
        let cross = |a: [f64; 2], b: [f64; 2], c: [f64; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);

        let mut idx: Vec<usize> = (0..n).collect();
        let mut out: Vec<[usize; 3]> = Vec::with_capacity(n - 2);
        let eps = 1e-9 * abs[drop].max(1.0);
        while idx.len() > 3 {
            let m = idx.len();
            let mut clipped = false;
            for k in 0..m {
                let (ip, ic, inx) = (idx[(k + m - 1) % m], idx[k], idx[(k + 1) % m]);
                let (a, b, c) = (p2[ip], p2[ic], p2[inx]);
                if cross(a, b, c) * orient <= eps {
                    continue; // vertice concavo o allineato: non e' un'orecchia
                }
                let blocked = idx.iter().any(|&j| {
                    if j == ip || j == ic || j == inx {
                        return false;
                    }
                    let q = p2[j];
                    if q == a || q == b || q == c {
                        return false; // vertice duplicato (contorno che si tocca)
                    }
                    cross(a, b, q) * orient >= -eps && cross(b, c, q) * orient >= -eps && cross(c, a, q) * orient >= -eps
                });
                if !blocked {
                    out.push([ip, ic, inx]);
                    idx.remove(k);
                    clipped = true;
                    break;
                }
            }
            if !clipped {
                // Contorno degenere: elimina il vertice piu' allineato e riprova.
                let k = (0..m)
                    .min_by(|&x, &y| {
                        let area = |k: usize| cross(p2[idx[(k + m - 1) % m]], p2[idx[k]], p2[idx[(k + 1) % m]]).abs();
                        area(x).partial_cmp(&area(y)).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .unwrap_or(0);
                idx.remove(k);
            }
        }
        if idx.len() == 3 && cross(p2[idx[0]], p2[idx[1]], p2[idx[2]]).abs() > eps {
            out.push([idx[0], idx[1], idx[2]]);
        }
        if out.is_empty() { fan() } else { out }
    }
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

        // 1) offset dell'header (rigido, poi tollerante ai piani degeneri); 2) ricerca (idem).
        let from_header = (header_plane_offset >= HEADER_LEN as i32)
            .then(|| header_plane_offset as usize)
            .and_then(|s| {
                walk_planes(data, s, plane_count, point_count, &version, false)
                    .or_else(|| walk_planes(data, s, plane_count, point_count, &version, true))
                    .map(|(p, skipped)| (s, p, skipped))
            });
        let planes_from_header = from_header.is_some();
        let (plane_start, planes, degenerate_planes) = match from_header {
            Some(found) => found,
            None => search_planes(data, HEADER_LEN, plane_count, point_count, &version, false)
                .or_else(|| search_planes(data, HEADER_LEN, plane_count, point_count, &version, true))
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

        let points_from_header = header_point_offset >= 0 && point_start == header_point_offset as usize;
        // Con DF_MODEL_DEBUG=1 segnala i modelli "anomali" (utile per trovare quello di un oggetto che
        // si carica male): piani degeneri scartati o struttura non trovata dove dice l'header.
        if std::env::var_os("DF_MODEL_DEBUG").is_some()
            && (degenerate_planes > 0 || !planes_from_header || !points_from_header)
        {
            eprintln!(
                "ARCH3D modello {id}: versione {} punti {point_count} piani {plane_count} (scartati {degenerate_planes}), \
piani da header: {planes_from_header} (header {header_plane_offset}, trovati {plane_start}), \
punti da header: {points_from_header} (header {header_point_offset}, trovati {point_start}), \
raggio header {radius_header} / calcolato {radius_computed}",
                String::from_utf8_lossy(&version),
            );
        }

        let info = ParseInfo {
            version,
            point_count,
            plane_count,
            header_point_offset,
            header_plane_offset,
            point_start,
            plane_start,
            planes_from_header,
            points_from_header,
            radius_header,
            radius_computed,
            degenerate_planes,
        };
        Ok((Arch3dModel { id, vertices, faces }, info))
    }

    /// Descrizione testuale completa del modello (vertici, facce, copertura della triangolazione),
    /// per diagnosticare geometrie che si caricano male. Vedi `DF_MODEL_DUMP` in `city_buildings.rs`.
    pub fn dump(&self) -> String {
        use std::fmt::Write;
        let mut o = String::new();
        let _ = writeln!(o, "MODELLO {}: {} vertici, {} facce", self.id, self.vertices.len(), self.faces.len());
        for (i, v) in self.vertices.iter().enumerate() {
            let _ = writeln!(o, "  v{i}: ({:.3}, {:.3}, {:.3}) m", v[0], v[1], v[2]);
        }
        for (fi, f) in self.faces.iter().enumerate() {
            let n = f.vertex_indices.len();
            let mut distinct = f.vertex_indices.clone();
            distinct.sort_unstable();
            distinct.dedup();
            let pts: Vec<[f32; 3]> =
                f.vertex_indices.iter().map(|&i| self.vertices.get(i as usize).copied().unwrap_or([0.0; 3])).collect();
            // Area con Newell (contorno) e area coperta dai triangoli: devono coincidere.
            let mut nrm = [0.0f64; 3];
            for i in 0..n {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                nrm[0] += (a[1] as f64 - b[1] as f64) * (a[2] as f64 + b[2] as f64);
                nrm[1] += (a[2] as f64 - b[2] as f64) * (a[0] as f64 + b[0] as f64);
                nrm[2] += (a[0] as f64 - b[0] as f64) * (a[1] as f64 + b[1] as f64);
            }
            let outline = 0.5 * (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt();
            let tris = f.triangulate(&self.vertices);
            let covered: f64 = tris
                .iter()
                .map(|t| {
                    let (a, b, c) = (pts[t[0]], pts[t[1]], pts[t[2]]);
                    let (e1, e2) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
                    let cr = [
                        e1[1] * e2[2] - e1[2] * e2[1],
                        e1[2] * e2[0] - e1[0] * e2[2],
                        e1[0] * e2[1] - e1[1] * e2[0],
                    ];
                    0.5 * ((cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2]) as f64).sqrt()
                })
                .sum();
            let ratio = if outline > 1e-9 { covered / outline } else { 1.0 };
            let _ = writeln!(
                o,
                "  f{fi}: tex {}/{} punti {n} (distinti {}) idx {:?} uv {:?} | contorno {outline:.3} m2, triangoli {} coprono {covered:.3} m2 ({:.0}%){}",
                f.texture_archive,
                f.texture_record,
                distinct.len(),
                f.vertex_indices,
                f.uvs.iter().map(|u| [(u[0] * 100.0).round() / 100.0, (u[1] * 100.0).round() / 100.0]).collect::<Vec<_>>(),
                tris.len(),
                ratio * 100.0,
                if (ratio - 1.0).abs() > 0.01 { "  <-- COPERTURA INCOMPLETA" } else { "" },
            );
        }
        o
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Modello sintetico v2.7: punti a 64, piani subito dopo. Ogni piano = (n punti, indici vertice).
    fn build(points: &[[i32; 3]], planes: &[&[u32]]) -> Vec<u8> {
        let mut d = vec![0u8; HEADER_LEN];
        d[0..4].copy_from_slice(b"v2.7");
        d[4..8].copy_from_slice(&(points.len() as i32).to_le_bytes());
        d[8..12].copy_from_slice(&(planes.len() as i32).to_le_bytes());
        let radius = max_radius(points) as i32;
        d[12..16].copy_from_slice(&radius.to_le_bytes());
        d[48..52].copy_from_slice(&(HEADER_LEN as i32).to_le_bytes());
        for p in points {
            for c in p {
                d.extend_from_slice(&c.to_le_bytes());
            }
        }
        let plane_start = d.len() as i32;
        d[60..64].copy_from_slice(&plane_start.to_le_bytes());
        for (i, pl) in planes.iter().enumerate() {
            d.extend_from_slice(&[pl.len() as u8, 0]);
            d.extend_from_slice(&(((74u16) << 7) | i as u16).to_le_bytes());
            d.extend_from_slice(&[0; 4]);
            for &v in pl.iter() {
                d.extend_from_slice(&((v * 12) as i32).to_le_bytes());
                d.extend_from_slice(&0i16.to_le_bytes());
                d.extend_from_slice(&0i16.to_le_bytes());
            }
        }
        d
    }

    const QUAD: [[i32; 3]; 4] = [[0, 0, 0], [256, 0, 0], [256, 256, 0], [0, 256, 0]];

    #[test]
    fn regular_model_parses_without_degenerate_planes() {
        let d = build(&QUAD, &[&[0, 1, 2, 3], &[0, 2, 3]]);
        let (m, info) = Arch3dModel::parse_with_info(1, &d).unwrap();
        assert_eq!(m.faces.len(), 2);
        assert!(info.planes_from_header && info.points_from_header);
        assert_eq!(info.degenerate_planes, 0);
    }

    #[test]
    fn degenerate_planes_are_skipped_instead_of_dropping_the_model() {
        // Un piano a 2 punti e uno con vertici ripetuti, in mezzo a due piani validi.
        let d = build(&QUAD, &[&[0, 1, 2, 3], &[0, 1], &[0, 0, 1], &[1, 2, 3]]);
        let (m, info) = Arch3dModel::parse_with_info(2, &d).unwrap();
        assert_eq!(m.faces.len(), 2, "restano solo i due piani validi");
        assert_eq!(m.faces[0].vertex_indices, vec![0, 1, 2, 3]);
        assert_eq!(m.faces[1].vertex_indices, vec![1, 2, 3]);
        assert_eq!(info.degenerate_planes, 2);
        assert!(info.planes_from_header, "la struttura va letta dove dice l'header, non cercata");
        assert_eq!(info.plane_start, HEADER_LEN + 4 * 12);
    }

    #[test]
    fn model_with_only_degenerate_planes_still_fails() {
        let d = build(&QUAD, &[&[0, 1], &[2, 2, 2]]);
        assert!(Arch3dModel::parse(3, &d).is_err());
    }

    fn face(pts: &[[f32; 2]]) -> (Arch3dFace, Vec<[f32; 3]>) {
        // Poligono verticale sul piano XY (z = 0), come una siepe vista di fronte.
        let vertices: Vec<[f32; 3]> = pts.iter().map(|p| [p[0], p[1], 0.0]).collect();
        let face = Arch3dFace {
            texture_archive: 33,
            texture_record: 0,
            vertex_indices: (0..pts.len() as u32).collect(),
            uvs: vec![[0.0, 0.0]; pts.len()],
        };
        (face, vertices)
    }

    fn area2(pts: &[[f32; 2]]) -> f32 {
        let n = pts.len();
        (0..n).map(|i| pts[i][0] * pts[(i + 1) % n][1] - pts[(i + 1) % n][0] * pts[i][1]).sum::<f32>() * 0.5
    }

    fn tri_area_sum(tris: &[[usize; 3]], pts: &[[f32; 2]]) -> f32 {
        tris.iter()
            .map(|t| area2(&[pts[t[0]], pts[t[1]], pts[t[2]]]))
            .sum()
    }

    fn tris_ccw(pts: &[[f32; 2]]) -> Vec<[usize; 3]> {
        let (f, v) = face(pts);
        f.triangulate(&v)
    }

    #[test]
    fn convex_quad_gives_two_triangles() {
        let pts = [[0.0, 0.0], [4.0, 0.0], [4.0, 2.0], [0.0, 2.0]];
        let (f, v) = face(&pts);
        let tris = f.triangulate(&v);
        assert_eq!(tris.len(), 2);
        assert!((tri_area_sum(&tris, &pts) - 8.0).abs() < 1e-4);
    }

    #[test]
    fn concave_hedge_profile_is_filled_exactly_in_both_windings() {
        // Siepe a merli: 10 punti, contorno concavo. Con il ventaglio dal vertice 0 il riempimento sbaglia.
        let ccw = [
            [0.0, 0.0], [10.0, 0.0], [10.0, 3.0], [8.0, 3.0], [8.0, 2.0], [6.0, 2.0],
            [6.0, 3.0], [4.0, 3.0], [4.0, 2.0], [0.0, 2.0],
        ];
        let area = area2(&ccw);
        assert!(area > 0.0);
        let (f, v) = face(&ccw);
        let tris = f.triangulate(&v);
        assert_eq!(tris.len(), 8);
        assert!((tri_area_sum(&tris, &ccw) - area).abs() < 1e-4, "ccw {tris:?}");
        // Verso inverso: l'area firmata resta coerente (tutti i triangoli col verso della faccia).
        let cw: Vec<[f32; 2]> = ccw.iter().rev().copied().collect();
        let (f, v) = face(&cw);
        let tris = f.triangulate(&v);
        assert_eq!(tris.len(), 8);
        assert!((tri_area_sum(&tris, &cw) - area2(&cw)).abs() < 1e-4, "cw {tris:?}");
        // Il ventaglio sbaglia su questo contorno: e' il difetto che rende "bucate" le siepi.
        let fan: Vec<[usize; 3]> = (1..ccw.len() - 1).map(|i| [0, i, i + 1]).collect();
        let flipped = |tris: &[[usize; 3]], pts: &[[f32; 2]]| {
            tris.iter().filter(|t| area2(&[pts[t[0]], pts[t[1]], pts[t[2]]]) <= 1e-6).count()
        };
        assert!(flipped(&fan, &ccw) > 0, "il ventaglio sarebbe corretto: triangoli ribaltati o nulli");
        assert_eq!(flipped(&tris_ccw(&ccw), &ccw), 0, "l'ear clipping non deve ribaltare nulla");
    }

    #[test]
    fn l_shape_and_collinear_points_are_handled() {
        let pts = [[0.0, 0.0], [2.0, 0.0], [4.0, 0.0], [4.0, 1.0], [1.0, 1.0], [1.0, 4.0], [0.0, 4.0]];
        let (f, v) = face(&pts);
        let tris = f.triangulate(&v);
        assert!((tri_area_sum(&tris, &pts) - area2(&pts)).abs() < 1e-4, "{tris:?}");
    }

    #[test]
    fn works_on_tilted_and_horizontal_planes_and_falls_back_when_degenerate() {
        // Faccia orizzontale (cima della siepe): piano XZ.
        let v: Vec<[f32; 3]> = vec![[0.0, 1.0, 0.0], [0.0, 1.0, 4.0], [3.0, 1.0, 4.0], [3.0, 1.0, 2.0], [1.0, 1.0, 2.0], [1.0, 1.0, 0.0]];
        let f = Arch3dFace { texture_archive: 33, texture_record: 0, vertex_indices: (0..6).collect(), uvs: vec![[0.0; 2]; 6] };
        let tris = f.triangulate(&v);
        assert_eq!(tris.len(), 4);
        // Faccia degenere (tutti i punti uguali): nessun panic, ripiega sul ventaglio.
        let v = vec![[1.0f32, 1.0, 1.0]; 5];
        let f = Arch3dFace { texture_archive: 33, texture_record: 0, vertex_indices: (0..5).collect(), uvs: vec![[0.0; 2]; 5] };
        assert_eq!(f.triangulate(&v).len(), 3);
        // Vertici mancanti: ventaglio.
        let f = Arch3dFace { texture_archive: 33, texture_record: 0, vertex_indices: vec![0, 1, 99, 3], uvs: vec![[0.0; 2]; 4] };
        assert_eq!(f.triangulate(&[[0.0; 3]; 4]).len(), 2);
    }

    #[test]
    fn keyhole_polygon_with_a_bridge_edge_keeps_its_hole() {
        // Quadrato 10x10 con foro 4x4, collegato da un ponte (vertici ripetuti): 10 punti.
        let pts = [
            [0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0], [0.0, 0.0],
            [3.0, 3.0], [3.0, 7.0], [7.0, 7.0], [7.0, 3.0], [3.0, 3.0],
        ];
        let (f, v) = face(&pts);
        let tris = f.triangulate(&v);
        let covered: f32 = tris.iter().map(|t| area2(&[pts[t[0]], pts[t[1]], pts[t[2]]]).abs()).sum();
        assert!((covered - 84.0).abs() < 1e-3, "area coperta {covered} invece di 84: {tris:?}");
    }

    #[test]
    fn dump_reports_complete_coverage_for_a_concave_face() {
        let prof = [[0.0, 0.0], [10.0, 0.0], [10.0, 3.0], [8.0, 3.0], [8.0, 2.0], [6.0, 2.0], [6.0, 3.0], [4.0, 3.0], [4.0, 2.0], [0.0, 2.0f32]];
        let (f, vertices) = face(&prof);
        let m = Arch3dModel { id: 750, vertices, faces: vec![f] };
        let d = m.dump();
        assert!(d.contains("MODELLO 750: 10 vertici, 1 facce") && d.contains("(100%)"), "{d}");
        assert!(!d.contains("COPERTURA INCOMPLETA"));
    }
}
