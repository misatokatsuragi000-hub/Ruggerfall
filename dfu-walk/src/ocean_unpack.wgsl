// Dopo l'FFT: toglie il fattore (-1)^(x+y) della griglia centrata, separa le parti
// reale/immaginaria dei 4 campi e scrive:
//   disp_map  = (Dx, h, Dz, dDx/dz)
//   norm_map  = (dh/dx, dh/dz, dDx/dx, dDz/dz)
//   foam_map  = schiuma persistente (cresce dove lo Jacobiano e' sotto soglia,
//               decade in modo esponenziale)

struct UnpackParams {
    choppiness: f32,
    foam_threshold: f32,
    foam_grow: f32,
    foam_decay: f32,
    delta_time: f32,
    num_cascades: u32,
    _pad0: f32,
    _pad1: f32,
};

@group(0) @binding(0) var<uniform> p: UnpackParams;
@group(0) @binding(1) var<storage, read> fft_buf: array<vec2<f32>>;
@group(0) @binding(2) var disp_map: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(3) var norm_map: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(4) var foam_map: texture_storage_2d_array<r32float, read_write>;

const N: u32 = 256u;

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= N || gid.y >= N || gid.z >= p.num_cascades) { return; }
    let c = gid.z;
    let sgn = 1.0 - 2.0 * f32((gid.x + gid.y) & 1u);
    let base = c * 4u * N * N;
    let idx = gid.y * N + gid.x;

    let a = fft_buf[base + 0u * N * N + idx] * sgn; // (h, Dx)
    let b = fft_buf[base + 1u * N * N + idx] * sgn; // (Dz, dh/dx)
    let d = fft_buf[base + 2u * N * N + idx] * sgn; // (dh/dz, dDx/dx)
    let e = fft_buf[base + 3u * N * N + idx] * sgn; // (dDz/dz, dDx/dz)

    let height = a.x;
    let dx = a.y;
    let dz = b.x;
    let dhdx = b.y;
    let dhdz = d.x;
    let ddxdx = d.y;
    let ddzdz = e.x;
    let ddxdz = e.y;

    let pos = vec2<i32>(gid.xy);
    textureStore(disp_map, pos, c, vec4<f32>(dx, height, dz, ddxdz));
    textureStore(norm_map, pos, c, vec4<f32>(dhdx, dhdz, ddxdx, ddzdz));

    // Schiuma: Jacobiano dello spostamento orizzontale (Tessendorf).
    let lam = p.choppiness;
    let jac = (1.0 + lam * ddxdx) * (1.0 + lam * ddzdz) - lam * lam * ddxdz * ddxdz;
    let prev = textureLoad(foam_map, pos, c).r;
    let dt = min(p.delta_time, 0.1);
    let grow = clamp(p.foam_threshold - jac, 0.0, 1.0) * p.foam_grow * dt;
    let foam = clamp(prev * exp(-p.foam_decay * dt) + grow, 0.0, 1.0);
    textureStore(foam_map, pos, c, vec4<f32>(foam, 0.0, 0.0, 0.0));
}
