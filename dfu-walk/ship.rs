//! Navi medievali controllabili dal giocatore.
//!
//! ## Flusso
//!
//! 1. All'avvio (`PostStartup`) cerca acqua a sud del map pixel di partenza
//!    e spawna una nave.
//! 2. Il giocatore cammina normalmente. Quando è entro `BOARDING_DISTANCE`
//!    dalla nave e preme **E**, entra in modalità nave: è al timone (a
//!    poppa), W/S = velocità, A/D = rotta (il mouse guarda liberamente).
//! 3. **E** di nuovo → esce dalla modalità nave **ovunque**, anche in mare:
//!    torna player normale, piazzato sul ponte. La nave si ferma.
//! 4. Da player normale si cammina sul ponte (`deck_height_at` è usata da
//!    `player.rs` come superficie calpestabile), si può scendere in acqua o
//!    a terra camminando oltre il bordo, e si può riprendere il timone con E.
//!
//! `player_move` fa early return finché `ShipState::player_aboard` è
//! valorizzato: in modalità nave il movimento è gestito da `ship_pilot`.
//!
//! ## Orientamento
//!
//! La prua è verso **-Z locale** (la direzione in cui la nave avanza), la
//! poppa verso +Z.
//!
//! ## Acqua e terra
//!
//! L'acqua è identificata da quota `< BEACH_ELEVATION - 1.0`. Sotto quella
//! soglia la nave naviga, sopra è terraferma (la nave si blocca).
//!
//! ## Rebase dell'origine
//!
//! `tiles::rebase_origin` sposta player, follower e tile quando l'origine
//! mobile cambia. Le navi vanno spostate **anche loro** (da fare in
//! `tiles.rs`).

use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;

use crate::data::heightmap::{BEACH_ELEVATION, OCEAN_ELEVATION};
use crate::data::terrain_mesh::{terrain_height, CELL_SIZE};
use crate::data::generate_default_samples;
use crate::dialog::DialogState;
use crate::dungeon::DungeonState;
use crate::interior::InteriorState;
use crate::player::{Player, EYE_HEIGHT};
use crate::world::{in_map, tile_translation, GameData, HeightField, Origin};
use crate::StartInfo;

// ---------------------------------------------------------------------------
// Costanti di gameplay
// ---------------------------------------------------------------------------

/// Offset verticale dell'origine della nave rispetto alla superficie
/// dell'acqua. Negativo = lo scafo affonda (5 m: 4 m in più di prima).
const SHIP_Y_OFFSET: f32 = -5.0;
/// Fattore di scala della nave (3× = ~45 m di lunghezza).
const SHIP_SCALE: f32 = 3.0;
/// Quota locale (non scalata) del ponte sopra l'origine.
const DECK_Y: f32 = 2.7;
/// Altezza del ponte sopra l'origine della nave, in metri mondo.
const DECK_LOCAL_Y: f32 = DECK_Y * SHIP_SCALE;
/// Estremi dello scafo lungo Z locale (la prua è verso -Z).
const Z_STERN: f32 = 7.0;
const Z_BOW: f32 = -8.0;
/// Mezza larghezza massima dello scafo (locale).
const HULL_BEAM: f32 = 2.6;
/// Posizione (Z locale) dove sta in piedi il timoniere.
const HELM_STAND_Z: f32 = 2.4;
/// Distanza entro cui il player può entrare in modalità nave (E).
const BOARDING_DISTANCE: f32 = 8.0 * SHIP_SCALE;
/// Velocità massima avanti (m/s).
const MAX_SPEED_FWD: f32 = 12.0;
/// Velocità massima retromarcia (m/s).
const MAX_SPEED_BWD: f32 = 4.0;
/// Accelerazione (m/s²).
const ACCELERATION: f32 = 3.0;
/// Attrito quando i tasti sono rilasciati (m/s²).
const FRICTION: f32 = 1.5;
/// Rateo di virata massimo (rad/s).
const YAW_RATE: f32 = 1.2;
/// Passo di scansione griglia per trovare acqua all'avvio.
const SCAN_STEP: usize = 4;

// ---------------------------------------------------------------------------
// Componenti / risorse
// ---------------------------------------------------------------------------

/// Marcatore della nave. Il `Transform` dell'entità è in coordinate mondo
/// (posizione della chiglia, y = superficie acqua + SHIP_Y_OFFSET).
#[derive(Component)]
pub struct Ship;

/// Stato di movimento della nave. `speed` è in m/s (positivo avanti,
/// negativo indietro).
#[derive(Component, Default)]
pub struct ShipMotion {
    pub speed: f32,
}

/// Player in modalità nave (al timone) e su quale nave.
#[derive(Resource, Default)]
pub struct ShipState {
    /// Nave pilotata dal player. None = player normale.
    pub player_aboard: Option<Entity>,
}

pub struct ShipPlugin;

impl Plugin for ShipPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShipState>()
            .add_systems(PostStartup, spawn_initial_ship)
            .add_systems(
                Update,
                // Ordine:
                //   1. boarding (E sali/scendi)  → aggiorna ShipState
                //   2. player_move               → early return se a bordo
                //   3. ship_pilot                → muove nave e player
                (ship_boarding, ship_pilot)
                    .chain()
                    .before(crate::player::player_move),
            );
    }
}

// ---------------------------------------------------------------------------
// Spawn iniziale
// ---------------------------------------------------------------------------

/// Al primo frame dopo lo Startup cerca l'acqua più vicina al giocatore
/// scansionando i map pixel a sud della partenza (dy 0..=4, dx -2..=2;
/// `my` cresce verso sud) e spawna lì una nave.
///
/// Ogni pixel viene scansionato con passo `SCAN_STEP` (4 celle ≈ 25.6 m);
/// tra tutte le celle d'acqua trovate vince quella più vicina al centro
/// del tile di partenza.
fn spawn_initial_ship(
    mut commands: Commands,
    data: Res<GameData>,
    start: Res<StartInfo>,
    origin: Res<Origin>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    *done = true;

    let (sx, sy) = (start.mx, start.my);
    let start_tile = tile_translation(&origin, sx, sy);
    let player_xz = Vec2::new(
        start_tile.x + 64.0 * CELL_SIZE,
        start_tile.z - 64.0 * CELL_SIZE,
    );

    // Cerca la cella d'acqua più vicina al giocatore, scansionando i pixel
    // a sud della partenza (`my` cresce verso sud).
    let mut best: Option<(f32, Vec3, (i32, i32))> = None;
    for dy in 0..=4 {
        for dx in -2..=2 {
            let (mx, my) = (sx + dx, sy + dy);
            if !in_map(mx, my) {
                continue;
            }
            let Ok(samples) = generate_default_samples(&data.woods, mx, my) else {
                continue;
            };
            let tile_org = tile_translation(&origin, mx, my);
            for gy in (SCAN_STEP..128 - SCAN_STEP).step_by(SCAN_STEP) {
                for gx in (SCAN_STEP..128 - SCAN_STEP).step_by(SCAN_STEP) {
                    let h = terrain_height(&samples, gx as f32, gy as f32);
                    if h < BEACH_ELEVATION - 1.0 {
                        let x = tile_org.x + gx as f32 * CELL_SIZE;
                        let z = tile_org.z - gy as f32 * CELL_SIZE;
                        let d = Vec2::new(x, z).distance(player_xz);
                        if best.map_or(true, |(bd, _, _)| d < bd) {
                            best = Some((d, Vec3::new(x, h.max(OCEAN_ELEVATION) + SHIP_Y_OFFSET, z), (mx, my)));
                        }
                    }
                }
            }
        }
    }

    match best {
        Some((d, pos, (mx, my))) => {
            spawn_ship(&mut commands, &mut meshes, &mut materials, pos, 0.0);
            info!(
                "Nave pronta nel map pixel ({mx}, {my}) a ({:.0}, {:.0}), {d:.0} m dal giocatore — premi E per prendere il timone",
                pos.x, pos.z
            );
        }
        None => info!(
            "Nessuna acqua entro 4 pixel a sud di ({sx}, {sy}): nessuna nave (avvia con --pixel vicino a una costa)"
        ),
    }
}

/// Mezza larghezza dello scafo a `t` in [0, 1] (0 = poppa, 1 = prua).
fn beam(t: f32) -> f32 {
    if t < 0.4 {
        HULL_BEAM * (0.8 + 0.2 * (t / 0.4 * std::f32::consts::FRAC_PI_2).sin())
    } else {
        let u = (t - 0.4) / 0.6;
        HULL_BEAM * (1.0 - 0.94 * u.powf(1.7))
    }
}

/// Quota del ponte (mondo) sotto `xz`, se il punto è sopra lo scafo di una
/// nave. Usata da `player.rs` per far camminare il player sul ponte.
pub fn deck_height_at(ship_tf: &Transform, xz: Vec2) -> Option<f32> {
    let rel = Vec3::new(xz.x - ship_tf.translation.x, 0.0, xz.y - ship_tf.translation.z);
    let local = ship_tf.rotation.inverse() * rel / SHIP_SCALE;
    if local.z > Z_STERN || local.z < Z_BOW {
        return None;
    }
    let t = ((Z_STERN - local.z) / (Z_STERN - Z_BOW)).clamp(0.0, 1.0);
    if local.x.abs() > beam(t) - 0.15 {
        return None;
    }
    Some(ship_tf.translation.y + DECK_LOCAL_Y)
}

fn build_mesh(pos: Vec<[f32; 3]>, idx: Vec<u32>) -> Mesh {
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    m.insert_attribute(Mesh::ATTRIBUTE_POSITION, pos);
    m.insert_indices(Indices::U32(idx));
    m.duplicate_vertices();
    m.compute_flat_normals();
    m
}

/// Scafo (carena rastremata, prua a punta) e ponte, come due mesh.
fn hull_meshes() -> (Mesh, Mesh) {
    const N: usize = 16;
    let mut rings: Vec<[[f32; 3]; 7]> = Vec::with_capacity(N + 1);
    for i in 0..=N {
        let t = i as f32 / N as f32;
        let z = Z_STERN + (Z_BOW - Z_STERN) * t;
        let w = beam(t);
        let top = DECK_Y;
        // La chiglia risale verso prua e poppa.
        let bot = 1.6 * (2.0 * t - 1.0).abs().powf(2.2);
        let h = top - bot;
        rings.push([
            [-w, top, z],
            [-0.97 * w, bot + 0.62 * h, z],
            [-0.72 * w, bot + 0.20 * h, z],
            [0.0, bot, z],
            [0.72 * w, bot + 0.20 * h, z],
            [0.97 * w, bot + 0.62 * h, z],
            [w, top, z],
        ]);
    }

    // Fiancate e fondo.
    let mut pos: Vec<[f32; 3]> = Vec::new();
    let mut idx: Vec<u32> = Vec::new();
    for r in &rings {
        pos.extend_from_slice(r);
    }
    for i in 0..N {
        for j in 0..6 {
            let a = (i * 7 + j) as u32;
            let b = a + 1;
            let c = ((i + 1) * 7 + j) as u32;
            let d = c + 1;
            idx.extend_from_slice(&[a, b, c, b, d, c]);
        }
    }
    // Specchio di poppa (ventaglio dal centro del primo anello).
    let centre = pos.len() as u32;
    pos.push([0.0, DECK_Y * 0.6, Z_STERN]);
    for j in 0..6u32 {
        idx.extend_from_slice(&[centre, j, j + 1]);
    }
    idx.extend_from_slice(&[centre, 6, 0]);
    let hull = build_mesh(pos, idx);

    // Ponte: striscia tra i due bordi superiori.
    let mut dpos: Vec<[f32; 3]> = Vec::new();
    let mut didx: Vec<u32> = Vec::new();
    for r in &rings {
        dpos.push(r[0]);
        dpos.push(r[6]);
    }
    for i in 0..N as u32 {
        let (l0, r0, l1, r1) = (i * 2, i * 2 + 1, i * 2 + 2, i * 2 + 3);
        didx.extend_from_slice(&[l0, r0, l1, r0, r1, l1]);
    }
    let deck = build_mesh(dpos, didx);
    (hull, deck)
}

/// Costruisce la nave: scafo rastremato con prua a punta (verso -Z), ponte,
/// cabina di poppa con timone, bompresso, due alberi con vele e bandiera.
/// La radice ha `Ship`, `ShipMotion` e `Transform` (scalato di `SHIP_SCALE`).
fn spawn_ship(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    pos: Vec3,
    yaw: f32,
) -> Entity {
    let solid = |c: Color| StandardMaterial {
        base_color: c,
        perceptual_roughness: 0.9,
        double_sided: true,
        cull_mode: None,
        ..default()
    };
    let wood = materials.add(solid(Color::srgb(0.32, 0.20, 0.12)));
    let deck_wood = materials.add(solid(Color::srgb(0.52, 0.38, 0.24)));
    let dark_wood = materials.add(solid(Color::srgb(0.16, 0.10, 0.06)));
    let canvas = materials.add(solid(Color::srgb(0.92, 0.88, 0.78)));
    let banner = materials.add(solid(Color::srgb(0.55, 0.15, 0.15)));

    let (hull_mesh, deck_mesh) = hull_meshes();
    let hull = meshes.add(hull_mesh);
    let deck = meshes.add(deck_mesh);
    let cabin = meshes.add(Cuboid::new(3.9, 2.0, 3.2));
    let cabin_roof = meshes.add(Cuboid::new(4.3, 0.15, 3.6));
    let helm_post = meshes.add(Cuboid::new(0.15, 0.9, 0.15));
    let helm_wheel = meshes.add(Cylinder::new(0.45, 0.08));
    let bowsprit = meshes.add(Cylinder::new(0.12, 4.0));
    let main_mast = meshes.add(Cylinder::new(0.15, 12.0));
    let fore_mast = meshes.add(Cylinder::new(0.12, 9.0));
    let main_yard = meshes.add(Cuboid::new(7.4, 0.15, 0.15));
    let fore_yard = meshes.add(Cuboid::new(5.4, 0.12, 0.12));
    let main_sail = meshes.add(Rectangle::new(7.0, 6.0));
    let fore_sail = meshes.add(Rectangle::new(5.0, 4.5));
    let flag = meshes.add(Rectangle::new(0.8, 0.5));

    let half_pi = std::f32::consts::FRAC_PI_2;
    let main_z = -0.5;
    let fore_z = -4.2;

    commands
        .spawn((
            Ship,
            ShipMotion::default(),
            Transform::from_translation(pos)
                .with_rotation(Quat::from_rotation_y(yaw))
                .with_scale(Vec3::splat(SHIP_SCALE)),
            Visibility::Visible,
        ))
        .with_children(|parent| {
            let mut add = |mesh: Handle<Mesh>, mat: &Handle<StandardMaterial>, tf: Transform| {
                parent.spawn((Mesh3d(mesh), MeshMaterial3d(mat.clone()), tf));
            };
            add(hull, &wood, Transform::IDENTITY);
            add(deck, &deck_wood, Transform::IDENTITY);
            // Cabina di poppa con tetto.
            add(cabin, &dark_wood, Transform::from_xyz(0.0, DECK_Y + 1.0, 5.4));
            add(cabin_roof, &wood, Transform::from_xyz(0.0, DECK_Y + 2.07, 5.4));
            // Timone: montante e ruota (asse lungo Z).
            add(helm_post, &dark_wood, Transform::from_xyz(0.0, DECK_Y + 0.45, 3.55));
            add(
                helm_wheel,
                &dark_wood,
                Transform::from_xyz(0.0, DECK_Y + 1.1, 3.4)
                    .with_rotation(Quat::from_rotation_x(half_pi)),
            );
            // Bompresso: punta verso prua (-Z) e leggermente in alto.
            add(
                bowsprit,
                &dark_wood,
                Transform::from_xyz(0.0, DECK_Y + 0.7, -8.8)
                    .with_rotation(Quat::from_rotation_x(-(half_pi - 0.25))),
            );
            // Albero maestro.
            add(main_mast, &dark_wood, Transform::from_xyz(0.0, DECK_Y + 6.0, main_z));
            add(main_yard, &dark_wood, Transform::from_xyz(0.0, DECK_Y + 8.6, main_z));
            add(main_sail, &canvas, Transform::from_xyz(0.0, DECK_Y + 5.5, main_z + 0.2));
            add(flag, &banner, Transform::from_xyz(0.4, DECK_Y + 12.2, main_z));
            // Trinchetto.
            add(fore_mast, &dark_wood, Transform::from_xyz(0.0, DECK_Y + 4.5, fore_z));
            add(fore_yard, &dark_wood, Transform::from_xyz(0.0, DECK_Y + 6.7, fore_z));
            add(fore_sail, &canvas, Transform::from_xyz(0.0, DECK_Y + 4.4, fore_z + 0.2));
        })
        .id()
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
    mut ships: Query<(Entity, &Transform, &mut ShipMotion), With<Ship>>,
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

    // --- Esci dalla modalità nave (ovunque, anche in mare) ---
    if let Some(ship_entity) = ship_state.player_aboard {
        if let Ok((_, ship_tf, mut motion)) = ships.get_mut(ship_entity) {
            motion.speed = 0.0;
            // Piazza il player in piedi sul ponte, al posto del timoniere.
            let offset = ship_tf.rotation * Vec3::new(0.0, 0.0, HELM_STAND_Z * SHIP_SCALE);
            tf.translation.x = ship_tf.translation.x + offset.x;
            tf.translation.z = ship_tf.translation.z + offset.z;
            p.feet_y = ship_tf.translation.y + DECK_LOCAL_Y;
            tf.translation.y = p.feet_y + EYE_HEIGHT;
        }
        p.vel_y = 0.0;
        p.on_ground = true;
        ship_state.player_aboard = None;
        info!("Fuori dalla modalità nave: sei sul ponte");
        return;
    }

    // --- Entra in modalità nave (si arriva al timone) ---
    let player_pos = tf.translation;
    let mut best: Option<(f32, Entity)> = None;
    for (ship_entity, ship_tf, _) in &ships {
        let dist = Vec2::new(
            ship_tf.translation.x - player_pos.x,
            ship_tf.translation.z - player_pos.z,
        )
        .length();
        if dist < BOARDING_DISTANCE && best.map_or(true, |(d, _)| dist < d) {
            best = Some((dist, ship_entity));
        }
    }
    if let Some((_, ship_entity)) = best {
        p.vel_y = 0.0;
        p.on_ground = true;
        p.climbing = false;
        ship_state.player_aboard = Some(ship_entity);
        info!("Al timone");
    }
}

// ---------------------------------------------------------------------------
// Pilotaggio (W/S + A/D + mouse libero)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn ship_pilot(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    field: Res<HeightField>,
    origin: Res<Origin>,
    ship_state: Res<ShipState>,
    mut ships: Query<(&mut Transform, &mut ShipMotion), (With<Ship>, Without<Player>)>,
    mut player: Query<(&mut Player, &mut Transform), Without<Ship>>,
) {
    let Some(ship_entity) = ship_state.player_aboard else {
        return;
    };
    let Ok((mut ship_tf, mut motion)) = ships.get_mut(ship_entity) else {
        return;
    };
    let Ok((mut p, mut tf)) = player.get_single_mut() else {
        return;
    };

    let dt = time.delta_secs().min(0.1);

    // --- Sterzo: A/D ---
    let steer = (keys.pressed(KeyCode::KeyA) as i32 as f32)
        - (keys.pressed(KeyCode::KeyD) as i32 as f32);
    if steer != 0.0 {
        let yaw = ship_tf.rotation.to_euler(EulerRot::YXZ).0;
        let new_yaw = (yaw + steer * YAW_RATE * dt).rem_euclid(std::f32::consts::TAU);
        ship_tf.rotation = Quat::from_rotation_y(new_yaw);
    }

    // --- Throttle: W/S con accelerazione e attrito ---
    let throttle = (keys.pressed(KeyCode::KeyW) as i32 as f32)
        - (keys.pressed(KeyCode::KeyS) as i32 as f32);
    let target_speed = if throttle > 0.0 {
        MAX_SPEED_FWD
    } else if throttle < 0.0 {
        -MAX_SPEED_BWD
    } else {
        0.0
    };
    let (accel, reference) = if target_speed.abs() > motion.speed.abs() {
        (ACCELERATION, target_speed)
    } else {
        (FRICTION, target_speed)
    };
    let step = accel * dt;
    if (motion.speed - reference).abs() <= step {
        motion.speed = reference;
    } else if motion.speed < reference {
        motion.speed += step;
    } else {
        motion.speed -= step;
    }

    // --- Movimento (avanti lungo lo yaw della nave) ---
    if motion.speed.abs() > 0.01 {
        let yaw = ship_tf.rotation.to_euler(EulerRot::YXZ).0;
        let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
        let candidate = ship_tf.translation + forward * motion.speed * dt;

        // Blocca sulla terra: se la nuova posizione è sopra la spiaggia, ferma.
        if let Some(h) = field.ground(&origin, candidate.x, candidate.z) {
            if h < BEACH_ELEVATION - 1.0 {
                ship_tf.translation.x = candidate.x;
                ship_tf.translation.z = candidate.z;
            } else {
                motion.speed = 0.0; // incagliata: azzera velocità
            }
        }
    }

    // --- Quota: sul livello dell'acqua ---
    let water_y = field
        .ground(&origin, ship_tf.translation.x, ship_tf.translation.z)
        .unwrap_or(OCEAN_ELEVATION)
        .max(OCEAN_ELEVATION);
    ship_tf.translation.y = water_y + SHIP_Y_OFFSET;

    // --- Aggancia il player al timone ---
    let offset = ship_tf.rotation * Vec3::new(0.0, 0.0, HELM_STAND_Z * SHIP_SCALE);
    p.feet_y = ship_tf.translation.y + DECK_LOCAL_Y;
    p.vel_y = 0.0;
    p.on_ground = true;
    tf.translation.x = ship_tf.translation.x + offset.x;
    tf.translation.z = ship_tf.translation.z + offset.z;
    tf.translation.y = p.feet_y + EYE_HEIGHT;
}
