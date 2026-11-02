// FFT inversa 1D per linea (radix-2, memoria condivisa), usata per righe e colonne.
// Nessuna normalizzazione: lo spettro e' gia' in metri, il risultato e' somma_k F(k) e^{+ikx}.
//
// Layout del buffer: `field` = cascata * 4 + f (f = 0..3), ogni campo N*N vec2<f32>.

const N: u32 = 256u;
const LOG_N: u32 = 8u;
const PI: f32 = 3.14159265;

@group(0) @binding(0) var<storage, read> input_buf: array<vec2<f32>>;
@group(0) @binding(1) var<storage, read_write> output_buf: array<vec2<f32>>;

var<workgroup> buf_a: array<vec2<f32>, 256>;
var<workgroup> buf_b: array<vec2<f32>, 256>;

fn cmul(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(a.x * b.x - a.y * b.y, a.x * b.y + a.y * b.x);
}

fn fft_line(field: u32, line: u32, lid: u32, horizontal: bool) {
    let base = field * N * N;
    if (horizontal) {
        buf_a[lid] = input_buf[base + line * N + lid];
    } else {
        buf_a[lid] = input_buf[base + lid * N + line];
    }
    workgroupBarrier();

    // permutazione bit-reverse
    var rev: u32 = 0u;
    for (var b: u32 = 0u; b < LOG_N; b = b + 1u) {
        rev = (rev << 1u) | ((lid >> b) & 1u);
    }
    buf_b[rev] = buf_a[lid];
    workgroupBarrier();

    var len: u32 = 2u;
    for (var s: u32 = 0u; s < LOG_N; s = s + 1u) {
        let half_len = len / 2u;
        let j = lid % len;
        if (j < half_len) {
            let ang = 2.0 * PI * f32(j) / f32(len);
            let w = vec2<f32>(cos(ang), sin(ang));
            let i0 = lid;
            let i1 = lid + half_len;
            let u = buf_b[i0];
            let v = cmul(buf_b[i1], w);
            buf_a[i0] = u + v;
            buf_a[i1] = u - v;
        }
        workgroupBarrier();
        buf_b[lid] = buf_a[lid];
        workgroupBarrier();
        len = len * 2u;
    }

    if (horizontal) {
        output_buf[base + line * N + lid] = buf_b[lid];
    } else {
        output_buf[base + lid * N + line] = buf_b[lid];
    }
}

@compute @workgroup_size(256)
fn horizontal(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    fft_line(wid.x / N, wid.x % N, lid.x, true);
}

@compute @workgroup_size(256)
fn vertical(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    fft_line(wid.x / N, wid.x % N, lid.x, false);
}
