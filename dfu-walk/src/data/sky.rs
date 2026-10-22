//! Cielo procedurale: posizione di sole e luna, colori della cupola, luci,
//! stelle e nuvole. Nessuna dipendenza da Bevy: restituisce numeri e buffer.
//!
//! Coordinate come nella mesh del terreno (pensate per Bevy): X = est, Y = su,
//! Z = sud. Tutti i colori sono RGB lineari.

use std::f32::consts::PI;

/// Inclinazione verso sud dell'arco del sole a mezzogiorno (radianti).
pub const SUN_TILT: f32 = 0.45;

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Colore sRGB (0..1) -> lineare.
pub fn lin(c: [f32; 3]) -> [f32; 3] {
    [srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2])]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Direzione (unitaria) verso il sole all'ora indicata (0..24): sorge a est,
/// passa a sud, tramonta a ovest.
pub fn sun_direction(hours: f32) -> [f32; 3] {
    let ha = (hours - 12.0) / 12.0 * PI;
    let (s, c) = ha.sin_cos();
    [-s, c * SUN_TILT.cos(), c * SUN_TILT.sin()]
}

#[derive(Clone, Copy, Debug)]
pub struct SkyState {
    pub sun: [f32; 3],
    pub moon: [f32; 3],
    /// Quota del sole sull'orizzonte (-1..1).
    pub elevation: f32,
    /// 0 = notte piena, 1 = giorno pieno.
    pub day: f32,
    /// 1 all'alba e al tramonto, 0 lontano da essi.
    pub twilight: f32,
    /// 1 = notte piena.
    pub night: f32,
    pub zenith: [f32; 3],
    pub horizon: [f32; 3],
}

pub fn sky_state(hours: f32) -> SkyState {
    let sun = sun_direction(hours);
    let moon = [-sun[0], -sun[1], -sun[2]];
    let e = sun[1];
    let day = smoothstep(-0.08, 0.30, e);
    let night = 1.0 - smoothstep(-0.18, 0.0, e);
    let twilight = (1.0 - e.abs() / 0.28).clamp(0.0, 1.0).powf(1.5);

    let night_z = lin([0.01, 0.015, 0.05]);
    let day_z = lin([0.15, 0.38, 0.80]);
    let twilight_z = lin([0.17, 0.20, 0.45]);
    let night_h = lin([0.03, 0.04, 0.09]);
    let day_h = lin([0.62, 0.78, 0.93]);
    let twilight_h = lin([0.95, 0.50, 0.28]);

    let zenith = mix(mix(night_z, day_z, day), twilight_z, twilight * 0.5);
    let horizon = mix(mix(night_h, day_h, day), twilight_h, twilight * 0.65);
    SkyState {
        sun,
        moon,
        elevation: e,
        day,
        twilight,
        night,
        zenith,
        horizon,
    }
}

/// Colore del cielo nella direzione `dir` (unitaria).
pub fn sky_color(dir: [f32; 3], st: &SkyState) -> [f32; 3] {
    let t = dir[1].max(0.0);
    let h = (1.0 - t).powf(5.0);
    let mut c = mix(st.zenith, st.horizon, h);
    if dir[1] < 0.0 {
        c = [st.horizon[0] * 0.8, st.horizon[1] * 0.8, st.horizon[2] * 0.8];
    }

    // Alone attorno al sole, caldo al tramonto.
    let g = dot(dir, st.sun).max(0.0);
    let strength = (g.powf(6.0) * 0.25 + g.powf(60.0) * 0.9) * st.day.max(st.twilight);
    let glow = mix(lin([1.0, 0.9, 0.7]), lin([1.0, 0.55, 0.25]), st.twilight);
    // Alone tenue attorno alla luna.
    let gm = dot(dir, st.moon).max(0.0);
    let moon_strength = gm.powf(40.0) * 0.15 * st.night;
    let moon_glow = lin([0.6, 0.7, 1.0]);
    for i in 0..3 {
        c[i] += glow[i] * strength + moon_glow[i] * moon_strength;
    }
    c
}

#[derive(Clone, Copy, Debug)]
pub struct Lighting {
    /// Direzione verso la sorgente di luce (sole o luna).
    pub light_from: [f32; 3],
    pub light_color: [f32; 3],
    pub illuminance: f32,
    pub ambient_color: [f32; 3],
    pub ambient_brightness: f32,
    pub fog_color: [f32; 3],
    pub cloud_tint: [f32; 3],
    pub star_alpha: f32,
}

pub fn lighting(st: &SkyState) -> Lighting {
    let e = st.elevation;
    let (light_from, light_color, illuminance) = if e >= 0.0 {
        let color = mix(lin([1.0, 0.5, 0.25]), lin([1.0, 0.96, 0.88]), smoothstep(0.02, 0.4, e));
        (st.sun, color, 11_000.0 * smoothstep(0.0, 0.22, e))
    } else {
        (st.moon, lin([0.55, 0.65, 1.0]), 900.0 * smoothstep(0.0, 0.2, -e))
    };

    let ambient_color = mix(
        mix(lin([0.30, 0.38, 0.75]), lin([0.85, 0.90, 1.0]), st.day),
        lin([1.0, 0.7, 0.55]),
        st.twilight * 0.4,
    );
    let cloud_tint = mix(
        mix(lin([0.05, 0.06, 0.11]), lin([0.95, 0.95, 0.95]), st.day),
        lin([1.0, 0.62, 0.45]),
        st.twilight * 0.7,
    );
    Lighting {
        light_from,
        light_color,
        illuminance,
        ambient_color,
        ambient_brightness: 40.0 + 410.0 * st.day,
        fog_color: st.horizon,
        cloud_tint,
        star_alpha: st.night * st.night,
    }
}

fn next_rand(state: &mut u32) -> f32 {
    // xorshift32
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state >> 8) as f32 / (1u32 << 24) as f32
}

#[derive(Default, Clone, Debug)]
pub struct QuadMesh {
    pub positions: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

/// Stelle come piccoli quadrati sulla sfera di raggio `radius` (solo sopra l'orizzonte).
pub fn generate_stars(count: usize, seed: u32, radius: f32) -> QuadMesh {
    let mut state = seed.max(1);
    let mut mesh = QuadMesh::default();
    while mesh.positions.len() < count * 4 {
        let y = next_rand(&mut state) * 2.0 - 1.0;
        let phi = next_rand(&mut state) * 2.0 * PI;
        let brightness_roll = next_rand(&mut state);
        if y < -0.1 {
            continue;
        }
        let r = (1.0 - y * y).max(0.0).sqrt();
        let d = [r * phi.cos(), y, r * phi.sin()];

        let b = 0.35 + 0.65 * brightness_roll.powi(3);
        let size = radius * 0.0016 * (0.7 + b);
        // Base ortonormale del piano tangente.
        let axis = if d[1].abs() < 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
        let t1 = normalize(cross(d, axis));
        let t2 = cross(d, t1);

        let base = mesh.positions.len() as u32;
        for (a, b2) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            mesh.positions.push([
                d[0] * radius + (t1[0] * a + t2[0] * b2) * size,
                d[1] * radius + (t1[1] * a + t2[1] * b2) * size,
                d[2] * radius + (t1[2] * a + t2[2] * b2) * size,
            ]);
            mesh.colors.push([b, b * 0.95, b * 0.85, b]);
        }
        mesh.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    mesh
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = dot(v, v).sqrt().max(1e-9);
    [v[0] / l, v[1] / l, v[2] / l]
}

fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ seed.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    h as f32 / u32::MAX as f32
}

/// Rumore di valore periodico di periodo `period` celle: il bordo destro si
/// ricollega a quello sinistro, quindi la texture si ripete senza giunture.
fn periodic_noise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let (ix, iy) = (x0 as i32, y0 as i32);
    let at = |dx: i32, dy: i32| hash2((ix + dx).rem_euclid(period), (iy + dy).rem_euclid(period), seed);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (s(fx), s(fy));
    let top = at(0, 0) + (at(1, 0) - at(0, 0)) * sx;
    let bottom = at(0, 1) + (at(1, 1) - at(0, 1)) * sx;
    top + (bottom - top) * sy
}

fn fbm(u: f32, v: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 0.5, 0.0);
    for octave in 0..6u32 {
        let period = 4i32 << octave;
        sum += amp * periodic_noise(u * period as f32, v * period as f32, period, seed + octave);
        norm += amp;
        amp *= 0.5;
    }
    sum / norm
}

/// Texture RGBA8 `size` x `size` di nuvole, ripetibile in entrambe le direzioni.
pub fn generate_cloud_texture(size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let (u, v) = (x as f32 / size as f32, y as f32 / size as f32);
            let density = smoothstep(0.48, 0.72, fbm(u, v, 11));
            let shade = fbm(u, v, 97);
            let lum = 1.0 - 0.25 * density * shade;
            let c = (lum * 255.0) as u8;
            out.extend_from_slice(&[c, c, c, (density * 0.95 * 255.0) as u8]);
        }
    }
    out
}

/// Catena di mipmap RGBA8 con filtro a scatola; `levels` include il livello 0.
pub fn mip_chain_rgba(base: &[u8], width: usize, height: usize, levels: usize) -> Vec<Vec<u8>> {
    let mut chain = vec![base.to_vec()];
    let (mut w, mut h) = (width, height);
    while chain.len() < levels && w > 1 && h > 1 {
        let prev = chain.last().unwrap();
        let (nw, nh) = (w / 2, h / 2);
        let mut next = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let at = |dx: usize, dy: usize| prev[((y * 2 + dy) * w + x * 2 + dx) * 4 + c] as u32;
                    next[(y * nw + x) * 4 + c] = ((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8;
                }
            }
        }
        chain.push(next);
        w = nw;
        h = nh;
    }
    chain
}

#[derive(Default, Clone, Debug)]
pub struct GridMesh {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

/// Piano orizzontale di lato `2 * radius` con `cells` x `cells` celle. Le UV
/// sono in unità di `tile_size` metri; l'alfa scende a 0 verso il bordo.
pub fn cloud_grid(radius: f32, cells: usize, tile_size: f32) -> GridMesh {
    let mut m = GridMesh::default();
    let n = cells + 1;
    for j in 0..n {
        for i in 0..n {
            let x = (i as f32 / cells as f32 * 2.0 - 1.0) * radius;
            let z = (j as f32 / cells as f32 * 2.0 - 1.0) * radius;
            let r = (x * x + z * z).sqrt() / radius;
            let alpha = 1.0 - smoothstep(0.55, 1.0, r);
            m.positions.push([x, 0.0, z]);
            m.uvs.push([x / tile_size, z / tile_size]);
            m.colors.push([1.0, 1.0, 1.0, alpha]);
        }
    }
    for j in 0..cells {
        for i in 0..cells {
            let a = (j * n + i) as u32;
            let (b, c, d) = (a + 1, a + n as u32, a + n as u32 + 1);
            m.indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn len(v: [f32; 3]) -> f32 {
        dot(v, v).sqrt()
    }

    #[test]
    fn sun_path_east_south_west() {
        for h in [0.0, 5.0, 9.5, 12.0, 15.0, 20.0, 23.5] {
            assert!((len(sun_direction(h)) - 1.0).abs() < 1e-5);
        }
        let noon = sun_direction(12.0);
        assert!(noon[1] > 0.85 && noon[2] > 0.3, "{noon:?}"); // alto e verso sud (+z)
        assert!(sun_direction(6.0)[0] > 0.9); // alba a est
        assert!(sun_direction(18.0)[0] < -0.9); // tramonto a ovest
        assert!(sun_direction(0.0)[1] < -0.8); // mezzanotte sotto l'orizzonte
    }

    #[test]
    fn day_twilight_night_factors() {
        let noon = sky_state(12.0);
        assert!(noon.day > 0.99 && noon.night < 0.01 && noon.twilight < 0.05);
        let midnight = sky_state(0.0);
        assert!(midnight.day < 0.01 && midnight.night > 0.99 && midnight.twilight < 0.05);
        let dawn = sky_state(6.0);
        assert!(dawn.twilight > 0.9, "{}", dawn.twilight);
        assert!(sky_state(18.0).twilight > 0.9);
    }

    #[test]
    fn sky_colors_make_sense() {
        let noon = sky_state(12.0);
        let up = sky_color([0.0, 1.0, 0.0], &noon);
        let low = sky_color([1.0, 0.02, 0.0], &noon);
        assert!(up[2] > up[0] * 2.0, "lo zenith è blu: {up:?}");
        assert!(low[0] + low[1] + low[2] > up[0] + up[1] + up[2], "l'orizzonte è più chiaro");

        let night = sky_state(0.0);
        let n = sky_color([0.0, 1.0, 0.0], &night);
        assert!(n[0] + n[1] + n[2] < 0.05, "{n:?}");

        // al tramonto verso il sole il cielo è caldo (rosso > blu)
        let dusk = sky_state(18.0);
        let toward_sun = sky_color(normalize([dusk.sun[0], dusk.sun[1].max(0.02), dusk.sun[2]]), &dusk);
        assert!(toward_sun[0] > toward_sun[2] * 1.5, "{toward_sun:?}");
        // sotto l'orizzonte non è più chiaro del bordo
        let below = sky_color([0.0, -1.0, 0.0], &noon);
        assert!(below[0] <= noon.horizon[0]);
    }

    #[test]
    fn lighting_follows_sun_and_moon() {
        let noon = lighting(&sky_state(12.0));
        assert!((noon.illuminance - 11_000.0).abs() < 50.0);
        assert!(noon.light_from[1] > 0.8);
        assert!(noon.star_alpha < 0.01 && noon.ambient_brightness > 400.0);

        let night = lighting(&sky_state(0.0));
        assert!(night.illuminance > 100.0 && night.illuminance < 1000.0);
        assert!(night.light_from[1] > 0.5, "di notte la luce viene dalla luna, in alto");
        assert!(night.star_alpha > 0.98 && night.ambient_brightness < 60.0);
        assert!(night.light_color[2] > night.light_color[0], "luce lunare bluastra");

        // nessun salto di luce al sorgere del sole
        let (a, b) = (lighting(&sky_state(5.95)), lighting(&sky_state(6.05)));
        assert!((a.illuminance - b.illuminance).abs() < 800.0, "{} {}", a.illuminance, b.illuminance);
    }

    #[test]
    fn stars_are_on_the_sphere_above_the_horizon() {
        let s = generate_stars(500, 7, 1000.0);
        assert_eq!(s.positions.len(), 2000);
        assert_eq!(s.indices.len(), 3000);
        assert_eq!(s.colors.len(), 2000);
        assert!(s.indices.iter().all(|&i| (i as usize) < s.positions.len()));
        for p in s.positions.iter().step_by(4) {
            assert!((len(*p) - 1000.0).abs() < 5.0, "{}", len(*p));
            assert!(p[1] > -150.0);
        }
        // deterministico
        assert_eq!(generate_stars(10, 7, 1000.0).positions, generate_stars(10, 7, 1000.0).positions);
    }

    #[test]
    fn cloud_texture_is_varied_and_tileable() {
        let n = 128;
        let t = generate_cloud_texture(n);
        assert_eq!(t.len(), n * n * 4);
        let alpha = |x: usize, y: usize| t[(y * n + x) * 4 + 3] as f32;
        let coverage = t.chunks(4).filter(|p| p[3] > 40).count() as f32 / (n * n) as f32;
        assert!(coverage > 0.1 && coverage < 0.8, "copertura {coverage}");
        // il salto fra l'ultimo e il primo pixel è come quello fra pixel vicini
        let (mut seam, mut inner) = (0.0f32, 0.0f32);
        for y in 0..n {
            seam += (alpha(n - 1, y) - alpha(0, y)).abs();
            inner += (alpha(n / 2, y) - alpha(n / 2 + 1, y)).abs();
        }
        assert!(seam < inner * 3.0 + 50.0, "giuntura {seam} contro {inner}");
        let (mut seam_v, mut inner_v) = (0.0f32, 0.0f32);
        for x in 0..n {
            seam_v += (alpha(x, n - 1) - alpha(x, 0)).abs();
            inner_v += (alpha(x, n / 2) - alpha(x, n / 2 + 1)).abs();
        }
        assert!(seam_v < inner_v * 3.0 + 50.0, "giuntura {seam_v} contro {inner_v}");
    }

    #[test]
    fn mip_chain_sizes() {
        let base = vec![200u8; 16 * 16 * 4];
        let chain = mip_chain_rgba(&base, 16, 16, 10);
        let sizes: Vec<usize> = chain.iter().map(|l| l.len()).collect();
        assert_eq!(sizes, vec![1024, 256, 64, 16, 4]);
        assert!(chain[3].iter().all(|&v| v == 200));
        assert_eq!(mip_chain_rgba(&base, 16, 16, 2).len(), 2);
    }

    #[test]
    fn cloud_grid_fades_at_the_edge() {
        let g = cloud_grid(1000.0, 10, 500.0);
        assert_eq!(g.positions.len(), 121);
        assert_eq!(g.indices.len(), 10 * 10 * 6);
        assert_eq!(g.colors[60][3], 1.0); // centro
        assert_eq!(g.colors[0][3], 0.0); // angolo
        assert_eq!(g.uvs[120], [2.0, 2.0]);
        assert!(g.indices.iter().all(|&i| (i as usize) < g.positions.len()));
    }
}
