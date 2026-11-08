//! Navi con fisica Rapier e galleggiamento sulle onde GPU.
//!
//! ## Due tipi di nave
//!
//! - **`ShipKind::Medieval`** (default): il `ship.glb` classico, caricato come
//!   `SceneRoot`. Collider trimesh ricavato dai mesh del GLB dopo il
//!   caricamento (`build_ship_collider`), punti di galleggiamento hardcoded
//!   (`hull_points_medieval`).
//! - **`ShipKind::Elven`**: veliero procedurale (`ships::elven_ship::build()`),
//!   **scalato di `ELVEN_SHIP_SCALE` (default 2×)**, con collider ricreato a
//!   scala 2 (`scaled_collider_from_built`). Mesh visiva figlia di un nodo
//!   intermedio `content` (rotazione `-π/2`, scala 2×). Vele e bandiere usano
//!   `SailMaterial` (vertex shader `assets/shaders/sail_animator.wgsl`),
//!   animate da `update_sail_time` in `ElvenShipPlugin`.
//!
//! ## Due navi sempre (principale + companion)
//!
//! `spawn_initial_ship` spawna la nave principale (`config.kind`) e una
//! **companion** dell'altro tipo, ad almeno `COMPANION_MIN_SEP` metri di
//! distanza. `respawn_ship_near_player` (chiamato da `fast_travel.rs` ad ogni
//! viaggio rapido) **fa lo stesso**: despawna tutte le navi esistenti e ne
//! spawna due nuove (principale + companion). Così dopo ogni teletrasporto
//! hai sempre due puntini viola sulla mappa.
//!
//! ## Draft, offset e massa per tipo
//!
//! `ship_y_offset`, `ship_draft`, `ship_mass` restituiscono i valori corretti
//! per il tipo. `ship_hydro` calibra `k_pt` in modo che
//! `k_pt * draft * N = massa * G`.
//!
//! ## Limite noto
//!
//! Il CPU mirror `OnceLock<ShipCollider>` (`deck_height_at`, `ship_floor_at`,
//! `ship_blocked_at`, `probe_ship`) viene popolato **dalla prima nave che
//! spawna**. Se in scena ci sono Medieval + Elven insieme, solo la prima
//! funziona col CPU mirror; l'altra usa il collider Rapier.
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use bevy::prelude::*;
use bevy::render::mesh::PrimitiveTopology;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{
    Buffer, BufferDescriptor, BufferUsages, CommandEncoderDescriptor, Extent3d, ImageCopyBuffer,
    ImageDataLayout, MapMode,
};
use bevy::render::renderer::{render_system, RenderDevice, RenderQueue};
use bevy::render::texture::GpuImage;
use bevy::render::{Render, RenderApp, RenderSet};
use bevy_rapier3d::prelude::*;

use crate::data::generate_default_samples;
use crate::data::heightmap::OCEAN_ELEVATION;
use crate::data::terrain_mesh::{terrain_height, CELL_SIZE, TERRAIN_HEIGHT_SCALE};
use crate::dialog::DialogState;
use crate::dungeon::DungeonState;
use crate::equipment::EquipmentState;
use crate::interior::InteriorState;
use crate::ocean::{
    OceanAssets, OceanSea, CHOPPINESS_CALM, CHOPPINESS_STORM, NUM_CASCADES, OCEAN_SIZE,
    TILE_LENGTHS,
};
use crate::player::{ClimbTarget, Player, EYE_HEIGHT};
use crate::ships::elven_ship::helm_sails::ElvenAnimatedMaterials;
use crate::ships::elven_ship::{self, ElvenMaterials, SailMaterial};
use crate::ships::{AnimatedPart, ElvenMaterial, ShipKind};
use crate::tiles::WallHit;
use crate::world::{
    in_map, map_pixel_of, tile_translation, GameData, HeightField, Origin, MAP_HEIGHT_PIXELS,
    MAP_WIDTH_PIXELS,
};
use crate::StartInfo;

// ---------------------------------------------------------------------------
// Scala del veliero elfico
// ---------------------------------------------------------------------------

/// Moltiplicatore dimensionale del veliero elfico. Con 2.0 la nave è 168 m
/// (lunghezza), 24 m (larghezza), 94 m (altezza). Per tornare alle
/// dimensioni originali basta mettere 1.0.
pub const ELVEN_SHIP_SCALE: f32 = 2.0;

// ---------------------------------------------------------------------------
// Costanti di gameplay / geometria (Medieval)
// ---------------------------------------------------------------------------

const SHIP_Y_OFFSET: f32 = -8.0;
const SHIP_SCALE: f32 = 3.0;
const DECK_Y: f32 = 14.0;
const DECK_LOCAL_Y: f32 = DECK_Y * SHIP_SCALE;
const Z_STERN: f32 = 7.0;
const Z_BOW: f32 = -28.0;
const HULL_BEAM: f32 = 2.6;
const HELM_STAND_Z: f32 = 7.4;
const BOARDING_DISTANCE: f32 = 8.0 * SHIP_SCALE;
const SHIP_CLEARANCE_CELLS: usize = 8;

/// Distanza minima (metri) tra nave principale e companion. Deve essere
/// maggiore della somma delle semilunghezze (Elven 168 m + Medieval 63 m =
/// 231 m), altrimenti le carene si sovrappongono e la camera dell'una
/// finisce dentro la mesh dell'altra. 500 m lascia margine.
const COMPANION_MIN_SEP: f32 = 500.0;

// ---------------------------------------------------------------------------
// TUNING CAMERA
// ---------------------------------------------------------------------------

const MAST_TOP_HEIGHT: f32 = 84.0;
const MAST_TOP_HEIGHT_ELVEN: f32 = 47.0;
const MAST_TOP_Z: f32 = -19.0;
const MAST_TOP_Z_ELVEN: f32 = 0.0;

// ---------------------------------------------------------------------------

const SCAN_STEP: usize = 4;
const SEARCH_RADIUS_NORMAL: i32 = 120;
const SEARCH_RADIUS_EXTENDED: i32 = 160;
const WATER_PIXEL_FAST_THRESHOLD: u8 = 4;

const MODEL_PATH: &str = "ship.glb#Scene0";
const MODEL_SCALE: f32 = 1.0;
const MODEL_OFFSET: Vec3 = Vec3::new(0.0, 0.0, 0.0);
const MODEL_YAW: f32 = 0.0;

const G: f32 = 9.81;
const SHIP_MASS: f32 = 400_000.0;
const SHIP_DRAFT: f32 = -SHIP_Y_OFFSET;
const SHIP_COM_Y: f32 = 2.0;
const SHIP_COM_Z: f32 = (Z_STERN + Z_BOW) / 2.0 * SHIP_SCALE;
const SHIP_COM: Vec3 = Vec3::new(0.0, SHIP_COM_Y, SHIP_COM_Z);
const SHIP_W: f32 = 2.0 * HULL_BEAM * SHIP_SCALE;
const SHIP_L: f32 = (Z_STERN - Z_BOW) * SHIP_SCALE;
const SHIP_H: f32 = DECK_LOCAL_Y + 4.0;
const INERTIA_FILL: f32 = 0.5;
const OCEAN_PLANE_Y: f32 = OCEAN_ELEVATION * TERRAIN_HEIGHT_SCALE;
const MAX_DEPTH: f32 = 24.0;
const SUBMERGE_REF: f32 = 2.0;
const HEAVE_DAMPING_RATIO: f32 = 0.4;
const DRAG_LATERAL: f32 = 2.5;
const DRAG_FORWARD: f32 = 0.15;
const ANGULAR_DAMPING: f32 = 1.0;
const GROUND_STIFFNESS: f32 = 4.0;
const GROUND_FRICTION: f32 = 4.0;

const ACCELERATION: f32 = 5.2;
const MAX_SPEED_FWD: f32 = 120.0;
const MAX_SPEED_BWD: f32 = 4.0;
const RUDDER_ACCEL: f32 = 5.5;
const RUDDER_REF_SPEED: f32 = 2.0;

const STEP_HEIGHT: f32 = 0.6;
const PLAYER_RADIUS: f32 = 0.4;
const PLAYER_MASS: f32 = 80.0;
/// Raggio entro cui `ship_walk_pre`/`ship_walk_post` legano il player alla
/// nave più vicina. Con l'Elven 2× (168 m di lunghezza) serve ≥ 120 m.
const WALK_RANGE: f32 = 150.0;
const LEAVE_AFTER: f32 = 0.6;
const CARRY_YAW: bool = true;

const WALKABLE_NY: f32 = 0.7;
const GRID_CELL: f32 = 3.0;

// ---------------------------------------------------------------------------
// Helper per tipo di nave (offset, draft, massa, boarding)
// ---------------------------------------------------------------------------

/// Offset verticale dalla superficie dell'acqua alla radice della nave.
pub fn ship_y_offset(kind: ShipKind) -> f32 {
    match kind {
        ShipKind::Medieval => SHIP_Y_OFFSET,
        ShipKind::Elven => SHIP_Y_OFFSET * 0.375 * ELVEN_SHIP_SCALE,
    }
}

/// Draft "fisico" della nave: di quanti metri i punti di galleggiamento
/// stanno sotto l'acqua quando la nave è in equilibrio.
pub fn ship_draft(kind: ShipKind) -> f32 {
    match kind {
        ShipKind::Medieval => SHIP_DRAFT,
        ShipKind::Elven => SHIP_DRAFT * 0.375 * ELVEN_SHIP_SCALE,
    }
}

/// Massa del corpo rigido. Per Elven scala col cubo delle dimensioni.
pub fn ship_mass(kind: ShipKind) -> f32 {
    match kind {
        ShipKind::Medieval => SHIP_MASS,
        ShipKind::Elven => SHIP_MASS * 1.35 * ELVEN_SHIP_SCALE.powi(3),
    }
}

/// Distanza entro cui il player può salire a bordo (tasto E).
fn boarding_distance(kind: ShipKind) -> f32 {
    match kind {
        ShipKind::Medieval => BOARDING_DISTANCE,
        ShipKind::Elven => 120.0 * ELVEN_SHIP_SCALE,
    }
}

// ---------------------------------------------------------------------------
// Camera della nave
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ShipCameraSlot {
    #[default]
    Helm,
    MastTop,
    Bow,
    Center,
    Stern,
}

impl ShipCameraSlot {
    pub const ALL: [Self; 5] = [
        Self::Helm,
        Self::MastTop,
        Self::Bow,
        Self::Center,
        Self::Stern,
    ];

    pub fn next(self) -> Self {
        match self {
            Self::Helm => Self::MastTop,
            Self::MastTop => Self::Bow,
            Self::Bow => Self::Center,
            Self::Center => Self::Stern,
            Self::Stern => Self::Helm,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Helm => "timone",
            Self::MastTop => "cima albero",
            Self::Bow => "prua",
            Self::Center => "centro",
            Self::Stern => "poppa",
        }
    }

    pub fn local_offset(self, kind: ShipKind) -> Vec3 {
        let s = match kind {
            ShipKind::Medieval => 1.0,
            ShipKind::Elven => ELVEN_SHIP_SCALE,
        };
        let deck = match kind {
            ShipKind::Medieval => DECK_LOCAL_Y,
            ShipKind::Elven => 5.0 * s,
        };
        match self {
            Self::Helm => match kind {
                ShipKind::Medieval => Vec3::new(0.0, deck, HELM_STAND_Z * SHIP_SCALE),
                ShipKind::Elven => Vec3::new(0.0, 15.5 * s, -26.0 * s),
            },
            Self::MastTop => match kind {
                ShipKind::Medieval => Vec3::new(0.0, MAST_TOP_HEIGHT, MAST_TOP_Z),
                ShipKind::Elven => Vec3::new(0.0, MAST_TOP_HEIGHT_ELVEN * s, MAST_TOP_Z_ELVEN * s),
            },
            Self::Bow => match kind {
                ShipKind::Medieval => Vec3::new(0.0, deck, Z_BOW * SHIP_SCALE * 2.0),
                ShipKind::Elven => Vec3::new(0.0, deck, 40.0 * s),
            },
            Self::Center => Vec3::new(0.0, deck, 0.0),
            Self::Stern => match kind {
                ShipKind::Medieval => Vec3::new(0.0, deck, Z_STERN * SHIP_SCALE * 2.0),
                ShipKind::Elven => Vec3::new(0.0, deck, -40.0 * s),
            },
        }
    }

    pub fn pitch_offset(self) -> f32 {
        match self {
            Self::MastTop => -0.25,
            _ => 0.0,
        }
    }

    pub fn snaps_to_floor(self) -> bool {
        !matches!(self, Self::MastTop)
    }
}

#[derive(Resource, Default)]
pub struct ShipCamera {
    pub slot: ShipCameraSlot,
}

// ---------------------------------------------------------------------------
// Componenti / risorse
// ---------------------------------------------------------------------------

#[derive(Component)]
pub struct Ship {
    pub kind: ShipKind,
}

#[derive(Component, Default)]
pub struct ShipMotion {
    pub speed: f32,
}

#[derive(Component, Default)]
pub struct ShipHydro {
    pub submerged: f32,
}

#[derive(Resource, Default)]
pub struct ShipState {
    pub player_aboard: Option<Entity>,
}

#[derive(Component)]
struct ShipPhysics;

#[derive(Resource)]
struct ShipPhysicsCtx(Entity);

#[derive(Resource)]
struct ShipColliderTemplate(Collider);

#[derive(Resource, Default)]
struct ShipWalk {
    ship: Option<Entity>,
    local: Vec3,
    last_yaw: f32,
    pre_pos: Vec3,
    pre_feet: f32,
    origin: (i32, i32),
    airborne: f32,
}

#[derive(Event, Debug, Clone, Copy)]
pub struct RespawnShipRequest {
    pub remove_old: bool,
}

/// Materiali + handle animati del veliero elfico. `animated` contiene due
/// `Handle<SailMaterial>` (vele e bandiere) il cui `params` viene aggiornato
/// ogni frame da `ElvenShipPlugin::update_sail_time`.
///
/// Le **vele** usano `assets/textures/vele.png` (via AssetServer). Se il PNG
/// non c'è, `ElvenAnimatedMaterials::load` ricade su una texture procedurale
/// e scrive un warning nel log.
#[derive(Resource)]
pub struct ElvenShipAssets {
    pub materials: ElvenMaterials,
    pub animated: ElvenAnimatedMaterials,
}

impl ElvenShipAssets {
    fn load(
        sail_mats: &mut Assets<SailMaterial>,
        materials: &mut Assets<StandardMaterial>,
        images: &mut Assets<Image>,
        asset_server: &AssetServer,
    ) -> Self {
        Self {
            materials: ElvenMaterials::load(materials, images),
            animated: ElvenAnimatedMaterials::load(sail_mats, images, asset_server),
        }
    }
}

/// Configurazione del gioco: quale tipo di nave spawnare come principale.
/// La companion è l'altro tipo.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct ShipConfig {
    pub kind: ShipKind,
}

pub struct ShipPlugin;

impl Plugin for ShipPlugin {
    fn build(&self, app: &mut App) {
        let wave = WaveField::default();

        app.insert_resource(wave.clone())
        .insert_resource(TimestepMode::Variable {
            max_dt: 1.0 / 30.0,
            time_scale: 1.0,
            substeps: 2,
        })
        .add_plugins(
            RapierPhysicsPlugin::<NoUserData>::default().with_custom_initialization(
                RapierContextInitialization::NoAutomaticRapierContext,
            ),
        )
        .add_plugins(crate::ships::elven_ship::ElvenShipPlugin)
        .init_resource::<ShipState>()
        .init_resource::<ShipWalk>()
        .init_resource::<ShipCamera>()
        .init_resource::<ShipConfig>()
        .add_event::<RespawnShipRequest>()
        .add_systems(Startup, setup_elven_assets)
        .add_systems(PreStartup, spawn_ship_physics_context)
        .add_systems(PostStartup, spawn_initial_ship)
        .add_systems(
            Update,
            (
                ship_boarding,
             reset_camera_on_aboard_change,
             cycle_ship_camera.before(crate::fps_weapon::update_weapon_input),
             ship_pilot,
             ship_walk_pre,
            )
            .chain()
            .before(crate::player::player_move),
        )
        .add_systems(Update, ship_walk_post.after(crate::player::player_move))
        .add_systems(
            Update,
            (
                ship_hydro,
             build_ship_collider,
             attach_ship_colliders,
             sync_ocean_wave_sampler,
             ship_debug,
             respawn_ship_near_player,
             ship_debug_recall,
            ),
        );

        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
            .insert_resource(wave)
            .init_resource::<WaveReadbackState>()
            .add_systems(
                Render,
                readback_displacement
                .after(render_system)
                .in_set(RenderSet::Render),
            );
        }
    }
}

fn setup_elven_assets(
    mut commands: Commands,
    mut sail_mats: ResMut<Assets<SailMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    assets: Res<AssetServer>,
) {
    commands.insert_resource(ElvenShipAssets::load(
        &mut sail_mats,
        &mut materials,
        &mut images,
        &assets,
    ));
}

fn spawn_ship_physics_context(mut commands: Commands) {
    let e = commands
    .spawn((
        Name::new("ShipPhysics"),
            RapierContext::default(),
            DefaultRapierContext,
            ShipPhysics,
    ))
    .id();
    commands.insert_resource(ShipPhysicsCtx(e));
}

// ---------------------------------------------------------------------------
// Ciclo camera (tasto Z al timone)
// ---------------------------------------------------------------------------

fn reset_camera_on_aboard_change(
    ship_state: Res<ShipState>,
    mut cam: ResMut<ShipCamera>,
    mut last: Local<Option<Entity>>,
) {
    let now = ship_state.player_aboard;
    if now != *last {
        if now.is_some() {
            cam.slot = ShipCameraSlot::Helm;
            info!("Camera nave: timone (default all'imbarco)");
        }
        *last = now;
    }
}

fn cycle_ship_camera(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    ship_state: Res<ShipState>,
    dialog: Res<DialogState>,
    equipment: Option<Res<EquipmentState>>,
    mut cam: ResMut<ShipCamera>,
) {
    if ship_state.player_aboard.is_none() {
        return;
    }
    if dialog.open || equipment.as_ref().is_some_and(|e| e.open) {
        return;
    }
    if !keys.just_pressed(KeyCode::KeyZ) {
        return;
    }
    cam.slot = cam.slot.next();
    info!("Camera nave: {}", cam.slot.label());
    keys.clear_just_pressed(KeyCode::KeyZ);
}
// ---------------------------------------------------------------------------
// Onde: readback GPU della texture displacement
// ---------------------------------------------------------------------------

const TEXEL_BYTES: usize = 8;
const ROW_BYTES: usize = OCEAN_SIZE as usize * TEXEL_BYTES;
const LAYER_BYTES: usize = ROW_BYTES * OCEAN_SIZE as usize;
const WAVE_BYTES: usize = LAYER_BYTES * NUM_CASCADES as usize;

#[derive(Default)]
struct WaveSlot {
    data: Vec<u8>,
    seq: u64,
}

#[derive(Resource, Clone, Default)]
pub struct WaveField(Arc<Mutex<WaveSlot>>);

#[derive(Resource)]
struct WaveReadbackState {
    buffer: Option<Buffer>,
    busy: Arc<AtomicBool>,
}

impl Default for WaveReadbackState {
    fn default() -> Self {
        Self {
            buffer: None,
            busy: Arc::new(AtomicBool::new(false)),
        }
    }
}

fn readback_displacement(
    assets: Option<Res<OceanAssets>>,
    gpu_images: Res<RenderAssets<GpuImage>>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    wave: Res<WaveField>,
    mut state: ResMut<WaveReadbackState>,
) {
    let Some(assets) = assets else { return };
    if state.busy.load(Ordering::Acquire) {
        return;
    }
    let Some(gpu_image) = gpu_images.get(&assets.displacement) else {
        return;
    };

    let buffer = state
    .buffer
    .get_or_insert_with(|| {
        device.create_buffer(&BufferDescriptor {
            label: Some("ship_wave_readback"),
                             size: WAVE_BYTES as u64,
                             usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
                             mapped_at_creation: false,
        })
    })
    .clone();

    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("ship_wave_readback_copy"),
    });
    encoder.copy_texture_to_buffer(
        gpu_image.texture.as_image_copy(),
                                   ImageCopyBuffer {
                                       buffer: &buffer,
                                       layout: ImageDataLayout {
                                           offset: 0,
                                           bytes_per_row: Some(ROW_BYTES as u32),
                                   rows_per_image: Some(OCEAN_SIZE),
                                       },
                                   },
                                   Extent3d {
                                       width: OCEAN_SIZE,
                                       height: OCEAN_SIZE,
                                       depth_or_array_layers: NUM_CASCADES,
                                   },
    );
    queue.submit(std::iter::once(encoder.finish()));

    state.busy.store(true, Ordering::Release);
    let busy = state.busy.clone();
    let slot = wave.0.clone();
    let b2 = buffer.clone();
    buffer.slice(..).map_async(MapMode::Read, move |res| {
        if res.is_ok() {
            let data = b2.slice(..).get_mapped_range();
            let copy = data.to_vec();
            drop(data);
            b2.unmap();
            if let Ok(mut s) = slot.lock() {
                s.data = copy;
                s.seq += 1;
            }
        }
        busy.store(false, Ordering::Release);
    });
}

fn sync_ocean_wave_sampler(
    wave: Res<WaveField>,
    time: Res<Time>,
    mut sampler: ResMut<crate::ocean::WaveSampler>,
    mut last_seq: Local<u64>,
) {
    let Ok(slot) = wave.0.lock() else { return };
    if slot.seq == *last_seq || slot.data.len() < WAVE_BYTES {
        return;
    }
    *last_seq = slot.seq;
    if sampler.buffer.len() != slot.data.len() {
        sampler.buffer = slot.data.clone();
    } else {
        sampler.buffer.copy_from_slice(&slot.data);
    }
    sampler.stamp = time.elapsed_secs_f64();
    sampler.ready = true;
}

fn f16_to_f32(h: u16) -> f32 {
    let s = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((h >> 10) & 0x1f) as i32;
    let m = (h & 0x3ff) as f32;
    if e == 0 {
        s * m * 2f32.powi(-24)
    } else if e == 31 {
        0.0
    } else {
        s * (1.0 + m / 1024.0) * 2f32.powi(e - 15)
    }
}

const WAVE_DISP_SCALE: [f32; 3] = [1.0, 1.0, 1.0];

fn smoothstep01(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn vertex_weight(i: usize, dist: f32) -> f32 {
    match i {
        0 => 1.0,
        1 => 1.0 - smoothstep01(100.0, 400.0, dist),
        _ => 0.0,
    }
}

struct WaveSampler<'a> {
    data: &'a [u8],
    lam: f32,
    cam: Vec2,
}

impl WaveSampler<'_> {
    fn texel(&self, c: usize, ix: i32, iy: i32) -> Vec3 {
        let n = OCEAN_SIZE as i32;
        let (ix, iy) = (ix.rem_euclid(n) as usize, iy.rem_euclid(n) as usize);
        let off = c * LAYER_BYTES + iy * ROW_BYTES + ix * TEXEL_BYTES;
        let rd = |k: usize| {
            f16_to_f32(u16::from_le_bytes([
                self.data[off + 2 * k],
                self.data[off + 2 * k + 1],
            ]))
        };
        Vec3::new(rd(0), rd(1), rd(2))
    }

    fn cascade(&self, c: usize, x: f32, z: f32) -> Vec3 {
        let n = OCEAN_SIZE as f32;
        let u = x / TILE_LENGTHS[c] * n - 0.5;
        let v = z / TILE_LENGTHS[c] * n - 0.5;
        let (u0, v0) = (u.floor(), v.floor());
        let (fu, fv) = (u - u0, v - v0);
        let (i, j) = (u0 as i32, v0 as i32);
        let top = self.texel(c, i, j).lerp(self.texel(c, i + 1, j), fu);
        let bot = self.texel(c, i, j + 1).lerp(self.texel(c, i + 1, j + 1), fu);
        top.lerp(bot, fv)
    }

    fn displacement(&self, x: f32, z: f32) -> Vec3 {
        let dist = self.cam.distance(Vec2::new(x, z));
        let mut disp = Vec3::ZERO;
        for c in 0..NUM_CASCADES as usize {
            let w = vertex_weight(c, dist) * WAVE_DISP_SCALE[c];
            if w == 0.0 {
                continue;
            }
            let d = self.cascade(c, x, z);
            disp += Vec3::new(d.x * self.lam, d.y, d.z * self.lam) * w;
        }
        disp
    }

    fn height_at(&self, x: f32, z: f32) -> f32 {
        let (mut bx, mut bz) = (x, z);
        for _ in 0..4 {
            let d = self.displacement(bx, bz);
            bx = x - d.x;
            bz = z - d.z;
        }
        self.displacement(bx, bz).y
    }
}

// ---------------------------------------------------------------------------
// Ricerca acqua
// ---------------------------------------------------------------------------

const MIN_WATER_BELOW_DRAFT: f32 = 6.0;

fn deep_limit() -> f32 {
    OCEAN_ELEVATION - SHIP_DRAFT - MIN_WATER_BELOW_DRAFT
}

fn clearance_at(
    samples: &crate::data::HeightSamples,
    gx: usize,
    gy: usize,
    radius_cells: usize,
    limit: f32,
) -> bool {
    let r = radius_cells as i32;
    let (gx_i, gy_i) = (gx as i32, gy as i32);
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy > r * r {
                continue;
            }
            let nx = gx_i + dx;
            let ny = gy_i + dy;
            if !(0..128).contains(&nx) || !(0..128).contains(&ny) {
                return false;
            }
            let h = terrain_height(samples, nx as f32, ny as f32);
            if h >= limit {
                return false;
            }
        }
    }
    true
}

fn find_water_in_pixel(
    data: &GameData,
    origin: &Origin,
    mx: i32,
    my: i32,
    player_xz: Vec2,
) -> Option<(Vec3, (i32, i32), f32)> {
    let limit = deep_limit();
    let Ok(samples) = generate_default_samples(&data.woods, mx, my) else {
        return None;
    };

    // FIX "spawn sulla terraferma": scarta i map pixel con media > 35 m.
    let avg_m: f32 = samples
    .data
    .iter()
    .sum::<f32>()
    / samples.data.len() as f32
    * crate::data::heightmap::MAX_TERRAIN_HEIGHT;
    if avg_m > 35.0 {
        return None;
    }

    let tile_org = tile_translation(origin, mx, my);
    let mut best: Option<(f32, f32, f32)> = None;
    for gy in (SCAN_STEP..128 - SCAN_STEP).step_by(SCAN_STEP) {
        for gx in (SCAN_STEP..128 - SCAN_STEP).step_by(SCAN_STEP) {
            let h = terrain_height(&samples, gx as f32, gy as f32);
            if h >= limit {
                continue;
            }
            if !clearance_at(&samples, gx, gy, SHIP_CLEARANCE_CELLS, limit) {
                continue;
            }
            let x = tile_org.x + gx as f32 * CELL_SIZE;
            let z = tile_org.z - gy as f32 * CELL_SIZE;
            let d = Vec2::new(x, z).distance(player_xz);
            if best.map_or(true, |(bd, _, _)| d < bd) {
                best = Some((d, x, z));
            }
        }
    }
    best.map(|(d, x, z)| {
        (
            Vec3::new(x, OCEAN_PLANE_Y + SHIP_Y_OFFSET, z),
         (mx, my),
         d,
        )
    })
}

fn find_water_near(
    data: &GameData,
    origin: &Origin,
    sx: i32,
    sy: i32,
    player_xz: Vec2,
    max_radius: i32,
) -> Option<(Vec3, (i32, i32), f32)> {
    let mut candidates: Vec<(f32, i32, i32)> = Vec::new();
    for radius in 0..=max_radius {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx.abs().max(dy.abs()) != radius {
                    continue;
                }
                let (mx, my) = (sx + dx, sy + dy);
                if !in_map(mx, my) {
                    continue;
                }
                if data.woods.height_value(mx, my) > WATER_PIXEL_FAST_THRESHOLD {
                    continue;
                }
                let tile_org = tile_translation(origin, mx, my);
                let cx = tile_org.x + 64.0 * CELL_SIZE;
                let cz = tile_org.z - 64.0 * CELL_SIZE;
                let d = Vec2::new(cx, cz).distance(player_xz);
                candidates.push((d, mx, my));
            }
        }
    }
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, mx, my) in candidates {
        if let Some(r) = find_water_in_pixel(data, origin, mx, my, player_xz) {
            return Some(r);
        }
    }
    None
}

fn find_water_fallback(
    data: &GameData,
    origin: &Origin,
    player_xz: Vec2,
) -> Option<(Vec3, (i32, i32), f32)> {
    let corners = [
        (0, MAP_HEIGHT_PIXELS / 2),
        (MAP_WIDTH_PIXELS - 1, MAP_HEIGHT_PIXELS / 2),
        (MAP_WIDTH_PIXELS / 2, 0),
        (MAP_WIDTH_PIXELS / 2, MAP_HEIGHT_PIXELS - 1),
    ];
    for (mx, my) in corners {
        if !in_map(mx, my) {
            continue;
        }
        if let Some(r) = find_water_in_pixel(data, origin, mx, my, player_xz) {
            return Some(r);
        }
    }
    None
}

/// Cerca un punto d'acqua per la companion, ad almeno `COMPANION_MIN_SEP` m
/// dalla nave principale. Espande il raggio di ricerca (5, 10, 15 map pixel)
/// finché non trova qualcosa. `None` se non c'è acqua navigabile abbastanza
/// lontana dalla principale (in quel caso la companion non viene spawnata).
fn find_companion_pos(
    data: &GameData,
    origin: &Origin,
    mx: i32,
    my: i32,
    player_xz: Vec2,
    primary_pos: Vec3,
) -> Option<Vec3> {
    let mut chosen: Option<Vec3> = None;
    for search_r in [5_i32, 10, 15] {
        if chosen.is_some() {
            break;
        }
        let mut candidates: Vec<(f32, i32, i32)> = Vec::new();
        for radius in 0..=search_r {
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx.abs().max(dy.abs()) != radius {
                        continue;
                    }
                    let (cx, cy) = (mx + dx, my + dy);
                    if !in_map(cx, cy) {
                        continue;
                    }
                    if data.woods.height_value(cx, cy) > WATER_PIXEL_FAST_THRESHOLD {
                        continue;
                    }
                    let tile_org = tile_translation(origin, cx, cy);
                    let center = Vec2::new(
                        tile_org.x + 64.0 * CELL_SIZE,
                        tile_org.z - 64.0 * CELL_SIZE,
                    );
                    candidates.push((center.distance(player_xz), cx, cy));
                }
            }
        }
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));

        let mut best_dist = f32::MAX;
        for (_, cx, cy) in candidates {
            let Some((p, _, _)) = find_water_in_pixel(data, origin, cx, cy, player_xz) else {
                continue;
            };
            let xz_dist = Vec2::new(primary_pos.x - p.x, primary_pos.z - p.z).length();
            if xz_dist >= COMPANION_MIN_SEP && xz_dist < best_dist {
                best_dist = xz_dist;
                chosen = Some(p);
            }
        }
    }
    chosen
}

// ---------------------------------------------------------------------------
// Spawn iniziale: principale + companion
// ---------------------------------------------------------------------------

fn spawn_initial_ship(
    mut commands: Commands,
    data: Res<GameData>,
    start: Res<StartInfo>,
    origin: Res<Origin>,
    ctx: Res<ShipPhysicsCtx>,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    config: Res<ShipConfig>,
    elven_assets: Option<Res<ElvenShipAssets>>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    *done = true;

    let Some(elven_assets) = elven_assets.as_deref() else {
        warn!("Nave: ElvenShipAssets non caricati, impossibile spawnare");
        return;
    };

    let start_tile = tile_translation(&origin, start.mx, start.my);
    let player_xz = Vec2::new(
        start_tile.x + 64.0 * CELL_SIZE,
        start_tile.z - 64.0 * CELL_SIZE,
    );

    let found = find_water_near(&data, &origin, start.mx, start.my, player_xz, SEARCH_RADIUS_NORMAL)
    .or_else(|| {
        info!(
            "Nave: nessuna acqua navigabile entro {SEARCH_RADIUS_NORMAL} map pixel, allargo la ricerca a {SEARCH_RADIUS_EXTENDED}..."
        );
        find_water_near(
            &data,
            &origin,
            start.mx,
            start.my,
            player_xz,
            SEARCH_RADIUS_EXTENDED,
        )
    })
    .or_else(|| {
        warn!(
            "Nave: nessuna acqua navigabile entro {SEARCH_RADIUS_EXTENDED} map pixel, la spawno al bordo della mappa (usa N per richiamarla quando ti avvicini al mare)."
        );
        find_water_fallback(&data, &origin, player_xz)
    });

    let Some((pos, (mx, my), d)) = found else {
        warn!(
            "Nave: impossibile trovare acqua navigabile in nessun punto della mappa. Nessuna nave."
        );
        return;
    };

    // 1. Nave principale.
    spawn_ship(
        &mut commands,
        &asset_server,
        ctx.0,
        pos,
        0.0,
        config.kind,
        &mut meshes,
        &mut images,
        &mut materials,
        elven_assets,
    );
    info!(
        "Nave originale ({:?}) nel map pixel ({mx}, {my}) a ({:.0}, {:.0}), {d:.0} m dal giocatore — premi E per prendere il timone",
          config.kind, pos.x, pos.z
    );

    // 2. Companion: spawnata sempre, così vedi sempre due navi.
    let companion_kind = match config.kind {
        ShipKind::Medieval => ShipKind::Elven,
        ShipKind::Elven => ShipKind::Medieval,
    };
    match find_companion_pos(&data, &origin, mx, my, player_xz, pos) {
        Some(comp_pos) => {
            let dist = Vec2::new(comp_pos.x - pos.x, comp_pos.z - pos.z).length();
            spawn_ship(
                &mut commands,
                &asset_server,
                ctx.0,
                comp_pos,
                0.0,
                companion_kind,
                &mut meshes,
                &mut images,
                &mut materials,
                elven_assets,
            );
            info!(
                "Nave companion ({:?}) a ({:.0}, {:.0}) — {:.0} m dalla prima — premi E per salire",
                  companion_kind, comp_pos.x, comp_pos.z, dist
            );
        }
        None => {
            warn!(
                "Nave companion ({:?}): nessuna acqua navigabile a >= {COMPANION_MIN_SEP} m dalla prima entro 15 map pixel. \
La companion non viene spawnata.",
companion_kind
            );
        }
    }
}

fn ship_debug_recall(
    keys: Res<ButtonInput<KeyCode>>,
    mut events: EventWriter<RespawnShipRequest>,
) {
    if keys.just_pressed(KeyCode::KeyN) {
        events.send(RespawnShipRequest { remove_old: true });
        info!("Richiamando le navi vicino al giocatore (tasto N)...");
    }
}

#[allow(clippy::too_many_arguments)]
fn respawn_ship_near_player(
    mut commands: Commands,
    mut requests: EventReader<RespawnShipRequest>,
    data: Res<GameData>,
    origin: Res<Origin>,
    ctx: Res<ShipPhysicsCtx>,
    asset_server: Res<AssetServer>,
    mut ship_state: ResMut<ShipState>,
    mut cam: ResMut<ShipCamera>,
    player: Query<&Transform, With<Player>>,
    ships: Query<Entity, With<Ship>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    config: Res<ShipConfig>,
    elven_assets: Option<Res<ElvenShipAssets>>,
) {
    let Ok(ptf) = player.get_single() else { return };
    for req in requests.read() {
        let (mx, my) = map_pixel_of(&origin, ptf.translation);
        let player_xz = Vec2::new(ptf.translation.x, ptf.translation.z);

        let water = find_water_near(&data, &origin, mx, my, player_xz, SEARCH_RADIUS_NORMAL)
        .or_else(|| {
            find_water_near(&data, &origin, mx, my, player_xz, SEARCH_RADIUS_EXTENDED)
        });

        let Some((pos, (fx, fy), d)) = water else {
            warn!(
                "Respawn nave: nessuna acqua navigabile entro {SEARCH_RADIUS_EXTENDED} map pixel da ({mx}, {my}). La nave resta dov'era."
            );
            continue;
        };

        if req.remove_old {
            for e in &ships {
                commands.entity(e).despawn_recursive();
            }
            if ship_state.player_aboard.is_some() {
                ship_state.player_aboard = None;
                cam.slot = ShipCameraSlot::Helm;
            }
        }
        let Some(elven_assets) = elven_assets.as_deref() else {
            warn!("Respawn nave: ElvenShipAssets non caricati");
            continue;
        };

        // 1. Nave principale.
        spawn_ship(
            &mut commands,
            &asset_server,
            ctx.0,
            pos,
            0.0,
            config.kind,
            &mut meshes,
            &mut images,
            &mut materials,
            elven_assets,
        );
        info!(
            "Nave principale ({:?}) respawnata nel map pixel ({fx}, {fy}) a ({:.0}, {:.0}), {d:.0} m dal giocatore",
              config.kind, pos.x, pos.z
        );

        // 2. Companion: spawnata sempre, così dopo ogni viaggio hai 2 navi.
        let companion_kind = match config.kind {
            ShipKind::Medieval => ShipKind::Elven,
            ShipKind::Elven => ShipKind::Medieval,
        };
        match find_companion_pos(&data, &origin, mx, my, player_xz, pos) {
            Some(comp_pos) => {
                let dist = Vec2::new(comp_pos.x - pos.x, comp_pos.z - pos.z).length();
                spawn_ship(
                    &mut commands,
                    &asset_server,
                    ctx.0,
                    comp_pos,
                    0.0,
                    companion_kind,
                    &mut meshes,
                    &mut images,
                    &mut materials,
                    elven_assets,
                );
                info!(
                    "Nave companion ({:?}) respawnata a ({:.0}, {:.0}) — {:.0} m dalla principale",
                      companion_kind, comp_pos.x, comp_pos.z, dist
                );
            }
            None => {
                warn!(
                    "Nave companion ({:?}): nessuna acqua a >= {COMPANION_MIN_SEP} m, non respawnata",
                      companion_kind
                );
            }
        }
    }
}

fn ship_debug(
    time: Res<Time>,
    ships: Query<(&Transform, &Velocity, &ShipHydro), With<Ship>>,
              mut timer: Local<f32>,
) {
    if std::env::var_os("DF_SHIP_DEBUG").is_none() {
        return;
    }
    *timer += time.delta_secs();
    if *timer < 1.0 {
        return;
    }
    *timer = 0.0;
    for (tf, vel, hydro) in &ships {
        info!(
            "ship: pos=({:.0},{:.0},{:.0}), linvel=({:.2},{:.2},{:.2}), angvel=({:.2},{:.2},{:.2}), submerged={:.2}",
              tf.translation.x, tf.translation.y, tf.translation.z,
              vel.linvel.x, vel.linvel.y, vel.linvel.z,
              vel.angvel.x, vel.angvel.y, vel.angvel.z,
              hydro.submerged
        );
    }
}

// ---------------------------------------------------------------------------
// Hull points (per il galleggiamento) — dipendono dal tipo di nave
// ---------------------------------------------------------------------------

fn beam_medieval(t: f32) -> f32 {
    if t < 0.4 {
        HULL_BEAM * (0.8 + 0.2 * (t / 0.4 * std::f32::consts::FRAC_PI_2).sin())
    } else {
        let u = (t - 0.4) / 0.6;
        HULL_BEAM * (1.0 - 0.94 * u.powf(1.7))
    }
}

fn hull_points_medieval() -> [Vec3; 8] {
    let ts = [0.1, 0.37, 0.63, 0.9];
    let mut pts = [Vec3::ZERO; 8];
    for (i, t) in ts.iter().enumerate() {
        let z = (Z_STERN - t * (Z_STERN - Z_BOW)) * SHIP_SCALE;
        let x = 0.85 * beam_medieval(*t) * SHIP_SCALE;
        pts[2 * i] = Vec3::new(-x, 0.0, z);
        pts[2 * i + 1] = Vec3::new(x, 0.0, z);
    }
    pts
}

/// Punti di galleggiamento del veliero elfico, in coordinate radice (prua
/// a +Z, poppa a -Z). Usa gli stessi 4 t del Medieval, ma con le dimensioni
/// del veliero (ST_DEFS da -41.5 a 42.5 scalate di `ELVEN_SHIP_SCALE`).
fn hull_points_elven() -> [Vec3; 8] {
    use crate::ships::elven_ship::hull::ST_DEFS;
    let s = ELVEN_SHIP_SCALE;
    let ts = [0.1f32, 0.37, 0.63, 0.9];
    let mut pts = [Vec3::ZERO; 8];
    let x_min = ST_DEFS[0].x;
    let x_max = ST_DEFS[ST_DEFS.len() - 1].x;
    for (i, t) in ts.iter().enumerate() {
        let x_local = (x_min + (x_max - x_min) * t) * s;
        let w_local = {
            let x_unscaled = x_local / s;
            let mut chosen = ST_DEFS[0];
            let mut best = f32::MAX;
            for st in &ST_DEFS {
                let d = (st.x - x_unscaled).abs();
                if d < best {
                    best = d;
                    chosen = *st;
                }
            }
            chosen.w_m * s
        };
        pts[2 * i] = Vec3::new(-w_local, 0.0, x_local);
        pts[2 * i + 1] = Vec3::new(w_local, 0.0, x_local);
    }
    pts
}

fn hull_points_for(kind: ShipKind) -> [Vec3; 8] {
    match kind {
        ShipKind::Medieval => hull_points_medieval(),
        ShipKind::Elven => hull_points_elven(),
    }
}

// ---------------------------------------------------------------------------
// Spawn della nave
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn spawn_ship(
    commands: &mut Commands,
    asset_server: &AssetServer,
    ctx: Entity,
    pos: Vec3,
    yaw: f32,
    kind: ShipKind,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    elven_assets: &ElvenShipAssets,
) -> Entity {
    // `pos.y` arriva come `OCEAN_PLANE_Y + SHIP_Y_OFFSET` (calibrato per la
    // Medieval). La mesh elfica è più sottile: offset diverso per allineare
    // la cinta al pelo dell'acqua.
    let pos = Vec3::new(
        pos.x,
        pos.y - SHIP_Y_OFFSET + ship_y_offset(kind),
                        pos.z,
    );
    match kind {
        ShipKind::Medieval => spawn_medieval_ship(commands, asset_server, ctx, pos, yaw),
        ShipKind::Elven => spawn_elven_ship(
            commands, ctx, pos, yaw, meshes, images, materials, elven_assets,
        ),
    }
}

fn spawn_medieval_ship(
    commands: &mut Commands,
    asset_server: &AssetServer,
    ctx: Entity,
    pos: Vec3,
    yaw: f32,
) -> Entity {
    let scene: Handle<Scene> = asset_server.load(MODEL_PATH);
    let m = SHIP_MASS * INERTIA_FILL / 12.0;

    commands
    .spawn((
        Ship { kind: ShipKind::Medieval },
        ShipMotion::default(),
            ShipHydro::default(),
            Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
            Visibility::Visible,
            RigidBody::Dynamic,
            RapierContextEntityLink(ctx),
            AdditionalMassProperties::MassProperties(MassProperties {
                local_center_of_mass: SHIP_COM,
                mass: SHIP_MASS,
                principal_inertia_local_frame: Quat::IDENTITY,
                principal_inertia: Vec3::new(
                    m * (SHIP_H * SHIP_H + SHIP_L * SHIP_L),
                                             m * (SHIP_W * SHIP_W + SHIP_L * SHIP_L),
                                             m * (SHIP_W * SHIP_W + SHIP_H * SHIP_H),
                ),
            }),
            Velocity::default(),
            ExternalForce::default(),
            ExternalImpulse::default(),
            Damping {
                linear_damping: 0.0,
                angular_damping: ANGULAR_DAMPING,
            },
            Sleeping::disabled(),
    ))
    .with_children(|parent| {
        parent.spawn((
            SceneRoot(scene),
                      Transform::from_translation(MODEL_OFFSET * SHIP_SCALE)
                      .with_rotation(Quat::from_rotation_y(MODEL_YAW))
                      .with_scale(Vec3::splat(MODEL_SCALE * SHIP_SCALE)),
        ));
    })
    .id()
}

#[allow(clippy::too_many_arguments)]
fn spawn_elven_ship(
    commands: &mut Commands,
    ctx: Entity,
    pos: Vec3,
    yaw: f32,
    meshes: &mut Assets<Mesh>,
    _images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    elven_assets: &ElvenShipAssets,
) -> Entity {
    let built = elven_ship::build();

    let elven_mass = ship_mass(ShipKind::Elven);
    let m = elven_mass * INERTIA_FILL / 12.0;
    let s = ELVEN_SHIP_SCALE;
    let com = Vec3::new(0.0, SHIP_COM_Y * s, 0.0);
    let w = 12.0 * s;
    let l = 84.0 * s;
    let h = 47.0 * s;

    let root = commands
    .spawn((
        Ship { kind: ShipKind::Elven },
        ShipMotion::default(),
            ShipHydro::default(),
            Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
            Visibility::Visible,
            RigidBody::Dynamic,
            RapierContextEntityLink(ctx),
            AdditionalMassProperties::MassProperties(MassProperties {
                local_center_of_mass: com,
                mass: elven_mass,
                principal_inertia_local_frame: Quat::IDENTITY,
                principal_inertia: Vec3::new(
                    m * (h * h + l * l),
                                             m * (w * w + l * l),
                                             m * (w * w + h * h),
                ),
            }),
            Velocity::default(),
            ExternalForce::default(),
            ExternalImpulse::default(),
            Damping {
                linear_damping: 0.0,
                angular_damping: ANGULAR_DAMPING,
            },
            Sleeping::disabled(),
    ))
    .id();

    // Collider trimesh ricreato a scala 2 (la BuiltShip ha vertici a scala 1).
    if let Some(col) = scaled_collider_from_built(&built, s) {
        commands
        .entity(root)
        .insert((col, ColliderMassProperties::Density(0.0)));
    }

    // Popola il CPU mirror (deck_height_at, ship_floor_at, ship_blocked_at,
    // probe_ship). Se già popolato (l'altra nave spawnata per prima), non
    // sovrascrive: warning e prosegue.
    populate_collider_from_built(&built);

    // Nodo intermedio: applica rotazione -π/2 (prua +X locale → +Z radice)
    // e scala `ELVEN_SHIP_SCALE` una sola volta per tutte le mesh figlie.
    let content = commands
    .spawn((
        Transform::from_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2))
        .with_scale(Vec3::splat(s)),
            Visibility::Inherited,
    ))
    .set_parent(root)
    .id();

    for (mat, sub) in &built.submeshes {
        let bevy_mesh = submesh_to_bevy(&built.mesh, sub, (64.0, 64.0));
        let handle = elven_assets.materials.handle(*mat);
        commands
        .spawn((
            Mesh3d(meshes.add(bevy_mesh)),
                MeshMaterial3d(handle),
                Transform::IDENTITY,
        ))
        .set_parent(content);
    }

    crate::ships::elven_ship::helm_sails::spawn_elven_animated_parts(
        commands,
        meshes,
        &elven_assets.animated,
        content,
        &built.animated,
    );

    root
}

/// Costruisce un `Collider::trimesh` dai vertici di `built.mesh` scalati di
/// `scale`. Serve perché `Collider` non ha una scala propria: se la mesh
/// visiva è scalata tramite un nodo figlio, il collider attaccato alla radice
/// deve avere vertici già scalati.
fn scaled_collider_from_built(built: &crate::ships::BuiltShip, scale: f32) -> Option<Collider> {
    if built.mesh.vertices.is_empty() {
        return None;
    }
    let mut verts: Vec<Vec3> = Vec::with_capacity(built.mesh.vertices.len());
    for v in &built.mesh.vertices {
        verts.push(Vec3::from(*v) * scale);
    }
    let mut indices: Vec<[u32; 3]> = Vec::new();
    for sub in &built.mesh.submeshes {
        for ch in sub.indices.chunks_exact(3) {
            indices.push([ch[0], ch[1], ch[2]]);
        }
    }
    if indices.is_empty() {
        return None;
    }
    Some(Collider::trimesh_with_flags(
        verts,
        indices,
        TriMeshFlags::FIX_INTERNAL_EDGES,
    ))
}

/// Submesh -> Bevy Mesh.
fn submesh_to_bevy(mesh: &crate::data::mesh_builder::WorldMesh, sm: &crate::data::mesh_builder::Submesh, size: (f32, f32)) -> Mesh {
    use bevy::render::mesh::Indices;
    let mut remap: HashMap<u32, u32> = HashMap::new();
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::with_capacity(sm.indices.len());
    for &i in &sm.indices {
        let new = *remap.entry(i).or_insert_with(|| {
            let k = positions.len() as u32;
            positions.push(mesh.vertices[i as usize]);
            normals.push(mesh.normals[i as usize]);
            let uv = mesh.uvs[i as usize];
            uvs.push([uv[0] / size.0, uv[1] / size.1]);
            k
        });
        indices.push(new);
    }
    let mut out = Mesh::new(PrimitiveTopology::TriangleList, bevy::render::render_asset::RenderAssetUsages::RENDER_WORLD);
    out.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    out.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    out.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    out.insert_indices(Indices::U32(indices));
    out
}

/// Popola il CPU mirror del veliero (per `deck_height_at` e simili) con i
/// vertici scalati di `ELVEN_SHIP_SCALE`. Se il `OnceLock` è già stato preso
/// (nave Medieval spawnata per prima), non fa nulla e logga un warning.
fn populate_collider_from_built(built: &crate::ships::BuiltShip) {
    if COLLIDER.get().is_some() {
        debug!(
            "Veliero elfico: CPU mirror già popolato (nave Medieval spawnata per prima), uso Rapier per il floor"
        );
        return;
    }
    let mut tris: Vec<Tri> = Vec::new();
    let mut verts: Vec<Vec3> = Vec::new();
    let mut indices: Vec<[u32; 3]> = Vec::new();
    // Ruota di -π/2 (prua +X locale → +Z radice) e scala di ELVEN_SHIP_SCALE.
    let q = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    for v in &built.mesh.vertices {
        verts.push(q * (Vec3::from(*v) * ELVEN_SHIP_SCALE));
    }
    for sub in &built.mesh.submeshes {
        for ch in sub.indices.chunks_exact(3) {
            indices.push([ch[0], ch[1], ch[2]]);
        }
    }
    for [a, b, c] in &indices {
        let pa = verts[*a as usize];
        let pb = verts[*b as usize];
        let pc = verts[*c as usize];
        let n = (pb - pa).cross(pc - pa);
        let len = n.length();
        if len < 1e-9 {
            continue;
        }
        tris.push(Tri {
            a: pa, b: pb, c: pc,
            n: n / len,
            min: pa.min(pb).min(pc),
                  max: pa.max(pb).max(pc),
        });
    }
    if !tris.is_empty() {
        let mirror = ShipCollider::new(tris);
        let _ = COLLIDER.set(mirror);
        info!("Veliero elfico: CPU mirror popolato ({} triangoli)", indices.len());
    }
}

// ---------------------------------------------------------------------------
// Collider: trimesh Rapier + specchio CPU
// ---------------------------------------------------------------------------

struct Tri {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    n: Vec3,
    min: Vec3,
    max: Vec3,
}

struct ShipCollider {
    tris: Vec<Tri>,
    grid: HashMap<(i32, i32), Vec<u32>>,
    min: Vec3,
    max: Vec3,
}

static COLLIDER: OnceLock<ShipCollider> = OnceLock::new();

fn cell_of(v: f32) -> i32 {
    (v / GRID_CELL).floor() as i32
}

fn in_tri_xz(t: &Tri, x: f32, z: f32) -> bool {
    let side = |p: Vec3, q: Vec3| (q.x - p.x) * (z - p.z) - (q.z - p.z) * (x - p.x);
    let (d1, d2, d3) = (side(t.a, t.b), side(t.b, t.c), side(t.c, t.a));
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

fn slice_tri(t: &Tri, y: f32) -> Option<(Vec2, Vec2)> {
    let v = [t.a, t.b, t.c];
    let mut pts = [Vec2::ZERO; 2];
    let mut n = 0;
    for k in 0..3 {
        let (p, q) = (v[k], v[(k + 1) % 3]);
        let (dp, dq) = (p.y - y, q.y - y);
        if (dp < 0.0) != (dq < 0.0) && n < 2 {
            let r = p + (q - p) * (dp / (dp - dq));
            pts[n] = Vec2::new(r.x, r.z);
            n += 1;
        }
    }
    (n == 2).then(|| (pts[0], pts[1]))
}

fn closest_on_segment(p: Vec2, a: Vec2, b: Vec2) -> Vec2 {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 < 1e-12 {
        return a;
    }
    a + ab * ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
}

impl ShipCollider {
    fn new(tris: Vec<Tri>) -> Self {
        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        let mut min = Vec3::splat(f32::MAX);
        let mut max = Vec3::splat(f32::MIN);
        for (i, t) in tris.iter().enumerate() {
            min = min.min(t.min);
            max = max.max(t.max);
            for cz in cell_of(t.min.z)..=cell_of(t.max.z) {
                for cx in cell_of(t.min.x)..=cell_of(t.max.x) {
                    grid.entry((cx, cz)).or_default().push(i as u32);
                }
            }
        }
        Self { tris, grid, min, max }
    }

    fn floor_height(&self, x: f32, z: f32, max_y: f32) -> Option<f32> {
        let ids = self.grid.get(&(cell_of(x), cell_of(z)))?;
        let mut best: Option<f32> = None;
        for &i in ids {
            let t = &self.tris[i as usize];
            if t.n.y.abs() < WALKABLE_NY || t.min.y > max_y || !in_tri_xz(t, x, z) {
                continue;
            }
            let y = t.a.y - (t.n.x * (x - t.a.x) + t.n.z * (z - t.a.z)) / t.n.y;
            if y <= max_y && best.map_or(true, |b| y > b) {
                best = Some(y);
            }
        }
        best
    }

    fn push_out(&self, mut p: Vec2, radius: f32, ys: &[f32]) -> Vec2 {
        for _ in 0..2 {
            for cz in cell_of(p.y - radius)..=cell_of(p.y + radius) {
                for cx in cell_of(p.x - radius)..=cell_of(p.x + radius) {
                    let Some(ids) = self.grid.get(&(cx, cz)) else {
                        continue;
                    };
                    for &i in ids {
                        let t = &self.tris[i as usize];
                        if t.n.y.abs() >= WALKABLE_NY {
                            continue;
                        }
                        for &y in ys {
                            if y < t.min.y || y > t.max.y {
                                continue;
                            }
                            let Some((s0, s1)) = slice_tri(t, y) else {
                                continue;
                            };
                            let c = closest_on_segment(p, s0, s1);
                            let d = p.distance(c);
                            if d < radius {
                                let dir = if d > 1e-5 {
                                    (p - c) / d
                                } else {
                                    Vec2::new(t.n.x, t.n.z).try_normalize().unwrap_or(Vec2::X)
                                };
                                p = c + dir * radius;
                            }
                        }
                    }
                }
            }
        }
        p
    }
}

fn build_ship_collider(
    mut commands: Commands,
    meshes: Res<Assets<Mesh>>,
    mesh_entities: Query<(Entity, &Mesh3d)>,
                       parents: Query<&Parent>,
                       transforms: Query<&Transform>,
                       ships: Query<(), With<Ship>>,
) {
    if COLLIDER.get().is_some() || ships.is_empty() {
        return;
    }
    let mut tris: Vec<Tri> = Vec::new();
    let mut verts: Vec<Vec3> = Vec::new();
    let mut indices: Vec<[u32; 3]> = Vec::new();
    let mut found = 0usize;

    for (e, mesh3d) in &mesh_entities {
        let mut m = Mat4::IDENTITY;
        let mut cur = e;
        let mut under_ship = false;
        loop {
            if let Ok(t) = transforms.get(cur) {
                m = t.compute_matrix() * m;
            }
            let Ok(par) = parents.get(cur) else {
                break;
            };
            let par = par.get();
            if ships.contains(par) {
                under_ship = true;
                break;
            }
            cur = par;
        }
        if !under_ship {
            continue;
        }
        let Some(mesh) = meshes.get(&mesh3d.0) else {
            return;
        };
        found += 1;
        if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
            continue;
        }
        let Some(pos) = mesh
        .attribute(Mesh::ATTRIBUTE_POSITION)
        .and_then(|a| a.as_float3())
        else {
            continue;
        };
        let world: Vec<Vec3> = pos
        .iter()
        .map(|p| m.transform_point3(Vec3::from(*p)))
        .collect();
        let idx: Vec<usize> = match mesh.indices() {
            Some(i) => i.iter().collect(),
            None => (0..world.len()).collect(),
        };
        let base = verts.len() as u32;
        verts.extend_from_slice(&world);
        for tri in idx.chunks_exact(3) {
            let (a, b, c) = (world[tri[0]], world[tri[1]], world[tri[2]]);
            let n = (b - a).cross(c - a);
            let len = n.length();
            if len < 1e-9 {
                continue;
            }
            indices.push([
                base + tri[0] as u32,
                base + tri[1] as u32,
                base + tri[2] as u32,
            ]);
            tris.push(Tri {
                a, b, c,
                n: n / len,
                min: a.min(b).min(c),
                      max: a.max(b).max(c),
            });
        }
    }
    if found == 0 || tris.is_empty() {
        return;
    }

    let col = Collider::trimesh_with_flags(verts, indices, TriMeshFlags::FIX_INTERNAL_EDGES);
    commands.insert_resource(ShipColliderTemplate(col));

    let mirror = ShipCollider::new(tris);
    info!(
        "Collider nave: {} triangoli, bounds {:?} .. {:?}",
        mirror.tris.len(),
          mirror.min,
          mirror.max
    );
    let _ = COLLIDER.set(mirror);
}

fn attach_ship_colliders(
    mut commands: Commands,
    template: Option<Res<ShipColliderTemplate>>,
    ships: Query<Entity, (With<Ship>, Without<Collider>)>,
) {
    let Some(template) = template else { return };
    for e in &ships {
        commands
        .entity(e)
        .insert((template.0.clone(), ColliderMassProperties::Density(0.0)));
    }
}

// ---------------------------------------------------------------------------
// API pure usate da player.rs
// ---------------------------------------------------------------------------

fn column_floor(lx: f32, lz: f32, max_y: f32) -> Option<f32> {
    if let Some(col) = COLLIDER.get() {
        return col.floor_height(lx, lz, max_y);
    }
    let (zl, xl) = (lz / SHIP_SCALE, lx / SHIP_SCALE);
    if zl > Z_STERN || zl < Z_BOW {
        return None;
    }
    let t = ((Z_STERN - zl) / (Z_STERN - Z_BOW)).clamp(0.0, 1.0);
    if xl.abs() > beam_medieval(t) - 0.15 {
        return None;
    }
    (DECK_LOCAL_Y <= max_y).then_some(DECK_LOCAL_Y)
}

fn floor_world_y(ship_tf: &Transform, xz: Vec2, y_guess: f32, max_local_y: f32) -> Option<f32> {
    let inv = ship_tf.rotation.inverse();
    let mut y = y_guess;
    for _ in 0..3 {
        let local = inv * (Vec3::new(xz.x, y, xz.y) - ship_tf.translation);
        let h = column_floor(local.x, local.z, max_local_y)?;
        let w = ship_tf.translation + ship_tf.rotation * Vec3::new(local.x, h, local.z);
        y = w.y;
    }
    Some(y)
}

pub fn deck_height_at(ship_tf: &Transform, xz: Vec2) -> Option<f32> {
    let guess = ship_tf.translation.y + (ship_tf.rotation * Vec3::new(0.0, DECK_LOCAL_Y, 0.0)).y;
    floor_world_y(ship_tf, xz, guess, DECK_LOCAL_Y + STEP_HEIGHT)
}

pub fn ship_floor_at(ship_tf: &Transform, xz: Vec2, feet_y: f32) -> Option<f32> {
    let inv = ship_tf.rotation.inverse();
    let feet_local = inv * (Vec3::new(xz.x, feet_y, xz.y) - ship_tf.translation);
    floor_world_y(ship_tf, xz, feet_y, feet_local.y + STEP_HEIGHT)
}

pub fn ship_blocked_at(ship_tf: &Transform, x: f32, z: f32, feet_y: f32, radius: f32) -> bool {
    let Some(col) = COLLIDER.get() else {
        return false;
    };
    let lp = ship_tf.rotation.inverse() * (Vec3::new(x, feet_y, z) - ship_tf.translation);
    let m = radius + 0.5;
    if lp.x < col.min.x - m || lp.x > col.max.x + m || lp.z < col.min.z - m || lp.z > col.max.z + m
    {
        return false;
    }
    let ys = [
        lp.y + STEP_HEIGHT + 0.05,
        lp.y + 1.0,
        lp.y + EYE_HEIGHT - 0.1,
    ];
    let p = Vec2::new(lp.x, lp.z);
    col.push_out(p, radius, &ys).distance(p) > 1e-4
}

pub fn probe_ship(
    ship_tf: &Transform,
    world_at: Vec2,
    world_dir: Vec2,
    reach: f32,
    feet_y: f32,
) -> Option<WallHit> {
    let col = COLLIDER.get()?;
    let inv = ship_tf.rotation.inverse();

    let feet_world = Vec3::new(world_at.x, feet_y, world_at.y);
    let feet_local = inv * (feet_world - ship_tf.translation);
    let origin = Vec3::new(feet_local.x, feet_local.y + 1.0, feet_local.z);

    let dir_local = {
        let d = inv * Vec3::new(world_dir.x, 0.0, world_dir.y);
        let xz = Vec2::new(d.x, d.z);
        if xz.length_squared() < 1e-9 {
            return None;
        }
        xz.normalize()
    };
    let ray = Vec3::new(dir_local.x, 0.0, dir_local.y);

    let m = reach + 1.0;
    if origin.x < col.min.x - m
        || origin.x > col.max.x + m
        || origin.z < col.min.z - m
        || origin.z > col.max.z + m
        {
            return None;
        }

        let cell_min_x = cell_of(origin.x - reach);
    let cell_max_x = cell_of(origin.x + reach);
    let cell_min_z = cell_of(origin.z - reach);
    let cell_max_z = cell_of(origin.z + reach);

    let mut best: Option<(f32, Vec3, Vec3, f32, f32)> = None;

    for cz in cell_min_z..=cell_max_z {
        for cx in cell_min_x..=cell_max_x {
            let Some(ids) = col.grid.get(&(cx, cz)) else {
                continue;
            };
            for &i in ids {
                let t = &col.tris[i as usize];
                if t.n.y.abs() >= WALKABLE_NY {
                    continue;
                }
                let n_xz = Vec2::new(t.n.x, t.n.z);
                let n_len = n_xz.length();
                if n_len < 1e-5 {
                    continue;
                }
                let normal_local = n_xz / n_len;
                if normal_local.dot(dir_local) > 0.0 {
                    continue;
                }
                let e1 = t.b - t.a;
                let e2 = t.c - t.a;
                let pv = ray.cross(e2);
                let det = e1.dot(pv);
                if det.abs() < 1e-7 {
                    continue;
                }
                let inv_det = 1.0 / det;
                let tv = origin - t.a;
                let u = tv.dot(pv) * inv_det;
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let qv = tv.cross(e1);
                let v = ray.dot(qv) * inv_det;
                if v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let dist = e2.dot(qv) * inv_det;
                if dist < 0.0 || dist > reach {
                    continue;
                }
                if best.map_or(true, |(d, _, _, _, _)| dist < d) {
                    best = Some((
                        dist,
                        origin + ray * dist,
                        normal_local.extend(0.0),
                                 t.min.y,
                                 t.max.y,
                    ));
                }
            }
        }
    }

    let (dist, hit_local, normal_local, y_min_local, y_max_local) = best?;
    let hit_world = ship_tf.translation + ship_tf.rotation * hit_local;
    let n_world = ship_tf.rotation * normal_local;
    Some(WallHit {
        point: Vec2::new(hit_world.x, hit_world.z),
         normal: Vec2::new(n_world.x, n_world.z).normalize_or_zero(),
         distance: dist,
         y_min: ship_tf.translation.y + y_min_local,
         y_max: ship_tf.translation.y + y_max_local,
    })
}

pub fn probe_ship_rapier(
    ctx: &RapierContext,
    ship_entity: Entity,
    ship_tf: &Transform,
    world_at: Vec2,
    world_dir: Vec2,
    reach: f32,
    feet_y: f32,
) -> Option<WallHit> {
    let origin = Vec3::new(world_at.x, feet_y + 1.0, world_at.y);
    let dir = Vec3::new(world_dir.x, 0.0, world_dir.y);
    let dir = if dir.length_squared() < 1e-9 {
        return None;
    } else {
        dir.normalize()
    };

    let filter = QueryFilter::default();
    let (entity, hit) = ctx.cast_ray_and_get_normal(origin, dir, reach, true, filter)?;
    if entity != ship_entity {
        return None;
    }
    if hit.normal.y.abs() >= WALKABLE_NY {
        return None;
    }
    let (y_min, y_max) = y_extent_at(ship_tf, hit.point, feet_y);
    Some(WallHit {
        point: Vec2::new(hit.point.x, hit.point.z),
         normal: Vec2::new(hit.normal.x, hit.normal.z).normalize_or_zero(),
         distance: hit.time_of_impact,
         y_min,
         y_max,
    })
}

fn y_extent_at(ship_tf: &Transform, hit_world: Vec3, feet_y: f32) -> (f32, f32) {
    let Some(col) = COLLIDER.get() else {
        return (feet_y, feet_y + 2.0);
    };
    let local = ship_tf.rotation.inverse() * (hit_world - ship_tf.translation);
    let tol = 0.1;
    let mut best: Option<&Tri> = None;
    let mut best_dist = f32::MAX;
    for t in &col.tris {
        if t.n.y.abs() >= WALKABLE_NY {
            continue;
        }
        let plane_dist = t.n.dot(local - t.a).abs();
        if plane_dist > tol {
            continue;
        }
        if !in_tri_xz(t, local.x, local.z) {
            continue;
        }
        if plane_dist < best_dist {
            best_dist = plane_dist;
            best = Some(t);
        }
    }
    let (y_min_local, y_max_local) = match best {
        Some(t) => (t.min.y, t.max.y),
        None => (local.y, local.y + 2.0),
    };
    (
        ship_tf.translation.y + y_min_local,
     ship_tf.translation.y + y_max_local,
    )
}

fn helm_position(ship_tf: &Transform, kind: ShipKind) -> Vec3 {
    match kind {
        ShipKind::Medieval => {
            let lz = HELM_STAND_Z * SHIP_SCALE;
            let ly = column_floor(0.0, lz, (DECK_Y + 1.0) * SHIP_SCALE).unwrap_or(DECK_LOCAL_Y);
            ship_tf.translation + ship_tf.rotation * Vec3::new(0.0, ly, lz)
        }
        ShipKind::Elven => {
            let s = ELVEN_SHIP_SCALE;
            ship_tf.translation + ship_tf.rotation * Vec3::new(0.0, 15.5 * s, -26.0 * s)
        }
    }
}

fn camera_feet_world(ship_tf: &Transform, slot: ShipCameraSlot, kind: ShipKind) -> Vec3 {
    let off = slot.local_offset(kind);
    let y = if slot.snaps_to_floor() {
        column_floor(off.x, off.z, off.y + 1.0).unwrap_or(off.y)
    } else {
        off.y
    };
    ship_tf.translation + ship_tf.rotation * Vec3::new(off.x, y, off.z)
}

// ---------------------------------------------------------------------------
// Salita / discesa (E)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn ship_boarding(
    keys: Res<ButtonInput<KeyCode>>,
    dialog: Res<DialogState>,
    interior: Res<InteriorState>,
    dungeon: Res<DungeonState>,
    mut ship_state: ResMut<ShipState>,
    mut walk: ResMut<ShipWalk>,
    mut cam: ResMut<ShipCamera>,
    ships: Query<(Entity, &Transform, &Ship), Without<Player>>,
                 mut player: Query<(&mut Player, &mut Transform), Without<Ship>>,
) {
    if dialog.open || interior.is_inside() || dungeon.is_inside() {
        return;
    }
    if !keys.just_pressed(KeyCode::KeyE) {
        return;
    }
    let Ok((mut p, mut tf)) = player.get_single_mut() else {
        return;
    };

    if let Some(ship_entity) = ship_state.player_aboard {
        if let Ok((_, ship_tf, ship)) = ships.get(ship_entity) {
            let feet = helm_position(ship_tf, ship.kind);
            tf.translation = feet + Vec3::Y * EYE_HEIGHT;
            p.feet_y = feet.y;
            walk.ship = Some(ship_entity);
            walk.local = ship_tf.rotation.inverse() * (tf.translation - ship_tf.translation);
            walk.last_yaw = ship_tf.rotation.to_euler(EulerRot::YXZ).0;
            walk.airborne = 0.0;
        }
        p.vel_y = 0.0;
        p.on_ground = true;
        ship_state.player_aboard = None;
        cam.slot = ShipCameraSlot::Helm;
        info!("Fuori dalla modalità nave: sei sul ponte");
        return;
    }

    let player_pos = tf.translation;
    let mut best: Option<(f32, Entity)> = None;
    for (ship_entity, ship_tf, ship) in &ships {
        let dist = Vec2::new(
            ship_tf.translation.x - player_pos.x,
            ship_tf.translation.z - player_pos.z,
        )
        .length();
        let limit = boarding_distance(ship.kind);
        if dist < limit && best.map_or(true, |(d, _)| dist < d) {
            best = Some((dist, ship_entity));
        }
    }
    if let Some((_, ship_entity)) = best {
        p.vel_y = 0.0;
        p.on_ground = true;
        p.climbing = false;
        walk.ship = None;
        ship_state.player_aboard = Some(ship_entity);
        cam.slot = ShipCameraSlot::Helm;
        info!("Al timone (premi Z per cambiare posizione camera, E per scendere)");
    }
}

// ---------------------------------------------------------------------------
// Pilotaggio
// ---------------------------------------------------------------------------

fn ship_pilot(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    ship_state: Res<ShipState>,
    cam: Res<ShipCamera>,
    mut ships: Query<
    (&Transform, &Velocity, &ShipHydro, &Ship, &mut ExternalImpulse),
              (With<Ship>, Without<Player>),
              >,
              mut player: Query<(&mut Player, &mut Transform), Without<Ship>>,
              mut last_slot: Local<Option<ShipCameraSlot>>,
) {
    let Some(ship_entity) = ship_state.player_aboard else {
        return;
    };
    let Ok((ship_tf, vel, hydro, ship, mut impulse)) = ships.get_mut(ship_entity) else {
        return;
    };
    let Ok((mut p, mut tf)) = player.get_single_mut() else {
        return;
    };

    let dt = time.delta_secs().min(1.0 / 30.0);
    let forward = ship_tf.rotation * Vec3::Z;
    let up = ship_tf.rotation * Vec3::Y;
    let v_fwd = vel.linvel.dot(forward);
    let wet = hydro.submerged.clamp(0.0, 1.0);
    let mass = ship_mass(ship.kind);

    let throttle = (keys.pressed(KeyCode::KeyW) as i32 as f32)
    - (keys.pressed(KeyCode::KeyS) as i32 as f32);
    let thrust = if throttle > 0.0 {
        mass * ACCELERATION * (1.0 - v_fwd / MAX_SPEED_FWD).clamp(0.0, 1.0)
    } else if throttle < 0.0 {
        -mass * ACCELERATION * (1.0 + v_fwd / MAX_SPEED_BWD).clamp(0.0, 1.0)
    } else {
        0.0
    };
    impulse.impulse += forward * (thrust * wet * dt);

    let steer = (keys.pressed(KeyCode::KeyA) as i32 as f32)
    - (keys.pressed(KeyCode::KeyD) as i32 as f32);
    if steer != 0.0 {
        let i_yaw = mass * INERTIA_FILL / 12.0 * (SHIP_W * SHIP_W + SHIP_L * SHIP_L);
        let flow = (v_fwd.abs() / RUDDER_REF_SPEED).clamp(0.25, 1.0);
        impulse.torque_impulse += up * (steer * i_yaw * RUDDER_ACCEL * flow * wet * dt);
    }

    let feet = camera_feet_world(ship_tf, cam.slot, ship.kind);
    p.feet_y = feet.y;
    p.vel_y = 0.0;
    p.on_ground = true;
    tf.translation = feet + Vec3::Y * EYE_HEIGHT;

    if *last_slot != Some(cam.slot) {
        let pitch_off = cam.slot.pitch_offset();
        if pitch_off != 0.0 {
            p.pitch = (p.pitch + pitch_off).clamp(-1.5, 1.5);
        }
        *last_slot = Some(cam.slot);
    }
}

// ---------------------------------------------------------------------------
// Galleggiamento
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn ship_hydro(
    wave: Res<WaveField>,
    sea: Option<Res<OceanSea>>,
    field: Res<HeightField>,
    origin: Res<Origin>,
    camera: Query<&Transform, (With<Player>, Without<Ship>)>,
              mut ships: Query<
              (
                  &Transform,
               &Velocity,
               &mut ExternalForce,
               &mut ShipHydro,
               &mut ShipMotion,
               &Ship,
              ),
              With<Ship>,
              >,
) {
    const N: f32 = 8.0;
    let lam = {
        let storm = sea.map_or(0.3, |s| s.storm.clamp(0.0, 1.0));
        CHOPPINESS_CALM + (CHOPPINESS_STORM - CHOPPINESS_CALM) * storm
    };

    let guard = wave.0.lock().expect("WaveField avvelenato");
    let cam = camera
    .get_single()
    .map_or(Vec2::splat(f32::MAX), |t| Vec2::new(t.translation.x, t.translation.z));
    let sampler = (guard.data.len() >= WAVE_BYTES).then(|| WaveSampler {
        data: &guard.data,
        lam,
        cam,
    });

    for (tf, vel, mut ext, mut hydro, mut motion, ship) in &mut ships {
        let mass = ship_mass(ship.kind);
        let draft = ship_draft(ship.kind);
        let k_pt = mass * G / (N * draft);
        let c_pt = HEAVE_DAMPING_RATIO * 2.0 * (N * k_pt * mass).sqrt() / N;
        let share = mass / N;

        let rot = tf.rotation;
        let com = tf.translation + rot * SHIP_COM;
        let right = rot * Vec3::X;
        let fwd = rot * Vec3::Z;
        let points = hull_points_for(ship.kind);

        let mut force = Vec3::ZERO;
        let mut torque = Vec3::ZERO;
        let mut sub_sum = 0.0;

        for pl in points {
            let pw = tf.translation + rot * pl;
            let r = pw - com;
            let vp = vel.linvel + vel.angvel.cross(r);
            let mut f = Vec3::ZERO;

            let water_y = OCEAN_PLANE_Y + sampler.as_ref().map_or(0.0, |s| s.height_at(pw.x, pw.z));
            let depth = water_y - pw.y;
            if depth > 0.0 {
                let sub = (depth / SUBMERGE_REF).min(1.0);
                sub_sum += sub;
                let fy = (k_pt * depth.min(MAX_DEPTH) - c_pt * vp.y).max(0.0);
                let vh = Vec3::new(vp.x, 0.0, vp.z);
                let drag = -(right * (vh.dot(right) * DRAG_LATERAL)
                + fwd * (vh.dot(fwd) * DRAG_FORWARD))
                * (share * sub);
                f += Vec3::new(0.0, fy, 0.0) + drag;
            }

            if let Some(g) = field.ground(&origin, pw.x, pw.z) {
                let pen = g - pw.y;
                if pen > 0.0 {
                    let k_g = share * G * GROUND_STIFFNESS;
                    let c_g = 0.7 * 2.0 * (k_g * share).sqrt();
                    let fy = (k_g * pen.min(4.0) - c_g * vp.y).max(0.0);
                    let vh = Vec3::new(vp.x, 0.0, vp.z);
                    f += Vec3::new(0.0, fy, 0.0) - vh * (share * GROUND_FRICTION * pen.min(1.0));
                }
            }

            force += f;
            torque += r.cross(f);
        }

        ext.force = force;
        ext.torque = torque;
        hydro.submerged = sub_sum / N;
        motion.speed = vel.linvel.dot(fwd);
    }
}

// ---------------------------------------------------------------------------
// Player sulla nave
// ---------------------------------------------------------------------------

fn ship_walk_pre(
    ship_state: Res<ShipState>,
    origin: Res<Origin>,
    mut walk: ResMut<ShipWalk>,
    ships: Query<(Entity, &Transform), (With<Ship>, Without<Player>)>,
                 mut player: Query<(&mut Player, &mut Transform), Without<Ship>>,
) {
    let Ok((mut p, mut tf)) = player.get_single_mut() else {
        return;
    };

    if walk.ship.is_none()
        && p.climbing
        && p.climb_target == ClimbTarget::Ship
        {
            let player_pos = tf.translation;
            let nearest = ships
            .iter()
            .filter_map(|(e, ship_tf)| {
                let d = ship_tf.translation.distance(player_pos);
                (d < WALK_RANGE).then_some((d, e, ship_tf))
            })
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            if let Some((_, e, ship_tf)) = nearest {
                walk.ship = Some(e);
                walk.local =
                ship_tf.rotation.inverse() * (player_pos - ship_tf.translation);
                walk.last_yaw = ship_tf.rotation.to_euler(EulerRot::YXZ).0;
                info!(
                    "Scalata nave: legato a {:?} (locale {:?})",
                      e, walk.local
                );
            }
        }

        if ship_state.player_aboard.is_none() {
            if let Some(e) = walk.ship {
                if let Ok((_, ship_tf)) = ships.get(e) {
                    let pos = ship_tf.translation + ship_tf.rotation * walk.local;
                    tf.translation = pos;
                    p.feet_y = pos.y - EYE_HEIGHT;
                    if CARRY_YAW {
                        let yaw = ship_tf.rotation.to_euler(EulerRot::YXZ).0;
                        let mut d = yaw - walk.last_yaw;
                        while d > std::f32::consts::PI {
                            d -= std::f32::consts::TAU;
                        }
                        while d < -std::f32::consts::PI {
                            d += std::f32::consts::TAU;
                        }
                        tf.rotation = Quat::from_rotation_y(d) * tf.rotation;
                        walk.last_yaw = yaw;
                    }
                } else {
                    walk.ship = None;
                }
            }
        }
        walk.pre_pos = tf.translation;
        walk.pre_feet = p.feet_y;
        walk.origin = (origin.east, origin.north);
}

#[allow(clippy::too_many_arguments)]
fn ship_walk_post(
    time: Res<Time>,
    ship_state: Res<ShipState>,
    interior: Res<InteriorState>,
    dungeon: Res<DungeonState>,
    origin: Res<Origin>,
    mut walk: ResMut<ShipWalk>,
    ships: Query<(Entity, &Transform), (With<Ship>, Without<Player>)>,
                  mut physics: WriteRapierContext,
                  mut player: Query<(&mut Player, &mut Transform), Without<Ship>>,
) {
    if ship_state.player_aboard.is_some() || interior.is_inside() || dungeon.is_inside() {
        walk.ship = None;
        return;
    }
    let Ok((mut p, mut tf)) = player.get_single_mut() else {
        return;
    };

    if p.climbing && p.climb_target == ClimbTarget::Ship {
        if walk.ship.is_none() {
            let player_pos = tf.translation;
            let nearest = ships
            .iter()
            .filter_map(|(e, ship_tf)| {
                let d = ship_tf.translation.distance(player_pos);
                (d < WALK_RANGE).then_some((d, e, ship_tf))
            })
            .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            if let Some((_, e, ship_tf)) = nearest {
                walk.ship = Some(e);
                walk.local =
                ship_tf.rotation.inverse() * (player_pos - ship_tf.translation);
            }
        }
        if let Some(e) = walk.ship {
            if let Ok((_, ship_tf)) = ships.get(e) {
                walk.local =
                ship_tf.rotation.inverse() * (tf.translation - ship_tf.translation);
                walk.last_yaw = ship_tf.rotation.to_euler(EulerRot::YXZ).0;
            }
        }
        return;
    }

    if walk.origin != (origin.east, origin.north) {
        return;
    }
    let desired = Vec3::new(
        tf.translation.x - walk.pre_pos.x,
        p.feet_y - walk.pre_feet,
        tf.translation.z - walk.pre_pos.z,
    );
    if desired.length() > 20.0 {
        return;
    }

    let mut nearest: Option<(Entity, f32)> = None;
    for (e, st) in &ships {
        let d = st.translation.distance(tf.translation);
        if d < WALK_RANGE && nearest.map_or(true, |(_, bd)| d < bd) {
            nearest = Some((e, d));
        }
    }
    let Some((ship_e, _)) = nearest else {
        walk.ship = None;
        return;
    };
    if physics.rapier_context.is_empty() {
        return;
    }
    let Ok((_, ship_tf)) = ships.get(ship_e) else {
        return;
    };

    let height = EYE_HEIGHT + 0.1;
    let shape = Collider::capsule_y((height - 2.0 * PLAYER_RADIUS) * 0.5, PLAYER_RADIUS);
    let from = Vec3::new(
        walk.pre_pos.x,
        walk.pre_feet + height * 0.5,
        walk.pre_pos.z,
    );
    let options = MoveShapeOptions {
        up: Vec3::Y,
        offset: CharacterLength::Absolute(0.02),
        slide: true,
        autostep: Some(CharacterAutostep {
            max_height: CharacterLength::Absolute(STEP_HEIGHT),
                       min_width: CharacterLength::Absolute(0.2),
                       include_dynamic_bodies: true,
        }),
        max_slope_climb_angle: 50f32.to_radians(),
        min_slope_slide_angle: 30f32.to_radians(),
        apply_impulse_to_dynamic_bodies: false,
        snap_to_ground: Some(CharacterLength::Absolute(0.3)),
        ..default()
    };
    let out = physics.rapier_context.single_mut().move_shape(
        desired,
        &shape,
        from,
        Quat::IDENTITY,
        PLAYER_MASS,
        &options,
        QueryFilter::default(),
                                                             |_| {},
    );

    let feet = Vec3::new(from.x, from.y - height * 0.5, from.z) + out.effective_translation;
    tf.translation = feet + Vec3::Y * EYE_HEIGHT;
    p.feet_y = feet.y;

    let dt = time.delta_secs();
    if out.grounded && p.vel_y <= 0.0 {
        p.vel_y = 0.0;
        p.on_ground = true;
        p.climbing = false;
        walk.airborne = 0.0;
        walk.ship = Some(ship_e);
    } else if walk.ship.is_some() {
        walk.airborne += dt;
        if walk.airborne > LEAVE_AFTER {
            walk.ship = None;
        }
    }
    if walk.ship.is_some() {
        walk.local = ship_tf.rotation.inverse() * (tf.translation - ship_tf.translation);
        walk.last_yaw = ship_tf.rotation.to_euler(EulerRot::YXZ).0;
    }
}
