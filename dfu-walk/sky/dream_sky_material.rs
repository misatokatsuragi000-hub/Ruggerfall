//! Materiale custom per il cielo procedurale Dynamic Skies.
//!
//! `SkyboxUniforms` deve combaciare byte-per-byte con `SkyboxUniforms` in
//! `assets/shaders/dream_sky.wgsl`. **Niente array** nella struct: gli array
//! WGSL nei uniform hanno stride 16 (non 4/8/12), quindi `[f32; 3]` diventerebbe
//! 48 byte invece di 12. Uso solo `f32`, `u32`, `Vec2`, `Vec3`.
//!
//! ## IMPORTANTE: i 2 campi nuovi sono in FONDO
//!
//! `cloud_top_opacity_pulse_speed` e `cloud_bottom_opacity_pulse_speed` sono
//! le **ultime 2 righe** della struct, sia in Rust sia in WGSL. Inserirli in
//! mezzo (per esempio dopo `cloud_top_speed`) li fa finire nel **padding**
//! già esistente prima del `Vec2` successivo: la dimensione totale non
//! cambia (592), mentre il WGSL che li ha in fondo arriva a 608, e wgpu
//! rifiuta la pipeline con "Buffer structure size 608 ... greater than 592".
//!
//! Se aggiungi altri campi, mettili **sempre in fondo**, sia qui sia nel WGSL.
//!
//! I binding sono in `group(2)` (convenzione Bevy per i materiali):
//! - `@binding(0)`  uniform, `SkyboxUniforms`
//! - `@binding(1)+(2)`   cloud top diffuse + sampler
//! - `@binding(3)+(4)`   cloud top normal + sampler
//! - `@binding(5)+(6)`   cloud bottom diffuse + sampler
//! - `@binding(7)+(8)`   cloud bottom normal + sampler
//! - `@binding(9)+(10)`  stelle + sampler
//! - `@binding(11)+(12)` star twinkle mask + sampler
//! - `@binding(13)+(14)` twinkle noise + sampler
//! - `@binding(15)+(16)` luna (Masser) + sampler
//! - `@binding(17)+(18)` luna (Secunda) + sampler
//!
//! ## Culling (Bevy 0.15)
//!
//! La camera sta **al centro** della sfera del cielo: tutte le facce visibili
//! sono back-face, quindi serve disabilitare il backface culling. In Bevy 0.15
//! `Material::cull_mode()` **non esiste** (è stato introdotto in versioni
//! successive) e l'unico modo è impostare `descriptor.primitive.cull_mode = None`
//! dentro `specialize()`. È lo stesso pattern usato da `StandardMaterial` nel
//! sorgente di Bevy 0.15 (`descriptor.primitive.cull_mode = self.cull_mode;`).

use bevy::pbr::{Material, MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::mesh::MeshVertexBufferLayoutRef;
use bevy::render::render_resource::{
    AsBindGroup, RenderPipelineDescriptor, ShaderRef, ShaderType,
    SpecializedMeshPipelineError,
};

/// Handle dello shader del cielo. Il sorgente è incorporato nell'eseguibile da
/// `DreamSkyPlugin::build` con `load_internal_asset!` (file `src/sky/dream_sky.wgsl`):
/// così il materiale usa SEMPRE il file che stai modificando, e non una copia
/// su disco in `assets/shaders/`.
pub const DREAM_SKY_SHADER: Handle<Shader> =
Handle::weak_from_u128(0x6d0c_7a52_93f1_4b8e_a1d4_5c2e_9b7f_3a10);

/// Uniform dello shader. Deve combaciare byte-per-byte con `SkyboxUniforms`
/// in `dream_sky.wgsl`. Nessun array, solo scalari e vettori.
///
/// **I due campi `*_opacity_pulse_speed` sono in fondo**: se li sposti in mezzo,
/// cambiano gli offset e il WGSL legge valori sbagliati (o wgpu rifiuta la
/// pipeline per mismatch di dimensione).
#[derive(Clone, Debug, ShaderType)]
pub struct SkyboxUniforms {
    // --- Atmosfera e sole ---
    pub sun_size: f32,
    pub sun_convergence: f32,
    pub atmosphere_normal: f32,
    pub atmosphere_dawn_dusk: f32,
    pub atmosphere_lerp: f32,

    // --- Colore base ---
    pub sky_tint: Vec3,
    pub ground_color: Vec3,
    pub exposure: f32,

    // --- Notte ---
    pub night_start: f32,
    pub night_end: f32,
    pub sky_fade_start: f32,
    pub sky_end_start: f32,

    // --- Nebbia ---
    pub fog_day: Vec3,
    pub fog_night: Vec3,
    pub fog_distance: f32,
    pub moon_night_color: Vec3,

    // --- Retro ---
    pub step_size: f32,
    pub reduce_color: u32,

    // --- Nuvole top ---
    pub cloud_top_tiling: Vec2,
    pub cloud_top_offset: Vec2,
    pub cloud_top_day: Vec3,
    pub cloud_top_alpha_threshold: f32,
    pub cloud_top_night: Vec3,
    pub cloud_top_alpha_max: f32,
    pub cloud_top_color_boost: f32,
    pub cloud_top_normal_effect: f32,
    pub cloud_top_opacity: f32,
    pub cloud_top_bending: f32,
    pub cloud_top_sun_color: Vec3,
    pub cloud_top_sun_scale: f32,
    pub cloud_top_sun_lerp_scale: f32,
    pub cloud_top_speed: f32,

    // --- Nuvole bottom ---
    pub cloud_bottom_tiling: Vec2,
    pub cloud_bottom_offset: Vec2,
    pub cloud_bottom_day: Vec3,
    pub cloud_bottom_alpha_threshold: f32,
    pub cloud_bottom_night: Vec3,
    pub cloud_bottom_alpha_max: f32,
    pub cloud_bottom_color_boost: f32,
    pub cloud_bottom_normal_effect: f32,
    pub cloud_bottom_opacity: f32,
    pub cloud_bottom_bending: f32,
    pub cloud_bottom_sun_color: Vec3,
    pub cloud_bottom_sun_scale: f32,
    pub cloud_bottom_sun_lerp_scale: f32,
    pub cloud_bottom_speed: f32,
    pub cloud_bottom_blend_speed: f32,
    pub cloud_bottom_blend_scale: f32,
    pub cloud_bottom_blend_lb: f32,
    pub cloud_bottom_blend_ub: f32,

    // --- Stelle ---
    pub star_tiling: Vec2,
    pub star_bending: f32,
    pub twinkle_tiling: Vec2,
    pub twinkle_boost: f32,
    pub twinkle_speed: f32,

    // --- Masser (luna grande) ---
    pub masser_color: Vec3,
    pub masser_min_size: f32,
    pub masser_max_size: f32,
    pub masser_orbit_angle: Vec3,
    pub masser_orbit_offset: f32,
    pub masser_orbit_speed: f32,
    pub masser_semi_major: f32,
    pub masser_semi_minor: f32,
    pub masser_auto_phase: u32,
    pub masser_tidal_angle: Vec3,
    pub masser_spin_speed: Vec3,

    // --- Secunda (luna piccola) ---
    pub secunda_color: Vec3,
    pub secunda_min_size: f32,
    pub secunda_max_size: f32,
    pub secunda_orbit_angle: Vec3,
    pub secunda_orbit_offset: f32,
    pub secunda_orbit_speed: f32,
    pub secunda_semi_major: f32,
    pub secunda_semi_minor: f32,
    pub secunda_auto_phase: u32,
    pub secunda_tidal_angle: Vec3,
    pub secunda_spin_speed: Vec3,

    // --- Tempo ---
    pub world_time: f32,
    pub sun_dir: Vec3,
    /// Altezza (sin dell'elevazione) sotto cui le nuvole svaniscono.
    pub cloud_fade_height: f32,

    // --- Pulsazione dell'opacità delle nuvole (0 = disattivata) ---
    //
    // DEVONO essere le ULTIME 2 righe della struct, nell'ordine esatto del
    // WGSL. Ogni fattore è `0.5 + 0.5 * cos(world_time * pulse_speed)`:
    //   - a t = 0 → 1.0 → opacità = valore base
    //   - a metà ciclo → 0.0 → nuvole invisibili
    //   - a fine ciclo → 1.0 → opacità = valore base
    // Periodo = 2π / pulse_speed secondi (0.05 → ~126 s).
    pub cloud_top_opacity_pulse_speed: f32,
    pub cloud_bottom_opacity_pulse_speed: f32,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct DreamSkyMaterial {
    #[uniform(0)]
    pub uniforms: SkyboxUniforms,

    #[texture(1)]
    #[sampler(2)]
    pub cloud_top_diffuse: Handle<Image>,
    #[texture(3)]
    #[sampler(4)]
    pub cloud_top_normal: Handle<Image>,

    #[texture(5)]
    #[sampler(6)]
    pub cloud_bottom_diffuse: Handle<Image>,
    #[texture(7)]
    #[sampler(8)]
    pub cloud_bottom_normal: Handle<Image>,

    #[texture(9)]
    #[sampler(10)]
    pub star_tex: Handle<Image>,
    #[texture(11)]
    #[sampler(12)]
    pub star_twinkle_mask: Handle<Image>,
    #[texture(13)]
    #[sampler(14)]
    pub twinkle_noise: Handle<Image>,

    #[texture(15)]
    #[sampler(16)]
    pub moon_tex: Handle<Image>,
    #[texture(17)]
    #[sampler(18)]
    pub secunda_tex: Handle<Image>,
}

impl Material for DreamSkyMaterial {
    fn vertex_shader() -> ShaderRef {
        ShaderRef::Handle(DREAM_SKY_SHADER)
    }
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(DREAM_SKY_SHADER)
    }

    fn specialize(
        _pipeline: &MaterialPipeline<Self>,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}
