//! Materiale custom per animare le vele e le bandiere del veliero elfico
//! direttamente sul vertex shader della GPU.
//!
//! La mesh è **quasi piatta** (sagitta base ridotta a 1/10). Tutta la
//! deformazione avviene nel vertex shader `sail_animator.wgsl` in base a:
//!
//! - `wind_strength` (0..1): magnitudine del vento, letta da `OceanSea.storm`;
//! - `wind_angle` (radianti): **direzione** del vento, letta da
//!   `OceanSea.wind_angle`.
//!
//! Le vele si gonfiano **nella direzione reale del vento**: se il vento
//! cambia direzione, il lato gonfio della vela cambia di conseguenza.

use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderRef, ShaderType};

/// Parametri del vertex shader delle vele/bandiere.
///
/// **8 campi, 32 byte** (7 f32/u32 + 1 pad). Il pad finale porta la struct a
/// un multiplo di 16 byte, che è il requisito di allineamento dei buffer
/// uniform in WGSL.
///
/// `SailParams` deve combaciare **byte-per-byte** con `SailParams` in
/// `sail_animator.wgsl`. Se aggiungi un campo, mettilo in fondo a entrambi.
#[derive(Clone, Debug, ShaderType, Reflect)]
pub struct SailParams {
    /// Tempo del gioco in secondi.
    pub time: f32,
    /// Ampiezza massima del gonfiore (metri, in unità locali della mesh).
    pub sail_amp: f32,
    /// Ampiezza dell'ondeggio delle bandiere (metri).
    pub flag_amp: f32,
    /// Sfasamento (radianti).
    pub phase: f32,
    /// `0` = vela triangolare, `1` = bandiera a coda di rondine.
    pub kind: u32,
    /// Forza del vento 0..1 (0 = calmo, 1 = tempesta), da `OceanSea.storm`.
    pub wind_strength: f32,
    /// Direzione del vento in radianti (0 = +X, π/2 = +Z), da `OceanSea.wind_angle`.
    pub wind_angle: f32,
    /// Padding per allineamento a 16 byte. Sempre 0.
    pub _pad0: f32,
}

impl Default for SailParams {
    fn default() -> Self {
        Self {
            time: 0.0,
            sail_amp: 6.5,
            flag_amp: 0.8,
            phase: 0.0,
            kind: 0,
            wind_strength: 0.4,
            wind_angle: 0.0,
            _pad0: 0.0,
        }
    }
}

#[derive(Asset, AsBindGroup, Reflect, Debug, Clone)]
pub struct SailExtension {
    #[uniform(100)]
    pub params: SailParams,
}

impl MaterialExtension for SailExtension {
    fn vertex_shader() -> ShaderRef {
        "shaders/sail_animator.wgsl".into()
    }
}

pub type SailMaterial = ExtendedMaterial<StandardMaterial, SailExtension>;

pub struct SailMaterialPlugin;

impl Plugin for SailMaterialPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<SailMaterial>::default());
    }
}
