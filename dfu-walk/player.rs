//! Giocatore in prima persona: mouse, WASD, salto e volo, con il suolo che
//! segue il terreno e un limite di pendenza.
//!
//! ## Nave e Nuoto
//!
//! `player_move` fa **early return** quando il giocatore è al timone di una
//! nave. Se il giocatore si trova sotto il livello del mare (`OCEAN_ELEVATION`),
//! entra in modalità "nuoto": la gravità viene sostituita dal galleggiamento,
//! la velocità è ridotta e può usare Spazio (su) e Ctrl (giù) per muoversi
//! verticalmente.

use crate::data::heightmap::OCEAN_ELEVATION;
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
const MAX_SLOPE: f32 = 1.4;
const SNAP_DISTANCE: f32 = 1.0;
const MOUSE_SENSITIVITY: f32 = 0.0022;
const PLAYER_RADIUS: f32 = 0.4;
const COLLISION_SUBSTEP: f32 = 0.25;
const SURFACE_STEP: f32 = 0.6;

// Nuoto
const SWIM_SPEED: f32 = 3.5;
const SWIM_VERTICAL_SPEED: f32 = 4.0;

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
    fast_travel: Option<Res<crate::fast_travel::FastTravelState>>,
    mut was_locked: Local<bool>,
) {
    if equipment.open {
        return;
    }
    if fast_travel.as_ref().is_some_and(|f| f.is_open) {
        return;
    }
    let Ok(mut window) = windows.get_single_mut() else { return };
    let now_locked = window.cursor_options.grab_mode == CursorGrabMode::Locked;

    if dialog.open {
        if now_locked {
            *was_locked = true;
        }
        window.cursor_options.grab_mode = CursorGrabMode::None;
        return;
    }

    if *was_locked {
        *was_locked = false;
        window.cursor_options.grab_mode = CursorGrabMode::Locked;
        window.cursor_options.visible = false;
        return;
    }

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
    fast_travel: Option<Res<crate::fast_travel::FastTravelState>>,
    weapon: Option<Res<crate::fps_weapon::FpsWeaponState>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut player: Query<(&mut Player, &mut Transform)>,
) {
    if dialog.open {
        return;
    }
    if fast_travel.as_ref().is_some_and(|f| f.is_open) {
        return;
    }
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

type ShipQuery<'w, 's> = Query<'w, 's, &'static Transform, (With<crate::ship::Ship>, Without<Player>)>;

#[allow(clippy::too_many_arguments)]
fn ground_height(
    field: &HeightField, origin: &Origin,
    colliders: &Query<(&TileCollision, &Transform), Without<Player>>,
                 ships: &ShipQuery,
                 x: f32, z: f32, feet_y: f32,
) -> Option<f32> {
    let terrain = field.ground(origin, x, z)?;
    let surface = surface_height_world(colliders.iter(), Vec2::new(x, z), feet_y + SURFACE_STEP);
    let mut ground = surface.map_or(terrain, |s| s.max(terrain));
    for ship_tf in ships.iter() {
        if let Some(deck) = crate::ship::deck_height_at(ship_tf, Vec2::new(x, z)) {
            if deck <= feet_y + SURFACE_STEP && deck > ground {
                ground = deck;
            }
        }
    }
    Some(ground)
}

#[allow(clippy::too_many_arguments)]
fn climb_update(
    keys: &ButtonInput<KeyCode>, field: &HeightField, origin: &Origin,
    colliders: &Query<(&TileCollision, &Transform), Without<Player>>,
                ships: &ShipQuery,
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
        if let Some(ground) = ground_height(field, origin, colliders, ships, pos.x, pos.z, p.feet_y) {
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
    fast_travel: Option<Res<crate::fast_travel::FastTravelState>>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    field: Res<HeightField>,
    origin: Res<Origin>,
    interior: Res<crate::interior::InteriorState>,
    dungeon: Res<crate::dungeon::DungeonState>,
    ship_state: Option<Res<crate::ship::ShipState>>,
    colliders: Query<(&TileCollision, &Transform), Without<Player>>,
                   ships: ShipQuery,
                   mut player: Query<(&mut Player, &mut Transform), Without<crate::ship::Ship>>,
) {
    if dialog.open {
        return;
    }
    if fast_travel.as_ref().is_some_and(|f| f.is_open) {
        return;
    }
    if ship_state
        .as_ref()
        .is_some_and(|s| s.player_aboard.is_some())
        {
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

        // Controlla se siamo in acqua
        let in_water = p.feet_y < OCEAN_ELEVATION;

        if !p.fly {
            // Non si può scalare sott'acqua
            if !in_water && climb_update(&keys, &field, &origin, &colliders, &ships, &mut p, &mut pos, forward, right, dt) {
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
            // Se in acqua, si nuota più lentamente e non si corre
            let speed = if in_water {
                SWIM_SPEED
            } else {
                WALK_SPEED * if running { RUN_MULTIPLIER } else { 1.0 }
            };

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

            if let Some(ground) = ground_height(&field, &origin, &colliders, &ships, pos.x, pos.z, p.feet_y) {
                if in_water {
                    // --- LOGICA DI NUOTO ---
                    if keys.pressed(KeyCode::Space) {
                        p.vel_y = SWIM_VERTICAL_SPEED; // Nuota verso l'alto
                    } else if keys.pressed(KeyCode::ControlLeft) {
                        p.vel_y = -SWIM_VERTICAL_SPEED; // Si immerge
                    } else {
                        // Galleggia dolcemente verso la superficie
                        let target_y = OCEAN_ELEVATION - EYE_HEIGHT * 0.8;
                        let diff = target_y - p.feet_y;
                        p.vel_y = (diff * 3.0).clamp(-2.0, 2.0);
                    }

                    p.feet_y += p.vel_y * dt;
                    p.on_ground = false;

                    // Impedisce di sprofondare sotto il fondale marino
                    if p.feet_y < ground {
                        p.feet_y = ground;
                        p.vel_y = 0.0;
                    }
                } else {
                    // --- LOGICA DI TERRAFERMA ---
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
        }
        pos.y = p.feet_y + EYE_HEIGHT;
        tf.translation = pos;
}
