//! Nemici dei dungeon come in Daggerfall (riferimenti: Daggerfall Unity `RDBLayout`,
//! `RandomEncounters`, `EnemyBasics`, `EnemyMotor`).
//!
//! * Negli RDB i nemici sono flat "editor" (archivio 199): record 15 = nemico casuale, 16 = fisso.
//! * **Streaming**: i nemici non vengono spawnati tutti all'ingresso. Restano in una lista
//!   `PendingDungeonEnemies` e compaiono solo quando il giocatore si avvicina (entro
//!   `STREAM_DISTANCE`).
//!
//! ## Parità con Daggerfall Unity (`RDBLayout`)
//!
//! `plan_enemies` distingue i nemici per **`kind`**, non per `mobile_id`:
//!
//! * `EnemySpawnKind::Fixed(Some(m))` (record 16): se `m` è una specie nota
//!   del catalogo, crea il nemico fisso. Se `m` non esiste (o è 99, già
//!   filtrato a monte in `dungeon_builder`), il marker viene **scartato**.
//!   DFU fa la stessa cosa: il cast a `MobileTypes` produce un valore fuori
//!   range che `SetupDemoEnemy.ApplyEnemySettings` ignora, e il nemico non
//!   appare. Cadere in un fallback random qui sarebbe il bug della sala del
//!   trono (marker NPC/quest trasformati in mostri casuali).
//! * `EnemySpawnKind::Random` (record 15): sceglie dalla tabella del dungeon,
//!   come `AddRandomRDBEnemyClassic`.
//!
//! ## Fix coordinate locale vs mondiale
//!
//! `BuiltDungeon::enemies[i].pos` sono in metri **relativi al root del dungeon** (che sta a
//! `DUNGEON_Y = 12000` in `dungeon.rs`). La `Transform` del giocatore è invece in coordinate
//! **mondiali** (con `y ≈ 12000 + offset`). Prima del fix il controllo di distanza confrontava
//! i due spazi direttamente, quindi `dist_sq` risultava sempre enorme e nessun nemico veniva
//! mai materializzato. Ora convertiamo `player_world` in coordinate locali del parent
//! (`player_local = player_world - root.translation()`) prima di calcolare la distanza.

use crate::data::dungeon_builder::{
    BuiltDungeon, DungeonCollision, DungeonEnemySpawn, EnemySpawnKind,
};
use crate::mod_textures::ModTextures;
use crate::player::Player;
use crate::tiles::{
    build_enemy_sprites, next_rand, orientation_index, EnemySpriteSet, ENEMY_ATTACK_FPS,
    ENEMY_FLY_FPS, ENEMY_IDLE_FPS, ENEMY_MOVE_FPS, ORIENTATIONS,
};
use crate::world::GameData;
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

pub const DEFAULT_MONSTER_POWER: f32 = 0.5;
const MONSTER_VARIANCE: usize = 4;

const NOTICE_DISTANCE: f32 = 8.0;
const LOSE_DISTANCE: f32 = 20.0;
const MELEE_RANGE: f32 = 2.0;
const ATTACK_COOLDOWN: f32 = 0.6;
const SENSE_INTERVAL: f32 = 0.4;
const BODY_RADIUS: f32 = 0.5;
const STEP_UP: f32 = 0.6;
const HOVER: f32 = 1.0;
const FACING_DOT_THRESHOLD: f32 = 0.35;

/// Distanza entro la quale un nemico viene "materializzato".
const STREAM_DISTANCE: f32 = 40.0;

// ---------------------------------------------------------------------------
// Tipi di dungeon
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DungeonKind {
    Crypt,
    OrcStronghold,
    HumanStronghold,
    PrisonOrCell,
    DesecratedTemple,
    Mine,
    NaturalCave,
    Coven,
    VampireHaunt,
    Laboratory,
    HarpyNest,
    RuinedCastle,
    SpiderNest,
    GiantStronghold,
    DragonsDen,
    BarbarianStronghold,
    VolcanicCaves,
    ScorpionNest,
    Cemetery,
}

impl DungeonKind {
    const ALL: [DungeonKind; 19] = [
        DungeonKind::Crypt,
        DungeonKind::OrcStronghold,
        DungeonKind::HumanStronghold,
        DungeonKind::PrisonOrCell,
        DungeonKind::DesecratedTemple,
        DungeonKind::Mine,
        DungeonKind::NaturalCave,
        DungeonKind::Coven,
        DungeonKind::VampireHaunt,
        DungeonKind::Laboratory,
        DungeonKind::HarpyNest,
        DungeonKind::RuinedCastle,
        DungeonKind::SpiderNest,
        DungeonKind::GiantStronghold,
        DungeonKind::DragonsDen,
        DungeonKind::BarbarianStronghold,
        DungeonKind::VolcanicCaves,
        DungeonKind::ScorpionNest,
        DungeonKind::Cemetery,
    ];

    pub fn from_index(index: u8) -> Option<Self> {
        Self::ALL.get(index as usize).copied()
    }
}

fn encounter_table(kind: DungeonKind) -> &'static [u8] {
    match kind {
        DungeonKind::Crypt => &[0, 15, 15, 17, 18, 19],
        DungeonKind::OrcStronghold => &[7, 7, 12, 12, 21],
        DungeonKind::HumanStronghold => &[7, 7, 12, 12, 21],
        DungeonKind::PrisonOrCell => &[0, 0, 15, 17, 18],
        DungeonKind::DesecratedTemple => &[1, 22, 18, 15, 17],
        DungeonKind::Mine => &[0, 3, 6, 7, 22],
        DungeonKind::NaturalCave => &[0, 3, 6, 4, 7, 20],
        DungeonKind::Coven => &[1, 10, 18, 21],
        DungeonKind::VampireHaunt => &[15, 17, 18, 19],
        DungeonKind::Laboratory => &[1, 17, 15, 22],
        DungeonKind::HarpyNest => &[3, 13, 13, 13],
        DungeonKind::RuinedCastle => &[15, 17, 22, 16],
        DungeonKind::SpiderNest => &[3, 6, 6, 6],
        DungeonKind::GiantStronghold => &[7, 12, 16, 16],
        DungeonKind::DragonsDen => &[20, 22, 16],
        DungeonKind::BarbarianStronghold => &[7, 9, 14, 12],
        DungeonKind::VolcanicCaves => &[1, 22, 20],
        DungeonKind::ScorpionNest => &[20, 20, 20],
        DungeonKind::Cemetery => &[15, 17, 18, 19],
    }
}

pub struct Species {
    pub id: u8,
    pub archive: u16,
    pub has_idle: bool,
    pub flying: bool,
    pub level: u8,
    pub health: (i32, i32),
    pub damage: (i32, i32),
    pub speed: f32,
    pub attack: &'static [i32],
}

pub static SPECIES: [Species; 22] = [
    Species { id: 0, archive: 255, has_idle: true, flying: false, level: 1, health: (9, 16), damage: (1, 4), speed: 3.5, attack: &[0, 1, 2, -1, 3, 4, 5] },
    Species { id: 1, archive: 256, has_idle: false, flying: true, level: 2, health: (11, 18), damage: (2, 15), speed: 3.5, attack: &[0, 1, 2, -1, 3, 1] },
    Species { id: 2, archive: 257, has_idle: true, flying: false, level: 3, health: (12, 26), damage: (1, 10), speed: 3.0, attack: &[0, 1, -1, 2, 3, 3, 3] },
    Species { id: 3, archive: 258, has_idle: false, flying: true, level: 3, health: (12, 26), damage: (2, 12), speed: 5.0, attack: &[0, 1, -1, 2, 3] },
    Species { id: 4, archive: 259, has_idle: true, flying: false, level: 4, health: (13, 34), damage: (1, 10), speed: 4.5, attack: &[0, 1, 2, -1, 3, 0] },
    Species { id: 5, archive: 260, has_idle: true, flying: false, level: 4, health: (13, 34), damage: (1, 15), speed: 5.5, attack: &[0, 1, 2, -1, 3, 4, 5] },
    Species { id: 6, archive: 261, has_idle: true, flying: false, level: 4, health: (13, 34), damage: (5, 15), speed: 3.5, attack: &[0, 1, -1, 2, 3, 4, 5] },
    Species { id: 7, archive: 262, has_idle: true, flying: false, level: 5, health: (13, 34), damage: (1, 6), speed: 3.0, attack: &[0, 1, 2, -1, 3, 4, -1, 5, 0] },
    Species { id: 8, archive: 263, has_idle: true, flying: false, level: 5, health: (14, 46), damage: (5, 15), speed: 5.0, attack: &[0, 1, 1, 1, 2, -1, 3, 3, 4] },
    Species { id: 9, archive: 264, has_idle: false, flying: false, level: 6, health: (17, 66), damage: (1, 12), speed: 4.5, attack: &[0, 1, -1, 2, -1, 2] },
    Species { id: 10, archive: 265, has_idle: true, flying: false, level: 6, health: (15, 50), damage: (1, 5), speed: 2.5, attack: &[0, 1, 2, 3, 4, -1, 5] },
    Species { id: 12, archive: 267, has_idle: true, flying: false, level: 7, health: (15, 50), damage: (5, 15), speed: 3.2, attack: &[0, -1, 1, 2, 3, 4, -1, 5, 0] },
    Species { id: 13, archive: 268, has_idle: false, flying: true, level: 8, health: (16, 85), damage: (5, 15), speed: 4.5, attack: &[0, 1, 2, -1, 3] },
    Species { id: 14, archive: 269, has_idle: false, flying: false, level: 8, health: (17, 66), damage: (2, 15), speed: 4.0, attack: &[0, -1, 1, 2, 2] },
    Species { id: 15, archive: 270, has_idle: true, flying: false, level: 9, health: (17, 66), damage: (5, 15), speed: 2.5, attack: &[0, 1, 2, 3, -1, 4, 5] },
    Species { id: 16, archive: 271, has_idle: true, flying: false, level: 10, health: (18, 74), damage: (10, 30), speed: 2.5, attack: &[0, 1, -1, 2, 3, 4, -1, 5] },
    Species { id: 17, archive: 272, has_idle: true, flying: false, level: 10, health: (52, 66), damage: (15, 50), speed: 1.5, attack: &[0, 1, 2, -1, 3, 4] },
    Species { id: 18, archive: 273, has_idle: false, flying: true, level: 11, health: (17, 66), damage: (10, 35), speed: 3.0, attack: &[0, 1, -1, 2, 3] },
    Species { id: 19, archive: 274, has_idle: true, flying: false, level: 11, health: (17, 66), damage: (5, 15), speed: 2.0, attack: &[0, 1, 2, -1, 3, 4] },
    Species { id: 20, archive: 275, has_idle: false, flying: false, level: 12, health: (18, 74), damage: (15, 25), speed: 3.0, attack: &[0, 1, -1, 3, 2, 1, 0] },
    Species { id: 21, archive: 276, has_idle: true, flying: false, level: 13, health: (18, 74), damage: (2, 20), speed: 3.0, attack: &[0, 1, -1, 3, 2, 1, 0] },
    Species { id: 22, archive: 277, has_idle: true, flying: false, level: 14, health: (19, 82), damage: (10, 15), speed: 3.0, attack: &[0, 2, 1, 2, 3, -1, 4, 0] },
];

fn species_index(id: u8) -> Option<usize> {
    SPECIES.iter().position(|s| s.id == id)
}

pub fn mobile_texture_public(mobile_id: u8) -> Option<(u16, u8)> {
    SPECIES.iter().find(|s| s.id == mobile_id).map(|s| (s.archive, 0))
}

fn choose_enemy(table: &[u8], power: f32, variance: usize, rng: &mut u64) -> u8 {
    let last = table.len() - 1;
    let base = (power.clamp(0.0, 1.0) * last as f32).round() as usize;
    let lo = base.saturating_sub(variance);
    let hi = (base + variance).min(last);
    let pick = lo + ((next_rand(rng) * (hi - lo + 1) as f32) as usize).min(hi - lo);
    table[pick]
}

#[derive(Debug, Clone, Copy)]
pub struct PlannedEnemy {
    pub pos: [f32; 3],
    pub species: usize,
}

/// Trasforma gli spawn grezzi di `DungeonEnemySpawn` in `PlannedEnemy` con
/// specie risolte. Parità DFU:
///
/// * `Fixed(Some(m))`: se `m` è una specie nota, lo spawna. Se `m` non è
///   noto (o è `None`), il marker viene **scartato**. In DFU il cast a
///   `MobileTypes` produce un valore fuori range che `ApplyEnemySettings`
///   ignora: stesso effetto.
/// * `Random`: sceglie dalla tabella del dungeon, come
///   `AddRandomRDBEnemyClassic`.
///
/// **Nessun fallback random** per MobileID sconosciuti: era il bug che
/// riempiva di mostri la sala del trono del Palazzo Reale.
pub fn plan_enemies(
    spawns: &[DungeonEnemySpawn],
    kind: DungeonKind,
    location_id: u32,
    power: f32,
) -> Vec<PlannedEnemy> {
    let table = encounter_table(kind);
    spawns
    .iter()
    .filter_map(|s| {
        let mut rng = ((location_id as u64) << 32)
        ^ (s.pos[0].to_bits() as u64)
        ^ ((s.pos[2].to_bits() as u64).rotate_left(16))
        ^ 0x9E37_79B9_7F4A_7C15;

        let id = match s.kind {
            EnemySpawnKind::Fixed(Some(m)) => {
                if species_index(m).is_some() {
                    m
                } else {
                    return None;
                }
            }
            EnemySpawnKind::Fixed(None) => return None,
                EnemySpawnKind::Random => {
                    choose_enemy(table, power, MONSTER_VARIANCE, &mut rng)
                }
        };
        species_index(id).map(|species| PlannedEnemy {
            pos: s.pos,
            species,
        })
    })
    .collect()
}

#[derive(Event)]
pub struct SpawnDungeonEnemies {
    pub parent: Entity,
    pub enemies: Vec<PlannedEnemy>,
    pub collision: Arc<DungeonCollision>,
    pub spawn_pos: [f32; 3],
}

impl SpawnDungeonEnemies {
    pub fn from_built(parent: Entity, built: &BuiltDungeon, kind: DungeonKind, location_id: u32, power: f32) -> Self {
        Self {
            parent,
            enemies: plan_enemies(&built.enemies, kind, location_id, power),
            collision: built.collision(),
            spawn_pos: built.spawn_pos,
        }
    }
}

#[derive(Event, Debug, Clone, Copy)]
pub struct EnemyAttackEvent {
    pub enemy: Entity,
    pub mobile_id: u8,
    pub damage: i32,
}

#[derive(Event, Debug, Clone, Copy)]
pub struct DamageDungeonEnemy {
    pub enemy: Entity,
    pub amount: i32,
}

#[derive(Resource, Default)]
pub struct PendingDungeonEnemies {
    pub items: Vec<PendingItem>,
}

#[derive(Clone)]
pub struct PendingItem {
    pub pos: [f32; 3],
    pub species: usize,
    pub parent: Entity,
    pub collision: Arc<DungeonCollision>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Idle,
    Chase,
}

#[derive(Component)]
pub struct DungeonEnemy {
    species: usize,
    sprites: Arc<EnemySpriteSet>,
    collision: Arc<DungeonCollision>,
    pub health: i32,
    mode: Mode,
    ground: f32,
    heading: Vec2,
    aware: bool,
    sense_timer: f32,
    cooldown: f32,
    attack_step: Option<usize>,
    attack_timer: f32,
    attack_frame: usize,
    frame: usize,
    frame_timer: f32,
    kind: u8,
    shown: Option<(u8, usize, bool, usize)>,
    rng: u64,
}

#[derive(Resource, Default)]
pub struct DungeonSpriteCache(HashMap<u16, Option<Arc<EnemySpriteSet>>>);

pub struct DungeonEnemyPlugin;

impl Plugin for DungeonEnemyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DungeonSpriteCache>()
        .init_resource::<PendingDungeonEnemies>()
        .add_event::<SpawnDungeonEnemies>()
        .add_event::<EnemyAttackEvent>()
        .add_event::<DamageDungeonEnemy>()
        .add_systems(
            Update,
            (
                handle_spawn_requests,
             stream_dungeon_enemies,
             apply_enemy_damage,
             update_dungeon_enemies,
            )
            .chain(),
        );
    }
}

// ---------------------------------------------------------------------------
// Sistemi
// ---------------------------------------------------------------------------

fn handle_spawn_requests(
    mut requests: EventReader<SpawnDungeonEnemies>,
    mut pending: ResMut<PendingDungeonEnemies>,
    mut commands: Commands,
) {
    for request in requests.read() {
        if commands.get_entity(request.parent).is_none() {
            continue;
        }
        let count = request.enemies.len();
        for planned in &request.enemies {
            pending.items.push(PendingItem {
                pos: planned.pos,
                species: planned.species,
                parent: request.parent,
                collision: request.collision.clone(),
            });
        }
        info!(
            "Dungeon: {count} nemici in coda (streaming progressivo, raggio {STREAM_DISTANCE} m)"
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn stream_dungeon_enemies(
    mut commands: Commands,
    mut pending: ResMut<PendingDungeonEnemies>,
    data: Res<GameData>,
    mut mods: ResMut<ModTextures>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut cache: ResMut<DungeonSpriteCache>,
    player: Query<&Transform, With<Player>>,
    // Trasformazione mondiale dei root dei dungeon: serve a convertire la posizione
    // del giocatore (mondiale) nelle coordinate locali del dungeon (in cui sono
    // espressi `PendingItem::pos`). Senza questa conversione il confronto di
    // distanza è sempre "lontanissimo" e i nemici non vengono mai materializzati.
    parents: Query<&GlobalTransform>,
    mut quad: Local<Option<Handle<Mesh>>>,
    mut materialized: Local<u32>,
) {
    let Ok(player_tf) = player.get_single() else { return };
    let player_world = player_tf.translation;

    if pending.items.is_empty() {
        return;
    }

    let quad_handle = quad
    .get_or_insert_with(|| meshes.add(Rectangle::new(1.0, 1.0)))
    .clone();

    let mut remaining: Vec<PendingItem> = Vec::with_capacity(pending.items.len());
    for item in pending.items.drain(..) {
        // Il parent (root del dungeon) potrebbe essere stato despawnato fra un
        // frame e l'altro: in quel caso scartiamo l'elemento invece di tenerlo
        // per sempre in coda.
        let Ok(root_gtf) = parents.get(item.parent) else {
            continue;
        };

        // Converte il giocatore nello spazio locale del dungeon. `item.pos` è
        // relativo al root; sottraiamo la sua traslazione mondiale.
        let player_local = player_world - root_gtf.translation();

        let dx = item.pos[0] - player_local.x;
        let dz = item.pos[2] - player_local.z;
        let dist_sq = dx * dx + dz * dz;

        if dist_sq > STREAM_DISTANCE * STREAM_DISTANCE {
            remaining.push(item);
            continue;
        }

        let species = &SPECIES[item.species];
        let sprites = cache
        .0
        .entry(species.archive)
        .or_insert_with(|| {
            build_enemy_sprites(
                species.archive,
                species.has_idle,
                &data,
                &mut mods,
                &mut images,
                &mut materials,
            )
            .map(Arc::new)
        })
        .clone();
        let Some(sprites) = sprites else {
            warn!(
                "Sprite del mostro {} (TEXTURE.{}) non trovati",
                  species.id, species.archive
            );
            continue;
        };

        let mut rng = (item.pos[0].to_bits() as u64) << 32
        ^ item.pos[2].to_bits() as u64
        ^ 0xD1B5_4A32_D192_ED03;
        let health = species.health.0
        + (next_rand(&mut rng) * (species.health.1 - species.health.0 + 1) as f32) as i32;
        let angle = next_rand(&mut rng) * std::f32::consts::TAU;
        let size = sprites.walk.1[0];
        let [x, y, z] = item.pos;
        let ground = item.collision.floor_at(x, z, y + 1.0, 2.5).unwrap_or(y);
        let hover = if species.flying { HOVER } else { 0.0 };

        let material = sprites.walk.0[0][0].clone();
        commands.entity(item.parent).with_children(|parent| {
            parent.spawn((
                Mesh3d(quad_handle.clone()),
                          MeshMaterial3d(material),
                          Transform::from_xyz(x, ground + size.y * 0.5 + hover, z)
                          .with_scale(size.extend(1.0)),
                          DungeonEnemy {
                              species: item.species,
                              sprites,
                              collision: item.collision.clone(),
                          health: health.max(1),
                          mode: Mode::Idle,
                          ground,
                          heading: Vec2::new(angle.sin(), angle.cos()),
                          aware: false,
                          sense_timer: next_rand(&mut rng) * SENSE_INTERVAL,
                          cooldown: 0.0,
                          attack_step: None,
                          attack_timer: 0.0,
                          attack_frame: 0,
                          frame: 0,
                          frame_timer: 0.0,
                          kind: 0,
                          shown: None,
                          rng,
                          },
            ));
        });
        *materialized += 1;
    }
    pending.items = remaining;

    if *materialized > 0 && pending.items.is_empty() {
        info!(
            "Dungeon: tutti i {} nemici materializzati",
            *materialized
        );
    }
}

fn apply_enemy_damage(
    mut events: EventReader<DamageDungeonEnemy>,
    mut enemies: Query<&mut DungeonEnemy>,
    mut commands: Commands,
) {
    for event in events.read() {
        let Ok(mut enemy) = enemies.get_mut(event.enemy) else { continue };
        enemy.health -= event.amount;
        enemy.aware = true;
        if enemy.health <= 0 {
            commands.entity(event.enemy).despawn_recursive();
        }
    }
}

#[allow(clippy::type_complexity)]
fn update_dungeon_enemies(
    time: Res<Time>,
    player: Query<&Transform, (With<Player>, Without<DungeonEnemy>)>,
                          mut enemies: Query<(
                              Entity,
                              &mut DungeonEnemy,
                              &mut Transform,
                              &GlobalTransform,
                              &mut MeshMaterial3d<StandardMaterial>,
                          )>,
                          mut attacks: EventWriter<EnemyAttackEvent>,
) {
    let dt = time.delta_secs().min(0.1);
    let Ok(player_tf) = player.get_single() else { return };
    let player_pos = player_tf.translation;
    let cam_forward = player_tf.forward();
    let cam_xz = Vec2::new(cam_forward.x, cam_forward.z).normalize_or_zero();
    let yaw = (cam_xz != Vec2::ZERO).then(|| (-cam_xz.x).atan2(-cam_xz.y));

    for (entity, mut enemy, mut tf, gtf, mut material) in &mut enemies {
        let e = &mut *enemy;
        let sp = &SPECIES[e.species];

        let world = gtf.translation();
        let offset = world - tf.translation;
        let to_player = Vec2::new(player_pos.x - world.x, player_pos.z - world.z);
        let dist = to_player.length();
        let dy = player_pos.y - world.y;

        e.sense_timer -= dt;
        if e.sense_timer <= 0.0 {
            e.sense_timer = SENSE_INTERVAL;
            let near = dist < NOTICE_DISTANCE && dy.abs() < 8.0;
            let facing = if dist > 0.01 {
                let to_target = to_player / dist;
                e.heading.dot(to_target) > FACING_DOT_THRESHOLD
            } else {
                true
            };
            let seen = near && facing && {
                let eyes = [tf.translation.x, e.ground + 1.4, tf.translation.z];
                let target = player_pos - offset;
                e.collision.line_clear(eyes, [target.x, target.y, target.z], 1.0)
            };
            e.aware = seen || (e.aware && dist < LOSE_DISTANCE);
        }
        e.cooldown = (e.cooldown - dt).max(0.0);

        let in_melee = dist <= MELEE_RANGE && dy.abs() < 2.5;
        if e.attack_step.is_none() {
            e.mode = if e.aware { Mode::Chase } else { Mode::Idle };
            if e.aware && in_melee && e.cooldown <= 0.0 {
                e.attack_step = Some(0);
                e.attack_timer = 0.0;
                e.attack_frame = 0;
            }
        }
        if e.aware && dist > 0.01 {
            e.heading = to_player / dist;
        }

        if let Some(mut step) = e.attack_step {
            e.attack_timer += dt;
            let period = 1.0 / ENEMY_ATTACK_FPS;
            while e.attack_timer >= period {
                e.attack_timer -= period;
                step += 1;
                if step >= sp.attack.len() {
                    e.attack_step = None;
                    e.cooldown = ATTACK_COOLDOWN;
                    break;
                }
                if sp.attack[step] == -1 {
                    let damage = sp.damage.0
                    + (next_rand(&mut e.rng) * (sp.damage.1 - sp.damage.0 + 1) as f32) as i32;
                    if in_melee {
                        attacks.send(EnemyAttackEvent {
                            enemy: entity,
                            mobile_id: sp.id,
                            damage,
                        });
                    }
                } else {
                    e.attack_frame = sp.attack[step] as usize;
                }
                e.attack_step = Some(step);
            }
        }

        let mut moving = false;
        if e.mode == Mode::Chase && e.attack_step.is_none() && dist > MELEE_RANGE * 0.8 {
            let step_len = sp.speed * dt;
            let dir = to_player / dist.max(1e-3);
            let here = Vec2::new(tf.translation.x, tf.translation.z);
            for angle in [0.0f32, 0.7, -0.7, 1.4, -1.4] {
                let (s, c) = angle.sin_cos();
                let d = Vec2::new(dir.x * c - dir.y * s, dir.x * s + dir.y * c);
                let next = here + d * step_len;
                if e.collision.blocked(next.x, next.y, e.ground, BODY_RADIUS) {
                    continue;
                }
                let Some(floor) = e.collision.floor_at(next.x, next.y, e.ground, STEP_UP) else {
                    continue;
                };
                tf.translation.x = next.x;
                tf.translation.z = next.y;
                e.ground = floor;
                e.heading = d;
                moving = true;
                break;
            }
        }

        let attacking = e.attack_step.is_some();
        let has_idle_anim = e.sprites.idle.is_some();
        let (kind, fps) = if attacking {
            (1u8, ENEMY_ATTACK_FPS)
        } else if moving {
            (0u8, if sp.flying { ENEMY_FLY_FPS } else { ENEMY_MOVE_FPS })
        } else if has_idle_anim {
            (2u8, ENEMY_IDLE_FPS)
        } else {
            (0u8, ENEMY_MOVE_FPS)
        };
        let hold = !attacking && !moving && !has_idle_anim;
        if kind != e.kind {
            e.kind = kind;
            e.frame = 0;
            e.frame_timer = 0.0;
        }
        if !hold && !attacking {
            e.frame_timer += dt;
            let period = 1.0 / fps;
            while e.frame_timer >= period {
                e.frame_timer -= period;
                e.frame = (e.frame + 1) % 4096;
            }
        }

        let (offset_index, flip) = ORIENTATIONS[orientation_index(e.heading, to_player)];
        let set = match kind {
            1 => &e.sprites.attack,
            2 => e.sprites.idle.as_ref().unwrap_or(&e.sprites.walk),
            _ => &e.sprites.walk,
        };
        let frames = &set.0[offset_index];
        let frame = if hold {
            0
        } else if attacking {
            e.attack_frame.min(frames.len() - 1)
        } else {
            e.frame % frames.len()
        };
        let size = set.1[offset_index];
        if e.shown != Some((kind, offset_index, flip, frame)) {
            *material = MeshMaterial3d(frames[frame].clone());
            e.shown = Some((kind, offset_index, flip, frame));
        }

        tf.scale = Vec3::new(if flip { -size.x } else { size.x }, size.y, 1.0);
        let hover = if sp.flying { HOVER } else { 0.0 };
        tf.translation.y = e.ground + size.y * 0.5 + hover;
        if let Some(yaw) = yaw {
            tf.rotation = Quat::from_rotation_y(yaw);
        }
    }
}
