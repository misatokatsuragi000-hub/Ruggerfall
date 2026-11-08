//! Oceano FFT (Tessendorf / GodotOceanWaves) per Bevy 0.15.
//!
//! ## Due piani sovrapposti (near + far)
//!
//! Il mare è composto da due copie dello **stesso mesh** con `Transform`
//! diversi:
//!
//! - **Near**: `Plane3d` 4096 m, scala 1, 2 m/quad. Copre 0-2048 m. `Y = water`.
//!   `discard_radius = 0`: non scarta mai.
//! - **Far**: **stesso mesh**, `Transform` scala 8 → 16 m/quad, copre
//!   0-16384 m. `Y = water - 0.3 m`. `discard_radius ≈ 1900 m`: il
//!   fragment shader scarta ogni pixel dove `dist < 1900`, così dove il
//!   near è già disegnato il far non contribuisce e non si vedono due
//!   superfici sovrapposte.
//!
//! ## Direzione del vento: WORLD-FISSA
//!
//! **Cambio rispetto alla versione precedente**: `wind_angle` è ora una
//! direzione **world fissa**, calcolata da una noise con periodo
//! `WIND_PERIOD` (60 s) e ampiezza `WIND_WANDER` (±π, un giro completo).
//! Prima seguiva `sample_sea_direction` (la direzione del mare aperto dal
//! player), che faceva **ruotare il vento insieme alla nave**: le vele
//! finivano col gonfiarsi sempre dallo stesso lato relativo alla nave,
//! perché l'angolo relativo vento-vela restava costante.
//!
//! La funzione `sample_sea_direction` resta come utility per eventuali usi
//! futuri (es. corrente locale), ma **non è più chiamata** da
//! `update_ocean_sea`.
//!
//! ## Nebbia allineata al cielo (fix striscia blu all'orizzonte)
//!
//! `update_ocean_lighting` usa **`horizon`** (colore astronomico modulato dal
//! meteo) come colore della nebbia del mare, **non** `fog_color` della
//! `DistanceFog` della camera.
//!
//! ## Readback GPU (galleggiamento del player) — spostato in `ship.rs`
//!
//! `WaveSampler` **non** viene più popolato da questo modulo: il readback GPU
//! del displacement è fatto da `ship.rs::readback_displacement` (copia di
//! **tutti e 3 i layer** della texture array in un buffer `MAP_READ`) e da
//! `ship.rs::sync_ocean_wave_sampler` (che riversa i byte dentro
//! `crate::ocean::WaveSampler`). Il vecchio `readback_waves` usava
//! `Readback::texture(displacement)`, che in Bevy 0.15 copia **solo il primo
//! layer** di una texture array: `wave_height_at` leggeva dati sbagliati per
//! le cascate 1 e 2 e il galleggiamento era sfasato.
//!
//! La texture `displacement` usa comunque `MAIN_WORLD | RENDER_WORLD` e
//! `COPY_SRC`: serve a `copy_texture_to_buffer` per il readback (in
//! `ship.rs`), e senza `COPY_SRC` andrebbe in "Validation Error".
//!
//! `wave_height_at(sampler, x, z, viewer_xz)` restituisce l'altezza
//! dell'onda nel punto mondo `(x, z)`, **relativa** a `OCEAN_ELEVATION`,
//! replicando **esattamente** il vertex shader di `ocean_render.wgsl`:
//!
//! 1. `uv = world_xz / TILE_LENGTHS[i]` (mascherato a `[0, 1)` perché la
//!    texture è `Repeat`);
//! 2. campiona `disp_tex` al layer `i` (cascata) a LOD 0;
//! 3. prende il **canale G** (indice 1) come altezza;
//! 4. somma le cascate con `vertex_weight(i, dist)`:
//!    - cascata 0 (409.6 m): **sempre** attiva;
//!    - cascata 1 (102.4 m): `1 - smoothstep(100, 400, dist)`;
//!    - cascata 2 (25.6 m): **peso 0** nel vertex shader (non contribuisce
//!      alla geometria; è usata solo dal fragment per normali e foam);
//! 5. `map_scales[i].z = 1.0` (nessun moltiplicatore extra).
//!
//! La distanza `dist` è la distanza **XZ** tra il punto e la **camera**
//! (`view.world_position.xz`), non tra il punto e la nave. Per questo la
//! firma richiede `viewer_xz`: passare `player_tf.translation.xz`.
//!
//! La usa `player.rs` (per il galleggiamento del player) e `ship.rs` (per
//! il galleggiamento della nave).

use bevy::core_pipeline::core_3d::graph::{Core3d, Node3d};
use bevy::core_pipeline::prepass::DepthPrepass;
use bevy::pbr::{DistanceFog, FogFalloff, Material, MaterialPlugin, NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;
use bevy::render::{
    extract_resource::{ExtractResource, ExtractResourcePlugin},
    render_asset::{RenderAssetUsages, RenderAssets},
    render_graph::{self, RenderGraphApp, RenderLabel},
    render_resource::{
        binding_types::{
            storage_buffer_read_only_sized, storage_buffer_sized, texture_storage_2d_array,
            uniform_buffer,
        },
        *,
    },
    renderer::{RenderContext, RenderDevice, RenderQueue},
    texture::GpuImage,
    view::NoFrustumCulling,
    Render, RenderApp, RenderSet,
};

use crate::data::heightmap::OCEAN_ELEVATION;
use crate::data::sky::sky_state;
use crate::data::terrain_mesh::TERRAIN_HEIGHT_SCALE;
use crate::player::Player;
use crate::sky::dream_sky::CurrentPreset;
use crate::sky::SkyClock;
use crate::world::{HeightField, Origin};

// =============================================================================
// Parametri
// =============================================================================

pub const OCEAN_SIZE: u32 = 256;
pub const NUM_CASCADES: u32 = 3;

pub const TILE_LENGTHS: [f32; 3] = [409.6, 102.4, 25.6];
pub const K_BANDS: [f32; 4] = [0.0, 1.6, 6.3, 25.0];

pub const WIND_CALM: f32 = 4.0;
pub const WIND_STORM: f32 = 18.0;
pub const FETCH_CALM: f32 = 20_000.0;
pub const FETCH_STORM: f32 = 220_000.0;
pub const WATER_DEPTH: f32 = 200.0;
pub const WIND_ANGLE: f32 = 0.4;
/// Ampiezza dell'oscillazione dell'angolo del vento attorno a `WIND_ANGLE`.
///
/// **Cambio**: era 0.5 (±28°, range 56°), troppo piccolo per far cambiare
/// segno a `dot(wind, sail_normal)` quando la nave ruota. Con `π` il vento
/// può puntare in qualunque direzione: la vela si gonfia da un lato e poi
/// dall'altro a ogni ciclo.
pub const WIND_WANDER: f32 = std::f32::consts::PI;
/// Periodo della noise dell'angolo del vento (secondi).
///
/// **Cambio**: era 300 s (5 min, troppo lento per accorgersene). Con 60 s
/// il vento completa un giro in un minuto: si vede il cambio di verso.
pub const WIND_PERIOD: f64 = 60.0;
pub const SPREAD_CALM: f32 = 0.9;
pub const SPREAD_STORM: f32 = 0.6;
pub const SWELL_CALM: f32 = 0.6;
pub const SWELL_STORM: f32 = 0.15;
pub const DETAIL: f32 = 0.97;
pub const CHOPPINESS_CALM: f32 = 0.45;
pub const CHOPPINESS_STORM: f32 = 0.95;

pub const WAVE_SCALE: f32 = 6.0;

pub const FOAM_THRESHOLD_CALM: f32 = 0.90;
pub const FOAM_THRESHOLD_STORM: f32 = 0.6;
pub const FOAM_GAIN_CALM: f32 = 1.0;
pub const FOAM_GAIN_STORM: f32 = 0.3;
pub const FOAM_GROW_CALM: f32 = 4.0;
pub const FOAM_GROW_STORM: f32 = 8.0;
pub const FOAM_DECAY_CALM: f32 = 0.5;
pub const FOAM_DECAY_STORM: f32 = 0.9;

pub const ROUGHNESS_CALM: f32 = 0.07;
pub const ROUGHNESS_STORM: f32 = 0.20;

pub const COAST_FOAM_GAIN: f32 = 1.0;
pub const COAST_REF_DEPTH: f32 = 20.0;

pub const SEA_SLOW_PERIOD: f64 = 300.0;
pub const SEA_FAST_PERIOD: f64 = 120.0;
pub const STORM_NOISE_LO: f32 = 0.30;
pub const STORM_NOISE_HI: f32 = 0.70;

const COAST_DIR_SCAN_RADIUS: f32 = 600.0;
const COAST_DIR_WATER_MARGIN: f32 = 5.0;

const OCEAN_VISIBLE_RADIUS: f32 = 2048.0;
const OCEAN_VISIBILITY_SAMPLES: usize = 16;
const OCEAN_VISIBILITY_STEPS: usize = 8;
const OCEAN_VISIBILITY_HYSTERESIS: f32 = 0.5;

/// **Near**: 4096 m, 2047 subd → 2 m/quad. Copre 0-2048 m.
const PLANE_SIZE: f32 = 4096.0;
const PLANE_SUBDIVISIONS: u32 = 2047;
const PLANE_SPACING: f32 = PLANE_SIZE / (PLANE_SUBDIVISIONS + 1) as f32;

/// **Far**: stesso mesh, `Transform` scalato di `FAR_SCALE` → copre 32768 m.
/// Con 2047 subd → 16 m/quad. Abbassato di `FAR_Y_OFFSET` metri.
const FAR_SCALE: f32 = 8.0;
const FAR_Y_OFFSET: f32 = 0.3;

/// Raggio entro cui il far-plane si scarta (in metri mondo). Deve essere
/// minore del lato del near (2048 m) per non lasciare buchi agli angoli;
/// 1900 lascia ~148 m di margine.
const FAR_DISCARD_RADIUS: f32 = 1900.0;

// =============================================================================
// Materiale
// =============================================================================

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct OceanMaterial {
    #[uniform(100)]
    pub params: OceanParams,
    #[texture(101, dimension = "2d_array")]
    #[sampler(102)]
    pub displacement: Handle<Image>,
    #[texture(103, dimension = "2d_array")]
    pub normal: Handle<Image>,
    #[texture(104, dimension = "2d_array", sample_type = "float", filterable = false)]
    pub foam: Handle<Image>,
}

/// Uniform dello shader. Deve combaciare **byte-per-byte** con `OceanParams`
/// in `ocean_render.wgsl`. Layout: 4+10 = 14 `vec4` (224 B), poi 4 `f32`
/// (240 B), poi `discard_radius` + 3 padding (256 B). I 3 padding non si
/// usano in WGSL ma servono a chiudere la struct a 16 byte di allineamento:
/// senza, encase aggiungerebbe padding implicito e il binding mismatch con
/// lo shader andrebbe in "Validation Error".
#[derive(Clone, Debug, PartialEq, ShaderType, Reflect)]
pub struct OceanParams {
    pub map_scales: [Vec4; 4],
    pub water_color: Vec4,
    pub scatter_color: Vec4,
    pub foam_color: Vec4,
    pub sky_zenith: Vec4,
    pub sky_horizon: Vec4,
    pub sun_dir: Vec4,
    pub sun_color: Vec4,
    pub fog: Vec4,
    pub fog_linear: Vec4,
    pub coast: Vec4,
    pub num_cascades: u32,
    pub choppiness: f32,
    pub roughness: f32,
    pub foam_gain: f32,
    /// 0 = non scarta mai (near). >0 = scarta se `dist < discard_radius` (far).
    pub discard_radius: f32,
    pub _pad0: f32,
    pub _pad1: f32,
    pub _pad2: f32,
}

impl Default for OceanParams {
    fn default() -> Self {
        let mut map_scales = [Vec4::ZERO; 4];
        for (i, len) in TILE_LENGTHS.iter().enumerate() {
            map_scales[i] = Vec4::new(1.0 / len, 1.0 / len, 1.0, 1.0);
        }
        Self {
            map_scales,
            water_color: Vec4::new(0.004, 0.045, 0.09, 1.0),
            scatter_color: Vec4::new(0.02, 0.18, 0.17, 1.0),
            foam_color: Vec4::new(0.95, 0.97, 1.0, 1.0),
            sky_zenith: Vec4::new(0.20, 0.38, 0.75, 1.0),
            sky_horizon: Vec4::new(0.60, 0.72, 0.88, 1.0),
            sun_dir: Vec4::new(0.3, 0.8, 0.4, 1.0),
            sun_color: Vec4::new(1.0, 0.95, 0.85, 1.0),
            fog: Vec4::ZERO,
            fog_linear: Vec4::ZERO,
            coast: Vec4::new(0.4, COAST_REF_DEPTH, 0.0, COAST_FOAM_GAIN),
            num_cascades: NUM_CASCADES,
            choppiness: CHOPPINESS_CALM,
            roughness: ROUGHNESS_CALM,
            foam_gain: FOAM_GAIN_CALM,
            discard_radius: 0.0,
            _pad0: 0.0,
            _pad1: 0.0,
            _pad2: 0.0,
        }
    }
}

impl Material for OceanMaterial {
    fn vertex_shader() -> ShaderRef { "shaders/ocean_render.wgsl".into() }
    fn fragment_shader() -> ShaderRef { "shaders/ocean_render.wgsl".into() }
    fn prepass_vertex_shader() -> ShaderRef { "shaders/ocean_render.wgsl".into() }

    fn specialize(
        _pipeline: &bevy::pbr::MaterialPipeline<Self>,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::render::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

// =============================================================================
// Componenti e risorse
// =============================================================================

#[derive(Component)]
pub struct OceanPlane;

#[derive(Resource, Clone, ExtractResource)]
pub struct OceanAssets {
    pub displacement: Handle<Image>,
    pub normal: Handle<Image>,
    pub foam: Handle<Image>,
}

/// Materiali delle due superfici: il near plane ha `discard_radius = 0`
/// (disegna tutto il suo footprint), il far plane ha `discard_radius ≈ 1900 m`
/// (scarta dove il near lo coprirebbe). `update_ocean_lighting` aggiorna
/// entrambi con gli stessi parametri atmosferici, differenziando solo
/// `discard_radius`.
#[derive(Resource)]
pub struct OceanMaterials {
    pub near: Handle<OceanMaterial>,
    pub far: Handle<OceanMaterial>,
}

#[derive(Resource)]
struct OceanGpuBuffers {
    fft_a: Buffer,
    fft_b: Buffer,
    spectrum_uniform: Buffer,
    unpack_uniform: Buffer,
}

#[derive(Resource)]
struct OceanPipelines {
    spectrum_layout: BindGroupLayout,
    spectrum: CachedComputePipelineId,
    fft_layout: BindGroupLayout,
    fft_h: CachedComputePipelineId,
    fft_v: CachedComputePipelineId,
    unpack_layout: BindGroupLayout,
    unpack: CachedComputePipelineId,
}

#[derive(Resource)]
struct OceanBindGroups {
    spectrum: BindGroup,
    fft_h: BindGroup,
    fft_v: BindGroup,
    unpack: BindGroup,
}

#[derive(Resource)]
struct OceanShaderHandles {
    spectrum: Handle<Shader>,
    stockham: Handle<Shader>,
    unpack: Handle<Shader>,
}

#[derive(Resource, Default)]
struct OceanTime {
    elapsed: f32,
}

// =============================================================================
// Uniform (compute)
// =============================================================================

#[derive(Clone, Debug, ShaderType)]
pub struct SpectrumParams {
    pub cascades: [Vec4; 4],
    pub alpha: f32,
    pub peak_frequency: f32,
    pub wind_speed: f32,
    pub angle: f32,
    pub depth: f32,
    pub swell: f32,
    pub detail: f32,
    pub spread: f32,
    pub time: f32,
    pub num_cascades: u32,
}

#[derive(Clone, Debug, ShaderType)]
pub struct UnpackParams {
    pub choppiness: f32,
    pub foam_threshold: f32,
    pub foam_grow: f32,
    pub foam_decay: f32,
    pub delta_time: f32,
    pub num_cascades: u32,
    pub _pad0: f32,
    pub _pad1: f32,
}

// =============================================================================
// Stato del mare
// =============================================================================

#[derive(Resource, Clone, Copy, Debug, ExtractResource)]
pub struct OceanSea {
    pub storm: f32,
    pub wind_angle: f32,
    pub forced: Option<f32>,
    pub seed_offset: f64,
    pub visible: bool,
}

impl Default for OceanSea {
    fn default() -> Self {
        let offset = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_nanos() % 1_000_000) as f64 / 1000.0)
        .unwrap_or(0.0);
        Self {
            storm: 0.3,
            wind_angle: WIND_ANGLE,
            forced: None,
                seed_offset: offset,
                visible: true,
        }
    }
}

fn hash01(i: i64, seed: u64) -> f32 {
    let mut s = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ seed.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    s ^= s >> 33;
    s = s.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    s ^= s >> 33;
    s = s.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    s ^= s >> 33;
    (s >> 40) as f32 / 16_777_216.0
}

fn value_noise(t: f64, seed: u64) -> f32 {
    let i = t.floor();
    let f = (t - i) as f32;
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash01(i as i64, seed);
    let b = hash01(i as i64 + 1, seed);
    a + (b - a) * u
}

fn mix(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Restituisce la direzione del mare aperto (più profondo) dal player.
///
/// **Non più usata da `update_ocean_sea`**: seguire la direzione del mare
/// aperto faceva ruotare il vento insieme alla nave (quindi le vele si
/// gonfiavano sempre dallo stesso lato relativo). La funzione resta come
/// utility per eventuali usi futuri (corrente locale, orientamento delle
/// onde costiere, ecc.).
#[allow(dead_code)]
fn sample_sea_direction(field: &HeightField, origin: &Origin, pos: Vec3) -> Option<Vec2> {
    const N_DIRS: usize = 16;
    let mut deepest_h = f32::MAX;
    let mut deepest_dir = Vec2::ZERO;
    let mut any_water = false;

    for i in 0..N_DIRS {
        let a = i as f32 / N_DIRS as f32 * std::f32::consts::TAU;
        let d = Vec2::new(a.cos(), a.sin());
        if let Some(h) = field.ground(origin, pos.x + d.x * COAST_DIR_SCAN_RADIUS, pos.z + d.y * COAST_DIR_SCAN_RADIUS) {
            if h < deepest_h {
                deepest_h = h;
                deepest_dir = d;
            }
            if h < OCEAN_ELEVATION + COAST_DIR_WATER_MARGIN {
                any_water = true;
            }
        }
    }
    any_water.then_some(deepest_dir)
}

/// Aggiorna `OceanSea.storm` e `OceanSea.wind_angle`.
///
/// - **`storm`**: rumore combinato (slow + fast), indipendente dal player.
///   Viene letto da `ship_hydro` (draft delle navi) e da `update_sail_time`
///   (magnitudine del vento delle vele elfiche).
/// - **`wind_angle`**: rumore con periodo `WIND_PERIOD` e ampiezza
///   `WIND_WANDER` (±π). **World-fisso**, non segue più il player. Viene
///   letto da `update_sail_time` (direzione del vento delle vele).
///
/// Il `player` è mantenuto nella firma per non rompere la registrazione
/// del sistema (e per eventuali usi futuri), ma **non viene letto**.
pub fn update_ocean_sea(
    time: Res<Time>,
    _field: Res<HeightField>,
    _origin: Res<Origin>,
    _player: Query<&Transform, With<Player>>,
    mut sea: ResMut<OceanSea>,
) {
    let dt = time.delta_secs();
    let t = time.elapsed_secs_f64();
    let o = sea.seed_offset;
    let slow = value_noise(t / SEA_SLOW_PERIOD + o, 11);
    let mid = value_noise(t / SEA_FAST_PERIOD + o * 1.7, 29);
    let n = 0.65 * slow + 0.35 * mid;
    let storm = smoothstep(STORM_NOISE_LO, STORM_NOISE_HI, n);
    sea.storm = sea.forced.map(|f| f.clamp(0.0, 1.0)).unwrap_or(storm);

    // Direzione del vento: solo noise, **world-fissa**. Il player/nave non
    // la influenzano: quando la nave ruota, il vento resta fermo nello
    // spazio mondo e il dot(wind_world, sail_normal_world) cambia segno.
    let target = WIND_ANGLE
    + WIND_WANDER * (2.0 * value_noise(t / WIND_PERIOD + o * 0.5, 53) - 1.0);

    let mut delta = target - sea.wind_angle;
    while delta > std::f32::consts::PI { delta -= std::f32::consts::TAU; }
    while delta < -std::f32::consts::PI { delta += std::f32::consts::TAU; }
    let rate = 1.0 - (-dt * 2.0).exp();
    let mut new_angle = sea.wind_angle + delta * rate;
    while new_angle > std::f32::consts::PI { new_angle -= std::f32::consts::TAU; }
    while new_angle < -std::f32::consts::PI { new_angle += std::f32::consts::TAU; }
    sea.wind_angle = new_angle;
}

pub fn control_sea_storm(
    keys: Res<ButtonInput<KeyCode>>,
    mut sea: ResMut<OceanSea>,
) {
    if !keys.just_pressed(KeyCode::F6) { return; }
    sea.forced = match sea.forced {
        None => Some(0.0),
        Some(v) if v < 0.15 => Some(0.30),
        Some(v) if v < 0.45 => Some(0.60),
        Some(v) if v < 0.70 => Some(0.85),
        Some(v) if v < 0.95 => Some(1.0),
        _ => None,
    };
    match sea.forced {
        Some(v) => println!("Mare forzato: storm = {:.2} (F6 per il prossimo)", v),
        None => println!("Mare automatico (rumore)"),
    }
}

fn update_ocean_visibility(
    time: Res<Time>,
    field: Res<HeightField>,
    origin: Res<Origin>,
    player: Query<&Transform, With<Player>>,
    mut sea: ResMut<OceanSea>,
    mut pending: Local<(bool, f32)>,
) {
    let Ok(ptf) = player.get_single() else { return; };
    let pos = ptf.translation;
    let water_y = OCEAN_ELEVATION * TERRAIN_HEIGHT_SCALE;

    let mut visible = false;
    'outer: for i in 0..OCEAN_VISIBILITY_SAMPLES {
        let a = i as f32 / OCEAN_VISIBILITY_SAMPLES as f32 * std::f32::consts::TAU;
        let dir = Vec2::new(a.cos(), a.sin());
        for s in 1..=OCEAN_VISIBILITY_STEPS {
            let r = OCEAN_VISIBLE_RADIUS * s as f32 / OCEAN_VISIBILITY_STEPS as f32;
            let x = pos.x + dir.x * r;
            let z = pos.z + dir.y * r;
            if let Some(h) = field.ground(&origin, x, z) {
                if h < water_y - 1.0 {
                    visible = true;
                    break 'outer;
                }
            }
        }
    }

    let (last_target, timer) = &mut *pending;
    if *last_target != visible {
        *last_target = visible;
        *timer = 0.0;
        return;
    }
    *timer += time.delta_secs();
    if *timer >= OCEAN_VISIBILITY_HYSTERESIS && sea.visible != visible {
        sea.visible = visible;
        if !visible {
            println!("Oceano: nessuna acqua visibile, sospendo il rendering");
        } else {
            println!("Oceano: acqua visibile, riprendo il rendering");
        }
    }
}

fn sea_hs(storm: f32) -> f32 {
    const HS: [f32; 5] = [0.38, 1.41, 2.40, 3.91, 6.52];
    let x = storm.clamp(0.0, 1.0) * 4.0;
    let i = (x.floor() as usize).min(3);
    mix(HS[i], HS[i + 1], x - i as f32) * WAVE_SCALE.sqrt()
}

fn sea_choppiness(storm: f32) -> f32 {
    mix(CHOPPINESS_CALM, CHOPPINESS_STORM, storm)
}

pub fn foam_threshold_for(storm: f32) -> f32 {
    mix(FOAM_THRESHOLD_CALM, FOAM_THRESHOLD_STORM, storm.clamp(0.0, 1.0))
}

pub fn foam_gain_for(storm: f32) -> f32 {
    mix(FOAM_GAIN_CALM, FOAM_GAIN_STORM, storm.clamp(0.0, 1.0))
}

fn spectrum_params(time: f32, sea: &OceanSea) -> SpectrumParams {
    const G: f32 = 9.81;
    let s = sea.storm.clamp(0.0, 1.0);
    let wind = mix(WIND_CALM, WIND_STORM, s);
    let fetch = mix(FETCH_CALM, FETCH_STORM, s);
    let mut cascades = [Vec4::ZERO; 4];
    for i in 0..NUM_CASCADES as usize {
        cascades[i] = Vec4::new(TILE_LENGTHS[i], K_BANDS[i], K_BANDS[i + 1], 0.0);
    }
    SpectrumParams {
        cascades,
        alpha: 0.076 * (wind * wind / (fetch * G)).powf(0.22) * WAVE_SCALE,
        peak_frequency: 22.0 * (G * G / (wind * fetch)).powf(1.0 / 3.0),
        wind_speed: wind,
        angle: sea.wind_angle,
        depth: WATER_DEPTH,
        swell: mix(SWELL_CALM, SWELL_STORM, s),
        detail: DETAIL,
        spread: mix(SPREAD_CALM, SPREAD_STORM, s),
        time,
        num_cascades: NUM_CASCADES,
    }
}

// =============================================================================
// Nodo del render graph
// =============================================================================

#[derive(Debug, Hash, PartialEq, Eq, Clone, RenderLabel)]
pub struct OceanComputeLabel;

struct OceanComputeNode;

impl FromWorld for OceanComputeNode {
    fn from_world(_world: &mut World) -> Self { Self }
}

impl render_graph::Node for OceanComputeNode {
    fn run(
        &self,
        _graph: &mut render_graph::RenderGraphContext,
        render_context: &mut RenderContext,
        world: &World,
    ) -> Result<(), render_graph::NodeRunError> {
        if let Some(sea) = world.get_resource::<OceanSea>() {
            if !sea.visible {
                return Ok(());
            }
        }

        let Some(pipelines) = world.get_resource::<OceanPipelines>() else { return Ok(()); };
        let Some(bind_groups) = world.get_resource::<OceanBindGroups>() else { return Ok(()); };
        let cache = world.resource::<PipelineCache>();
        let (Some(spectrum), Some(fft_h), Some(fft_v), Some(unpack)) = (
            cache.get_compute_pipeline(pipelines.spectrum),
                                                                        cache.get_compute_pipeline(pipelines.fft_h),
                                                                        cache.get_compute_pipeline(pipelines.fft_v),
                                                                        cache.get_compute_pipeline(pipelines.unpack),
        ) else { return Ok(()); };

        let groups = OCEAN_SIZE / 16;
        let encoder = render_context.command_encoder();

        { let mut p = encoder.begin_compute_pass(&ComputePassDescriptor { label: Some("ocean_spectrum"), timestamp_writes: None });
        p.set_pipeline(spectrum); p.set_bind_group(0, &bind_groups.spectrum, &[]);
        p.dispatch_workgroups(groups, groups, NUM_CASCADES); }
        { let mut p = encoder.begin_compute_pass(&ComputePassDescriptor { label: Some("ocean_fft_h"), timestamp_writes: None });
        p.set_pipeline(fft_h); p.set_bind_group(0, &bind_groups.fft_h, &[]);
        p.dispatch_workgroups(OCEAN_SIZE * 4 * NUM_CASCADES, 1, 1); }
        { let mut p = encoder.begin_compute_pass(&ComputePassDescriptor { label: Some("ocean_fft_v"), timestamp_writes: None });
        p.set_pipeline(fft_v); p.set_bind_group(0, &bind_groups.fft_v, &[]);
        p.dispatch_workgroups(OCEAN_SIZE * 4 * NUM_CASCADES, 1, 1); }
        { let mut p = encoder.begin_compute_pass(&ComputePassDescriptor { label: Some("ocean_unpack"), timestamp_writes: None });
        p.set_pipeline(unpack); p.set_bind_group(0, &bind_groups.unpack, &[]);
        p.dispatch_workgroups(groups, groups, NUM_CASCADES); }
        Ok(())
    }
}

// =============================================================================
// Galleggiamento del player (lettura CPU del displacement)
// =============================================================================

/// Ultima copia CPU della texture di displacement, popolata da
/// `ship.rs::sync_ocean_wave_sampler` (che legge le 3 cascate dal `WaveField`
/// di `ship.rs`). Usata da `wave_height_at`.
#[derive(Resource, Default)]
pub struct WaveSampler {
    /// Buffer piatto del readback (layer 0 riga 0 → layer 0 riga 255 →
    /// layer 1 riga 0 → ...), formato Rgba16Float.
    pub buffer: Vec<u8>,
    /// Time dell'ultimo aggiornamento (per debug).
    pub stamp: f64,
    /// True quando almeno un readback è arrivato.
    pub ready: bool,
}

/// Canale del displacement che contiene l'altezza (0=R, 1=G, 2=B, 3=A).
///
/// **Verificato in `ocean_render.wgsl`**: il vertex shader legge `d.y` di
/// `textureSampleLevel(disp_tex, …)` e lo somma a `world_pos.y`, quindi
/// l'altezza è nel **canale G** (indice 1).
const HEIGHT_CHANNEL: usize = 1;

/// Converte un half-float IEEE 754 a `f32` senza aggiungere la crate `half`.
fn f16_to_f32(bits: u16) -> f32 {
    let sign = (bits >> 15) & 1;
    let exp = (bits >> 10) & 0x1F;
    let mant = bits & 0x3FF;
    let val = if exp == 0 {
        if mant == 0 {
            0.0
        } else {
            (mant as f32) / 1024.0 / 16384.0
        }
    } else if exp == 0x1F {
        if mant == 0 { f32::INFINITY } else { f32::NAN }
    } else {
        let m = 1.0 + (mant as f32) / 1024.0;
        m * 2.0f32.powi(exp as i32 - 15)
    };
    if sign == 1 { -val } else { val }
}

/// Peso della cascata `i` per la **posizione geometrica**, replicando
/// `vertex_weight` di `ocean_render.wgsl`.
///
/// - Cascata 0 (409.6 m): **sempre attiva**.
/// - Cascata 1 (102.4 m): `1 - smoothstep(100, 400, dist)`.
/// - Cascata 2 (25.6 m): **peso 0** nel vertex shader (contribuisce solo a
///   normali e foam nel fragment).
fn vertex_weight(i: u32, dist: f32) -> f32 {
    match i {
        0 => 1.0,
        1 => 1.0 - smoothstep(100.0, 400.0, dist),
        _ => 0.0,
    }
}

/// Altezza dell'onda (metri, **relativa** a `OCEAN_ELEVATION`) nel punto
/// mondo `(world_x, world_z)`, vista dalla camera in `viewer_xz`.
///
/// **Da chiamare con `viewer_xz = player_tf.translation.xz`** (non con la
/// posizione della nave): il vertex shader pesa le cascate in base alla
/// distanza **dalla camera**, non dal punto di galleggiamento.
pub fn wave_height_at(
    sampler: &WaveSampler,
    world_x: f32,
    world_z: f32,
    viewer_xz: Vec2,
) -> Option<f32> {
    if !sampler.ready || sampler.buffer.is_empty() {
        return None;
    }
    let dist = Vec2::new(world_x, world_z).distance(viewer_xz);
    let mut total = 0.0f32;
    let mut any = false;

    for i in 0..NUM_CASCADES {
        let w = vertex_weight(i, dist);
        if w <= 0.0 {
            continue;
        }
        let tile = TILE_LENGTHS[i as usize];
        let u = (world_x / tile).rem_euclid(1.0);
        let v = (world_z / tile).rem_euclid(1.0);
        let px = ((u * OCEAN_SIZE as f32) as usize).min(OCEAN_SIZE as usize - 1);
        let py = ((v * OCEAN_SIZE as f32) as usize).min(OCEAN_SIZE as usize - 1);

        // Layout readback D2Array: (cascata * 256 + y) * 256 + x, 8 byte per pixel.
        let pixel_idx =
        (i as usize * OCEAN_SIZE as usize + py) * OCEAN_SIZE as usize + px;
        let offset = pixel_idx * 8 + HEIGHT_CHANNEL * 2;
        if offset + 2 > sampler.buffer.len() {
            continue;
        }
        let bits =
        u16::from_le_bytes([sampler.buffer[offset], sampler.buffer[offset + 1]]);
        total += f16_to_f32(bits) * w;
        any = true;
    }
    any.then_some(total)
}

// =============================================================================
// Plugin
// =============================================================================

pub struct OceanPlugin;

impl Plugin for OceanPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            MaterialPlugin::<OceanMaterial> {
                prepass_enabled: false,
                shadows_enabled: false,
                ..default()
            },
            ExtractResourcePlugin::<OceanAssets>::default(),
                         ExtractResourcePlugin::<OceanSea>::default(),
        ))
        .init_resource::<OceanSea>()
        .init_resource::<WaveSampler>()
        .add_systems(Startup, (setup_ocean_textures, setup_ocean_plane).chain())
        .add_systems(
            Update,
            (
                control_sea_storm,
             update_ocean_sea.after(control_sea_storm),
             update_ocean_visibility.after(update_ocean_sea),
             enable_depth_prepass,
             update_ocean_plane.after(update_ocean_visibility),
             update_ocean_lighting
             .after(update_ocean_sea)
             .after(crate::sky::update_sky)
             .after(crate::calendar::sync_season_from_calendar),
            ),
        );

        let render_app = app.sub_app_mut(RenderApp);
        render_app
        .init_resource::<OceanTime>()
        .add_systems(
            Render,
            (ensure_ocean_gpu_buffers, prepare_ocean_pipelines).in_set(RenderSet::Prepare),
        )
        .add_systems(Render, write_ocean_uniforms.in_set(RenderSet::PrepareResources))
        .add_systems(Render, prepare_ocean_bind_groups.in_set(RenderSet::PrepareBindGroups));
    }

    fn finish(&self, app: &mut App) {
        let (spectrum, stockham, unpack) = {
            let mut shaders = app.world_mut().resource_mut::<Assets<Shader>>();
            (
                shaders.add(Shader::from_wgsl(include_str!("ocean_spectrum.wgsl"), "ocean_spectrum.wgsl")),
             shaders.add(Shader::from_wgsl(include_str!("stockham.wgsl"), "stockham.wgsl")),
             shaders.add(Shader::from_wgsl(include_str!("ocean_unpack.wgsl"), "ocean_unpack.wgsl")),
            )
        };
        let render_app = app.sub_app_mut(RenderApp);
        render_app.insert_resource(OceanShaderHandles { spectrum, stockham, unpack });
        render_app
        .add_render_graph_node::<OceanComputeNode>(Core3d, OceanComputeLabel)
        .add_render_graph_edge(Core3d, OceanComputeLabel, Node3d::StartMainPass);
    }
}

// =============================================================================
// Setup
// =============================================================================

/// Crea una texture dell'oceano.
///
/// - `main_world = true` (solo per il **displacement**): la texture ha una
///   copia CPU (`MAIN_WORLD`) e l'usage flag `COPY_SRC`, necessari per il
///   readback GPU (`ship.rs::readback_displacement`).
/// - `main_world = false` (normal, foam): GPU-only, `RENDER_WORLD` senza
///   `COPY_SRC`.
fn make_ocean_texture(
    images: &mut Assets<Image>,
    label: &'static str,
    format: TextureFormat,
        bytes_per_texel: u32,
        main_world: bool,
) -> Handle<Image> {
    let mut img = Image::new(
        Extent3d { width: OCEAN_SIZE, height: OCEAN_SIZE, depth_or_array_layers: NUM_CASCADES },
        TextureDimension::D2,
        vec![0u8; (OCEAN_SIZE * OCEAN_SIZE * NUM_CASCADES * bytes_per_texel) as usize],
                             format,
                                 if main_world {
                                     RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD
                                 } else {
                                     RenderAssetUsages::RENDER_WORLD
                                 },
    );
    img.texture_descriptor.usage = TextureUsages::STORAGE_BINDING
    | TextureUsages::TEXTURE_BINDING
    | if main_world { TextureUsages::COPY_SRC } else { TextureUsages::empty() };
    img.texture_descriptor.label = Some(label);
    img.texture_view_descriptor = Some(TextureViewDescriptor {
        dimension: Some(TextureViewDimension::D2Array),
                                       ..default()
    });
    img.sampler = bevy::image::ImageSampler::Descriptor(bevy::image::ImageSamplerDescriptor {
        address_mode_u: bevy::image::ImageAddressMode::Repeat,
        address_mode_v: bevy::image::ImageAddressMode::Repeat,
        address_mode_w: bevy::image::ImageAddressMode::Repeat,
        mag_filter: bevy::image::ImageFilterMode::Linear,
        min_filter: bevy::image::ImageFilterMode::Linear,
        mipmap_filter: bevy::image::ImageFilterMode::Linear,
        ..default()
    });
    images.add(img)
}

fn setup_ocean_textures(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    commands.insert_resource(OceanAssets {
        displacement: make_ocean_texture(
            &mut images,
            "ocean_displacement",
            TextureFormat::Rgba16Float,
            8,
            true, // MAIN_WORLD + COPY_SRC necessari per il readback GPU
        ),
        normal: make_ocean_texture(
            &mut images,
            "ocean_normal",
            TextureFormat::Rgba16Float,
            8,
            false,
        ),
        foam: make_ocean_texture(
            &mut images,
            "ocean_foam",
            TextureFormat::R32Float,
            4,
            false,
        ),
    });
}

/// Crea i due piani (near + far) con due materiali distinti. Il near ha
/// `discard_radius = 0` (disegna tutto), il far ha `discard_radius ≈ 1900`
/// (scarta dove il near è già disegnato).
fn setup_ocean_plane(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<OceanMaterial>>,
    assets: Res<OceanAssets>,
) {
    let mesh = meshes.add(
        Plane3d::default().mesh().size(PLANE_SIZE, PLANE_SIZE).subdivisions(PLANE_SUBDIVISIONS),
    );
    let water_y = OCEAN_ELEVATION * TERRAIN_HEIGHT_SCALE;

    let near = materials.add(OceanMaterial {
        params: OceanParams { discard_radius: 0.0, ..OceanParams::default() },
                             displacement: assets.displacement.clone(),
                             normal: assets.normal.clone(),
                             foam: assets.foam.clone(),
    });
    let far = materials.add(OceanMaterial {
        params: OceanParams { discard_radius: FAR_DISCARD_RADIUS, ..OceanParams::default() },
                            displacement: assets.displacement.clone(),
                            normal: assets.normal.clone(),
                            foam: assets.foam.clone(),
    });

    commands.insert_resource(OceanMaterials { near: near.clone(), far: far.clone() });

    commands.spawn((
        Mesh3d(mesh.clone()),
                    MeshMaterial3d(near),
                    Transform::from_xyz(0.0, water_y, 0.0),
                    OceanPlane,
                    NotShadowCaster,
                    NotShadowReceiver,
                    NoFrustumCulling,
    ));

    commands.spawn((
        Mesh3d(mesh),
                    MeshMaterial3d(far),
                    Transform::from_xyz(0.0, water_y - FAR_Y_OFFSET, 0.0)
                    .with_scale(Vec3::splat(FAR_SCALE)),
                    OceanPlane,
                    NotShadowCaster,
                    NotShadowReceiver,
                    NoFrustumCulling,
    ));
}

pub fn update_ocean_plane(
    player: Query<&Transform, With<crate::player::Player>>,
    sea: Res<OceanSea>,
    mut ocean: Query<
    (&mut Transform, &mut Visibility),
                          (With<OceanPlane>, Without<crate::player::Player>),
                          >,
) {
    let Ok(pt) = player.get_single() else { return; };
    let vis_target = if sea.visible { Visibility::Visible } else { Visibility::Hidden };
    let x = (pt.translation.x / PLANE_SPACING).round() * PLANE_SPACING;
    let z = (pt.translation.z / PLANE_SPACING).round() * PLANE_SPACING;
    let base_y = OCEAN_ELEVATION * TERRAIN_HEIGHT_SCALE;

    for (mut ot, mut vis) in &mut ocean {
        *vis = vis_target;
        ot.translation.x = x;
        ot.translation.z = z;
        let is_far = (ot.scale.x - FAR_SCALE).abs() < 0.01;
        ot.translation.y = if is_far { base_y - FAR_Y_OFFSET } else { base_y };
    }
}

fn enable_depth_prepass(
    mut commands: Commands,
    cams: Query<Entity, (With<Camera3d>, Without<DepthPrepass>)>,
) {
    for e in &cams { commands.entity(e).insert(DepthPrepass); }
}

fn fog_exp2_density(falloff: &FogFalloff) -> f32 {
    match *falloff {
        FogFalloff::Linear { end, .. } => 1.978 / end.max(1.0),
        FogFalloff::Exponential { density } => density * 0.506,
        FogFalloff::ExponentialSquared { density } => density,
        FogFalloff::Atmospheric { extinction, .. } => extinction.max_element() * 0.5,
    }
}

fn luma(c: Vec3) -> f32 { 0.2126 * c.x + 0.7152 * c.y + 0.0722 * c.z }

/// Allinea l'oceano alla scena. Aggiorna **entrambi** i materiali (near e
/// far): stessi colori/nebbia/sole, differiscono solo per `discard_radius`.
fn update_ocean_lighting(
    suns: Query<(&GlobalTransform, &DirectionalLight)>,
                         fogs: Query<&DistanceFog, With<Camera3d>>,
                         clock: Option<Res<SkyClock>>,
                         preset: Option<Res<CurrentPreset>>,
                         sea: Res<OceanSea>,
                         mats: Option<Res<OceanMaterials>>,
                         mut materials: ResMut<Assets<OceanMaterial>>,
) {
    let Some(mats) = mats else { return };
    let Some(current) = materials.get(&mats.near) else { return; };
    let mut p = current.params.clone();

    // --- sole ---
    if let Some((gt, light)) = suns.iter().max_by(|a, b| a.1.illuminance.total_cmp(&b.1.illuminance)) {
        let forward: Vec3 = *gt.forward();
        let to_sun = -forward;
        let color: LinearRgba = light.color.into();
        p.sun_dir = Vec4::new(to_sun.x, to_sun.y, to_sun.z, (light.illuminance / 10_000.0).clamp(0.0, 3.0));
        p.sun_color = Vec4::new(color.red, color.green, color.blue, p.sun_color.w);
    }

    // --- cielo astronomico (SkyClock) ---
    let (astro_zenith, astro_horizon, astro_day) = match clock.as_ref() {
        Some(c) => {
            let st = sky_state(c.hours);
            (Vec3::from(st.zenith), Vec3::from(st.horizon), st.day)
        }
        None => (Vec3::new(0.15, 0.38, 0.80), Vec3::new(0.62, 0.78, 0.93), 1.0),
    };

    // --- meteo (CurrentPreset → scene_lighting) ---
    let (weather_tint, weather_fog_gray, weather_fog_bright) = match preset.as_ref() {
        Some(pr) => {
            let wl = pr.current.scene_lighting();
            (Vec3::from_array(wl.tint), wl.fog_gray, wl.fog_bright)
        }
        None => (Vec3::ONE, 0.0, 1.0),
    };

    let day_factor = 0.35 + 0.65 * astro_day;
    let zenith = astro_zenith * weather_tint * day_factor;
    let mut horizon = astro_horizon * weather_tint * day_factor;
    let lum_h = luma(horizon);
    horizon = horizon.lerp(Vec3::splat(lum_h), weather_fog_gray) * weather_fog_bright;
    let zenith_gray = Vec3::splat(luma(zenith));
    let zenith = zenith.lerp(zenith_gray, weather_fog_gray * 0.5) * weather_fog_bright;

    // --- densità nebbia (dalla camera, per coerenza con la scena) ---
    let (fog_w, fog_linear) = match fogs.iter().next() {
        Some(f) => {
            let lin = match f.falloff {
                FogFalloff::Linear { start, end } => Vec4::new(start, end, 1.0, 0.0),
                _ => Vec4::ZERO,
            };
            (fog_exp2_density(&f.falloff), lin)
        }
        None => (0.0, Vec4::ZERO),
    };
    let lum = luma(horizon);
    let day_light = (lum * 3.0).clamp(0.05, 1.0);

    // --- colore acqua ---
    let base_deep = Vec3::new(0.004, 0.045, 0.09);
    let base_scatter = Vec3::new(0.02, 0.18, 0.17);
    let mut water = base_deep.lerp(zenith * 0.20, 0.5);
    let mut scatter = base_scatter.lerp(horizon * 0.35, 0.35);
    let storm_gray = Vec3::splat(lum * 0.15 + 0.004);
    water = water.lerp(storm_gray, sea.storm * 0.55);
    scatter = scatter.lerp(storm_gray * 2.0, sea.storm * 0.6) * (1.0 - 0.35 * sea.storm);
    let dim = 0.15 + 0.85 * (lum * 5.0).clamp(0.0, 1.0);
    water *= dim;
    scatter *= dim;

    p.sky_horizon = horizon.extend(1.0);
    p.sky_zenith = zenith.extend(1.0);
    p.water_color = water.extend(1.0);
    p.scatter_color = scatter.extend(1.0);
    // Nebbia del mare = horizon, non fog_color. Elimina la striscia blu.
    p.fog = horizon.extend(fog_w);
    p.fog_linear = fog_linear;
    p.sun_color.w = day_light;

    let s = sea.storm;
    p.choppiness = sea_choppiness(s);
    p.roughness = mix(ROUGHNESS_CALM, ROUGHNESS_STORM, s);
    p.foam_gain = foam_gain_for(s);
    p.coast = Vec4::new(sea_hs(s), COAST_REF_DEPTH, s, COAST_FOAM_GAIN);

    // Near: tutti i params condivisi, discard_radius resta 0.
    if let Some(m) = materials.get_mut(&mats.near) {
        let mut new_params = p.clone();
        new_params.discard_radius = 0.0;
        if m.params != new_params {
            m.params = new_params;
        }
    }
    // Far: stessi params, discard_radius fisso a FAR_DISCARD_RADIUS.
    if let Some(m) = materials.get_mut(&mats.far) {
        let mut new_params = p;
        new_params.discard_radius = FAR_DISCARD_RADIUS;
        if m.params != new_params {
            m.params = new_params;
        }
    }
}

// =============================================================================
// Render world
// =============================================================================

fn ensure_ocean_gpu_buffers(
    mut commands: Commands,
    render_device: Res<RenderDevice>,
    existing: Option<Res<OceanGpuBuffers>>,
) {
    if existing.is_some() { return; }
    let fft_size = (OCEAN_SIZE * OCEAN_SIZE) as u64 * 8 * 4 * NUM_CASCADES as u64;
    let make_storage = |label: &'static str| render_device.create_buffer(&BufferDescriptor {
        label: Some(label), size: fft_size,
                                                                         usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
                                                                         mapped_at_creation: false,
    });
    let make_uniform = |label: &'static str, size: u64| render_device.create_buffer(&BufferDescriptor {
        label: Some(label), size: size.next_multiple_of(16),
                                                                                    usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
                                                                                    mapped_at_creation: false,
    });
    commands.insert_resource(OceanGpuBuffers {
        fft_a: make_storage("ocean_fft_a"),
                             fft_b: make_storage("ocean_fft_b"),
                             spectrum_uniform: make_uniform("ocean_spectrum_uniform", SpectrumParams::min_size().get()),
                             unpack_uniform: make_uniform("ocean_unpack_uniform", UnpackParams::min_size().get()),
    });
}

fn write_ocean_uniforms(
    mut ocean_time: ResMut<OceanTime>,
    time: Res<Time>,
    sea: Option<Res<OceanSea>>,
    gpu: Option<Res<OceanGpuBuffers>>,
    render_queue: Res<RenderQueue>,
) {
    let Some(gpu) = gpu else { return };
    ocean_time.elapsed += time.delta_secs();
    let sea = sea.map(|s| *s).unwrap_or_default();
    let storm = sea.storm.clamp(0.0, 1.0);

    use bevy::render::render_resource::encase::UniformBuffer;

    let mut sbuf = UniformBuffer::new(Vec::new());
    if sbuf.write(&spectrum_params(ocean_time.elapsed, &sea)).is_ok() {
        render_queue.write_buffer(&gpu.spectrum_uniform, 0, sbuf.as_ref());
    }

    let unpack = UnpackParams {
        choppiness: sea_choppiness(storm),
        foam_threshold: foam_threshold_for(storm),
        foam_grow: mix(FOAM_GROW_CALM, FOAM_GROW_STORM, storm),
        foam_decay: mix(FOAM_DECAY_CALM, FOAM_DECAY_STORM, storm),
        delta_time: time.delta_secs(),
        num_cascades: NUM_CASCADES,
        _pad0: 0.0, _pad1: 0.0,
    };
    let mut ubuf = UniformBuffer::new(Vec::new());
    if ubuf.write(&unpack).is_ok() {
        render_queue.write_buffer(&gpu.unpack_uniform, 0, ubuf.as_ref());
    }
}

fn prepare_ocean_pipelines(
    mut commands: Commands,
    pipeline_cache: Res<PipelineCache>,
    render_device: Res<RenderDevice>,
    shader_handles: Res<OceanShaderHandles>,
    existing: Option<Res<OceanPipelines>>,
) {
    if existing.is_some() { return; }
    let spectrum_layout = render_device.create_bind_group_layout("ocean_spectrum_layout", &[
        uniform_buffer::<SpectrumParams>(false).build(0, ShaderStages::COMPUTE),
                                                                 storage_buffer_sized(false, None).build(1, ShaderStages::COMPUTE),
    ]);
    let fft_layout = render_device.create_bind_group_layout("ocean_fft_layout", &[
        storage_buffer_read_only_sized(false, None).build(0, ShaderStages::COMPUTE),
                                                            storage_buffer_sized(false, None).build(1, ShaderStages::COMPUTE),
    ]);
    let unpack_layout = render_device.create_bind_group_layout("ocean_unpack_layout", &[
        uniform_buffer::<UnpackParams>(false).build(0, ShaderStages::COMPUTE),
                                                               storage_buffer_read_only_sized(false, None).build(1, ShaderStages::COMPUTE),
                                                               texture_storage_2d_array(TextureFormat::Rgba16Float, StorageTextureAccess::WriteOnly).build(2, ShaderStages::COMPUTE),
                                                               texture_storage_2d_array(TextureFormat::Rgba16Float, StorageTextureAccess::WriteOnly).build(3, ShaderStages::COMPUTE),
                                                               texture_storage_2d_array(TextureFormat::R32Float, StorageTextureAccess::ReadWrite).build(4, ShaderStages::COMPUTE),
    ]);
    let queue = |label: &'static str, layout: &BindGroupLayout, shader: &Handle<Shader>, entry: &'static str| {
        pipeline_cache.queue_compute_pipeline(ComputePipelineDescriptor {
            label: Some(label.into()), layout: vec![layout.clone()],
                                              push_constant_ranges: vec![], shader: shader.clone(), shader_defs: vec![],
                                              entry_point: entry.into(), zero_initialize_workgroup_memory: false,
        })
    };
    let spectrum = queue("ocean_spectrum_pipeline", &spectrum_layout, &shader_handles.spectrum, "main");
    let fft_h = queue("ocean_fft_h_pipeline", &fft_layout, &shader_handles.stockham, "horizontal");
    let fft_v = queue("ocean_fft_v_pipeline", &fft_layout, &shader_handles.stockham, "vertical");
    let unpack = queue("ocean_unpack_pipeline", &unpack_layout, &shader_handles.unpack, "main");
    commands.insert_resource(OceanPipelines {
        spectrum_layout, spectrum, fft_layout, fft_h, fft_v, unpack_layout, unpack,
    });
}

fn prepare_ocean_bind_groups(
    mut commands: Commands,
    assets: Option<Res<OceanAssets>>,
    gpu: Option<Res<OceanGpuBuffers>>,
    pipelines: Option<Res<OceanPipelines>>,
    render_device: Res<RenderDevice>,
    images: Res<RenderAssets<GpuImage>>,
    existing: Option<Res<OceanBindGroups>>,
) {
    if existing.is_some() { return; }
    let (Some(assets), Some(gpu), Some(pipelines)) = (assets, gpu, pipelines) else { return; };
    let (Some(disp), Some(norm), Some(foam)) = (
        images.get(&assets.displacement), images.get(&assets.normal), images.get(&assets.foam),
    ) else { return; };

    let spectrum = render_device.create_bind_group("ocean_spectrum_bind_group", &pipelines.spectrum_layout, &[
        BindGroupEntry { binding: 0, resource: gpu.spectrum_uniform.as_entire_binding() },
                                                   BindGroupEntry { binding: 1, resource: gpu.fft_a.as_entire_binding() },
    ]);
    let fft_h = render_device.create_bind_group("ocean_fft_h_bind_group", &pipelines.fft_layout, &[
        BindGroupEntry { binding: 0, resource: gpu.fft_a.as_entire_binding() },
                                                BindGroupEntry { binding: 1, resource: gpu.fft_b.as_entire_binding() },
    ]);
    let fft_v = render_device.create_bind_group("ocean_fft_v_bind_group", &pipelines.fft_layout, &[
        BindGroupEntry { binding: 0, resource: gpu.fft_b.as_entire_binding() },
                                                BindGroupEntry { binding: 1, resource: gpu.fft_a.as_entire_binding() },
    ]);
    let unpack = render_device.create_bind_group("ocean_unpack_bind_group", &pipelines.unpack_layout, &[
        BindGroupEntry { binding: 0, resource: gpu.unpack_uniform.as_entire_binding() },
                                                 BindGroupEntry { binding: 1, resource: gpu.fft_a.as_entire_binding() },
                                                 BindGroupEntry { binding: 2, resource: BindingResource::TextureView(&disp.texture_view) },
                                                 BindGroupEntry { binding: 3, resource: BindingResource::TextureView(&norm.texture_view) },
                                                 BindGroupEntry { binding: 4, resource: BindingResource::TextureView(&foam.texture_view) },
    ]);
    commands.insert_resource(OceanBindGroups { spectrum, fft_h, fft_v, unpack });
}

// =============================================================================
// Test
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_weight_matches_the_shader() {
        for d in [0.0, 50.0, 200.0, 1000.0, 10_000.0] {
            assert!((vertex_weight(0, d) - 1.0).abs() < 1e-6, "cascata 0 a d={d}");
        }
        assert!((vertex_weight(1, 0.0) - 1.0).abs() < 1e-6);
        assert!((vertex_weight(1, 100.0) - 1.0).abs() < 1e-6);
        assert!((vertex_weight(1, 400.0) - 0.0).abs() < 1e-6);
        assert!((vertex_weight(1, 10_000.0) - 0.0).abs() < 1e-6);
        let mid = vertex_weight(1, 250.0);
        assert!(mid > 0.0 && mid < 1.0, "a 250 m deve essere intermedio, è {mid}");
        for d in [0.0, 50.0, 200.0, 1000.0] {
            assert!(vertex_weight(2, d).abs() < 1e-6, "cascata 2 a d={d}");
        }
        assert!(vertex_weight(3, 10.0).abs() < 1e-6);
    }

    #[test]
    fn wave_height_returns_none_before_readback() {
        let sampler = WaveSampler::default();
        assert!(wave_height_at(&sampler, 0.0, 0.0, Vec2::ZERO).is_none());
    }

    #[test]
    fn wave_height_is_zero_with_zero_buffer() {
        let sampler = WaveSampler {
            buffer: vec![0u8; (OCEAN_SIZE * OCEAN_SIZE * NUM_CASCADES * 8) as usize],
            stamp: 0.0,
            ready: true,
        };
        let h = wave_height_at(&sampler, 0.0, 0.0, Vec2::ZERO);
        assert_eq!(h, Some(0.0));
        let h = wave_height_at(&sampler, 100.0, 100.0, Vec2::ZERO);
        assert_eq!(h, Some(0.0));
    }

    #[test]
    fn f16_decoder_basics() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0xBC00), -1.0);
        assert_eq!(f16_to_f32(0x4000), 2.0);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert!(f16_to_f32(0x7C00).is_infinite());
        assert!(f16_to_f32(0x7E00).is_nan());
    }
}
