// Port in WGSL dello shader BLBProceduralSkybox.shader di Dynamic Skies.
// Include lo scattering atmosferico di Preetham (da Scattering.cginc),
// le orbite lunari ellittiche (da MoonFunctions.cginc) e i due layer
// di nuvole con bending, normal effect e sun color lerp.
//
// ============================================================================
// DIFFERENZE RISPETTO ALLA VERSIONE ORIGINALE
// ============================================================================
//
// 1. TUTTI i binding del materiale sono in `@group(2)` (convenzione Bevy 0.15).
// 2. `saturate` e `remap` sono funzioni custom (non builtin WGSL).
// 3. Le `textureSample` dentro `if (ray.y > 0.0)` usano `textureSampleLevel`,
//    perché `textureSample` richiede uniform control flow.
// 4. `ScatterResult(..., ...)` costruito POSIZIONALMENTE con parentesi tonde:
//    WGSL non supporta `NomeStruct { campo: valore }`.
// 5. Vertex shader forza `clip.z = 0.0` (reverse-Z, cielo a profondità infinita).
//
// ============================================================================
// PULSAZIONE DELL'OPACITÀ DELLE NUVOLE
// ============================================================================
//
// I due campi `cloud_top_opacity_pulse_speed` e `cloud_bottom_opacity_pulse_speed`
// sono **in fondo** alla struct `SkyboxUniforms` (dopo `cloud_fade_height`):
// devono combaciare byte-per-byte con la struct Rust in
// `dream_sky_material.rs`. Se li sposti in mezzo, gli offset cambiano e
// wgpu rifiuta la pipeline ("Buffer structure size 608 ... greater than 592").
//
// Se `pulse_speed > 0`, il fattore di opacità è
//     factor = 0.5 + 0.5 * cos(world_time * pulse_speed)
// quindi parte dal valore base (a t=0), scende a 0 a metà ciclo, torna al
// valore base a fine ciclo. Periodo = `2π / pulse_speed` secondi.
//
// Con `pulse_speed = 0`, factor = 1.0 → l'opacità resta il valore base
// (`cloud_top_opacity` / `cloud_bottom_opacity`) e il comportamento è
// identico a prima.
//
// ============================================================================
// DEBUG
// ============================================================================
//
// DEBUG_MODE: vedi la costante più sotto. Metti 0 per il cielo normale.
//
// ============================================================================

#import bevy_pbr::mesh_view_bindings::view
#import bevy_pbr::mesh_functions::{get_world_from_local, mesh_position_local_to_world}

const DEBUG_MODE: u32 = 0u;
const SKY_AT_INFINITY: bool = false;

// --- INTERRUTTORI PER IL PRESET CLOUDY ---
const INVERT_CLOUDY_TOP: bool = false;
const INVERT_CLOUDY_BOTTOM: bool = true;

const CLOUDY_TOP_ALPHA_THRESHOLD: f32 = 0.0;
const CLOUDY_TOP_ALPHA_MAX: f32 = 0.8;
const CLOUDY_BOTTOM_ALPHA_THRESHOLD: f32 = 0.0;
const CLOUDY_BOTTOM_ALPHA_MAX: f32 = 0.8;

const BOTTOM_CLOUD_ALPHA: f32 = 0.70;
const CLOUDY_SCENE_DARKNESS: f32 = 0.5;

const NIGHT_CLOUD_DARKNESS: f32 = 0.05;
const NIGHT_DARKEN_ONLY_SUNNY: bool = true;

// --- STELLE ---
const STAR_TILING_SCALE: f32 = 0.4;
const STAR_BRIGHTNESS: f32 = 2.0;

// --- LUNE ---
const MOON_OFFSET_BOOST: f32 = 4.0;
const SECUNDA_PHASE_SHIFT: f32 = 2.0;
const MOON_BRIGHTNESS: f32 = 0.3;
const MOON_AMBIENT: f32 = 0.15;

// --- TRAMONTO / ALBA ---
// Il crepuscolo sostituisce gradualmente il colore fisico dello scattering con una
// tavolozza (lo scattering a 2 campioni passa per giallo-verde e rende lo zenit
// olivastro quando il sole è basso). Colori in sRGB "visto a schermo", convertiti
// in lineare nello shader.
const SUNSET_WARM: vec3<f32> = vec3<f32>(1.00, 0.50, 0.18);   // orizzonte verso il sole
const SUNSET_COOL: vec3<f32> = vec3<f32>(0.78, 0.55, 0.72);   // orizzonte opposto (rosato)
const SUNSET_ZENITH: vec3<f32> = vec3<f32>(0.16, 0.24, 0.52); // zenit al crepuscolo
const SUNSET_SUN_TINT: vec3<f32> = vec3<f32>(1.0, 0.45, 0.15); // disco solare e riflessi
// Quota del sole (sin dell'elevazione) a cui il crepuscolo comincia/finisce:
// 0 sopra TWILIGHT_END_HIGH (~30°), 1 sotto TWILIGHT_END_LOW (~1°).
const TWILIGHT_END_LOW: f32 = 0.02;
const TWILIGHT_END_HIGH: f32 = 0.50;
// Sotto questa quota (sin) l'atmosfera si ispessisce (alba/tramonto); sopra resta normale.
const TWILIGHT_THICKNESS_SPAN: f32 = 0.35;
// Spessore massimo al tramonto (il preset ha 1.5: troppo, sega via il blu anche allo zenit).
const DUSK_THICKNESS_MAX: f32 = 1.2;
// Luminosità e forza del crepuscolo.
const TWILIGHT_BRIGHTNESS: f32 = 1.0;
const TWILIGHT_STRENGTH: f32 = 1.0;

// --- Ritocchi d'aspetto (non cambiano la logica) ---
const SUN_SIZE_SCALE: f32 = 0.4;
const SKY_SATURATION: f32 = 1.2;
const SKY_GAIN: f32 = 1.0;

struct SkyboxUniforms {
    // Atmosfera e sole
    sun_size: f32,
    sun_convergence: f32,
    atmosphere_normal: f32,
    atmosphere_dawn_dusk: f32,
    atmosphere_lerp: f32,

    // Colore base
    sky_tint: vec3<f32>,
    ground_color: vec3<f32>,
    exposure: f32,

    // Notte
    night_start: f32,
    night_end: f32,
    sky_fade_start: f32,
    sky_end_start: f32,

    // Nebbia
    fog_day: vec3<f32>,
    fog_night: vec3<f32>,
    fog_distance: f32,
    moon_night_color: vec3<f32>,

    // Retro
    step_size: f32,
    reduce_color: u32,

    // Nuvole top
    cloud_top_tiling: vec2<f32>,
    cloud_top_offset: vec2<f32>,
    cloud_top_day: vec3<f32>,
    cloud_top_alpha_threshold: f32,
    cloud_top_night: vec3<f32>,
    cloud_top_alpha_max: f32,
    cloud_top_color_boost: f32,
    cloud_top_normal_effect: f32,
    cloud_top_opacity: f32,
    cloud_top_bending: f32,
    cloud_top_sun_color: vec3<f32>,
    cloud_top_sun_scale: f32,
    cloud_top_sun_lerp_scale: f32,
    cloud_top_speed: f32,

    // Nuvole bottom
    cloud_bottom_tiling: vec2<f32>,
    cloud_bottom_offset: vec2<f32>,
    cloud_bottom_day: vec3<f32>,
    cloud_bottom_alpha_threshold: f32,
    cloud_bottom_night: vec3<f32>,
    cloud_bottom_alpha_max: f32,
    cloud_bottom_color_boost: f32,
    cloud_bottom_normal_effect: f32,
    cloud_bottom_opacity: f32,
    cloud_bottom_bending: f32,
    cloud_bottom_sun_color: vec3<f32>,
    cloud_bottom_sun_scale: f32,
    cloud_bottom_sun_lerp_scale: f32,
    cloud_bottom_speed: f32,
    cloud_bottom_blend_speed: f32,
    cloud_bottom_blend_scale: f32,
    cloud_bottom_blend_lb: f32,
    cloud_bottom_blend_ub: f32,

    // Stelle
    star_tiling: vec2<f32>,
    star_bending: f32,
    twinkle_tiling: vec2<f32>,
    twinkle_boost: f32,
    twinkle_speed: f32,

    // Masser (luna grande)
    masser_color: vec3<f32>,
    masser_min_size: f32,
    masser_max_size: f32,
    masser_orbit_angle: vec3<f32>,
    masser_orbit_offset: f32,
    masser_orbit_speed: f32,
    masser_semi_major: f32,
    masser_semi_minor: f32,
    masser_auto_phase: u32,
    masser_tidal_angle: vec3<f32>,
    masser_spin_speed: vec3<f32>,

    // Secunda (luna piccola)
    secunda_color: vec3<f32>,
    secunda_min_size: f32,
    secunda_max_size: f32,
    secunda_orbit_angle: vec3<f32>,
    secunda_orbit_offset: f32,
    secunda_orbit_speed: f32,
    secunda_semi_major: f32,
    secunda_semi_minor: f32,
    secunda_auto_phase: u32,
    secunda_tidal_angle: vec3<f32>,
    secunda_spin_speed: vec3<f32>,

    // Tempo
    world_time: f32,
    sun_dir: vec3<f32>,

    // Altezza (sin dell'elevazione) sotto cui le nuvole svaniscono.
    cloud_fade_height: f32,

    // Pulsazione dell'opacità delle nuvole (0 = disattivata).
    // Ogni fattore è `0.5 + 0.5 * cos(world_time * pulse_speed)`:
    //   - a t = 0 il fattore è 1.0 → opacità = valore base
    //   - a metà ciclo il fattore è 0.0 → opacità = 0
    //   - a fine ciclo il fattore torna a 1.0 → opacità = valore base
    // Periodo = 2π / pulse_speed secondi.
    // QUESTI DEVONO ESSERE GLI ULTIMI 2 CAMPI, in questo ordine esatto,
    // sia qui sia in `SkyboxUniforms` di `dream_sky_material.rs`.
    cloud_top_opacity_pulse_speed: f32,
    cloud_bottom_opacity_pulse_speed: f32,
}

@group(2) @binding(0) var<uniform> sky: SkyboxUniforms;

@group(2) @binding(1) var cloud_top_diffuse: texture_2d<f32>;
@group(2) @binding(2) var cloud_top_sampler: sampler;
@group(2) @binding(3) var cloud_top_normal: texture_2d<f32>;
@group(2) @binding(4) var cloud_top_normal_sampler: sampler;

@group(2) @binding(5) var cloud_bottom_diffuse: texture_2d<f32>;
@group(2) @binding(6) var cloud_bottom_sampler: sampler;
@group(2) @binding(7) var cloud_bottom_normal: texture_2d<f32>;
@group(2) @binding(8) var cloud_bottom_normal_sampler: sampler;

@group(2) @binding(9) var star_tex: texture_2d<f32>;
@group(2) @binding(10) var star_sampler: sampler;
@group(2) @binding(11) var star_twinkle_mask: texture_2d<f32>;
@group(2) @binding(12) var star_twinkle_mask_sampler: sampler;
@group(2) @binding(13) var twinkle_noise: texture_2d<f32>;
@group(2) @binding(14) var twinkle_noise_sampler: sampler;

@group(2) @binding(15) var moon_tex: texture_2d<f32>;
@group(2) @binding(16) var moon_sampler: sampler;
@group(2) @binding(17) var secunda_tex: texture_2d<f32>;
@group(2) @binding(18) var secunda_sampler: sampler;

// ============================================================================
// Utility
// ============================================================================

fn saturate(x: f32) -> f32 {
    return clamp(x, 0.0, 1.0);
}

fn saturate3(x: vec3<f32>) -> vec3<f32> {
    return clamp(x, vec3<f32>(0.0), vec3<f32>(1.0));
}

/// Porta nel range [0,1] preservando la tinta (un arancio sovraesposto resta
/// arancio invece di diventare giallo) e sbianca appena le zone molto luminose.
fn soft_clip(c: vec3<f32>) -> vec3<f32> {
    let m = max(max(c.r, c.g), max(c.b, 1.0));
    let w = min((m - 1.0) * 0.15, 0.5);
    return mix(c / m, vec3<f32>(1.0), w);
}

fn remap(v: f32, in_min: f32, in_max: f32, out_min: f32, out_max: f32) -> f32 {
    return out_min + (v - in_min) * (out_max - out_min) / (in_max - in_min);
}

// ============================================================================
// Costanti di scattering (da Scattering.cginc)
// ============================================================================

const OUTER_RADIUS: f32 = 1.025;
const INNER_RADIUS: f32 = 1.0;
const CAMERA_HEIGHT: f32 = 0.0001;
const MAX_SCATTER: f32 = 50.0;

const K_SCALE_DEPTH: f32 = 0.25;
const K_SCALE_OVER_SCALE_DEPTH: f32 = (1.0 / (OUTER_RADIUS - 1.0)) / K_SCALE_DEPTH;
const K_SAMPLES: u32 = 2u;

const MIE_G: f32 = -0.990;
const MIE_G2: f32 = 0.9801;

const HD_SUN_INTENSITY: f32 = 15.0;
const SUN_BRIGHTNESS: f32 = 20.0;
const SUN_SCALE: f32 = 400.0 * SUN_BRIGHTNESS;

// ============================================================================
// Funzioni di scattering
// ============================================================================

fn scale_optical(in_cos: f32) -> f32 {
    let x = 1.0 - in_cos;
    return 0.25 * exp(-0.00287 + x * (0.459 + x * (3.83 + x * (-6.80 + x * 5.25))));
}

fn get_rayleigh_phase(eye_cos2: f32) -> f32 {
    return 0.75 + 0.75 * eye_cos2;
}

fn get_mie_phase(eye_cos: f32, eye_cos2: f32, sun_size: f32) -> f32 {
    var temp = 1.0 + MIE_G2 - 2.0 * MIE_G * eye_cos;
    temp = pow(temp, pow(sun_size, 0.65) * 10.0);
    temp = max(temp, 1.0e-4);
    return 1.5 * ((1.0 - MIE_G2) / (2.0 + MIE_G2)) * (1.0 + eye_cos2) / temp;
}

fn calc_sun_attenuation(light_pos: vec3<f32>, ray: vec3<f32>, sun_size: f32, sun_convergence: f32) -> f32 {
    let focused_eye_cos = pow(saturate(dot(light_pos, ray)), sun_convergence);
    return get_mie_phase(-focused_eye_cos, focused_eye_cos * focused_eye_cos, sun_size);
}

struct ScatterResult {
    c_in: vec3<f32>,
    c_out: vec3<f32>,
}

fn scatter(eye_ray: vec3<f32>, sun_dir: vec3<f32>, atm_thickness: f32, sky_tint: vec3<f32>) -> ScatterResult {
    let k_rayleigh = mix(0.0, 0.0025, pow(atm_thickness, 2.5));
    let k_mie = 0.0010;
    let k_kr_e_sun = k_rayleigh * SUN_BRIGHTNESS;
    let k_kr4_pi = k_rayleigh * 4.0 * 3.14159265;
    let k_km_e_sun = k_mie * SUN_BRIGHTNESS;
    let k_km4_pi = k_mie * 4.0 * 3.14159265;

    let default_wl = vec3<f32>(0.65, 0.57, 0.475);
    let var_range = vec3<f32>(0.15, 0.15, 0.15);
    let tinted = mix(sky_tint, vec3<f32>(1.0), saturate(1.0 - abs(sun_dir.y)));
    let wl = mix(default_wl - var_range, default_wl + var_range, vec3<f32>(1.0) - tinted);
    let inv_wl = 1.0 / pow(wl, vec3<f32>(4.0));

    let camera_pos = vec3<f32>(0.0, INNER_RADIUS + CAMERA_HEIGHT, 0.0);
    let far = 0.5 * (sqrt(OUTER_RADIUS * OUTER_RADIUS
    + INNER_RADIUS * INNER_RADIUS * eye_ray.y * eye_ray.y
    - INNER_RADIUS * INNER_RADIUS)
    - INNER_RADIUS * eye_ray.y);

    let sample_length = far / f32(K_SAMPLES);
    let scaled_length = sample_length * (1.0 / (OUTER_RADIUS - 1.0));
    let sample_ray = eye_ray * sample_length;
    var sample_point = camera_pos + sample_ray * 0.5;

    let height = INNER_RADIUS + CAMERA_HEIGHT;
    let depth0 = exp(K_SCALE_OVER_SCALE_DEPTH * (-CAMERA_HEIGHT));
    let start_angle = dot(eye_ray, camera_pos) / height;
    let start_offset = depth0 * scale_optical(start_angle);

    var front = vec3<f32>(0.0);
    for (var i = 0u; i < K_SAMPLES; i = i + 1u) {
        let h = length(sample_point);
        let d = exp(K_SCALE_OVER_SCALE_DEPTH * (INNER_RADIUS - h));
        let light_angle = dot(sun_dir, sample_point) / h;
        let camera_angle = dot(eye_ray, sample_point) / h;
        let scat = start_offset + d * (scale_optical(light_angle) - scale_optical(camera_angle));
        let attenuate = exp(-clamp(scat, 0.0, MAX_SCATTER) * (inv_wl * k_kr4_pi + k_km4_pi));
        front += attenuate * (d * scaled_length);
        sample_point += sample_ray;
    }

    return ScatterResult(
        front * (inv_wl * k_kr_e_sun),
                         front * k_km_e_sun,
    );
}

// ============================================================================
// Funzioni per le lune (da MoonFunctions.cginc)
// ============================================================================

fn rotate_arbitrary_axis(v: vec3<f32>, angle_deg: f32, axis: vec3<f32>) -> vec3<f32> {
    let a = radians(angle_deg);
    let c = cos(a);
    let s = sin(a);
    let k = axis;
    let m = mat3x3<f32>(
        c + k.x*k.x*(1.0-c),      k.x*k.y*(1.0-c) - k.z*s, k.x*k.z*(1.0-c) + k.y*s,
                        k.y*k.x*(1.0-c) + k.z*s,  c + k.y*k.y*(1.0-c),     k.y*k.z*(1.0-c) - k.x*s,
                        k.z*k.x*(1.0-c) - k.y*s,  k.z*k.y*(1.0-c) + k.x*s, c + k.z*k.z*(1.0-c),
    );
    return m * v;
}

fn ellipse_position(maj_min: vec2<f32>, angle: f32) -> vec3<f32> {
    return normalize(vec3<f32>(maj_min.x * cos(angle), 0.0, maj_min.y * sin(angle)));
}

fn orbit_position(orbit_angles: vec3<f32>, maj_min: vec2<f32>, angle: f32) -> vec3<f32> {
    var p = ellipse_position(maj_min, angle);
    p = rotate_arbitrary_axis(p, orbit_angles.x, vec3<f32>(1.0, 0.0, 0.0));
    p = rotate_arbitrary_axis(p, orbit_angles.y, vec3<f32>(0.0, 1.0, 0.0));
    p = rotate_arbitrary_axis(p, orbit_angles.z, vec3<f32>(0.0, 0.0, 1.0));
    return p;
}

fn moon_distance(min_size: f32, max_size: f32, maj_min: vec2<f32>, angle: f32) -> f32 {
    let p = ellipse_position(maj_min, angle);
    let lerp_factor = abs(dot(p, vec3<f32>(0.0, 0.0, 1.0)));
    return mix(min_size, max_size, smoothstep(0.0, 1.0, lerp_factor));
}

fn sphere_intersect(ray_o: vec3<f32>, ray_d: vec3<f32>, sphere_p: vec3<f32>, r: f32) -> f32 {
    let oc = ray_o - sphere_p;
    let b = dot(oc, ray_d);
    let c = dot(oc, oc) - r*r;
    let h = b*b - c;
    if (h < 0.0) { return -1.0; }
    return -b - sqrt(h);
}

// ============================================================================
// Vertex
// ============================================================================

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
}

@vertex
fn vertex(
    @builtin(instance_index) instance_index: u32,
          @location(0) position: vec3<f32>,
) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = get_world_from_local(instance_index);
    let world_pos = mesh_position_local_to_world(world_from_local, vec4<f32>(position, 1.0)).xyz;
    var clip = view.clip_from_world * vec4<f32>(world_pos, 1.0);
    if (SKY_AT_INFINITY) {
        clip.z = 0.0;
    }
    out.clip_position = clip;
    out.world_pos = world_pos;
    return out;
}

// ============================================================================
// Fragment
// ============================================================================

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let ray = normalize(in.world_pos - view.world_position.xyz);

    if (DEBUG_MODE == 4u && sky.world_time >= 0.0) {
        return vec4<f32>(1.0, 0.0, 1.0, 1.0);
    }
    if (DEBUG_MODE != 0u && sky.world_time >= 0.0) {
        let debug_uv = (ray.xz / (ray.y + sky.cloud_top_bending)) * sky.cloud_top_tiling + sky.cloud_top_offset;
        var dbg = textureSample(cloud_top_diffuse, cloud_top_sampler, debug_uv).rgb;
        if (DEBUG_MODE == 2u) {
            dbg = vec3<f32>(fract(debug_uv), 0.0);
        } else if (DEBUG_MODE == 5u) {
            let cell = floor(clamp(debug_uv, vec2<f32>(-1.0e4), vec2<f32>(1.0e4)));
            let chk = f32((i32(cell.x) + i32(cell.y)) & 1);
            dbg = vec3<f32>(0.1 + 0.8 * chk);
            if (ray.y <= 0.0) {
                dbg = vec3<f32>(0.0, 0.0, 0.4);
            }
        } else if (DEBUG_MODE == 3u) {
            dbg = vec3<f32>(saturate(sky.cloud_top_tiling.x), saturate(sky.cloud_top_tiling.y), saturate(sky.cloud_top_bending));
        }
        return vec4<f32>(dbg, 1.0);
    }

    // --- Atmosfera (Preetham) ---
    var atm_thickness = sky.atmosphere_normal;
    let sun_y = sky.sun_dir.y;
    // 0 con il sole alto, 1 con il sole all'orizzonte o sotto. (La formula
    // precedente usava `atmosphere_dawn_dusk` anche come bordo dello smoothstep e
    // dava già 1.25 di spessore con il sole a 35°: orizzonte giallo-verde alle 16.)
    let lerp_scale = 1.0 - smoothstep(0.0, TWILIGHT_THICKNESS_SPAN * max(sky.atmosphere_lerp, 0.01), sun_y);
    atm_thickness = mix(sky.atmosphere_normal, min(sky.atmosphere_dawn_dusk, DUSK_THICKNESS_MAX), lerp_scale);

    // Calcolo giorno/notte spostato in alto per poter scurire il terreno
    let day = saturate((sun_y - sky.night_end) / (sky.night_start - sky.night_end));
    let night = 1.0 - day;

    let tint_gamma = pow(max(sky.sky_tint, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    let scatter_result = scatter(ray, sky.sun_dir, atm_thickness, tint_gamma);
    // La fase di Rayleigh vuole il coseno AL QUADRATO (0.75 + 0.75 cos²). Con il
    // coseno semplice il lato opposto al sole andava a zero: mezzo cielo nero/olivastro.
    let sun_cos = dot(sky.sun_dir, ray);
    var sky_color = sky.exposure * (scatter_result.c_in * get_rayleigh_phase(sun_cos * sun_cos));

    // Terreno e nebbia scuri di notte (fog_night di solito è nero)
    let ground_color = mix(sky.fog_day, sky.fog_night, night);
    let fog_color = mix(sky.fog_day, sky.fog_night, night);

    let sun_attenuation = calc_sun_attenuation(sky.sun_dir, ray, sky.sun_size * SUN_SIZE_SCALE, sky.sun_convergence);
    let sun_color = vec3<f32>(sun_attenuation);

    if (DEBUG_MODE == 6u && sky.world_time >= 0.0) {
        return vec4<f32>(sky_color, 1.0);
    }
    if (DEBUG_MODE == 7u && sky.world_time >= 0.0) {
        return vec4<f32>(atm_thickness / 3.0, lerp_scale, saturate(sky.sun_dir.y), 1.0);
    }

    let dot_world = dot(ray, vec3<f32>(0.0, 1.0, 0.0));
    let horizon = 1.0 - saturate((dot_world - sky.sky_end_start) / (sky.sky_fade_start - sky.sky_end_start));
    let sky_mask = 1.0 - horizon;
    let ground_mask = saturate(-ray.y / 0.01);

    let is_cloudy = abs(sky.cloud_bottom_color_boost - 1.0) < 0.01;

    // --- TRAMONTO / ALBA ---
    // Prima rientra nel range preservando la tinta, poi sfuma verso la tavolozza.
    sky_color = soft_clip(sky_color);

    // 0 = sole alto, 1 = sole all'orizzonte (e sotto).
    let twilight = (1.0 - smoothstep(TWILIGHT_END_LOW, TWILIGHT_END_HIGH, sun_y)) * TWILIGHT_STRENGTH;
    let sun_h = normalize(vec2<f32>(sky.sun_dir.x, sky.sun_dir.z) + vec2<f32>(1.0e-5));
    let ray_h = normalize(vec2<f32>(ray.x, ray.z) + vec2<f32>(1.0e-5));
    let toward_sun = 0.5 + 0.5 * dot(ray_h, sun_h);          // 1 verso il sole, 0 opposto
    let horizon_band = pow(1.0 - saturate(ray.y), 2.5);       // 1 all'orizzonte, 0 allo zenit
    var band_color = mix(
        pow(SUNSET_COOL, vec3<f32>(2.2)),
                         pow(SUNSET_WARM, vec3<f32>(2.2)),
                         pow(toward_sun, 1.5)
    );
    band_color *= mix(0.55, 1.0, toward_sun);
    let twilight_color = mix(pow(SUNSET_ZENITH, vec3<f32>(2.2)), band_color, horizon_band);
    // Il cielo si spegne quando il sole scende sotto l'orizzonte.
    let twilight_light = smoothstep(-0.18, 0.02, sun_y);
    sky_color = mix(sky_color, twilight_color * twilight_light * TWILIGHT_BRIGHTNESS, twilight);

    // Il disco del sole e i riflessi sulle nuvole diventano caldi, senza estrapolare.
    var sun_color_tinted = sun_color * mix(vec3<f32>(1.0), SUNSET_SUN_TINT, saturate(twilight));

    var col = mix(sky_color, ground_color, ground_mask);
    if (ray.y > 0.0) {
        col += sun_color_tinted;
    }

    // --- Stelle ---
    let star_uv = ray.xz / (ray.y + sky.star_bending);
    let star_val = textureSampleLevel(star_tex, star_sampler, star_uv * sky.star_tiling * STAR_TILING_SCALE, 0.0).r;
    let twinkle_mask = textureSampleLevel(star_twinkle_mask, star_twinkle_mask_sampler, star_uv * sky.twinkle_tiling * STAR_TILING_SCALE, 0.0).r;
    let twinkle = textureSampleLevel(
        twinkle_noise,
        twinkle_noise_sampler,
        star_uv * sky.twinkle_tiling * STAR_TILING_SCALE + vec2<f32>(1.0, 0.0) * sky.world_time * sky.twinkle_speed,
                                     0.0
    ).r;
    var stars = star_val * twinkle_mask * twinkle * sky.twinkle_boost * STAR_BRIGHTNESS;
    stars = saturate(stars);
    if (ray.y > 0.0) {
        col = mix(col, vec3<f32>(stars), night * sky_mask);
        col = max(col, sky.moon_night_color);
    }

    // --- Lune ---
    let masser_angle = sky.masser_orbit_offset + sky.world_time * sky.masser_orbit_speed + MOON_OFFSET_BOOST;
    let secunda_angle = sky.secunda_orbit_offset + sky.world_time * sky.secunda_orbit_speed + MOON_OFFSET_BOOST + SECUNDA_PHASE_SHIFT;

    let masser_pos = orbit_position(sky.masser_orbit_angle, vec2<f32>(sky.masser_semi_major, sky.masser_semi_minor), masser_angle);
    let secunda_pos = orbit_position(sky.secunda_orbit_angle, vec2<f32>(sky.secunda_semi_major, sky.secunda_semi_minor), secunda_angle);

    let masser_r = moon_distance(sky.masser_min_size, sky.masser_max_size, vec2<f32>(sky.masser_semi_major, sky.masser_semi_minor), masser_angle);
    let secunda_r = moon_distance(sky.secunda_min_size, sky.secunda_max_size, vec2<f32>(sky.secunda_semi_major, sky.secunda_semi_minor), secunda_angle);

    let masser_hit = sphere_intersect(vec3<f32>(0.0), ray, masser_pos, masser_r);
    let secunda_hit = sphere_intersect(vec3<f32>(0.0), ray, secunda_pos, secunda_r);

    let moon_vis = night * smoothstep(0.0, 0.02, ray.y);

    if (ray.y > 0.0) {
        if (masser_hit > 0.0) {
            let normal = normalize(ray * masser_hit - masser_pos);
            let ndotl = dot(sky.sun_dir, normal);
            let moon_uv = vec2<f32>(atan2(normal.z, normal.x) / (2.0 * 3.14159265), acos(-normal.y) / 3.14159265);
            let tex = textureSampleLevel(moon_tex, moon_sampler, moon_uv, 0.0).rgb * sky.masser_color * MOON_BRIGHTNESS;
            let lit = saturate(ndotl) * (1.0 - MOON_AMBIENT) + MOON_AMBIENT;
            col = mix(col, mix(sky.moon_night_color, tex, lit), moon_vis);
        }
        if (secunda_hit > 0.0) {
            let normal = normalize(ray * secunda_hit - secunda_pos);
            let ndotl = dot(sky.sun_dir, normal);
            let moon_uv = vec2<f32>(atan2(normal.z, normal.x) / (2.0 * 3.14159265), acos(-normal.y) / 3.14159265);
            let tex = textureSampleLevel(secunda_tex, secunda_sampler, moon_uv, 0.0).rgb * sky.secunda_color * MOON_BRIGHTNESS;
            let lit = saturate(ndotl) * (1.0 - MOON_AMBIENT) + MOON_AMBIENT;
            col = mix(col, mix(sky.moon_night_color, tex, lit), moon_vis);
        }
    }

    // ========================================================================
    // Pulsazione dell'opacità delle nuvole
    // ========================================================================
    // Se `pulse_speed > 0`, il fattore è `0.5 + 0.5 * cos(world_time * pulse_speed)`:
    //   - a t = 0 → 1.0 → opacità = valore base
    //   - a metà ciclo → 0.0 → nuvole invisibili
    //   - a fine ciclo → 1.0 → opacità = valore base
    // Periodo = 2π / pulse_speed secondi (con 0.05 → ~126 s).
    // Con `pulse_speed = 0` il fattore resta 1.0 → comportamento invariato.
    var top_opacity_factor = 1.0;
    if (sky.cloud_top_opacity_pulse_speed > 0.0) {
        top_opacity_factor = 0.5 + 0.5 * cos(sky.world_time * sky.cloud_top_opacity_pulse_speed);
    }
    var bottom_opacity_factor = 1.0;
    if (sky.cloud_bottom_opacity_pulse_speed > 0.0) {
        bottom_opacity_factor = 0.5 + 0.5 * cos(sky.world_time * sky.cloud_bottom_opacity_pulse_speed);
    }

    // --- Nuvole top ---
    let cloud_top_uv = ray.xz / (ray.y + sky.cloud_top_bending);
    let cloud_top_sample = textureSample(
        cloud_top_diffuse,
        cloud_top_sampler,
        cloud_top_uv * sky.cloud_top_tiling + sky.cloud_top_offset + sky.world_time * sky.cloud_top_speed * vec2<f32>(1.0, 1.0)
    ).r;

    var cloud_top_val = cloud_top_sample;
    if (INVERT_CLOUDY_TOP && is_cloudy) {
        cloud_top_val = 1.0 - cloud_top_val;
    }

    var top_alpha_threshold = sky.cloud_top_alpha_threshold;
    var top_alpha_max = sky.cloud_top_alpha_max;
    if (is_cloudy) {
        top_alpha_threshold = CLOUDY_TOP_ALPHA_THRESHOLD;
        top_alpha_max = CLOUDY_TOP_ALPHA_MAX;
    }

    var clouds_top = smoothstep(top_alpha_threshold, top_alpha_max, cloud_top_val);
    let cloud_fade = saturate(ray.y / max(sky.cloud_fade_height, 0.0001));
    clouds_top *= sky_mask * cloud_fade;
    var cloud_top_color = mix(sky.cloud_top_day, sky.cloud_top_night, night);
    cloud_top_color = saturate3(cloud_top_color / max(1.0 - sky.cloud_top_color_boost, 0.001));
    let cloud_top_normal_val = textureSample(
        cloud_top_normal,
        cloud_top_normal_sampler,
        cloud_top_uv * sky.cloud_top_tiling + sky.cloud_top_offset
    ).rgb * 2.0 - 1.0;
    let ndotup_top = remap(
        dot(cloud_top_normal_val, vec3<f32>(0.0, 1.0, 0.0)),
                           -1.0, 1.0,
                           1.0 - sky.cloud_top_normal_effect, 1.0
    );
    cloud_top_color *= ndotup_top;
    let lerp_scale_cloud = lerp_scale;
    let cloud_sun_factor = sqrt(lerp_scale_cloud) * saturate(1.0 + sky.sun_dir.y);
    cloud_top_color = mix(
        cloud_top_color,
        (sun_color_tinted + sky.cloud_top_sun_color) * sky.cloud_top_sun_scale * ndotup_top,
                          cloud_sun_factor * sky.cloud_top_sun_lerp_scale
    );

    if (NIGHT_DARKEN_ONLY_SUNNY && !is_cloudy) {
        cloud_top_color *= mix(1.0, NIGHT_CLOUD_DARKNESS, night);
    }
    col = mix(col, cloud_top_color, clouds_top * sky.cloud_top_opacity * top_opacity_factor);

    // --- Nuvole bottom ---
    let cloud_bottom_uv = ray.xz / (ray.y + sky.cloud_bottom_bending);
    let cloud_bottom_sample = textureSample(
        cloud_bottom_diffuse,
        cloud_bottom_sampler,
        cloud_bottom_uv * sky.cloud_bottom_tiling + sky.cloud_bottom_offset + sky.world_time * sky.cloud_bottom_speed * vec2<f32>(1.0, 1.0)
    ).r;

    var cloud_bottom_val = cloud_bottom_sample;
    if (INVERT_CLOUDY_BOTTOM && is_cloudy) {
        cloud_bottom_val = 1.0 - cloud_bottom_val;
    }

    var bottom_alpha_threshold = sky.cloud_bottom_alpha_threshold;
    var bottom_alpha_max = sky.cloud_bottom_alpha_max;
    if (is_cloudy) {
        bottom_alpha_threshold = CLOUDY_BOTTOM_ALPHA_THRESHOLD;
        bottom_alpha_max = CLOUDY_BOTTOM_ALPHA_MAX;
    }

    var clouds_bottom = smoothstep(bottom_alpha_threshold, bottom_alpha_max, cloud_bottom_val);
    clouds_bottom *= sky_mask * cloud_fade;
    var cloud_bottom_color = mix(sky.cloud_bottom_day, sky.cloud_bottom_night, night);
    cloud_bottom_color = saturate3(cloud_bottom_color / max(1.0 - sky.cloud_bottom_color_boost, 0.001));
    let cloud_bottom_normal_val = textureSample(
        cloud_bottom_normal,
        cloud_bottom_normal_sampler,
        cloud_bottom_uv * sky.cloud_bottom_tiling + sky.cloud_bottom_offset
    ).rgb * 2.0 - 1.0;
    let ndotup_bottom = remap(
        dot(cloud_bottom_normal_val, vec3<f32>(0.0, 1.0, 0.0)),
                              -1.0, 1.0,
                              1.0 - sky.cloud_bottom_normal_effect, 1.0
    );
    cloud_bottom_color *= ndotup_bottom;
    let cloud_sun_factor_bottom = sqrt(sqrt(sqrt(lerp_scale_cloud))) * saturate(1.0 - sky.sun_dir.y);
    cloud_bottom_color = mix(
        cloud_bottom_color,
        (sun_color_tinted + sky.cloud_bottom_sun_color) * sky.cloud_bottom_sun_scale * ndotup_bottom,
                             cloud_sun_factor_bottom * sky.cloud_bottom_sun_lerp_scale
    );

    if (NIGHT_DARKEN_ONLY_SUNNY && !is_cloudy) {
        cloud_bottom_color *= mix(1.0, NIGHT_CLOUD_DARKNESS, night);
    }

    var bottom_alpha_mult = 1.0;
    if (is_cloudy) {
        bottom_alpha_mult = BOTTOM_CLOUD_ALPHA;
    }
    col = mix(col, cloud_bottom_color, clouds_bottom * sky.cloud_bottom_opacity * bottom_alpha_mult * bottom_opacity_factor);

    // --- Fog ---
    // Usa fog_color (che è mix di day e night) per scurire l'orizzonte di notte
    col = mix(col, fog_color, horizon);

    // --- Ritocco finale: saturazione e luminosità ---
    let luma_out = dot(col, vec3<f32>(0.299, 0.587, 0.114));
    col = max(mix(vec3<f32>(luma_out), col, SKY_SATURATION) * SKY_GAIN, vec3<f32>(0.0));

    if (is_cloudy) {
        col *= CLOUDY_SCENE_DARKNESS;
    }

    // --- Riduzione colore ---
    if (sky.reduce_color != 0u) {
        let step = sky.step_size;
        col.r = ceil(col.r / step) * step;
        col.g = ceil(col.g / step) * step;
        col.b = ceil(col.b / step) * step;
    }

    return vec4<f32>(col, 1.0);
}
