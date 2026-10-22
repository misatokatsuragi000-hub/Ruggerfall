//! Cielo: cupola con colori che cambiano nel corso della giornata, sole, luna,
//! stelle e uno strato di nuvole che scorre. Segue il giocatore, quindi non ha
//! parallasse; la matematica sta in `crate::data::sky`.

use crate::player::Player;
use crate::world::{absolute, Origin};
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::math::Affine2;
use bevy::pbr::DistanceFog;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, Face, TextureDimension, TextureFormat};
use crate::data::sky::{
    cloud_grid, generate_cloud_texture, generate_stars, lighting, mip_chain_rgba, sky_color, sky_state, SkyState,
};

const DOME_RADIUS: f32 = 50_000.0;
const SUN_DISTANCE: f32 = 45_000.0;
const SUN_RADIUS: f32 = 1_100.0;
const MOON_RADIUS: f32 = 900.0;
const STARS_RADIUS: f32 = 48_000.0;
const STAR_COUNT: usize = 1800;
const CLOUD_ALTITUDE: f32 = 3_000.0;
const CLOUD_RADIUS: f32 = 35_000.0;
const CLOUD_TILE: f32 = 6_000.0;
const CLOUD_TEXTURE: usize = 512;
const CLOUD_MIPS: usize = 9;
/// Vento in m/s verso est e verso sud.
const WIND: (f64, f64) = (9.0, 3.0);
/// Ogni quanto si ricolora la cupola (secondi).
const DOME_UPDATE_INTERVAL: f32 = 0.2;

#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum SkyPart {
    Dome,
    Sun,
    Moon,
    Stars,
    Clouds,
}

#[derive(Component)]
pub struct SunLight;

#[derive(Resource)]
pub struct SkyConfig {
    pub hour: f32,
    /// Minuti reali per un giorno intero; 0 = ora ferma.
    pub day_minutes: f32,
}

#[derive(Resource)]
pub struct SkyClock {
    pub hours: f32,
    pub hours_per_second: f32,
    pub paused: bool,
}

#[derive(Resource)]
pub struct SkyHandles {
    dome: Handle<Mesh>,
    stars: Handle<StandardMaterial>,
    clouds: Handle<StandardMaterial>,
}

fn dome_colors(mesh: &Mesh, st: &SkyState) -> Vec<[f32; 4]> {
    let Some(VertexAttributeValues::Float32x3(positions)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
        return Vec::new();
    };
    positions
        .iter()
        .map(|p| {
            let l = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt().max(1e-6);
            let c = sky_color([p[0] / l, p[1] / l, p[2] / l], st);
            [c[0], c[1], c[2], 1.0]
        })
        .collect()
}

fn unlit(color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color,
        unlit: true,
        fog_enabled: false,
        ..default()
    }
}

pub fn setup_sky(
    mut commands: Commands,
    config: Res<SkyConfig>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let hours = config.hour.rem_euclid(24.0);
    let st = sky_state(hours);
    commands.insert_resource(SkyClock {
        hours,
        hours_per_second: if config.day_minutes > 0.0 {
            24.0 / (config.day_minutes * 60.0)
        } else {
            0.0
        },
        paused: config.day_minutes <= 0.0,
    });

    // Cupola: sfera vista dall'interno, colorata per vertice.
    let mut dome = Sphere::new(DOME_RADIUS).mesh().uv(64, 32);
    let colors = dome_colors(&dome, &st);
    dome.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    let dome_handle = meshes.add(dome);
    commands.spawn((
        Mesh3d(dome_handle.clone()),
        MeshMaterial3d(materials.add(StandardMaterial {
            cull_mode: Some(Face::Front),
            ..unlit(Color::WHITE)
        })),
        Transform::default(),
        SkyPart::Dome,
    ));

    // Sole e luna.
    commands.spawn((
        Mesh3d(meshes.add(Sphere::new(SUN_RADIUS).mesh().uv(24, 12))),
        MeshMaterial3d(materials.add(unlit(Color::srgb(1.0, 0.93, 0.70)))),
        Transform::default(),
        Visibility::Hidden,
        SkyPart::Sun,
    ));
    commands.spawn((
        Mesh3d(meshes.add(Sphere::new(MOON_RADIUS).mesh().uv(24, 12))),
        MeshMaterial3d(materials.add(unlit(Color::srgb(0.88, 0.91, 1.0)))),
        Transform::default(),
        Visibility::Hidden,
        SkyPart::Moon,
    ));

    // Stelle: piccoli quadrati sulla sfera, visibili solo di notte.
    let stars = generate_stars(STAR_COUNT, 20_240_607, STARS_RADIUS);
    let mut star_mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    star_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, stars.positions);
    star_mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, stars.colors);
    star_mesh.insert_indices(Indices::U32(stars.indices));
    let star_material = materials.add(StandardMaterial {
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..unlit(Color::linear_rgba(1.0, 1.0, 1.0, 0.0))
    });
    commands.spawn((
        Mesh3d(meshes.add(star_mesh)),
        MeshMaterial3d(star_material.clone()),
        Transform::default(),
        Visibility::Hidden,
        SkyPart::Stars,
    ));

    // Nuvole: piano alto con texture ripetibile e bordo che sfuma.
    let chain = mip_chain_rgba(&generate_cloud_texture(CLOUD_TEXTURE), CLOUD_TEXTURE, CLOUD_TEXTURE, CLOUD_MIPS);
    let mut image = Image::default();
    image.texture_descriptor.size = Extent3d {
        width: CLOUD_TEXTURE as u32,
        height: CLOUD_TEXTURE as u32,
        depth_or_array_layers: 1,
    };
    image.texture_descriptor.dimension = TextureDimension::D2;
    image.texture_descriptor.format = TextureFormat::Rgba8UnormSrgb;
    image.texture_descriptor.mip_level_count = chain.len() as u32;
    image.data = chain.concat();
    image.asset_usage = RenderAssetUsages::RENDER_WORLD;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..default()
    });
    let grid = cloud_grid(CLOUD_RADIUS, 40, CLOUD_TILE);
    let mut cloud_mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    cloud_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, grid.positions);
    cloud_mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, grid.uvs);
    cloud_mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, grid.colors);
    cloud_mesh.insert_indices(Indices::U32(grid.indices));
    let cloud_material = materials.add(StandardMaterial {
        base_color_texture: Some(images.add(image)),
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        ..unlit(Color::WHITE)
    });
    commands.spawn((
        Mesh3d(meshes.add(cloud_mesh)),
        MeshMaterial3d(cloud_material.clone()),
        Transform::from_xyz(0.0, CLOUD_ALTITUDE, 0.0),
        SkyPart::Clouds,
    ));

    // Luce direzionale (sole o luna), aggiornata in `update_sky`.
    commands.spawn((
        DirectionalLight {
            illuminance: 11_000.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::default(),
        SunLight,
    ));

    commands.insert_resource(SkyHandles {
        dome: dome_handle,
        stars: star_material,
        clouds: cloud_material,
    });
}

#[allow(clippy::too_many_arguments)]
pub fn update_sky(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    origin: Res<Origin>,
    handles: Res<SkyHandles>,
    mut clock: ResMut<SkyClock>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut ambient: ResMut<AmbientLight>,
    mut clear: ResMut<ClearColor>,
    player: Query<&Transform, With<Player>>,
    mut parts: Query<(&SkyPart, &mut Transform, &mut Visibility), Without<Player>>,
    mut light: Query<(&mut DirectionalLight, &mut Transform), (With<SunLight>, Without<Player>, Without<SkyPart>)>,
    mut fog: Query<&mut DistanceFog>,
    mut dome_timer: Local<f32>,
) {
    let Ok(cam) = player.get_single().map(|t| t.translation) else { return };
    let dt = time.delta_secs();

    // Orologio: T ferma/riprende, "," e "." spostano di un'ora.
    let mut jumped = false;
    if keys.just_pressed(KeyCode::KeyT) {
        clock.paused = !clock.paused && clock.hours_per_second > 0.0;
        if clock.hours_per_second == 0.0 {
            clock.hours_per_second = 24.0 / (24.0 * 60.0);
            clock.paused = false;
        }
    }
    if keys.just_pressed(KeyCode::Comma) {
        clock.hours = (clock.hours - 1.0).rem_euclid(24.0);
        jumped = true;
    }
    if keys.just_pressed(KeyCode::Period) {
        clock.hours = (clock.hours + 1.0).rem_euclid(24.0);
        jumped = true;
    }
    if !clock.paused {
        clock.hours = (clock.hours + clock.hours_per_second * dt).rem_euclid(24.0);
    }

    let st = sky_state(clock.hours);
    let lit = lighting(&st);

    // Cupola ricolorata a intervalli, non a ogni fotogramma.
    *dome_timer += dt;
    if *dome_timer >= DOME_UPDATE_INTERVAL || jumped {
        *dome_timer = 0.0;
        let colors = meshes.get(&handles.dome).map(|m| dome_colors(m, &st));
        if let (Some(colors), Some(mesh)) = (colors, meshes.get_mut(&handles.dome)) {
            if !colors.is_empty() {
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
            }
        }
    }

    // Tutto il cielo segue la camera.
    let stars_visible = lit.star_alpha > 0.01;
    for (part, mut tf, mut vis) in &mut parts {
        match part {
            SkyPart::Dome => tf.translation = cam,
            SkyPart::Sun => {
                tf.translation = cam + Vec3::from(st.sun) * SUN_DISTANCE;
                *vis = if st.sun[1] > -0.05 { Visibility::Visible } else { Visibility::Hidden };
            }
            SkyPart::Moon => {
                tf.translation = cam + Vec3::from(st.moon) * SUN_DISTANCE;
                *vis = if st.moon[1] > -0.05 { Visibility::Visible } else { Visibility::Hidden };
            }
            SkyPart::Stars => {
                tf.translation = cam;
                *vis = if stars_visible { Visibility::Visible } else { Visibility::Hidden };
            }
            SkyPart::Clouds => tf.translation = Vec3::new(cam.x, CLOUD_ALTITUDE, cam.z),
        }
    }

    if let Some(m) = materials.get_mut(&handles.stars) {
        m.base_color = Color::linear_rgba(1.0, 1.0, 1.0, lit.star_alpha);
    }
    if let Some(m) = materials.get_mut(&handles.clouds) {
        let c = lit.cloud_tint;
        m.base_color = Color::linear_rgb(c[0], c[1], c[2]);
        // Le nuvole restano ferme nel mondo e derivano con il vento.
        let (east, north) = absolute(&origin, cam);
        let t = time.elapsed_secs_f64();
        let u = ((east - WIND.0 * t) / CLOUD_TILE as f64).rem_euclid(1.0) as f32;
        let v = ((-north - WIND.1 * t) / CLOUD_TILE as f64).rem_euclid(1.0) as f32;
        m.uv_transform = Affine2::from_translation(Vec2::new(u, v));
    }

    // Luce, ambiente e nebbia.
    if let Ok((mut l, mut tf)) = light.get_single_mut() {
        let c = lit.light_color;
        l.illuminance = lit.illuminance;
        l.color = Color::linear_rgb(c[0], c[1], c[2]);
        *tf = Transform::default().looking_to(-Vec3::from(lit.light_from), Vec3::Y);
    }
    let a = lit.ambient_color;
    ambient.color = Color::linear_rgb(a[0], a[1], a[2]);
    ambient.brightness = lit.ambient_brightness;
    let f = lit.fog_color;
    clear.0 = Color::linear_rgb(f[0], f[1], f[2]);
    for mut fog in &mut fog {
        fog.color = Color::linear_rgb(f[0], f[1], f[2]);
    }
}
