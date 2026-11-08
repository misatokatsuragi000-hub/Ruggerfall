//! Veliero elfico: porting procedurale di `navelfica.html` (Three.js) in
//! Bevy. Il file è diviso in moduli per non superare le 500 righe ciascuno.
//!
//! ## Cosa genera
//!
//! Geometria statica + componenti animati:
//!
//! - **scafo** (`hull.rs`): 19 stazioni `stDefs` → lower hull, deck a 3
//!   strisce, bulwark, mid rail, top rail, stem trim;
//! - **decorazioni** (`decorations.rs`): torri, porte, finestre, merli,
//!   parapetti, gemme, dormer;
//! - **castello di poppa** (`aft_castle.rs`): 4 livelli P1-P4;
//! - **scudi** (`shields.rs`): `createKiteShield`;
//! - **baliste** (`ballista.rs`): `createBallista`;
//! - **timone** (`rudder.rs`): pala + asta + cerniere;
//! - **rostro e polena** (`prow.rs`): rostro a stella, polena femminile;
//! - **timone radiale, vele, bandiere** (`helm_sails.rs`): ruota a 8 razze
//!   (statica), 3 vele triangolari + 3 bandiere a coda di rondine.
//!
//! ## Animazione GPU
//!
//! Le vele e le bandiere usano `SailMaterial` (`sail_material.rs` +
//! `assets/shaders/sail_animator.wgsl`): il vertex shader sposta i vertici in
//! base a `params.time`, aggiornato ogni frame da `update_sail_time`. Costa
//! alla CPU un solo `f32` per materiale per frame.
//!
//! ## Visibilità dei moduli
//!
//! `helm_sails` e `hull` sono **`pub`** perché vengono usati anche da
//! `crate::ship` (per `ElvenAnimatedMaterials`, `spawn_elven_animated_parts`
//! e `hull::ST_DEFS`).

use bevy::prelude::*;

use super::{AnimatedPart, BuiltShip, ElvenMaterial};

mod aft_castle;
mod ballista;
mod decorations;
pub mod helm_sails;
pub mod hull;
mod primitives;
mod prow;
mod rudder;
mod sail_material;
mod shields;

pub use primitives::Part;
pub use sail_material::{SailExtension, SailMaterial, SailMaterialPlugin, SailParams};

/// Costruisce l'intero veliero elfico.
pub fn build() -> BuiltShip {
    let mut builder = primitives::MeshBuilder::new();

    hull::build(&mut builder);
    decorations::build(&mut builder);
    aft_castle::build(&mut builder);
    ballista::build(&mut builder);
    rudder::build(&mut builder);
    prow::build(&mut builder);
    shields::build(&mut builder);
    helm_sails::build_helm_wheel(&mut builder);

    let mut animated: Vec<AnimatedPart> = Vec::new();
    animated.extend(helm_sails::build_sails());
    animated.extend(helm_sails::build_flags());

    builder.finish(animated)
}

// =============================================================================
// Plugin
// =============================================================================

pub struct ElvenShipPlugin;

impl Plugin for ElvenShipPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(SailMaterialPlugin)
        .add_systems(Update, update_sail_time);
    }
}

/// Aggiorna `params.time` su tutte le `SailMaterial` in `Assets`. Costa
/// un `f32` per materiale per frame (nessun ricalcolo di mesh).
///
/// Iterare su `Assets<SailMaterial>` (invece di tenere traccia di handle
/// specifici) è più semplice e gestisce automaticamente il respawn: quando
/// `spawn_elven_ship` crea un nuovo `SailMaterial`, il sistema lo vede al
/// frame successivo.
/// Aggiorna `time`, `wind_strength` e `wind_angle` su tutte le `SailMaterial`.
///
/// - `time`: tempo del gioco in secondi (per l'oscillazione).
/// - `wind_strength`: da `OceanSea.storm` (0 = calmo, 1 = tempesta).
/// - `wind_angle`: da `OceanSea.wind_angle` (direzione reale del vento).
///
/// Costa alla CPU 3 `f32` per materiale per frame.
fn update_sail_time(
    time: Res<Time>,
    sea: Option<Res<crate::ocean::OceanSea>>,
    mut materials: ResMut<Assets<SailMaterial>>,
) {
    let t = time.elapsed_secs();
    let (wind, angle) = match sea.as_ref() {
        Some(s) => (s.storm.clamp(0.0, 1.0), s.wind_angle),
        // Fallback senza oceano: vento moderato che ruota lentamente.
        None => (0.5 + 0.4 * (t * 0.05).sin(), t * 0.02),
    };
    for (_, mat) in materials.iter_mut() {
        mat.extension.params.time = t;
        mat.extension.params.wind_strength = wind;
        mat.extension.params.wind_angle = angle;
    }
}

// =============================================================================
// Materiali statici della nave (invariato)
// =============================================================================

#[derive(Resource)]
pub struct ElvenMaterials {
    pub handles: Vec<Handle<StandardMaterial>>,
}

impl ElvenMaterials {
    pub fn load(materials: &mut Assets<StandardMaterial>, images: &mut Assets<Image>) -> Self {
        let ivory = materials.add(StandardMaterial {
            base_color: Color::srgb(0.988, 0.988, 0.976),
                                  perceptual_roughness: 0.32,
                                  metallic: 0.08,
                                  cull_mode: None,
                                  double_sided: true,
                                  ..default()
        });
        let deck = materials.add(StandardMaterial {
            base_color: Color::srgb(0.839, 0.761, 0.659),
                                 perceptual_roughness: 0.7,
                                 metallic: 0.06,
                                 cull_mode: None,
                                 double_sided: true,
                                 ..default()
        });
        let gold = materials.add(StandardMaterial {
            base_color: Color::srgb(0.851, 0.467, 0.024),
                                 perceptual_roughness: 0.22,
                                 metallic: 0.9,
                                 cull_mode: None,
                                 double_sided: true,
                                 ..default()
        });
        let dark_window = materials.add(StandardMaterial {
            base_color: Color::srgb(0.020, 0.075, 0.141),
                                        perceptual_roughness: 0.12,
                                        metallic: 0.75,
                                        cull_mode: None,
                                        double_sided: true,
                                        ..default()
        });
        let door_wood = materials.add(StandardMaterial {
            base_color: Color::srgb(0.141, 0.086, 0.055),
                                      perceptual_roughness: 0.55,
                                      metallic: 0.25,
                                      ..default()
        });
        let blue_door = materials.add(StandardMaterial {
            base_color: Color::srgb(0.118, 0.227, 0.541),
                                      perceptual_roughness: 0.28,
                                      metallic: 0.35,
                                      ..default()
        });
        let blue_roof = materials.add(StandardMaterial {
            base_color: Color::srgb(0.114, 0.306, 0.847),
                                      perceptual_roughness: 0.32,
                                      metallic: 0.35,
                                      cull_mode: None,
                                      double_sided: true,
                                      ..default()
        });
        let ruby = materials.add(StandardMaterial {
            base_color: Color::srgb(0.882, 0.114, 0.282),
                                 perceptual_roughness: 0.12,
                                 metallic: 0.65,
                                 emissive: LinearRgba::new(0.624, 0.071, 0.224, 1.0),
                                 cull_mode: None,
                                 double_sided: true,
                                 ..default()
        });
        let topaz = materials.add(StandardMaterial {
            base_color: Color::srgb(0.761, 0.255, 0.047),
                                  perceptual_roughness: 0.08,
                                  metallic: 0.45,
                                  emissive: LinearRgba::new(0.486, 0.176, 0.071, 1.0),
                                  cull_mode: None,
                                  double_sided: true,
                                  ..default()
        });
        let handles = vec![
            ivory, deck, gold, dark_window,
            door_wood, blue_door, blue_roof, ruby, topaz,
        ];
        let _ = images;
        Self { handles }
    }

    pub fn handle(&self, m: ElvenMaterial) -> Handle<StandardMaterial> {
        let idx = match m {
            ElvenMaterial::Ivory => 0,
            ElvenMaterial::DeckWood => 1,
            ElvenMaterial::Gold => 2,
            ElvenMaterial::DarkWindow => 3,
            ElvenMaterial::DoorWood => 4,
            ElvenMaterial::BlueDoor => 5,
            ElvenMaterial::BlueRoof => 6,
            ElvenMaterial::Ruby => 7,
            ElvenMaterial::Topaz => 8,
        };
        self.handles[idx].clone()
    }
}
