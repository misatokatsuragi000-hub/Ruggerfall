//! Cielo: solo orologio e illuminazione.
//!
//! Dopo l'integrazione di `dream_sky.rs` (Dynamic Skies), questo modulo
//! **non gestisce più** la cupola, le nuvole, le stelle, le lune o l'overcast
//! DREAM: quelle sono gestite da `DreamSkyPlugin` (in `sky/dream_sky.rs`).
//!
//! Restano qui:
//! - `SkyClock` (condiviso con `dream_sky.rs` per `sun_direction`)
//! - `SkyConfig` (parametri di avvio: ora, minuti per giorno, mese, giorno)
//! - `setup_sky` che crea `SunLight` (DirectionalLight) e `SkyClock`
//! - `update_sky` che aggiorna **solo** `DirectionalLight`, `AmbientLight`,
//!   `ClearColor` e `DistanceFog` in base a `sky_state(clock.hours)`.
//!
//! ## Sottomoduli esposti
//!
//! - `sky::dream_sky` (`DreamSkyPlugin`, `CurrentPreset`, `DreamSkyDome`,
//!   `DreamSkySpawned`)
//! - `sky::dream_sky_material` (`DreamSkyMaterial`, `SkyboxUniforms`)
//! - `sky::presets` (`WeatherPreset`, `preset_for`, `SUNNY`, `CLOUDY`, ...)
//!
//! **NON** usare `sky::dream_sky_plugin`: è un file vecchio/incompleto che
//! ha `unimplemented!()` in `resolve_texture`. Non è dichiarato come modulo
//! qui (e non lo era nemmeno prima).
//!
//! ## Cosa è stato rimosso
//!
//! - **Tasto F3 in `update_sky`**: era la causa del "dopo 8s il cielo si
//!   copre". Impostava `SkyOvercast.target = 1` e in `OVERCAST_FADE_SECS`
//!   secondi copriva tutto con il layer DREAM. F3 ora è solo in
//!   `dream_sky::cycle_preset_manual` (cambia il preset del cielo).
//! - **`drive_overcast_from_weather`**: era il pilota automatico
//!   dell'overcast DREAM. Ora è no-op.
//! - **`update_dream_texture`**: caricava la texture `2k sky N` per il
//!   vecchio layer. Ora è no-op (il layer è nascosto).
//! - **Aggiornamento dei materiali della vecchia cupola / nuvole / stelle**:
//!   non servono più (le entità sono nascoste da `hide_legacy_sky_parts` in
//!   `dream_sky.rs`).

use crate::calendar::GameDate;
use crate::data::climate::Season;
use crate::player::Player;
use crate::world::{GameData, Origin};
use bevy::pbr::{DistanceFog, FogFalloff};
use self::dream_sky::CurrentPreset;
use self::presets::WeatherLighting;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, Face, TextureDimension, TextureFormat};
use crate::data::sky::{
    cloud_grid, generate_cloud_texture, generate_stars, lighting, mip_chain_rgba,
    sky_state, SkyState,
};

// =============================================================================
// Sottomoduli (esposti a main.rs)
// =============================================================================

pub mod dream_sky;
pub mod dream_sky_material;
pub mod presets;

// =============================================================================
// Costanti (mantenute per setup_sky e per le vecchie entità nascoste)
// =============================================================================

const DOME_RADIUS: f32 = 50_000.0;
const SUN_DISTANCE: f32 = 30_000.0;
const SUN_RADIUS: f32 = 730.0;
const MOON_RADIUS: f32 = 600.0;
const STARS_RADIUS: f32 = 48_000.0;
const STAR_COUNT: usize = 1800;
const CLOUD_ALTITUDE: f32 = 3_000.0;
const CLOUD_RADIUS: f32 = 35_000.0;
const CLOUD_TILE: f32 = 6_000.0;
const CLOUD_TEXTURE: usize = 512;
const CLOUD_MIPS: usize = 9;
const STARS_DEPTH_BIAS: f32 = -200_000.0;

// =============================================================================
// Componenti e risorse
// =============================================================================

#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum SkyPart {
    Dome,
    Sun,
    Moon,
    Stars,
    Clouds,
    DreamOverlay,
    DreamOvercast,
}

#[derive(Component)]
pub struct SunLight;

#[derive(Resource)]
pub struct SkyConfig {
    pub hour: f32,
    pub day_minutes: f32,
    pub start_month: u8,
    pub start_day: u8,
}

impl Default for SkyConfig {
    fn default() -> Self {
        Self {
            hour: 10.5,
            day_minutes: 24.0,
            start_month: 0,
            start_day: 5,
        }
    }
}

#[derive(Resource)]
pub struct SkyClock {
    pub date: GameDate,
    pub hours: f32,
    pub hours_per_second: f32,
    pub paused: bool,
}

impl SkyClock {
    pub fn advance_seconds(&mut self, seconds: f32) {
        if self.paused || self.hours_per_second == 0.0 {
            return;
        }
        self.date
        .add_hours(self.hours_per_second as f64 * seconds as f64);
        self.hours = self.date.hour;
    }

    pub fn advance_hours(&mut self, hours: f64) {
        self.date.add_hours(hours);
        self.hours = self.date.hour;
    }
}

/// Overcast del vecchio sistema DREAM: **non più usato**. La risorsa esiste
/// ancora per retro-compatibilità con eventuali moduli esterni, ma il livello
/// è sempre 0 e non copre più niente.
#[derive(Resource, Default)]
pub struct SkyOvercast {
    pub target: f32,
    pub level: f32,
    pub manual_override: Option<f32>,
}

/// Se true, il maltempo accorcia anche la portata della nebbia di distanza
/// (funziona solo con `FogFalloff::Linear`).
const WEATHER_FOG_RANGE: bool = true;
/// Costante di tempo (secondi reali) con cui la luce segue il cambio di meteo.
const WEATHER_LIGHT_TAU: f32 = 4.0;

/// Stato della luce "di meteo" di `update_sky`: i moltiplicatori inseguono il
/// valore del preset con una transizione morbida, così non scattano.
pub struct WeatherLightState {
    sun: f32,
    ambient: f32,
    tint: [f32; 3],
    fog_bright: f32,
    fog_gray: f32,
    fog_range: f32,
    /// Nebbia originale (start, end) catturata la prima volta, per riportarla com'era.
    base_fog: Option<(f32, f32)>,
    fog_touched: bool,
    snapped: bool,
}

impl Default for WeatherLightState {
    fn default() -> Self {
        let n = WeatherLighting::NEUTRAL;
        Self {
            sun: n.sun,
            ambient: n.ambient,
            tint: n.tint,
            fog_bright: n.fog_bright,
            fog_gray: n.fog_gray,
            fog_range: n.fog_range,
            base_fog: None,
            fog_touched: false,
            snapped: false,
        }
    }
}

pub struct SkyLocal {
    pub dome_timer: f32,
    pub dream_hidden: bool,
}

impl Default for SkyLocal {
    fn default() -> Self {
        Self {
            dome_timer: 0.0,
            dream_hidden: false,
        }
    }
}

/// Handle dei materiali della vecchia cupola/nuvole/DREAM. La risorsa esiste
/// ancora perché `setup_sky` la crea e `main.rs` la passa come argomento in
/// `setup_sky`, ma `update_sky` non la usa più.
#[derive(Resource)]
pub struct SkyHandles {
    pub dome: Handle<Mesh>,
    pub stars: Handle<StandardMaterial>,
    pub clouds: Handle<StandardMaterial>,
    pub dream_overlay_mat: Handle<StandardMaterial>,
    pub dream_overcast_mat: Handle<StandardMaterial>,
}

// =============================================================================
// Setup: crea SkyClock + DirectionalLight + le vecchie entità
// =============================================================================

pub fn setup_sky(
    mut commands: Commands,
    config: Res<SkyConfig>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let hours = config.hour.rem_euclid(24.0);
    let date = GameDate::new(405, config.start_month, config.start_day, hours);
    let _st = sky_state(hours);
    commands.insert_resource(SkyClock {
        date,
        hours,
        hours_per_second: if config.day_minutes > 0.0 {
            24.0 / (config.day_minutes * 60.0)
        } else {
            0.0
        },
        paused: config.day_minutes <= 0.0,
    });

    // --- Vecchia cupola procedurale (nascosta) ---
    let mut dome = Sphere::new(DOME_RADIUS).mesh().uv(64, 32);
    let colors = match dome.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(positions)) => positions
        .iter()
        .map(|_| [0.5, 0.5, 0.5, 1.0])
        .collect(),
        _ => Vec::new(),
    };
    if !colors.is_empty() {
        dome.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    }
    let dome_handle = meshes.add(dome);
    commands.spawn((
        Mesh3d(dome_handle.clone()),
                    MeshMaterial3d(materials.add(StandardMaterial {
                        cull_mode: Some(Face::Front),
                                                 ..unlit(Color::WHITE)
                    })),
                    Transform::default(),
                    SkyPart::Dome,
                    Visibility::Hidden,
    ));

    // --- Sole e luna (nascosti) ---
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

    // --- Stelle (nascoste) ---
    let stars = generate_stars(STAR_COUNT, 20_240_607, STARS_RADIUS);
    let mut star_mesh =
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    star_mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, stars.positions);
    star_mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, stars.colors);
    star_mesh.insert_indices(Indices::U32(stars.indices));
    let star_material = materials.add(StandardMaterial {
        alpha_mode: AlphaMode::Blend,
        cull_mode: None,
        depth_bias: STARS_DEPTH_BIAS,
        ..unlit(Color::linear_rgba(1.0, 1.0, 1.0, 0.0))
    });
    commands.spawn((
        Mesh3d(meshes.add(star_mesh)),
                    MeshMaterial3d(star_material.clone()),
                    Transform::default(),
                    Visibility::Hidden,
                    SkyPart::Stars,
    ));

    // --- Nuvole (nascoste) ---
    let chain = mip_chain_rgba(
        &generate_cloud_texture(CLOUD_TEXTURE),
                               CLOUD_TEXTURE,
                               CLOUD_TEXTURE,
                               CLOUD_MIPS,
    );
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
    let grid = cloud_grid(CLOUD_RADIUS, 40, CLOUD_TILE);
    let mut cloud_mesh =
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
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
                    Visibility::Hidden,
                    SkyPart::Clouds,
    ));

    // --- Layer DREAM (nascosto) ---
    let placeholder_img = Image::new(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0u8, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    let dream_placeholder = images.add(placeholder_img);
    let dream_mat = materials.add(StandardMaterial {
        base_color: Color::linear_rgba(1.0, 1.0, 1.0, 0.0),
                                  base_color_texture: Some(dream_placeholder),
                                  alpha_mode: AlphaMode::Blend,
                                  unlit: true,
                                  cull_mode: None,
                                  fog_enabled: false,
                                  ..default()
    });
    let dream_mesh = meshes.add(Sphere::new(DOME_RADIUS * 0.78).mesh().uv(64, 32));
    commands.spawn((
        Mesh3d(dream_mesh.clone()),
                    MeshMaterial3d(dream_mat.clone()),
                    Transform::default(),
                    Visibility::Hidden,
                    SkyPart::DreamOverlay,
    ));
    commands.spawn((
        Mesh3d(dream_mesh),
                    MeshMaterial3d(dream_mat.clone()),
                    Transform::default(),
                    Visibility::Hidden,
                    SkyPart::DreamOvercast,
    ));
    commands.insert_resource(SkyOvercast::default());

    // --- DirectionalLight (SUNLIGHT) ---
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
        dream_overlay_mat: dream_mat.clone(),
                             dream_overcast_mat: dream_mat,
    });

    info!("Cielo: setup_sky completato (solo orologio e DirectionalLight attivi)");
}

// =============================================================================
// Update: SOLO illuminazione e orologio
// =============================================================================

pub fn update_sky(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut clock: ResMut<SkyClock>,
    mut ambient: ResMut<AmbientLight>,
    mut clear: ResMut<ClearColor>,
    mut light: Query<(&mut DirectionalLight, &mut Transform), With<SunLight>>,
                  mut fog: Query<&mut DistanceFog>,
    preset: Option<Res<CurrentPreset>>,
    interior: Option<Res<crate::interior::InteriorState>>,
    dungeon: Option<Res<crate::dungeon::DungeonState>>,
    mut weather_light: Local<WeatherLightState>,
) {
    let dt = time.delta_secs();

    if keys.just_pressed(KeyCode::KeyP) {
        clock.paused = !clock.paused && clock.hours_per_second > 0.0;
        if clock.hours_per_second == 0.0 {
            clock.hours_per_second = 24.0 / (24.0 * 60.0);
            clock.paused = false;
        }
    }
    if keys.just_pressed(KeyCode::Comma) {
        clock.advance_hours(-1.0);
    }
    if keys.just_pressed(KeyCode::Period) {
        clock.advance_hours(1.0);
    }
    clock.advance_seconds(dt);

    let st = sky_state(clock.hours);
    let lit = lighting(&st);

    // --- Meteo: la scena si scurisce con nuvole, pioggia, neve... ---------------
    // All'aperto segue il preset corrente; in interni e dungeon non si applica
    // (lì l'illuminazione è gestita dai rispettivi moduli).
    let outside = !interior.as_ref().is_some_and(|i| i.is_inside())
        && !dungeon.as_ref().is_some_and(|d| d.is_inside());
    let target = match (&preset, outside) {
        (Some(p), true) => p.current.scene_lighting(),
        _ => WeatherLighting::NEUTRAL,
    };
    // Primo frame: salta direttamente al valore; poi transizione esponenziale.
    let k = if weather_light.snapped { 1.0 - (-dt / WEATHER_LIGHT_TAU).exp() } else { 1.0 };
    weather_light.snapped = true;
    weather_light.sun += (target.sun - weather_light.sun) * k;
    weather_light.ambient += (target.ambient - weather_light.ambient) * k;
    weather_light.fog_bright += (target.fog_bright - weather_light.fog_bright) * k;
    weather_light.fog_gray += (target.fog_gray - weather_light.fog_gray) * k;
    weather_light.fog_range += (target.fog_range - weather_light.fog_range) * k;
    for i in 0..3 {
        weather_light.tint[i] += (target.tint[i] - weather_light.tint[i]) * k;
    }
    let tint = weather_light.tint;

    if let Ok((mut l, mut tf)) = light.get_single_mut() {
        let c = lit.light_color;
        l.illuminance = lit.illuminance * weather_light.sun;
        l.color = Color::linear_rgb(c[0] * tint[0], c[1] * tint[1], c[2] * tint[2]);
        *tf = Transform::default().looking_to(-Vec3::from(lit.light_from), Vec3::Y);
    }

    let a = lit.ambient_color;
    ambient.color = Color::linear_rgb(a[0] * tint[0], a[1] * tint[1], a[2] * tint[2]);
    ambient.brightness = lit.ambient_brightness * weather_light.ambient;

    // Nebbia di distanza: colore dell'ora, desaturato e scurito col maltempo.
    let f = lit.fog_color;
    let gray = 0.299 * f[0] + 0.587 * f[1] + 0.114 * f[2];
    let g = weather_light.fog_gray;
    let b = weather_light.fog_bright;
    let fog_rgb = [
        (f[0] + (gray - f[0]) * g) * b,
        (f[1] + (gray - f[1]) * g) * b,
        (f[2] + (gray - f[2]) * g) * b,
    ];
    let range_changed = (weather_light.fog_range - 1.0).abs() > 0.001;
    for mut fog in &mut fog {
        fog.color = Color::linear_rgb(fog_rgb[0], fog_rgb[1], fog_rgb[2]);

        if WEATHER_FOG_RANGE {
            if let FogFalloff::Linear { start, end } = &mut fog.falloff {
                if range_changed {
                    let base = *weather_light.base_fog.get_or_insert((*start, *end));
                    *start = base.0 * weather_light.fog_range;
                    *end = base.1 * weather_light.fog_range;
                    weather_light.fog_touched = true;
                } else if weather_light.fog_touched {
                    // Tornato sereno: ripristina la nebbia originale una volta sola.
                    if let Some(base) = weather_light.base_fog {
                        *start = base.0;
                        *end = base.1;
                    }
                    weather_light.fog_touched = false;
                }
            }
        }
    }
    let _ = &mut clear; // ClearColor: lo copre la cupola del cielo, non serve toccarlo.
}

// =============================================================================
// No-op (retro-compatibilità con main.rs)
// =============================================================================

/// **No-op**: l'overcast DREAM è disattivato. La funzione esiste solo per
/// non rompere `main.rs` se la chiama ancora.
pub fn drive_overcast_from_weather(
    _data: Res<GameData>,
    mut overcast: ResMut<SkyOvercast>,
) {
    overcast.target = 0.0;
    overcast.level = 0.0;
    overcast.manual_override = None;
}

/// **No-op**: la vecchia texture DREAM non viene più caricata (il layer è
/// nascosto). La funzione esiste solo per non rompere `main.rs` se la chiama
/// ancora in `.add_systems(Update, (...))`.
pub fn update_dream_texture() {}

// =============================================================================
// Helper
// =============================================================================

fn unlit(color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color,
        unlit: true,
        fog_enabled: false,
        ..default()
    }
}

// =============================================================================
// Fix stagione (retro-compatibilità)
// =============================================================================

pub fn season_from_config(config: &SkyConfig) -> Season {
    GameDate::new(405, config.start_month, config.start_day, config.hour).season()
}
