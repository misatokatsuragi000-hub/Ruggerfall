//! Generatore procedurale del Palazzo Reale per Daggerfall.
//! Contiene esclusivamente il Palazzo Reale con fossato, corte d'onore, torri e portale monumentale.

use super::arch3d::BuildingTextures;
use super::building_gen::{BuildingMesh, MeshPart};
use std::f32::consts::PI;

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

pub fn generate_royal_palace(wall_tex: Option<(u16, u8)>, roof_tex: Option<(u16, u8)>) -> BuildingMesh {
    let mut mesh = BuildingMesh::default();
    mesh.wall_texture = wall_tex.or(Some((317, 0)));
    mesh.roof_texture = roof_tex.or(Some((69, 0)));
    mesh.door_texture = Some((74, 0));
    mesh.window_texture = None;

    let stone = [1.0, 1.0, 1.0, 1.0];
    let roof_c = [1.0, 1.0, 1.0, 1.0];
    let trim_c = [0.25, 0.22, 0.20, 1.0];
    let water_c = [0.08, 0.22, 0.36, 1.0];
    let iron_c = [0.03, 0.03, 0.035, 1.0];
    let stud_c = [0.08, 0.08, 0.09, 1.0];

    let BuildingMesh { parts: [wall, roof, trim, _], doors, windows, .. } = &mut mesh;

    let y_ground_paved = 0.19f32;
    let y_terrace = 1.35f32;
    let y_main_roof = 33.5f32;

    // 1. Corpo del palazzo a 5 piani
    push_box(wall, [-34.0, -0.4, -25.0], [34.0, y_terrace, 25.0], stone, 5.0);
    push_box(wall, [-18.0, y_terrace, -13.0], [18.0, y_main_roof, 13.0], stone, 4.0);

    for &y_cornice in &[7.2f32, 13.7f32, 20.2f32, 26.7f32] {
        push_box(trim, [-18.3, y_cornice - 0.12, -13.2], [18.3, y_cornice + 0.12, 13.2], trim_c, 2.0);
    }

    push_box(wall, [-9.2, 0.0, 13.0], [9.2, y_terrace + 0.02, 25.0], stone, 2.5);
    push_box(wall, [-9.6, y_terrace, 13.0], [-9.1, y_terrace + 0.35, 25.0], stone, 1.5);
    push_box(wall, [9.1, y_terrace, 13.0], [9.6, y_terrace + 0.35, 25.0], stone, 1.5);

    let r_ins = [0.0, y_main_roof, 0.0];
    let y_ridge = y_main_roof + 8.5;
    push_face(roof, &[[-19.0, y_main_roof, 14.0], [19.0, y_main_roof, 14.0], [19.0, y_ridge, 0.0], [-19.0, y_ridge, 0.0]], roof_c, 3.5, r_ins);
    push_face(roof, &[[19.0, y_main_roof, -14.0], [-19.0, y_main_roof, -14.0], [-19.0, y_ridge, 0.0], [19.0, y_ridge, 0.0]], roof_c, 3.5, r_ins);
    push_face(wall, &[[-18.0, y_main_roof, 13.0], [-18.0, y_main_roof, -13.0], [-18.0, y_ridge, 0.0]], stone, 4.0, r_ins);
    push_face(wall, &[[18.0, y_main_roof, -13.0], [18.0, y_main_roof, 13.0], [18.0, y_ridge, 0.0]], stone, 4.0, r_ins);

    // Torri angolari a 5 piani
    let y_tower_top = y_main_roof + 7.5;
    for (tx, tz) in [(-18.0, -13.0), (18.0, -13.0), (18.0, 13.0), (-18.0, 13.0)] {
        push_box(wall, [tx - 4.0, y_terrace, tz - 4.0], [tx + 4.0, y_tower_top, tz + 4.0], stone, 4.0);
        push_box(trim, [tx - 4.5, y_tower_top - 0.8, tz - 4.5], [tx + 4.5, y_tower_top, tz + 4.5], trim_c, 3.0);
        push_pyr(roof, [tx - 4.1, tz - 4.1], [tx + 4.1, tz + 4.1], y_tower_top, [tx, y_tower_top + 10.0, tz], roof_c, 3.0);
        push_pyr(trim, [tx - 0.2, tz - 0.2], [tx + 0.2, tz + 0.2], y_tower_top + 10.0, [tx, y_tower_top + 12.0, tz], trim_c, 1.0);
    }

    // Maschio centrale
    let y_donjon_top = y_main_roof + 16.0;
    push_box(wall, [-5.0, y_main_roof, -16.0], [5.0, y_donjon_top, -6.0], stone, 4.0);
    push_pyr(roof, [-4.6, -15.6], [4.6, -6.4], y_donjon_top, [0.0, y_donjon_top + 11.0, -11.0], roof_c, 3.0);

    // Ali laterali
    push_box(wall, [-30.0, y_terrace, -11.0], [-18.0, 21.0, 11.0], stone, 4.0);
    push_box(wall, [18.0, y_terrace, -11.0], [30.0, 21.0, 11.0], stone, 4.0);

    for s in 0..10 {
        let f = s as f32 / 10.0;
        let hw = 9.2 - f * 1.4;
        let y_step = y_terrace - f * (y_terrace - y_ground_paved);
        let z0 = 25.0 + f * 1.1;
        let z1 = z0 + 1.1;
        push_box(wall, [-hw, 0.0, z0], [hw, y_step, z1], stone, 2.5);
    }

    // Portale monumentale 3D del palazzo
    let r_arch = 2.2f32;
    let y_door_base = y_terrace;
    let y_spring = y_door_base + 4.8f32;
    let y_arch_top = y_spring + r_arch;
    let z_door_front = 13.40f32;
    let z_door_back = 13.30f32;

    push_box(wall, [-3.8, y_door_base, 13.0], [-2.2, y_spring, 13.8], stone, 2.0);
    push_box(wall, [2.2, y_door_base, 13.0], [3.8, y_spring, 13.8], stone, 2.0);
    push_box(wall, [-3.8, y_spring, 13.0], [-2.2, 8.95, 13.8], stone, 2.0);
    push_box(wall, [2.2, y_spring, 13.0], [3.8, 8.95, 13.8], stone, 2.0);
    push_box(wall, [-3.8, 8.95, 13.0], [3.8, 10.5, 13.8], stone, 2.0);
    push_box(wall, [-4.0, 10.5, 12.8], [4.0, 11.0, 13.9], stone, 2.0);

    let vault_segs = 16;
    for k in 0..vault_segs {
        let t0 = (k as f32 / vault_segs as f32) * PI;
        let t1 = ((k + 1) as f32 / vault_segs as f32) * PI;

        let x0 = -r_arch * t0.cos();
        let y0 = y_spring + r_arch * t0.sin();
        let x1 = -r_arch * t1.cos();
        let y1 = y_spring + r_arch * t1.sin();

        push_face(wall, &[[x0, y0, 13.0], [x1, y1, 13.0], [x1, y1, 13.8], [x0, y0, 13.8]], stone, 2.0, [0.0, y_spring, 13.4]);
        push_face(wall, &[[x0, y0, 13.8], [x1, y1, 13.8], [x1, 8.95, 13.8], [x0, 8.95, 13.8]], stone, 2.0, [0.0, y_spring, 13.4]);
    }

    let z_mid = (z_door_front + z_door_back) * 0.5;
    push_box(wall, [-2.2, y_door_base, z_door_back - 0.05], [2.2, y_arch_top, z_door_back], stone, 2.0);

    let build_continuous_vertical_plank = |doors: &mut MeshPart, x0: f32, x1: f32, col: [f32; 4]| {
        let n_slices = 3;
        let ins = [(x0 + x1) * 0.5, y_spring, z_mid];

        for s in 0..n_slices {
            let ua = x0 + (x1 - x0) * (s as f32 / n_slices as f32);
            let ub = x0 + (x1 - x0) * ((s + 1) as f32 / n_slices as f32);

            let ya = y_spring + (r_arch * r_arch - (ua * ua).min(r_arch * r_arch - 1e-4)).sqrt();
            let yb = y_spring + (r_arch * r_arch - (ub * ub).min(r_arch * r_arch - 1e-4)).sqrt();

            push_face(doors, &[[ua, y_door_base, z_door_front], [ub, y_door_base, z_door_front], [ub, yb, z_door_front], [ua, ya, z_door_front]], col, 2.0, ins);
            push_face(doors, &[[ua, y_door_base, z_door_back], [ua, ya, z_door_back], [ub, yb, z_door_back], [ub, y_door_base, z_door_back]], col, 2.0, ins);
            push_face(doors, &[[ua, ya, z_door_front], [ua, ya, z_door_back], [ub, yb, z_door_back], [ub, yb, z_door_front]], col, 2.0, [0.0, y_spring, z_mid]);
        }

        push_face(doors, &[[x0, y_door_base, z_door_front], [x0, y_door_base, z_door_back], [x1, y_door_base, z_door_back], [x1, y_door_base, z_door_front]], col, 2.0, [ins[0], y_door_base + 0.5, z_mid]);

        let y_edge_left = y_spring + (r_arch * r_arch - (x0 * x0).min(r_arch * r_arch - 1e-4)).sqrt();
        let y_edge_right = y_spring + (r_arch * r_arch - (x1 * x1).min(r_arch * r_arch - 1e-4)).sqrt();

        push_face(doors, &[[x0, y_door_base, z_door_front], [x0, y_edge_left, z_door_front], [x0, y_edge_left, z_door_back], [x0, y_door_base, z_door_back]], col, 2.0, [x1, y_spring, z_mid]);
        push_face(doors, &[[x1, y_door_base, z_door_front], [x1, y_door_base, z_door_back], [x1, y_edge_right, z_door_back], [x1, y_edge_right, z_door_front]], col, 2.0, [x0, y_spring, z_mid]);
    };

    let wood_plank_tones = [
        [1.0, 1.0, 1.0, 1.0],
        [0.97, 0.97, 0.97, 1.0],
        [0.99, 0.99, 0.99, 1.0],
        [0.95, 0.95, 0.95, 1.0],
    ];

    for i in 0..4 {
        let x0 = -2.18 + i as f32 * 0.54;
        let x1 = x0 + 0.515;
        let col = wood_plank_tones[i % 4];
        build_continuous_vertical_plank(doors, x0, x1, col);
    }
    for i in 0..4 {
        let x0 = 0.025 + i as f32 * 0.54;
        let x1 = x0 + 0.515;
        let col = wood_plank_tones[(3 - i) % 4];
        build_continuous_vertical_plank(doors, x0, x1, col);
    }

    // Ferramenta
    for &ys in &[y_door_base + 1.2f32, y_door_base + 3.1, y_door_base + 4.8] {
        push_box(trim, [-2.15, ys - 0.06, z_door_front], [-0.04, ys + 0.06, z_door_front + 0.025], iron_c, 1.0);
        for &bx in &[-1.9f32, -1.4, -0.9, -0.4] {
            push_box(trim, [bx - 0.03, ys - 0.03, z_door_front + 0.025], [bx + 0.03, ys + 0.03, z_door_front + 0.045], stud_c, 1.0);
        }
        push_box(trim, [0.04, ys - 0.06, z_door_front], [2.15, ys + 0.06, z_door_front + 0.025], iron_c, 1.0);
        for &bx in &[0.4f32, 0.9, 1.4, 1.9] {
            push_box(trim, [bx - 0.03, ys - 0.03, z_door_front + 0.025], [bx + 0.03, ys + 0.03, z_door_front + 0.045], stud_c, 1.0);
        }
    }

    push_box(trim, [-0.03, y_door_base, z_door_front + 0.005], [0.03, y_arch_top - 0.04, z_door_front + 0.03], iron_c, 1.0);
    for &by in &[2.05f32, 3.55, 5.05, 6.55, 7.85] {
        push_box(trim, [-0.02, by - 0.02, z_door_front + 0.03], [0.02, by + 0.02, z_door_front + 0.048], stud_c, 1.0);
    }
    push_box(trim, [-0.46, y_door_base + 2.6, z_door_front + 0.025], [-0.34, y_door_base + 2.8, z_door_front + 0.06], iron_c, 1.0);
    push_box(trim, [0.34, y_door_base + 2.6, z_door_front + 0.025], [0.46, y_door_base + 2.8, z_door_front + 0.06], iron_c, 1.0);

    let win_r = 0.92f32;
    let win_h_rect = 2.05f32;
    let iron_bar_c = [0.03, 0.03, 0.035, 1.0];

    for &fy in &[8.0f32, 14.5f32, 21.0f32, 27.5f32] {
        for &wx in &[-11.0f32, -7.0f32, 7.0f32, 11.0f32] {
            let wy_spring = fy + win_h_rect;
            let wy_top = wy_spring + win_r;

            push_box(wall, [wx - 1.25, fy - 0.25, 12.92], [wx + 1.25, fy, 13.16], stone, 2.0);
            push_box(wall, [wx - 1.20, fy, 12.96], [wx - win_r, wy_spring, 13.12], stone, 1.5);
            push_box(wall, [wx + win_r, fy, 12.96], [wx + 1.20, wy_spring, 13.12], stone, 1.5);

            let n_win_arch = 12;
            let r_out_win = win_r + 0.25;
            for k in 0..n_win_arch {
                let t0 = (k as f32 / n_win_arch as f32) * PI;
                let t1 = ((k + 1) as f32 / n_win_arch as f32) * PI;

                let px0_in = wx - win_r * t0.cos();
                let py0_in = wy_spring + win_r * t0.sin();
                let px1_in = wx - win_r * t1.cos();
                let py1_in = wy_spring + win_r * t1.sin();

                let px0_out = wx - r_out_win * t0.cos();
                let py0_out = wy_spring + r_out_win * t0.sin();
                let px1_out = wx - r_out_win * t1.cos();
                let py1_out = wy_spring + r_out_win * t1.sin();

                push_face(wall, &[[px0_in, py0_in, 13.12], [px1_in, py1_in, 13.12], [px1_out, py1_out, 13.12], [px0_out, py0_out, 13.12]], stone, 1.5, [wx, wy_spring, 12.90]);
                push_face(wall, &[[px0_in, py0_in, 12.96], [px1_in, py1_in, 12.96], [px1_in, py1_in, 13.12], [px0_in, py0_in, 13.12]], stone, 1.5, [wx, wy_spring, 13.04]);
            }

            push_box(wall, [wx - 0.16, wy_top - 0.05, 12.95], [wx + 0.16, wy_top + 0.22, 13.15], stone, 1.0);

            let z_glass = 13.04f32;
            let glass_col_at = |x: f32, y: f32| -> [f32; 4] {
                let u_norm = ((x - (wx - win_r)) / (win_r * 2.0)).clamp(0.0, 1.0);
                let v_norm = ((y - fy) / (wy_top - fy)).clamp(0.0, 1.0);
                let diag = (1.0 - u_norm) * 0.55 + v_norm * 0.45;
                [0.10 + 0.38 * diag, 0.22 + 0.50 * diag, 0.48 + 0.46 * diag, 1.0]
            };

            let g_base = windows.positions.len() as u32;
            let p_bl = [wx - win_r, fy, z_glass];
            let p_br = [wx + win_r, fy, z_glass];
            let p_tr = [wx + win_r, wy_spring, z_glass];
            let p_tl = [wx - win_r, wy_spring, z_glass];

            for p in [p_bl, p_br, p_tr, p_tl] {
                windows.positions.push(p);
                windows.normals.push([0.0, 0.0, 1.0]);
                windows.colors.push(glass_col_at(p[0], p[1]));
            }
            windows.uvs.extend_from_slice(&[[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
            windows.indices.extend_from_slice(&[g_base, g_base + 1, g_base + 2, g_base, g_base + 2, g_base + 3]);

            let c_center = [wx, wy_spring, z_glass];
            let c_idx = windows.positions.len() as u32;
            windows.positions.push(c_center);
            windows.normals.push([0.0, 0.0, 1.0]);
            windows.colors.push(glass_col_at(wx, wy_spring));
            windows.uvs.push([0.5, 0.5]);

            let n_fan = 12;
            for k in 0..=n_fan {
                let t = (k as f32 / n_fan as f32) * PI;
                let gx = wx - win_r * t.cos();
                let gy = wy_spring + win_r * t.sin();

                windows.positions.push([gx, gy, z_glass]);
                windows.normals.push([0.0, 0.0, 1.0]);
                windows.colors.push(glass_col_at(gx, gy));
                windows.uvs.push([(gx - (wx - win_r)) / (win_r * 2.0), (gy - fy) / (wy_top - fy)]);
            }
            for k in 0..n_fan {
                let p0 = c_idx + 1 + k;
                let p1 = c_idx + 2 + k;
                windows.indices.extend_from_slice(&[c_idx, p1, p0]);
            }

            let t_bar = 0.035f32;
            push_box(trim, [wx - t_bar * 0.5, fy, 13.055], [wx + t_bar * 0.5, wy_top - 0.02, 13.085], iron_bar_c, 1.0);
            let y_cross = fy + (wy_spring - fy) * 0.58;
            push_box(trim, [wx - win_r, y_cross - t_bar * 0.5, 13.055], [wx + win_r, y_cross + t_bar * 0.5, 13.085], iron_bar_c, 1.0);
            push_box(trim, [wx - 0.035, y_cross - 0.035, 13.085], [wx + 0.035, y_cross + 0.035, 13.10], [0.08, 0.08, 0.09, 1.0], 1.0);
        }
    }

    // Barbacane e mura sud
    push_box(wall, [-9.5, -1.4, 36.0], [-4.5, 17.5, 43.0], stone, 4.0);
    push_pyr(roof, [-9.2, 36.3], [-4.8, 42.7], 17.5, [-7.0, 23.5, 39.5], roof_c, 3.0);
    push_box(wall, [4.5, -1.4, 36.0], [9.5, 17.5, 43.0], stone, 4.0);
    push_pyr(roof, [4.8, 36.3], [9.2, 42.7], 17.5, [7.0, 23.5, 39.5], roof_c, 3.0);
    push_box(wall, [-4.5, 7.5, 37.5], [4.5, 15.3, 41.5], stone, 3.0);

    push_box(wall, [-4.5, 15.15, 37.0], [4.5, 16.0, 37.6], stone, 2.0);
    for mx in [-4.5f32, -1.3, 1.9] {
        push_box(wall, [mx, 15.95, 37.0], [mx + 1.3, 17.2, 37.6], stone, 2.0);
        push_box(wall, [mx - 0.05, 17.15, 36.95], [mx + 1.35, 17.3, 37.65], stone, 1.5);
    }

    push_box(wall, [-8.5, 0.0, 25.0], [8.5, y_ground_paved, 39.0], stone, 3.0);
    push_box(wall, [-6.5, 0.0, 36.0], [6.5, y_ground_paved, 43.0], stone, 3.0);
    push_box(wall, [-6.0, -0.4, 39.0], [6.0, y_ground_paved, 52.0], stone, 3.0);
    push_box(wall, [-6.6, y_ground_paved, 39.0], [-5.8, y_ground_paved + 1.3, 52.0], stone, 2.0);
    push_box(wall, [5.8, y_ground_paved, 39.0], [6.6, y_ground_paved + 1.3, 52.0], stone, 2.0);

    let z_road_start = 52.0f32;
    let z_road_end = 600.0f32;
    let curb_w = 0.12f32;
    let y_curb = y_ground_paved + 0.05;

    push_box(wall, [-6.0, 0.0, z_road_start], [6.0, y_ground_paved, z_road_end], stone, 3.0);
    push_box(wall, [-6.0 - curb_w, 0.0, z_road_start], [-6.0, y_curb, z_road_end], stone, 1.5);
    push_box(wall, [6.0, 0.0, z_road_start], [6.0 + curb_w, y_curb, z_road_end], stone, 1.5);

    // Maschio circolare e fossato
    let r_wall_in = 37.5;
    let r_wall_out = 40.5;
    let r_moat_out = 48.0;
    let wall_h = 11.0;
    let water_y = -0.4;
    let center_ins = [0.0, 5.0, 0.0];

    let r_merl_out = r_wall_out - 0.10;
    let r_merl_in = r_merl_out - 0.65;
    let y_base = wall_h - 0.40;
    let y_walk = wall_h;
    let y_parapet = wall_h + 0.85;
    let y_merl = wall_h + 1.85;
    let y_cap = y_merl + 0.12;

    let gate_alpha = (9.5f32 / r_wall_out).asin();
    let segs = 56;
    let total_arc = 2.0 * PI - (2.0 * gate_alpha);

    for i in 0..segs {
        let a0 = gate_alpha + (i as f32 / segs as f32) * total_arc;
        let a1 = gate_alpha + ((i + 1) as f32 / segs as f32) * total_arc;
        let am = a0 + (a1 - a0) * 0.65;

        let s0 = a0.sin();
        let c0 = a0.cos();
        let sm = am.sin();
        let cm = am.cos();
        let s1 = a1.sin();
        let c1 = a1.cos();

        let wp = [
            [r_wall_out * s0, water_y, r_wall_out * c0],
            [r_moat_out * s0, water_y, r_moat_out * c0],
            [r_moat_out * s1, water_y, r_moat_out * c1],
            [r_wall_out * s1, water_y, r_wall_out * c1],
        ];
        push_face(trim, &wp, water_c, 4.0, [0.0, water_y - 1.0, 0.0]);

        let wf_out = [
            [r_wall_out * s0, -1.2, r_wall_out * c0],
            [r_wall_out * s1, -1.2, r_wall_out * c1],
            [r_wall_out * s1, wall_h, r_wall_out * c1],
            [r_wall_out * s0, wall_h, r_wall_out * c0],
        ];
        push_face(wall, &wf_out, stone, 4.0, center_ins);

        let ledge = [
            [r_merl_out * s0, wall_h, r_merl_out * c0],
            [r_wall_out * s0, wall_h, r_wall_out * c0],
            [r_wall_out * s1, wall_h, r_wall_out * c1],
            [r_merl_out * s1, wall_h, r_merl_out * c1],
        ];
        push_face(wall, &ledge, stone, 2.0, [0.0, wall_h - 0.5, 0.0]);

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
            [r_merl_in * s0, wall_h, r_merl_in * c0],
            [r_merl_in * s1, wall_h, r_merl_in * c1],
            [r_wall_in * s1, wall_h, r_wall_in * c1],
        ];
        push_face(wall, &walk, stone, 3.0, [0.0, wall_h - 1.0, 0.0]);

        let r_rail = r_wall_in + 0.35;
        let rail_ins = [r_wall_in * mid_s, y_walk + 0.45, r_wall_in * mid_c];
        push_face(wall, &[
            [r_rail * s0, y_base, r_rail * c0],
            [r_rail * s0, y_walk + 0.9, r_rail * c0],
            [r_rail * s1, y_walk + 0.9, r_rail * c1],
            [r_rail * s1, y_base, r_rail * c1],
        ], stone, 2.0, rail_ins);
        push_face(wall, &[
            [r_wall_in * s0, y_walk + 0.9, r_wall_in * c0],
            [r_rail * s0, y_walk + 0.9, r_rail * c0],
            [r_rail * s1, y_walk + 0.9, r_rail * c1],
            [r_wall_in * s1, y_walk + 0.9, r_rail * c1],
        ], stone, 2.0, [0.0, y_walk, 0.0]);

        let para_ins = [r_merl_out * mid_s, (y_base + y_parapet) * 0.5, r_merl_out * mid_c];
        push_face(wall, &[
            [r_merl_out * s0, y_base, r_merl_out * c0],
            [r_merl_out * s1, y_base, r_merl_out * c1],
            [r_merl_out * s1, y_parapet, r_merl_out * c1],
            [r_merl_out * s0, y_parapet, r_merl_out * c0],
        ], stone, 2.0, center_ins);

        push_face(wall, &[
            [r_merl_in * s1, y_base, r_merl_in * c1],
            [r_merl_in * s0, y_base, r_merl_in * c0],
            [r_merl_in * s0, y_parapet, r_merl_in * c0],
            [r_merl_in * s1, y_parapet, r_merl_in * c1],
        ], stone, 2.0, para_ins);

        push_face(wall, &[
            [r_merl_in * sm, y_parapet, r_merl_in * cm],
            [r_merl_out * sm, y_parapet, r_merl_out * cm],
            [r_merl_out * s1, y_parapet, r_merl_out * c1],
            [r_merl_in * s1, y_parapet, r_merl_in * c1],
        ], stone, 2.0, [0.0, y_parapet - 0.5, 0.0]);

        let y_merl_start = y_parapet - 0.08;
        let a_mid = (a0 + am) * 0.5;
        let r_mid = (r_merl_in + r_merl_out) * 0.5;
        let m_ins = [r_mid * a_mid.sin(), (y_merl_start + y_merl) * 0.5, r_mid * a_mid.cos()];

        push_face(wall, &[
            [r_merl_out * s0, y_merl_start, r_merl_out * c0],
            [r_merl_out * sm, y_merl_start, r_merl_out * cm],
            [r_merl_out * sm, y_merl, r_merl_out * cm],
            [r_merl_out * s0, y_merl, r_merl_out * c0],
        ], stone, 2.0, m_ins);

        push_face(wall, &[
            [r_merl_in * sm, y_merl_start, r_merl_in * cm],
            [r_merl_in * s0, y_merl_start, r_merl_in * c0],
            [r_merl_in * s0, y_merl, r_merl_in * c0],
            [r_merl_in * sm, y_merl, r_merl_in * cm],
        ], stone, 2.0, m_ins);

        push_face(wall, &[
            [r_merl_in * s0, y_merl_start, r_merl_in * c0],
            [r_merl_out * s0, y_merl_start, r_merl_out * c0],
            [r_merl_out * s0, y_merl, r_merl_out * c0],
            [r_merl_in * s0, y_merl, r_merl_in * c0],
        ], stone, 2.0, m_ins);

        push_face(wall, &[
            [r_merl_out * sm, y_merl_start, r_merl_out * cm],
            [r_merl_in * sm, y_merl_start, r_merl_in * cm],
            [r_merl_in * sm, y_merl, r_merl_in * cm],
            [r_merl_out * sm, y_merl, r_merl_out * cm],
        ], stone, 2.0, m_ins);

        let r_cap_in = r_merl_in - 0.04;
        let r_cap_out = r_merl_out + 0.04;
        push_face(wall, &[
            [r_cap_in * s0, y_cap, r_cap_in * c0],
            [r_cap_out * s0, y_cap, r_cap_out * c0],
            [r_cap_out * sm, y_cap, r_cap_out * cm],
            [r_cap_in * sm, y_cap, r_cap_in * cm],
        ], stone, 2.0, m_ins);
    }

    for (bx, bz) in [(0.0, -39.0), (39.0, 0.0), (-39.0, 0.0)] {
        push_box(wall, [bx - 4.5, -0.5, bz - 4.5], [bx + 4.5, 17.0, bz + 4.5], stone, 4.0);
        push_box(trim, [bx - 5.0, 16.2, bz - 5.0], [bx + 5.0, 17.0, bz + 5.0], trim_c, 3.0);
        push_pyr(roof, [bx - 4.6, bz - 4.6], [bx + 4.6, bz + 4.6], 17.0, [bx, 24.5, bz], roof_c, 3.0);
    }

    mesh
}

pub fn decode_face_texture(archive: u16, record: u8) -> (u16, u8) {
    (archive, record)
}
