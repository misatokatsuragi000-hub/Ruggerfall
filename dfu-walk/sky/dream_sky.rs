//! Cielo procedurale Dynamic Skies (DREAM-SKY) con `DreamSkyMaterial`.
//!
//! ## Cosa fa
//!
//! - Spawna una sfera raggio `SKY_DOME_RADIUS` centrata sulla camera con
//!   `DreamSkyMaterial`.
//! - Carica le texture del preset dal `.dfmod` via `ModTextures` (cloud top/bottom
//!   diffuse + normal, stelle + twinkle mask + noise, Masser, Secunda). Se una
//!   texture manca, crea un fallback 1×1 (bianco, nero o normale piatto) e logga
//!   un warning una volta sola.
//! - Aggiorna `world_time` (per l'orbita delle lune e lo scrolling delle nuvole) e
//!   `sun_dir` (per lo scattering atmosferico) ad ogni frame.
//!
//! ## Visibilità della cupola
//!
//! `update_dome_visibility` nasconde `DreamSkyDome` quando il player è in un
//! **dungeon** o in un **interno** (vedi `InteriorState::is_inside()` e
//! `DungeonState::is_inside()`). In quei casi `sky::update_sky` imposta
//! `ClearColor::BLACK` e luce ambiente/sole bassi: vedere il cielo di
//! superficie dentro una cripta sarebbe sbagliato, e questa funzione lo
//! impedisce. All'uscita torna `Visibility::Visible` e il cielo procedurale
//! riprende a funzionare normalmente.
//!
//! ## Fix striscia blu all'orizzonte
//!
//! Il mare in `ocean.rs::update_ocean_lighting` usa `sky_state(clock.hours).horizon`
//! (colore astronomico dell'orizzonte) per la sua nebbia (`p.fog = horizon`), mentre
//! il DreamSky aveva `ground_color`/`fog_day`/`fog_night` fissi dai preset: i due
//! colori divergevano → striscia netta tra mare e cielo. `update_dream_sky_uniforms`
//! ora sovrascrive ogni frame quei tre uniform partendo da `horizon`.
//!
//! ## Fix nebbia allineata al meteo
//!
//! `horizon` da solo è il colore astronomico dell'ora: azzurro di giorno anche sotto
//! un temporale. Viene mixato con `fog_day` del preset meteo usando `fog_gray` come
//! fattore (lo stesso mix che `sky.rs::update_sky` applica alla `DistanceFog`):
//! sereno = colore dell'ora, pioggia = grigio scuro, neve = lattiginoso.
//!
//! ## Sole e lune nascosti col maltempo
//!
//! - **Sole**: `sun_size`/`sun_convergence` seguono il fade `sun_hide` (0 = visibile,
//!   1 = nascosto), che sale a 1 con Pioggia/Temporale/Neve (`is_rain() || is_snow()`).
//! - **Lune**: `masser_color`/`secunda_color`/`moon_night_color` seguono il fade
//!   `moon_hide`, che insegue `cover` (= `fog_gray`): si spengono col maltempo sia di
//!   giorno sia di notte (e, per coerenza fisica, anche con Coperto/Nebbia).
//! - Entrambi i fade sono esponenziali (tau ~2 s): niente "pop" al cambio di meteo.
//!
//! ## Notte col maltempo
//!
//! Di notte `exposure` e i moltiplicatori diurni sono inerti (la base è già nera),
//! quindi la copertura notturna si esprime alzando le nuvole a muro opaco (che
//! occludono stelle e lune), spingendo `fog_night`/`ground_color` verso
//! `NIGHT_OVERCAST` e spegnendo le lune. Il fattore è `night_cover = cover * night`.
//!
//! ## Idempotenza
//!
//! I valori base di sole/nuvole/lune/fog si leggono dal **preset** (`pr`), mai da
//! `mat.uniforms`: altrimenti il `lerp` cumulato frame-su-frame li farebbe derivare
//! a zero anche col sereno.
//!
//! - Non sceglie il meteo: legge `CurrentPreset.current`, che scrive
//!   `calendar::sync_season_from_calendar`.
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::pbr::{MaterialPlugin, NotShadowCaster};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::render::view::NoFrustumCulling;
use super::dream_sky_material::{DreamSkyMaterial, SkyboxUniforms, DREAM_SKY_SHADER};
use super::presets::{preset_for, SkyboxPreset, WeatherPreset};
use super::SkyClock;
use crate::data::sky::{sky_state, sun_direction};
use crate::mod_textures::ModTextures;
use crate::player::Player;

/// Raggio della sfera del cielo, in metri.
const SKY_DOME_RADIUS: f32 = 20_000.0;
const SKY_FOLLOW_CAMERA: bool = true;

/// Fondo del cielo notturno coperto (lineare): grigio-bluastro leggibile, non nero.
/// Con maltempo di notte la cupola converge qui invece che al nero stellato del sereno.
const NIGHT_OVERCAST: Vec3 = Vec3::new(0.05, 0.06, 0.08);

/// Lerp scalare (f32 non ha `.lerp` inherent in Rust; Vec3 sì, è metodo Bevy).
#[inline]
fn lf(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

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
            .find(|l| l.starts_with("const DEBUG_MODE "))
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
             // NUOVO: nasconde la cupola quando il player è in dungeon o in interno.
             update_dome_visibility,
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

/// Nasconde la cupola del cielo quando il player è in un dungeon o in un
/// interno. In dungeon, `sky::update_sky` ha già spento il sole e messo
/// `ClearColor::BLACK`: senza questo nascondiglio, la cupola
/// (r = `SKY_DOME_RADIUS`, `NoFrustumCulling`, sempre visibile) coprirebbe il
/// nero e il giocatore vedrebbe il cielo di superficie dentro la cripta.
///
/// Quando il player esce, torna `Visibility::Visible` e il DreamSky riprende
/// a funzionare normalmente (uniform, presets, lune, stelle: tutto invariato).
fn update_dome_visibility(
    interior: Option<Res<crate::interior::InteriorState>>,
    dungeon: Option<Res<crate::dungeon::DungeonState>>,
    mut domes: Query<&mut Visibility, With<DreamSkyDome>>,
) {
    let inside = interior.as_ref().is_some_and(|i| i.is_inside())
    || dungeon.as_ref().is_some_and(|d| d.is_inside());
    let target = if inside { Visibility::Hidden } else { Visibility::Visible };
    for mut vis in &mut domes {
        if *vis != target {
            *vis = target;
        }
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
        info!("Cielo: materiale sostituito per preset {:?}", preset.current);
    }

    preset.last_applied = Some(preset.current);
}

/// Aggiorna gli uniform del cielo ad ogni frame. Vedi il doc del modulo per le
/// singole fix (striscia blu, nebbia meteo, sole/lune nascosti, notte coperta).
fn update_dream_sky_uniforms(
    time: Res<Time>,
    clock: Res<SkyClock>,
    preset: Option<Res<CurrentPreset>>,
    mut materials: ResMut<Assets<DreamSkyMaterial>>,
    dome: Query<&MeshMaterial3d<DreamSkyMaterial>, With<DreamSkyDome>>,
    mut sun_hide: Local<f32>,
    mut moon_hide: Local<f32>,
) {
    let Ok(mat_handle) = dome.get_single() else { return; };
    let Some(mat) = materials.get_mut(&mat_handle.0) else { return; };

    mat.uniforms.world_time = time.elapsed_secs();
    mat.uniforms.sun_dir = Vec3::from(sun_direction(clock.hours));

    let st = sky_state(clock.hours);
    let horizon = Vec3::from(st.horizon);
    let night = 1.0 - st.day.clamp(0.0, 1.0);

    // Valori base dal preset (NON da mat.uniforms: evita deriva cumulativa).
    let pr = match preset.as_ref() {
        Some(p) => preset_for(p.current),
        None => preset_for(WeatherPreset::Sunny),
    };
    // "Quanto il meteo copre il cielo": 0 sereno, 0.85 pioggia/neve, 0.90 temporale.
    let cover = match preset.as_ref() {
        Some(p) => p.current.scene_lighting().fog_gray.clamp(0.0, 1.0),
        None => 0.0,
    };
    let night_cover = cover * night;

    // Transizione esponenziale tau ~2 s, condivisa da sole e lune.
    let k = 1.0 - (-time.delta_secs() / 2.0).exp();

    // --- 1. Sole nascosto con pioggia/temporale/neve -----------------------
    let sun_target = match preset.as_ref() {
        Some(p) if p.current.is_rain() || p.current.is_snow() => 1.0,
        _ => 0.0,
    };
    *sun_hide += (sun_target - *sun_hide) * k;
    mat.uniforms.sun_size = pr.sun_size * (1.0 - *sun_hide);
    mat.uniforms.sun_convergence = pr.sun_convergence * (1.0 + 7.0 * *sun_hide);

    // --- 2. Lune nascoste col maltempo (giorno e notte) --------------------
    // Insegue `cover`: si spengono con pioggia/diluvio/neve (0.85-0.90) e,
    // per coerenza fisica, anche con coperto/nebbia. Per limitarle ai soli
    // tre casi, sostituisci `cover` con un target binario is_rain()||is_snow().
    *moon_hide += (cover - *moon_hide) * k;
    mat.uniforms.moon_night_color =
    Vec3::from_array(pr.moon_night_color).lerp(Vec3::ZERO, *moon_hide);
    mat.uniforms.masser_color =
    Vec3::from_array(pr.masser.color).lerp(Vec3::ZERO, *moon_hide);
    mat.uniforms.secunda_color =
    Vec3::from_array(pr.secunda.color).lerp(Vec3::ZERO, *moon_hide);

    // --- 3. Nebbia diurna allineata al meteo -------------------------------
    mat.uniforms.fog_day = horizon * (1.0 - cover) + Vec3::from_array(pr.fog_day) * cover;

    // --- 4. Notte: maltempo = cielo coperto, non nero stellato -------------
    mat.uniforms.fog_night =
    Vec3::from_array(pr.fog_night).lerp(NIGHT_OVERCAST, night_cover);
    mat.uniforms.ground_color = horizon.lerp(NIGHT_OVERCAST, night_cover);

    // Nuvole a muro: di notte col maltempo coprono stelle e lune.
    mat.uniforms.cloud_top_opacity = lf(pr.top_clouds.opacity, 0.95, night_cover);
    mat.uniforms.cloud_bottom_opacity = lf(pr.bottom_clouds.opacity, 0.92, night_cover);
    mat.uniforms.cloud_top_alpha_max = lf(pr.top_clouds.alpha_max, 0.90, night_cover);
    mat.uniforms.cloud_bottom_alpha_max = lf(pr.bottom_clouds.alpha_max, 0.95, night_cover);
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
        p.top_clouds.texture, "cloud_top_diffuse", [255, 255, 255, 255], mods, images,
    );
    let cloud_top_normal = load_texture(
        p.top_clouds.normal_texture, "cloud_top_normal", [128, 128, 255, 255], mods, images,
    );
    let cloud_bottom_diffuse = load_texture(
        p.bottom_clouds.texture, "cloud_bottom_diffuse", [255, 255, 255, 255], mods, images,
    );
    let cloud_bottom_normal = load_texture(
        p.bottom_clouds.normal_texture, "cloud_bottom_normal", [128, 128, 255, 255], mods, images,
    );
    let star_tex = load_texture(p.stars.texture, "star_tex", [0, 0, 0, 255], mods, images);
    let star_twinkle_mask = load_texture(
        p.stars.twinkle_mask, "star_twinkle_mask", [255, 255, 255, 255], mods, images,
    );
    let twinkle_noise = load_texture(
        p.stars.twinkle_noise, "twinkle_noise", [255, 255, 255, 255], mods, images,
    );
    let moon_tex = load_texture(
        p.masser.texture, "moon_tex (Masser)", [255, 255, 255, 255], mods, images,
    );
    let secunda_tex = load_texture(
        p.secunda.texture, "secunda_tex (Secunda)", [255, 255, 255, 255], mods, images,
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
        Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
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
        cloud_top_opacity_pulse_speed: p.top_clouds.opacity_pulse_speed,
        cloud_bottom_opacity_pulse_speed: p.bottom_clouds.opacity_pulse_speed,
    }
}
