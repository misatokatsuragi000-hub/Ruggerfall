//! Generatore procedurale di edifici custom per Daggerfall:
//! - Templi Pagani Classici degli Otto Divini (crepidoma a gradoni, maestoso pronao con colonnato ellenico-romano,
//!   trabeazione continua, grande timpano triangolare a due falde, portale ad arco in pietra con ante door.png).
//! - Torre della Gilda dei Maghi: **ottagono** solido senza forature, portale ad arco su facciata Sud piana,
//!   **7 piani** con **4 finestre per piano alternate** (lati pari) **in stile Palazzo Reale** a mezza scala
//!   (davanzale, stipiti, arco a tutto sesto, chiave di volta, vetro con gradiente diagonale, inferriate).
//! - Fighters Hall / Maschio dei Guerrieri (torrette, feritoie e portale monumentale).
//! - Thieves Guild & Hall generica (architettura fortificata con portale ad arco).
//!
//! ## Finestre arricchite (fix)
//!
//! Prima di questa fix, `generate_fighters_hall` e `generate_generic_hall` disegnavano
//! le finestre come semplici box piatti (un quadrato nero per le feritoie, un quadrato
//! blu per il vetro). Il risultato era "rozzo": nessuna cornice, nessuna inferriata,
//! nessun davanzale.
//!
//! Ora entrambe usano `push_rich_window`, che disegna:
//! - cornice in pietra (in `wall`, quindi eredita la stessa texture del muro);
//! - architrave sporgente in cima e davanzale in fondo;
//! - vetro blu (in `windows`);
//! - due inferriate incrociate (in `trim`).
//!
//! Ispirata alle finestre del Palazzo Reale (`palace_gen.rs`), che hanno cornice
//! in pietra, gradiente nel vetro e inferriate incrociate.
//!
//! ## Cornicione in pietra (fix)
//!
//! La sporgenza a metà del secondo piano era un `push_box(trim, ...)` con colore
//! fisso marrone scuro (`trim_c`). L'utente la vedeva come "senza texture": era
//! un blocco di colore uniforme, privo della texture del muro. Ora è in `wall`
//! (`stone`), quindi eredita la stessa texture del muro circostante (pietra,
//! mattoni, legno, a seconda dell'edificio).
//!
//! ## Torre dei Maghi ottagonale a 7 piani con finestre del Palazzo Reale
//!
//! La torre era un cilindro quasi perfetto (`sides = 20`) a 5 piani, con finestre
//! a quadrato blu piatto. Ora è un **ottagono** (`sides = 8`), quindi con **lati
//! piatti** come nelle torri originali di Daggerfall, a **7 piani**, e ogni faccia
//! **pari** di ogni piano ha una finestra in stile Palazzo Reale disegnata da
//! `push_palace_window` a **mezza scala** (`scale = 0.5`). Le facce **dispari**
//! restano senza finestre (4 finestre per piano, alternate). Il portale d'ingresso
//! è sul lato con centro a 270° in direzione `-Z` (`door_side_idx = 5`, dispari),
//! dove arriva il giocatore, quindi il portale non ha mai una finestra sopra.
use super::arch3d::BuildingTextures;
use super::building_gen::{BuildingMesh, Footprint, MeshPart, Side};
use std::f32::consts::{PI, TAU};

type V3 = [f32; 3];

fn normal(a: V3, b: V3, c: V3) -> Option<V3> {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l < 1e-6 {
        None
    } else {
        Some([n[0] / l, n[1] / l, n[2] / l])
    }
}

fn push_face(part: &mut MeshPart, pts: &[V3], color: [f32; 4], s: f32, inside: V3) {
    let raw = match normal(pts[0], pts[1], pts[2]) {
        Some(n) => n,
        None => return,
    };
    let c = [
        pts.iter().map(|p| p[0]).sum::<f32>() / pts.len() as f32,
        pts.iter().map(|p| p[1]).sum::<f32>() / pts.len() as f32,
        pts.iter().map(|p| p[2]).sum::<f32>() / pts.len() as f32,
    ];
    let to_c = [c[0] - inside[0], c[1] - inside[1], c[2] - inside[2]];
    let flip = raw[0] * to_c[0] + raw[1] * to_c[1] + raw[2] * to_c[2] < 0.0;
    let n = if flip { [-raw[0], -raw[1], -raw[2]] } else { raw };
    let base = part.positions.len() as u32;
    let order: Vec<usize> = if flip {
        (0..pts.len()).rev().collect()
    } else {
        (0..pts.len()).collect()
    };
    for &i in &order {
        let p = pts[i];
        let (ax, ay, az) = (n[0].abs(), n[1].abs(), n[2].abs());
        let (u, v) = if ay >= ax && ay >= az {
            (p[0], p[2])
        } else if ax >= az {
            (p[2], p[1])
        } else {
            (p[0], p[1])
        };
        part.positions.push(p);
        part.normals.push(n);
        part.uvs.push([u / s, -v / s]);
        part.colors.push(color);
    }
    for k in 1..(pts.len() as u32 - 1) {
        part.indices.extend_from_slice(&[base, base + k, base + k + 1]);
    }
}

fn push_box(part: &mut MeshPart, a: V3, b: V3, col: [f32; 4], s: f32) {
    let p = [
        [a[0], a[1], a[2]],
        [b[0], a[1], a[2]],
        [a[0], b[1], a[2]],
        [b[0], b[1], a[2]],
        [a[0], a[1], b[2]],
        [b[0], a[1], b[2]],
        [a[0], b[1], b[2]],
        [b[0], b[1], b[2]],
    ];
    let ins = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5];
    for f in [
        [0, 2, 3, 1],
        [5, 7, 6, 4],
        [4, 6, 2, 0],
        [1, 3, 7, 5],
        [2, 6, 7, 3],
        [4, 0, 1, 5],
    ] {
        push_face(part, &[p[f[0]], p[f[1]], p[f[2]], p[f[3]]], col, s, ins);
    }
}

/// Come `push_box`, ma prende gli 8 corner già calcolati (per box orientati
/// arbitrariamente). L'ordine dei corner deve essere lo stesso di `push_box`:
/// `corner[i]` con bit 0 = max u, bit 1 = max v, bit 2 = max n.
fn push_box_from_corners(part: &mut MeshPart, corners: [V3; 8], color: [f32; 4], s: f32) {
    let mut ins = [0.0f32; 3];
    for corner in &corners {
        for k in 0..3 {
            ins[k] += corner[k] / 8.0;
        }
    }
    let faces: [[usize; 4]; 6] = [
        [0, 2, 3, 1],
        [5, 7, 6, 4],
        [4, 6, 2, 0],
        [1, 3, 7, 5],
        [2, 6, 7, 3],
        [4, 0, 1, 5],
    ];
    for f in faces {
        push_face(
            part,
            &[corners[f[0]], corners[f[1]], corners[f[2]], corners[f[3]]],
            color,
            s,
            ins,
        );
    }
}

/// Normale di un muro: quale asse è "fuori".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WallAxis {
    /// Muro frontale, la cui normale punta verso -Z (il giocatore vede il muro venendogli incontro).
    FrontNegZ,
    /// Muro posteriore, normale +Z.
    BackPosZ,
    /// Muro laterale sinistro, normale +X.
    LeftNegX,
    /// Muro laterale destro, normale -X.
    RightPosX,
}

/// Sistema di coordinate locali a un muro: `u` (orizzontale lungo il muro),
/// `v` (verticale, sempre su), `n` (normale verso l'esterno del muro).
struct AxisFrame {
    u: V3,
    v: V3,
    n: V3,
}

impl AxisFrame {
    fn from_wall_axis(axis: WallAxis) -> Self {
        let (u, n) = match axis {
            WallAxis::FrontNegZ => ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
            WallAxis::BackPosZ => ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            WallAxis::LeftNegX => ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            WallAxis::RightPosX => ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0]),
        };
        Self { u, v: [0.0, 1.0, 0.0], n }
    }

    fn local_to_world(&self, center: V3, u: f32, v: f32, n: f32) -> V3 {
        [
            center[0] + self.u[0] * u + self.v[0] * v + self.n[0] * n,
            center[1] + self.u[1] * u + self.v[1] * v + self.n[1] * n,
            center[2] + self.u[2] * u + self.v[2] * v + self.n[2] * n,
        ]
    }

    /// Box allineato al muro: (u_min..u_max, v_min..v_max, n_min..n_max)
    /// rispetto al `center`.
    fn push_box_local(
        &self,
        part: &mut MeshPart,
        center: V3,
        u: (f32, f32),
                      v: (f32, f32),
                      n: (f32, f32),
                      color: [f32; 4],
                      s: f32,
    ) {
        let mut corners = [[0.0f32; 3]; 8];
        for (i, c) in corners.iter_mut().enumerate() {
            let uu = if i & 1 == 0 { u.0 } else { u.1 };
            let vv = if i & 2 == 0 { v.0 } else { v.1 };
            let nn = if i & 4 == 0 { n.0 } else { n.1 };
            *c = self.local_to_world(center, uu, vv, nn);
        }
        push_box_from_corners(part, corners, color, s);
    }
}

/// Disegna una finestra "ricca":
/// - cornice in pietra in `wall` (eredita la texture del muro);
/// - vetro colorato in `windows`;
/// - due inferriate incrociate in `trim`.
///
/// Ispirata alle finestre del Palazzo Reale: cornice rettangolare, architrave
/// e davanzale sporgenti, vetro con colore fisso, inferriate quasi nere.
///
/// # Parametri
/// * `center` — punto di ancoraggio sulla superficie esterna del muro
/// * `axis` — normale del muro (quale direzione è "fuori")
/// * `width`, `height` — dimensioni del VETRO (la cornice sporge oltre)
/// * `stone`, `glass`, `iron` — colori dei tre materiali
pub fn push_rich_window(
    wall: &mut MeshPart,
    windows: &mut MeshPart,
    trim: &mut MeshPart,
    center: V3,
    axis: WallAxis,
    width: f32,
    height: f32,
    stone: [f32; 4],
    glass: [f32; 4],
    iron: [f32; 4],
) {
    let frame = AxisFrame::from_wall_axis(axis);
    let hw = width * 0.5;
    let hh = height * 0.5;

    let frame_w = 0.22;
    let frame_depth = 0.10;
    let glass_n = 0.02;
    let glass_depth = 0.02;
    let wall_scale = 4.0;
    let glass_scale = 1.0;
    let iron_scale = 1.0;

    frame.push_box_local(
        windows,
        center,
        (-hw, hw),
                         (-hh, hh),
                         (glass_n, glass_n + glass_depth),
                         glass,
                         glass_scale,
    );

    frame.push_box_local(
        wall,
        center,
        (-hw - frame_w, -hw),
                         (-hh - frame_w, hh + frame_w),
                         (0.0, frame_depth),
                         stone,
                         wall_scale,
    );
    frame.push_box_local(
        wall,
        center,
        (hw, hw + frame_w),
                         (-hh - frame_w, hh + frame_w),
                         (0.0, frame_depth),
                         stone,
                         wall_scale,
    );
    frame.push_box_local(
        wall,
        center,
        (-hw, hw),
                         (-hh - frame_w, -hh),
                         (0.0, frame_depth),
                         stone,
                         wall_scale,
    );
    frame.push_box_local(
        wall,
        center,
        (-hw, hw),
                         (hh, hh + frame_w),
                         (0.0, frame_depth),
                         stone,
                         wall_scale,
    );

    frame.push_box_local(
        wall,
        center,
        (-hw - frame_w - 0.04, hw + frame_w + 0.04),
                         (hh + frame_w, hh + frame_w + 0.08),
                         (0.0, frame_depth + 0.04),
                         stone,
                         wall_scale,
    );

    frame.push_box_local(
        wall,
        center,
        (-hw - frame_w - 0.04, hw + frame_w + 0.04),
                         (-hh - frame_w - 0.08, -hh - frame_w),
                         (0.0, frame_depth + 0.04),
                         stone,
                         wall_scale,
    );

    let bar_n0 = glass_n + glass_depth;
    let bar_n1 = bar_n0 + 0.02;
    let bar_w = 0.04;

    frame.push_box_local(
        trim,
        center,
        (-bar_w * 0.5, bar_w * 0.5),
                         (-hh, hh),
                         (bar_n0, bar_n1),
                         iron,
                         iron_scale,
    );
    frame.push_box_local(
        trim,
        center,
        (-hw, hw),
                         (-bar_w * 0.5, bar_w * 0.5),
                         (bar_n0, bar_n1),
                         iron,
                         iron_scale,
    );
}

fn push_pyr(
    part: &mut MeshPart,
    xz0: [f32; 2],
    xz1: [f32; 2],
    y0: f32,
    apex: V3,
    col: [f32; 4],
    s: f32,
) {
    let ins = [(xz0[0] + xz1[0]) * 0.5, y0 - 1.0, (xz0[1] + xz1[1]) * 0.5];
    let b = [
        [xz0[0], y0, xz0[1]],
        [xz1[0], y0, xz0[1]],
        [xz1[0], y0, xz1[1]],
        [xz0[0], y0, xz1[1]],
    ];
    push_face(part, &[b[0], b[1], apex], col, s, ins);
    push_face(part, &[b[1], b[2], apex], col, s, ins);
    push_face(part, &[b[2], b[3], apex], col, s, ins);
    push_face(part, &[b[3], b[0], apex], col, s, ins);
}

fn push_cylinder(
    part: &mut MeshPart,
    cx: f32,
    cz: f32,
    r: f32,
    y0: f32,
    y1: f32,
    sides: usize,
    color: [f32; 4],
    s: f32,
    angle_offset: f32,
    skip_side: Option<usize>,
) {
    let inside = [cx, (y0 + y1) * 0.5, cz];
    for i in 0..sides {
        if skip_side == Some(i) {
            continue;
        }
        let a0 = angle_offset + (i as f32 / sides as f32) * TAU;
        let a1 = angle_offset + ((i + 1) as f32 / sides as f32) * TAU;
        let p0 = [cx + a0.cos() * r, y0, cz + a0.sin() * r];
        let p1 = [cx + a1.cos() * r, y0, cz + a1.sin() * r];
        let p2 = [cx + a1.cos() * r, y1, cz + a1.sin() * r];
        let p3 = [cx + a0.cos() * r, y1, cz + a0.sin() * r];
        push_face(part, &[p0, p1, p2, p3], color, s, inside);
    }
}

fn push_cone(
    part: &mut MeshPart,
    cx: f32,
    cz: f32,
    r: f32,
    y_base: f32,
    apex_y: f32,
    sides: usize,
    color: [f32; 4],
    s: f32,
    angle_offset: f32,
) {
    let apex = [cx, apex_y, cz];
    let inside = [cx, y_base, cz];
    for i in 0..sides {
        let a0 = angle_offset + (i as f32 / sides as f32) * TAU;
        let a1 = angle_offset + ((i + 1) as f32 / sides as f32) * TAU;
        let p0 = [cx + a0.cos() * r, y_base, cz + a0.sin() * r];
        let p1 = [cx + a1.cos() * r, y_base, cz + a1.sin() * r];
        push_face(part, &[p0, p1, apex], color, s, inside);
    }
}

fn push_ring(
    part: &mut MeshPart,
    cx: f32,
    cz: f32,
    r_inner: f32,
    r_outer: f32,
    y0: f32,
    y1: f32,
    sides: usize,
    color: [f32; 4],
    s: f32,
    angle_offset: f32,
    skip_side: Option<usize>,
) {
    let inside_mid = [cx, (y0 + y1) * 0.5, cz];
    for i in 0..sides {
        if skip_side == Some(i) {
            continue;
        }
        let a0 = angle_offset + (i as f32 / sides as f32) * TAU;
        let a1 = angle_offset + ((i + 1) as f32 / sides as f32) * TAU;

        let p0 = [cx + a0.cos() * r_outer, y0, cz + a0.sin() * r_outer];
        let p1 = [cx + a1.cos() * r_outer, y0, cz + a1.sin() * r_outer];
        let p2 = [cx + a1.cos() * r_outer, y1, cz + a1.sin() * r_outer];
        let p3 = [cx + a0.cos() * r_outer, y1, cz + a0.sin() * r_outer];
        push_face(part, &[p0, p1, p2, p3], color, s, inside_mid);

        let q0 = [cx + a0.cos() * r_inner, y0, cz + a0.sin() * r_inner];
        let q1 = [cx + a1.cos() * r_inner, y0, cz + a1.sin() * r_inner];
        let q2 = [cx + a1.cos() * r_inner, y1, cz + a1.sin() * r_inner];
        let q3 = [cx + a0.cos() * r_inner, y1, cz + a0.sin() * r_inner];
        push_face(part, &[q0, q1, q2, q3], color, s, inside_mid);

        push_face(
            part,
            &[
                [cx + a0.cos() * r_outer, y1, cz + a0.sin() * r_outer],
                  [cx + a1.cos() * r_outer, y1, cz + a1.sin() * r_outer],
                  [cx + a1.cos() * r_inner, y1, cz + a1.sin() * r_inner],
                  [cx + a0.cos() * r_inner, y1, cz + a0.sin() * r_inner],
            ],
            color,
            s,
            [cx, y1 - 0.5, cz],
        );

        push_face(
            part,
            &[
                [cx + a0.cos() * r_outer, y0, cz + a0.sin() * r_outer],
                  [cx + a1.cos() * r_outer, y0, cz + a1.sin() * r_outer],
                  [cx + a1.cos() * r_inner, y0, cz + a1.sin() * r_inner],
                  [cx + a0.cos() * r_inner, y0, cz + a0.sin() * r_inner],
            ],
            color,
            s,
            [cx, y0 + 0.5, cz],
        );
    }
}

fn push_window_quad(
    part: &mut MeshPart,
    cx: f32,
    cz: f32,
    half_w: f32,
    half_h: f32,
    angle: f32,
    y_center: f32,
    color: [f32; 4],
    s: f32,
) {
    let nx = angle.cos();
    let nz = angle.sin();
    let ox = nx * 0.02;
    let oz = nz * 0.02;
    let tx = -nz;
    let tz = nx;
    let gx = cx + ox;
    let gz = cz + oz;
    let p0 = [gx - tx * half_w, y_center - half_h, gz - tz * half_w];
    let p1 = [gx + tx * half_w, y_center - half_h, gz + tz * half_w];
    let p2 = [gx + tx * half_w, y_center + half_h, gz + tz * half_w];
    let p3 = [gx - tx * half_w, y_center + half_h, gz - tz * half_w];
    push_face(part, &[p0, p1, p2, p3], color, s, [cx - nx, y_center, cz - nz]);
}

fn merlon_row(part: &mut MeshPart, along_x: bool, edge: f32, half_len: f32, y0: f32, color: [f32; 4]) {
    const MERLON_H: f32 = 1.0;
    let len = half_len * 2.0;
    let n = ((len / 2.0).floor() as usize).clamp(1, 30);
    let pitch = len / n as f32;
    let ml = pitch * 0.55;
    let t = 0.5;
    let (b0, b1) = if edge >= 0.0 { (edge - t, edge) } else { (edge, edge + t) };
    for i in 0..n {
        let a = -half_len + (i as f32 + 0.5) * pitch;
        let (a0, a1) = (a - ml * 0.5, a + ml * 0.5);
        let (min, max) = if along_x {
            ([a0, y0, b0], [a1, y0 + MERLON_H, b1])
        } else {
            ([b0, y0, a0], [b1, y0 + MERLON_H, a1])
        };
        push_box(part, min, max, color, 2.0);
    }
}

// =========================================================================
// PORTALE AD ARCO IN PIETRA (USATO DA TEMPLI E GILDE)
// =========================================================================

pub fn build_arched_guild_portal(
    wall: &mut MeshPart,
    doors: &mut MeshPart,
    trim: &mut MeshPart,
    stone: [f32; 4],
    center_x: f32,
    wall_front_z: f32,
    y_floor: f32,
) {
    let r_arch = 0.75f32;
    let r_ext = 1.05f32;
    let y_door_base = y_floor;
    let y_spring = y_door_base + 2.05f32;
    let y_arch_top = y_spring + r_arch;
    let y_arch_ext = y_spring + r_ext;
    let jamb_w = 0.35f32;
    let total_hw = r_arch + jamb_w;

    let z_front = wall_front_z - 0.28f32;
    let z_door = wall_front_z - 0.06f32;
    let z_back = wall_front_z + 0.18f32;

    let iron_c = [0.03, 0.03, 0.035, 1.0];
    let stud_c = [0.08, 0.08, 0.09, 1.0];
    let bldg_interior = [center_x, y_spring, wall_front_z + 2.0f32];

    push_box(
        wall,
        [center_x - total_hw - 0.06, y_door_base - 0.25, z_front - 0.08],
        [center_x + total_hw + 0.06, y_door_base + 0.04, z_back],
        stone,
        2.0,
    );

    push_box(
        wall,
        [center_x - total_hw, y_door_base, z_front],
        [center_x - r_arch, y_spring, z_back],
        stone,
        2.0,
    );
    push_box(
        wall,
        [center_x + r_arch, y_door_base, z_front],
        [center_x + total_hw, y_spring, z_back],
        stone,
        2.0,
    );

    push_box(
        wall,
        [center_x - total_hw - 0.03, y_spring - 0.08, z_front - 0.04],
        [center_x - r_arch + 0.03, y_spring + 0.04, z_back],
        stone,
        1.0,
    );
    push_box(
        wall,
        [center_x + r_arch - 0.03, y_spring - 0.08, z_front - 0.04],
        [center_x + total_hw + 0.03, y_spring + 0.04, z_back],
        stone,
        1.0,
    );

    let y_ent_base = y_arch_ext + 0.08f32;
    push_box(
        wall,
        [center_x - total_hw, y_spring, z_front],
        [center_x - r_ext, y_ent_base, z_back],
        stone,
        2.0,
    );
    push_box(
        wall,
        [center_x + r_ext, y_spring, z_front],
        [center_x + total_hw, y_ent_base, z_back],
        stone,
        2.0,
    );

    let vault_segs = 16;
    let z_arch_front = z_front - 0.04f32;
    for k in 0..vault_segs {
        let t0 = (k as f32 / vault_segs as f32) * PI;
        let t1 = ((k + 1) as f32 / vault_segs as f32) * PI;

        let xi0 = center_x - r_arch * t0.cos();
        let yi0 = y_spring + r_arch * t0.sin();
        let xi1 = center_x - r_arch * t1.cos();
        let yi1 = y_spring + r_arch * t1.sin();

        let xe0 = center_x - r_ext * t0.cos();
        let ye0 = y_spring + r_ext * t0.sin();
        let xe1 = center_x - r_ext * t1.cos();
        let ye1 = y_spring + r_ext * t1.sin();

        push_face(
            wall,
            &[
                [xi0, yi0, z_arch_front],
                [xi1, yi1, z_arch_front],
                [xi1, yi1, z_door],
                [xi0, yi0, z_door],
            ],
            stone,
            1.5,
            [center_x, y_spring + r_arch + 1.0, (z_front + z_door) * 0.5],
        );

        push_face(
            wall,
            &[
                [xi0, yi0, z_arch_front],
                [xi1, yi1, z_arch_front],
                [xe1, ye1, z_arch_front],
                [xe0, ye0, z_arch_front],
            ],
            stone,
            1.5,
            bldg_interior,
        );

        push_face(
            wall,
            &[
                [xe0, ye0, z_front],
                [xe1, ye1, z_front],
                [xe1, ye1, z_arch_front],
                [xe0, ye0, z_arch_front],
            ],
            stone,
            1.5,
            [center_x, y_spring, z_front],
        );

        push_face(
            wall,
            &[
                [xe0, ye0, z_front],
                [xe1, ye1, z_front],
                [xe1, y_ent_base, z_front],
                [xe0, y_ent_base, z_front],
            ],
            stone,
            1.5,
            bldg_interior,
        );
    }

    let kw_bot = 0.11f32;
    let kw_top = 0.17f32;
    let y_ks_bot = y_arch_top - 0.05f32;
    let y_ks_top = y_arch_ext + 0.12f32;
    let z_ks_front = z_arch_front - 0.05f32;
    push_face(wall, &[[center_x - kw_bot, y_ks_bot, z_ks_front], [center_x + kw_bot, y_ks_bot, z_ks_front], [center_x + kw_top, y_ks_top, z_ks_front], [center_x - kw_top, y_ks_top, z_ks_front]], stone, 1.0, bldg_interior);
    push_face(wall, &[[center_x - kw_bot, y_ks_bot, z_arch_front], [center_x - kw_bot, y_ks_bot, z_ks_front], [center_x - kw_top, y_ks_top, z_ks_front], [center_x - kw_top, y_ks_top, z_arch_front]], stone, 1.0, bldg_interior);
    push_face(wall, &[[center_x + kw_bot, y_ks_bot, z_ks_front], [center_x + kw_bot, y_ks_bot, z_arch_front], [center_x + kw_top, y_ks_top, z_arch_front], [center_x + kw_top, y_ks_top, z_ks_front]], stone, 1.0, bldg_interior);

    let y_ent_top = y_ent_base + 0.20f32;
    let y_cornice_top = y_ent_top + 0.10f32;

    push_box(wall, [center_x - total_hw - 0.03, y_ent_base, z_front - 0.02], [center_x + total_hw + 0.03, y_ent_top, z_back], stone, 2.0);
    push_box(wall, [center_x - total_hw - 0.08, y_ent_top, z_front - 0.08], [center_x + total_hw + 0.08, y_cornice_top, z_back], stone, 1.5);

    let y_timp_base = y_cornice_top;
    let y_timp_top = y_timp_base + 0.45f32;
    let hw_timp = total_hw + 0.08f32;
    let z_timp_front = z_front - 0.08f32;

    let p_left = [center_x - hw_timp, y_timp_base, z_timp_front];
    let p_right = [center_x + hw_timp, y_timp_base, z_timp_front];
    let p_apex = [center_x, y_timp_top, z_timp_front];
    let p_left_b = [center_x - hw_timp, y_timp_base, z_back];
    let p_right_b = [center_x + hw_timp, y_timp_base, z_back];
    let p_apex_b = [center_x, y_timp_top, z_back];

    push_face(wall, &[p_left, p_right, p_apex], stone, 1.5, bldg_interior);
    push_face(wall, &[p_left, p_apex, p_apex_b, p_left_b], stone, 1.5, [center_x, y_timp_base - 0.5, wall_front_z]);
    push_face(wall, &[p_apex, p_right, p_right_b, p_apex_b], stone, 1.5, [center_x, y_timp_base - 0.5, wall_front_z]);

    let n_planks = 4;
    let plank_w = (2.0 * r_arch) / n_planks as f32;
    for i in 0..n_planks {
        let x0 = -r_arch + i as f32 * plank_w;
        let x1 = x0 + plank_w;
        let n_slices = 3;
        for s in 0..n_slices {
            let ua = x0 + (x1 - x0) * (s as f32 / n_slices as f32);
            let ub = x0 + (x1 - x0) * ((s + 1) as f32 / n_slices as f32);
            let ya = y_spring + (r_arch * r_arch - (ua * ua).min(r_arch * r_arch - 1e-4)).sqrt();
            let yb = y_spring + (r_arch * r_arch - (ub * ub).min(r_arch * r_arch - 1e-4)).sqrt();

            let u0 = ((ua + r_arch) / (2.0 * r_arch)).clamp(0.0, 1.0);
            let u1 = ((ub + r_arch) / (2.0 * r_arch)).clamp(0.0, 1.0);
            let v_top_a = ((ya - y_door_base) / (y_arch_top - y_door_base)).clamp(0.0, 1.0);
            let v_top_b = ((yb - y_door_base) / (y_arch_top - y_door_base)).clamp(0.0, 1.0);

            let v_bl = [center_x + ua, y_door_base, z_door];
            let v_br = [center_x + ub, y_door_base, z_door];
            let v_tr = [center_x + ub, yb, z_door];
            let v_tl = [center_x + ua, ya, z_door];

            let b_base = doors.positions.len() as u32;
            doors.positions.push(v_bl); doors.normals.push([0.0, 0.0, -1.0]); doors.uvs.push([u0, 1.0]); doors.colors.push([1.0, 1.0, 1.0, 1.0]);
            doors.positions.push(v_tl); doors.normals.push([0.0, 0.0, -1.0]); doors.uvs.push([u0, 1.0 - v_top_a]); doors.colors.push([1.0, 1.0, 1.0, 1.0]);
            doors.positions.push(v_tr); doors.normals.push([0.0, 0.0, -1.0]); doors.uvs.push([u1, 1.0 - v_top_b]); doors.colors.push([1.0, 1.0, 1.0, 1.0]);
            doors.positions.push(v_br); doors.normals.push([0.0, 0.0, -1.0]); doors.uvs.push([u1, 1.0]); doors.colors.push([1.0, 1.0, 1.0, 1.0]);
            doors.indices.extend_from_slice(&[b_base, b_base + 1, b_base + 2, b_base, b_base + 2, b_base + 3]);

            let b_back = doors.positions.len() as u32;
            doors.positions.push(v_bl); doors.normals.push([0.0, 0.0, 1.0]); doors.uvs.push([u0, 1.0]); doors.colors.push([1.0, 1.0, 1.0, 1.0]);
            doors.positions.push(v_br); doors.normals.push([0.0, 0.0, 1.0]); doors.uvs.push([u1, 1.0]); doors.colors.push([1.0, 1.0, 1.0, 1.0]);
            doors.positions.push(v_tr); doors.normals.push([0.0, 0.0, 1.0]); doors.uvs.push([u1, 1.0 - v_top_b]); doors.colors.push([1.0, 1.0, 1.0, 1.0]);
            doors.positions.push(v_tl); doors.normals.push([0.0, 0.0, 1.0]); doors.uvs.push([u0, 1.0 - v_top_a]); doors.colors.push([1.0, 1.0, 1.0, 1.0]);
            doors.indices.extend_from_slice(&[b_back, b_back + 1, b_back + 2, b_back, b_back + 2, b_back + 3]);
        }
    }

    for &ys in &[y_door_base + 0.65f32, y_door_base + 1.80f32] {
        push_box(trim, [center_x - r_arch + 0.04, ys - 0.04, z_door - 0.015], [center_x + r_arch - 0.04, ys + 0.04, z_door], iron_c, 1.0);
        for &bx in &[-0.50f32, -0.20, 0.20, 0.50] {
            push_box(trim, [center_x + bx - 0.02, ys - 0.02, z_door - 0.025], [center_x + bx + 0.02, ys + 0.02, z_door - 0.015], stud_c, 1.0);
        }
    }
    push_box(trim, [center_x - 0.02, y_door_base, z_door - 0.01], [center_x + 0.02, y_arch_top - 0.05, z_door], iron_c, 1.0);
    push_box(trim, [center_x - 0.18, y_door_base + 1.15, z_door - 0.035], [center_x - 0.12, y_door_base + 1.25, z_door - 0.01], iron_c, 1.0);
}

// =========================================================================
// TEMPIO PAGANO CLASSICO DEGLI OTTO DIVINI
// =========================================================================

pub fn generate_pagan_temple(fp: &Footprint, btex: BuildingTextures) -> BuildingMesh {
    let mut mesh = BuildingMesh::default();
    mesh.wall_texture = btex.wall.or(Some((335, 0)));
    mesh.roof_texture = btex.roof.or(Some((339, 0)));
    mesh.door_texture = btex.door.or(Some((74, 0)));
    mesh.window_texture = None;

    let stone = [1.0, 1.0, 1.0, 1.0];
    let marble_trim = [0.90, 0.88, 0.82, 1.0];
    let roof_c = [1.0, 1.0, 1.0, 1.0];

    let BuildingMesh { parts: [wall, roof, trim, _], doors, .. } = &mut mesh;

    let hw = (fp.width * 0.5).max(10.0);
    let hd = (fp.depth * 0.5).max(14.0);
    let total_h = fp.height.max(14.0);

    let y_base = -0.6f32;
    let y_step1 = -0.2f32;
    let y_step2 = 0.2f32;
    let y_floor = 0.6f32;

    push_box(wall, [-hw - 1.2, y_base, -hd - 1.2], [hw + 1.2, y_step1, hd + 1.2], stone, 3.0);
    push_box(wall, [-hw - 0.7, y_step1, -hd - 0.7], [hw + 0.7, y_step2, hd + 0.7], stone, 3.0);
    push_box(wall, [-hw - 0.2, y_step2, -hd - 0.2], [hw + 0.2, y_floor, hd + 0.2], stone, 3.0);

    let cella_hw = hw * 0.70;
    let cella_front = -hd + 5.5;
    let cella_back = hd - 1.2;
    let y_architrave = total_h * 0.65;

    push_box(wall, [-cella_hw, y_floor, cella_front], [cella_hw, y_architrave, cella_back], stone, 4.0);

    let col_h = y_architrave - y_floor;
    let col_r = 0.52f32;
    let col_sides = 16;

    let n_cols_front = 6;
    for i in 0..n_cols_front {
        let t = i as f32 / (n_cols_front - 1) as f32;
        let cx = (-hw + 1.4) * (1.0 - t) + (hw - 1.4) * t;
        let cz = -hd + 1.2;

        push_box(trim, [cx - col_r * 1.35, y_floor, cz - col_r * 1.35], [cx + col_r * 1.35, y_floor + 0.35, cz + col_r * 1.35], marble_trim, 1.0);
        push_cylinder(wall, cx, cz, col_r, y_floor + 0.35, y_floor + col_h - 0.45, col_sides, stone, 2.0, 0.0, None);
        push_box(trim, [cx - col_r * 1.45, y_floor + col_h - 0.45, cz - col_r * 1.45], [cx + col_r * 1.45, y_floor + col_h, cz + col_r * 1.45], marble_trim, 1.0);
    }

    for &cz in &[-hd + 2.8, -hd + 4.4] {
        for &cx in &[-hw + 1.4, hw - 1.4] {
            push_box(trim, [cx - col_r * 1.35, y_floor, cz - col_r * 1.35], [cx + col_r * 1.35, y_floor + 0.35, cz + col_r * 1.35], marble_trim, 1.0);
            push_cylinder(wall, cx, cz, col_r, y_floor + 0.35, y_floor + col_h - 0.45, col_sides, stone, 2.0, 0.0, None);
            push_box(trim, [cx - col_r * 1.45, y_floor + col_h - 0.45, cz - col_r * 1.45], [cx + col_r * 1.45, y_floor + col_h, cz + col_r * 1.45], marble_trim, 1.0);
        }
    }

    let y_frieze_top = y_architrave + 1.2f32;
    push_box(trim, [-hw - 0.4, y_architrave, -hd - 0.4], [hw + 0.4, y_frieze_top, hd + 0.4], marble_trim, 2.5);

    let y_cornice = y_frieze_top + 0.35f32;
    push_box(trim, [-hw - 0.8, y_frieze_top, -hd - 0.8], [hw + 0.8, y_cornice, hd + 0.8], marble_trim, 2.0);

    let y_ridge = y_cornice + (hw * 0.42).clamp(3.0, 5.5);
    let z_front_edge = -hd - 0.8;
    let z_back_edge = hd + 0.8;
    let ins_temple = [0.0, y_cornice, 0.0];

    push_face(wall, &[
        [-hw - 0.8, y_cornice, z_front_edge],
        [hw + 0.8, y_cornice, z_front_edge],
        [0.0, y_ridge, z_front_edge],
    ], stone, 3.0, [0.0, y_cornice, z_front_edge + 2.0]);

    push_face(wall, &[
        [hw + 0.8, y_cornice, z_back_edge],
        [-hw - 0.8, y_cornice, z_back_edge],
        [0.0, y_ridge, z_back_edge],
    ], stone, 3.0, [0.0, y_cornice, z_back_edge - 2.0]);

    push_face(roof, &[
        [-hw - 1.0, y_cornice, -hd - 1.0],
        [-hw - 1.0, y_cornice, hd + 1.0],
        [0.0, y_ridge, hd + 1.0],
        [0.0, y_ridge, -hd - 1.0],
    ], roof_c, 3.5, ins_temple);

    push_face(roof, &[
        [hw + 1.0, y_cornice, hd + 1.0],
        [hw + 1.0, y_cornice, -hd - 1.0],
        [0.0, y_ridge, -hd - 1.0],
        [0.0, y_ridge, hd + 1.0],
    ], roof_c, 3.5, ins_temple);

    build_arched_guild_portal(wall, doors, trim, stone, 0.0, cella_front, y_floor);

    mesh
}

// =========================================================================
// FINESTRA IN STILE PALAZZO REALE (con parametro di scala)
// =========================================================================

/// Disegna una finestra **in stile Palazzo Reale** (`palace_gen.rs`) su una
/// faccia piana con normale `normal` (orizzontale, unitaria).
///
/// `base_center` è il punto sul muro al **centro della base** della finestra
/// (alla quota `fy`, sotto lo stipite), non al centro geometrico. La finestra
/// si estende verso l'esterno lungo `normal`.
///
/// `scale` scala **tutte** le dimensioni: con `scale = 1.0` la finestra è
/// identica a quella del palazzo (davanzale, stipiti, arco a tutto sesto,
/// chiave di volta, vetro con gradiente diagonale, inferriate a croce e
/// bottone centrale). Con `scale = 0.5` è esattamente metà in ogni dimensione.
///
/// `win_r` e `win_h_rect` sono le dimensioni del **vetro** (raggio dell'arco
/// e altezza del rettangolo). Devono essere già scalati coerentemente con
/// `scale`: in genere `win_r = 0.92 * scale`, `win_h_rect = 2.05 * scale`.
fn push_palace_window(
    wall: &mut MeshPart,
    windows: &mut MeshPart,
    trim: &mut MeshPart,
    base_center: V3,
    normal: V3,
    win_r: f32,
    win_h_rect: f32,
    scale: f32,
    stone: [f32; 4],
    iron_bar_c: [f32; 4],
) {
    let sc = scale.max(1e-3);

    // Sistema locale (t, up, n) destrorso: `t × up = n`.
    let n = [normal[0], 0.0, normal[2]];
    let t = [n[2], 0.0, -n[0]];
    let up = [0.0f32, 1.0, 0.0];

    let pw = |u: f32, v: f32, off: f32| -> V3 {
        [
            base_center[0] + t[0] * u + n[0] * off,
            base_center[1] + up[1] * v + n[1] * off,
            base_center[2] + t[2] * u + n[2] * off,
        ]
    };

    // Punto dentro il muro: `push_face` lo usa per l'orientamento delle facce.
    let inside = [
        base_center[0] - n[0] * 1.0,
        base_center[1] + win_h_rect * 0.5,
        base_center[2] - n[2] * 1.0,
    ];

    // Box orientato nel sistema (t, up, n).
    let mut obox = |part: &mut MeshPart,
    u0: f32, u1: f32,
    v0: f32, v1: f32,
    n0: f32, n1: f32,
    col: [f32; 4], s: f32| {
        let mut corners = [[0.0f32; 3]; 8];
        for (i, c) in corners.iter_mut().enumerate() {
            let uu = if i & 1 == 0 { u0 } else { u1 };
            let vv = if i & 2 == 0 { v0 } else { v1 };
            let nn = if i & 4 == 0 { n0 } else { n1 };
            *c = pw(uu, vv, nn);
        }
        push_box_from_corners(part, corners, col, s);
    };

    let wy_spring = win_h_rect;
    let wy_top = wy_spring + win_r;

    // 1. Davanzale
    obox(wall, -1.25 * sc, 1.25 * sc, -0.25 * sc, 0.0, -0.04 * sc, 0.16 * sc, stone, 2.0);

    // 2. Stipiti
    obox(wall, -1.20 * sc, -win_r, 0.0, wy_spring, 0.0, 0.16 * sc, stone, 1.5);
    obox(wall, win_r, 1.20 * sc, 0.0, wy_spring, 0.0, 0.16 * sc, stone, 1.5);

    // 3. Arco a tutto sesto in pietra (12 segmenti)
    let r_out_win = win_r + 0.25 * sc;
    let n_arch = 12;
    for k in 0..n_arch {
        let a0 = (k as f32 / n_arch as f32) * PI;
        let a1 = ((k + 1) as f32 / n_arch as f32) * PI;

        let u0_in = -win_r * a0.cos();
        let v0_in = wy_spring + win_r * a0.sin();
        let u1_in = -win_r * a1.cos();
        let v1_in = wy_spring + win_r * a1.sin();

        let u0_out = -r_out_win * a0.cos();
        let v0_out = wy_spring + r_out_win * a0.sin();
        let u1_out = -r_out_win * a1.cos();
        let v1_out = wy_spring + r_out_win * a1.sin();

        // Anello frontale
        push_face(wall, &[
            pw(u0_in, v0_in, 0.0),
                  pw(u1_in, v1_in, 0.0),
                  pw(u1_out, v1_out, 0.0),
                  pw(u0_out, v0_out, 0.0),
        ], stone, 1.5, inside);

        // Estrusione esterna
        push_face(wall, &[
            pw(u0_out, v0_out, 0.0),
                  pw(u1_out, v1_out, 0.0),
                  pw(u1_out, v1_out, 0.16 * sc),
                  pw(u0_out, v0_out, 0.16 * sc),
        ], stone, 1.5, inside);

        // Intradosso
        push_face(wall, &[
            pw(u0_in, v0_in, 0.0),
                  pw(u1_in, v1_in, 0.0),
                  pw(u1_in, v1_in, 0.16 * sc),
                  pw(u0_in, v0_in, 0.16 * sc),
        ], stone, 1.5, inside);
    }

    // 4. Chiave di volta
    obox(wall,
         -0.16 * sc, 0.16 * sc,
         wy_top - 0.05 * sc, wy_top + 0.22 * sc,
         0.0, 0.20 * sc,
         stone, 1.0);

    // 5. Vetro con gradiente diagonale
    let z_glass = 0.05 * sc;
    let glass_norm = [n[0], 0.0, n[2]];
    let glass_color_at = |u: f32, v: f32| -> [f32; 4] {
        let u_norm = ((u + win_r) / (win_r * 2.0)).clamp(0.0, 1.0);
        let v_norm = (v / wy_top).clamp(0.0, 1.0);
        let diag = (1.0 - u_norm) * 0.55 + v_norm * 0.45;
        [0.10 + 0.38 * diag, 0.22 + 0.50 * diag, 0.48 + 0.46 * diag, 1.0]
    };

    // Rettangolo del vetro
    let g_base = windows.positions.len() as u32;
    let quad = [
        (pw(-win_r, 0.0, z_glass), glass_color_at(-win_r, 0.0), [0.0, 1.0]),
        (pw(win_r, 0.0, z_glass), glass_color_at(win_r, 0.0), [1.0, 1.0]),
        (pw(win_r, wy_spring, z_glass), glass_color_at(win_r, wy_spring), [1.0, 0.0]),
        (pw(-win_r, wy_spring, z_glass), glass_color_at(-win_r, wy_spring), [0.0, 0.0]),
    ];
    for (pos, col, uv) in quad {
        windows.positions.push(pos);
        windows.normals.push(glass_norm);
        windows.uvs.push(uv);
        windows.colors.push(col);
    }
    windows.indices.extend_from_slice(&[
        g_base, g_base + 1, g_base + 2,
        g_base, g_base + 2, g_base + 3,
    ]);

    // Arco del vetro a ventaglio (semicerchio)
    let c_center = pw(0.0, wy_spring, z_glass);
    let c_idx = windows.positions.len() as u32;
    windows.positions.push(c_center);
    windows.normals.push(glass_norm);
    windows.uvs.push([0.5, 0.5]);
    windows.colors.push(glass_color_at(0.0, wy_spring));

    let n_fan = 12;
    for k in 0..=n_fan {
        let a = (k as f32 / n_fan as f32) * PI;
        let u = -win_r * a.cos();
        let v = wy_spring + win_r * a.sin();
        windows.positions.push(pw(u, v, z_glass));
        windows.normals.push(glass_norm);
        windows.uvs.push([(u + win_r) / (win_r * 2.0), v / wy_top]);
        windows.colors.push(glass_color_at(u, v));
    }
    for k in 0..n_fan {
        let p0 = c_idx + 1 + k;
        let p1 = c_idx + 2 + k;
        windows.indices.extend_from_slice(&[c_idx, p1, p0]);
    }

    // 6. Inferriate incrociate
    let t_bar = 0.035 * sc;
    let bar_n0 = z_glass + 0.01 * sc;
    let bar_n1 = z_glass + 0.04 * sc;

    obox(trim, -t_bar * 0.5, t_bar * 0.5,
         0.0, wy_top - 0.02 * sc,
         bar_n0, bar_n1,
         iron_bar_c, 1.0);

    let y_cross = wy_spring * 0.58;
    obox(trim, -win_r, win_r,
         y_cross - t_bar * 0.5, y_cross + t_bar * 0.5,
         bar_n0, bar_n1,
         iron_bar_c, 1.0);

    obox(trim, -0.035 * sc, 0.035 * sc,
         y_cross - 0.035 * sc, y_cross + 0.035 * sc,
         bar_n1, bar_n1 + 0.02 * sc,
         [0.08, 0.08, 0.09, 1.0], 1.0);
}

// =========================================================================
// TORRE DELLA GILDA DEI MAGHI (MAGE) — OTTAGONO A 7 PIANI
// =========================================================================

pub fn generate_mage_tower(fp: &Footprint, btex: BuildingTextures) -> BuildingMesh {
    let mut mesh = BuildingMesh::default();
    mesh.wall_texture = btex.wall.or(Some((335, 0)));
    mesh.roof_texture = btex.roof.or(Some((339, 0)));
    mesh.door_texture = btex.door.or(Some((74, 0)));
    mesh.window_texture = None;

    let stone = [1.0, 1.0, 1.0, 1.0];
    let trim_c = [0.25, 0.22, 0.20, 1.0];
    let dome_c = [1.0, 1.0, 1.0, 1.0];
    let gold_c = [0.85, 0.70, 0.25, 1.0];
    let iron_bar_c = [0.03, 0.03, 0.035, 1.0];

    let r = (fp.width.min(fp.depth) * 0.5).max(7.0);
    let total_h = fp.height.max(46.0);
    // Ottagono: lati piatti come nelle torri dei maghi di Daggerfall.
    let sides = 8;

    let BuildingMesh { parts: [wall, roof, trim, _], doors, windows, .. } = &mut mesh;

    let angle_offset = PI / sides as f32;
    // Lato con centro a 270° (verso -Z, dove arriva il giocatore):
    //   angle_offset + (i + 0.5) * 2PI/sides = 270° → con sides = 8 → i = 5.
    let door_side_idx = 5usize;
    let wall_front_z = -r * (PI / sides as f32).cos();

    // ===== FONDAZIONE =====
    let r_base = r * 1.12;
    push_cylinder(wall, 0.0, 0.0, r_base, -0.5, 0.2, sides, stone, 4.0,
                  angle_offset, Some(door_side_idx));
    push_ring(trim, 0.0, 0.0, r_base * 0.98, r_base * 1.02, 0.1, 0.3, sides,
              trim_c, 2.0, angle_offset, Some(door_side_idx));

    // ===== CORPO A 7 PIANI =====
    const N_FLOORS: usize = 10;
    let y_floor = 0.0;
    let floor_h = total_h / N_FLOORS as f32;
    let mut floor_ys = Vec::with_capacity(N_FLOORS + 1);
    floor_ys.push(y_floor);
    for i in 1..=N_FLOORS {
        floor_ys.push(y_floor + floor_h * i as f32);
    }

    for i in 0..N_FLOORS {
        push_cylinder(wall, 0.0, 0.0, r, floor_ys[i], floor_ys[i + 1],
                      sides, stone, 4.0, angle_offset, None);
    }

    // Marcapiani tra i piani
    let r_cornice = r + 0.28;
    for i in 1..N_FLOORS {
        let y = floor_ys[i];
        push_ring(wall, 0.0, 0.0, r, r_cornice, y - 0.15, y + 0.15, sides,
                  trim_c, 2.0, angle_offset, None);
    }

    // Anello in cima
    let r_top = r + 0.55;
    let y_top_ring = floor_ys[N_FLOORS];
    push_ring(trim, 0.0, 0.0, r, r_top, y_top_ring, y_top_ring + 0.65, sides,
              trim_c, 2.0, angle_offset, None);

    // Cupola e guglia
    let dome_r = r_top;
    let dome_h = r * 2.9;
    push_cone(roof, 0.0, 0.0, dome_r, y_top_ring + 0.65,
              y_top_ring + 0.65 + dome_h, sides, dome_c, 3.5, angle_offset);
    let spire_base_y = y_top_ring + 0.65 + dome_h;
    push_cone(trim, 0.0, 0.0, 0.6, spire_base_y, spire_base_y + 3.5, 8,
              gold_c, 1.5, 0.0);

    // ===== PORTALE =====
    build_arched_guild_portal(wall, doors, trim, stone, 0.0, wall_front_z, y_floor);

    // ===== FINESTRE IN STILE PALAZZO REALE (a mezza scala) =====
    // Dimensioni fedeli del palazzo, dimezzate.
    let win_scale = 0.5f32;
    let win_r = 0.92 * win_scale;         // 0.46 m
    let win_h_rect = 2.05 * win_scale;    // 1.025 m
    let win_h_total = win_h_rect + win_r; // 1.485 m

    let apothem = r * (PI / sides as f32).cos();

    for floor_idx in 0..N_FLOORS {
        // Piano terra: nessuna finestra (solo il portale).
        if floor_idx == 0 {
            continue;
        }

        let floor_bottom = floor_ys[floor_idx];
        let local_floor_h = floor_ys[floor_idx + 1] - floor_bottom;

        // Margine minimo di 0.3 m dal fondo del piano; se non basta, salta.
        let margin = (local_floor_h - win_h_total).max(0.0);
        let win_base_v = (margin * 0.5).max(0.3);
        if win_base_v + win_h_total > local_floor_h {
            continue;
        }
        let win_base_y = floor_bottom + win_base_v;

        // Alterna la parità del lato per piano, così le finestre di piani
        // consecutivi non sono allineate verticalmente (zig-zag).
        //   Piano 1 (parità 1): lati 1, 3, 5, 7
        //   Piano 2 (parità 0): lati 0, 2, 4, 6
        //   Piano 3 (parità 1): lati 1, 3, 5, 7
        //   ...
        let face_parity = floor_idx % 2;

        for face_idx in 0..sides {
            if face_idx % 2 != face_parity {
                continue;
            }

            let angle_center =
            angle_offset + (face_idx as f32 + 0.5) * (TAU / sides as f32);
            let (sin_a, cos_a) = angle_center.sin_cos();

            let base_center = [apothem * cos_a, win_base_y, apothem * sin_a];
            let face_normal = [cos_a, 0.0, sin_a];

            push_palace_window(
                wall, windows, trim,
                base_center, face_normal,
                win_r, win_h_rect, win_scale,
                stone, iron_bar_c,
            );
        }
    }

    mesh
}

// =========================================================================
// MASCHIO DEI GUERRIERI (FIGH)
// =========================================================================

pub fn generate_fighters_hall(fp: &Footprint, btex: BuildingTextures) -> BuildingMesh {
    let mut mesh = BuildingMesh::default();
    mesh.wall_texture = btex.wall.or(Some((317, 0)));
    mesh.roof_texture = btex.roof.or(Some((69, 0)));
    mesh.door_texture = btex.door.or(Some((74, 0)));
    mesh.window_texture = None;

    let stone = [1.0, 1.0, 1.0, 1.0];
    let trim_c = [0.25, 0.22, 0.20, 1.0];
    let guild_glass = [0.10, 0.28, 0.55, 1.0];
    let guild_iron = [0.03, 0.03, 0.035, 1.0];

    let BuildingMesh { parts: [wall, _roof, trim, _], doors, windows, .. } = &mut mesh;

    let hw = (fp.width * 0.5).max(7.0);
    let hd = (fp.depth * 0.5).max(7.0);
    let total_h = fp.height.max(12.0);

    let y_floor = 0.0;
    let y1 = total_h * 0.55;
    let y2 = total_h - 1.5;

    push_box(wall, [-hw, y_floor, -hd], [hw, y1, hd], stone, 4.0);
    push_box(wall, [-hw, y1, -hd], [hw, y2, hd], stone, 4.0);

    // Cornicione in pietra (prima era in `trim` con colore fisso, "senza texture":
    // ora è in `wall` e eredita la texture del muro).
    push_box(
        wall,
        [-hw - 0.25, y1 - 0.15, -hd - 0.25],
        [hw + 0.25, y1 + 0.15, hd + 0.25],
        stone,
        4.0,
    );

    merlon_row(wall, true, hd, hw, y2, stone);
    merlon_row(wall, true, -hd, hw, y2, stone);
    merlon_row(wall, false, hw, hd, y2, stone);
    merlon_row(wall, false, -hw, hd, y2, stone);

    build_arched_guild_portal(wall, doors, trim, stone, 0.0, -hd, y_floor);

    // Finestre arricchite (piano terra + secondo piano).
    for &wx in &[-hw * 0.6, hw * 0.6] {
        push_rich_window(
            wall, windows, trim,
            [wx, y_floor + 1.6, -hd],
            WallAxis::FrontNegZ,
            0.5, 1.0,
            stone, guild_glass, guild_iron,
        );
        push_rich_window(
            wall, windows, trim,
            [wx, y1 + 1.2, -hd],
            WallAxis::FrontNegZ,
            0.5, 1.2,
            stone, guild_glass, guild_iron,
        );
    }
    for &wz in &[-hd * 0.5, hd * 0.5] {
        push_rich_window(
            wall, windows, trim,
            [-hw, y_floor + 1.6, wz],
            WallAxis::LeftNegX,
            0.5, 1.0,
            stone, guild_glass, guild_iron,
        );
        push_rich_window(
            wall, windows, trim,
            [hw, y_floor + 1.6, wz],
            WallAxis::RightPosX,
            0.5, 1.0,
            stone, guild_glass, guild_iron,
        );
    }

    let tx = hw;
    let tz = -hd;
    let t_size = 2.5;
    let t_h = total_h + 3.0;
    push_box(wall, [tx - t_size, y_floor, tz - t_size], [tx + t_size, t_h, tz + t_size], stone, 3.0);
    push_box(trim, [tx - t_size - 0.2, t_h - 0.3, tz - t_size - 0.2], [tx + t_size + 0.2, t_h, tz + t_size + 0.2], trim_c, 2.0);

    mesh
}

// =========================================================================
// SALA GENERICA / THIEVES GUILD (THIE, DARK, GUIL)
// =========================================================================

pub fn generate_generic_hall(fp: &Footprint, btex: BuildingTextures) -> BuildingMesh {
    let mut mesh = BuildingMesh::default();
    mesh.wall_texture = btex.wall.or(Some((335, 0)));
    mesh.roof_texture = btex.roof.or(Some((339, 0)));
    mesh.door_texture = btex.door.or(Some((74, 0)));
    mesh.window_texture = None;

    let stone = [1.0, 1.0, 1.0, 1.0];
    let guild_glass = [0.10, 0.28, 0.55, 1.0];
    let guild_iron = [0.03, 0.03, 0.035, 1.0];

    let BuildingMesh { parts: [wall, _roof, trim, _], doors, windows, .. } = &mut mesh;

    let hw = (fp.width * 0.5).max(6.0);
    let hd = (fp.depth * 0.5).max(6.0);
    let total_h = fp.height.max(10.0);

    let y_floor = 0.0;
    let y1 = total_h * 0.55;
    let y2 = total_h - 1.5;

    push_box(wall, [-hw, y_floor, -hd], [hw, y1, hd], stone, 4.0);
    push_box(wall, [-hw, y1, -hd], [hw, y2, hd], stone, 4.0);

    // Cornicione in pietra (prima in `trim` con colore fisso).
    push_box(
        wall,
        [-hw - 0.25, y1 - 0.15, -hd - 0.25],
        [hw + 0.25, y1 + 0.15, hd + 0.25],
        stone,
        4.0,
    );

    merlon_row(wall, true, hd, hw, y2, stone);
    merlon_row(wall, true, -hd, hw, y2, stone);

    build_arched_guild_portal(wall, doors, trim, stone, 0.0, -hd, y_floor);

    // Finestre arricchite su fronte e retro (prima erano box blu piatti).
    for (fy, fh) in [(y_floor + 1.6, 1.0f32), (y1 + 1.2, 1.2)] {
        for &wx in &[-hw * 0.6, hw * 0.6] {
            push_rich_window(
                wall, windows, trim,
                [wx, fy + fh * 0.5, -hd],
                WallAxis::FrontNegZ,
                0.8, fh,
                stone, guild_glass, guild_iron,
            );
            push_rich_window(
                wall, windows, trim,
                [wx, fy + fh * 0.5, hd],
                WallAxis::BackPosZ,
                0.8, fh,
                stone, guild_glass, guild_iron,
            );
        }
    }

    mesh
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuildStyle {
    Mage,
    Fighter,
    Thief,
    Hall,
}

pub fn generate_guild_hall(
    fp: &Footprint,
    _front: Side,
    _seed: u32,
    btex: BuildingTextures,
    style: GuildStyle,
) -> BuildingMesh {
    match style {
        GuildStyle::Mage => generate_mage_tower(fp, btex),
        GuildStyle::Fighter => generate_fighters_hall(fp, btex),
        GuildStyle::Thief => generate_generic_hall(fp, btex),
        GuildStyle::Hall => generate_generic_hall(fp, btex),
    }
}
