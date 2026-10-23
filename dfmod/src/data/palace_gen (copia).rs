//! Generatore procedurale del Palazzo Reale per Daggerfall con maschio circolare saldato al portale e fossato.
use super::building_gen::{BuildingMesh, MeshPart};
use std::f32::consts::PI;

type V3 = [f32; 3];

fn normal(a: V3, b: V3, c: V3) -> Option<V3> {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [ab[1]*ac[2] - ab[2]*ac[1], ab[2]*ac[0] - ab[0]*ac[2], ab[0]*ac[1] - ab[1]*ac[0]];
    let l = (n[0]*n[0] + n[1]*n[1] + n[2]*n[2]).sqrt();
    if l < 1e-6 { None } else { Some([n[0]/l, n[1]/l, n[2]/l]) }
}

fn push_face(part: &mut MeshPart, pts: &[V3], color: [f32; 4], s: f32, inside: V3) {
    let raw = match normal(pts[0], pts[1], pts[2]) { Some(n) => n, None => return };
    let c = [pts.iter().map(|p| p[0]).sum::<f32>() / pts.len() as f32,
    pts.iter().map(|p| p[1]).sum::<f32>() / pts.len() as f32,
    pts.iter().map(|p| p[2]).sum::<f32>() / pts.len() as f32];
    let to_c = [c[0] - inside[0], c[1] - inside[1], c[2] - inside[2]];
    let flip = raw[0]*to_c[0] + raw[1]*to_c[1] + raw[2]*to_c[2] < 0.0;
    let n = if flip { [-raw[0], -raw[1], -raw[2]] } else { raw };
    let base = part.positions.len() as u32;
    let order: Vec<usize> = if flip { (0..pts.len()).rev().collect() } else { (0..pts.len()).collect() };
    for &i in &order {
        let p = pts[i];
        let (ax, ay, az) = (n[0].abs(), n[1].abs(), n[2].abs());
        let (u, v) = if ay >= ax && ay >= az { (p[0], p[2]) } else if ax >= az { (p[2], p[1]) } else { (p[0], p[1]) };
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
    let p = [[a[0],a[1],a[2]],[b[0],a[1],a[2]],[a[0],b[1],a[2]],[b[0],b[1],a[2]],
    [a[0],a[1],b[2]],[b[0],a[1],b[2]],[a[0],b[1],b[2]],[b[0],b[1],b[2]]];
    let ins = [(a[0]+b[0])*0.5, (a[1]+b[1])*0.5, (a[2]+b[2])*0.5];
    for f in [[0,2,3,1],[5,7,6,4],[4,6,2,0],[1,3,7,5],[2,6,7,3],[4,0,1,5]] {
        push_face(part, &[p[f[0]], p[f[1]], p[f[2]], p[f[3]]], col, s, ins);
    }
}

fn push_pyr(part: &mut MeshPart, xz0: [f32; 2], xz1: [f32; 2], y0: f32, apex: V3, col: [f32; 4], s: f32) {
    let ins = [(xz0[0]+xz1[0])*0.5, y0 - 1.0, (xz0[1]+xz1[1])*0.5];
    let b = [[xz0[0], y0, xz0[1]], [xz1[0], y0, xz0[1]], [xz1[0], y0, xz1[1]], [xz0[0], y0, xz1[1]]];
    push_face(part, &[b[0], b[1], apex], col, s, ins);
    push_face(part, &[b[1], b[2], apex], col, s, ins);
    push_face(part, &[b[2], b[3], apex], col, s, ins);
    push_face(part, &[b[3], b[0], apex], col, s, ins);
}

pub fn generate_royal_palace(wall_tex: Option<(u16, u8)>, roof_tex: Option<(u16, u8)>) -> BuildingMesh {
    let mut mesh = BuildingMesh::default();
    mesh.wall_texture = wall_tex.or(Some((317, 0)));
    mesh.roof_texture = roof_tex.or(Some((69, 0)));
    mesh.door_texture = Some((74, 0));
    mesh.window_texture = Some((171, 4));

    let stone = [1.0, 1.0, 1.0, 1.0];
    let roof_c = [1.0, 1.0, 1.0, 1.0];
    let trim_c = [0.25, 0.22, 0.20, 1.0];
    let gold_c = [0.85, 0.70, 0.25, 1.0];
    let water_c = [0.08, 0.22, 0.36, 1.0];

    let BuildingMesh { parts: [wall, roof, trim, _], doors, windows, .. } = &mut mesh;

    // =========================================================================
    // 1. IL PALAZZO INTERNO
    // =========================================================================
    push_box(wall, [-34.0, -0.4, -25.0], [34.0, 1.5, 25.0], stone, 5.0);
    push_box(wall, [-18.0, 1.5, -13.0], [18.0, 21.5, 13.0], stone, 4.0);

    let r_ins = [0.0, 20.0, 0.0];
    push_face(roof, &[[-19.0, 21.5, 14.0], [19.0, 21.5, 14.0], [19.0, 29.5, 0.0], [-19.0, 29.5, 0.0]], roof_c, 3.5, r_ins);
    push_face(roof, &[[19.0, 21.5, -14.0], [-19.0, 21.5, -14.0], [-19.0, 29.5, 0.0], [19.0, 29.5, 0.0]], roof_c, 3.5, r_ins);
    push_face(wall, &[[-18.0, 21.5, 13.0], [-18.0, 21.5, -13.0], [-18.0, 29.5, 0.0]], stone, 4.0, r_ins);
    push_face(wall, &[[18.0, 21.5, -13.0], [18.0, 21.5, 13.0], [18.0, 29.5, 0.0]], stone, 4.0, r_ins);

    for (tx, tz) in [(-18.0, -13.0), (18.0, -13.0), (18.0, 13.0), (-18.0, 13.0)] {
        push_box(wall, [tx - 4.0, 1.5, tz - 4.0], [tx + 4.0, 28.0, tz + 4.0], stone, 4.0);
        push_box(trim, [tx - 4.5, 27.2, tz - 4.5], [tx + 4.5, 28.0, tz + 4.5], trim_c, 3.0);
        push_pyr(roof, [tx - 4.1, tz - 4.1], [tx + 4.1, tz + 4.1], 28.0, [tx, 37.0, tz], roof_c, 3.0);
        push_box(trim, [tx - 0.15, 37.0, tz - 0.15], [tx + 0.15, 38.8, tz + 0.15], gold_c, 1.0);
    }

    push_box(wall, [-5.0, 21.5, -16.0], [5.0, 36.0, -6.0], stone, 4.0);
    push_pyr(roof, [-4.6, -15.6], [4.6, -6.4], 36.0, [0.0, 46.0, -11.0], roof_c, 3.0);

    push_box(wall, [-30.0, 1.5, -11.0], [-18.0, 14.5, 11.0], stone, 4.0);
    push_box(wall, [18.0, 1.5, -11.0], [30.0, 14.5, 11.0], stone, 4.0);

    for s in 0..10 {
        let f = s as f32 / 10.0;
        let hw = 9.0 - f * 1.5;
        let y_top = 1.5 - f * 0.7;
        let z0 = 25.0 + f * 1.4;
        let z1 = z0 + 1.4;
        push_box(wall, [-hw, 0.0, z0], [hw, y_top, z1], stone, 2.5);
    }

    push_box(wall, [-3.8, 1.5, 13.0], [-2.2, 8.5, 13.8], stone, 2.0);
    push_box(wall, [2.2, 1.5, 13.0], [3.8, 8.5, 13.8], stone, 2.0);
    push_box(wall, [-3.8, 8.5, 13.0], [3.8, 10.5, 13.8], stone, 2.0);
    push_box(trim, [-4.2, 10.5, 12.8], [4.2, 11.2, 14.0], gold_c, 1.0);

    let d_base = doors.positions.len() as u32;
    for p in [[-2.2, 1.5, 13.3], [2.2, 1.5, 13.3], [2.2, 8.5, 13.3], [-2.2, 8.5, 13.3]] {
        doors.positions.push(p);
        doors.normals.push([0.0, 0.0, 1.0]);
        doors.colors.push([1.0, 1.0, 1.0, 1.0]);
    }
    doors.uvs.extend_from_slice(&[[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
    doors.indices.extend_from_slice(&[d_base, d_base + 1, d_base + 2, d_base, d_base + 2, d_base + 3]);

    for fy in [6.0f32, 13.0] {
        for wx in [-11.0, -7.0, 7.0, 11.0] {
            let wb = windows.positions.len() as u32;
            for p in [[wx - 0.9, fy, 13.05], [wx + 0.9, fy, 13.05], [wx + 0.9, fy + 3.2, 13.05], [wx - 0.9, fy + 3.2, 13.05]] {
                windows.positions.push(p);
                windows.normals.push([0.0, 0.0, 1.0]);
                windows.colors.push([1.0, 1.0, 1.0, 1.0]);
            }
            windows.uvs.extend_from_slice(&[[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
            windows.indices.extend_from_slice(&[wb, wb + 1, wb + 2, wb, wb + 2, wb + 3]);
            push_box(trim, [wx - 1.1, fy - 0.2, 12.95], [wx + 1.1, fy, 13.1], trim_c, 1.0);
        }
    }

    // =========================================================================
    // 2. PORTALE AD ARCO E BARBACANE A SUD
    // =========================================================================
    push_box(wall, [-9.5, -1.4, 36.0], [-4.5, 17.5, 43.0], stone, 4.0);
    push_pyr(roof, [-9.2, 36.3], [-4.8, 42.7], 17.5, [-7.0, 23.5, 39.5], roof_c, 3.0);

    push_box(wall, [4.5, -1.4, 36.0], [9.5, 17.5, 43.0], stone, 4.0);
    push_pyr(roof, [4.8, 36.3], [9.2, 42.7], 17.5, [7.0, 23.5, 39.5], roof_c, 3.0);

    push_box(wall, [-4.5, 7.5, 37.5], [4.5, 14.5, 41.5], stone, 3.0);
    push_box(trim, [-4.8, 14.5, 37.0], [4.8, 15.2, 42.0], gold_c, 1.0);
    push_box(wall, [-4.0, 15.2, 37.2], [-1.0, 16.6, 38.2], stone, 1.5);
    push_box(wall, [1.0, 15.2, 37.2], [4.0, 16.6, 38.2], stone, 1.5);

    push_box(wall, [-6.0, -0.4, 39.0], [6.0, 0.8, 52.0], stone, 3.0);
    push_box(wall, [-6.6, 0.8, 39.0], [-5.8, 2.1, 52.0], stone, 2.0);
    push_box(wall, [5.8, 0.8, 39.0], [6.6, 2.1, 52.0], stone, 2.0);

    // =========================================================================
    // 3. MASCHIO CIRCOLARE SALDATO ALLE TORRI DEL PORTALE
    // =========================================================================
    let r_wall_in = 37.5;
    let r_wall_out = 40.5;
    let r_moat_out = 48.0;
    let wall_h = 11.0;
    let water_y = -0.4;
    let center_ins = [0.0, 5.0, 0.0];

    let gate_alpha = (9.5f32 / r_wall_out).asin();
    let segs = 28;
    let total_arc = 2.0 * PI - (2.0 * gate_alpha);

    for i in 0..segs {
        let a0 = gate_alpha + (i as f32 / segs as f32) * total_arc;
        let a1 = gate_alpha + ((i + 1) as f32 / segs as f32) * total_arc;

        let s0 = a0.sin();
        let c0 = a0.cos();
        let s1 = a1.sin();
        let c1 = a1.cos();

        let wp = [
            [r_wall_out * s0, water_y, r_wall_out * c0],
            [r_moat_out * s0, water_y, r_moat_out * c0],
            [r_moat_out * s1, water_y, r_moat_out * c1],
            [r_wall_out * s1, water_y, r_wall_out * c1],
        ];
        push_face(trim, &wp, water_c, 4.0, [0.0, water_y - 1.0, 0.0]);

        // RIMOSSO: l'argine esterno del fossato (`bank`).
        // Era un muro verticale isolato a raggio 48 che sporgeva di 0.8 m
        // sopra il terreno, senza faccia superiore né collegamenti. Era
        // il "bordo di mattoni staccato" che girava intorno al maschio.

        let wf_out = [
            [r_wall_out * s0, -1.2, r_wall_out * c0],
            [r_wall_out * s1, -1.2, r_wall_out * c1],
            [r_wall_out * s1, wall_h, r_wall_out * c1],
            [r_wall_out * s0, wall_h, r_wall_out * c0],
        ];
        push_face(wall, &wf_out, stone, 4.0, center_ins);

        let mid_s = (s0 + s1) * 0.5;
        let mid_c = (c0 + c1) * 0.5;
        let seg_inside = [r_wall_in * mid_s * 1.5, 5.0, r_wall_in * mid_c * 1.5];

        let wf_in = [
            [r_wall_in * s0, -0.4, r_wall_in * c0],
            [r_wall_in * s0, wall_h, r_wall_in * c0],
            [r_wall_in * s1, wall_h, r_wall_in * c1],
            [r_wall_in * s1, -0.4, r_wall_in * c1],
        ];
        push_face(wall, &wf_in, stone, 4.0, seg_inside);

        let walk = [
            [r_wall_in * s0, wall_h, r_wall_in * c0],
            [r_wall_out * s0, wall_h, r_wall_out * c0],
            [r_wall_out * s1, wall_h, r_wall_out * c1],
            [r_wall_in * s1, wall_h, r_wall_in * c1],
        ];
        push_face(wall, &walk, stone, 3.0, [0.0, wall_h - 1.0, 0.0]);

        if i % 2 == 0 {
            let merl = [
                [r_wall_out * s0, wall_h, r_wall_out * c0],
                [r_wall_out * s1, wall_h, r_wall_out * c1],
                [r_wall_out * s1, wall_h + 1.4, r_wall_out * c1],
                [r_wall_out * s0, wall_h + 1.4, r_wall_out * c0],
            ];
            push_face(wall, &merl, stone, 2.0, center_ins);
        }
    }

    for (bx, bz) in [(0.0, -39.0), (39.0, 0.0), (-39.0, 0.0)] {
        push_box(wall, [bx - 4.5, -0.5, bz - 4.5], [bx + 4.5, 17.0, bz + 4.5], stone, 4.0);
        push_box(trim, [bx - 5.0, 16.2, bz - 5.0], [bx + 5.0, 17.0, bz + 5.0], trim_c, 3.0);
        push_pyr(roof, [bx - 4.6, bz - 4.6], [bx + 4.6, bz + 4.6], 17.0, [bx, 24.5, bz], roof_c, 3.0);
    }

    mesh
}
