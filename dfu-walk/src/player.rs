//! Giocatore in prima persona: mouse, WASD, salto e volo, con il suolo che
//! segue il terreno e un limite di pendenza.
//!
//! ## Nave e Nuoto
//!
//! `player_move` fa **early return** quando il giocatore è al timone di una
//! nave. Se il giocatore si trova sotto la superficie dell'acqua
//! (`OCEAN_ELEVATION * TERRAIN_HEIGHT_SCALE + wave_height_at(...)`), entra in
//! modalità "nuoto": la gravità viene sostituita dal galleggiamento verso la
//! superficie dell'onda, la velocità è ridotta e può usare Spazio (su) e Ctrl
//! (giù) per muoversi verticalmente.
//!
//! ## Scalata e auto-step (quattro funzioni separate)
//!
//! `player_move` invoca, in ordine:
//!
//! 1. **`try_step_up`** — auto-step su piccoli rilievi **di città** (gradini,
//!    dossi, bordi di marciapiede). Se il player è a terra e ha davanti un
//!    muro di città alto al massimo `STEP_UP_HEIGHT` (70 cm), ci sale sopra
//!    automaticamente, **senza premere Q**.
//! 2. **`try_step_up_ship`** — come sopra, ma **sulla nave**. Fallback per
//!    scale con gradini più alti di `STEP_UP_HEIGHT` del character controller
//!    (`ship.rs::STEP_HEIGHT = 0.8`). Non è necessario nella maggior parte
//!    dei casi (l'autostep di Rapier fa il lavoro), ma evita che il player
//!    si blocchi su qualche scala modellata male.
//! 3. **`try_climb_ship`** — scalata sulla **nave** con **Q** premuto:
//!    `probe_ship_rapier` (Rapier) con fallback CPU `probe_ship`.
//! 4. **`try_climb_city`** — scalata sui **muri di città** con **Q** premuto:
//!    `probe_world` (triangolare, non più segment-based).
//!
//! Ognuna ritorna `true` se ha gestito il movimento; `player_move` esce
//! subito in quel caso. `ClimbTarget` ricorda su cosa è agganciato il
//! player, così le due scalate non si rubano l'aggancio a vicenda.
//!
//! ## Scalata multi-height (più permissiva)
//!
//! Prima il probe di scalata testava un solo raggio all'altezza dell'occhio
//! (`feet_y + 0.9`): qualunque sporgenza che esisteva solo in basso
//! (cornicione, davanzale) o in alto (balcone, trave) veniva ignorata, e
//! `CLIMB_REACH = 0.9` con `PLAYER_RADIUS = 0.4` lasciava fuori ogni
//! sporgenza più profonda di ~0.5 m oltre il muro principale.
//!
//! Ora:
//!
//! - `probe_climb_multi` testa **tre altezze** (`CLIMB_PROBE_HEIGHTS` =
//!   0.3 / 0.9 / 1.6 m sopra i piedi) e sceglie l'hit con `y_max` più alto
//!   (il muro più scalabile fra quelli colpiti);
//! - `CLIMB_REACH` è salito a **1.6 m** (copre una sporgenza di ~0.7 m
//!   oltre il muro con player incollato) e il `tiles::Colliders::probe`
//!   corrispondente usa `PROBE_MAX_REACH = 2.0` per non troncare il raggio;
//! - `CLIMB_MIN_HEIGHT` è scesa a **0.3 m** (una sporgenza di 30 cm basta);
//! - `CLIMB_LEDGE` è salita a **0.7 m** (accetta anche sporgenze che stanno
//!   un po' più in alto del solito);
//! - la soglia di "frontale" è scesa da `0.3` a `0.2` (accetta spigoli di
//!   cornici leggermente inclinati).
//!
//! `probe_ship`/`probe_ship_rapier` in `ship.rs` sono state aggiornate con
//! lo stesso schema multi-height, così la scalata nave si comporta come
//! quella città.
//!
//! ## Comportamento della scalata (Q + Spazio)
//!
//! - **Q premuto**: si scala (W/S su-giù, A/D di lato) e ci si aggancia al
//!   muro davanti.
//! - **Q rilasciato**: il player **resta attaccato esattamente dove si
//!   trova** (nessuna gravità, nessun movimento), a metà muro come in cima.
//!   Non importa più se è "in cima" o no: prima, rilasciando Q a metà della
//!   cupola della torre dei maghi (o con `last_climb_top` non allineato),
//!   `climbing` veniva azzerato e il player cadeva attraverso il cono, che
//!   non ha un collider calpestabile.
//! - **Spazio**: è l'unico modo per abbandonare la scalata. Il player si
//!   stacca con un piccolo balzo all'indietro (`climb_jump_off`), sia con Q
//!   premuto sia con Q rilasciato.
//! - Se il muro non viene più trovato dal probe (bordo, punta della cupola)
//!   il player resta comunque agganciato, invece di cadere.
//! - Si esce dalla scalata anche in modo "naturale" scendendo fino a terra
//!   (W/S con Q premuto) o salendo oltre il bordo dove c'è un pavimento
//!   reale (atterraggio automatico), oppure col volo (F) o in acqua.

use crate::data::heightmap::OCEAN_ELEVATION;
use crate::data::terrain_mesh::TERRAIN_HEIGHT_SCALE;
use crate::ship::{probe_ship, probe_ship_rapier, ship_floor_at};
use crate::tiles::{probe_world, resolve_world, surface_height_world, TileCollision, WallHit};
use crate::world::*;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, PrimaryWindow};
use bevy_rapier3d::prelude::{RapierContext, WriteRapierContext};

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

const SWIM_SPEED: f32 = 3.5;
const SWIM_VERTICAL_SPEED: f32 = 4.0;

// ============================================================================
// COSTANTI DI SCALATA — multi-height, più permissive
// ============================================================================

/// Raggio del probe di scalata. Prima era 0.9, che lasciava fuori ogni
/// sporgenza più profonda di ~0.5 m (perché il probe partiva dal centro
/// del player e il `COLLIDER_MARGIN` di `tiles.rs` lo troncava a 1.0 m).
/// Ora 1.6 m: copre una sporgenza di ~0.7 m oltre il muro principale anche
/// se il player è incollato al muro (raggio 0.4), più il caso di sporgenze
/// "a scalini" successive (cornicione + finestra).
const CLIMB_REACH: f32 = 1.6;
const CLIMB_GAP: f32 = 0.05;

/// Altezza minima di uno spuntone per essere scalabile. Abbassata da 0.5 a
/// 0.3: una sporgenza di 30 cm basta ad agganciarsi.
const CLIMB_MIN_HEIGHT: f32 = 0.3;
const CLIMB_SEARCH_RADIUS: f32 = 1.0;
const CLIMB_SPEED: f32 = 2.5;
const CLIMB_LATERAL_SPEED: f32 = 1.5;

/// Quanto sopra i piedi può stare il bordo per considerare il ledge
/// raggiunto. Alzato da 0.5 a 0.7: accetta anche sporgenze che stanno un
/// po' più in alto del solito.
const CLIMB_LEDGE: f32 = 0.7;

/// Altezze (metri sopra i piedi) a cui il probe di scalata cerca un muro
/// davanti al player. Prima era un solo raggio a 0.9 m (altezza occhi),
/// che mancava tutte le sporgenze che esistevano solo in basso (cornicioni
/// sotto il ginocchio) o in alto (balconi, travi). Ora tre raggi: basso,
/// occhi, alto. Il primo che trova un muro scalabile vince; fra più hit,
/// quello con `y_max` più alto.
const CLIMB_PROBE_HEIGHTS: [f32; 3] = [0.3, 0.9, 1.6];

const SHIP_CLIMB_MAX_DIST: f32 = 20.0;

/// Altezza massima del gradino superabile automaticamente dai due
/// `try_step_up`. In `ship.rs` c'è una costante omonima (0.8) usata
/// dall'autostep del character controller: sono indipendenti, ma va bene
/// che siano vicine. Se vedi che il player "salta" su gradini troppo alti
/// a piedi, abbassa questa.
const STEP_UP_HEIGHT: f32 = 0.7;
const STEP_UP_REACH: f32 = 0.7;
const STEP_UP_FORWARD: f32 = 0.6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ClimbTarget {
    #[default]
    None,
    City,
    Ship,
}

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
    pub climb_target: ClimbTarget,
    /// `y_max` dell'ultimo muro agganciato. Serve a capire se il player è
    /// **in cima** al muro (e quindi non deve cadere se non c'è un
    /// pavimento oltre il bordo, come sulla torre dei maghi che è cava).
    pub last_climb_top: f32,
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
            climb_target: ClimbTarget::None,
            last_climb_top: 0.0,
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
        if w.is_parrying() {
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

type ChunkQuery<'w, 's> = Query<
'w,
's,
(&'static TileCollision, &'static Transform),
Without<Player>,
>;

type ShipQuery<'w, 's> = Query<
'w,
's,
(Entity, &'static Transform),
(With<crate::ship::Ship>, Without<Player>),
>;

#[allow(clippy::too_many_arguments)]
fn ground_height(
    field: &HeightField,
    origin: &Origin,
    colliders: &ChunkQuery,
    ships: &ShipQuery,
    x: f32,
    z: f32,
    feet_y: f32,
) -> Option<f32> {
    let terrain = field.ground(origin, x, z)?;
    let surface = surface_height_world(colliders.iter(), Vec2::new(x, z), feet_y + SURFACE_STEP);
    let mut ground = surface.map_or(terrain, |s| s.max(terrain));
    for (_, ship_tf) in ships.iter() {
        if let Some(deck) = crate::ship::deck_height_at(ship_tf, Vec2::new(x, z)) {
            if deck <= feet_y + SURFACE_STEP && deck > ground {
                ground = deck;
            }
        }
    }
    Some(ground)
}

// ============================================================================
// AUTO-STEP (città)
// ============================================================================

/// Auto-step su piccoli rilievi di città (gradini, dossi, bordi di
/// marciapiede).
///
/// Se il player è **a terra** e ha davanti un muro di città alto al massimo
/// `STEP_UP_HEIGHT` (70 cm), ci sale sopra automaticamente, **senza premere Q**.
///
/// Non tocca la nave: per la nave c'è `try_step_up_ship`.
#[allow(clippy::too_many_arguments)]
fn try_step_up(
    field: &HeightField,
    origin: &Origin,
    colliders: &ChunkQuery,
    p: &mut Player,
    pos: &mut Vec3,
    forward: Vec3,
) -> bool {
    if !p.on_ground || p.vel_y.abs() > 0.5 {
        return false;
    }

    let facing = Vec2::new(forward.x, forward.z);
    if facing.length_squared() < 0.01 {
        return false;
    }
    let facing = facing.normalize();
    let here = Vec2::new(pos.x, pos.z);

    let Some(hit) = probe_world(colliders.iter(), here, facing, STEP_UP_REACH, p.feet_y) else {
        return false;
    };

    let step_height = hit.y_max - p.feet_y;
    if step_height <= 0.05 || step_height > STEP_UP_HEIGHT {
        return false;
    }

    let beyond = here + facing * STEP_UP_FORWARD;
    let feet_above = p.feet_y + step_height;
    if probe_world(colliders.iter(), beyond, -facing, 0.5, feet_above).is_some() {
        return false;
    }

    let landing = surface_height_world(colliders.iter(), beyond, feet_above + 0.1);
    let terrain = field.ground(origin, beyond.x, beyond.y);
    let target_y = match (landing, terrain) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    let Some(target_y) = target_y else {
        return false;
    };
    if target_y > p.feet_y + STEP_UP_HEIGHT + 0.1 {
        return false;
    }

    p.feet_y = target_y;
    pos.x = beyond.x;
    pos.z = beyond.y;
    p.vel_y = 0.0;
    p.on_ground = true;
    true
}

// ============================================================================
// AUTO-STEP (nave)
// ============================================================================

/// Auto-step su piccoli rilievi **della nave** (scale modellate a mano, bordi
/// di ponte, gradini del cassero).
///
/// È un fallback per i casi in cui l'autostep del character controller di
/// `ship.rs` (che gira in `ship_walk_post`) non riesce a salire (scale con
/// gradini > 0.8 m, rampe ripide, o edge case di Rapier). Normalmente
/// l'autostep basta, ma questa funzione evita che il player si blocchi.
///
/// Usa `probe_ship_rapier` (Rapier) con fallback `probe_ship`, **solo**
/// sulla nave più vicina entro `SHIP_CLIMB_MAX_DIST`. Non tocca mai i muri
/// di città.
#[allow(clippy::too_many_arguments)]
fn try_step_up_ship(
    field: &HeightField,
    origin: &Origin,
    ships: &ShipQuery,
    physics: Option<&RapierContext>,
    p: &mut Player,
    pos: &mut Vec3,
    forward: Vec3,
) -> bool {
    if !p.on_ground || p.vel_y.abs() > 0.5 {
        return false;
    }
    let facing = Vec2::new(forward.x, forward.z);
    if facing.length_squared() < 0.01 {
        return false;
    }
    let facing = facing.normalize();
    let here = Vec2::new(pos.x, pos.z);

    let nearest = ships
    .iter()
    .filter_map(|(entity, tf)| {
        let d = Vec2::new(tf.translation.x, tf.translation.z).distance(here);
        (d <= SHIP_CLIMB_MAX_DIST).then_some((d, entity, tf))
    })
    .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let Some((_, ship_entity, ship_tf)) = nearest else {
        return false;
    };

    let hit = physics
    .and_then(|ctx| probe_ship_rapier(ctx, ship_entity, ship_tf, here, facing, STEP_UP_REACH, p.feet_y))
    .or_else(|| probe_ship(ship_tf, here, facing, STEP_UP_REACH, p.feet_y));
    let Some(hit) = hit else {
        return false;
    };

    let step_height = hit.y_max - p.feet_y;
    if step_height <= 0.05 || step_height > STEP_UP_HEIGHT {
        return false;
    }

    let beyond = here + facing * STEP_UP_FORWARD;
    let feet_above = p.feet_y + step_height;
    let blocked_ahead = physics
    .and_then(|ctx| probe_ship_rapier(ctx, ship_entity, ship_tf, beyond, -facing, 0.5, feet_above))
    .or_else(|| probe_ship(ship_tf, beyond, -facing, 0.5, feet_above))
    .is_some();
    if blocked_ahead {
        return false;
    }

    let ship_landing = ship_floor_at(ship_tf, beyond, feet_above + 0.1);
    let terrain = field.ground(origin, beyond.x, beyond.y);
    let target_y = match (ship_landing, terrain) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    let Some(target_y) = target_y else {
        return false;
    };
    if target_y > p.feet_y + STEP_UP_HEIGHT + 0.1 {
        return false;
    }

    p.feet_y = target_y;
    pos.x = beyond.x;
    pos.z = beyond.y;
    p.vel_y = 0.0;
    p.on_ground = true;
    true
}

// ============================================================================
// PROBE DI SCALATA MULTI-HEIGHT
// ============================================================================

/// Prova il probe di scalata a più altezze (`CLIMB_PROBE_HEIGHTS`) e
/// restituisce l'hit **scalabile** con `y_max` più alto.
///
/// `find_at` è una closure che, data una quota `probe_feet_y` (i piedi del
/// player traslati all'altezza del raggio), restituisce il `WallHit` del
/// muro colpito. La usano sia `try_climb_city` che `try_climb_ship` per
/// non duplicare la logica.
///
/// Il filtro `y_max - feet_y >= CLIMB_MIN_HEIGHT` scarta i muri troppo
/// bassi per essere scalati (una sporgenza di 5 cm non è una sporgenza).
fn probe_climb_multi<F>(feet_y: f32, mut find_at: F) -> Option<WallHit>
where
F: FnMut(f32) -> Option<WallHit>,
{
    let mut best: Option<WallHit> = None;
    for h in CLIMB_PROBE_HEIGHTS {
        if let Some(hit) = find_at(feet_y + h) {
            if hit.ledge_height(feet_y) < CLIMB_MIN_HEIGHT {
                continue;
            }
            if best.as_ref().map_or(true, |b| hit.y_max > b.y_max) {
                best = Some(hit);
            }
        }
    }
    best
}

// ============================================================================
// SCALATA (città + nave)
// ============================================================================

/// Stacco dalla scalata con **Spazio**: il player lascia il muro con un
/// piccolo balzo verso l'esterno (lungo la normale del muro) e torna
/// soggetto alla gravità. È l'unico modo di abbandonare la scalata a metà.
fn climb_jump_off(p: &mut Player, pos: &mut Vec3) {
    p.climbing = false;
    p.climb_target = ClimbTarget::None;
    p.vel_y = JUMP_SPEED * 0.7;
    p.on_ground = false;
    pos.x += p.climb_normal.x * 0.6;
    pos.z += p.climb_normal.y * 0.6;
}
#[allow(clippy::too_many_arguments)]
fn try_climb_city(
    keys: &ButtonInput<KeyCode>,
    field: &HeightField,
    origin: &Origin,
    colliders: &ChunkQuery,
    ships: &ShipQuery,
    p: &mut Player,
    pos: &mut Vec3,
    forward: Vec3,
        right: Vec3,
        dt: f32,
) -> bool {
    if p.climbing && p.climb_target != ClimbTarget::City {
        return false;
    }
    // Spazio = unico modo di abbandonare la scalata (con Q premuto o no).
    if p.climbing && keys.just_pressed(KeyCode::Space) {
        climb_jump_off(p, pos);
        return false;
    }

    let holding = keys.pressed(KeyCode::KeyQ);
    if !p.climbing && !holding {
        return false;
    }
    if p.climbing && !holding {
        // Q rilasciato: restiamo fermi dove siamo, senza cadere.
        return true;
    }

    let here = Vec2::new(pos.x, pos.z);
    let find_hit = |at: Vec2, dir: Vec2, reach: f32, feet_y: f32| -> Option<WallHit> {
        probe_world(colliders.iter(), at, dir, reach, feet_y)
    };

    if !p.climbing {
        let facing = Vec2::new(forward.x, forward.z);
        let Some(hit) = probe_climb_multi(p.feet_y, |probe_feet_y| {
            find_hit(here, facing, CLIMB_REACH, probe_feet_y)
        }) else {
            return false;
        };
        if facing.dot(-hit.normal) < 0.2 {
            return false;
        }
        p.climbing = true;
        p.climb_target = ClimbTarget::City;
        p.climb_normal = hit.normal;
        p.last_climb_top = hit.y_max;
        p.vel_y = 0.0;
        p.on_ground = false;
    }

    let toward = -p.climb_normal;

    // Se il probe di riaggancio fallisce (bordo, punta della cupola, torre
    // cava) restiamo comunque agganciati: si esce solo con Spazio.
    let Some(hit) = probe_climb_multi(p.feet_y, |probe_feet_y| {
        find_hit(here, toward, CLIMB_REACH, probe_feet_y)
    }) else {
        p.feet_y = p.feet_y.min(p.last_climb_top - CLIMB_LEDGE);
        return true;
    };

    p.climb_normal = hit.normal;
    p.last_climb_top = hit.y_max;

    let axis = |k_pos: KeyCode, k_neg: KeyCode| {
        keys.pressed(k_pos) as i32 as f32 - keys.pressed(k_neg) as i32 as f32
    };
    let vertical = axis(KeyCode::KeyW, KeyCode::KeyS);
    let sideways = axis(KeyCode::KeyD, KeyCode::KeyA);

    let mut tangent = Vec2::new(-hit.normal.y, hit.normal.x);
    if tangent.dot(Vec2::new(right.x, right.z)) < 0.0 {
        tangent = -tangent;
    }

    let mut anchor = hit;
    if sideways != 0.0 {
        let try_pos = here + tangent * sideways * CLIMB_LATERAL_SPEED * dt;
        if let Some(h2) = find_hit(try_pos, toward, CLIMB_REACH, p.feet_y) {
            anchor = h2;
            // ← FIX 2: sincronizza last_climb_top quando l'anchor cambia
            // (es. guglia affusolata che ha y_max più basso spostandosi lateralmente).
            // Senza questo, al rilascio di Q il check at_top fallisce perché
            // last_climb_top è rimasto al valore originale più alto.
            p.last_climb_top = h2.y_max;
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
                p.climb_target = ClimbTarget::None;
                p.on_ground = true;
                p.vel_y = 0.0;
                return false;
            }
        }
    }

    let top = anchor.y_max;
    if top - p.feet_y < CLIMB_LEDGE {
        let beyond = Vec2::new(pos.x, pos.z) + toward * (PLAYER_RADIUS + 0.6);
        let city_landing = surface_height_world(colliders.iter(), beyond, top + 1.0);
        let terrain_landing = field.ground(origin, beyond.x, beyond.y).filter(|g| *g >= top - 0.5);
        let landing = [city_landing, terrain_landing]
        .into_iter()
        .flatten()
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        match landing {
            Some(h) => {
                pos.x = beyond.x;
                pos.z = beyond.y;
                p.feet_y = h;
                p.climbing = false;
                p.climb_target = ClimbTarget::None;
                p.on_ground = true;
                p.vel_y = 0.0;
                return false;
            }
            None => p.feet_y = p.feet_y.min(top - CLIMB_LEDGE),
        }
    }
    true
}
#[allow(clippy::too_many_arguments)]
fn try_climb_ship(
    keys: &ButtonInput<KeyCode>,
    field: &HeightField,
    origin: &Origin,
    ships: &ShipQuery,
    physics: Option<&RapierContext>,
    p: &mut Player,
    pos: &mut Vec3,
    _forward: Vec3,
    right: Vec3,
    dt: f32,
) -> bool {
    if p.climbing && p.climb_target != ClimbTarget::Ship {
        return false;
    }

    // Spazio = unico modo di abbandonare la scalata (con Q premuto o no).
    if p.climbing && keys.just_pressed(KeyCode::Space) {
        climb_jump_off(p, pos);
        return false;
    }

    let holding = keys.pressed(KeyCode::KeyQ);
    if !p.climbing && !holding {
        return false;
    }
    if p.climbing && !holding {
        // Q rilasciato: restiamo fermi dove siamo, senza cadere.
        return true;
    }

    let here = Vec2::new(pos.x, pos.z);

    let nearest = ships
    .iter()
    .filter_map(|(entity, tf)| {
        let d = Vec2::new(tf.translation.x, tf.translation.z).distance(here);
        (d <= SHIP_CLIMB_MAX_DIST).then_some((d, entity, tf))
    })
    .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let Some((_, ship_entity, ship_tf)) = nearest else {
        if p.climb_target == ClimbTarget::Ship {
            p.climbing = false;
            p.climb_target = ClimbTarget::None;
        }
        return false;
    };

    let find_hit = |at: Vec2, dir: Vec2, reach: f32, feet_y: f32| -> Option<WallHit> {
        let rapier_hit = physics
        .and_then(|ctx| probe_ship_rapier(ctx, ship_entity, ship_tf, at, dir, reach, feet_y));
        rapier_hit.or_else(|| probe_ship(ship_tf, at, dir, reach, feet_y))
    };

    // Ricerca radiale: prova il probe in N direzioni intorno al player,
    // con reach corto (CLIMB_SEARCH_RADIUS). Restituisce l'hit più vicino
    // che supera CLIMB_MIN_HEIGHT. Multi-height (0.3/0.9/1.6 m) per
    // trovare anche sporgenze che esistono solo in basso o in alto.
    let radial_search = |feet_y: f32| -> Option<WallHit> {
        const N_DIRS: usize = 8;
        let mut best: Option<WallHit> = None;
        for i in 0..N_DIRS {
            let a = i as f32 / N_DIRS as f32 * std::f32::consts::TAU;
            let dir = Vec2::new(a.cos(), a.sin());
            if let Some(hit) = probe_climb_multi(feet_y, |probe_feet_y| {
                find_hit(here, dir, CLIMB_SEARCH_RADIUS, probe_feet_y)
            }) {
                if best.as_ref().map_or(true, |b| hit.distance < b.distance) {
                    best = Some(hit);
                }
            }
        }
        best
    };

    if !p.climbing {
        // Aggancio iniziale: cerco un solido intorno, non solo davanti.
        // Con la nave, il player può agganciarsi al parapetto, all'albero,
        // alla tolda rialzata o a qualunque ostacolo entro 1 m.
        let Some(hit) = radial_search(p.feet_y) else {
            return false;
        };
        p.climbing = true;
        p.climb_target = ClimbTarget::Ship;
        p.climb_normal = hit.normal;
        p.last_climb_top = hit.y_max;
        p.vel_y = 0.0;
        p.on_ground = false;
    }

    let toward = -p.climb_normal;

    // Riaggancio durante la scalata:
    // 1. prima cerco nella direzione della normale attuale (comportamento
    //    classico, reach lungo): la scalata procede in modo regolare lungo
    //    la superficie;
    // 2. se non trovo (superficie curva, bordo, sporgenza laterale), cerco
    //    intorno con `radial_search`: il player si aggancia al solido più
    //    vicino entro CLIMB_SEARCH_RADIUS, cambiando normale se serve.
    let hit_opt = probe_climb_multi(p.feet_y, |probe_feet_y| {
        find_hit(here, toward, CLIMB_REACH, probe_feet_y)
    })
    .or_else(|| radial_search(p.feet_y));

    let Some(hit) = hit_opt else {
        // Nessun solido trovato: restiamo agganciati, si esce con Spazio.
        p.feet_y = p.feet_y.min(p.last_climb_top - CLIMB_LEDGE);
        return true;
    };
    p.climb_normal = hit.normal;
    p.last_climb_top = hit.y_max;

    let axis = |k_pos: KeyCode, k_neg: KeyCode| {
        keys.pressed(k_pos) as i32 as f32 - keys.pressed(k_neg) as i32 as f32
    };
    let vertical = axis(KeyCode::KeyW, KeyCode::KeyS);
    let sideways = axis(KeyCode::KeyD, KeyCode::KeyA);

    let mut tangent = Vec2::new(-hit.normal.y, hit.normal.x);
    if tangent.dot(Vec2::new(right.x, right.z)) < 0.0 {
        tangent = -tangent;
    }
    let mut anchor = hit;
    if sideways != 0.0 {
        let try_pos = here + tangent * sideways * CLIMB_LATERAL_SPEED * dt;
        if let Some(h2) = find_hit(try_pos, toward, CLIMB_REACH, p.feet_y) {
            anchor = h2;
        }
    }
    let at = anchor.point + anchor.normal * (PLAYER_RADIUS + CLIMB_GAP);
    pos.x = at.x;
    pos.z = at.y;

    p.feet_y += vertical * CLIMB_SPEED * dt;
    if vertical < 0.0 {
        let xz = Vec2::new(pos.x, pos.z);
        let ship_deck = ship_floor_at(ship_tf, xz, p.feet_y + SURFACE_STEP);
        let terrain = field.ground(origin, xz.x, xz.y);
        let ground = match (ship_deck, terrain) {
            (Some(a), Some(b)) => Some(a.max(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        if let Some(ground) = ground {
            if p.feet_y <= ground {
                p.feet_y = ground;
                p.climbing = false;
                p.climb_target = ClimbTarget::None;
                p.on_ground = true;
                p.vel_y = 0.0;
                return false;
            }
        }
    }

    let top = anchor.y_max;
    if top - p.feet_y < CLIMB_LEDGE {
        let beyond = Vec2::new(pos.x, pos.z) + toward * (PLAYER_RADIUS + 0.6);
        let ship_landing = ship_floor_at(ship_tf, beyond, top + 1.0);
        let terrain_landing = field.ground(origin, beyond.x, beyond.y).filter(|g| *g >= top - 0.5);
        let landing = [ship_landing, terrain_landing]
        .into_iter()
        .flatten()
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        match landing {
            Some(h) => {
                pos.x = beyond.x;
                pos.z = beyond.y;
                p.feet_y = h;
                p.climbing = false;
                p.climb_target = ClimbTarget::None;
                p.on_ground = true;
                p.vel_y = 0.0;
                return false;
            }
            None => p.feet_y = p.feet_y.min(top - CLIMB_LEDGE),
        }
    }
    true
}
#[allow(clippy::too_many_arguments)]
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
    wave_sampler: Option<Res<crate::ocean::WaveSampler>>,
    colliders: ChunkQuery,
    ships: ShipQuery,
    mut physics: WriteRapierContext,
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
                p.climb_target = ClimbTarget::None;
            }
            return;
        }
        let Ok((mut p, mut tf)) = player.get_single_mut() else { return };
        let dt = time.delta_secs().min(0.1);

        if keys.just_pressed(KeyCode::KeyF) {
            p.fly = !p.fly;
            p.vel_y = 0.0;
            p.climbing = false;
            p.climb_target = ClimbTarget::None;
        }

        let forward = Vec3::new(-p.yaw.sin(), 0.0, -p.yaw.cos());
        let right = Vec3::new(p.yaw.cos(), 0.0, -p.yaw.sin());
        let axis = |pos: KeyCode, neg: KeyCode| keys.pressed(pos) as i32 as f32 - keys.pressed(neg) as i32 as f32;
        let wish = forward * axis(KeyCode::KeyW, KeyCode::KeyS) + right * axis(KeyCode::KeyD, KeyCode::KeyA);
        let running = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

        let mut pos = tf.translation;

        let viewer_xz = Vec2::new(pos.x, pos.z);
        let wave = wave_sampler
        .as_ref()
        .and_then(|s| crate::ocean::wave_height_at(s, pos.x, pos.z, viewer_xz))
        .unwrap_or(0.0);
        let water_surface = OCEAN_ELEVATION * TERRAIN_HEIGHT_SCALE + wave;
        let in_water = p.feet_y < water_surface;

        // ------------------------------------------------------------------
        // Auto-step e scalata. Ordine:
        //   1. try_step_up      — piccoli rilievi di città senza Q
        //   2. try_step_up_ship — piccoli rilievi di nave senza Q (fallback
        //                          per scale con gradini > autostep Rapier)
        //   3. try_climb_ship   — scalata nave con Q
        //   4. try_climb_city   — scalata città con Q
        // ------------------------------------------------------------------
        if !p.fly && !in_water {
            if try_step_up(&field, &origin, &colliders, &mut p, &mut pos, forward) {
                pos.y = p.feet_y + EYE_HEIGHT;
                tf.translation = pos;
                return;
            }

            let ctx_guard = physics.rapier_context.get_single_mut().ok();
            let ctx_opt: Option<&RapierContext> = ctx_guard.as_deref();

            if try_step_up_ship(&field, &origin, &ships, ctx_opt, &mut p, &mut pos, forward) {
                pos.y = p.feet_y + EYE_HEIGHT;
                tf.translation = pos;
                return;
            }

            let climbed = try_climb_ship(
                &keys,
                &field,
                &origin,
                &ships,
                ctx_opt,
                &mut p,
                &mut pos,
                forward,
                    right,
                    dt,
            ) || try_climb_city(
                &keys,
                &field,
                &origin,
                &colliders,
                &ships,
                &mut p,
                &mut pos,
                forward,
                    right,
                    dt,
            );

            if climbed {
                pos.y = p.feet_y + EYE_HEIGHT;
                tf.translation = pos;
                return;
            }
        } else {
            if p.climbing {
                p.climbing = false;
                p.climb_target = ClimbTarget::None;
            }
        }

        if p.fly {
            let speed = FLY_SPEED * if running { FLY_RUN_MULTIPLIER } else { 1.0 };
            let vertical = axis(KeyCode::Space, KeyCode::ControlLeft);
            pos += (wish.normalize_or_zero() + Vec3::Y * vertical) * speed * dt;
            p.feet_y = pos.y - EYE_HEIGHT;
        } else {
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
                    if keys.pressed(KeyCode::Space) {
                        p.vel_y = SWIM_VERTICAL_SPEED;
                    } else if keys.pressed(KeyCode::ControlLeft) {
                        p.vel_y = -SWIM_VERTICAL_SPEED;
                    } else {
                        let target_y = water_surface - EYE_HEIGHT * 0.8;
                        let diff = target_y - p.feet_y;
                        p.vel_y = (diff * 3.0).clamp(-2.0, 2.0);
                    }
                    p.feet_y += p.vel_y * dt;
                    p.on_ground = false;
                    if p.feet_y < ground {
                        p.feet_y = ground;
                        p.vel_y = 0.0;
                    }
                } else {
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
