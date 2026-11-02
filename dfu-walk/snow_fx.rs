//! Neve soffice attorno al giocatore (3500 particelle procedurali).
//!
//! Attiva quando il meteo effettivo è `Neve` (`CurrentPreset`, deciso da
//! `calendar::sync_season_from_calendar`: calendario oppure override F4).

use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::player::Player;
use crate::sky::dream_sky::CurrentPreset;

const PARTICLE_COUNT: usize = 3500;
const RADIUS: f32 = 24.0;
const HEIGHT: f32 = 22.0;
const BASE_FALL_SPEED: f32 = 1.4;
const BASE_WIND: Vec2 = Vec2::new(1.4, 0.7);
const FLOOR_OFFSET: f32 = 3.0;
const CAMERA_DEADZONE: f32 = 0.6;
const FLAKE_MIN_SIZE: f32 = 0.05;
const FLAKE_MAX_SIZE: f32 = 0.22;

struct Rng(u64);

impl Default for Rng {
    fn default() -> Self {
        Self(0x9E37_79B9_7F4A_7C15)
    }
}

impl Rng {
    fn seed(mut self, seed: u64) -> Self {
        self.0 = if seed == 0 { 0x9E37_79B9_7F4A_7C15 } else { seed };
        self
    }
    #[inline]
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    #[inline]
    fn f32(&mut self) -> f32 {
        ((self.next() >> 40) as f32) / 16_777_216.0
    }
}

#[derive(Resource, Default)]
pub struct SnowState {
    active: bool,
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
    particles: Vec<Entity>,
    last_center_pos: Vec3,
    rng: Rng,
}

#[derive(Component)]
pub struct SnowFlake {
    fall_speed: f32,
    sway_phase_x: f32,
    sway_phase_z: f32,
    sway_freq: f32,
    sway_amplitude: f32,
    mass_factor: f32,
}

pub struct SnowFxPlugin;

impl Plugin for SnowFxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SnowState>().add_systems(
            Update,
            (
                setup_snow_assets,
             update_snow_activation,
             update_snow_particles,
            )
            .chain(),
        );
    }
}

fn create_soft_snowflake_image() -> Image {
    const SIZE: usize = 64;
    let mut data = Vec::with_capacity(SIZE * SIZE * 4);
    let center = (SIZE as f32 - 1.0) * 0.5;

    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = (x as f32 - center) / center;
            let dy = (y as f32 - center) / center;
            let dist_sq = dx * dx + dy * dy;

            let alpha = if dist_sq >= 1.0 {
                0.0
            } else {
                let dist = dist_sq.sqrt();
                let falloff = ((1.0 - dist) * std::f32::consts::FRAC_PI_2).sin();
                falloff.powf(1.6)
            };

            let a_byte = (alpha * 255.0).clamp(0.0, 255.0) as u8;
            data.extend_from_slice(&[255, 255, 255, a_byte]);
        }
    }

    Image::new(
        Extent3d {
            width: SIZE as u32,
            height: SIZE as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

fn setup_snow_assets(
    mut state: ResMut<SnowState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    if state.mesh.is_some() {
        return;
    }

    let hw = 0.5;
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [-hw, -hw, 0.0],
            [hw, -hw, 0.0],
            [hw, hw, 0.0],
            [-hw, hw, 0.0],
        ],
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 4]);
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
    );
    mesh.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));
    state.mesh = Some(meshes.add(mesh));

    let snow_img_handle = images.add(create_soft_snowflake_image());
    state.material = Some(materials.add(StandardMaterial {
        base_color_texture: Some(snow_img_handle),
                                        base_color: Color::srgba(1.0, 1.0, 1.0, 0.95),
                                        alpha_mode: AlphaMode::Blend,
                                        unlit: true,
                                        cull_mode: None,
                                        fog_enabled: false,
                                        ..default()
    }));
}

fn update_snow_activation(
    mut commands: Commands,
    preset: Option<Res<CurrentPreset>>,
    interior: Option<Res<crate::interior::InteriorState>>,
    dungeon: Option<Res<crate::dungeon::DungeonState>>,
    mut state: ResMut<SnowState>,
    player: Query<&Transform, With<Player>>,
) {
    let outside = !interior.as_ref().is_some_and(|i| i.is_inside())
    && !dungeon.as_ref().is_some_and(|d| d.is_inside());

    let snowing = preset.as_ref().is_some_and(|p| p.current.is_snow());

    let should_be_active = outside && snowing;
    if should_be_active == state.active {
        return;
    }

    if !should_be_active {
        for e in state.particles.drain(..) {
            commands.entity(e).despawn();
        }
        state.active = false;
        info!("Neve: spenta");
        return;
    }

    let Ok(player_tf) = player.get_single() else {
        return;
    };
    let (Some(mesh), Some(material)) = (state.mesh.clone(), state.material.clone()) else {
        return;
    };

    let center = player_tf.translation;
    state.rng = std::mem::take(&mut state.rng).seed(
        center.x.to_bits() as u64 ^ ((center.z.to_bits() as u64).rotate_left(32)),
    );

    let mut new_particles = Vec::with_capacity(PARTICLE_COUNT);
    for _ in 0..PARTICLE_COUNT {
        let e = spawn_flake(&mut commands, &mesh, &material, center, &mut state.rng, false);
        new_particles.push(e);
    }
    state.particles = new_particles;
    state.last_center_pos = center;
    state.active = true;
    info!("Neve attiva: {} fiocchi", state.particles.len());
}

fn spawn_flake(
    commands: &mut Commands,
    mesh: &Handle<Mesh>,
    material: &Handle<StandardMaterial>,
    origin: Vec3,
    rng: &mut Rng,
    spawn_at_top: bool,
) -> Entity {
    let angle = rng.f32() * std::f32::consts::TAU;
    let radius = CAMERA_DEADZONE + rng.f32().sqrt() * (RADIUS - CAMERA_DEADZONE);

    let y_pos = if spawn_at_top {
        origin.y + HEIGHT * (0.85 + rng.f32() * 0.15)
    } else {
        origin.y + rng.f32() * HEIGHT
    };

    let pos = Vec3::new(
        origin.x + angle.cos() * radius,
                        y_pos,
                        origin.z + angle.sin() * radius,
    );

    let size_factor = rng.f32().powf(2.4);
    let size = FLAKE_MIN_SIZE + size_factor * (FLAKE_MAX_SIZE - FLAKE_MIN_SIZE);

    let flake = SnowFlake {
        fall_speed: BASE_FALL_SPEED * (0.7 + (1.0 - size_factor) * 0.6),
        sway_phase_x: rng.f32() * std::f32::consts::TAU,
        sway_phase_z: rng.f32() * std::f32::consts::TAU,
        sway_freq: 1.2 + rng.f32() * 1.5,
        sway_amplitude: 0.25 + rng.f32() * 0.35,
        mass_factor: 0.6 + size_factor * 0.6,
    };

    commands
    .spawn((
        Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(pos).with_scale(Vec3::splat(size)),
            flake,
    ))
    .id()
}

fn update_snow_particles(
    time: Res<Time>,
    mut state: ResMut<SnowState>,
    player_q: Query<&Transform, (With<Player>, Without<SnowFlake>)>,
                         mut flakes: Query<(&mut Transform, &mut SnowFlake), (With<SnowFlake>, Without<Player>)>,
) {
    if !state.active {
        return;
    }

    let Ok(player_tf) = player_q.get_single() else {
        return;
    };
    let center = player_tf.translation;
    let camera_rot = player_tf.rotation;
    let dt = time.delta_secs().min(0.05);
    let t = time.elapsed_secs();

    let gust_val = (t * 0.35).sin() * 0.6 + (t * 0.85).cos() * 0.3;
    let current_wind = Vec2::new(
        BASE_WIND.x + gust_val * 1.2,
        BASE_WIND.y + gust_val * 0.7,
    );

    let teleported = center.distance_squared(state.last_center_pos) > 40.0 * 40.0;
    state.last_center_pos = center;

    for (mut transform, mut flake) in &mut flakes {
        if teleported {
            let angle = state.rng.f32() * std::f32::consts::TAU;
            let radius = CAMERA_DEADZONE + state.rng.f32().sqrt() * (RADIUS - CAMERA_DEADZONE);
            transform.translation = Vec3::new(
                center.x + angle.cos() * radius,
                                              center.y + state.rng.f32() * HEIGHT,
                                              center.z + angle.sin() * radius,
            );
            continue;
        }

        flake.sway_phase_x += flake.sway_freq * dt;
        flake.sway_phase_z += (flake.sway_freq * 0.75) * dt;

        let sway = Vec3::new(
            flake.sway_phase_x.sin() * flake.sway_amplitude,
                             (flake.sway_phase_x + flake.sway_phase_z).sin() * 0.1,
                             flake.sway_phase_z.cos() * (flake.sway_amplitude * 0.85),
        );

        let velocity = Vec3::new(
            current_wind.x * flake.mass_factor,
            -flake.fall_speed,
            current_wind.y * flake.mass_factor,
        );

        transform.translation += (velocity + sway) * dt;

        let offset_xz = Vec2::new(
            transform.translation.x - center.x,
            transform.translation.z - center.z,
        );
        let dist_xz = offset_xz.length();

        if dist_xz > RADIUS {
            let normal_back = -offset_xz.normalize();
            transform.translation.x = center.x + normal_back.x * (RADIUS * 0.95);
            transform.translation.z = center.z + normal_back.y * (RADIUS * 0.95);
        }

        if transform.translation.y < center.y - FLOOR_OFFSET {
            let angle = state.rng.f32() * std::f32::consts::TAU;
            let radius = state.rng.f32().sqrt() * RADIUS;
            transform.translation = Vec3::new(
                center.x + angle.cos() * radius,
                                              center.y + HEIGHT * (0.9 + state.rng.f32() * 0.1),
                                              center.z + angle.sin() * radius,
            );
        }

        transform.rotation = camera_rot;
    }
}
