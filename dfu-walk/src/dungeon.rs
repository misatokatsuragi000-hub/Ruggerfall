//! Sistema di gestione dei Dungeon in Bevy: transizione con E/U, camminata,
//! collisioni sotterranee e porte apribili col tasto E o col tasto sinistro.
//!
//! ## Nemici
//!
//! I nemici sono i flat editor 199 con `FactionID != 0`, estratti da
//! `dungeon_builder::build_dungeon` in `BuiltDungeon::enemies`. Vengono gestiti
//! da `dungeon_enemies` tramite l'evento `SpawnDungeonEnemies`.
//!
//! ## NPC di quest
//!
//! I PNG della main quest (Lhotun, Akorithi, ecc.) NON sono flat del RDB.
//! Vengono spawnati dinamicamente da `qbn_npc.rs` sui marcatori di quest
//! (flat dell'archivio 199). Questo modulo spawna solo i flat decorativi
//! originali del RDB.
//!
//! ## Fix crash all'uscita
//!
//! All'uscita il root del dungeon viene despawnato (deferred). La coda
//! `PendingDungeonEnemies` contiene riferimenti a quel root: se non viene
//! svuotata, `stream_dungeon_enemies` al frame successivo prova ad aggiungere
//! figli a un'entità già despawnata (panic in Bevy 0.15). Per questo
//! `dungeon_enter_exit` azzera la coda prima del `despawn_recursive`.
//!
//! ## Priorità porta vs uscita (fix palazzo reale)
//!
//! Prima, `dungeon_enter_exit` usciva dal dungeon ogni volta che il player
//! premeva E entro `DUNGEON_EXIT_DISTANCE` (4 m) da `exit_pos`. Nei dungeon
//! senza marker 70300 vero (come il palazzo reale), `exit_pos` è il fallback
//! `spawn_pos`: premere E su una porta vicino allo spawn teletrasportava il
//! player fuori dal dungeon. Inoltre, anche con marker vero, una porta a
//! portata veniva ignorata perché il marker vinceva.
//!
//! Ora:
//!
//! 1. `dungeon_enter_exit` dà **priorità alla porta**: se c'è una porta entro
//!    `DOOR_INTERACTION_REACH` dal player, il tasto E non esce (lo gestirà
//!    `dungeon_move` aprendola).
//! 2. L'uscita con E scatta se il player è entro `DUNGEON_EXIT_DISTANCE` da
//!    `exit_pos` **e** non c'è una porta a portata, indipendentemente da
//!    `has_exit_marker`. Backspace esce sempre.
//! 3. `dungeon_move` non ha più il check "sei vicino all'uscita?": apre la
//!    porta se ce n'è una a portata, indipendentemente dall'uscita.
//!
//! ## Ingresso nel palazzo (Sentinel, Wayrest, ...)
//!
//! In Daggerfall il palazzo di ogni città è un **dungeon RDB** (come per la
//! città di Daggerfall stessa), non un interno. Il centro del palazzo viene
//! calcolato da `palace_center_in_tile`:
//!
//! - **Daggerfall**: layout procedurale hardcoded nel blocco `(5, 6)` (dove
//!   `city_buildings.rs` piazza il palazzo procedurale);
//! - **Altre città**: si cerca un qualunque blocco con un subrecord
//!   `building_type == 16` (la stessa logica di `city_map::poi_kind`), e si
//!   usa l'ancora di quel subrecord come centro. In alcune città il palazzo
//!   non è un blocco `PALA*`/`CAST*` ma può stare in `RESI*`, `GENR*`, `BLDG*`
//!   e simili — per questo filtrare per nome non basta.
//!
//! Se non si trova nessun blocco con `building_type == 16`,
//! `player_distance_from_palace` ritorna `None` e `spawn_and_enter_dungeon`
//! esce con un log esplicito (invece del vecchio `return false` silenzioso).
//!
//! ## Collisione per i follower
//!
//! `DungeonState::blocked(x, z, feet_y, radius)` permette a
//! `tiles::update_followers` di far fermare i follower sui muri e sulle porte
//! chiuse. Le coordinate sono **mondo**; vengono convertite in locali
//! sottraendo `ActiveDungeon::origin`.

use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use crate::data::blocks::RmbBlock;
use crate::data::bsa::BsaArchive;
use crate::data::building_gen::{BLOCK_SIZE_M, DF_UNIT};
use crate::data::dungeon_builder::{
    build_dungeon, BuiltDungeon,
};
use crate::data::dungeon_layout::DungeonData;
use crate::data::maps::DFLocation;
use crate::data::mesh_builder::{Submesh, WorldMesh};
use crate::data::terrain_mesh::MAP_PIXEL_SIZE;
use crate::dialog::DialogState;
use crate::dungeon_enemies::{
    SpawnDungeonEnemies, DungeonKind, PendingDungeonEnemies, DEFAULT_MONSTER_POWER,
};
use crate::fps_weapon::FpsWeaponState;
use crate::mod_textures::ModTextures;
use crate::player::{Player, EYE_HEIGHT};
use crate::wall_textures::WallTextures;
use crate::world::*;
use std::collections::HashMap;

const DUNGEON_Y: f32 = 12000.0;
const DUNGEON_EXIT_DISTANCE: f32 = 4.0;
const WALK_SPEED: f32 = 4.5;
const RUN_MULTIPLIER: f32 = 1.8;
const GRAVITY: f32 = 20.0;
const JUMP_SPEED: f32 = 5.5;
const RADIUS: f32 = 0.35;
const STEP_UP: f32 = 0.8;
const SNAP_DOWN: f32 = 0.8;

const SPAWN_CLEARANCE: f32 = 0.6;
const PALACE_DUNGEON_RADIUS: f32 = 50.0;
/// Raggio entro cui E attiva l'ingresso di una località dungeon (quest, fortezze...)
/// che non ha un palazzo. Si misura dalla porta del dungeon, o dal centro della
/// località se il modello della porta non è noto.
const DUNGEON_ENTRANCE_RADIUS: f32 = 12.0;

const DOOR_OPEN_DURATION: f32 = 1.5;
const DOOR_INTERACTION_REACH: f32 = 2.8;

/// Con l'arma sguainata il click sinistro è del combattimento e non apre porte.
const CLICK_OPENS_DOORS: bool = true;

#[derive(Component)]
pub struct DungeonFlat;

#[derive(Component)]
pub struct DungeonFlatSource {
    pub archive: u16,
    pub record: u8,
    pub size: Vec2,
}
#[derive(Component)]
pub struct DungeonDoorEntity {
    pub index: usize,
    pub closed_yaw: f32,
    pub progress: f32,
}

pub struct ActiveDungeon {
    pub root: Entity,
    /// Nome della località (es. "Castle Faallem"): serve a piazzare gli oggetti di quest.
    pub name: String,
    pub origin: Vec3,
    pub dungeon: BuiltDungeon,
    pub saved_translation: Vec3,
    pub saved_feet_y: f32,
    pub saved_yaw: f32,
    pub saved_origin: (i32, i32),
}

#[derive(Resource, Default)]
pub struct DungeonState {
    pub active: Option<ActiveDungeon>,
}

impl DungeonState {
    pub fn is_inside(&self) -> bool {
        self.active.is_some()
    }

    /// True se un cerchio di raggio `radius` centrato in `(x, z)` (coordinate
    /// **mondo**, come `Transform::translation`) alla quota `feet_y` (piedi,
    /// sempre **mondo**) tocca un muro o una porta chiusa del dungeon corrente.
    pub fn blocked(&self, x: f32, z: f32, feet_y: f32, radius: f32) -> bool {
        match &self.active {
            Some(a) => a.dungeon.blocked(
                x - a.origin.x,
                z - a.origin.z,
                feet_y - a.origin.y,
                radius,
            ),
            None => false,
        }
    }
}

pub struct DungeonPlugin;

impl Plugin for DungeonPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DungeonState>()
        .add_systems(
            Update,
            (
                check_initial_dungeon_spawn,
             dungeon_enter_exit,
             handle_dungeon_door_click,
             dungeon_move,
             face_dungeon_flats,
            )
            .chain()
            .before(crate::interior::interior_enter_exit)
            .before(crate::player::player_move),
        );
    }
}

fn submesh_to_bevy(mesh: &WorldMesh, sm: &Submesh, size: (f32, f32)) -> Mesh {
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
    let mut out = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    out.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    out.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    out.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    out.insert_indices(Indices::U32(indices));
    out
}

fn ensure_spawn_is_free(built: &mut BuiltDungeon) {
    let original = built.spawn_pos;
    if !built.blocked(original[0], original[2], original[1], SPAWN_CLEARANCE) {
        return;
    }
    for radius in [0.6f32, 0.9, 1.2, 1.6, 2.0, 2.5, 3.0, 4.0, 5.0] {
        for k in 0..24 {
            let a = k as f32 / 24.0 * std::f32::consts::TAU;
            let nx = original[0] + a.cos() * radius;
            let nz = original[2] + a.sin() * radius;
            if let Some(fy) = built.floor_at(nx, nz, original[1], 2.0) {
                if !built.blocked(nx, nz, fy, SPAWN_CLEARANCE) {
                    built.spawn_pos = [nx, fy, nz];
                    println!(
                        "Dungeon: spawn spostato dal marcatore ({:.1},{:.1},{:.1}) a ({:.1},{:.1},{:.1})",
                             original[0], original[1], original[2],
                             built.spawn_pos[0], built.spawn_pos[1], built.spawn_pos[2],
                    );
                    return;
                }
            }
        }
    }
}

// =============================================================================
// Riconoscimento del palazzo reale (robusto al nome della location)
// =============================================================================

/// `true` se il map pixel contiene il palazzo reale (procedurale per
/// Daggerfall, o un `building_type == 16` in un altro blocco).
pub fn is_palace_dungeon(loc: &DFLocation, data: &GameData) -> bool {
    palace_center_in_tile(loc, data).is_some()
}

/// Centro del palazzo nel map pixel, in coordinate tile (metri, come
/// `tile_translation`).
fn palace_center_in_tile(loc: &DFLocation, data: &GameData) -> Option<(f32, f32)> {
    let bw = loc.width.min(8);
    let bh = loc.height.min(8);
    let start_bx = (8 - bw) / 2;
    let start_by = (8 - bh) / 2;

    // Daggerfall: layout procedurale hardcoded, blocco (5, 6).
    if loc.name.eq_ignore_ascii_case("DAGGERFALL") {
        let target_bx = 5.min(bw.saturating_sub(2));
        let target_by = 6.min(bh.saturating_sub(2));
        let cx = (start_bx + target_bx) as f32 * BLOCK_SIZE_M + BLOCK_SIZE_M * 0.5;
        let cz = -((start_by + target_by) as f32 * BLOCK_SIZE_M + BLOCK_SIZE_M * 0.5);
        return Some((cx, cz));
    }

    // Altre città: cerca un qualunque blocco con un subrecord di tipo 16.
    let blocks = BsaArchive::parse(&data.blocks_buf).ok()?;
    let mut scanned = 0usize;
    let mut with_type16 = 0usize;
    for by in 0..bh {
        for bx in 0..bw {
            let Some(name) = loc.block_names.get(by * loc.width + bx) else { continue };
            scanned += 1;
            let Some(raw) = blocks.get_by_name(name) else { continue };
            let Ok(mut rmb) = RmbBlock::parse(raw) else { continue };
            rmb.set_mirror_local(true);

            let Some(sub) = rmb.buildings.iter().position(|b| b.building_type == 16) else {
                continue;
            };
            with_type16 += 1;

            let Some(ax) = rmb.subrecord_anchor_x(sub) else { continue };
            let Some(an) = rmb.subrecord_anchor_n(sub) else { continue };

            let gx = start_bx + bx;
            let gz = start_by + by;
            let bx0 = gx as f32 * BLOCK_SIZE_M;
            let bz0 = gz as f32 * BLOCK_SIZE_M;
            let cx = bx0 + ax * DF_UNIT;
            let cz = -(bz0 + an * DF_UNIT);
            println!(
                "Palazzo '{}' trovato: blocco {} ({}), sub {} (building_type 16), centro ({:.1}, {:.1})",
                     loc.name, name, name.to_ascii_uppercase(), sub, cx, cz
            );
            return Some((cx, cz));
        }
    }
    println!(
        "Nessun building_type 16 in '{}' ({} blocchi scansionati, {} con tipo 16)",
             loc.name, scanned, with_type16
    );
    None
}

/// Modelli ARCH3D delle porte d'ingresso dei dungeon. Si possono indicare anche con
/// la variabile d'ambiente `DF_DUNGEON_DOOR_MODELS="1234,5678"`.
const DUNGEON_DOOR_MODEL_IDS: &[u32] = &[];

fn dungeon_door_models() -> Vec<u32> {
    let mut ids: Vec<u32> = DUNGEON_DOOR_MODEL_IDS.to_vec();
    if let Ok(v) = std::env::var("DF_DUNGEON_DOOR_MODELS") {
        ids.extend(v.split(',').filter_map(|t| t.trim().parse::<u32>().ok()));
    }
    ids
}

/// Posizione (coordinate tile) dell'ingresso di una località dungeon senza palazzo.
fn dungeon_entrance_in_tile(loc: &DFLocation, data: &GameData) -> (f32, f32) {
    let bw = loc.width.min(8);
    let bh = loc.height.min(8);
    let start_bx = (8 - bw) / 2;
    let start_by = (8 - bh) / 2;
    let door_ids = dungeon_door_models();

    if !door_ids.is_empty() {
        if let Ok(blocks) = BsaArchive::parse(&data.blocks_buf) {
            for by in 0..bh {
                for bx in 0..bw {
                    let Some(name) = loc.block_names.get(by * loc.width + bx) else { continue };
                    let Some(raw) = blocks.get_by_name(name) else { continue };
                    let Ok(mut rmb) = RmbBlock::parse(raw) else { continue };
                    rmb.set_mirror_local(true);
                    let bx0 = (start_bx + bx) as f32 * BLOCK_SIZE_M;
                    let bz0 = (start_by + by) as f32 * BLOCK_SIZE_M;
                    for pm in rmb.placed_models().iter() {
                        if door_ids.contains(&pm.model_id) {
                            let (ex, ez) = (bx0 + pm.x * DF_UNIT, -(bz0 + pm.n * DF_UNIT));
                            println!(
                                "Dungeon '{}': porta (modello {}) nel blocco {} a ({:.1}, {:.1}).",
                                     loc.name, pm.model_id, name, ex, ez
                            );
                            return (ex, ez);
                        }
                    }
                }
            }
        }
    }

    let cx = (start_bx as f32 + bw as f32 * 0.5) * BLOCK_SIZE_M;
    let cz = -((start_by as f32 + bh as f32 * 0.5) * BLOCK_SIZE_M);
    (cx, cz)
}

fn player_distance_from_palace(
    loc: &DFLocation,
    origin: &Origin,
    mx: i32,
    my: i32,
    translation: Vec3,
    data: &GameData,
) -> Option<f32> {
    let (px, pz) = palace_center_in_tile(loc, data)?;
    let tile = tile_translation(origin, mx, my);
    let dx = translation.x - tile.x - px;
    let dz = translation.z - tile.z - pz;
    Some((dx * dx + dz * dz).sqrt())
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_and_enter_dungeon(
    commands: &mut Commands,
    enemy_events: &mut EventWriter<SpawnDungeonEnemies>,
    data: &GameData,
    origin: &Origin,
    state: &mut DungeonState,
    clear: &mut ClearColor,
    meshes: &mut Assets<Mesh>,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    wall_textures: &mut WallTextures,
    mods: &mut ModTextures,
    p: &mut Player,
    tf: &mut Transform,
    loc: &DFLocation,
    mx: i32,
    my: i32,
) -> bool {
    let near_external_dungeon_entrance =
    match player_distance_from_palace(loc, origin, mx, my, tf.translation, data) {
        Some(dist) => {
            if dist > PALACE_DUNGEON_RADIUS {
                println!(
                    "Palazzo '{}': sei a {:.1} m dal centro, servono entro {:.0} m.",
                    loc.name, dist, PALACE_DUNGEON_RADIUS
                );
                return false;
            }
            println!(
                "Vicino al palazzo reale ({:.1} m): provo a entrare nel dungeon di {}.",
                     dist, loc.name,
            );
            true
        }
        None => {
            let (ex, ez) = dungeon_entrance_in_tile(loc, data);
            let tile = tile_translation(origin, mx, my);
            let dx = tf.translation.x - tile.x - ex;
            let dz = tf.translation.z - tile.z - ez;
            let d = (dx * dx + dz * dz).sqrt();
            if d > DUNGEON_ENTRANCE_RADIUS {
                println!(
                    "Dungeon '{}': sei a {:.1} m dall'ingresso, servono entro {:.0} m.",
                    loc.name, d, DUNGEON_ENTRANCE_RADIUS
                );
                return false;
            }
            println!(
                "Vicino all'ingresso del dungeon '{}' ({:.1} m): provo a entrare.",
                     loc.name, d
            );
            true
        }
    };
    let _ = near_external_dungeon_entrance;

    let Ok(maps_bsa) = BsaArchive::parse(&data.maps_buf) else {
        println!("Impossibile parsare MAPS.BSA");
        return false;
    };
    let Ok(blocks_bsa) = BsaArchive::parse(&data.blocks_buf) else {
        println!("Impossibile parsare BLOCKS.BSA");
        return false;
    };
    let Ok(arch_bsa) = BsaArchive::parse(&data.arch3d_buf) else {
        println!("Impossibile parsare ARCH3D.BSA");
        return false;
    };

    let Some(dungeon_data) =
    DungeonData::read_from_maps(&maps_bsa, loc.region_index, loc.location_id)
    else {
        println!(
            "DungeonData mancante per '{}' (region {}, loc_id {}): \
MAPDITEM non contiene RDB per questo location_id.",
loc.name, loc.region_index, loc.location_id
        );
        return false;
    };

    println!(
        "Caricamento dungeon di {} ({} blocchi, location_id {}).",
             loc.name,
             dungeon_data.blocks.len(),
             loc.location_id,
    );
    let Some(mut built) = build_dungeon(&dungeon_data, &blocks_bsa, &arch_bsa) else {
        println!(
            "build_dungeon fallito per '{}' ({} blocchi RDB).",
                 loc.name, dungeon_data.blocks.len()
        );
        return false;
    };

    if !built.has_exit_marker {
        info!(
            "Dungeon di {}: nessun marker di uscita (modello 70300) nel RDB. \
`exit_pos` è il fallback a `spawn_pos` ({:.1},{:.1}): il tasto E (vicino al \
punto d'ingresso) e Backspace escono. Le porte vicino allo spawn ora si aprono.",
loc.name, built.spawn_pos[0], built.spawn_pos[2]
        );
    }

    ensure_spawn_is_free(&mut built);

    let origin_world = Vec3::new(tf.translation.x, DUNGEON_Y, tf.translation.z);
    let climate = crate::tiles::climate_of(data, mx, my);

    let root = commands
    .spawn((Transform::from_translation(origin_world), Visibility::default()))
    .with_children(|parent| {
        // Mesh principale (muri, pavimenti, soffitti).
        for sm in &built.mesh.submeshes {
            if sm.indices.is_empty() { continue; }
            let size = wall_textures.original_size(sm.texture_archive, sm.texture_record, data);
            let handle = wall_textures.material(
                sm.texture_archive, sm.texture_record, climate,
                data, mods, images, materials,
            );
            if let Some(m) = materials.get_mut(&handle) {
                m.double_sided = true;
                m.cull_mode = None;
            }
            let mut bevy_mesh = submesh_to_bevy(&built.mesh, sm, size);
            wall_textures.prepare_mesh(&handle, &mut bevy_mesh);
            parent.spawn((
                Mesh3d(meshes.add(bevy_mesh)),
                          MeshMaterial3d(handle),
                          Transform::IDENTITY,
            ));
        }

        // Porta d'uscita (modello 70300).
        for sm in &built.visible_only.submeshes {
            if sm.indices.is_empty() { continue; }
            let size = wall_textures.original_size(sm.texture_archive, sm.texture_record, data);
            let handle = wall_textures.material(
                sm.texture_archive, sm.texture_record, climate,
                data, mods, images, materials,
            );
            if let Some(m) = materials.get_mut(&handle) {
                m.double_sided = true;
                m.cull_mode = None;
            }
            let mut bevy_mesh = submesh_to_bevy(&built.visible_only, sm, size);
            wall_textures.prepare_mesh(&handle, &mut bevy_mesh);
            parent.spawn((
                Mesh3d(meshes.add(bevy_mesh)),
                          MeshMaterial3d(handle),
                          Transform::IDENTITY,
            ));
        }

        // Porte apribili.
        for (i, d) in built.doors.iter().enumerate() {
            parent
            .spawn((
                Transform::from_translation(Vec3::from(d.pos))
                .with_rotation(Quat::from_rotation_y(d.closed_yaw.to_radians())),
                    Visibility::default(),
                    DungeonDoorEntity {
                        index: i,
                        closed_yaw: d.closed_yaw,
                        progress: 0.0,
                    },
            ))
            .with_children(|d_parent| {
                for sm in &d.mesh.submeshes {
                    if sm.indices.is_empty() { continue; }
                    let size = wall_textures.original_size(
                        sm.texture_archive, sm.texture_record, data,
                    );
                    let handle = wall_textures.material(
                        sm.texture_archive, sm.texture_record, climate,
                        data, mods, images, materials,
                    );
                    if let Some(m) = materials.get_mut(&handle) {
                        m.double_sided = true;
                        m.cull_mode = None;
                    }
                    let mut b_mesh = submesh_to_bevy(&d.mesh, sm, size);
                    wall_textures.prepare_mesh(&handle, &mut b_mesh);
                    d_parent.spawn((
                        Mesh3d(meshes.add(b_mesh)),
                                    MeshMaterial3d(handle),
                                    Transform::IDENTITY,
                    ));
                }
            });
        }

        // Luci puntuali del dungeon.
        for (l_pos, radius) in &built.lights {
            parent.spawn((
                PointLight {
                    color: Color::srgb(1.0, 0.65, 0.28),
                          intensity: 85_000.0,
                          range: (*radius).max(8.0),
                          shadows_enabled: false,
                          ..default()
                },
                Transform::from_xyz(l_pos[0], l_pos[1], l_pos[2]),
            ));
        }

        // Luce d'ingresso.
        parent.spawn((
            PointLight {
                color: Color::srgb(1.0, 0.80, 0.45),
                      intensity: 90_000.0,
                      range: 18.0,
                      shadows_enabled: false,
                      ..default()
            },
            Transform::from_xyz(
                built.spawn_pos[0],
                built.spawn_pos[1] + 2.0,
                built.spawn_pos[2],
            ),
        ));

        // Flat decorativi originali del RDB (arazzi, quadri, cortigiani, ecc.).
        // Gli NPC di quest sono spawnati da `qbn_npc.rs`, non qui.
        for (f_pos, archive, record) in &built.flats {
            let Some((material, w_px, h_px)) = wall_textures.flat_material(
                *archive, *record, data, mods, images, materials,
            ) else { continue };

            let (w, h) = (w_px * DF_UNIT, h_px * DF_UNIT);

            parent.spawn((
                Mesh3d(meshes.add(Rectangle::new(w, h))),
                          MeshMaterial3d(material),
                          Transform::from_xyz(f_pos[0], f_pos[1], f_pos[2]),
                          DungeonFlat,
                          DungeonFlatSource {
                              archive: *archive,
                              record: *record,
                              size: Vec2::new(w, h),
                          },
            ));
        }
    })
    .id();

    // Nemici.
    enemy_events.send(SpawnDungeonEnemies::from_built(
        root,
        &built,
        DungeonKind::Crypt,
        loc.location_id,
        DEFAULT_MONSTER_POWER,
    ));

    let spawn = built.spawn_pos;
    let floor_y = built
    .floor_at(spawn[0], spawn[2], spawn[1], 2.0)
    .unwrap_or(spawn[1]);

    state.active = Some(ActiveDungeon {
        root,
        name: loc.name.clone(),
                        origin: origin_world,
                        dungeon: built,
                        saved_translation: tf.translation,
                        saved_feet_y: p.feet_y,
                        saved_yaw: p.yaw,
                        saved_origin: (origin.east, origin.north),
    });

    p.feet_y = origin_world.y + floor_y;
    p.vel_y = 0.0;
    p.on_ground = true;
    p.fly = false;
    tf.translation = Vec3::new(
        origin_world.x + spawn[0],
        p.feet_y + EYE_HEIGHT,
        origin_world.z + spawn[2],
    );

    clear.0 = Color::BLACK;
    true
}

#[allow(clippy::too_many_arguments)]
fn check_initial_dungeon_spawn(
    mut commands: Commands,
    mut enemy_events: EventWriter<SpawnDungeonEnemies>,
    mut started: Local<bool>,
    data: Res<GameData>,
    origin: Res<Origin>,
    mut state: ResMut<DungeonState>,
    mut clear: ResMut<ClearColor>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut wall_textures: ResMut<WallTextures>,
    mut mods: ResMut<ModTextures>,
    mut player: Query<(&mut Player, &mut Transform)>,
                               start_config: Option<Res<crate::StartDungeonConfig>>,
) {
    if *started { return; }
    *started = true;

    let should_start_in_dungeon = start_config.map_or(true, |c| c.start_in_dungeon);
    if !should_start_in_dungeon { return; }

    let Ok((mut p, mut tf)) = player.get_single_mut() else { return };
    let (mx, my) = map_pixel_of(&origin, tf.translation);

    let target_loc = data.locations.get(&(mx, my)).or_else(|| {
        data.locations
        .values()
        .find(|l| l.name.to_ascii_uppercase().contains("PRIVATEER"))
    });
    let Some(loc) = target_loc else { return };

    if spawn_and_enter_dungeon(
        &mut commands, &mut enemy_events, &data, &origin, &mut state, &mut clear,
        &mut meshes, &mut images, &mut materials, &mut wall_textures, &mut mods,
        &mut p, &mut tf, loc, mx, my,
    ) {
        println!("\n=======================================================");
        println!(" Benvenuto in Daggerfall: ti risvegli a Privateer's Hold!");
        println!(" Trova l'uscita o premi Backspace per uscire all'aperto.");
        println!(" E o click sinistro (arma rinfoderata) = apri/chiudi la porta.");
        println!("=======================================================\n");
    }
}

#[allow(clippy::too_many_arguments)]
pub fn dungeon_enter_exit(
    mut commands: Commands,
    mut enemy_events: EventWriter<SpawnDungeonEnemies>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    data: Res<GameData>,
    origin: Res<Origin>,
    mut state: ResMut<DungeonState>,
    mut clear: ResMut<ClearColor>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut wall_textures: ResMut<WallTextures>,
    mut mods: ResMut<ModTextures>,
    mut player: Query<(&mut Player, &mut Transform)>,
                          // La coda nemici contiene `item.parent` = root del dungeon che stiamo
                          // per despawnare. Svuotarla PRIMA del `despawn_recursive` evita che
                          // `stream_dungeon_enemies` al frame successivo provi ad aggiungere
                          // figli a un'entità già despawnata (panic in Bevy 0.15).
                          mut pending_enemies: ResMut<PendingDungeonEnemies>,
) {
    let leave = keys.just_pressed(KeyCode::Backspace);
    let act = keys.just_pressed(KeyCode::KeyE);
    if !leave && !act { return; }
    let Ok((mut p, mut tf)) = player.get_single_mut() else { return };

    if let Some(active) = &state.active {
        let local = tf.translation - active.origin;
        let d_exit = (local.x - active.dungeon.exit_pos[0])
        .hypot(local.z - active.dungeon.exit_pos[2]);

        // Priorità alla porta.
        let feet_local = p.feet_y - active.origin.y;
        let door_in_reach = active.dungeon.doors.iter().any(|d| {
            if (d.center[1] - feet_local).abs() > 2.5 {
                return false;
            }
            let dist = (d.center[0] - local.x).hypot(d.center[2] - local.z);
            dist <= DOOR_INTERACTION_REACH
        });

        let real_exit_in_reach = d_exit < DUNGEON_EXIT_DISTANCE;
        let exit_now = leave || (act && real_exit_in_reach && !door_in_reach);

        if exit_now {
            let active = state.active.take().unwrap();

            let cleared = pending_enemies.items.len();
            pending_enemies.items.clear();
            if cleared > 0 {
                info!("Dungeon: {} nemici in coda scartati (uscita)", cleared);
            }

            let (de, dn) = (
                origin.east - active.saved_origin.0,
                origin.north - active.saved_origin.1,
            );
            let mut pos = active.saved_translation;
            pos.x -= de as f32 * MAP_PIXEL_SIZE;
            pos.z += dn as f32 * MAP_PIXEL_SIZE;

            p.feet_y = active.saved_feet_y;
            p.vel_y = 0.0;
            p.on_ground = true;
            p.yaw = active.saved_yaw + std::f32::consts::PI;
            tf.translation = pos;

            commands.entity(active.root).despawn_recursive();
            clear.0 = Color::srgb(0.55, 0.72, 0.92);

            let reason = if leave { "Backspace" } else { "E sull'uscita" };
            println!("Sei uscito dal dungeon ({reason}).");

            keys.clear_just_pressed(KeyCode::KeyE);
            keys.clear_just_pressed(KeyCode::Backspace);
        }
        return;
    }

    if !act { return; }
    let (mx, my) = map_pixel_of(&origin, tf.translation);
    let Some(loc) = data.locations.get(&(mx, my)) else {
        println!("Nessuna location al map pixel ({}, {}).", mx, my);
        return;
    };

    if spawn_and_enter_dungeon(
        &mut commands, &mut enemy_events, &data, &origin, &mut state, &mut clear,
        &mut meshes, &mut images, &mut materials, &mut wall_textures, &mut mods,
        &mut p, &mut tf, loc, mx, my,
    ) {
        keys.clear_just_pressed(KeyCode::KeyE);
    }
}

pub fn handle_dungeon_door_click(
    mouse: Res<ButtonInput<MouseButton>>,
    dialog: Res<DialogState>,
    weapon: Option<Res<FpsWeaponState>>,
    mut state: ResMut<DungeonState>,
    player: Query<&Transform, With<Player>>,
) {
    if !CLICK_OPENS_DOORS { return; }
    if dialog.open { return; }
    if !mouse.just_pressed(MouseButton::Left) { return; }
    if weapon.as_ref().is_some_and(|w| w.drawn) { return; }

    let Some(active) = state.active.as_mut() else { return };
    let Ok(tf) = player.get_single() else { return };

    let local = tf.translation - active.origin;
    let feet_local = local.y - EYE_HEIGHT;

    if let Some(i) = active
        .dungeon
        .toggle_nearest_door(local.x, local.z, feet_local, DOOR_INTERACTION_REACH)
        {
            println!(
                "Porta {} {}.",
                i,
                if active.dungeon.door_open[i] { "aperta" } else { "chiusa" }
            );
        }
}

pub fn dungeon_move(
    dialog: Res<DialogState>,
    time: Res<Time>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut state: ResMut<DungeonState>,
    mut doors_q: Query<(&mut Transform, &mut DungeonDoorEntity), Without<Player>>,
                    mut player: Query<(&mut Player, &mut Transform)>,
) {
    if dialog.open { return; }
    let Some(active) = state.active.as_mut() else { return };
    let Ok((mut p, mut tf)) = player.get_single_mut() else { return };

    let dt = time.delta_secs().min(0.05);
    let (dungeon, o) = (&mut active.dungeon, active.origin);

    if keys.just_pressed(KeyCode::KeyE) {
        let (lx, lz) = (tf.translation.x - o.x, tf.translation.z - o.z);
        let feet = p.feet_y - o.y;

        if let Some(door_idx) = dungeon.toggle_nearest_door(lx, lz, feet, DOOR_INTERACTION_REACH) {
            println!(
                "Porta {} {}.",
                door_idx,
                if dungeon.door_open[door_idx] { "aperta" } else { "chiusa" }
            );
            keys.clear_just_pressed(KeyCode::KeyE);
        }
    }

    let speed = 1.0 / DOOR_OPEN_DURATION;
    for (mut d_tf, mut d_ent) in &mut doors_q {
        if d_ent.index >= dungeon.doors.len() { continue; }
        let door = &dungeon.doors[d_ent.index];
        let target = if dungeon.door_open[d_ent.index] { 1.0 } else { 0.0 };
        if d_ent.progress < target {
            d_ent.progress = (d_ent.progress + speed * dt).min(target);
        } else if d_ent.progress > target {
            d_ent.progress = (d_ent.progress - speed * dt).max(target);
        }
        let angle = d_ent.closed_yaw.to_radians()
        + d_ent.progress * door.open_angle_deg.to_radians();
        d_tf.rotation = Quat::from_rotation_y(angle);
    }

    let forward = Vec3::new(-p.yaw.sin(), 0.0, -p.yaw.cos());
    let right = Vec3::new(p.yaw.cos(), 0.0, -p.yaw.sin());
    let axis = |pos: KeyCode, neg: KeyCode| {
        keys.pressed(pos) as i32 as f32 - keys.pressed(neg) as i32 as f32
    };
    let wish = (forward * axis(KeyCode::KeyW, KeyCode::KeyS)
    + right * axis(KeyCode::KeyD, KeyCode::KeyA))
    .normalize_or_zero();

    let running = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let speed_walk = WALK_SPEED * if running { RUN_MULTIPLIER } else { 1.0 };
    let total = speed_walk * dt;
    let steps = (total / 0.1).ceil().max(1.0) as i32;
    let step = wish * (total / steps as f32);

    let mut pos = tf.translation;
    if step != Vec3::ZERO {
        for _ in 0..steps {
            let feet = p.feet_y - o.y;
            let cur = dungeon.clearance(pos.x - o.x, pos.z - o.z, feet, RADIUS);
            for cand in [step, Vec3::new(step.x, 0.0, 0.0), Vec3::new(0.0, 0.0, step.z)] {
                if cand == Vec3::ZERO { continue; }
                let n = pos + cand;
                let (nx, nz) = (n.x - o.x, n.z - o.z);
                let ok = if cur >= RADIUS {
                    !dungeon.blocked(nx, nz, feet, RADIUS)
                } else {
                    dungeon.clearance(nx, nz, feet, RADIUS) >= cur - 1e-4
                };
                if ok {
                    pos = n;
                    break;
                }
            }
        }
    }

    let (lx, lz) = (pos.x - o.x, pos.z - o.z);
    let feet_local = p.feet_y - o.y;
    let floor = dungeon.floor_at(lx, lz, feet_local, STEP_UP);

    if keys.just_pressed(KeyCode::Space) && p.on_ground {
        p.vel_y = JUMP_SPEED;
        p.on_ground = false;
    }

    match floor {
        Some(f) if p.vel_y <= 0.0 && feet_local - f <= SNAP_DOWN => {
            p.feet_y = o.y + f;
            p.vel_y = 0.0;
            p.on_ground = true;
        }
        _ => {
            p.vel_y -= GRAVITY * dt;
            p.feet_y += p.vel_y * dt;
            p.on_ground = false;
            if let Some(f) = floor {
                if p.feet_y - o.y <= f {
                    p.feet_y = o.y + f;
                    p.vel_y = 0.0;
                    p.on_ground = true;
                }
            }
        }
    }

    if p.feet_y - o.y < dungeon.bounds_min[1] - 10.0 {
        let open_doors: Vec<usize> = dungeon
        .door_open
        .iter()
        .enumerate()
        .filter(|(_, open)| **open)
        .map(|(i, _)| i)
        .collect();
        println!(
            "Dungeon: CADUTO fuori dalla mappa (locale x={:.1} z={:.1} feet_y={:.1}, \
minimo mesh y={:.1}); porte aperte: {:?}. Respawn all'ingresso.",
pos.x - o.x, pos.z - o.z, p.feet_y - o.y, dungeon.bounds_min[1], open_doors,
        );
        let sp = dungeon.spawn_pos;
        let fy = dungeon.floor_at(sp[0], sp[2], sp[1], 2.0).unwrap_or(sp[1]);
        pos = Vec3::new(o.x + sp[0], 0.0, o.z + sp[2]);
        p.feet_y = o.y + fy;
        p.vel_y = 0.0;
        p.on_ground = true;
    }

    pos.y = p.feet_y + EYE_HEIGHT;
    tf.translation = pos;
}

pub fn face_dungeon_flats(
    player: Query<&Player>,
    mut flats: Query<&mut Transform, (With<DungeonFlat>, Without<Player>)>,
) {
    let Ok(p) = player.get_single() else { return };
    let rot = Quat::from_rotation_y(p.yaw);
    for mut tf in &mut flats {
        tf.rotation = rot;
    }
}
