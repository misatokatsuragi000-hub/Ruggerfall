//! Giocatore in prima persona: mouse, WASD, salto e volo, con il suolo che
//! segue il terreno e un limite di pendenza.

use crate::tiles::{probe_world, resolve_world, surface_height_world, TileCollision};
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
/// Raggio del corpo per le collisioni con edifici e mura.
const PLAYER_RADIUS: f32 = 0.4;
/// Lunghezza massima di un sotto-passo: evita di attraversare i muri a velocità alta.
const COLLISION_SUBSTEP: f32 = 0.25;
/// Altezza massima di una superficie (tetto, cima di un muro) su cui si sale camminando.
const SURFACE_STEP: f32 = 0.6;

// Scalata (si tiene premuto Q davanti a un muro).
const CLIMB_REACH: f32 = 0.9;
const CLIMB_GAP: f32 = 0.05;
const CLIMB_MIN_HEIGHT: f32 = 0.9;
const CLIMB_SPEED: f32 = 2.5;
const CLIMB_LATERAL_SPEED: f32 = 1.5;
const CLIMB_LEDGE: f32 = 0.5;

#[derive(Component)]
pub struct Player {
    pub yaw: f32,
    pub pitch: f32,
    pub feet_y: f32,
    pub vel_y: f32,
    pub on_ground: bool,
    pub fly: bool,
    pub climbing: bool,
    pub climb_normal: Vec2,
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
            climbing: false,
            climb_normal: Vec2::ZERO,
        }
    }
}

pub fn grab_cursor(
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    equipment: Res<crate::equipment::EquipmentState>,
    dialog: Res<crate::dialog::DialogState>,
    mut was_locked: Local<bool>,
) {
    if equipment.open {
        return;
    }
    let Ok(mut window) = windows.get_single_mut() else { return };
    let now_locked = window.cursor_options.grab_mode == CursorGrabMode::Locked;

    // --- Dialogo aperto: cursore libero, non lockato. ---
    // NON tocchiamo `visible`: ci pensa `custom_cursor` in PostUpdate a
    // nascondere il cursore di sistema e mostrare quello custom.
    if dialog.open {
        if now_locked {
            *was_locked = true;
        }
        window.cursor_options.grab_mode = CursorGrabMode::None;
        return;
    }

    // --- Dialogo chiuso: se eravamo Locked prima, ripristina ---
    if *was_locked {
        *was_locked = false;
        window.cursor_options.grab_mode = CursorGrabMode::Locked;
        window.cursor_options.visible = false;
        return;
    }

    // --- Gioco normale: click sinistro = acquisisce, Esc = rilascia ---
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
    dialog: Res<crate::dialog::DialogState>,
    weapon: Option<Res<crate::fps_weapon::FpsWeaponState>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut player: Query<(&mut Player, &mut Transform)>,
) {
    if dialog.open {
        return;
    }

    // Se l'arma è sguainata e si tiene premuto il tasto sinistro per attaccare,
    // la visuale rimane ferma in stile Daggerfall classico.
    // Quando il tasto NON è premuto, la visuale si muove normalmente.
    if let Some(ref w) = weapon {
        if w.drawn && mouse.pressed(MouseButton::Left) {
            return;
        }
    }

    let Ok(window) = windows.get_single() else { return };
    let Ok((mut p, mut tf)) = player.get_single_mut() else { return };
    if window.cursor_options.grab_mode == CursorGrabMode::Locked {
        p.yaw -= motion.delta.x * MOUSE_SENSITIVITY;
        p.pitch = (p.pitch - motion.delta.y * MOUSE_SENSITIVITY).clamp(-1.5, 1.5);
    }
    tf.rotation = Quat::from_euler(EulerRot::YXZ, p.yaw, p.pitch, 0.0);
}

fn step_allowed(field: &HeightField, origin: &Origin, from: Vec3, delta: Vec3, feet_y: f32) -> bool {
    let to = from + delta;
    let (Some(g_here), Some(g_new)) = (field.ground(origin, from.x, from.z), field.ground(origin, to.x, to.z)) else {
        return false;
    };
    let airborne = feet_y > g_here + 0.3;
    airborne || (g_new - g_here) / delta.length().max(1e-4) <= MAX_SLOPE
}

fn ground_height(
    field: &HeightField, origin: &Origin,
    colliders: &Query<(&TileCollision, &Transform), Without<Player>>,
                 x: f32, z: f32, feet_y: f32,
) -> Option<f32> {
    let terrain = field.ground(origin, x, z)?;
    let surface = surface_height_world(colliders.iter(), Vec2::new(x, z), feet_y + SURFACE_STEP);
    Some(surface.map_or(terrain, |s| s.max(terrain)))
}

#[allow(clippy::too_many_arguments)]
fn climb_update(
    keys: &ButtonInput<KeyCode>, field: &HeightField, origin: &Origin,
    colliders: &Query<(&TileCollision, &Transform), Without<Player>>,
                p: &mut Player, pos: &mut Vec3, forward: Vec3, right: Vec3, dt: f32,
) -> bool {
    let holding = keys.pressed(KeyCode::KeyQ);
    let here = Vec2::new(pos.x, pos.z);

    if !p.climbing {
        if !holding { return false; }
        let facing = Vec2::new(forward.x, forward.z);
        let Some(hit) = probe_world(colliders.iter(), here, facing, CLIMB_REACH, p.feet_y) else { return false };
        if hit.ledge_height(p.feet_y) < CLIMB_MIN_HEIGHT || facing.dot(-hit.normal) < 0.3 { return false; }
        p.climbing = true;
        p.climb_normal = hit.normal;
        p.vel_y = 0.0;
        p.on_ground = false;
    }

    if !holding {
        p.climbing = false;
        return false;
    }
    let toward = -p.climb_normal;
    let Some(hit) = probe_world(colliders.iter(), here, toward, CLIMB_REACH, p.feet_y) else {
        p.climbing = false;
        return false;
    };
    p.climb_normal = hit.normal;

    if keys.just_pressed(KeyCode::Space) {
        p.climbing = false;
        p.vel_y = JUMP_SPEED * 0.7;
        p.on_ground = false;
        pos.x += hit.normal.x * 0.6;
        pos.z += hit.normal.y * 0.6;
        return false;
    }

    let axis = |pos: KeyCode, neg: KeyCode| keys.pressed(pos) as i32 as f32 - keys.pressed(neg) as i32 as f32;
    let vertical = axis(KeyCode::KeyW, KeyCode::KeyS);
    let sideways = axis(KeyCode::KeyD, KeyCode::KeyA);

    let mut tangent = Vec2::new(-hit.normal.y, hit.normal.x);
    if tangent.dot(Vec2::new(right.x, right.z)) < 0.0 { tangent = -tangent; }
    let mut anchor = hit;
    if sideways != 0.0 {
        let try_pos = here + tangent * sideways * CLIMB_LATERAL_SPEED * dt;
        if let Some(h2) = probe_world(colliders.iter(), try_pos, toward, CLIMB_REACH, p.feet_y) {
            anchor = h2;
        }
    }
    let at = anchor.point + anchor.normal * (PLAYER_RADIUS + CLIMB_GAP);
    pos.x = at.x;
    pos.z = at.y;

    p.feet_y += vertical * CLIMB_SPEED * dt;
    if vertical < 0.0 {
        if let Some(ground) = ground_height(field, origin, colliders, pos.x, pos.z, p.feet_y) {
            if p.feet_y <= ground {
                p.feet_y = ground;
                p.climbing = false;
                p.on_ground = true;
                p.vel_y = 0.0;
                return false;
            }
        }
    }

    let top = anchor.y_max;
    if top - p.feet_y < CLIMB_LEDGE {
        let beyond = Vec2::new(pos.x, pos.z) + toward * (PLAYER_RADIUS + 0.6);
        let landing = surface_height_world(colliders.iter(), beyond, top + 1.0)
        .or_else(|| field.ground(origin, beyond.x, beyond.y).filter(|g| *g >= top - 0.5));
        match landing {
            Some(h) => {
                pos.x = beyond.x;
                pos.z = beyond.y;
                p.feet_y = h;
                p.climbing = false;
                p.on_ground = true;
                p.vel_y = 0.0;
                return false;
            }
            None => p.feet_y = p.feet_y.min(top - CLIMB_LEDGE),
        }
    }
    true
}

pub fn player_move(
    dialog: Res<crate::dialog::DialogState>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    field: Res<HeightField>,
    origin: Res<Origin>,
    interior: Res<crate::interior::InteriorState>,
    dungeon: Res<crate::dungeon::DungeonState>,
    colliders: Query<(&TileCollision, &Transform), Without<Player>>,
                   mut player: Query<(&mut Player, &mut Transform)>,
) {
    if dialog.open {
        return;
    }
    if interior.is_inside() || dungeon.is_inside() {
        if let Ok((mut p, _)) = player.get_single_mut() {
            p.climbing = false;
        }
        return;
    }
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
    if !p.fly {
        if climb_update(&keys, &field, &origin, &colliders, &mut p, &mut pos, forward, right, dt) {
            pos.y = p.feet_y + EYE_HEIGHT;
            tf.translation = pos;
            return;
        }
    } else {
        p.climbing = false;
    }
    if p.fly {
        let speed = FLY_SPEED * if running { FLY_RUN_MULTIPLIER } else { 1.0 };
        let vertical = axis(KeyCode::Space, KeyCode::ControlLeft);
        pos += (wish.normalize_or_zero() + Vec3::Y * vertical) * speed * dt;
        p.feet_y = pos.y - EYE_HEIGHT;
    } else {
        let speed = WALK_SPEED * if running { RUN_MULTIPLIER } else { 1.0 };
        let delta = wish.normalize_or_zero() * speed * dt;
        if delta != Vec3::ZERO {
            for candidate in [delta, Vec3::new(delta.x, 0.0, 0.0), Vec3::new(0.0, 0.0, delta.z)] {
                if candidate != Vec3::ZERO && step_allowed(&field, &origin, pos, candidate, p.feet_y) {
                    let steps = ((candidate.length() / COLLISION_SUBSTEP).ceil() as usize).max(1);
                    let sub = candidate / steps as f32;
                    for _ in 0..steps {
                        let next = pos + sub;
                        let fixed = resolve_world(colliders.iter(), Vec2::new(next.x, next.z), p.feet_y, PLAYER_RADIUS);
                        pos.x = fixed.x;
                        pos.z = fixed.y;
                    }
                    break;
                }
            }
        }

        if let Some(ground) = ground_height(&field, &origin, &colliders, pos.x, pos.z, p.feet_y) {
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
