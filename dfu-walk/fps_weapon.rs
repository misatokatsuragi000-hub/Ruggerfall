//! Arma in prima persona: due billboard figli del Player (arma e scudo).
//!
//! - l'arma usa sempre la texture `cif_0-0` dell'arma equipaggiata (RightHand);
//! - lo scudo mostra la texture `SHIELD_TEXTURE` (`"112361_13-0"`) quando è
//!   equipaggiato un oggetto in `EquipSlot::LeftHand`;
//! - **entrambi rispettano l'aspect ratio nativo** della texture.
//! - l'arma ruota rigidamente attorno al **centro del bordo inferiore** del
//!   quad, che resta fisso in camera space;
//! - **RMB tenuto premuto**: la spada raggiunge `RMB_MAX_PITCH` e ci resta;
//!   ogni `RMB_REPEAT_INTERVAL` emette un `PlayerStrike { kind: Thrust }`
//!   invisibile.
//! - **MMB da solo** è il **power attack**;
//! - durante un **fendente** (RMB + LMB) la parte superiore ruota su X.
//!
//! ## Punta della spada libera durante l'affondo (RMB)
//!
//! Con **RMB tenuto** (senza LMB/MMB) la **punta** (angolo in alto a sinistra
//! della texture) cade esattamente sul cursore, quindi raggiunge qualunque
//! punto dello schermo (entro `TIP_NDC_MIN/MAX`), **a dimensione costante**
//! (`TIP_SCALE`). La spada punta lungo la direzione ancoraggio → cursore e la
//! sua base scivola lungo quella direzione: si avvicina al cursore quando è
//! lontano e arretra sotto l'ancoraggio (fuori schermo) quando è vicino. La
//! base sta sempre a `-WEAPON_DIST` (davanti alla camera) e il quad resta
//! parallelo allo schermo (niente pitch).
//! La direzione della punta (o, quando non c'è affondo, quella del cursore) è
//! pubblicata ogni frame in `combat::SwordTip` e usata da
//! `apply_player_strike` per mirare affondo, power attack e fendente
//! dall'alto. La transizione dalla posa di
//! riposo dura `TIP_BLEND_SECS`.
//!
//! ## Modalità edit dello scudo (`Ctrl+LMB`)
//!
//! Tenendo premuto **Ctrl** e **LMB** insieme, si entra in modalità "edit
//! shield": lo scudo segue il movimento del mouse (delta) e la distanza dalla
//! camera si regola con la rotella. Al rilascio di Ctrl o LMB, la posizione
//! viene **salvata automaticamente** in `shield_pose.txt` e ripristinata da
//! lì all'avvio successivo.
//!
//! Vantaggi:
//!
//! - **Non richiede file di config manuali**: la posa di default è quella
//!   storica (`-0.8, -1.2, 0.6, 0.05`), e si può cambiare in gioco.
//! - **Non attiva la parata**: `update_weapon_input` sopprime `parrying` quando
//!   Ctrl è premuto, così l'arma non cerca di parare mentre muovi lo scudo.
//! - **Non ruota la camera**: `is_parrying()` include `editing_shield`, quindi
//!   `player_look` fa `return` e la camera resta ferma. Il delta del mouse
//!   viene consumato solo dall'edit.
//! - **Persistente**: `ShieldTuning::load` legge `shield_pose.txt` all'avvio;
//!   `ShieldTuning::save` scrive il file appena esci dall'edit.
//!
//! Sensibilità:
//!
//! - mouse: `SHIELD_EDIT_SENSITIVITY = 0.0015` → 1.5 mm per pixel,
//! - rotella: `SHIELD_EDIT_DIST_SENSITIVITY = 0.05` → 5 cm per tacca,
//! - clamp: `x,y ∈ [-1.5, 1.5]`, `dist ∈ [0.3, 1.5]`.
//!
//! ## Scudo (fisso in basso a sinistra)
//!
//! Fisso in basso a sinistra (`ShieldTuning.anchor_x = -0.8`), nessuna
//! rotazione con il mouse. **Lo scudo conta per la parata**:
//! `WeaponCover::covers(ndc)` restituisce `true` se il punto dell'alone è
//! coperto dall'**arma** oppure dallo **scudo**.
//!
//! ## Copertura locazionale (`WeaponCover`)
//!
//! `WeaponCover::covers(ndc)` proietta un raggio camera → punto, lo
//! interseca col piano del quad, e verifica che il punto cada dentro il
//! rettangolo **e** su un pixel opaco della texture. Usato da `combat.rs`
//! per la parata locazionale.
//!
//! ## Camera lock
//!
//! `is_parrying()` restituisce `true` quando LMB, RMB, MMB sono premuti con
//! l'arma in mano, **oppure** quando un attacco è in corso, **oppure**
//! quando l'edit dello scudo è attivo.

use crate::combat::{PlayerStrike, StrikeKind, SwordTip};
use crate::dialog::DialogState;
use crate::equipment::{EquipSlot, EquipmentState, PlayerInventory};
use crate::mod_textures::ModTextures;
use crate::player::Player;
use crate::vitals::PlayerVitals;
use bevy::ecs::system::SystemParam;
use bevy::input::mouse::{AccumulatedMouseMotion, MouseWheel};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use std::collections::HashMap;
use std::f32::consts::PI;
use std::path::Path;

// =========================================================================
// COSTANTI PUBBLICHE
// =========================================================================
pub const ATTACK_DURATION: f32 = 0.28;
pub const ATTACK_HIT_FRACTION: f32 = 0.45;

// =========================================================================
// COSTANTI DI POSA (arma)
// =========================================================================
const WEAPON_DIST: f32 = 0.9;
const WEAPON_SCALE: f32 = 1.35;
const BASE_W: f32 = 0.50;
const BASE_H: f32 = 1.05;
const WEAPON_MAX_SIZE: f32 = BASE_H * WEAPON_SCALE;

const QUAD_W: f32 = BASE_W * WEAPON_SCALE;
const QUAD_H: f32 = BASE_H * WEAPON_SCALE;

const WEAPON_FOV_VERTICAL_DEG: f32 = 70.0;
const BOTTOM_SCREEN_Y: f32 = -WEAPON_DIST * 0.7002;
const BOTTOM_PADDING_EXTRA: f32 = -0.25;

const ANCHOR_X: f32 = 0.45;
const ANCHOR_Y: f32 = BOTTOM_SCREEN_Y + BOTTOM_PADDING_EXTRA;
const AIM_SMOOTHING: f32 = 14.0;
const MAX_ANGLE_DEG: f32 = 45.0;

/// Limiti (NDC) entro cui la punta può seguire il cursore con RMB.
/// Fino ai bordi dello schermo, così si mirano anche bersagli appena visibili
/// (es. nemici volanti in alto).
const TIP_NDC_MIN: Vec2 = Vec2::new(-1.0, -0.9);
const TIP_NDC_MAX: Vec2 = Vec2::new(1.0, 1.0);
/// Scala della spada durante l'affondo: costante, così la dimensione non
/// cambia muovendo il cursore (1.0 = come a riposo).
const TIP_SCALE: f32 = 1.0;
/// Secondi per passare dalla posa di riposo alla punta sul cursore.
const TIP_BLEND_SECS: f32 = 0.10;

// =========================================================================
// COSTANTI DI POSA (scudo, valori di default)
// =========================================================================
const SHIELD_TEXTURE: &str = "112361_13-0";
const SHIELD_ANCHOR_X: f32 = -0.8;
const SHIELD_ANCHOR_Y: f32 = -1.2;
const SHIELD_MAX_SIZE: f32 = BASE_H * WEAPON_SCALE * 0.9;
const SHIELD_QUAD_W: f32 = SHIELD_MAX_SIZE;
const SHIELD_QUAD_H: f32 = SHIELD_MAX_SIZE;
const SHIELD_DIST: f32 = 0.6;
const SHIELD_Z_OFFSET: f32 = 0.05;

// =========================================================================
// COSTANTI DI EDIT DELLO SCUDO
// =========================================================================
const SHIELD_POSE_FILE: &str = "shield_pose.txt";
/// Sensibilità del mouse in modalità edit: 1.5 mm per pixel.
const SHIELD_EDIT_SENSITIVITY: f32 = 0.0015;
/// Sensibilità della rotella in modalità edit: 5 cm per tacca.
const SHIELD_EDIT_DIST_SENSITIVITY: f32 = 0.05;
const SHIELD_ANCHOR_X_MIN: f32 = -1.5;
const SHIELD_ANCHOR_X_MAX: f32 = 1.5;
const SHIELD_ANCHOR_Y_MIN: f32 = -1.5;
const SHIELD_ANCHOR_Y_MAX: f32 = 1.5;
const SHIELD_DIST_MIN: f32 = 0.3;
const SHIELD_DIST_MAX: f32 = 1.5;

// =========================================================================
// ANIMAZIONE D'ATTACCO
// =========================================================================
const ATTACK_PITCH_MAX_RAD: f32 = 0.75;
const THRUST_DEPTH: f32 = 0.08;
const POWER_ATTACK_MULT: f32 = 2.0;

const RMB_MAX_PITCH: f32 = -ATTACK_PITCH_MAX_RAD;
const RMB_REPEAT_INTERVAL: f32 = 1.0;

const SWING_PITCH_MAX_RAD: f32 = 0.3;
const SWING_RAMP_DURATION_SECS: f32 = 0.25;
const SWING_THRUST_DEPTH: f32 = 0.05;

// =========================================================================
// FENDENTE DALL'ALTO
// =========================================================================
const OVERHEAD_AIM_Y: f32 = 0.35;
const OVERHEAD_DURATION: f32 = 0.55;
const OVERHEAD_WINDUP_END: f32 = 0.35;
const OVERHEAD_STRIKE_END: f32 = 0.65;
const OVERHEAD_HIT_T: f32 = 0.60;
const OVERHEAD_WINDUP_PITCH: f32 = 0.35;
const OVERHEAD_RAISE: f32 = 0.30;
const OVERHEAD_CHOP_PITCH: f32 = 0.90;
const OVERHEAD_DROP: f32 = 0.05;
const OVERHEAD_THRUST_DEPTH: f32 = 0.12;

// =========================================================================
// CONSUMO DI STAMINA
// =========================================================================
const ATTACK_STAMINA_COST: f32 = 5.0;
const SWING_STAMINA_COST: f32 = 5.0;
const POWER_ATTACK_STAMINA_COST: f32 = 10.0;
const OVERHEAD_STAMINA_COST: f32 = 8.0;

// =========================================================================
// MICRO TRASLAZIONE
// =========================================================================
const AIM_FOLLOW_FACTOR_BASE: f32 = 0.10;
const AIM_FOLLOW_FACTOR_AIM: f32 = 0.25;

const DRAW_SPEED: f32 = 8.0;
const VISIBLE_THRESHOLD: f32 = 0.02;
const ALPHA_CUTOFF: f32 = 0.4;

// =========================================================================
// STATO
// =========================================================================
#[derive(Resource)]
pub struct FpsWeaponState {
    pub drawn: bool,
    pub parrying: bool,
    pub rmb_held: bool,
    pub mmb_held: bool,
    pub swinging: bool,
    pub swing_pitch: f32,
    pub aim: Vec2,
    pub has_texture: bool,
    /// `true` quando il player sta modificando la posa dello scudo
    /// (`Ctrl+LMB`). Impedisce la parata e blocca la camera.
    pub editing_shield: bool,

    aim_raw: Vec2,
    draw_progress: f32,
    attack_timer: f32,
    attack_active: bool,
    is_special: bool,
    attack_strike_emitted: bool,
    swing_strike_emitted: bool,
    rmb_repeat_timer: f32,

    overhead_active: bool,
    overhead_timer: f32,
    overhead_strike_emitted: bool,
    overhead_latched: bool,
    combo_prev: bool,
}

impl Default for FpsWeaponState {
    fn default() -> Self {
        Self {
            drawn: false,
            parrying: false,
            rmb_held: false,
            mmb_held: false,
            swinging: false,
            swing_pitch: 0.0,
            aim: Vec2::ZERO,
            has_texture: false,
            editing_shield: false,
            aim_raw: Vec2::ZERO,
            draw_progress: 0.0,
            attack_timer: 0.0,
            attack_active: false,
            is_special: false,
            attack_strike_emitted: false,
            swing_strike_emitted: false,
            rmb_repeat_timer: 0.0,
            overhead_active: false,
            overhead_timer: 0.0,
            overhead_strike_emitted: false,
            overhead_latched: false,
            combo_prev: false,
        }
    }
}

impl FpsWeaponState {
    pub fn current_attack(&self) -> Option<(bool, f32)> {
        self.attack_active
        .then(|| (self.is_special, (self.attack_timer / ATTACK_DURATION).clamp(0.0, 1.0)))
    }

    pub fn current_overhead(&self) -> Option<f32> {
        self.overhead_active
        .then(|| (self.overhead_timer / OVERHEAD_DURATION).clamp(0.0, 1.0))
    }

    pub fn is_overhead_active(&self) -> bool {
        self.overhead_active
    }

    pub fn aim_zone_x(&self) -> f32 {
        self.aim.x * 0.5
    }

    /// `true` se il player sta modificando la posa dello scudo (`Ctrl+LMB`).
    pub fn is_editing_shield(&self) -> bool {
        self.editing_shield
    }

    pub fn is_parrying(&self) -> bool {
        self.drawn
        && (self.parrying
        || self.rmb_held
        || self.mmb_held
        || self.attack_active
        || self.overhead_active
        || self.editing_shield)
    }

    pub fn is_parry_active(&self) -> bool {
        self.drawn
        && self.parrying
        && !self.swinging
        && !self.attack_active
        && !self.overhead_active
        && !self.editing_shield
    }

    pub fn is_aiming_with_attack(&self) -> bool {
        self.drawn && (self.rmb_held || self.mmb_held)
    }

    pub fn is_drawn(&self) -> bool {
        self.drawn
    }

    pub fn is_swinging(&self) -> bool {
        self.swinging
    }

    pub fn is_attack_active(&self) -> bool {
        self.attack_active
    }

    pub fn is_lmb_held(&self) -> bool {
        self.parrying
    }

    pub fn is_weapon_visible(&self) -> bool {
        self.draw_progress >= VISIBLE_THRESHOLD && self.has_texture
    }
}

// =========================================================================
// POSA DELLO SCUDO (configurabile e persistita)
// =========================================================================

/// Parametri della posa dello scudo. Di default coincidono con le costanti
/// storiche (`-0.8, -1.2, 0.6, 0.05`); possono essere modificati in gioco con
/// `Ctrl+LMB` (sposta con il mouse, rotella per la distanza) e vengono
/// salvati automaticamente in `shield_pose.txt`.
#[derive(Resource, Debug, Clone)]
pub struct ShieldTuning {
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub dist: f32,
    pub z_offset: f32,
}

impl Default for ShieldTuning {
    fn default() -> Self {
        Self {
            anchor_x: SHIELD_ANCHOR_X,
            anchor_y: SHIELD_ANCHOR_Y,
            dist: SHIELD_DIST,
            z_offset: SHIELD_Z_OFFSET,
        }
    }
}

impl ShieldTuning {
    /// Carica la posa da `path` (`chiave = valore`, `#` per i commenti). Se il
    /// file non esiste o è illeggibile, ritorna i valori di default.
    pub fn load(path: &Path) -> Self {
        let mut t = Self::default();
        let Ok(text) = std::fs::read_to_string(path) else {
            info!(
                "ShieldTuning: {} non trovato, uso la posa predefinita (Ctrl+LMB per modificare)",
                  path.display()
            );
            return t;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "anchor_x" => if let Ok(x) = v.parse() { t.anchor_x = x; },
                "anchor_y" => if let Ok(x) = v.parse() { t.anchor_y = x; },
                "dist" => if let Ok(x) = v.parse() { t.dist = x; },
                "z_offset" => if let Ok(x) = v.parse() { t.z_offset = x; },
                _ => {}
            }
        }
        info!(
            "ShieldTuning: posa caricata da {} (x={:.3}, y={:.3}, dist={:.3}, z_offset={:.3})",
              path.display(), t.anchor_x, t.anchor_y, t.dist, t.z_offset
        );
        t
    }

    /// Salva la posa in `path`. Errori solo loggati (non bloccano il gioco).
    pub fn save(&self, path: &Path) {
        let text = format!(
            "# Posa dello scudo (Ctrl+LMB per spostare, rotella per distanza)\n\
# anchor_x/anchor_y: posizione in camera space. dist: distanza dalla camera.\n\
# z_offset: offset Z aggiuntivo (arretra lo scudo dietro l'arma).\n\
anchor_x = {}\n\
anchor_y = {}\n\
dist = {}\n\
z_offset = {}\n",
self.anchor_x, self.anchor_y, self.dist, self.z_offset
        );
        if let Err(e) = std::fs::write(path, text) {
            warn!("ShieldTuning: impossibile scrivere {}: {e}", path.display());
        } else {
            info!(
                "ShieldTuning: posa salvata in {} (x={:.3}, y={:.3}, dist={:.3})",
                  path.display(), self.anchor_x, self.anchor_y, self.dist
            );
        }
    }
}

// =========================================================================
// COPERTURA LOCAZIONALE
// =========================================================================

struct QuadInfo<'a> {
    tf: &'a Transform,
    material: &'a MeshMaterial3d<StandardMaterial>,
    width: f32,
    height: f32,
}

#[derive(SystemParam)]
pub struct WeaponCover<'w, 's> {
    state: Option<Res<'w, FpsWeaponState>>,
    weapon_billboard: Query<
    'w,
    's,
    (
        &'static FpsWeaponBillboard,
     &'static Transform,
     &'static MeshMaterial3d<StandardMaterial>,
     &'static Visibility,
    ),
    >,
    shield_billboard: Query<
    'w,
    's,
    (
        &'static ShieldBillboard,
     &'static Transform,
     &'static MeshMaterial3d<StandardMaterial>,
     &'static Visibility,
    ),
    Without<FpsWeaponBillboard>,
    >,
    materials: Res<'w, Assets<StandardMaterial>>,
    images: Res<'w, Assets<Image>>,
    windows: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
}

impl WeaponCover<'_, '_> {
    pub fn lmb_held(&self) -> bool {
        self.state.as_ref().is_some_and(|s| s.is_lmb_held())
    }

    pub fn covers(&self, ndc: Vec2) -> bool {
        let weapon_visible = self
        .state
        .as_ref()
        .is_some_and(|s| s.is_weapon_visible());
        if weapon_visible {
            if let Some((billboard, tf, mat, vis)) = self.weapon_billboard.iter().next() {
                if *vis != Visibility::Hidden {
                    let quad = QuadInfo {
                        tf,
                        material: mat,
                        width: billboard.quad.x,
                        height: billboard.quad.y,
                    };
                    if self.hit_quad(&quad, ndc) {
                        return true;
                    }
                }
            }
        }

        if let Some((billboard, tf, mat, vis)) = self.shield_billboard.iter().next() {
            if *vis != Visibility::Hidden && billboard.loaded {
                let quad = QuadInfo {
                    tf,
                    material: mat,
                    width: billboard.quad.x,
                    height: billboard.quad.y,
                };
                if self.hit_quad(&quad, ndc) {
                    return true;
                }
            }
        }

        false
    }

    fn hit_quad(&self, quad: &QuadInfo, ndc: Vec2) -> bool {
        let aspect = self
        .windows
        .get_single()
        .map(|w| (w.width() / w.height().max(1.0)).max(1e-3))
        .unwrap_or(16.0 / 9.0);
        let half_h = WEAPON_DIST * (WEAPON_FOV_VERTICAL_DEG * 0.5).to_radians().tan();
        let half_w = half_h * aspect;
        let dir = Vec3::new(ndc.x * half_w, ndc.y * half_h, -WEAPON_DIST).normalize();

        let normal = quad.tf.rotation * Vec3::Z;
        let denom = normal.dot(dir);
        if denom.abs() < 1e-4 {
            return false;
        }
        let t = normal.dot(quad.tf.translation) / denom;
        if t <= 0.0 {
            return false;
        }
        let local = (quad.tf.rotation.inverse() * (dir * t - quad.tf.translation))
        / quad.tf.scale;
        if local.x.abs() > quad.width * 0.5 || local.y.abs() > quad.height * 0.5 {
            return false;
        }

        let Some(image) = self
        .materials
        .get(&quad.material.0)
        .and_then(|m| m.base_color_texture.as_ref())
        .and_then(|h| self.images.get(h))
        else {
            return true;
        };
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 {
            return true;
        }
        let u = (local.x / quad.width + 0.5).clamp(0.0, 1.0);
        let v = (0.5 - local.y / quad.height).clamp(0.0, 1.0);
        let px = ((u * w as f32) as u32).min(w - 1);
        let py = ((v * h as f32) as u32).min(h - 1);
        match image.get_color_at(px, py) {
            Ok(c) => c.to_srgba().alpha >= ALPHA_CUTOFF,
            Err(_) => true,
        }
    }
}

#[derive(Component)]
pub struct FpsWeaponBillboard {
    pub quad: Vec2,
}

#[derive(Component)]
pub struct ShieldBillboard {
    pub loaded: bool,
    pub quad: Vec2,
}

// =========================================================================
// PLUGIN
// =========================================================================
pub struct FpsWeaponPlugin;

impl Plugin for FpsWeaponPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FpsWeaponState>()
        .insert_resource(ShieldTuning::load(Path::new(SHIELD_POSE_FILE)))
        .add_systems(
            Update,
            (
                update_aim_from_mouse,
             spawn_weapon_billboard,
             spawn_shield_billboard,
             update_weapon_input,
             // L'edit dello scudo va DOPO `update_weapon_input` (che setta
             // `editing_shield`) e PRIMA di `update_shield_pose` (che usa i
             // valori aggiornati).
             edit_shield_pose,
             tick_attack_timer,
             update_weapon_texture,
             update_shield_texture,
             update_weapon_pose,
             update_shield_pose,
             emit_strikes,
            )
            .chain()
            .before(crate::player::player_look),
        );
    }
}

// =========================================================================
// 1. AIM
// =========================================================================
fn update_aim_from_mouse(
    time: Res<Time>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut state: ResMut<FpsWeaponState>,
) {
    let dt = time.delta_secs().min(0.1);

    let target = windows
    .get_single()
    .ok()
    .and_then(|w| {
        let p = w.cursor_position()?;
        let size = Vec2::new(w.width(), w.height());
        if size.x <= 0.0 || size.y <= 0.0 {
            return None;
        }
        Some(Vec2::new(
            (p.x / size.x) * 2.0 - 1.0,
                       1.0 - (p.y / size.y) * 2.0,
        ))
    })
    .unwrap_or(Vec2::ZERO)
    .clamp(Vec2::splat(-1.0), Vec2::ONE);

    state.aim_raw = target;
    let k = 1.0 - (-AIM_SMOOTHING * dt).exp();
    let delta = (state.aim_raw - state.aim) * k;
    state.aim = (state.aim + delta).clamp(Vec2::splat(-1.0), Vec2::ONE);
}

// =========================================================================
// 2. SPAWN
// =========================================================================
fn spawn_weapon_billboard(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    player_q: Query<Entity, With<Player>>,
    mut spawned: Local<bool>,
) {
    if *spawned {
        return;
    }
    let Ok(player_entity) = player_q.get_single() else {
        return;
    };
    *spawned = true;

    let mesh = meshes.add(Rectangle::new(QUAD_W, QUAD_H));
    let material = materials.add(weapon_material(None));

    commands.entity(player_entity).with_children(|p| {
        p.spawn((
            Name::new("FPS Weapon Billboard"),
                 Mesh3d(mesh),
                 MeshMaterial3d(material),
                 Transform::from_xyz(0.0, 0.0, -WEAPON_DIST),
                 Visibility::Hidden,
                 FpsWeaponBillboard {
                     quad: Vec2::new(QUAD_W, QUAD_H),
                 },
        ));
    });
}

fn spawn_shield_billboard(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    player_q: Query<Entity, With<Player>>,
    mut spawned: Local<bool>,
) {
    if *spawned {
        return;
    }
    let Ok(player_entity) = player_q.get_single() else {
        return;
    };
    *spawned = true;

    let mesh = meshes.add(Rectangle::new(SHIELD_QUAD_W, SHIELD_QUAD_H));
    let material = materials.add(weapon_material(None));

    commands.entity(player_entity).with_children(|p| {
        p.spawn((
            Name::new("FPS Shield Billboard"),
                 Mesh3d(mesh),
                 MeshMaterial3d(material),
                 Transform::from_xyz(0.0, 0.0, -SHIELD_DIST),
                 Visibility::Hidden,
                 ShieldBillboard {
                     loaded: false,
                     quad: Vec2::new(SHIELD_QUAD_W, SHIELD_QUAD_H),
                 },
        ));
    });
}

fn weapon_material(texture: Option<Handle<Image>>) -> StandardMaterial {
    StandardMaterial {
        base_color: Color::WHITE,
        base_color_texture: texture,
        unlit: true,
        alpha_mode: AlphaMode::Mask(ALPHA_CUTOFF),
        cull_mode: None,
        double_sided: true,
        ..default()
    }
}

// =========================================================================
// 3. INPUT
// =========================================================================
pub fn update_weapon_input(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    equipment: Res<EquipmentState>,
    dialog: Res<DialogState>,
    mut vitals: Option<ResMut<PlayerVitals>>,
    mut state: ResMut<FpsWeaponState>,
) {
    let dt = time.delta_secs().min(0.1);

    if dialog.open || equipment.open {
        state.parrying = false;
        state.rmb_held = false;
        state.mmb_held = false;
        state.swinging = false;
        state.combo_prev = false;
        state.overhead_latched = false;
        state.rmb_repeat_timer = 0.0;
        state.editing_shield = false;
        return;
    }

    if keys.just_pressed(KeyCode::KeyZ) {
        state.drawn = !state.drawn;
        if !state.drawn {
            state.attack_active = false;
            state.attack_timer = 0.0;
            state.parrying = false;
            state.rmb_held = false;
            state.mmb_held = false;
            state.swinging = false;
            state.attack_strike_emitted = false;
            state.swing_strike_emitted = false;
            state.overhead_active = false;
            state.overhead_timer = 0.0;
            state.overhead_strike_emitted = false;
            state.rmb_repeat_timer = 0.0;
            state.combo_prev = false;
            state.overhead_latched = false;
            state.editing_shield = false;
        }
    }

    if !state.drawn {
        state.parrying = false;
        state.rmb_held = false;
        state.mmb_held = false;
        state.swinging = false;
        state.combo_prev = false;
        state.overhead_latched = false;
        state.rmb_repeat_timer = 0.0;
        state.editing_shield = false;
        return;
    }

    let lmb = mouse.pressed(MouseButton::Left);
    let rmb = mouse.pressed(MouseButton::Right);
    let mmb = mouse.pressed(MouseButton::Middle);
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);

    // --- Modalità edit dello scudo: Ctrl+LMB ---
    //
    // Sopprime tutti gli altri input (parata, attacchi, swing). Il sistema
    // `edit_shield_pose` (più avanti nella catena) legge `editing_shield` e
    // muove lo scudo in base al delta del mouse / rotella.
    if ctrl && lmb && !state.attack_active && !state.overhead_active && !state.swinging {
        state.editing_shield = true;
        state.parrying = false;
        state.rmb_held = false;
        state.mmb_held = false;
        state.combo_prev = false;
        state.overhead_latched = false;
        state.rmb_repeat_timer = 0.0;
        return;
    }
    state.editing_shield = false;

    state.parrying = lmb;
    state.rmb_held = rmb;
    state.mmb_held = mmb;

    // --- Fendente dall'alto: LMB+RMB con il cursore in alto ---
    let combo = lmb && rmb;
    let combo_started = combo && !state.combo_prev;
    state.combo_prev = combo;
    if !combo {
        state.overhead_latched = false;
    }
    if combo_started && state.aim.y >= OVERHEAD_AIM_Y {
        state.overhead_latched = true;
        if !state.overhead_active {
            state.overhead_active = true;
            state.overhead_timer = 0.0;
            state.overhead_strike_emitted = false;
            state.attack_active = false;
            state.attack_timer = 0.0;
            if let Some(v) = vitals.as_deref_mut() {
                v.stamina = (v.stamina - OVERHEAD_STAMINA_COST).clamp(0.0, v.max_stamina);
            }
        }
    }

    let was_swinging = state.swinging;
    state.swinging = combo && !state.overhead_latched;

    if state.swinging && !was_swinging {
        state.attack_active = false;
        state.attack_timer = 0.0;
        state.swing_strike_emitted = false;
        if let Some(v) = vitals.as_deref_mut() {
            v.stamina = (v.stamina - SWING_STAMINA_COST).clamp(0.0, v.max_stamina);
        }
    }

    let thrust_eligible =
    rmb && !lmb && !state.swinging && !state.overhead_active;
    if thrust_eligible {
        state.rmb_repeat_timer = (state.rmb_repeat_timer - dt).max(0.0);
        if !state.attack_active && state.rmb_repeat_timer <= 0.0 {
            state.attack_active = true;
            state.attack_timer = 0.0;
            state.is_special = false;
            state.attack_strike_emitted = false;
            state.rmb_repeat_timer = RMB_REPEAT_INTERVAL;
            if let Some(v) = vitals.as_deref_mut() {
                v.stamina = (v.stamina - ATTACK_STAMINA_COST).clamp(0.0, v.max_stamina);
            }
        }
    } else {
        state.rmb_repeat_timer = 0.0;
    }

    if !state.swinging
        && !state.attack_active
        && !state.overhead_active
        && mouse.just_pressed(MouseButton::Middle)
        {
            state.attack_active = true;
            state.attack_timer = 0.0;
            state.is_special = true;
            state.attack_strike_emitted = false;
            if let Some(v) = vitals.as_deref_mut() {
                v.stamina = (v.stamina - POWER_ATTACK_STAMINA_COST).clamp(0.0, v.max_stamina);
            }
        }
}

// =========================================================================
// 4. EDIT DELLO SCUDO
// =========================================================================
//
// Muove lo scudo con il delta del mouse (Ctrl+LMB) e ne regola la distanza
// con la rotella. Quando si esce dall'edit, salva la posa in
// `shield_pose.txt`.
fn edit_shield_pose(
    state: Res<FpsWeaponState>,
    motion: Res<AccumulatedMouseMotion>,
    mut wheel: EventReader<MouseWheel>,
    mut tuning: ResMut<ShieldTuning>,
    mut last_editing: Local<bool>,
) {
    if !state.editing_shield {
        if *last_editing {
            tuning.save(Path::new(SHIELD_POSE_FILE));
            *last_editing = false;
        }
        // Consuma gli eventi della rotella per non accumularli.
        wheel.clear();
        return;
    }
    *last_editing = true;

    // Movimento con il delta del mouse. `anchor_y` cresce verso l'alto, il
    // delta del mouse cresce verso il basso → segno invertito.
    let delta = motion.delta;
    if delta != Vec2::ZERO {
        tuning.anchor_x = (tuning.anchor_x + delta.x * SHIELD_EDIT_SENSITIVITY)
        .clamp(SHIELD_ANCHOR_X_MIN, SHIELD_ANCHOR_X_MAX);
        tuning.anchor_y = (tuning.anchor_y - delta.y * SHIELD_EDIT_SENSITIVITY)
        .clamp(SHIELD_ANCHOR_Y_MIN, SHIELD_ANCHOR_Y_MAX);
    }

    // Distanza con la rotella.
    let mut scroll = 0.0f32;
    for ev in wheel.read() {
        scroll += ev.y;
    }
    if scroll != 0.0 {
        tuning.dist = (tuning.dist - scroll * SHIELD_EDIT_DIST_SENSITIVITY)
        .clamp(SHIELD_DIST_MIN, SHIELD_DIST_MAX);
    }
}

// =========================================================================
// 5. TIMER
// =========================================================================
fn tick_attack_timer(time: Res<Time>, mut state: ResMut<FpsWeaponState>) {
    let dt = time.delta_secs().min(0.1);

    let target = if state.drawn { 1.0 } else { 0.0 };
    let p = state.draw_progress + (target - state.draw_progress) * dt * DRAW_SPEED;
    state.draw_progress = p.clamp(0.0, 1.0);

    if state.attack_active {
        state.attack_timer += dt;
        if state.attack_timer >= ATTACK_DURATION {
            state.attack_active = false;
            state.attack_timer = 0.0;
        }
    }

    if state.overhead_active {
        state.overhead_timer += dt;
        if state.overhead_timer >= OVERHEAD_DURATION {
            state.overhead_active = false;
            state.overhead_timer = 0.0;
        }
    }
}

// =========================================================================
// 6. POSA
// =========================================================================

pub fn update_weapon_pose(
    time: Res<Time>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut state: ResMut<FpsWeaponState>,
    mut tip: ResMut<SwordTip>,
    mut tip_blend: Local<f32>,
    mut q: Query<
    (&mut Transform, &mut Visibility, &FpsWeaponBillboard),
                          With<FpsWeaponBillboard>,
                          >,
) {
    let dt = time.delta_secs().min(0.1);

    let aim_angle = -state.aim.x * MAX_ANGLE_DEG.to_radians();

    let swing_target = if state.swinging { SWING_PITCH_MAX_RAD } else { 0.0 };
    let swing_speed = SWING_PITCH_MAX_RAD / SWING_RAMP_DURATION_SECS;
    let step = swing_speed * dt;
    let diff = swing_target - state.swing_pitch;
    if diff.abs() <= step {
        state.swing_pitch = swing_target;
    } else {
        state.swing_pitch += diff.signum() * step;
    }

    let (attack_pitch, attack_z) = if state.swinging
    || state.overhead_active
    || state.rmb_held
    {
        (0.0, 0.0)
    } else {
        match state.current_attack() {
            Some((special, t)) => {
                let mult = if special { POWER_ATTACK_MULT } else { 1.0 };
                let s = (PI * t).sin() * mult;
                (-s * ATTACK_PITCH_MAX_RAD, -s * THRUST_DEPTH)
            }
            None => (0.0, 0.0),
        }
    };

    let swing_progress = (state.swing_pitch / SWING_PITCH_MAX_RAD).clamp(0.0, 1.0);
    let swing_z = -swing_progress * SWING_THRUST_DEPTH;

    let (oh_pitch, oh_dy, oh_dz) = state
    .current_overhead()
    .map(overhead_pose)
    .unwrap_or((0.0, 0.0, 0.0));

    // Affondo RMB puro: la punta segue il cursore.
    let thrust_aim = state.rmb_held
    && !state.parrying
    && !state.swinging
    && !state.overhead_active
    && !state.editing_shield;

    let rmb_base_pitch = if thrust_aim
    {
        RMB_MAX_PITCH
    } else {
        0.0
    };

    let total_pitch = rmb_base_pitch - state.swing_pitch + attack_pitch + oh_pitch;
    let total_z = attack_z + swing_z + oh_dz;

    let rot = Quat::from_rotation_z(aim_angle) * Quat::from_rotation_x(total_pitch);

    let anchor = Vec3::new(ANCHOR_X, ANCHOR_Y, -WEAPON_DIST);

    let follow_factor = if state.is_aiming_with_attack() {
        AIM_FOLLOW_FACTOR_AIM
    } else {
        AIM_FOLLOW_FACTOR_BASE
    };
    let aspect = windows
    .get_single()
    .map(|w| (w.width() / w.height().max(1.0)).max(1e-3))
    .unwrap_or(16.0 / 9.0);
    let half_h = WEAPON_DIST * (WEAPON_FOV_VERTICAL_DEG * 0.5).to_radians().tan();
    let half_w = half_h * aspect;
    let aim_offset = {
        let cursor_cam = Vec2::new(state.aim.x * half_w, state.aim.y * half_h);
        let delta = cursor_cam - Vec2::new(ANCHOR_X, ANCHOR_Y);
        Vec3::new(
            delta.x * follow_factor,
            delta.y * follow_factor + oh_dy,
            0.0,
        )
    };

    let visible = state.draw_progress >= VISIBLE_THRESHOLD && state.has_texture;
    let target_vis = if visible { Visibility::Visible } else { Visibility::Hidden };

    // Blend riposo → punta sul cursore (smoothstep).
    let blend_dir = if thrust_aim { 1.0 } else { -1.0 };
    *tip_blend = (*tip_blend + blend_dir * dt / TIP_BLEND_SECS).clamp(0.0, 1.0);
    let blend = *tip_blend * *tip_blend * (3.0 - 2.0 * *tip_blend);

    for (mut tf, mut vis, billboard) in &mut q {
        let (w, h) = (billboard.quad.x, billboard.quad.y);
        let pivot = Vec3::new(0.0, -h * 0.5, 0.0);

        // --- Posa di riposo (invariata) ---
        let rest_translation = anchor - rot * pivot
        + Vec3::new(0.0, 0.0, total_z)
        + aim_offset;

        // --- Posa "punta sul cursore" (dimensione costante) ---
        // Il quad è parallelo allo schermo (ruota solo su Z). Direzione
        // ancoraggio → cursore: la spada ci si allinea e la punta cade sul
        // cursore; la base scivola lungo la stessa direzione: avanza verso
        // il cursore se è più lontano di una lunghezza di spada, arretra
        // sotto l'ancoraggio (fuori schermo in basso) se è più vicino.
        // Sempre a `-WEAPON_DIST`, quindi mai dietro la camera.
        let a = Vec2::new(anchor.x, anchor.y);
        // Vettore base → punta (angolo in alto a sinistra) nel quad.
        let v_local = Vec2::new(-w * 0.5, h);
        let reach = v_local.length().max(1e-4) * TIP_SCALE;
        let tip_ndc = state.aim.clamp(TIP_NDC_MIN, TIP_NDC_MAX);
        let target = Vec2::new(tip_ndc.x * half_w, tip_ndc.y * half_h);
        let d = target - a;
        let dist = d.length();
        let dir = if dist > 1e-4 { d / dist } else { Vec2::Y };
        let theta = dir.y.atan2(dir.x) - v_local.y.atan2(v_local.x);
        let base = a + dir * (dist - reach);
        let tr_rot = Quat::from_rotation_z(theta);
        let tr_translation =
        Vec3::new(base.x, base.y, -WEAPON_DIST) - tr_rot * (pivot * TIP_SCALE);
        // La punta cade sempre sul cursore.
        let tip_xy = target;

        tf.rotation = rot.slerp(tr_rot, blend);
        tf.translation = rest_translation.lerp(tr_translation, blend);
        tf.scale = Vec3::ONE.lerp(Vec3::splat(TIP_SCALE), blend);

        // Direzione di mira (spazio camera) usata da
        // `combat::apply_player_strike`, aggiornata ogni frame: la punta
        // reale durante l'affondo, il cursore per power attack e fendente
        // dall'alto (la cui animazione non porta la punta sul cursore).
        let aim_xy = if thrust_aim { tip_xy } else { target };
        tip.dir = Some(Vec3::new(aim_xy.x, aim_xy.y, -WEAPON_DIST).normalize());

        if *vis != target_vis {
            *vis = target_vis;
        }
    }
}

fn overhead_pose(t: f32) -> (f32, f32, f32) {
    fn smooth(x: f32) -> f32 {
        x * x * (3.0 - 2.0 * x)
    }
    fn lerp3(a: (f32, f32, f32), b: (f32, f32, f32), k: f32) -> (f32, f32, f32) {
        (
            a.0 + (b.0 - a.0) * k,
         a.1 + (b.1 - a.1) * k,
         a.2 + (b.2 - a.2) * k,
        )
    }

    let t = t.clamp(0.0, 1.0);
    let rest = (0.0, 0.0, 0.0);
    let raised = (OVERHEAD_WINDUP_PITCH, OVERHEAD_RAISE, 0.0);
    let chopped = (-OVERHEAD_CHOP_PITCH, -OVERHEAD_DROP, -OVERHEAD_THRUST_DEPTH);

    if t < OVERHEAD_WINDUP_END {
        lerp3(rest, raised, smooth(t / OVERHEAD_WINDUP_END))
    } else if t < OVERHEAD_STRIKE_END {
        let u = (t - OVERHEAD_WINDUP_END) / (OVERHEAD_STRIKE_END - OVERHEAD_WINDUP_END);
        lerp3(raised, chopped, u * u)
    } else {
        let u = (t - OVERHEAD_STRIKE_END) / (1.0 - OVERHEAD_STRIKE_END);
        lerp3(chopped, rest, smooth(u))
    }
}

fn update_shield_pose(
    inventory: Res<PlayerInventory>,
    state: Res<FpsWeaponState>,
    tuning: Res<ShieldTuning>,
    mut shields: Query<(&mut Transform, &mut Visibility, &ShieldBillboard)>,
) {
    let Ok((mut tf, mut vis, shield)) = shields.get_single_mut() else {
        return;
    };

    let has_shield = inventory
    .items
    .iter()
    .any(|i| i.equipped && i.slot == EquipSlot::LeftHand);

    let visible = has_shield
    && shield.loaded
    && state.drawn
    && state.draw_progress >= VISIBLE_THRESHOLD;

    let target_vis = if visible {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    if *vis != target_vis {
        *vis = target_vis;
    }
    if !visible {
        return;
    }

    let anchor = Vec3::new(
        tuning.anchor_x,
        tuning.anchor_y,
        -tuning.dist - tuning.z_offset,
    );

    tf.rotation = Quat::IDENTITY;
    tf.translation = anchor - Vec3::new(0.0, -shield.quad.y * 0.5, 0.0);
    tf.scale = Vec3::ONE;
}

// =========================================================================
// 7. EMISSIONE DEI COLPI
// =========================================================================

pub fn emit_strikes(
    mut strikes: EventWriter<PlayerStrike>,
    mut state: ResMut<FpsWeaponState>,
) {
    if !state.drawn {
        return;
    }

    if state.attack_active && !state.attack_strike_emitted {
        let t = (state.attack_timer / ATTACK_DURATION).clamp(0.0, 1.0);
        if t >= 0.5 {
            let kind = if state.is_special {
                StrikeKind::PowerAttack
            } else {
                StrikeKind::Thrust
            };
            strikes.send(PlayerStrike { kind });
            state.attack_strike_emitted = true;
        }
    }

    if state.overhead_active && !state.overhead_strike_emitted {
        let t = (state.overhead_timer / OVERHEAD_DURATION).clamp(0.0, 1.0);
        if t >= OVERHEAD_HIT_T {
            strikes.send(PlayerStrike {
                kind: StrikeKind::Overhead,
            });
            state.overhead_strike_emitted = true;
        }
    }

    if state.swinging && !state.swing_strike_emitted {
        let threshold = SWING_PITCH_MAX_RAD * 0.9;
        if state.swing_pitch >= threshold {
            strikes.send(PlayerStrike {
                kind: StrikeKind::Swing,
            });
            state.swing_strike_emitted = true;
        }
    }
}

// =========================================================================
// 8. TEXTURE
// =========================================================================
type TextureIndex = HashMap<(String, Option<String>), String>;

fn fit_quad(w_px: f32, h_px: f32, max_size: f32, fallback: Vec2) -> Vec2 {
    if w_px <= 0.0 || h_px <= 0.0 || max_size <= 0.0 {
        return fallback;
    }
    let aspect = w_px / h_px;
    if aspect >= 1.0 {
        Vec2::new(max_size, max_size / aspect)
    } else {
        Vec2::new(max_size * aspect, max_size)
    }
}

#[allow(clippy::too_many_arguments)]
fn update_weapon_texture(
    inventory: Res<PlayerInventory>,
    mut state: ResMut<FpsWeaponState>,
    mut mods: ResMut<ModTextures>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut billboards: Query<(
        &mut MeshMaterial3d<StandardMaterial>,
        &mut Mesh3d,
        &mut FpsWeaponBillboard,
    )>,
    mut scanned: Local<bool>,
    mut index: Local<TextureIndex>,
    mut cache: Local<Option<(u32, Handle<StandardMaterial>, Vec2)>>,
                         mut failed_item: Local<Option<u32>>,
) {
    if !*scanned {
        let all: Vec<String> = mods
        .names()
        .map(|n| n.to_string())
        .chain(mods.loose_names().map(|n| n.to_string()))
        .collect();

        for name in &all {
            let lower = name.to_lowercase();
            if !lower.contains(".cif_0-0") || is_auxiliary_map(&lower) {
                continue;
            }
            let Some(p) = parse_mod_weapon_name(name) else {
                continue;
            };
            if p.archive != 0 || p.record != 0 {
                continue;
            }
            match index.get_mut(&(p.base.clone(), p.material.clone())) {
                Some(existing) => {
                    let existing_w = has_weapon_prefix(existing);
                    let name_w = has_weapon_prefix(name);
                    let should_replace = match (name_w, existing_w) {
                        (true, false) => true,
                        (false, true) => false,
                        _ => name.len() < existing.len(),
                    };
                    if should_replace {
                        *existing = name.clone();
                    }
                }
                None => {
                    index.insert((p.base, p.material), name.clone());
                }
            }
        }

        if !all.is_empty() {
            *scanned = true;
            info!(
                "FpsWeapon: indicizzate {} texture cif_0-0 su {} nomi.",
                index.len(),
                  all.len()
            );
        }
    }

    let Ok((mut mesh_mat, mut mesh3d, mut billboard)) = billboards.get_single_mut() else {
        return;
    };

    let weapon = inventory
    .items
    .iter()
    .find(|i| i.equipped && i.slot == EquipSlot::RightHand);

    let Some(w) = weapon else {
        *cache = None;
        if state.has_texture {
            state.has_texture = false;
        }
        return;
    };

    if let Some((id, handle, quad)) = cache.as_ref() {
        if *id == w.id {
            if mesh_mat.0 != *handle {
                mesh_mat.0 = handle.clone();
            }
            if billboard.quad != *quad {
                billboard.quad = *quad;
                let new_mesh = meshes.add(Rectangle::new(quad.x, quad.y));
                *mesh3d = Mesh3d(new_mesh);
            }
            if !state.has_texture {
                state.has_texture = true;
            }
            return;
        }
    }

    if *failed_item == Some(w.id) {
        if state.has_texture {
            state.has_texture = false;
        }
        return;
    }

    let (base, material): (Option<String>, Option<String>) = match parse_mod_weapon_name(&w.name) {
        Some(p) => (Some(p.base), p.material),
        None => (infer_base_from_name(&w.name), infer_material_from_name(&w.name)),
    };

    let file_name = base
    .as_deref()
    .and_then(|b| pick_cif00_name(&index, b, material.as_deref()))
    .cloned();

    let loaded = file_name
    .as_deref()
    .and_then(|n| mods.image_by_name(n, &mut images));

    match loaded {
        Some(image) => {
            let size_px = mods
            .image_size(&image)
            .or_else(|| images.get(&image).map(|i| i.size_f32()))
            .unwrap_or(Vec2::new(QUAD_W, QUAD_H));
            let quad = fit_quad(
                size_px.x,
                size_px.y,
                WEAPON_MAX_SIZE,
                Vec2::new(QUAD_W, QUAD_H),
            );

            let handle = materials.add(weapon_material(Some(image)));
            mesh_mat.0 = handle.clone();
            let new_mesh = meshes.add(Rectangle::new(quad.x, quad.y));
            *mesh3d = Mesh3d(new_mesh);
            billboard.quad = quad;
            *cache = Some((w.id, handle, quad));
            *failed_item = None;
            state.has_texture = true;
            info!(
                "FpsWeapon: texture arma '{}' caricata, quad {:.3}×{:.3} m (aspect nativo {:.2})",
                  file_name.as_deref().unwrap_or("?"),
                  quad.x, quad.y,
                  size_px.x / size_px.y.max(1e-3)
            );
        }
        None => {
            warn!(
                "FpsWeapon: nessuna texture cif_0-0 per '{}' (base {:?}, materiale {:?}, file {:?}). cif_0-0 nella mod: {:?}",
                  w.name, base, material, file_name,
                  mods.names_containing("cif_0-0", 20)
            );
            *cache = None;
            *failed_item = Some(w.id);
            state.has_texture = false;
        }
    }
}

fn update_shield_texture(
    inventory: Res<PlayerInventory>,
    mut mods: ResMut<ModTextures>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut shields: Query<(
        &mut MeshMaterial3d<StandardMaterial>,
        &mut Mesh3d,
        &mut ShieldBillboard,
    )>,
    mut cache: Local<Option<Handle<StandardMaterial>>>,
    mut tried: Local<bool>,
) {
    let Ok((mut mat, mut mesh3d, mut shield)) = shields.get_single_mut() else {
        return;
    };

    let has_shield = inventory
    .items
    .iter()
    .any(|i| i.equipped && i.slot == EquipSlot::LeftHand);
    if !has_shield {
        return;
    }

    if cache.is_none() && !*tried {
        *tried = true;
        if let Some(handle) = mods.image_by_name(SHIELD_TEXTURE, &mut images) {
            let size_px = mods
            .image_size(&handle)
            .or_else(|| images.get(&handle).map(|i| i.size_f32()))
            .unwrap_or(Vec2::new(SHIELD_QUAD_W, SHIELD_QUAD_H));
            let quad = fit_quad(
                size_px.x,
                size_px.y,
                SHIELD_MAX_SIZE,
                Vec2::new(SHIELD_QUAD_W, SHIELD_QUAD_H),
            );

            let handle_mat = materials.add(weapon_material(Some(handle)));
            *cache = Some(handle_mat);
            let new_mesh = meshes.add(Rectangle::new(quad.x, quad.y));
            *mesh3d = Mesh3d(new_mesh);
            shield.quad = quad;
            shield.loaded = true;
            info!(
                "FpsShield: texture '{}' caricata, quad {:.3}×{:.3} m (aspect nativo {:.2})",
                  SHIELD_TEXTURE,
                  quad.x, quad.y,
                  size_px.x / size_px.y.max(1e-3)
            );
        } else {
            warn!(
                "FpsShield: texture '{}' non trovata (né .dfmod né StreamingAssets/Textures). \
Lo scudo resterà invisibile finché la texture non viene fornita.",
SHIELD_TEXTURE
            );
        }
    }

    if let Some(cached) = cache.as_ref() {
        if mat.0 != *cached {
            mat.0 = cached.clone();
        }
    }
}

fn has_weapon_prefix(raw: &str) -> bool {
    let lower = raw.to_lowercase();
    let clean = lower.split(" #").next().unwrap_or(&lower).trim();
    clean.starts_with("w_")
}

fn is_auxiliary_map(lower: &str) -> bool {
    const AUX: &[&str] = &["normal", "metallic", "gloss", "emission", "mask", "roughness"];
    AUX.iter().any(|k| lower.contains(k))
}

fn pick_cif00_name<'a>(
    index: &'a TextureIndex,
    base: &str,
    material: Option<&str>,
) -> Option<&'a String> {
    let key = |m: Option<&str>| (base.to_string(), m.map(str::to_string));

    if let Some(m) = material {
        if let Some(n) = index.get(&key(Some(m))) {
            return Some(n);
        }
    }
    for m in ["steel", "iron"] {
        if let Some(n) = index.get(&key(Some(m))) {
            return Some(n);
        }
    }
    if let Some((_, n)) = index
        .iter()
        .filter(|(k, _)| k.0 == base && k.1.is_some())
        .min_by(|a, b| a.0 .1.cmp(&b.0 .1))
        {
            return Some(n);
        }
        index.get(&key(None))
}

// =========================================================================
// PARSING DEL NOME
// =========================================================================
#[derive(Debug, PartialEq, Eq)]
struct Parsed {
    base: String,
    archive: u8,
    record: u8,
    material: Option<String>,
}

fn parse_mod_weapon_name(raw: &str) -> Option<Parsed> {
    let clean = raw.split(" #").next().unwrap_or(raw).trim();
    let lower = clean.to_lowercase();

    let s = lower
    .strip_suffix(".png")
    .or_else(|| lower.strip_suffix(".tga"))
    .unwrap_or(&lower);
    let s = s.strip_prefix("w_").unwrap_or(s);

    let (base, meta) = s.split_once(".cif_")?;
    if base.is_empty() {
        return None;
    }

    let (nums, material) = match meta.split_once('_') {
        Some((n, m)) if !m.is_empty() => (n, Some(m.to_string())),
        Some((n, _)) => (n, None),
        None => (meta, None),
    };

    let (a, r) = nums.split_once('-')?;
    let archive: u8 = a.parse().ok()?;
    let record: u8 = r.parse().ok()?;

    Some(Parsed {
        base: base.to_string(),
         archive,
         record,
         material,
    })
}

fn infer_base_from_name(name: &str) -> Option<String> {
    let n = name.to_lowercase().replace(' ', "");
    const BASES: &[&str] = &[
        "daikatana", "claymore", "broadsword", "longsword", "shortsword", "wakazashi", "tanto",
        "dagger", "saber", "katana", "battleaxe", "waraxe", "warhammer", "flail", "mace", "staff",
        "bow", "arrow",
    ];
    BASES.iter().find(|b| n.contains(**b)).map(|b| b.to_string())
}

fn infer_material_from_name(name: &str) -> Option<String> {
    let n = name.to_lowercase().replace(' ', "");
    const MATERIALS: &[&str] = &[
        "daedric", "orcish", "ebony", "adamantium", "mithril", "dwarven", "elven", "silver",
        "steel", "iron",
    ];
    MATERIALS.iter().find(|m| n.contains(**m)).map(|m| m.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_unity_names_with_suffix() {
        let p = parse_mod_weapon_name("claymore.cif_2-2_ebony #5911").unwrap();
        assert_eq!(
            p,
            Parsed { base: "claymore".into(), archive: 2, record: 2, material: Some("ebony".into()) }
        );
    }

    #[test]
    fn weapon_prefix_detection() {
        assert!(has_weapon_prefix("w_LONGSWORD.CIF_0-0_Elven"));
        assert!(!has_weapon_prefix("LONGSWORD.CIF_0-0_Elven"));
    }

    #[test]
    fn is_parrying_requires_weapon_drawn_and_either_button() {
        let mut state = FpsWeaponState::default();
        state.parrying = true;
        state.rmb_held = true;
        state.mmb_held = true;
        assert!(!state.is_parrying());
        state.drawn = true;
        state.parrying = false;
        state.rmb_held = false;
        state.mmb_held = false;
        assert!(!state.is_parrying());
        state.parrying = true;
        assert!(state.is_parrying());
        state.parrying = false;
        state.rmb_held = true;
        assert!(state.is_parrying());
        state.rmb_held = false;
        state.mmb_held = true;
        assert!(state.is_parrying());
    }

    #[test]
    fn editing_shield_counts_as_parrying_for_camera_lock() {
        let mut state = FpsWeaponState { drawn: true, ..Default::default() };
        assert!(!state.is_parrying());
        state.editing_shield = true;
        assert!(state.is_parrying());
        // Non conta come "parry active" (non è una parata vera).
        assert!(!state.is_parry_active());
    }

    #[test]
    fn is_parry_active_is_stricter_than_is_parrying() {
        let mut state = FpsWeaponState { drawn: true, ..Default::default() };
        assert!(!state.is_parry_active());
        state.parrying = true;
        assert!(state.is_parry_active());
        state.attack_active = true;
        assert!(!state.is_parry_active());
        assert!(state.is_parrying());
        state.attack_active = false;
        state.swinging = true;
        assert!(!state.is_parry_active());
        assert!(state.is_parrying());
        state.swinging = false;
        state.parrying = false;
        state.rmb_held = true;
        assert!(!state.is_parry_active());
        assert!(state.is_parrying());
    }

    #[test]
    fn shield_tuning_load_and_save_roundtrip() {
        let dir = std::env::temp_dir().join(format!("shield_pose_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shield_pose.txt");

        // File assente: default.
        let t = ShieldTuning::load(&path);
        assert_eq!(t.anchor_x, SHIELD_ANCHOR_X);
        assert_eq!(t.anchor_y, SHIELD_ANCHOR_Y);
        assert_eq!(t.dist, SHIELD_DIST);
        assert_eq!(t.z_offset, SHIELD_Z_OFFSET);

        // Salva valori custom e ricarica.
        let custom = ShieldTuning { anchor_x: -0.5, anchor_y: -0.9, dist: 0.8, z_offset: 0.1 };
        custom.save(&path);
        assert!(path.exists());
        let back = ShieldTuning::load(&path);
        assert_eq!(back.anchor_x, -0.5);
        assert_eq!(back.anchor_y, -0.9);
        assert_eq!(back.dist, 0.8);
        assert_eq!(back.z_offset, 0.1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shield_tuning_clamp_values() {
        // Verifica i limiti usati nell'edit.
        let mut t = ShieldTuning::default();
        t.anchor_x = 10.0;
        t.anchor_x = t.anchor_x.clamp(SHIELD_ANCHOR_X_MIN, SHIELD_ANCHOR_X_MAX);
        assert_eq!(t.anchor_x, SHIELD_ANCHOR_X_MAX);
        t.dist = 0.0;
        t.dist = t.dist.clamp(SHIELD_DIST_MIN, SHIELD_DIST_MAX);
        assert_eq!(t.dist, SHIELD_DIST_MIN);
    }

    #[test]
    fn overhead_pose_starts_and_ends_at_rest() {
        let (p0, y0, z0) = overhead_pose(0.0);
        let (p1, y1, z1) = overhead_pose(1.0);
        for v in [p0, y0, z0, p1, y1, z1] {
            assert!(v.abs() < 1e-5, "la posa deve partire e finire a riposo");
        }
    }

    #[test]
    fn overhead_pose_winds_up_then_chops() {
        let (p, y, _) = overhead_pose(OVERHEAD_WINDUP_END);
        assert!(p > 0.0 && y > 0.0);
        let (p, _, z) = overhead_pose(OVERHEAD_STRIKE_END);
        assert!(p < 0.0 && z < 0.0);
        let (p_hit, _, _) = overhead_pose(OVERHEAD_HIT_T);
        assert!(p_hit < 0.0, "al momento del colpo l'arma deve essere in avanti");
        assert!(OVERHEAD_WINDUP_END < OVERHEAD_HIT_T && OVERHEAD_HIT_T < OVERHEAD_STRIKE_END);
    }

    #[test]
    fn overhead_blocks_parry_and_locks_camera() {
        let mut state = FpsWeaponState { drawn: true, parrying: true, ..Default::default() };
        assert!(state.is_parry_active());
        state.overhead_active = true;
        assert!(!state.is_parry_active());
        assert!(state.is_parrying());
        state.parrying = false;
        assert!(state.is_parrying(), "camera lock anche a tasti rilasciati");
        assert!(state.is_overhead_active());
        assert!(state.current_overhead().is_some());
    }

    #[test]
    fn rmb_hold_pitches_weapon_at_max_toward_enemy() {
        assert!(
            (RMB_MAX_PITCH - (-ATTACK_PITCH_MAX_RAD)).abs() < 1e-6,
                "RMB_MAX_PITCH deve essere -ATTACK_PITCH_MAX_RAD (inclinazione massima)"
        );
        assert!(RMB_MAX_PITCH < 0.0);
        assert!((RMB_REPEAT_INTERVAL - 1.0).abs() < 1e-6);
    }

    #[test]
    fn stamina_costs_are_as_expected() {
        assert!((OVERHEAD_STAMINA_COST - 8.0).abs() < 1e-6);
        assert!((ATTACK_STAMINA_COST - 5.0).abs() < 1e-6);
        assert!((SWING_STAMINA_COST - 5.0).abs() < 1e-6);
        assert!((POWER_ATTACK_STAMINA_COST - 10.0).abs() < 1e-6);
    }

    #[test]
    fn shield_texture_constant_is_the_expected_stem() {
        assert_eq!(SHIELD_TEXTURE, "112361_13-0");
    }

    #[test]
    fn fit_quad_keeps_aspect_and_fits_in_box() {
        let m = 10.0;
        let q = fit_quad(100.0, 100.0, m, Vec2::ONE);
        assert!((q.x - 10.0).abs() < 1e-6 && (q.y - 10.0).abs() < 1e-6);
        let q = fit_quad(50.0, 100.0, m, Vec2::ONE);
        assert!((q.x - 5.0).abs() < 1e-6 && (q.y - 10.0).abs() < 1e-6);
        let q = fit_quad(200.0, 100.0, m, Vec2::ONE);
        assert!((q.x - 10.0).abs() < 1e-6 && (q.y - 5.0).abs() < 1e-6);
        let fb = Vec2::new(3.0, 4.0);
        assert_eq!(fit_quad(0.0, 100.0, m, fb), fb);
        assert_eq!(fit_quad(100.0, 0.0, m, fb), fb);
    }

    #[test]
    fn follow_factor_is_base_without_attack_and_larger_with_attack() {
        let base = AIM_FOLLOW_FACTOR_BASE;
        let aim = AIM_FOLLOW_FACTOR_AIM;
        assert!(base > 0.0);
        assert!(aim > base);
        let mut state = FpsWeaponState { drawn: true, ..Default::default() };
        assert!(!state.is_aiming_with_attack());
        state.rmb_held = true;
        assert!(state.is_aiming_with_attack());
        state.rmb_held = false;
        state.mmb_held = true;
        assert!(state.is_aiming_with_attack());
        state.mmb_held = false;
        assert!(!state.is_aiming_with_attack());
    }
}
