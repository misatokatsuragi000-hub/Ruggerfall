//! Torcia del giocatore: una `PointLight` che segue la camera.
//!
//! - Di notte, nei dungeon e negli interni si accende automaticamente.
//! - Di giorno all'aperto si spegne (il sole basta), a meno che `TorchTuning::force_on` sia attivo.
//! - Il tasto **X** cicla tra tre livelli di intensità:
//!     High (60.000) → Medium (30.000) → Low (15.000) → High → ...
//! - Flickering lieve per un aspetto naturale.
//!
//! ## Tuning (menu F1, scheda "Torcia")
//!
//! La risorsa [`TorchTuning`] permette di sovrascrivere intensità, range, colore,
//! flickering e di forzare l'accensione. Il tasto **X** continua a funzionare: cambia
//! solo lo stato (`TorchState`), e l'override di intensità, se presente, ha la precedenza
//! sul valore del livello.

use bevy::prelude::*;

use crate::data::sky::sky_state;
use crate::dungeon::DungeonState;
use crate::interior::InteriorState;
use crate::player::Player;
use crate::sky::SkyClock;

/// Tre livelli di intensità. Il tasto X cicla tra questi.
pub const LEVEL_HIGH: f32 = 60_000.0;
pub const LEVEL_MEDIUM: f32 = 30_000.0;
pub const LEVEL_LOW: f32 = 15_000.0;

/// Raggio d'azione in metri (default).
pub const TORCH_RANGE: f32 = 20.0;
/// Colore della fiamma (giallo-arancio caldo).
pub const TORCH_COLOR: Color = Color::srgb(1.0, 0.72, 0.35);
/// Frequenza base del flickering (Hz).
pub const FLICKER_FREQ: f32 = 7.0;
/// Ampiezza del flickering (frazione dell'intensità).
pub const FLICKER_AMP: f32 = 0.10;
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

/// Sovrascritture dei valori fissi della torcia, regolabili dal menu F1.
/// Se i campi `*_override` sono `None` si usano le costanti del modulo.
#[derive(Resource, Debug, Clone)]
pub struct TorchTuning {
    /// Se `Some`, sostituisce `level.intensity()` (il tasto X resta attivo ma
    /// cambia solo lo stato, non l'intensità, finché questo override è attivo).
    pub intensity_override: Option<f32>,
    /// Se `Some`, sostituisce `TORCH_RANGE`.
    pub range_override: Option<f32>,
    /// Se `Some`, sostituisce `TORCH_COLOR`.
    pub color_override: Option<Color>,
    /// Se `true`, la torcia resta accesa indipendentemente da ora/luogo.
    /// Utile per testare l'effetto su un muro di giorno all'aperto.
    pub force_on: bool,
    /// Ampiezza del flickering (0.0 = nessuno, 0.10 = default).
    pub flicker_amp: f32,
    /// Frequenza del flickering in Hz (0.0 = fermo, 7.0 = default).
    pub flicker_freq: f32,
}

impl Default for TorchTuning {
    fn default() -> Self {
        Self {
            intensity_override: None,
            range_override: None,
            color_override: None,
            force_on: false,
                flicker_amp: FLICKER_AMP,
                flicker_freq: FLICKER_FREQ,
        }
    }
}

/// Caricamento/salvataggio di `TorchTuning` su file di testo.
/// Formato: righe `chiave = valore`, `#` per i commenti.
impl TorchTuning {
    pub fn from_text(text: &str) -> Self {
        let mut t = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "intensity_override" => {
                    t.intensity_override = if v.eq_ignore_ascii_case("none") || v.is_empty() {
                        None
                    } else {
                        v.parse().ok()
                    };
                }
                "range_override" => {
                    t.range_override = if v.eq_ignore_ascii_case("none") || v.is_empty() {
                        None
                    } else {
                        v.parse().ok()
                    };
                }
                "color_r" | "color_g" | "color_b" => {
                    let srgba = t
                    .color_override
                    .unwrap_or(TORCH_COLOR)
                    .to_srgba();
                    let (r, g, b) = match k {
                        "color_r" => (v.parse().unwrap_or(srgba.red), srgba.green, srgba.blue),
                        "color_g" => (srgba.red, v.parse().unwrap_or(srgba.green), srgba.blue),
                        _ => (srgba.red, srgba.green, v.parse().unwrap_or(srgba.blue)),
                    };
                    t.color_override = Some(Color::srgb(r, g, b));
                }
                "force_on" => {
                    t.force_on = v == "1" || v.eq_ignore_ascii_case("true");
                }
                "flicker_amp" => t.flicker_amp = v.parse().unwrap_or(FLICKER_AMP),
                "flicker_freq" => t.flicker_freq = v.parse().unwrap_or(FLICKER_FREQ),
                _ => {}
            }
        }
        t
    }

    pub fn to_text(&self) -> String {
        let mut s = String::from("# Parametri della torcia (menu F1)\n");
        s.push_str(&format!(
            "intensity_override = {}\n",
            self.intensity_override
            .map_or_else(|| "none".to_string(), |v| v.to_string())
        ));
        s.push_str(&format!(
            "range_override = {}\n",
            self.range_override
            .map_or_else(|| "none".to_string(), |v| v.to_string())
        ));
        if let Some(c) = self.color_override {
            let srgba = c.to_srgba();
            s.push_str(&format!(
                "color_r = {}\ncolor_g = {}\ncolor_b = {}\n",
                srgba.red, srgba.green, srgba.blue
            ));
        }
        s.push_str(&format!("force_on = {}\n", self.force_on as u32));
        s.push_str(&format!("flicker_amp = {}\n", self.flicker_amp));
        s.push_str(&format!("flicker_freq = {}\n", self.flicker_freq));
        s
    }

    pub fn load_or_default(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
        .map(|t| Self::from_text(&t))
        .unwrap_or_default()
    }
}

pub struct TorchPlugin;

impl Plugin for TorchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TorchState>()
        .init_resource::<TorchTuning>()
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
/// al livello selezionato, spenta di giorno all'aperto (salvo `force_on`).
/// Fade graduale e flickering regolabili via `TorchTuning`.
fn update_torch(
    time: Res<Time>,
    state: Res<TorchState>,
    tuning: Res<TorchTuning>,
    clock: Res<SkyClock>,
    interior: Res<InteriorState>,
    dungeon: Res<DungeonState>,
    mut torch: Query<(&mut TorchLight, &mut PointLight)>,
) {
    let dt = time.delta_secs().min(0.1);

    // Intensità obiettivo: override se presente, altrimenti il livello (X).
    let level_intensity = tuning
    .intensity_override
    .unwrap_or_else(|| state.level.intensity());

    // La torcia si accende in interni/dungeon o di notte, oppure sempre se
    // `force_on` è attivo.
    let want_on = tuning.force_on
    || interior.is_inside()
    || dungeon.is_inside()
    || sky_state(clock.hours).day < DAY_THRESHOLD;

    let target = if want_on { level_intensity } else { 0.0 };

    // Il fade usa il massimo tra il livello corrente e il livello "High"
    // standard: così anche con override molto bassi la transizione resta
    // fluida (non scattosa) e non dipende dai livelli fissi.
    let peak_for_fade = level_intensity.max(LEVEL_HIGH);
    let step = (peak_for_fade / FADE_DURATION) * dt;

    for (mut t, mut light) in &mut torch {
        // Fade graduale verso il target.
        if (t.current - target).abs() <= step {
            t.current = target;
        } else if t.current < target {
            t.current += step;
        } else {
            t.current -= step;
        }

        // Flickering solo quando è (quasi) accesa e se l'ampiezza è > 0.
        t.phase += tuning.flicker_freq * dt;
        let flicker = if t.current > 100.0 && tuning.flicker_amp > 0.0 {
            let a = t.phase.sin() * 0.5
            + (t.phase * 1.7).cos() * 0.3
            + (t.phase * 2.3).sin() * 0.2;
            1.0 + a * tuning.flicker_amp
        } else {
            1.0
        };

        light.intensity = t.current * flicker;
        light.color = tuning.color_override.unwrap_or(TORCH_COLOR);
        if let Some(r) = tuning.range_override {
            light.range = r.max(0.5);
        } else {
            light.range = TORCH_RANGE;
        }
    }
}
