//! Cielo procedurale Dynamic Skies (DREAM-SKY) con `DreamSkyMaterial`.
//!
//! ## Cosa fa
//!
//! - Spawna una sfera raggio `SKY_DOME_RADIUS` centrata sulla camera con
//!   `DreamSkyMaterial`.
//! - Carica le texture del preset dal `.dfmod` via `ModTextures` (cloud top/bottom
//!   diffuse + normal, stelle + twinkle mask + noise, Masser, Secunda). Se una
//!   texture manca, crea un fallback 1×1 (bianco, nero o normale piatto) e logga
//!   un warning **una volta sola**.
//! - Aggiorna `world_time` (per l'orbita delle lune e lo scrolling delle nuvole) e
//!   `sun_dir` (per lo scattering atmosferico) ad ogni frame.
//! - **Non sceglie il meteo**: legge `CurrentPreset.current`, che scrive
//!   `calendar::sync_season_from_calendar`. La catena è
//!   `SkyClock.date → WeatherPreset → Season → terreno/alberi/pioggia/neve`,
//!   e `WeatherPreset → CurrentPreset → questo cielo`. Per forzare un preset
//!   si usa l'override del calendario (F4).

use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::{MaterialPlugin, NotShadowCaster};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::view::NoFrustumCulling;

use super::dream_sky_material::{DreamSkyMaterial, SkyboxUniforms, DREAM_SKY_SHADER};
use super::presets::{preset_for, SkyboxPreset, WeatherPreset};
use super::SkyClock;
use crate::data::sky::sun_direction;
use crate::mod_textures::ModTextures;
use crate::player::Player;

/// Raggio della sfera del cielo, in metri.
const SKY_DOME_RADIUS: f32 = 20_000.0;
const SKY_FOLLOW_CAMERA: bool = true;

#[derive(Resource)]
pub struct CurrentPreset {
    pub current: WeatherPreset,
    pub manual_override: bool,
    last_applied: Option<WeatherPreset>,
}

impl Default for CurrentPreset {
    fn default() -> Self {
        Self { current: WeatherPreset::Cloudy, manual_override: false, last_applied: None }
    }
}

#[derive(Resource, Default)]
pub struct DreamSkySpawned(pub bool);

#[derive(Component)]
pub struct DreamSkyDome;

pub struct DreamSkyPlugin;

impl Plugin for DreamSkyPlugin {
    fn build(&self, app: &mut App) {
        {
            let src = include_str!("dream_sky.wgsl");
            let at_infinity = src.contains("const SKY_AT_INFINITY: bool = true;");
            let debug_line = src
            .lines()
            .find(|l| l.starts_with("const DEBUG_MODE"))
            .unwrap_or("const DEBUG_MODE: ?");
            info!(
                "Cielo: shader incorporato → SKY_AT_INFINITY={}, {} | follow_camera={}",
                at_infinity,
                debug_line.trim(),
                  SKY_FOLLOW_CAMERA
            );
        }
        bevy::asset::load_internal_asset!(
            app,
            DREAM_SKY_SHADER,
            "dream_sky.wgsl",
            Shader::from_wgsl
        );
        app.add_plugins(MaterialPlugin::<DreamSkyMaterial>::default())
        .init_resource::<CurrentPreset>()
        .init_resource::<DreamSkySpawned>()
        .add_systems(PostStartup, hide_legacy_sky_parts)
        .add_systems(
            Update,
            (
                spawn_or_update_sky.after(crate::calendar::sync_season_from_calendar),
             update_dream_sky_uniforms,
             follow_camera
             .after(crate::player::player_look)
             .after(crate::player::player_move),
            )
            .chain(),
        );
    }
}

fn hide_legacy_sky_parts(mut commands: Commands, parts: Query<Entity, With<super::SkyPart>>) {
    let count = parts.iter().count();
    for e in &parts {
        commands.entity(e).insert(Visibility::Hidden);
    }
    if count > 0 {
        info!("Cielo: {} entità legacy (SkyPart::*) nascoste", count);
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_or_update_sky(
    mut commands: Commands,
    mut spawned: ResMut<DreamSkySpawned>,
    mut preset: ResMut<CurrentPreset>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<DreamSkyMaterial>>,
    mut mods: ResMut<ModTextures>,
    player: Query<&Transform, With<Player>>,
    mut dome: Query<(Entity, &mut MeshMaterial3d<DreamSkyMaterial>), With<DreamSkyDome>>,
) {
    let need_spawn = !spawned.0;
    let preset_changed = preset.last_applied != Some(preset.current);
    if !need_spawn && !preset_changed {
        return;
    }

    let Ok(player_tf) = player.get_single() else {
        return;
    };

    let material = build_material(preset.current, &mut mods, &mut images, &mut materials);

    if need_spawn {
        let mesh = meshes.add(Sphere::new(SKY_DOME_RADIUS).mesh().uv(64, 32));
        commands.spawn((
            Name::new("DreamSkyDome"),
                        Mesh3d(mesh),
                        MeshMaterial3d(material),
                        Transform::from_translation(player_tf.translation),
                        Visibility::Visible,
                        DreamSkyDome,
                        NotShadowCaster,
                        NoFrustumCulling,
        ));
        spawned.0 = true;
        info!(
            "Cielo: sfera spawnata con DreamSkyMaterial (preset {:?}, raggio {} m)",
              preset.current, SKY_DOME_RADIUS
        );
    } else {
        for (_e, mut mat) in &mut dome {
            mat.0 = material.clone();
        }
        info!(
            "Cielo: materiale sostituito per preset {:?}",
            preset.current
        );
    }

    preset.last_applied = Some(preset.current);
}

fn update_dream_sky_uniforms(
    time: Res<Time>,
    clock: Res<SkyClock>,
    mut materials: ResMut<Assets<DreamSkyMaterial>>,
    dome: Query<&MeshMaterial3d<DreamSkyMaterial>, With<DreamSkyDome>>,
) {
    let Ok(mat_handle) = dome.get_single() else {
        return;
    };
    let Some(mat) = materials.get_mut(&mat_handle.0) else {
        return;
    };

    mat.uniforms.world_time = time.elapsed_secs();
    mat.uniforms.sun_dir = Vec3::from(sun_direction(clock.hours));
}

fn follow_camera(
    camera: Query<&Transform, (With<Player>, Without<DreamSkyDome>)>,
                 mut domes: Query<&mut Transform, (With<DreamSkyDome>, Without<Player>)>,
) {
    if !SKY_FOLLOW_CAMERA {
        return;
    }
    let Ok(cam) = camera.get_single() else { return };
    let pos = cam.translation;
    for mut tf in &mut domes {
        tf.translation = pos;
    }
}

fn build_material(
    preset: WeatherPreset,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
    materials: &mut Assets<DreamSkyMaterial>,
) -> Handle<DreamSkyMaterial> {
    let p = preset_for(preset);

    let cloud_top_diffuse = load_texture(
        p.top_clouds.texture,
        "cloud_top_diffuse",
        [255, 255, 255, 255],
        mods,
        images,
    );
    let cloud_top_normal = load_texture(
        p.top_clouds.normal_texture,
        "cloud_top_normal",
        [128, 128, 255, 255],
        mods,
        images,
    );
    let cloud_bottom_diffuse = load_texture(
        p.bottom_clouds.texture,
        "cloud_bottom_diffuse",
        [255, 255, 255, 255],
        mods,
        images,
    );
    let cloud_bottom_normal = load_texture(
        p.bottom_clouds.normal_texture,
        "cloud_bottom_normal",
        [128, 128, 255, 255],
        mods,
        images,
    );
    let star_tex = load_texture(
        p.stars.texture,
        "star_tex",
        [0, 0, 0, 255],
        mods,
        images,
    );
    let star_twinkle_mask = load_texture(
        p.stars.twinkle_mask,
        "star_twinkle_mask",
        [255, 255, 255, 255],
        mods,
        images,
    );
    let twinkle_noise = load_texture(
        p.stars.twinkle_noise,
        "twinkle_noise",
        [255, 255, 255, 255],
        mods,
        images,
    );
    let moon_tex = load_texture(
        p.masser.texture,
        "moon_tex (Masser)",
                                [255, 255, 255, 255],
                                mods,
                                images,
    );
    let secunda_tex = load_texture(
        p.secunda.texture,
        "secunda_tex (Secunda)",
                                   [255, 255, 255, 255],
                                   mods,
                                   images,
    );

    materials.add(DreamSkyMaterial {
        uniforms: uniforms_from_preset(p),
                  cloud_top_diffuse,
                  cloud_top_normal,
                  cloud_bottom_diffuse,
                  cloud_bottom_normal,
                  star_tex,
                  star_twinkle_mask,
                  twinkle_noise,
                  moon_tex,
                  secunda_tex,
    })
}

fn load_texture(
    name: &str,
    label: &'static str,
    fallback_rgba: [u8; 4],
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
) -> Handle<Image> {
    let handle = load_texture_raw(name, label, fallback_rgba, mods, images);
    let is_data = !label.starts_with("moon") && !label.starts_with("secunda");
    if is_data {
        if let Some(img) = images.get_mut(&handle) {
            let fmt = img.texture_descriptor.format;
            img.texture_descriptor.format = fmt.remove_srgb_suffix();
        }
    }
    if let Some(img) = images.get_mut(&handle) {
        img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            address_mode_w: ImageAddressMode::Repeat,
            mag_filter: ImageFilterMode::Linear,
            min_filter: ImageFilterMode::Linear,
            mipmap_filter: ImageFilterMode::Linear,
            anisotropy_clamp: 8,
            ..default()
        });
    } else {
        debug!("Cielo: '{name}' ({label}) già sulla GPU, sampler invariato");
    }
    handle
}

fn load_texture_raw(
    name: &str,
    label: &'static str,
    fallback_rgba: [u8; 4],
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
) -> Handle<Image> {
    if mods.has_image(name) {
        if let Some(h) = mods.image_by_name(name, images) {
            return h;
        }
    }

    if let Some(found) = mods.find_by_prefix(name, &[]) {
        if let Some(h) = mods.image_by_name(&found, images) {
            info!("Cielo: '{name}' ({label}) risolto come '{found}'");
            return h;
        }
    }

    warn!("Cielo: texture '{name}' ({label}) non trovata, uso fallback 1×1");
    images.add(solid_image(fallback_rgba))
}

fn solid_image(rgba: [u8; 4]) -> Image {
    let mut img = Image::new(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba.to_vec(),
                             TextureFormat::Rgba8Unorm,
                             RenderAssetUsages::default(),
    );
    img.sampler = bevy::image::ImageSampler::linear();
    img
}

fn uniforms_from_preset(p: &SkyboxPreset) -> SkyboxUniforms {
    SkyboxUniforms {
        sun_size: p.sun_size,
        sun_convergence: p.sun_convergence,
        atmosphere_normal: p.atmosphere_normal,
        atmosphere_dawn_dusk: p.atmosphere_dawn_dusk,
        atmosphere_lerp: p.atmosphere_lerp,
        sky_tint: Vec3::from_array(p.sky_tint),
        ground_color: Vec3::from_array(p.ground_color),
        exposure: p.exposure,

        night_start: p.night_start,
        night_end: p.night_end,
        sky_fade_start: p.sky_fade_start,
        sky_end_start: p.sky_end_start,

        fog_day: Vec3::from_array(p.fog_day),
        fog_night: Vec3::from_array(p.fog_night),
        fog_distance: p.fog_distance,
        moon_night_color: Vec3::from_array(p.moon_night_color),

        step_size: p.step_size,
        reduce_color: 0,

        cloud_top_tiling: Vec2::from_array(p.top_clouds.tiling),
        cloud_top_offset: Vec2::from_array(p.top_clouds.offset),
        cloud_top_day: Vec3::from_array(p.top_clouds.day_color),
        cloud_top_alpha_threshold: p.top_clouds.alpha_threshold,
        cloud_top_night: Vec3::from_array(p.top_clouds.night_color),
        cloud_top_alpha_max: p.top_clouds.alpha_max,
        cloud_top_color_boost: p.top_clouds.color_boost,
        cloud_top_normal_effect: p.top_clouds.normal_effect,
        cloud_top_opacity: p.top_clouds.opacity,
        cloud_top_bending: p.top_clouds.bending,
        cloud_top_sun_color: Vec3::from_array(p.top_clouds.sun_color),
        cloud_top_sun_scale: p.top_clouds.sun_color_scale,
        cloud_top_sun_lerp_scale: p.top_clouds.sun_color_lerp_scale,
        cloud_top_speed: p.top_clouds.speed,

        cloud_bottom_tiling: Vec2::from_array(p.bottom_clouds.tiling),
        cloud_bottom_offset: Vec2::from_array(p.bottom_clouds.offset),
        cloud_bottom_day: Vec3::from_array(p.bottom_clouds.day_color),
        cloud_bottom_alpha_threshold: p.bottom_clouds.alpha_threshold,
        cloud_bottom_night: Vec3::from_array(p.bottom_clouds.night_color),
        cloud_bottom_alpha_max: p.bottom_clouds.alpha_max,
        cloud_bottom_color_boost: p.bottom_clouds.color_boost,
        cloud_bottom_normal_effect: p.bottom_clouds.normal_effect,
        cloud_bottom_opacity: p.bottom_clouds.opacity,
        cloud_bottom_bending: p.bottom_clouds.bending,
        cloud_bottom_sun_color: Vec3::from_array(p.bottom_clouds.sun_color),
        cloud_bottom_sun_scale: p.bottom_clouds.sun_color_scale,
        cloud_bottom_sun_lerp_scale: p.bottom_clouds.sun_color_lerp_scale,
        cloud_bottom_speed: p.bottom_clouds.speed,
        cloud_bottom_blend_speed: -0.02,
        cloud_bottom_blend_scale: 1.0,
        cloud_bottom_blend_lb: 0.17,
        cloud_bottom_blend_ub: 0.32,

        star_tiling: Vec2::from_array(p.stars.tiling),
        star_bending: p.stars.bending,
        twinkle_tiling: Vec2::splat(1.0),
        twinkle_boost: p.stars.twinkle_boost,
        twinkle_speed: p.stars.twinkle_speed,

        masser_color: Vec3::from_array(p.masser.color),
        masser_min_size: p.masser.min_size,
        masser_max_size: p.masser.max_size,
        masser_orbit_angle: Vec3::from_array(p.masser.orbit_angle),
        masser_orbit_offset: p.masser.orbit_offset,
        masser_orbit_speed: p.masser.orbit_speed,
        masser_semi_major: p.masser.semi_major,
        masser_semi_minor: p.masser.semi_minor,
        masser_auto_phase: p.masser.auto_phase as u32,
        masser_tidal_angle: Vec3::from_array(p.masser.tidal_angle),
        masser_spin_speed: Vec3::from_array(p.masser.spin_speed),

        secunda_color: Vec3::from_array(p.secunda.color),
        secunda_min_size: p.secunda.min_size,
        secunda_max_size: p.secunda.max_size,
        secunda_orbit_angle: Vec3::from_array(p.secunda.orbit_angle),
        secunda_orbit_offset: p.secunda.orbit_offset,
        secunda_orbit_speed: p.secunda.orbit_speed,
        secunda_semi_major: p.secunda.semi_major,
        secunda_semi_minor: p.secunda.semi_minor,
        secunda_auto_phase: p.secunda.auto_phase as u32,
        secunda_tidal_angle: Vec3::from_array(p.secunda.tidal_angle),
        secunda_spin_speed: Vec3::from_array(p.secunda.spin_speed),

        world_time: 0.0,
        sun_dir: Vec3::ZERO,
        cloud_fade_height: p.cloud_fade_height,

        // Pulsazione dell'opacità delle nuvole: 0 = nessuna pulsazione.
        // Se nei preset hai `opacity_pulse_speed: 0.0` (default) il cielo è
        // identico a prima. Per attivarla metti 0.05 (o simile) su `SUNNY`.
        cloud_top_opacity_pulse_speed: p.top_clouds.opacity_pulse_speed,
        cloud_bottom_opacity_pulse_speed: p.bottom_clouds.opacity_pulse_speed,
    }
}
