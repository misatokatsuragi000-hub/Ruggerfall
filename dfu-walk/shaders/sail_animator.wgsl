// Vertex shader per le vele e le bandiere del veliero elfico.
//
// Adattato secondo la formulazione geometrica di `createDrowTriangularSail`
// e la dinamica del vento (pressione nodi, raffiche, angolo d'attacco).
//
// ## Vele (kind = 0)
//
// Parametrizzazione UV:
//   - `along`  (t in [0, 1]): da Head (cima balumina, t=0) a Clew (base balumina, t=1).
//   - `height` (s in [0, 1]): da Tack (attacco a prua, s=0) alla balumina (bordo posteriore, s=1).
//
// Componenti geometriche dello spostamento (lungo la normale):
//   1. Arco primario guida: sagitta H_main = sagMag * 0.6 su tutta la balumina.
//   2. 3 sub-vele (k in 0..2): ciascuna con arco circolare di sagitta H_incavo (0.22)
//      e spanciamento windBulgeSub = inwardOffset * (sagMag / 3.2).
//   3. Pancia radiale interna: bellySpan = sin(s * PI) * sagMag * 0.1, modulata
//      da subBelly e bloccata in testa da yardLock (vincolo al boma su k=0, tj=0).
//   4. Somma totale: displacement = s * (alongWind + windBulgeSub) + totalBelly.
//
// ## Bandiere (kind = 1)
//
// Porting 1:1 di `animaVeleEBandiereVelieroElfico`.

#import bevy_pbr::forward_io::{Vertex, VertexOutput}
#import bevy_pbr::mesh_functions
#import bevy_pbr::view_transformations::position_world_to_clip

// =============================================================
// Uniform
// =============================================================
struct SailParams {
    /// Tempo del gioco in secondi.
    time: f32,
    /// Sagitta base della vela (Three.js initialSag = 3.2). Se 0, usa il default 3.2.
    sail_amp: f32,
    /// Altezza della mesh della bandiera in unità locali.
    flag_amp: f32,
    /// Sfasamento dell'oscillazione (radianti).
    phase: f32,
    /// `0` = vela, `1` = bandiera.
    kind: u32,
    /// Forza del vento 0..1 (da OceanSea.storm).
    wind_strength: f32,
    /// Direzione del vento in radianti nel piano XZ mondo (0 = +X, π/2 = +Z).
    wind_angle: f32,
    /// Padding per allineamento a 16 byte.
    _pad0: f32,
};

// =============================================================
// Costanti
// =============================================================
const PI: f32 = 3.141592653589793;

// --- Vele ---
/// Sagitta base standard da Three.js (`initialSag = 3.2`).
const SAIL_BASE_SAG: f32 = 3.2;
/// Lunghezza di riferimento balumina di una sotto-vela (L_sub).
/// In Three.js: H_incavo = L_sub * 0.22. Con L_sub ~ 4.0 -> H_incavo ~ 0.88.
const SUB_INCAVO_MAX: f32 = 0.88;

/// Canale UV per la direzione Head -> Clew (t):
/// true = uv.x, false = uv.y.
const ALONG_IS_U: bool = true;

// --- Bandiere ---
const FLAG_H_MIN: f32 = 0.1;

// =============================================================
// Funzioni di supporto (vele)
// =============================================================

/// Arco di circonferenza esatto normalizzato (valore in [0, 1]).
/// `t` in [0, 1] lungo la corda; vale 0 agli estremi (t=0, t=1) e 1 al centro (t=0.5).
/// `s_rel` è il rapporto sagitta/corda (H / L).
fn circular_arc(t: f32, s_rel: f32) -> f32 {
    let s = clamp(s_rel, 0.001, 0.49);
    let sin_theta0 = (4.0 * s) / (1.0 + 4.0 * s * s);
    let cos_theta0 = sqrt(max(0.0, 1.0 - sin_theta0 * sin_theta0));

    let x = 2.0 * t - 1.0;
    let sin_theta = x * sin_theta0;
    let cos_theta = sqrt(max(0.0, 1.0 - sin_theta * sin_theta));

    let denom = 1.0 - cos_theta0;
    if denom < 1e-5 {
        return 4.0 * t * (1.0 - t); // Fallback parabolico per sagitta quasi nulla
    }
    return max(0.0, (cos_theta - cos_theta0) / denom);
}

@group(2) @binding(100) var<uniform> sail: SailParams;

// =============================================================
// Vertex shader
// =============================================================
@vertex
fn vertex(input: Vertex) -> VertexOutput {
    var out: VertexOutput;

    let world_from_local = mesh_functions::get_world_from_local(input.instance_index);
    var local_position = input.position;
    let uv = input.uv;
    let n = input.normal;

    if sail.kind == 0u {
        // =========================================================
        // VELA — Porting 1:1 della logica Three.js
        // =========================================================

        // --- 1. Calcolo dinamico del vento (Three.js) ---
        let n_world = mesh_functions::mesh_normal_local_to_world(n, input.instance_index);
        let n_h = vec2<f32>(n_world.x, n_world.z);
        let n_len = length(n_h);

        let wind_vec = vec2<f32>(cos(sail.wind_angle), sin(sail.wind_angle));
        var projection = 0.0;
        if n_len > 1e-4 {
            projection = dot(wind_vec, n_h / n_len);
        }

        // Segno del lato in cui spinge il vento (+1 o -1 lungo la normale)
        let side_sign = select(-1.0, 1.0, projection >= 0.0);

        // Angolo d'incidenza (angleEff)
        let angle_eff = 0.55 + 0.45 * sqrt(max(0.0, 1.0 - projection * projection));

        // Nodi equivalenti (massimo 22 come in Three.js)
        let knots_per_vela = clamp(sail.wind_strength * 22.0, 0.0, 22.0);
        let wind_pressure = pow(knots_per_vela / 16.0, 1.4);

        // Raffica dinamica dipendente da tempo e nodi
        let gust = sin(sail.time * (1.8 + knots_per_vela * 0.1) + sail.phase) * (0.15 + knots_per_vela * 0.02);

        // Moltiplicatore di forza e sagitta totale (targetSag)
        let force_multiplier = max(0.2, 0.5 + 2.8 * wind_pressure * angle_eff + gust);
        let base_sag = select(SAIL_BASE_SAG, sail.sail_amp, sail.sail_amp > 0.01);
        let sag_mag = max(0.01, base_sag * force_multiplier);

        // --- 2. Parametrizzazione mesh ---
        // t: lungo la balumina (0 = Head, 1 = Clew)
        // s: radiale dal Tack (0 = Tack, 1 = Balumina)
        let t = clamp(select(uv.y, uv.x, ALONG_IS_U), 0.0, 1.0);
        let s = clamp(select(uv.x, uv.y, ALONG_IS_U), 0.0, 1.0);

        // --- 3. Arco guida primario (da Head a Clew) ---
        // H_main = sagMag * 0.6
        let h_main = sag_mag * 0.5;
        let arc_main = circular_arc(t, 0.16);
        let along_wind = h_main * arc_main;

        // --- 4. Calcolo delle 3 sotto-vele (sub-sails) ---
        let k_f = floor(t * 3.0);
        let k = min(u32(k_f), 2u);
        let tj = clamp(t * 3.0 - f32(k), 0.0, 1.0);

        // Incavo del sotto-arco (H_incavo = L_sub * 0.22)
        let inward_offset = SUB_INCAVO_MAX * circular_arc(tj, 1.16);
        // Rigonfiamento del sotto-arco sotto vento
        let wind_bulge_sub = inward_offset * (sag_mag / 0.2);

        // --- 5. Pancia interna radiale (belly) ---
        // bellySpan = sin(s * PI) * sagMag * 0.1 (zero al tack e sulla balumina)
        let belly_span = sin(s * PI) * sag_mag * 0.1;
        // subBelly = sin(tj * PI) * 0.35 + 0.65 (profilo interno per ciascun pannello)
        let sub_belly = sin(tj * PI) * 0.35 + 0.65;
        // Vincolo al boma: yardLock annulla il movimento in testa solo per la prima vela (k=0, tj=0)
        let yard_lock = select(1.0, sin(tj * PI * 0.5), k == 0u);
        let total_belly = belly_span * sub_belly * yard_lock;

        // --- 6. Deformazione finale ---
        // p = P_tack * (1 - s) + pt * s + windNormal * totalBelly
        // Lo spostamento complessivo lungo la normale locale `n`:
        let total_bulge = s * (along_wind + wind_bulge_sub) + total_belly;
        local_position += n * (total_bulge * side_sign);

    } else {
        // =========================================================
        // BANDIERA — porting 1:1 di biblo.py
        // =========================================================
        let H = max(sail.flag_amp, FLAG_H_MIN);

        let c0 = normalize(world_from_local[0].xyz);
        let c1 = normalize(world_from_local[1].xyz);
        let c2 = normalize(world_from_local[2].xyz);
        let rot_world_from_local = mat3x3<f32>(c0, c1, c2);

        let wind_world = vec3<f32>(cos(sail.wind_angle), 0.0, sin(sail.wind_angle));
        let wind_mesh = normalize(transpose(rot_world_from_local) * wind_world);

        let target_rot_y = atan2(-wind_mesh.z, wind_mesh.x);

        let along = uv.x;
        let by = local_position.y + H * 0.5;

        let wave_a = sin(by * 1.5 - along * 4.0 + sail.time * 4.5) * (H * 0.45);
        let wave_b = sin(by * 0.8 - along * 2.2 + sail.time * 2.8 + 1.2) * (H * 0.25);

        let tip_factor = pow(max(0.0, along), 0.75);
        let wind_drag = along * H * 0.35;

        let bulge = (wave_a + wave_b) * tip_factor + wind_drag;
        local_position.z += bulge;

        let c = cos(target_rot_y);
        let s_rot = sin(target_rot_y);
        let rx = local_position.x * c + local_position.z * s_rot;
        let rz = -local_position.x * s_rot + local_position.z * c;
        local_position.x = rx;
        local_position.z = rz;
    }

    // =============================================================
    // Output standard Bevy
    // =============================================================
    let world_pos = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(local_position, 1.0)
    );

    out.position = position_world_to_clip(world_pos.xyz);
    out.world_position = world_pos;
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        n,
        input.instance_index
    );
    out.uv = uv;

    #ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = input.instance_index;
    #endif
    #ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        input.instance_index, world_from_local[3]);
    #endif

    return out;
}
