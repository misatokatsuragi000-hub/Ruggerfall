#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput;
#import bevy_render::view::View

// Deve coincidere con `UnderwaterFx` in underwater_fx.rs (stesso ordine dei campi).
struct UnderwaterFx {
    enabled: u32,
    time: f32,
    blend: f32,
    amplitude: f32,
    tint: vec4<f32>,
    tint_strength: f32,
    vignette_strength: f32,
    caustics_strength: f32,   // 0 = caustiche spente
    water_y: f32,             // quota del livello del mare (m)
    sun_dir: vec4<f32>,       // xyz = verso il sole, w = intensita' (1 = sole pieno)
    sun_color: vec4<f32>,
    fog_range: vec4<f32>,     // x = start, y = end della nebbia subacquea (m)
};

@group(0) @binding(0) var scene_texture: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;
@group(0) @binding(2) var<uniform> fx: UnderwaterFx;

// Depth del prepass (solo geometrie opache: terreno, alberi... l'oceano non c'e').
#ifdef MULTISAMPLED
@group(0) @binding(3) var depth_texture: texture_depth_multisampled_2d;
#else
@group(0) @binding(3) var depth_texture: texture_depth_2d;
#endif

@group(0) @binding(4) var<uniform> view: View;

const TAU: f32 = 6.28318530718;

// Distorsione a due onde sovrapposte: la prima a bassa frequenza (grossa),
// la seconda ad alta frequenza e ampiezza dimezzata (dettaglio fine).
fn distort(uv: vec2<f32>, t: f32, freq: f32) -> vec2<f32> {
    let w1 = vec2<f32>(
        sin(uv.y * freq + t * 2.0),
        cos(uv.x * freq + t * 1.7),
    );
    let w2 = vec2<f32>(
        sin(uv.y * freq * 2.3 + t * 3.1) * 0.5,
        cos(uv.x * freq * 2.7 + t * 2.9) * 0.5,
    );
    return w1 + w2;
}

// Caustiche procedurali "tileable" (joltz0r / Dave Hoskins): periodo 1 in `uv`,
// quindi continue tra tile e stabili quando l'origine mobile viene ribasata.
fn caustic_layer(uv: vec2<f32>, t: f32) -> f32 {
    let p = fract(uv) * TAU - 250.0;
    var i = p;
    var c = 1.0;
    let inten = 0.005;
    for (var n: i32 = 0; n < 5; n = n + 1) {
        let tt = t * (1.0 - (3.5 / f32(n + 1)));
        i = p + vec2<f32>(cos(tt - i.x) + sin(tt + i.y), sin(tt - i.y) + cos(tt + i.x));
        c += 1.0 / length(vec2<f32>(p.x / (sin(i.x + tt) / inten), p.y / (cos(i.y + tt) / inten)));
    }
    c = c / 5.0;
    c = 1.17 - pow(c, 1.4);
    return min(pow(abs(c), 8.0), 2.0);
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let t = fx.time;
    // Ampiezza scalata dal blend: 0 = superficie, 1 = pienamente subacqueo.
    let amp = fx.amplitude * fx.blend;

    // Offset in UV. A 60 FPS il wobble e' stabile perche' `t` e' l'elapsed.
    let offset = distort(in.uv, t, 8.0) * amp;
    let uv = clamp(in.uv + offset, vec2<f32>(0.0), vec2<f32>(1.0));
    var color = textureSample(scene_texture, scene_sampler, uv).rgb;

    // --- Caustiche con dispersione cromatica (effetto arcobaleno) -------------
    // Posizione nel mondo del pixel (stessa uv della scena distorta): da depth + view.
    let dims = vec2<f32>(textureDimensions(depth_texture));
    let pixel = clamp(vec2<i32>(uv * dims), vec2<i32>(0), vec2<i32>(dims) - vec2<i32>(1));
    let depth = textureLoad(depth_texture, pixel, 0);
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0);
    let wp = view.world_from_clip * ndc;
    let world = wp.xyz / wp.w;

    // Derivate sempre in flusso uniforme (prima di qualsiasi if).
    let ddx = dpdx(world);
    let ddy = dpdy(world);

    if (depth > 0.0 && fx.caustics_strength > 0.0 && fx.blend > 0.0) {
        let to_cam = view.world_position - world;
        let dist = length(to_cam);
        let below = fx.water_y - world.y;             // profondita' sotto il pelo dell'acqua
        var nrm = cross(ddx, ddy);
        let nl = length(nrm);

        // Scarta i bordi di silhouette (salti di profondita'): le normali li' sono casuali.
        let step_len = max(length(ddx), length(ddy));
        if (below > 0.05 && nl > 1e-9 && step_len < dist * 0.06) {
            nrm = nrm / nl;
            if (dot(nrm, to_cam) < 0.0) {
                nrm = -nrm;
            }
            let L = normalize(fx.sun_dir.xyz);
            let sun_up = smoothstep(0.02, 0.25, L.y);
            let ndl = clamp(dot(nrm, L), 0.0, 1.0);

            // Punto della superficie da cui la luce del sole arriva a questo punto
            // (le caustiche si allungano con il sole basso).
            let ly = max(L.y, 0.2);
            let surf = world.xz + L.xz / ly * below;

            // --- DISPERSIONE CROMATICA (effetto arcobaleno) ---
            // Simula la rifrazione differenziale: rosso, verde e blu vengono proiettati
            // in posizioni leggermente diverse sul fondale.
            let dispersion = below * 0.12; // 1.2 cm per metro di profondità
            let disp_dir = normalize(L.xz + vec2<f32>(0.001, 0.001)); // evita div/0 se sole zenith
            let offset_r = disp_dir * dispersion * 1.0;
            let offset_g = disp_dir * dispersion * 0.0;
            let offset_b = disp_dir * dispersion * -1.0;

            // Due strati di caustiche per ogni canale RGB, con offset diversi
            let c1_r = caustic_layer((surf + offset_r) / 6.4, t * 0.5 + 23.0);
            let c1_g = caustic_layer((surf + offset_g) / 6.4, t * 0.5 + 23.0);
            let c1_b = caustic_layer((surf + offset_b) / 6.4, t * 0.5 + 23.0);

            let c2_r = caustic_layer((surf + offset_r) / 5.12 + vec2<f32>(0.37, 0.11), t * 0.43 + 7.0);
            let c2_g = caustic_layer((surf + offset_g) / 5.12 + vec2<f32>(0.37, 0.11), t * 0.43 + 7.0);
            let c2_b = caustic_layer((surf + offset_b) / 5.12 + vec2<f32>(0.37, 0.11), t * 0.43 + 7.0);

            // Combina i layer per ogni canale separatamente
            let cr = max(0.65 * c1_r + 0.65 * c2_r - 0.12, 0.0) * 1.3;
            let cg = max(0.65 * c1_g + 0.65 * c2_g - 0.12, 0.0) * 1.3;
            let cb = max(0.65 * c1_b + 0.65 * c2_b - 0.12, 0.0) * 1.3;

            let c = vec3<f32>(cr, cg, cb);

            let depth_fade = smoothstep(0.0, 0.7, below) * exp(-below / 60.0);
            let fog_vis = 1.0 - clamp(
                (dist - fx.fog_range.x) / max(fx.fog_range.y - fx.fog_range.x, 0.1),
                0.0,
                1.0,
            );
            let k = ndl * depth_fade * fog_vis * sun_up * fx.sun_dir.w
                * fx.caustics_strength * fx.blend;
            color += fx.sun_color.rgb * vec3<f32>(0.75, 1.0, 1.0) * c * (k * 1.2);
        }
    }

    // Tint blu-verde subacqueo.
    color = mix(color, color * fx.tint.rgb, fx.tint_strength * fx.blend);

    // Vignettaggio radiale: scurisce i bordi, come se ci fosse acqua intorno.
    let center = vec2<f32>(0.5, 0.5);
    let d = distance(in.uv, center);
    let vignette = 1.0 - smoothstep(0.35, 0.80, d) * fx.vignette_strength * fx.blend;
    color = color * vignette;

    return vec4<f32>(color, 1.0);
}
