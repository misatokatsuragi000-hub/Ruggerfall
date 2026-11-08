//! Effetti visivi di feedback del combattimento:
//!
//! * **Flash bianco** quando una parata riesce (player che para o nemico che
//!   para). Simmetrico: il feedback è lo stesso in entrambe le direzioni.
//!   Usa due overlay sovrapponibili a schermo intero con alpha che sfuma in
//!   `PARRY_FLASH_DURATION` secondi.
//!
//! * **Numero fluttuante del danno** quando un colpo del player trova un
//!   target valido (NPC civile o nemico dungeon). Il numero è rosso se il
//!   colpo è pieno, arancione se è stato parato (danno ridotto), e fluttua
//!   verso l'alto per `DAMAGE_LIFETIME` secondi sfumando.
//!
//! ## Proiezione mondo → schermo
//!
//! La posizione del target viene proiettata una sola volta al momento del
//! colpo con `Camera::world_to_viewport`. Il numero poi sale in coordinate
//! schermo: se la camera ruota, il numero resta dov'era proiettato (è il
//! comportamento classico dei floating damage, non segue il nemico). Se il
//! target è dietro la camera o fuori schermo, il numero appare al centro.
//!
//! ## Evento `FloatingDamageEvent`
//!
//! Emesso da `combat::apply_player_strike` per ogni colpo del player che
//! colpisce qualcosa. Contiene la posizione mondo del target, il danno
//! effettivo (dopo eventuale parata) e se il colpo è stato parato.
//!
//! ## Eventi ascoltati
//!
//! Il flash bianco **non ha un evento suo**: ascolta direttamente
//! `combat::PlayerParried` (parata del player) e `combat::EnemyParried`
//! (parata del nemico). Il primo è già usato da `play_parry_sound` per il
//! suono, quindi convivono senza problemi (`EventReader` indipendenti).
//!
//! ## Z-order
//!
//! - `ParryFlashOverlay`: `GlobalZIndex(150)`, **sopra** il dialogo NPC
//!   (`GlobalZIndex(100)`) e la mappa del viaggio rapido (`GlobalZIndex(100)`),
//!   **sotto** il cursore custom (`GlobalZIndex(9999)`).
//! - `FloatingDamage`: `GlobalZIndex(140)`, sopra la maggior parte della UI
//!   ma sotto il flash (se un numero è sullo schermo mentre arriva una parata,
//!   il flash lo copre brevemente: è voluto).

use bevy::prelude::*;
use bevy::ui::UiScale;
use bevy::window::PrimaryWindow;

use crate::combat::{EnemyParried, PlayerParried};

// =========================================================================
// Evento
// =========================================================================

/// Emesso da `combat::apply_player_strike` ogni volta che un colpo del player
/// colpisce un target valido (NPC civile o nemico dungeon).
///
/// - `world_pos`: posizione del target **al momento del colpo**, prima che
///   venga eventualmente despawnato (nemico ucciso). Serve a proiettare il
///   numero fluttuante sullo schermo.
/// - `damage`: danno **effettivo** (già ridotto se parato).
/// - `parried`: `true` se il nemico ha parato (danno ridotto). Cambia solo
///   il colore del numero (arancione invece di rosso).
#[derive(Event, Debug, Clone, Copy)]
pub struct FloatingDamageEvent {
    pub world_pos: Vec3,
    pub damage: i32,
    pub parried: bool,
}

// =========================================================================
// Costanti
// =========================================================================

// --- Flash bianco ---
/// Alpha massima del flash. 0.45 è visibile ma non accecante.
const PARRY_FLASH_MAX: f32 = 0.45;
/// Durata del flash: alpha scende linearmente da `PARRY_FLASH_MAX` a 0.
const PARRY_FLASH_DURATION: f32 = 0.22;

// --- Numeri fluttuanti ---
const DAMAGE_FONT_SIZE: f32 = 44.0;
/// Durata della vita del numero (secondi).
const DAMAGE_LIFETIME: f32 = 0.9;
/// Velocità di risalita (px/s).
const DAMAGE_RISE_SPEED: f32 = 90.0;
/// Deriva orizzontale massima (px/s).
const DAMAGE_DRIFT_MAX: f32 = 15.0;
/// Colore del danno pieno (rosso sangue).
const COLOR_DAMAGE: Color = Color::srgb(1.0, 0.18, 0.12);
/// Colore del danno parato (arancione).
const COLOR_DAMAGE_PARRY: Color = Color::srgb(1.0, 0.75, 0.15);

// =========================================================================
// Componenti e risorse
// =========================================================================

#[derive(Component)]
struct ParryFlashOverlay;

/// Componente di ogni numero fluttuante in scena.
#[derive(Component)]
struct FloatingDamage {
    /// Tempo di vita residuo (secondi).
    lifetime: f32,
    /// Velocità (px/s) in coordinate schermo.
    velocity: Vec2,
}

#[derive(Resource, Default)]
struct ParryFlash {
    alpha: f32,
}

// =========================================================================
// Plugin
// =========================================================================

pub struct DamageFxPlugin;

impl Plugin for DamageFxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ParryFlash>()
        .add_event::<FloatingDamageEvent>()
        .add_systems(Startup, setup_parry_flash)
        .add_systems(
            Update,
            (
                handle_parry_flash,
             update_parry_flash,
             spawn_floating_damage,
             update_floating_damage,
            ),
        );
    }
}

// =========================================================================
// Flash bianco
// =========================================================================

fn setup_parry_flash(mut commands: Commands) {
    commands.spawn((
        Name::new("Parry flash overlay"),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
            },
            BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.0)),
            Visibility::Hidden,
            GlobalZIndex(150),
            ParryFlashOverlay,
    ));
}

fn handle_parry_flash(
    mut player_parried: EventReader<PlayerParried>,
    mut enemy_parried: EventReader<EnemyParried>,
    mut flash: ResMut<ParryFlash>,
    mut overlays: Query<&mut Visibility, With<ParryFlashOverlay>>,
) {
    let mut triggered = false;
    // Parata del player: un colpo in arrivo è stato bloccato.
    for _ in player_parried.read() {
        triggered = true;
    }
    // Parata del nemico: il colpo del player è stato parato.
    for _ in enemy_parried.read() {
        triggered = true;
    }
    if !triggered {
        return;
    }
    flash.alpha = PARRY_FLASH_MAX;
    for mut vis in &mut overlays {
        *vis = Visibility::Inherited;
    }
}

fn update_parry_flash(
    time: Res<Time>,
    mut flash: ResMut<ParryFlash>,
    mut overlays: Query<(&mut BackgroundColor, &mut Visibility), With<ParryFlashOverlay>>,
) {
    if flash.alpha <= 0.0 {
        return;
    }
    let decay = PARRY_FLASH_MAX / PARRY_FLASH_DURATION.max(0.01);
    flash.alpha = (flash.alpha - decay * time.delta_secs()).max(0.0);

    for (mut bg, mut vis) in &mut overlays {
        bg.0 = Color::srgba(1.0, 1.0, 1.0, flash.alpha);
        *vis = if flash.alpha > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

// =========================================================================
// Numeri fluttuanti del danno
// =========================================================================

fn spawn_floating_damage(
    mut events: EventReader<FloatingDamageEvent>,
    mut commands: Commands,
    cameras: Query<(&Camera, &GlobalTransform), With<Camera3d>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    ui_scale: Res<UiScale>,
) {
    let Ok((camera, cam_gtf)) = cameras.get_single() else { return };
    let Ok(window) = windows.get_single() else { return };
    let scale = ui_scale.0.max(0.01);
    let win_w = window.width() / scale;
    let win_h = window.height() / scale;

    for ev in events.read() {
        // Proietta la posizione mondo del target sullo schermo.
        let screen = camera.world_to_viewport(cam_gtf, ev.world_pos).ok();
        let (x, y) = match screen {
            Some(p) => (p.x / scale, p.y / scale),
            None => (win_w * 0.5, win_h * 0.45),
        };

        // Se il target è fuori schermo (es. dietro la camera), il numero
        // apparirebbe in un punto strano: lo clampa ai bordi visibili.
        let x = x.clamp(40.0, win_w - 40.0);
        let y = y.clamp(40.0, win_h - 40.0);

        let color = if ev.parried { COLOR_DAMAGE_PARRY } else { COLOR_DAMAGE };

        // Deriva laterale: hash deterministico della posizione, così due
        // colpi ravvicinati non generano sempre lo stesso offset.
        let drift_seed = (x as i32)
        .wrapping_mul(37)
        .wrapping_add((y as i32).wrapping_mul(91));
        let drift = ((drift_seed & 0x1F) as f32 - 15.0) / 15.0 * DAMAGE_DRIFT_MAX;

        commands.spawn((
            Name::new("Floating damage"),
                Text::new(ev.damage.to_string()),
                TextFont {
                    font_size: DAMAGE_FONT_SIZE,
                    ..default()
                },
                TextColor(color),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(x - 20.0),
                top: Val::Px(y - 30.0),
                ..default()
                },
                GlobalZIndex(140),
                FloatingDamage {
                    lifetime: DAMAGE_LIFETIME,
                    velocity: Vec2::new(drift, -DAMAGE_RISE_SPEED),
                },
        ));
    }
}

fn update_floating_damage(
    time: Res<Time>,
    mut commands: Commands,
    mut query: Query<(Entity, &mut FloatingDamage, &mut Node, &mut TextColor)>,
) {
    let dt = time.delta_secs();
    for (entity, mut fd, mut node, mut color) in &mut query {
        fd.lifetime -= dt;
        if fd.lifetime <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }

        // Movimento verticale (e lieve deriva orizzontale).
        if let Val::Px(x) = node.left {
            node.left = Val::Px(x + fd.velocity.x * dt);
        }
        if let Val::Px(y) = node.top {
            node.top = Val::Px(y + fd.velocity.y * dt);
        }

        // Fade out: alpha = (lifetime / max_lifetime)^0.6, così resta
        // leggibile per buona parte della salita e svanisce sul finale.
        let t = (fd.lifetime / DAMAGE_LIFETIME).clamp(0.0, 1.0);
        let alpha = t.powf(0.6);
        let c = color.0.to_srgba();
        color.0 = Color::srgba(c.red, c.green, c.blue, alpha);
    }
}