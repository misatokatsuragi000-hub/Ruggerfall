//! Sistema di gestione dei Dungeon in Bevy: transizione con E/U, camminata e collisioni sotterranee.

use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use crate::data::bsa::BsaArchive;
use crate::data::building_gen::DF_UNIT;
use crate::data::dungeon_builder::{build_dungeon, BuiltDungeon};
use crate::data::dungeon_layout::DungeonData;
use crate::data::maps::DFLocation;
use crate::data::mesh_builder::{Submesh, WorldMesh};
use crate::data::terrain_mesh::MAP_PIXEL_SIZE;
use crate::dialog::DialogState;
use crate::mod_textures::ModTextures;
use crate::player::{Player, EYE_HEIGHT};
use crate::tiles::climate_of;
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
const STEP_UP: f32 = 0.6;
const SNAP_DOWN: f32 = 0.5;

#[derive(Component)]
pub struct DungeonFlat;

pub struct ActiveDungeon {
    pub root: Entity,
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

/// Carica e instanzia un dungeon, teletrasportando il giocatore sull'Enter Marker
pub fn spawn_and_enter_dungeon(
    commands: &mut Commands,
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
    let Ok(maps_bsa) = BsaArchive::parse(&data.maps_buf) else { return false };
    let Ok(blocks_bsa) = BsaArchive::parse(&data.blocks_buf) else { return false };
    let Ok(arch_bsa) = BsaArchive::parse(&data.arch3d_buf) else { return false };

    let dungeon_data = DungeonData::read_from_maps(&maps_bsa, loc.region_index, loc.location_id)
    .or_else(|| {
        (0..65535u32).find_map(|cand| DungeonData::read_from_maps(&maps_bsa, loc.region_index, cand))
    });

    let Some(dungeon_data) = dungeon_data else { return false };

    println!("Caricamento dungeon di {} ({} blocchi)...", loc.name, dungeon_data.blocks.len());
    let Some(built) = build_dungeon(&dungeon_data, &blocks_bsa, &arch_bsa) else {
        println!("Costruzione dungeon fallita.");
        return false;
    };

    let origin_world = Vec3::new(tf.translation.x, DUNGEON_Y, tf.translation.z);
    let climate = climate_of(data, mx, my);

    let root = commands.spawn((
        Transform::from_translation(origin_world),
                               Visibility::default(),
    )).with_children(|parent| {
        // Geometria corridoi e stanze
        for sm in &built.mesh.submeshes {
            if sm.indices.is_empty() { continue; }
            let size = wall_textures.original_size(sm.texture_archive, sm.texture_record, data);
            let handle = wall_textures.material(
                sm.texture_archive,
                sm.texture_record,
                climate,
                data,
                mods,
                images,
                materials,
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

        // Torce del dungeon
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

        // Luce torcia del giocatore / spawn
        parent.spawn((
            PointLight {
                color: Color::srgb(1.0, 0.80, 0.45),
                      intensity: 90_000.0,
                      range: 18.0,
                      shadows_enabled: false,
                      ..default()
            },
            Transform::from_xyz(built.spawn_pos[0], built.spawn_pos[1] + 2.0, built.spawn_pos[2]),
        ));

        // Flat e decorazioni
        for (f_pos, archive, record) in &built.flats {
            let Some((material, w_px, h_px)) = wall_textures.flat_material(
                *archive,
                *record,
                data,
                mods,
                images,
                materials,
            ) else { continue };

            let (w, h) = (w_px * DF_UNIT, h_px * DF_UNIT);
            parent.spawn((
                Mesh3d(meshes.add(Rectangle::new(w, h))),
                          MeshMaterial3d(material),
                          Transform::from_xyz(f_pos[0], f_pos[1] + h * 0.5, f_pos[2]),
                          DungeonFlat,
            ));
        }
    }).id();

    let spawn = built.spawn_pos;
    let floor_y = built.floor_at(spawn[0], spawn[2], spawn[1], 2.0).unwrap_or(spawn[1]);

    state.active = Some(ActiveDungeon {
        root,
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

/// Sistema eseguito al primo frame per svegliare il giocatore direttamente dentro Privateer's Hold
#[allow(clippy::too_many_arguments)]
fn check_initial_dungeon_spawn(
    mut commands: Commands,
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
    if *started {
        return;
    }
    *started = true;

    let should_start_in_dungeon = start_config.map_or(true, |c| c.start_in_dungeon);
    if !should_start_in_dungeon {
        return;
    }

    let Ok((mut p, mut tf)) = player.get_single_mut() else { return };
    let (mx, my) = map_pixel_of(&origin, tf.translation);

    // Cerca Privateer's Hold (o la location attuale)
    let target_loc = data.locations.get(&(mx, my))
    .or_else(|| {
        data.locations.values().find(|l| l.name.to_ascii_uppercase().contains("PRIVATEER"))
    });

    let Some(loc) = target_loc else { return };

    if spawn_and_enter_dungeon(
        &mut commands,
        &data,
        &origin,
        &mut state,
        &mut clear,
        &mut meshes,
        &mut images,
        &mut materials,
        &mut wall_textures,
        &mut mods,
        &mut p,
        &mut tf,
        loc,
        mx,
        my,
    ) {
        println!("\n=======================================================");
        println!(" Benvenuto in Daggerfall: ti risvegli a Privateer's Hold!");
        println!(" Trova l'uscita o premi Backspace per uscire all'aperto.");
        println!("=======================================================\n");
    }
}

#[allow(clippy::too_many_arguments)]
pub fn dungeon_enter_exit(
    mut commands: Commands,
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
) {
    let leave = keys.just_pressed(KeyCode::Backspace);
    let act = keys.just_pressed(KeyCode::KeyE);
    let fast_enter = keys.just_pressed(KeyCode::KeyU);
    if !leave && !act && !fast_enter {
        return;
    }
    let Ok((mut p, mut tf)) = player.get_single_mut() else { return };

    // USCITA DAL DUNGEON
    if let Some(active) = &state.active {
        let local = tf.translation - active.origin;
        let d_exit = (local.x - active.dungeon.exit_pos[0]).hypot(local.z - active.dungeon.exit_pos[2]);

        if leave || (act && d_exit < DUNGEON_EXIT_DISTANCE) {
            let active = state.active.take().unwrap();
            let (de, dn) = (origin.east - active.saved_origin.0, origin.north - active.saved_origin.1);
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
            println!("Sei uscito dal dungeon.");
        }
        keys.clear_just_pressed(KeyCode::KeyE);
        keys.clear_just_pressed(KeyCode::Backspace);
        return;
    }

    // INGRESSO DALL'ESTERNO
    let (mx, my) = map_pixel_of(&origin, tf.translation);
    let Some(loc) = data.locations.get(&(mx, my)) else { return };

    if spawn_and_enter_dungeon(
        &mut commands,
        &data,
        &origin,
        &mut state,
        &mut clear,
        &mut meshes,
        &mut images,
        &mut materials,
        &mut wall_textures,
        &mut mods,
        &mut p,
        &mut tf,
        loc,
        mx,
        my,
    ) {
        keys.clear_just_pressed(KeyCode::KeyE);
    }
}

pub fn dungeon_move(
    dialog: Res<DialogState>,
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<DungeonState>,
    mut player: Query<(&mut Player, &mut Transform)>,
) {
    if dialog.open { return; }
    let Some(active) = state.active.as_ref() else { return };
    let Ok((mut p, mut tf)) = player.get_single_mut() else { return };

    let dt = time.delta_secs().min(0.05);
    let (dungeon, o) = (&active.dungeon, active.origin);

    let forward = Vec3::new(-p.yaw.sin(), 0.0, -p.yaw.cos());
    let right = Vec3::new(p.yaw.cos(), 0.0, -p.yaw.sin());
    let axis = |pos: KeyCode, neg: KeyCode| {
        keys.pressed(pos) as i32 as f32 - keys.pressed(neg) as i32 as f32
    };
    let wish = (forward * axis(KeyCode::KeyW, KeyCode::KeyS)
    + right * axis(KeyCode::KeyD, KeyCode::KeyA))
    .normalize_or_zero();

    let running = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    let speed = WALK_SPEED * if running { RUN_MULTIPLIER } else { 1.0 };
    let total = speed * dt;
    let steps = (total / 0.1).ceil().max(1.0) as i32;
    let step = wish * (total / steps as f32);

    let mut pos = tf.translation;
    if step != Vec3::ZERO {
        for _ in 0..steps {
            let feet = p.feet_y - o.y;
            for cand in [step, Vec3::new(step.x, 0.0, 0.0), Vec3::new(0.0, 0.0, step.z)] {
                if cand == Vec3::ZERO { continue; }
                let n = pos + cand;
                if !dungeon.blocked(n.x - o.x, n.z - o.z, feet, RADIUS) {
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
        pos = Vec3::new(o.x + dungeon.spawn_pos[0], 0.0, o.z + dungeon.spawn_pos[2]);
        p.feet_y = o.y + dungeon.spawn_pos[1];
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
