//! Torcia del giocatore: una `PointLight` che segue la camera.
//!
//! - Di notte, nei dungeon e negli interni si accende automaticamente.
//! - Di giorno all'aperto si spegne (il sole basta).
//! - Il tasto **X** cicla tra tre livelli di intensità:
//!     High (45.000) → Medium (22.000) → Low (11.000) → High → ...
//!   Tutti e tre restano in modalità automatica: cambia solo quanto forte è
//!   la luce quando si accende.
//! - Flickering lieve per un aspetto naturale.

use bevy::prelude::*;

use crate::data::sky::sky_state;
use crate::dungeon::DungeonState;
use crate::interior::InteriorState;
use crate::player::Player;
use crate::sky::SkyClock;

/// Tre livelli di intensità. Il tasto X cicla tra questi.
const LEVEL_HIGH: f32 = 60_000.0;
const LEVEL_MEDIUM: f32 = 30_000.0;
const LEVEL_LOW: f32 = 15_000.0;

/// Raggio d'azione in metri.
const TORCH_RANGE: f32 = 20.0;
/// Colore della fiamma (giallo-arancio caldo).
const TORCH_COLOR: Color = Color::srgb(1.0, 0.72, 0.35);
/// Frequenza base del flickering (Hz).
const FLICKER_FREQ: f32 = 7.0;
/// Ampiezza del flickering (frazione dell'intensità).
const FLICKER_AMP: f32 = 0.10;
/// Durata dell'accensione/spegnimento graduale (secondi).
const FADE_DURATION: f32 = 0.3;
/// Sotto questo valore di `SkyState.day` la torcia si accende automaticamente.
/// 0 = notte piena, 1 = giorno pieno. 0.4 copre anche crepuscolo/alba.
const DAY_THRESHOLD: f32 = 0.4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TorchLevel {
    High,
    Medium,
    Low,
}

impl TorchLevel {
    pub fn intensity(self) -> f32 {
        match self {
            TorchLevel::High => LEVEL_HIGH,
            TorchLevel::Medium => LEVEL_MEDIUM,
            TorchLevel::Low => LEVEL_LOW,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TorchLevel::High => "alta",
            TorchLevel::Medium => "media",
            TorchLevel::Low => "bassa",
        }
    }

    pub fn next(self) -> Self {
        match self {
            TorchLevel::High => TorchLevel::Medium,
            TorchLevel::Medium => TorchLevel::Low,
            TorchLevel::Low => TorchLevel::High,
        }
    }
}

#[derive(Component)]
pub struct TorchLight {
    /// Intensità attuale in lumen (0 = spenta, `level.intensity()` = accesa).
    pub current: f32,
    /// Fase del flickering, aggiornata ogni frame.
    pub phase: f32,
}

#[derive(Resource)]
pub struct TorchState {
    /// Livello selezionato con X. Default: High.
    pub level: TorchLevel,
}

impl Default for TorchState {
    fn default() -> Self {
        Self { level: TorchLevel::High }
    }
}

pub struct TorchPlugin;

impl Plugin for TorchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TorchState>()
        // Tutto in Update: `spawn_torch_light` gira ogni frame ma spawna
        // solo la prima volta che il Player esiste, così non dipende
        // dall'ordine dei sistemi Startup.
        .add_systems(
            Update,
            (spawn_torch_light, torch_cycle, update_torch).chain(),
        );
    }
}

/// Spawna la `PointLight` come figlia della camera/Player, la prima volta
/// che il Player esiste.
fn spawn_torch_light(
    mut commands: Commands,
    player: Query<Entity, With<Player>>,
    mut spawned: Local<bool>,
) {
    if *spawned {
        return;
    }
    let Ok(player_entity) = player.get_single() else {
        return;
    };
    *spawned = true;
    info!("Torch: luce agganciata al giocatore");
    commands.entity(player_entity).with_children(|parent| {
        parent.spawn((
            TorchLight { current: 0.0, phase: 0.0 },
            PointLight {
                color: TORCH_COLOR,
                intensity: 0.0,
                range: TORCH_RANGE,
                shadows_enabled: false,
                ..default()
            },
            // Offset nella mano destra, sotto l'occhio e leggermente avanti.
            // La camera ruota (yaw+pitch), la luce la segue perché figlia.
            Transform::from_xyz(0.35, -0.4, -0.3),
                      // La camera NON ha Visibility: senza Visible esplicito la luce
                      // sarebbe nascosta perché il padre non propaga visibilità.
                      Visibility::Visible,
        ));
    });
}

/// Cicla tra i tre livelli di intensità con X.
fn torch_cycle(keys: Res<ButtonInput<KeyCode>>, mut state: ResMut<TorchState>) {
    if !keys.just_pressed(KeyCode::KeyX) {
        return;
    }
    state.level = state.level.next();
    println!(
        "Torcia: intensità {} ({:.0} lumen, automatica notte/dungeon/interni)",
             state.level.label(),
             state.level.intensity()
    );
}

/// Aggiorna l'intensità della torcia: accesa di notte / in dungeon / in interni
/// al livello selezionato, spenta di giorno all'aperto. Fade graduale e
/// flickering lieve.
fn update_torch(
    time: Res<Time>,
    state: Res<TorchState>,
    clock: Res<SkyClock>,
    interior: Res<InteriorState>,
    dungeon: Res<DungeonState>,
    mut torch: Query<(&mut TorchLight, &mut PointLight)>,
) {
    let dt = time.delta_secs().min(0.1);

    // La torcia si accende solo in queste condizioni (per tutti i livelli).
    let want_on = if interior.is_inside() || dungeon.is_inside() {
        true
    } else {
        sky_state(clock.hours).day < DAY_THRESHOLD
    };

    let target = if want_on { state.level.intensity() } else { 0.0 };

    for (mut t, mut light) in &mut torch {
        // Fade graduale verso il target: quando cambi livello con X, la luce
        // passa al nuovo valore in FADE_DURATION secondi invece di scattare.
        let step = (LEVEL_HIGH / FADE_DURATION) * dt;
        if (t.current - target).abs() <= step {
            t.current = target;
        } else if t.current < target {
            t.current += step;
        } else {
            t.current -= step;
        }

        // Flickering solo quando è (quasi) accesa.
        t.phase += FLICKER_FREQ * dt;
        let flicker = if t.current > 100.0 {
            let a = t.phase.sin() * 0.5
            + (t.phase * 1.7).cos() * 0.3
            + (t.phase * 2.3).sin() * 0.2;
            1.0 + a * FLICKER_AMP
        } else {
            1.0
        };

        light.intensity = t.current * flicker;
        light.color = TORCH_COLOR;
    }
}
