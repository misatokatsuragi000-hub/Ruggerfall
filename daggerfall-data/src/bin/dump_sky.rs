//! Anteprima del cielo in PNG: una vista a terra (90 gradi di campo, rivolta
//! verso il sole o l'orizzonte ovest) per diverse ore del giorno, con sole,
//! luna e stelle ma senza nuvole. Non usa la GPU.
//!
//! Uso: dump_sky file.png

use daggerfall_data::sky::{generate_stars, lighting, sky_color, sky_state};
use daggerfall_data::png;

const HOURS: [f32; 12] = [0.0, 4.5, 5.5, 6.0, 7.0, 9.0, 12.0, 15.0, 17.5, 18.0, 19.0, 21.0];
const COLS: usize = 4;
const W: usize = 240;
const H: usize = 150;

fn to_srgb(c: f32) -> u8 {
    // tonemap semplice + gamma
    let t = 1.0 - (-c * 1.3).exp();
    (t.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0) as u8
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / l, v[1] / l, v[2] / l]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("uso: dump_sky file.png");
        std::process::exit(1)
    });
    let rows = HOURS.len().div_ceil(COLS);
    let (sw, sh) = (COLS * W, rows * H);
    let mut rgba = vec![255u8; sw * sh * 4];
    let stars = generate_stars(1800, 20_240_607, 1.0);

    for (i, &hour) in HOURS.iter().enumerate() {
        let st = sky_state(hour);
        let lit = lighting(&st);
        let (ox, oy) = ((i % COLS) * W, (i / COLS) * H);

        // Guarda verso il sole in azimut (o a ovest se è sotto l'orizzonte), con 15 gradi di inclinazione.
        let az = if st.sun[1] > -0.05 { [st.sun[0], 0.0, st.sun[2]] } else { [-1.0, 0.0, 0.0] };
        let az = norm(az);
        let pitch = 0.26f32;
        let f = norm([az[0] * pitch.cos(), pitch.sin(), az[2] * pitch.cos()]);
        let r = norm(cross(f, [0.0, 1.0, 0.0]));
        let u = cross(r, f);

        for py in 0..H {
            for px in 0..W {
                let sx = (px as f32 + 0.5) / W as f32 * 2.0 - 1.0;
                let sy = 1.0 - (py as f32 + 0.5) / H as f32 * 2.0;
                let aspect = H as f32 / W as f32;
                let d = norm([
                    f[0] + r[0] * sx + u[0] * sy * aspect,
                    f[1] + r[1] * sx + u[1] * sy * aspect,
                    f[2] + r[2] * sx + u[2] * sy * aspect,
                ]);
                let mut c = if d[1] < -0.01 {
                    // terreno di riferimento
                    let k = 0.10 + 0.9 * st.day.max(st.twilight * 0.5);
                    [0.10 * k, 0.14 * k, 0.06 * k]
                } else {
                    sky_color(d, &st)
                };
                // sole e luna
                if d[1] >= -0.01 {
                    if dot(d, st.sun) > 0.9997 {
                        c = [4.0, 3.6, 2.8];
                    }
                    if dot(d, st.moon) > 0.9998 {
                        c = [1.2, 1.25, 1.4];
                    }
                }
                let p = ((oy + py) * sw + ox + px) * 4;
                rgba[p] = to_srgb(c[0]);
                rgba[p + 1] = to_srgb(c[1]);
                rgba[p + 2] = to_srgb(c[2]);
            }
        }

        // Stelle: un punto per ognuna, se davanti alla camera e sopra l'orizzonte.
        if lit.star_alpha > 0.01 {
            for quad in stars.positions.chunks(4) {
                let s = norm(quad[0]);
                if s[1] < 0.0 {
                    continue;
                }
                let depth = dot(s, f);
                if depth <= 0.05 {
                    continue;
                }
                let sx = dot(s, r) / depth;
                let sy = dot(s, u) / depth / (H as f32 / W as f32);
                if sx.abs() > 1.0 || sy.abs() > 1.0 {
                    continue;
                }
                let px = ((sx * 0.5 + 0.5) * W as f32) as usize;
                let py = ((1.0 - (sy * 0.5 + 0.5)) * H as f32) as usize;
                let p = ((oy + py.min(H - 1)) * sw + ox + px.min(W - 1)) * 4;
                let v = (lit.star_alpha * 255.0) as u32;
                for c in 0..3 {
                    rgba[p + c] = (rgba[p + c] as u32).max(v) as u8;
                }
            }
        }
    }
    std::fs::write(&path, png::encode_rgba(sw as u32, sh as u32, &rgba)).unwrap();
    println!("anteprima salvata in {path} (ore: {HOURS:?})");
}
