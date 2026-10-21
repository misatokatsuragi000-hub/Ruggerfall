//! Giocatore in prima persona: mouse, WASD, salto e volo, con il suolo che
//! segue il terreno e un limite di pendenza.

use crate::world::*;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, PrimaryWindow};

pub const EYE_HEIGHT: f32 = 1.7;
const WALK_SPEED: f32 = 7.0;
const RUN_MULTIPLIER: f32 = 3.5;
const FLY_SPEED: f32 = 150.0;
const FLY_RUN_MULTIPLIER: f32 = 5.0;
const GRAVITY: f32 = 25.0;
const JUMP_SPEED: f32 = 7.0;
const MAX_FALL_SPEED: f32 = 60.0;
/// Pendenza massima percorribile (rapporto salita/avanzamento, circa 55 gradi).
const MAX_SLOPE: f32 = 1.4;
/// Se si è a meno di questa altezza dal suolo si resta incollati (discese).
const SNAP_DISTANCE: f32 = 1.0;
const MOUSE_SENSITIVITY: f32 = 0.0022;

#[derive(Component)]
pub struct Player {
    pub yaw: f32,
    pub pitch: f32,
    pub feet_y: f32,
    pub vel_y: f32,
    pub on_ground: bool,
    pub fly: bool,
}

impl Player {
    pub fn new(feet_y: f32) -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            feet_y,
            vel_y: 0.0,
            on_ground: true,
            fly: false,
        }
    }
}

pub fn grab_cursor(
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let Ok(mut window) = windows.get_single_mut() else { return };
    if mouse.just_pressed(MouseButton::Left) {
        window.cursor_options.grab_mode = CursorGrabMode::Locked;
        window.cursor_options.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) {
        window.cursor_options.grab_mode = CursorGrabMode::None;
        window.cursor_options.visible = true;
    }
}

pub fn player_look(
    motion: Res<AccumulatedMouseMotion>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut player: Query<(&mut Player, &mut Transform)>,
) {
    let Ok(window) = windows.get_single() else { return };
    let Ok((mut p, mut tf)) = player.get_single_mut() else { return };
    if window.cursor_options.grab_mode == CursorGrabMode::Locked {
        p.yaw -= motion.delta.x * MOUSE_SENSITIVITY;
        p.pitch = (p.pitch - motion.delta.y * MOUSE_SENSITIVITY).clamp(-1.5, 1.5);
    }
    tf.rotation = Quat::from_euler(EulerRot::YXZ, p.yaw, p.pitch, 0.0);
}

/// Un passo orizzontale è consentito se la salita non supera la pendenza massima
/// e non porta fuori dai map pixel caricati. In volo o in aria non ci sono limiti.
fn step_allowed(field: &HeightField, origin: &Origin, from: Vec3, delta: Vec3, feet_y: f32) -> bool {
    let to = from + delta;
    let (Some(g_here), Some(g_new)) = (field.ground(origin, from.x, from.z), field.ground(origin, to.x, to.z)) else {
        return false;
    };
    let airborne = feet_y > g_here + 0.3;
    airborne || (g_new - g_here) / delta.length().max(1e-4) <= MAX_SLOPE
}

pub fn player_move(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    field: Res<HeightField>,
    origin: Res<Origin>,
    mut player: Query<(&mut Player, &mut Transform)>,
) {
    let Ok((mut p, mut tf)) = player.get_single_mut() else { return };
    let dt = time.delta_secs().min(0.1);

    if keys.just_pressed(KeyCode::KeyF) {
        p.fly = !p.fly;
        p.vel_y = 0.0;
    }

    let forward = Vec3::new(-p.yaw.sin(), 0.0, -p.yaw.cos());
    let right = Vec3::new(p.yaw.cos(), 0.0, -p.yaw.sin());
    let axis = |pos: KeyCode, neg: KeyCode| keys.pressed(pos) as i32 as f32 - keys.pressed(neg) as i32 as f32;
    let wish = forward * axis(KeyCode::KeyW, KeyCode::KeyS) + right * axis(KeyCode::KeyD, KeyCode::KeyA);
    let running = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

    let mut pos = tf.translation;
    if p.fly {
        let speed = FLY_SPEED * if running { FLY_RUN_MULTIPLIER } else { 1.0 };
        let vertical = axis(KeyCode::Space, KeyCode::ControlLeft);
        pos += (wish.normalize_or_zero() + Vec3::Y * vertical) * speed * dt;
        p.feet_y = pos.y - EYE_HEIGHT;
    } else {
        let speed = WALK_SPEED * if running { RUN_MULTIPLIER } else { 1.0 };
        let delta = wish.normalize_or_zero() * speed * dt;
        if delta != Vec3::ZERO {
            // Prima il movimento intero, poi scorrimento lungo un solo asse.
            for candidate in [delta, Vec3::new(delta.x, 0.0, 0.0), Vec3::new(0.0, 0.0, delta.z)] {
                if candidate != Vec3::ZERO && step_allowed(&field, &origin, pos, candidate, p.feet_y) {
                    pos += candidate;
                    break;
                }
            }
        }

        // Verticale: gravità e salto, ma solo dove il terreno è caricato.
        if let Some(ground) = field.ground(&origin, pos.x, pos.z) {
            if keys.just_pressed(KeyCode::Space) && p.on_ground {
                p.vel_y = JUMP_SPEED;
                p.on_ground = false;
            }
            if p.on_ground && p.feet_y - ground <= SNAP_DISTANCE && p.vel_y <= 0.0 {
                p.feet_y = ground;
                p.vel_y = 0.0;
            } else {
                p.vel_y = (p.vel_y - GRAVITY * dt).max(-MAX_FALL_SPEED);
                p.feet_y += p.vel_y * dt;
                p.on_ground = false;
                if p.feet_y <= ground {
                    p.feet_y = ground;
                    p.vel_y = 0.0;
                    p.on_ground = true;
                }
            }
        }
    }
    pos.y = p.feet_y + EYE_HEIGHT;
    tf.translation = pos;
}
