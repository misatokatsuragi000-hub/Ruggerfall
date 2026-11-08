//! Mappa 3D del dungeon (tasto L dentro un dungeon): camera orbitante + luce.
//!
//! Non crea nessuna mesh copia: sposta il `Transform` del player (che in questo
//! gioco *è* la camera) su un'orbita circolare di `ORBIT_DISTANCE` metri intorno
//! alla posizione reale del player (salvata in `MapOrbit.center`). Ogni frame
//! la camera si riposiziona a `center - forward * distance` con `forward`
//! calcolato da yaw/pitch, poi guarda il centro.
//!
//! ## Perché funziona senza toccare gli altri sistemi
//!
//! `dialog.open = true` blocca `player_look`, `player_move`, `dungeon_move`,
//! `interior_move` e `update_dialog` (tutti hanno un `if dialog.open { return; }`).
//! Quindi:
//!
//! - il player non si muove (il Transform è quello orbitale);
//! - `p.yaw`/`p.pitch` non vengono toccati (li salvo e ripristino);
//! - `AccumulatedMouseMotion` è libero per l'orbita;
//! - WASD muove solo la camera (nei sistemi di orbita, non in `player_move`).
//!
//! I due sistemi che **non** controllano `dialog.open` e vanno patchati per
//! non far muovere nemici e follower verso la camera in aria:
//! - `dungeon_enemies::update_dungeon_enemies`
//! - `tiles::update_followers`
//!
//! ## Luce
//!
//! Dentro un dungeon la `SunLight` (DirectionalLight globale) è ferma
//! all'ultima posizione di superficie e le `PointLight` del dungeon hanno
//! range breve (8-18 m). A 20+ m di distanza non illuminano il pavimento.
//! Per questo, mentre la mappa è aperta:
//!
//! - la `SunLight` viene nascosta (`Visibility::Hidden`), ripristinata alla
//!   chiusura;
//! - viene spawnata una `DirectionalLight` dedicata (`DungeonMapLight`) che
//!   punta dritto in basso con illuminance alta, così il pavimento attorno al
//!   player è sempre visibile a qualunque distanza.
//!
//! ## Controlli
//!
//! - **Mouse** o **A/D** o **←/→** = yaw
//! - **Mouse** o **W/S** o **↑/↓** = pitch (clampato: non passa sotto il player)
//! - **Rotella** = distanza (2..80 m, default 20 m)
//! - **L** o **Esc** = chiudi
//!
//! ## Limiti noti
//!
//! - La camera **attraversa i muri**: da 20 m di distanza è spesso fuori
//!   dalla stanza. Ruota intorno al player con il mouse, o riduci la distanza
//!   con la rotella, per vederla dall'interno.

use bevy::input::mouse::{AccumulatedMouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, PrimaryWindow};

use crate::dialog::DialogState;
use crate::dungeon::DungeonState;
use crate::player::{Player, EYE_HEIGHT};
use crate::sky::SunLight;

/// Distanza iniziale della camera dal player (metri).
const ORBIT_DISTANCE: f32 = 20.0;
const ORBIT_DISTANCE_MIN: f32 = 2.0;
const ORBIT_DISTANCE_MAX: f32 = 80.0;

/// Pitch iniziale (radiante). -0.7 ≈ -40°: la camera sta sopra e dietro al
/// player e guarda il pavimento intorno a lui.
const ORBIT_PITCH_DEFAULT: f32 = -0.7;
const ORBIT_PITCH_MIN: f32 = -1.45;
const ORBIT_PITCH_MAX: f32 = 0.3;

/// Sensibilità mouse (radianti per pixel).
const ORBIT_MOUSE_SENS: f32 = 0.005;
/// Velocità rotazione con i tasti (radianti/sec).
const ORBIT_YAW_KEY_SPEED: f32 = 1.2;
const ORBIT_PITCH_KEY_SPEED: f32 = 0.8;
/// Fattore di zoom per tacca di rotella (applicato con `.powf(scroll)`).
const ORBIT_ZOOM_STEP: f32 = 1.30;

/// Illuminance della DirectionalLight temporanea (lux).
const MAP_LIGHT_ILLUMINANCE: f32 = 35_000.0;

#[derive(Clone, Copy)]
struct SavedPlayer {
    translation: Vec3,
    rotation: Quat,
    feet_y: f32,
    yaw: f32,
    pitch: f32,
    vel_y: f32,
    on_ground: bool,
}

#[derive(Resource, Default)]
struct MapOrbit {
    open: bool,
    saved: Option<SavedPlayer>,
    /// Centro dell'orbita = posizione reale del player al momento dell'apertura.
    center: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    /// Direzione verso cui punta la DirectionalLight spawnata.
    map_light_dir: Vec3,
    /// Entità della DirectionalLight spawnata.
    light_entity: Option<Entity>,
    /// Visibilità della SunLight prima dell'apertura.
    saved_sun_vis: Option<Visibility>,
}

#[derive(Component)]
struct DungeonMapLight;

pub struct DungeonMapPlugin;

impl Plugin for DungeonMapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapOrbit>()
        .add_systems(
            Update,
            (
                toggle_dungeon_map,
             update_orbit_input,
             apply_orbit,
             apply_map_state,
             force_close_on_exit,
            )
            .chain()
            // `player_look` e `player_move` devono girare DOPO il
            // nostro `apply_orbit`, così la loro eventuale rotazione
            // viene sovrascritta. In pratica fanno già `return` perché
            // `dialog.open` è true, ma è una rete di sicurezza.
            .before(crate::player::player_look)
            .before(crate::player::player_move)
            .before(crate::dungeon::dungeon_move)
            .before(crate::dungeon::dungeon_enter_exit),
        );
    }
}

// =============================================================================
// Apertura / chiusura
// =============================================================================

#[allow(clippy::too_many_arguments)]
fn toggle_dungeon_map(
    keys: Res<ButtonInput<KeyCode>>,
    dungeon: Res<DungeonState>,
    mut state: ResMut<MapOrbit>,
    mut commands: Commands,
    mut player: Query<(&mut Player, &mut Transform), Without<DungeonMapLight>>,
                      mut sun: Query<(&SunLight, &mut Visibility), Without<DungeonMapLight>>,
) {
    let toggle = if state.open {
        keys.just_pressed(KeyCode::KeyL) || keys.just_pressed(KeyCode::Escape)
    } else {
        keys.just_pressed(KeyCode::KeyL) && dungeon.is_inside()
    };
    if !toggle {
        return;
    }
    let Ok((mut p, mut tf)) = player.get_single_mut() else {
        return;
    };

    // --- CHIUDI ---
    if state.open {
        if let Some(saved) = state.saved.take() {
            tf.translation = saved.translation;
            tf.rotation = saved.rotation;
            p.feet_y = saved.feet_y;
            p.yaw = saved.yaw;
            p.pitch = saved.pitch;
            p.vel_y = saved.vel_y;
            p.on_ground = saved.on_ground;
        }
        if let Some(e) = state.light_entity.take() {
            commands.entity(e).despawn_recursive();
        }
        if let Some(vis) = state.saved_sun_vis.take() {
            for (_, mut v) in &mut sun {
                *v = vis;
            }
        }
        state.open = false;
        info!("Mappa dungeon chiusa");
        return;
    }

    // --- APRI ---
    state.saved = Some(SavedPlayer {
        translation: tf.translation,
        rotation: tf.rotation,
        feet_y: p.feet_y,
        yaw: p.yaw,
        pitch: p.pitch,
        vel_y: p.vel_y,
        on_ground: p.on_ground,
    });
    state.center = tf.translation;
    state.yaw = 0.0;
    state.pitch = ORBIT_PITCH_DEFAULT;
    state.distance = ORBIT_DISTANCE;

    // Direzione della luce: dritto in basso + leggerissimo tilt per non avere
    // una normale ambigua. Sparata sopra il centro dell'orbita.
    state.map_light_dir = Vec3::new(0.0, -1.0, -0.05).normalize();
    let light_entity = commands
    .spawn((
        DungeonMapLight,
        DirectionalLight {
            illuminance: MAP_LIGHT_ILLUMINANCE,
            shadows_enabled: false,
            color: Color::srgb(1.0, 0.95, 0.85),
            ..default()
        },
        Transform::from_translation(state.center + Vec3::Y * 100.0)
        .looking_to(state.map_light_dir, Vec3::Y),
    ))
    .id();
    state.light_entity = Some(light_entity);

    // Nascondi la SunLight (se esiste) e ricorda la visibilità precedente.
    for (_, mut v) in &mut sun {
        state.saved_sun_vis = Some(*v);
        *v = Visibility::Hidden;
    }

    state.open = true;
    p.vel_y = 0.0;
    p.on_ground = false;

    info!(
        "Mappa dungeon aperta: centro ({:.1}, {:.1}, {:.1}), distanza {:.1} m",
          state.center.x, state.center.y, state.center.z, state.distance
    );
}

// =============================================================================
// Input: yaw / pitch / zoom
// =============================================================================

fn update_orbit_input(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    mut wheel: EventReader<MouseWheel>,
    mut state: ResMut<MapOrbit>,
) {
    if !state.open {
        wheel.clear();
        return;
    }
    let dt = time.delta_secs();

    // Yaw.
    let mut yaw_in = 0.0f32;
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        yaw_in -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        yaw_in += 1.0;
    }
    // Pitch.
    let mut pitch_in = 0.0f32;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        pitch_in += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        pitch_in -= 1.0;
    }

    state.yaw += yaw_in * ORBIT_YAW_KEY_SPEED * dt + motion.delta.x * ORBIT_MOUSE_SENS;
    state.pitch = (state.pitch
    + pitch_in * ORBIT_PITCH_KEY_SPEED * dt
    + motion.delta.y * ORBIT_MOUSE_SENS)
    .clamp(ORBIT_PITCH_MIN, ORBIT_PITCH_MAX);

    // Zoom con la rotella. `.powf(scroll)` gestisce sia mouse (una tacca = 1.0)
    // sia touchpad (molte tacche piccole): il risultato finale è coerente.
    let mut scroll: f32 = wheel.read().map(|e| e.y).sum();
    // Normalizza i touchpad che emettono valori frazionari: sotto 0.3 lo
    // tratto come una tacca singola.
    if scroll.abs() > 0.01 && scroll.abs() < 0.3 {
        scroll = scroll.signum();
    }
    if scroll.abs() >= 0.01 {
        let factor = ORBIT_ZOOM_STEP.powf(scroll);
        state.distance = (state.distance * factor)
        .clamp(ORBIT_DISTANCE_MIN, ORBIT_DISTANCE_MAX);
    }
}

// =============================================================================
// Posizionamento camera
// =============================================================================

fn apply_orbit(
    state: Res<MapOrbit>,
    mut player: Query<(&mut Player, &mut Transform)>,
) {
    if !state.open {
        return;
    }
    let Ok((_p, mut tf)) = player.get_single_mut() else {
        return;
    };

    // Direzione di sguardo: parte da -Z (avanti Bevy) ruotata di yaw+pitch.
    let rotation = Quat::from_euler(EulerRot::YXZ, state.yaw, state.pitch, 0.0);
    let forward = rotation * Vec3::NEG_Z;

    // Camera = centro - forward * distanza → guarda il centro.
    tf.translation = state.center - forward * state.distance;
    tf.rotation = rotation;

    // `p.yaw`/`p.pitch` restano quelli salvati: non li tocchiamo perché
    // `player_look` fa `return` (dialog.open = true) e comunque li
    // ripristiniamo alla chiusura. `feet_y` non viene letto da nessuno
    // mentre la mappa è aperta.
    let _ = EYE_HEIGHT;
}

// =============================================================================
// DialogState / cursore / soppressione tasti
// =============================================================================

fn apply_map_state(
    state: Res<MapOrbit>,
    mut dialog: ResMut<DialogState>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut last: Local<Option<bool>>,
) {
    let prev = *last;
    *last = Some(state.open);
    let Ok(mut window) = windows.get_single_mut() else {
        return;
    };

    if state.open {
        dialog.open = true;
        dialog.forced_by_menu = true;
        window.cursor_options.grab_mode = CursorGrabMode::None;
        window.cursor_options.visible = true;

        // Sopprime i tasti "azione" (E, Backspace, Spazio, Z, X, F) così non
        // attivano porte/armi/salto/volo mentre sei in mappa. L e Escape sono
        // esclusi: chiudono.
        for k in [
            KeyCode::KeyE,
            KeyCode::Backspace,
            KeyCode::Space,
            KeyCode::KeyZ,
            KeyCode::KeyX,
            KeyCode::KeyF,
        ] {
            keys.clear_just_pressed(k);
        }
    } else if prev == Some(true) {
        dialog.open = false;
        dialog.forced_by_menu = false;
        window.cursor_options.grab_mode = CursorGrabMode::Locked;
        window.cursor_options.visible = false;
    }
}

fn force_close_on_exit(
    dungeon: Res<DungeonState>,
    mut state: ResMut<MapOrbit>,
    mut commands: Commands,
    mut player: Query<(&mut Player, &mut Transform), Without<DungeonMapLight>>,
                       mut sun: Query<(&SunLight, &mut Visibility), Without<DungeonMapLight>>,
) {
    if !state.open || dungeon.is_inside() {
        return;
    }
    // Uscita dal dungeon con la mappa aperta: ripristina tutto.
    let Ok((mut p, mut tf)) = player.get_single_mut() else {
        return;
    };
    if let Some(saved) = state.saved.take() {
        tf.translation = saved.translation;
        tf.rotation = saved.rotation;
        p.feet_y = saved.feet_y;
        p.yaw = saved.yaw;
        p.pitch = saved.pitch;
        p.vel_y = saved.vel_y;
        p.on_ground = saved.on_ground;
    }
    if let Some(e) = state.light_entity.take() {
        commands.entity(e).despawn_recursive();
    }
    if let Some(vis) = state.saved_sun_vis.take() {
        for (_, mut v) in &mut sun {
            *v = vis;
        }
    }
    state.open = false;
    info!("Mappa dungeon chiusa (uscita dal dungeon)");
}
