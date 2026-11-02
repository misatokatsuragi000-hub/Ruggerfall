#import bevy_pbr::{
    forward_io::{Vertex, VertexOutput, FragmentOutput},
    mesh_functions,
    mesh_view_bindings::view,
}

struct OceanParams {
    map_scales: array<vec4<f32>, 4>,
    water_color: vec4<f32>,
    scatter_color: vec4<f32>,
    foam_color: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_horizon: vec4<f32>,
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    fog: vec4<f32>,
    num_cascades: u32,
    choppiness: f32,
    roughness: f32,
    foam_gain: f32,
};

@group(2) @binding(100) var<uniform> op: OceanParams;
@group(2) @binding(101) var disp_tex: texture_2d_array<f32>;
@group(2) @binding(102) var samp: sampler;
@group(2) @binding(103) var norm_tex: texture_2d_array<f32>;
@group(2) @binding(104) var foam_tex: texture_2d_array<f32>;

const PI: f32 = 3.14159265;
const OCEAN_SIZE: i32 = 256;

fn vertex_weight(i: u32, dist: f32) -> f32 {
    if (i == 0u) { return 1.0; }
    if (i == 1u) { return 1.0 - smoothstep(100.0, 400.0, dist); }
    return 0.0;
}

fn pixel_weight(i: u32, dist: f32) -> f32 {
    if (i == 0u) { return 1.0; }
    if (i == 1u) { return 1.0 - smoothstep(500.0, 1500.0, dist); }
    return 1.0 - smoothstep(150.0, 600.0, dist);
}

fn wrap_texel(c: vec2<i32>) -> vec2<i32> {
    let n = vec2<i32>(OCEAN_SIZE);
    return ((c % n) + n) % n;
}

fn foam_at(uv: vec2<f32>, layer: u32) -> f32 {
    let p = uv * f32(OCEAN_SIZE) - vec2<f32>(0.5);
    let i = vec2<i32>(floor(p));
    let f = fract(p);
    let a = textureLoad(foam_tex, wrap_texel(i), layer, 0).r;
    let b = textureLoad(foam_tex, wrap_texel(i + vec2<i32>(1, 0)), layer, 0).r;
    let c = textureLoad(foam_tex, wrap_texel(i + vec2<i32>(0, 1)), layer, 0).r;
    let d = textureLoad(foam_tex, wrap_texel(i + vec2<i32>(1, 1)), layer, 0).r;
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var world_pos = world_from_local * vec4<f32>(vertex.position, 1.0);

    let base_xz = world_pos.xz;
    let dist = length(view.world_position.xz - base_xz);

    var disp = vec3<f32>(0.0);
    for (var i: u32 = 0u; i < op.num_cascades; i = i + 1u) {
        let w = vertex_weight(i, dist);
        let uv = base_xz * op.map_scales[i].xy;
        let d = textureSampleLevel(disp_tex, samp, uv, i, 0.0);
        disp += vec3<f32>(d.x * op.choppiness, d.y, d.z * op.choppiness) * (w * op.map_scales[i].z);
    }
    world_pos = vec4<f32>(world_pos.xyz + disp, 1.0);

    out.world_position = world_pos;
    out.position = view.clip_from_world * world_pos;
    out.world_normal = vec3<f32>(0.0, 1.0, 0.0);
    out.uv = base_xz;
    return out;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var out: FragmentOutput;

    let world = in.world_position.xyz;
    let to_cam = view.world_position - world;
    let dist = max(length(to_cam), 0.001);
    let V = to_cam / dist;

    var slope = vec2<f32>(0.0);
    var dxx = 0.0;
    var dzz = 0.0;
    var height = 0.0;
    var foam = 0.0;
    for (var i: u32 = 0u; i < op.num_cascades; i = i + 1u) {
        let w = pixel_weight(i, dist);
        let uv = in.uv * op.map_scales[i].xy;
        let d = textureSampleLevel(disp_tex, samp, uv, i, 0.0);
        let n = textureSampleLevel(norm_tex, samp, uv, i, 0.0);
        slope += n.xy * (w * op.map_scales[i].w);
        dxx += n.z * w * op.map_scales[i].z;
        dzz += n.w * w * op.map_scales[i].z;
        height += d.y * (w * op.map_scales[i].z);
        foam += foam_at(uv, i) * w;
    }
    foam = clamp(foam * op.foam_gain, 0.0, 1.0);

    let lam = op.choppiness;
    let sx = slope.x / max(1.0 + lam * dxx, 0.3);
    let sz = slope.y / max(1.0 + lam * dzz, 0.3);
    var N = normalize(vec3<f32>(-sx, 1.0, -sz));
    N = normalize(mix(N, vec3<f32>(0.0, 1.0, 0.0), smoothstep(400.0, 2500.0, dist)));

    // Se stiamo guardando la superficie dal basso (sott'acqua), invertiamo la normale
    if (!is_front) {
        N = -N;
    }

    let L = normalize(op.sun_dir.xyz);
    let sun_i = op.sun_dir.w;
    let day = clamp(op.sun_dir.y * 4.0 + 0.15, 0.06, 1.0);

    let ndv = clamp(dot(N, V), 0.0, 1.0);
    let ndl = clamp(dot(N, L), 0.0, 1.0);
    let fresnel = 0.02 + 0.98 * pow(clamp(1.0 - ndv, 0.0, 1.0), 5.0);

    let R = reflect(-V, N);
    let sky = mix(op.sky_horizon.rgb, op.sky_zenith.rgb, pow(clamp(max(R.y, 0.0), 0.0, 1.0), 0.5)) * day;

    let crest = clamp(height * 0.3 + 0.5, 0.0, 1.0);
    let body = mix(op.water_color.rgb, op.scatter_color.rgb, crest * crest * (0.4 + 0.6 * ndl)) * day;

    let rough = mix(op.roughness, 0.35, smoothstep(100.0, 2000.0, dist));
    let a = max(rough * rough, 0.002);
    let a2 = a * a;
    let H = normalize(L + V);
    let ndh = max(dot(N, H), 0.0);
    let dd = ndh * ndh * (a2 - 1.0) + 1.0;
    let ggx = a2 / (PI * dd * dd);
    let sun_horizon = smoothstep(0.0, 0.08, L.y);
    let spec = ggx * fresnel * 0.25 / max(ndv, 0.1) * ndl * sun_horizon;

    var color = mix(body, sky, fresnel);
    color += op.sun_color.rgb * (sun_i * spec);

    let foam_lit = op.foam_color.rgb * (0.25 + 0.75 * ndl) * (0.3 + 0.7 * day);
    color = mix(color, foam_lit, foam);

    let f = 1.0 - exp(-pow(dist * op.fog.w, 2.0));
    color = mix(color, op.fog.rgb, clamp(f, 0.0, 1.0));

    out.color = vec4<f32>(color, 1.0);
    return out;
}
