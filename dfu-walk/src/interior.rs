//! Interni delle taverne: con E vicino a una taverna si entra, con E vicino alla porta
//! (o Backspace ovunque) si esce. L'interno e' costruito con i modelli originali del
//! secondo BlockData del sotto-record e messo 6000 m sopra il punto d'ingresso, cosi' non si
//! sovrappone a nulla e il resto del mondo resta caricato. Dentro si cammina sui pavimenti dei
//! modelli (scale comprese) e le pareti bloccano.
use crate::mod_textures::ModTextures;
use crate::player::{Player, EYE_HEIGHT};
use crate::tiles::climate_of;
use crate::wall_textures::WallTextures;
use crate::world::*;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use crate::data::blocks::RmbBlock;
use crate::data::bsa::BsaArchive;
use crate::data::building_gen::DF_UNIT;
use crate::data::city_map::{collect_city_map, MapBuilding, PoiKind};
use crate::data::interior::{build_interior, InteriorScene};
use crate::data::mesh_builder::{Submesh, WorldMesh};
use crate::data::terrain_mesh::MAP_PIXEL_SIZE;
use std::collections::HashMap;

/// Quota (m) a cui si costruisce l'interno.
const INTERIOR_Y: f32 = 6000.0;

/// Distanza massima dal perimetro della taverna per poter entrare.
const ENTER_DISTANCE: f32 = 3.5;

/// Distanza dalla porta interna entro cui E fa uscire.
const EXIT_DISTANCE: f32 = 2.5;

const WALK_SPEED: f32 = 4.0;
const RUN_MULTIPLIER: f32 = 2.0;
const GRAVITY: f32 = 20.0;
const JUMP_SPEED: f32 = 5.5;
const RADIUS: f32 = 0.3;

/// Salita massima di un gradino e discesa a cui si resta incollati al pavimento.
const STEP_UP: f32 = 0.6;
const SNAP_DOWN: f32 = 0.5;

/// Distanza massima dalla porta di una stanza per aprirla/chiuderla con E.
const DOOR_REACH: f32 = 2.2;
const DOOR_OPEN_SECONDS: f32 = 1.5;

/// Verso di apertura: 1.0 come in Daggerfall Unity (angolo -90). Se le porte si aprono verso
/// il muro invece che verso la stanza, metti -1.0.
const DOOR_OPEN_SIGN: f32 = 1.0;

struct Active {
    root: Entity,
    /// posizione nel mondo del punto (0,0,0) dell'interno
    origin: Vec3,
    scene: InteriorScene,
    saved_translation: Vec3,
    saved_feet_y: f32,
    saved_yaw: f32,
    saved_origin: (i32, i32),
}

#[derive(Resource, Default)]
pub struct InteriorState {
    active: Option<Active>,
}

impl InteriorState {
    pub fn is_inside(&self) -> bool {
        self.active.is_some()
    }
}

/// Condizione per i sistemi che valgono solo all'aperto (il cielo).
pub fn outside(state: Res<InteriorState>) -> bool {
    !state.is_inside()
}

#[derive(Component)]
pub struct FlatBillboard;

/// Porta di una stanza: ruota attorno alla sua origine tra `closed_y` e `closed_y - 90 gradi`.
#[derive(Component)]
pub struct RoomDoor {
    index: usize,
    closed_y: f32,
    progress: f32,
}

fn distance_to_building(b: &MapBuilding, px: f32, pz: f32) -> f32 {
    // inverso della rotazione usata da CityBuilder: x' = x c + z s, z' = -x s + z c
    let (s, c) = b.rot_deg.to_radians().sin_cos();
    let (dx, dz) = (px - b.cx, pz - b.cz);
    let lx = dx * c - dz * s;
    let lz = dx * s + dz * c;
    let ox = (lx.abs() - b.half_w).max(0.0);
    let oz = (lz.abs() - b.half_d).max(0.0);
    ox.hypot(oz)
}

/// Mesh Bevy di una sola texture: tiene solo i vertici usati dal submesh.
/// Le UV di ARCH3D arrivano in pixel (gia' divise per 16): si dividono per le dimensioni
/// della texture originale per portarle in 0..1.
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

/// Diagnostica: metri di parete per ogni ripetizione della texture (mediana sugli spigoli).
/// Per una texture da 64 px ci si aspetta circa 1.6 m.
fn debug_uv_scale(mesh: &WorldMesh, sm: &Submesh, size: (f32, f32)) {
    let mut ratios = Vec::new();
    for t in sm.indices.chunks_exact(3) {
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let (ia, ib) = (t[a] as usize, t[b] as usize);
            let (pa, pb) = (mesh.vertices[ia], mesh.vertices[ib]);
            let d = ((pa[0] - pb[0]).powi(2) + (pa[1] - pb[1]).powi(2) + (pa[2] - pb[2]).powi(2))
            .sqrt();
            let (ua, ub) = (mesh.uvs[ia], mesh.uvs[ib]);
            let (du, dv) = ((ua[0] - ub[0]) / size.0, (ua[1] - ub[1]) / size.1);
            let duv = (du * du + dv * dv).sqrt();
            if d > 0.05 && duv > 1e-6 {
                ratios.push(d / duv);
            }
        }
    }
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if let Some(m) = ratios.get(ratios.len() / 2) {
        println!(
            "UV dbg {}/{} size {:?}: {} spigoli, metri per ripetizione (mediana) = {:.4}",
                 sm.texture_archive,
                 sm.texture_record,
                 size,
                 ratios.len(),
                 m
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn interior_enter_exit(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    data: Res<GameData>,
    origin: Res<Origin>,
    mut state: ResMut<InteriorState>,
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
    if !leave && !act {
        return;
    }
    let Ok((mut p, mut tf)) = player.get_single_mut() else {
        return;
    };
    // ---- uscita ----
    if state.active.is_some() {
        let near_exit = {
            let a = state.active.as_ref().unwrap();
            let local = tf.translation - a.origin;
            a.scene
            .exit
            .map_or(false, |e| (local.x - e[0]).hypot(local.z - e[2]) < EXIT_DISTANCE)
        };
        if leave || near_exit {
            let a = state.active.take().unwrap();
            // se nel frattempo e' cambiata l'origine del mondo, si corregge come `rebase_origin`
            let (de, dn) = (origin.east - a.saved_origin.0, origin.north - a.saved_origin.1);
            let mut pos = a.saved_translation;
            pos.x -= de as f32 * MAP_PIXEL_SIZE;
            pos.z += dn as f32 * MAP_PIXEL_SIZE;
            p.feet_y = a.saved_feet_y;
            p.vel_y = 0.0;
            p.on_ground = true;
            p.yaw = a.saved_yaw + std::f32::consts::PI; // si esce guardando la strada
            tf.translation = pos;
            commands.entity(a.root).despawn_recursive();
            clear.0 = Color::srgb(0.55, 0.72, 0.92);
            println!("Sei uscito dalla taverna.");
        }
        return;
    }
    // ---- ingresso ----
    if !act {
        return;
    }
    let (mx, my) = map_pixel_of(&origin, tf.translation);
    let Some(loc) = data.locations.get(&(mx, my)) else {
        println!("Qui non c'e' nessuna citta'.");
        return;
    };
    let (Ok(blocks), Ok(arch)) = (
        BsaArchive::parse(&data.blocks_buf),
                                  BsaArchive::parse(&data.arch3d_buf),
    ) else {
        println!("Interni: impossibile leggere BLOCKS.BSA o ARCH3D.BSA.");
        return;
    };
    let map = collect_city_map(loc, &blocks, &arch);
    let tile = tile_translation(&origin, mx, my);
    let (px, pz) = (tf.translation.x - tile.x, tf.translation.z - tile.z);
    let mut best: Option<(f32, (usize, usize, u8))> = None;
    let (mut taverns, mut nearest) = (0usize, f32::MAX);
    for b in &map.buildings {
        if b.kind != Some(PoiKind::Tavern) {
            continue;
        }
        let Some(src) = b.source else { continue };
        let d = distance_to_building(b, px, pz);
        taverns += 1;
        nearest = nearest.min(d);
        if d <= ENTER_DISTANCE && best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, src));
        }
    }
    let Some((_, (bx, by, sub))) = best else {
        if taverns == 0 {
            println!(
                "Interni: nessuna taverna trovata in {} (map pixel {mx},{my}).",
                     loc.name
            );
        } else {
            println!(
                "Interni: {} taverne in {}, la piu' vicina a {:.1} m (serve entro {ENTER_DISTANCE} m).",
                     taverns, loc.name, nearest
            );
        }
        return;
    };
    let Some(name) = loc.block_names.get(by * loc.width + bx) else {
        println!("Interni: blocco ({bx},{by}) fuori dalla location.");
        return;
    };
    let Some(raw) = blocks.get_by_name(name) else {
        println!("Interni: il blocco {name} non esiste in BLOCKS.BSA.");
        return;
    };
    let Ok(rmb) = RmbBlock::parse(raw) else {
        println!("Interni: il blocco {name} non si legge.");
        return;
    };
    let Some(scene) = build_interior(&rmb, sub as usize, &arch) else {
        println!("L'interno di {name} (sotto-record {sub}) non ha modelli 3D.");
        return;
    };
    let origin_world = Vec3::new(tf.translation.x, INTERIOR_Y, tf.translation.z);
    let climate = climate_of(&data, mx, my);
    let root = commands
    .spawn((Transform::from_translation(origin_world), Visibility::default()))
    .with_children(|parent| {
        for sm in &scene.mesh.submeshes {
            if sm.indices.is_empty() {
                continue;
            }
            let size = wall_textures.original_size(sm.texture_archive, sm.texture_record, &data);
            debug_uv_scale(&scene.mesh, sm, size);
            let handle = wall_textures.material(
                sm.texture_archive,
                sm.texture_record,
                climate,
                &data,
                &mut mods,
                &mut images,
                &mut materials,
            );
            // il verso delle facce dei modelli non e' garantito: si disegnano da entrambi i lati
            if let Some(m) = materials.get_mut(&handle) {
                m.double_sided = true;
                m.cull_mode = None;
            }
            parent.spawn((
                Mesh3d(meshes.add(submesh_to_bevy(&scene.mesh, sm, size))),
                          MeshMaterial3d(handle),
                          Transform::IDENTITY,
            ));
        }
        for (i, d) in scene.doors.iter().enumerate() {
            // la porta ruota attorno al proprio punto d'origine (la cerniera)
            let closed = -d.yaw_deg.to_radians();
            parent
            .spawn((
                Transform {
                    translation: Vec3::from(d.pos),
                    rotation: Quat::from_rotation_y(closed),
                    ..default()
                },
                Visibility::default(),
                    RoomDoor {
                        index: i,
                        closed_y: closed,
                        progress: 0.0,
                    },
            ))
            .with_children(|door| {
                for sm in &d.mesh.submeshes {
                    if sm.indices.is_empty() {
                        continue;
                    }
                    let size = wall_textures.original_size(
                        sm.texture_archive,
                        sm.texture_record,
                        &data,
                    );
                    let handle = wall_textures.material(
                        sm.texture_archive,
                        sm.texture_record,
                        climate,
                        &data,
                        &mut mods,
                        &mut images,
                        &mut materials,
                    );
                    if let Some(m) = materials.get_mut(&handle) {
                        m.double_sided = true;
                        m.cull_mode = None;
                    }
                    door.spawn((
                        Mesh3d(meshes.add(submesh_to_bevy(&d.mesh, sm, size))),
                                MeshMaterial3d(handle),
                                Transform::IDENTITY,
                    ));
                }
            });
        }
        // Flat (arredi statici: tavoli, barili, quadri, luci)
        for f in &scene.flats {
            let Some((material, w_px, h_px)) = wall_textures.flat_material(
                f.archive,
                f.record,
                &data,
                &mut images,
                &mut materials,
            ) else {
                continue;
            };
            let (w, h) = (w_px * DF_UNIT, h_px * DF_UNIT);
            parent.spawn((
                Mesh3d(meshes.add(Rectangle::new(w, h))),
                          MeshMaterial3d(material),
                          Transform::from_xyz(f.pos[0], f.pos[1] + h * 0.5, f.pos[2]),
                          FlatBillboard,
            ));
        }
        // AGGIUNTA: People (NPC - oste, clienti, mercanti) come billboard
        for f in &scene.people {
            let Some((material, w_px, h_px)) = wall_textures.flat_material(
                f.archive,
                f.record,
                &data,
                &mut images,
                &mut materials,
            ) else {
                continue;
            };
            let (w, h) = (w_px * DF_UNIT, h_px * DF_UNIT);
            parent.spawn((
                Mesh3d(meshes.add(Rectangle::new(w, h))),
                          MeshMaterial3d(material),
                          Transform::from_xyz(f.pos[0], f.pos[1] + h * 0.5, f.pos[2]),
                          FlatBillboard,
            ));
        }
        // luce calda al centro della sala
        let cx = (scene.bounds_min[0] + scene.bounds_max[0]) * 0.5;
        let cz = (scene.bounds_min[2] + scene.bounds_max[2]) * 0.5;
        parent.spawn((
            PointLight {
                color: Color::srgb(1.0, 0.86, 0.62),
                      intensity: 600_000.0,
                      range: 40.0,
                      shadows_enabled: false,
                      ..default()
            },
            Transform::from_xyz(cx, scene.spawn[1] + 2.3, cz),
        ));
    })
    .id();
    // il giocatore compare davanti alla porta, rivolto verso la sala
    let spawn = Vec3::from(scene.spawn);
    let cx = (scene.bounds_min[0] + scene.bounds_max[0]) * 0.5;
    let cz = (scene.bounds_min[2] + scene.bounds_max[2]) * 0.5;
    let (dx, dz) = (cx - spawn.x, cz - spawn.z);
    let saved = Active {
        root,
        origin: origin_world,
        saved_translation: tf.translation,
        saved_feet_y: p.feet_y,
        saved_yaw: p.yaw,
        saved_origin: (origin.east, origin.north),
        scene,
    };
    p.feet_y = origin_world.y + spawn.y;
    p.vel_y = 0.0;
    p.on_ground = true;
    p.fly = false;
    if dx.hypot(dz) > 0.5 {
        p.yaw = (-dx).atan2(-dz);
    }
    tf.translation = Vec3::new(
        origin_world.x + spawn.x,
        p.feet_y + EYE_HEIGHT,
        origin_world.z + spawn.z,
    );
    clear.0 = Color::BLACK;
    state.active = Some(saved);
    println!("Sei entrato in {name} (sotto-record {sub}). E vicino alla porta o Backspace per uscire.");
}

pub fn interior_move(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<InteriorState>,
    mut player: Query<(&mut Player, &mut Transform)>,
) {
    let Some(active) = state.active.as_ref() else {
        return;
    };
    let Ok((mut p, mut tf)) = player.get_single_mut() else {
        return;
    };
    let dt = time.delta_secs().min(0.05);
    let (scene, o) = (&active.scene, active.origin);
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
    // passi da al massimo 10 cm, cosi' non si attraversano le pareti sottili
    let total = speed * dt;
    let steps = (total / 0.1).ceil().max(1.0) as i32;
    let step = wish * (total / steps as f32);
    let mut pos = tf.translation;
    if step != Vec3::ZERO {
        for _ in 0..steps {
            let feet = p.feet_y - o.y;
            // prima il passo intero, poi scorrimento lungo un solo asse
            for cand in [
                step,
                Vec3::new(step.x, 0.0, 0.0),
                Vec3::new(0.0, 0.0, step.z),
            ] {
                if cand == Vec3::ZERO {
                    continue;
                }
                let n = pos + cand;
                if !scene.blocked(n.x - o.x, n.z - o.z, feet, RADIUS) {
                    pos = n;
                    break;
                }
            }
        }
    }
    // verticale: pavimento dei modelli, gravita' e salto
    let (lx, lz) = (pos.x - o.x, pos.z - o.z);
    let feet_local = p.feet_y - o.y;
    let floor = scene.floor_at(lx, lz, feet_local, STEP_UP);
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
    // caduto nel vuoto: si riparte dall'ingresso
    if p.feet_y - o.y < scene.bounds_min[1] - 5.0 {
        pos = Vec3::new(o.x + scene.spawn[0], 0.0, o.z + scene.spawn[2]);
        p.feet_y = o.y + scene.spawn[1];
        p.vel_y = 0.0;
        p.on_ground = true;
    }
    pos.y = p.feet_y + EYE_HEIGHT;
    tf.translation = pos;
}

/// Gli sprite (tavoli, persone...) guardano sempre la camera.
pub fn face_flats(
    player: Query<&Player>,
    mut flats: Query<&mut Transform, (With<FlatBillboard>, Without<crate::tiles::NpcWalker>)>,
) {
    let Ok(p) = player.get_single() else { return };
    let rot = Quat::from_rotation_y(p.yaw);
    for mut tf in &mut flats {
        tf.rotation = rot;
    }
}

/// E vicino a una porta di stanza: la apre o la chiude.
pub fn interior_toggle_doors(
    keys: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<InteriorState>,
    player: Query<(&Player, &Transform)>,
) {
    if !keys.just_pressed(KeyCode::KeyE) {
        return;
    }
    let Some(active) = state.active.as_mut() else {
        return;
    };
    let Ok((p, tf)) = player.get_single() else { return };
    let o = active.origin;
    let (lx, lz) = (tf.translation.x - o.x, tf.translation.z - o.z);
    let feet = p.feet_y - o.y;
    if let Some(i) = active.scene.toggle_nearest_door(lx, lz, feet, DOOR_REACH) {
        let open = active.scene.door_open[i];
        println!("Porta {i} {}.", if open { "aperta" } else { "chiusa" });
    }
}

/// Anima le porte verso lo stato (aperta/chiusa) indicato dalla scena.
pub fn animate_doors(
    time: Res<Time>,
    state: Res<InteriorState>,
    mut doors: Query<(&mut RoomDoor, &mut Transform)>,
) {
    let Some(active) = state.active.as_ref() else {
        return;
    };
    let step = time.delta_secs() / DOOR_OPEN_SECONDS;
    for (mut d, mut tf) in &mut doors {
        let open = active.scene.door_open.get(d.index).copied().unwrap_or(false);
        d.progress = if open {
            (d.progress + step).min(1.0)
        } else {
            (d.progress - step).max(0.0)
        };
        let eased = d.progress * d.progress * (3.0 - 2.0 * d.progress);
        tf.rotation =
        Quat::from_rotation_y(d.closed_y - DOOR_OPEN_SIGN * eased * std::f32::consts::FRAC_PI_2);
    }
}
