// Spettro direzionale TMA/JONSWAP (Horvath) + spread di Hasselmann con swell,
// evoluto nel tempo e "impacchettato" per l'FFT inversa (come GodotOceanWaves).
//
// Griglia spettrale centrata: l'indice N/2 e' k = 0. Il fattore (-1)^(x+y) che
// sposta il risultato nel dominio spaziale e' applicato in ocean_unpack.wgsl.
//
// Per ogni cascata si producono 4 campi complessi (ognuno un'FFT, il cui reale e
// l'immaginario sono due campi reali distinti: a + i*b):
//   f0 = H            + i * Dx
//   f1 = Dz           + i * dh/dx
//   f2 = dh/dz        + i * dDx/dx
//   f3 = dDz/dz       + i * dDx/dz
// con H altezza, D = i*(k/|k|)*H spostamento orizzontale (choppy).

struct SpectrumParams {
    // x = lato del tile (m), y = k_min, z = k_max (rad/m), w = non usato
    cascades: array<vec4<f32>, 4>,
    alpha: f32,
    peak_frequency: f32,
    wind_speed: f32,
    angle: f32,
    depth: f32,
    swell: f32,
    detail: f32,
    spread: f32,
    time: f32,
    num_cascades: u32,
};

@group(0) @binding(0) var<uniform> p: SpectrumParams;
@group(0) @binding(1) var<storage, read_write> fft_buf: array<vec2<f32>>;

const PI: f32 = 3.14159265;
const G: f32 = 9.81;
const LN2: f32 = 0.69314718;
const GAMMA: f32 = 3.3;
const N: u32 = 256u;

fn pcg2d(v_in: vec2<u32>) -> vec2<u32> {
    var v = v_in * 1664525u + 1013904223u;
    v.x += v.y * 1664525u;
    v.y += v.x * 1664525u;
    v = v ^ (v >> vec2<u32>(16u));
    v.x += v.y * 1664525u;
    v.y += v.x * 1664525u;
    v = v ^ (v >> vec2<u32>(16u));
    return v;
}

// Due numeri casuali in (0, 1) indipendenti per ogni (cella, cascata).
fn rand2(id: vec2<i32>, c: u32) -> vec2<f32> {
    let v = pcg2d(vec2<u32>(u32(id.x) + c * 4099u, u32(id.y) + c * 9157u + 17u));
    let f = vec2<f32>(v >> vec2<u32>(8u)) / 16777216.0;
    return vec2<f32>(max(f.x, 1e-7), f.y);
}

// Gaussiana complessa: le due componenti sono N(0, 1).
fn gauss(u: vec2<f32>) -> vec2<f32> {
    let r = sqrt(-2.0 * log(u.x));
    let t = 2.0 * PI * u.y;
    return vec2<f32>(r * cos(t), r * sin(t));
}

fn cmul(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

// i * a
fn cmul_i(a: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(-a.y, a.x);
}

fn disp_rel(k: f32) -> f32 {
    return sqrt(G * k * tanh(min(k * p.depth, 20.0)));
}

fn disp_deriv(k: f32) -> f32 {
    let kd = min(k * p.depth, 20.0);
    let ch = cosh(kd);
    return G * (p.depth * k / (ch * ch) + tanh(kd)) / (2.0 * disp_rel(k));
}

// JONSWAP
fn jonswap(w: f32) -> f32 {
    let wp = p.peak_frequency;
    let sigma = select(0.09, 0.07, w <= wp);
    let r = exp(-(w - wp) * (w - wp) / (2.0 * sigma * sigma * wp * wp));
    return p.alpha * G * G / pow(w, 5.0) * exp(-1.25 * pow(wp / w, 4.0)) * pow(GAMMA, r);
}

// Attenuazione di profondita' (TMA)
fn tma(w: f32) -> f32 {
    let wh = w * sqrt(p.depth / G);
    if (wh <= 1.0) { return 0.5 * wh * wh; }
    if (wh < 2.0) { return 1.0 - 0.5 * (2.0 - wh) * (2.0 - wh); }
    return 1.0;
}

// Fattore di normalizzazione Q(s) di |cos(theta/2)|^(2s) (approssimazione).
fn hasselmann_q(s: f32) -> f32 {
    if (s <= 0.4) {
        return 0.09 * s * s * s + (LN2 * LN2 / PI - PI / 12.0) * s * s + (LN2 / PI) * s + 0.5 / PI;
    }
    return sqrt(s) / (2.0 * sqrt(PI)) + 1.0 / (16.0 * sqrt(PI * s));
}

// Spread direzionale misto: piatto (1/2pi) <-> Hasselmann, con swell.
// `theta` e' l'angolo rispetto alla direzione di propagazione (0 = nella direzione del vento).
fn directional(w: f32, theta: f32) -> f32 {
    let wp = p.peak_frequency;
    let r = w / wp;
    var s: f32;
    if (w <= wp) {
        s = 6.97 * pow(r, 4.06);
    } else {
        s = 9.77 * pow(r, -2.33 - 1.45 * (wp * p.wind_speed / G - 1.17));
    }
    s = s + 16.0 * tanh(wp / w) * p.swell * p.swell;
    let d = hasselmann_q(s) * pow(abs(cos(theta * 0.5)), 2.0 * s);
    return mix(0.5 / PI, d, p.spread);
}

// Spettro iniziale h0(k) per la cella `id` della cascata `c`.
fn h0(id: vec2<i32>, c: u32) -> vec2<f32> {
    let info = p.cascades[c];
    let dk = 2.0 * PI / info.x;
    let kvec = (vec2<f32>(id) - vec2<f32>(f32(N) * 0.5)) * dk;
    let k = length(kvec);
    if (k < 1e-4) { return vec2<f32>(0.0); }

    let w = disp_rel(k);
    // h0(k) descrive onde con vettore d'onda k, che si propagano verso -k (e^{i(kx+wt)}):
    // si valuta quindi lo spread nella direzione opposta a quella del vento.
    let t = atan2(kvec.y, kvec.x) - p.angle - PI;
    let theta = atan2(sin(t), cos(t));

    // Banda di numeri d'onda della cascata (somma delle energie = 1 nelle sovrapposizioni).
    var band = 1.0 - smoothstep(0.85 * info.z, info.z, k);
    if (info.y > 0.0) {
        band = band * smoothstep(0.85 * info.y, info.y, k);
    }

    let s = jonswap(w) * tma(w);
    let fade = exp(-(1.0 - p.detail) * (1.0 - p.detail) * k * k);
    // P = 0.5 * S(w) D(theta) |dw/dk| / k * dk^2 ; h0 = gauss * sqrt(P / 2)
    let amp = 0.5 * sqrt(s * directional(w, theta) * disp_deriv(k) / k * band * fade) * dk;
    return gauss(rand2(id, c)) * amp;
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let c = gid.z;
    if (gid.x >= N || gid.y >= N || c >= p.num_cascades) { return; }

    let id = vec2<i32>(gid.xy);
    let idn = (vec2<i32>(i32(N)) - id) % vec2<i32>(i32(N)); // indice di -k
    let info = p.cascades[c];
    let dk = 2.0 * PI / info.x;
    let kvec = (vec2<f32>(id) - vec2<f32>(f32(N) * 0.5)) * dk;
    let k = length(kvec);

    let base = c * 4u * N * N;
    let idx = gid.y * N + gid.x;

    if (k < 1e-4) {
        for (var f: u32 = 0u; f < 4u; f = f + 1u) {
            fft_buf[base + f * N * N + idx] = vec2<f32>(0.0);
        }
        return;
    }

    let omega = disp_rel(k);
    let e = vec2<f32>(cos(omega * p.time), sin(omega * p.time));
    let hk = h0(id, c);
    let hkn = h0(idn, c);
    // H(k,t) = h0(k) e^{iwt} + conj(h0(-k)) e^{-iwt}   (hermitiano -> campo reale)
    let hh = cmul(hk, e) + cmul(vec2<f32>(hkn.x, -hkn.y), vec2<f32>(e.x, -e.y));

    let kn = kvec / k;
    let ih = cmul_i(hh);

    let dx = ih * kn.x;                  //  i kx/|k| H
    let dz = ih * kn.y;                  //  i kz/|k| H
    let dhdx = ih * kvec.x;              //  i kx H
    let dhdz = ih * kvec.y;              //  i kz H
    let ddxdx = -hh * (kvec.x * kn.x);   // -kx^2/|k| H
    let ddzdz = -hh * (kvec.y * kn.y);   // -kz^2/|k| H
    let ddxdz = -hh * (kvec.x * kn.y);   // -kx kz/|k| H

    fft_buf[base + 0u * N * N + idx] = hh + cmul_i(dx);
    fft_buf[base + 1u * N * N + idx] = dz + cmul_i(dhdx);
    fft_buf[base + 2u * N * N + idx] = dhdz + cmul_i(ddxdx);
    fft_buf[base + 3u * N * N + idx] = ddzdz + cmul_i(ddxdz);
}
