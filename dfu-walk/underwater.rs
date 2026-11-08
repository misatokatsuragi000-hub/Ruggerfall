//! Effetto subacqueo quando la testa del giocatore scende sotto il livello del mare.
//!
//! `update_underwater_state` calcola la profondità dell'occhio del giocatore
//! (`feet_y + EYE_HEIGHT`) rispetto a `OCEAN_ELEVATION * TERRAIN_HEIGHT_SCALE`.
//! Se è sotto, `is_underwater = true` e `blend` sale verso 1 con un fade di
//! `FADE_SECS`. In interni e dungeon non si attiva.
//!
//! `apply_underwater_effects` gira **dopo** `sky::update_sky` (che scrive già
//! nebbia, ambient e sole di superficie) e li mixa con i valori subacquei per
//! `blend`:
//!
//! - **DistanceFog**: colore blu-verde scuro, falloff lineare denso (0.5-14 m).
//!   Il falloff originale viene salvato al primo ingresso in acqua e
//!   ripristinato all'uscita.
//! - **AmbientLight**: tint blu-verde, brightness ridotta a `0.35`.
//! - **DirectionalLight**: illuminance ridotta a `0.20`, colore spostato verso
//!   il blu (come se il sole fosse filtrato dall'acqua).
//!
//! `underwater_camera_wobble` aggiunge un piccolo roll/pitch alla camera
//! proporzionale a `blend`. Gira dopo `player_look` (che ogni frame resetta la
//! rotazione a yaw/pitch puri), quindi il wobble non si accumula. Se dà
//! fastidio, metti `WATER_WOBBLE_AMPLITUDE = 0.0`.

use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;

use crate::data::heightmap::OCEAN_ELEVATION;
use crate::data::terrain_mesh::TERRAIN_HEIGHT_SCALE;
use crate::dungeon::DungeonState;
use crate::interior::InteriorState;
use crate::player::{Player, EYE_HEIGHT};

/// Livello del mare in metri (coerente con `sky::update_sky`).
const WATER_Y: f32 = OCEAN_ELEVATION * TERRAIN_HEIGHT_SCALE;
/// Durata della transizione superficie ↔ subacqueo (secondi).
const FADE_SECS: f32 = 0.4;

/// Colore della nebbia subacquea. (Teal leggibile, non nero).
const UNDERWATER_FOG_COLOR: Color = Color::linear_rgba(0.02, 0.13, 0.15, 1.0);
/// Distanze della nebbia lineare subacquea (m).
const UNDERWATER_FOG_START: f32 = 1.0;
const UNDERWATER_FOG_END: f32 = 60.0;      // Esteso per vedere il fondale

/// Colore dell'ambient subacqueo.
const UNDERWATER_AMBIENT_COLOR: Color = Color::srgb(0.0, 0.05, 0.15);
/// Fattore di attenuazione dell'ambient (rispetto a superficie).
const UNDERWATER_AMBIENT_FACTOR: f32 = 0.65;

/// Fattore di attenuazione della directional light.
pub const UNDERWATER_SUN_FACTOR: f32 = 0.45;
/// Colore verso cui spostare la directional light (filtro acqua).
const UNDERWATER_LIGHT_COLOR: Color = Color::srgb(0.35, 0.60, 0.75);

/// Oscillazione della camera (rad). 0.0 = disattivata.
const WATER_WOBBLE_AMPLITUDE: f32 = 0.008;
/// Frequenza dell'oscillazione (rad/s).
const WATER_WOBBLE_FREQ: f32 = 2.5;
/// Soglia di profondità (m) sotto cui si è "sott'acqua".
const DEPTH_THRESHOLD: f32 = 0.05;

#[derive(Resource, Default)]
pub struct UnderwaterState {
    pub is_underwater: bool,
    /// Profondità dell'occhio sotto il livello del mare (m), 0 se sopra.
    pub depth: f32,
    /// 0 = superficie, 1 = pienamente subacqueo.
    pub blend: f32,
}

pub struct UnderwaterPlugin;

impl Plugin for UnderwaterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UnderwaterState>()
            .add_systems(
                Update,
                (
                    update_underwater_state,
                    apply_underwater_effects,
                    underwater_camera_wobble.after(crate::player::player_look),
                )
                    .chain()
                    .after(crate::sky::update_sky)
                    .after(crate::player::player_move),
            );
    }
}

fn update_underwater_state(
    time: Res<Time>,
    interior: Option<Res<InteriorState>>,
    dungeon: Option<Res<DungeonState>>,
    player: Query<&Player>,
    mut state: ResMut<UnderwaterState>,
) {
    let inside = interior.as_ref().is_some_and(|i| i.is_inside())
        || dungeon.as_ref().is_some_and(|d| d.is_inside());

    let depth = if inside {
        0.0
    } else {
        player.get_single().ok().map_or(0.0, |p| {
            let eye_y = p.feet_y + EYE_HEIGHT;
            (WATER_Y - eye_y).max(0.0)
        })
    };

    state.depth = depth;
    state.is_underwater = depth > DEPTH_THRESHOLD;

    let target = if state.is_underwater { 1.0 } else { 0.0 };
    let step = (1.0 / FADE_SECS) * time.delta_secs();
    if (state.blend - target).abs() <= step {
        state.blend = target;
    } else if state.blend < target {
        state.blend += step;
    } else {
        state.blend -= step;
    }
}

/// Falloff originale salvato al primo ingresso in acqua, per ripristinarlo
/// all'uscita. Vive come `Local` in `apply_underwater_effects`.
#[derive(Default)]
pub(crate) struct SavedFalloff {
    falloff: Option<FogFalloff>,
}

pub(crate) fn apply_underwater_effects(
    state: Res<UnderwaterState>,
    mut fogs: Query<&mut DistanceFog, With<Camera3d>>,
    mut ambient: ResMut<AmbientLight>,
    mut suns: Query<&mut DirectionalLight>,
    mut saved: Local<SavedFalloff>,
) {
    let b = state.blend;

    // Fuori dall'acqua: ripristina il falloff originale (se l'avevamo toccato)
    // e non fare nient'altro. `update_sky` ha già scritto i valori di superficie.
    if b <= 0.001 {
        if let Some(orig) = saved.falloff.take() {
            for mut fog in &mut fogs {
                fog.falloff = orig.clone();
            }
        }
        return;
    }

    for mut fog in &mut fogs {
        // Salva il falloff di superficie al primo frame subacqueo.
        if saved.falloff.is_none() {
            saved.falloff = Some(fog.falloff.clone());
        }

        // Colore: mix tra superficie e subacqueo.
        let surface: LinearRgba = fog.color.into();
        let underwater: LinearRgba = UNDERWATER_FOG_COLOR.into();
        fog.color = Color::from(surface.mix(&underwater, b));

        // Falloff lineare subacqueo.
        let (s_start, s_end) = match &saved.falloff {
            Some(FogFalloff::Linear { start, end }) => (*start, *end),
            _ => (500.0, 3000.0),
        };
        fog.falloff = FogFalloff::Linear {
            start: s_start * (1.0 - b) + UNDERWATER_FOG_START * b,
            end: s_end * (1.0 - b) + UNDERWATER_FOG_END * b,
        };
    }

    // Ambient: tint + attenuazione.
    let surface: LinearRgba = ambient.color.into();
    let underwater: LinearRgba = UNDERWATER_AMBIENT_COLOR.into();
    ambient.color = Color::from(surface.mix(&underwater, b));
    ambient.brightness *= 1.0 - b * (1.0 - UNDERWATER_AMBIENT_FACTOR);

    // Sole: attenuato + tint blu.
    for mut sun in &mut suns {
        sun.illuminance *= 1.0 - b * (1.0 - UNDERWATER_SUN_FACTOR);
        let surface: LinearRgba = sun.color.into();
        let underwater: LinearRgba = UNDERWATER_LIGHT_COLOR.into();
        sun.color = Color::from(surface.mix(&underwater, b));
    }
}

/// Piccolo roll/pitch della camera quando sott'acqua, come se si galleggiasse.
/// Gira **dopo** `player_look`, che ogni frame riscrive la rotazione a
/// yaw/pitch puri: il wobble non si accumula.
fn underwater_camera_wobble(
    time: Res<Time>,
    state: Res<UnderwaterState>,
    mut cams: Query<&mut Transform, With<Camera3d>>,
) {
    if state.blend <= 0.001 || WATER_WOBBLE_AMPLITUDE <= 0.0 {
        return;
    }
    let Ok(mut tf) = cams.get_single_mut() else {
        return;
    };
    let t = time.elapsed_secs();
    let amp = WATER_WOBBLE_AMPLITUDE * state.blend;
    let roll = amp * (t * WATER_WOBBLE_FREQ).sin();
    let pitch = amp * 0.7 * (t * WATER_WOBBLE_FREQ * 0.8 + 1.3).cos();
    let (yaw, base_pitch, _) = tf.rotation.to_euler(EulerRot::YXZ);
    tf.rotation = Quat::from_euler(EulerRot::YXZ, yaw, base_pitch + pitch, roll);
}
