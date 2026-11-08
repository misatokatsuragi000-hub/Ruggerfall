#import bevy_pbr::{
    forward_io::{Vertex, VertexOutput, FragmentOutput},
    mesh_functions,
    mesh_view_bindings::{view, globals},
    prepass_utils,
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
    fog_linear: vec4<f32>,   // x = start, y = end, z = 1 se la nebbia lineare e' attiva
    coast: vec4<f32>,        // x = Hs (m), y = profondita' di riferimento (m), z = stato del mare, w = guadagno
    num_cascades: u32,
    choppiness: f32,
    roughness: f32,
    foam_gain: f32,
    discard_radius: f32,     // 0 = non scarta mai; >0 = scarta se dist < questo
    // Padding a 16 byte: la struct Rust ha solo `discard_radius`, encase
    // aggiunge il padding implicito, ma qui va dichiarato esplicitamente
    // per mantenere l'allineamento dell'uniform buffer.
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(2) @binding(100) var<uniform> op: OceanParams;
@group(2) @binding(101) var disp_tex: texture_2d_array<f32>;
@group(2) @binding(102) var samp: sampler;
@group(2) @binding(103) var norm_tex: texture_2d_array<f32>;
@group(2) @binding(104) var foam_tex: texture_2d_array<f32>;

const PI: f32 = 3.14159265;
const OCEAN_SIZE: i32 = 256;

// Quanto scuro è il mare prima di fondersi col cielo.
// 1.0 = si fonde subito, 0.2 = rimane scuro fino all'orizzonte.
const FOG_DARKNESS: f32 = 0.75;

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

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(0.1031, 0.1030));
    q += dot(q, q.yx + 33.33);
    return fract((q.x + q.y) * q.x);
}

// Value noise periodico con periodo `n` celle (continuo tra map pixel, stabile al rebase).
fn noise_periodic(p: vec2<f32>, n: f32) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let i0 = i - n * floor(i / n);
    let i1 = (i + 1.0) - n * floor((i + 1.0) / n);
    let a = hash21(i0);
    let b = hash21(vec2<f32>(i1.x, i0.y));
    let c = hash21(vec2<f32>(i0.x, i1.y));
    let d = hash21(i1);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Distanza lungo la vista da una profondita' NDC (prospettiva infinita, reverse-z di Bevy).
fn linear_depth(ndc: f32) -> f32 {
    return view.clip_from_view[3][2] / ndc;
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

    // Far plane: scarta i pixel dentro il raggio coperto dal near plane.
    // `discard_radius` è 0 sul near (mai scarta) e ~1900 m sul far, così
    // dove il near è già disegnato il far non contribuisce e non si vedono
    // due superfici sovrapposte.
    if (op.discard_radius > 0.0 && dist < op.discard_radius) {
        discard;
    }

    var depth_h = 1000.0;
#ifdef DEPTH_PREPASS
    let scene_depth = prepass_utils::prepass_depth(in.position, 0u);
    if (scene_depth > 0.0 && is_front) {
        let thickness = max(linear_depth(scene_depth) - linear_depth(in.position.z), 0.0);
        depth_h = thickness * clamp(V.y, 0.05, 1.0);
    }
#endif

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

    // --- onde vicino a riva: shoaling (Green) e frangimento (Miche) ---
    var shoal = 1.0;
    var shallow = 0.0;
    if (depth_h < 200.0) {
        let hs = op.coast.x;
        let storm = op.coast.z;
        shoal = clamp(pow(op.coast.y / max(depth_h, 0.3), 0.25), 1.0, 2.0);
        shallow = 1.0 - smoothstep(0.0, 5.0, depth_h);
        let ratio = hs * shoal / max(depth_h, 0.05);
        let miche = smoothstep(0.35, 0.78, ratio);

        let t = globals.time;
        let q = in.uv / 819.2 * 256.0 + vec2<f32>(t * 0.35, -t * 0.25);
        let n1 = noise_periodic(q, 256.0);
        let n2 = noise_periodic(q * 2.0 - vec2<f32>(t * 0.5, t * 0.3), 512.0);
        let nz = 0.6 * n1 + 0.4 * n2;

        let swash = 0.5 + 0.5 * sin(t * 0.9 + n1 * 6.0);
        let reach = mix(0.5, 2.0, swash) * (1.0 + 3.0 * storm);
        let edge = 1.0 - smoothstep(0.0, reach, depth_h);
        let foam_edge = edge * smoothstep(0.25, 0.7, nz + edge * 0.55);
        let foam_break = miche * smoothstep(0.35, 0.75, nz);
        foam = max(foam, clamp((foam_edge + foam_break) * op.coast.w, 0.0, 1.0));
    }
    slope = slope * shoal;

    let lam = op.choppiness;
    let sx = slope.x / max(1.0 + lam * dxx, 0.3);
    let sz = slope.y / max(1.0 + lam * dzz, 0.3);
    // N resta SEMPRE verso l'alto (fuori dall'acqua) in entrambe le viste:
    // il ramo subacqueo gestisce l'orientazione con I = -V.
    var N = normalize(vec3<f32>(-sx, 1.0, -sz));
    N = normalize(mix(N, vec3<f32>(0.0, 1.0, 0.0), smoothstep(400.0, 2500.0, dist)));

    let L = normalize(op.sun_dir.xyz);
    let sun_i = op.sun_dir.w;
    let day = op.sun_color.w;
    let ndl = clamp(dot(N, L), 0.0, 1.0);

    var color: vec3<f32>;

    // =========================================================================
    // VISTA DA SOTT'ACQUA: Finestra di Snell + TIR + assorbimento dell'acqua
    // =========================================================================
    if (!is_front) {
        // Raggio incidente: dalla camera verso la superficie.
        let I = -V;
        let cos_i = clamp(dot(N, I), 0.0, 1.0);

        // Profondita' della camera sotto questo punto di superficie (m).
        let cam_depth = max(world.y - view.world_position.y, 0.0);
        // Beer-Lambert sul percorso superficie -> camera: il rosso e' assorbito
        // prima, quindi il cielo rifratto arriva teal, non azzurro pieno.
        let transmission = exp(-vec3<f32>(0.10, 0.032, 0.024) * cam_depth);

        // Indici di rifrazione acqua->aria per canale (dispersione cromatica).
        let eta_r = 1.331;
        let eta_g = 1.333;
        let eta_b = 1.340;

        // Legge di Snell: sin^2(theta_t) = eta^2 * (1 - cos^2(theta_i))
        let sin_t2_g = eta_g * eta_g * (1.0 - cos_i * cos_i);

        if (sin_t2_g >= 1.0) {
            // === RIFLESSIONE TOTALE INTERNA (TIR) ===
            // Oltre l'angolo critico (~48.6 gradi) la superficie e' uno specchio
            // che riflette il volume d'acqua, non il cielo.
            let reflection_color = mix(op.water_color.rgb, op.scatter_color.rgb, 0.55) * 0.9;

            // Speculare del sole riflesso sulla "cupola" a specchio.
            let H = normalize(op.sun_dir.xyz + V);
            let ndh = max(dot(N, H), 0.0);
            let a = max(op.roughness * op.roughness, 0.002);
            let a2 = a * a;
            let dd = ndh * ndh * (a2 - 1.0) + 1.0;
            let ggx = a2 / (PI * dd * dd);
            let spec = ggx * 0.25 / max(cos_i, 0.1) * ndl * sun_i;

            color = reflection_color + op.sun_color.rgb * spec;
        } else {
            // === FINESTRA DI SNELL (rifrazione) ===
            let cos_t_g = sqrt(1.0 - sin_t2_g);

            // Fresnel esatto per dielettrici (mezzo denso -> mezzo raro).
            let r_perp = (eta_g * cos_i - cos_t_g) / (eta_g * cos_i + cos_t_g);
            let r_par = (cos_i - eta_g * cos_t_g) / (cos_i + eta_g * cos_t_g);
            let fresnel_underwater = 0.5 * (r_perp * r_perp + r_par * r_par);

            // Vettori rifratti per canale RGB: aberrazione cromatica ai bordi.
            let T_r = normalize(refract(I, N, eta_r));
            let T_g = normalize(refract(I, N, eta_g));
            let T_b = normalize(refract(I, N, eta_b));

            // Cielo campionato lungo i raggi rifratti (stesso gradiente di sopra).
            let sky_r = mix(op.sky_horizon.r, op.sky_zenith.r, pow(clamp(max(T_r.y, 0.0), 0.0, 1.0), 0.5));
            let sky_g = mix(op.sky_horizon.g, op.sky_zenith.g, pow(clamp(max(T_g.y, 0.0), 0.0, 1.0), 0.5));
            let sky_b = mix(op.sky_horizon.b, op.sky_zenith.b, pow(clamp(max(T_b.y, 0.0), 0.0, 1.0), 0.5));
            // FIX: il cielo rifratto viene filtrato dalla colonna d'acqua.
            let sky_refracted = vec3<f32>(sky_r, sky_g, sky_b) * transmission;

            // Sole diretto attraverso la superficie + glow largo.
            let T_center = normalize(refract(I, N, eta_g));
            let sun_trans = pow(max(dot(T_center, op.sun_dir.xyz), 0.0), 128.0) * sun_i * 2.5;
            let wide = pow(max(dot(T_center, op.sun_dir.xyz), 0.0), 8.0) * sun_i * 0.15;

            // Riflesso interno: acqua scura ma leggibile, non nero.
            let under_reflection = mix(op.water_color.rgb, op.scatter_color.rgb, 0.5) * 0.8;

            color = mix(sky_refracted + op.sun_color.rgb * sun_trans * transmission,
                        under_reflection, fresnel_underwater);
            color += op.sun_color.rgb * wide * (1.0 - fresnel_underwater) * transmission;
        }

        // Schiuma vista dal basso: bolle/strati traslucidi.
        let foam_lit = op.foam_color.rgb * 0.6;
        color = mix(color, foam_lit, foam * 0.7);

        // In-scatter del volume: la superficie lontana sfuma nel colore
        // dell'acqua (teal scuro), MAI nel colore del cielo.
        let in_scatter = clamp(1.0 - exp(-dist * 0.045), 0.0, 0.9);
        color = mix(color, op.water_color.rgb * 1.3 + op.scatter_color.rgb * 0.7, in_scatter);

    // =========================================================================
    // VISTA DA SOPRA L'ACQUA (logica originale intatta)
    // =========================================================================
    } else {
        let ndv = clamp(dot(N, V), 0.0, 1.0);
        let fresnel = 0.02 + 0.98 * pow(clamp(1.0 - ndv, 0.0, 1.0), 5.0);

        let R = reflect(-V, N);
        let sky = mix(op.sky_horizon.rgb, op.sky_zenith.rgb, pow(clamp(max(R.y, 0.0), 0.0, 1.0), 0.5));

        let crest = clamp(height * 0.3 + 0.5, 0.0, 1.0);
        var body = mix(op.water_color.rgb, op.scatter_color.rgb, crest * crest * (0.4 + 0.6 * ndl));
        body = mix(body, op.scatter_color.rgb * 1.6, shallow * 0.6);

        let rough = mix(op.roughness, 0.35, smoothstep(100.0, 2000.0, dist));
        let a = max(rough * rough, 0.002);
        let a2 = a * a;
        let H = normalize(L + V);
        let ndh = max(dot(N, H), 0.0);
        let dd = ndh * ndh * (a2 - 1.0) + 1.0;
        let ggx = a2 / (PI * dd * dd);
        let sun_horizon = smoothstep(0.0, 0.08, L.y);
        let spec = ggx * fresnel * 0.25 / max(ndv, 0.1) * ndl * sun_horizon;

        color = mix(body, sky, fresnel);
        color += op.sun_color.rgb * (sun_i * spec);

        let foam_lit = op.foam_color.rgb * (0.25 + 0.75 * ndl) * (0.3 + 0.7 * day);
        color = mix(color, foam_lit, foam);
    }

    // =========================================================================
    // NEBBIA E FUSIONE COL CIELO: SOLO vista da sopra.
    // Da sotto il cielo non deve mai tingere la superficie: era questo mix
    // a trasformare il mare in una lastra azzurra guardando da sott'acqua.
    // =========================================================================
    if (is_front) {
        var fog_amount = 1.0 - exp(-pow(dist * op.fog.w, 2.0));
        if (op.fog_linear.z > 0.5) {
            fog_amount = (dist - op.fog_linear.x) / max(op.fog_linear.y - op.fog_linear.x, 1.0);
        }
        fog_amount = clamp(fog_amount, 0.0, 1.0);

        let sky_at_view = mix(op.sky_horizon.rgb, op.sky_zenith.rgb, pow(clamp(V.y, 0.0, 1.0), 0.5));
        let fog_target = sky_at_view * FOG_DARKNESS;
        color = mix(color, fog_target, fog_amount);
    }

    out.color = vec4<f32>(color, 1.0);
    return out;
}
