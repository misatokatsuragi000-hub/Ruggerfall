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
//! ## Buio del dungeon proporzionale alla stanza
//!
//! Se `DungeonState::is_inside()` è vero, `update_sky` calcola un **fattore**
//! in base alla dimensione della stanza in cui si trova il player: 16 raggi
//! orizzontali con `clearance`, accoppiati in 8 **corde** (raggio + raggio
//! opposto, quindi la misura non dipende da dove sta il player nella stanza);
//! la dimensione e' la **seconda corda piu' corta** (ignora un pilastro o una
//! porta aperta che falsano una sola direzione), poi curva `smoothstep^exponent`
//! fra `dungeon_room_dim_min` e `dungeon_room_dim_max`.
//!
//! ## Luce "sole" nel dungeon: segue la camera
//!
//! Il sole del dungeon e' una `DirectionalLight` senza ombre: illumina anche
//! attraverso muri e soffitti, con una direzione che restava quella dell'ultimo
//! frame all'aperto. Le facce a shading `N.L` risultavano quindi molto piu'
//! chiare o scure a seconda di come erano orientate rispetto a una direzione
//! arbitraria (e dell'ora del giorno all'ingresso). Con `dungeon_headlamp` la
//! luce guarda nella direzione in cui guarda il player, inclinata verso il
//! basso di `dungeon_headlamp_pitch` gradi: tutte le superfici che vedi ricevono
//! una luce coerente, e il fattore stanza resta l'unico che decide la luminosita'.
//! Il fattore moltiplica `dungeon_sun_illuminance` e
//! `dungeon_ambient_brightness`.
//!
//! Con i default (dim 3–20 m, factor_min 0.03, exponent 1.5):
//! - tunnel e stanze < 6 m: fattore 0.03 → quasi buio pesto (10 ambient, 105 sole)
//! - stanze 8–12 m: fattore 0.09–0.43 → media luce
//! - stanze 15 m: fattore 0.76 → chiara
//! - sala del trono 20 m+: fattore 1.0 → luce piena (350 ambient, 3500 sole)
//!
//! Il probe è **rumoroso** (pilastri, cornici, archi possono far "vedere" o
//! "non vedere" un muro a seconda del pixel): per questo il fattore viene
//! **smussato esponenzialmente** (`FACTOR_SMOOTH_TAU ≈ 0.4 s`) prima di
//! essere applicato alla luce. Al primo frame in un nuovo dungeon il fattore
//! smoothed parte **direttamente dal valore grezzo** (non da 0), così non
//! c'è un fade-in indesiderato.
//!
//! ## Override manuali (menu F1, scheda "Luce cielo")
//!
//! `SkyLightTuning` può essere modificata dal menu F1 e salvata su
//! `sky_settings.txt`. `update_sky` la legge con `Option<Res<...>>`: se la
//! risorsa non esiste usa il comportamento astronomico normale.
//!
//! ## Nebbia di distanza allineata al cielo
//!
//! Il colore della nebbia è ottenuto mixando il colore dell'orizzonte
//! calcolato dall'ora del giorno con il colore del fog del preset meteo
//! corrente. Il fattore di mix è `WeatherLighting::fog_gray` del preset.

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
    sky_state, SkyState, lin,
};
use std::path::Path;

// =============================================================================
// Sottomoduli (esposti a main.rs)
// =============================================================================

pub mod dream_sky;
pub mod dream_sky_material;
pub mod presets;

// =============================================================================
// Costanti
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

// --- Buio stanza: costanti del probe ---
const ROOM_PROBE_MAX: f32 = 40.0;
/// Numero di raggi (pari): `ROOM_RAYS / 2` corde.
const ROOM_RAYS: usize = 16;
/// Passo grossolano del raggio, poi affinato a `ROOM_PROBE_FINE` sull'ultimo tratto.
const ROOM_PROBE_COARSE: f32 = 1.0;
const ROOM_PROBE_FINE: f32 = 0.25;
/// Quale corda (0 = la piu' corta) vale come dimensione della stanza.
const ROOM_CHORD_RANK: usize = 1;
const ROOM_PROBE_CLEARANCE_CAP: f32 = 1.0;
const ROOM_PROBE_BLOCK_THRESHOLD: f32 = 0.6;
const ROOM_CACHE_INTERVAL_SECS: f32 = 0.3;
const ROOM_CACHE_MOVE_SQ: f32 = 1.0;

/// Costante di tempo dello smoothing esponenziale del fattore stanza.
/// Più alta = transizioni più lente e morbide. 0.4 s è un buon compromesso:
/// abbastanza per assorbire il rumore del probe, abbastanza basso per
/// accorgersi del passaggio stanza→corridoio.
const FACTOR_SMOOTH_TAU: f32 = 0.4;

// =============================================================================
// TUNING MANUALE (menu F1)
// =============================================================================

pub const SKY_SETTINGS_FILE: &str = "sky_settings.txt";

/// Override manuali di `DirectionalLight` (sole) e `AmbientLight`, più i
/// parametri del buio del dungeon.
#[derive(Resource, Debug, Clone)]
pub struct SkyLightTuning {
    pub sun_illuminance_override: Option<f32>,
    pub sun_color_override: Option<[f32; 3]>,
    pub ambient_brightness_override: Option<f32>,
    pub ambient_color_override: Option<[f32; 3]>,

    pub dungeon_sun_illuminance: f32,
    pub dungeon_ambient_brightness: f32,
    pub dungeon_ambient_color: [f32; 3],
    pub dungeon_fog_start: f32,
    pub dungeon_fog_end: f32,

    pub dungeon_room_scale_enabled: bool,
    pub dungeon_room_dim_min: f32,
    pub dungeon_room_dim_max: f32,
    pub dungeon_room_factor_min: f32,
    /// Esponente applicato alla curva smoothstep. 1.0 = smoothstep puro,
    /// > 1 = più selettiva (favorisce le stanze grandi, penalizza le medie),
    /// < 1 = più permissiva.
    pub dungeon_room_curve_exponent: f32,

    /// Nel dungeon la luce "sole" segue la direzione dello sguardo (vedi doc del modulo).
    pub dungeon_headlamp: bool,
    /// Inclinazione verso il basso della luce (gradi): 0 = orizzontale, 90 = dall'alto.
    pub dungeon_headlamp_pitch: f32,
}

impl Default for SkyLightTuning {
    fn default() -> Self {
        Self {
            sun_illuminance_override: None,
            sun_color_override: None,
            ambient_brightness_override: None,
            ambient_color_override: None,

            // Valori "tetto" (fattore 1.0) alzati per la sala del trono:
            // 350 ambient (2.3× rispetto a prima) e 3500 sole.
            dungeon_sun_illuminance: 3500.0,
            dungeon_ambient_brightness: 350.0,
            dungeon_ambient_color: [0.15, 0.14, 0.20],
            dungeon_fog_start: 30.0,
            dungeon_fog_end: 150.0,

            // Curva più selettiva: solo stanze > 12 m ricevono luce media,
            // solo 20 m+ ricevono luce piena. Tunnel e stanze piccole: buio.
            dungeon_room_scale_enabled: true,
            dungeon_room_dim_min: 3.0,
            dungeon_room_dim_max: 20.0,
            dungeon_room_factor_min: 0.03,
            dungeon_room_curve_exponent: 1.5,

            dungeon_headlamp: true,
            dungeon_headlamp_pitch: 35.0,
        }
    }
}

fn fopt(v: Option<f32>) -> String {
    v.map_or_else(|| "none".to_string(), |x| format!("{x}"))
}

fn copt(v: Option<[f32; 3]>) -> String {
    v.map_or_else(|| "none".to_string(), |c| format!("{},{},{}", c[0], c[1], c[2]))
}

fn parse_fopt(s: &str) -> Option<Option<f32>> {
    let t = s.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("none") {
        Some(None)
    } else {
        t.parse().ok().map(Some)
    }
}

fn parse_copt(s: &str) -> Option<Option<[f32; 3]>> {
    let t = s.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("none") {
        return Some(None);
    }
    let p: Vec<&str> = t.split(',').map(str::trim).collect();
    if p.len() != 3 {
        return None;
    }
    Some(Some([p[0].parse().ok()?, p[1].parse().ok()?, p[2].parse().ok()?]))
}

fn parse_color_or(s: &str, default: [f32; 3]) -> [f32; 3] {
    let p: Vec<&str> = s.split(',').map(str::trim).collect();
    if p.len() != 3 {
        return default;
    }
    [
        p[0].parse().unwrap_or(default[0]),
        p[1].parse().unwrap_or(default[1]),
        p[2].parse().unwrap_or(default[2]),
    ]
}

impl SkyLightTuning {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            info!(
                "SkyLightTuning: {} non trovato, uso i valori predefiniti (F1 per regolare)",
                  path.display()
            );
            return Self::default();
        };
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Self {
        let mut t = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "sun_illuminance_override" => {
                    if let Some(o) = parse_fopt(v) { t.sun_illuminance_override = o; }
                }
                "sun_color_override" => {
                    if let Some(o) = parse_copt(v) { t.sun_color_override = o; }
                }
                "ambient_brightness_override" => {
                    if let Some(o) = parse_fopt(v) { t.ambient_brightness_override = o; }
                }
                "ambient_color_override" => {
                    if let Some(o) = parse_copt(v) { t.ambient_color_override = o; }
                }
                "dungeon_sun_illuminance" => {
                    if let Ok(n) = v.parse() { t.dungeon_sun_illuminance = n; }
                }
                "dungeon_ambient_brightness" => {
                    if let Ok(n) = v.parse() { t.dungeon_ambient_brightness = n; }
                }
                "dungeon_ambient_color" => {
                    t.dungeon_ambient_color = parse_color_or(v, t.dungeon_ambient_color);
                }
                "dungeon_fog_start" => {
                    if let Ok(n) = v.parse() { t.dungeon_fog_start = n; }
                }
                "dungeon_fog_end" => {
                    if let Ok(n) = v.parse() { t.dungeon_fog_end = n; }
                }
                "dungeon_room_scale_enabled" => {
                    t.dungeon_room_scale_enabled =
                    matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
                }
                "dungeon_room_dim_min" => {
                    if let Ok(n) = v.parse() { t.dungeon_room_dim_min = n; }
                }
                "dungeon_room_dim_max" => {
                    if let Ok(n) = v.parse() { t.dungeon_room_dim_max = n; }
                }
                "dungeon_room_factor_min" => {
                    if let Ok(n) = v.parse() { t.dungeon_room_factor_min = n; }
                }
                "dungeon_room_curve_exponent" => {
                    if let Ok(n) = v.parse() { t.dungeon_room_curve_exponent = n; }
                }
                "dungeon_headlamp" => {
                    t.dungeon_headlamp =
                    matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on");
                }
                "dungeon_headlamp_pitch" => {
                    if let Ok(n) = v.parse() { t.dungeon_headlamp_pitch = n; }
                }
                _ => {}
            }
        }
        t
    }

    pub fn to_text(&self) -> String {
        let mut s = String::new();
        s.push_str("# Override manuali del cielo (menu F1). Modifica e riavvia.\n");
        s.push_str("# `none` = usa il valore astronomico/meteo calcolato.\n");
        s.push_str("# I colori sono in sRGB 0..1, separati da virgola: \"r,g,b\".\n\n");
        s.push_str(&format!("sun_illuminance_override = {}\n", fopt(self.sun_illuminance_override)));
        s.push_str(&format!("sun_color_override = {}\n", copt(self.sun_color_override)));
        s.push_str(&format!("ambient_brightness_override = {}\n", fopt(self.ambient_brightness_override)));
        s.push_str(&format!("ambient_color_override = {}\n\n", copt(self.ambient_color_override)));
        s.push_str("# Buio del dungeon (applicato quando DungeonState::is_inside())\n");
        s.push_str("# I valori di sole/ambient sono MOLTIPLICATI per il fattore stanza.\n");
        s.push_str(&format!("dungeon_sun_illuminance = {}\n", self.dungeon_sun_illuminance));
        s.push_str(&format!("dungeon_ambient_brightness = {}\n", self.dungeon_ambient_brightness));
        s.push_str(&format!(
            "dungeon_ambient_color = {},{},{}\n",
            self.dungeon_ambient_color[0],
            self.dungeon_ambient_color[1],
            self.dungeon_ambient_color[2]
        ));
        s.push_str(&format!("dungeon_fog_start = {}\n", self.dungeon_fog_start));
        s.push_str(&format!("dungeon_fog_end = {}\n\n", self.dungeon_fog_end));
        s.push_str("# Curva stanza: piu' grande la stanza, piu' luce.\n");
        s.push_str("# `dungeon_room_dim_min` = dimensione (m) sotto cui il fattore e' minimo\n");
        s.push_str("# `dungeon_room_dim_max` = dimensione (m) sopra cui il fattore e' 1.0\n");
        s.push_str("# `dungeon_room_curve_exponent` = 1.0 = smoothstep puro, >1 = piu' selettiva\n");
        s.push_str(&format!("dungeon_room_scale_enabled = {}\n", self.dungeon_room_scale_enabled as u32));
        s.push_str(&format!("dungeon_room_dim_min = {}\n", self.dungeon_room_dim_min));
        s.push_str(&format!("dungeon_room_dim_max = {}\n", self.dungeon_room_dim_max));
        s.push_str(&format!("dungeon_room_factor_min = {}\n", self.dungeon_room_factor_min));
        s.push_str(&format!("dungeon_room_curve_exponent = {}\n\n", self.dungeon_room_curve_exponent));
        s.push_str("# Luce sole nel dungeon: 1 = segue lo sguardo del player (inclinata verso il\n");
        s.push_str("# basso di `dungeon_headlamp_pitch` gradi), 0 = direzione fissa dell'ultimo frame\n");
        s.push_str("# all'aperto (illuminazione dipendente dall'orientamento delle facce).\n");
        s.push_str(&format!("dungeon_headlamp = {}\n", self.dungeon_headlamp as u32));
        s.push_str(&format!("dungeon_headlamp_pitch = {}\n", self.dungeon_headlamp_pitch));
        s
    }

    pub fn load_or_default(path: &Path) -> Self {
        Self::load(path)
    }
}

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

#[derive(Resource, Default)]
pub struct SkyOvercast {
    pub target: f32,
    pub level: f32,
    pub manual_override: Option<f32>,
}

const WEATHER_FOG_RANGE: bool = true;
const WEATHER_LIGHT_TAU: f32 = 4.0;

pub struct WeatherLightState {
    sun: f32,
    ambient: f32,
    tint: [f32; 3],
    fog_bright: f32,
    fog_gray: f32,
    fog_range: f32,
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

/// Cache del fattore stanza.
///
/// - `raw_factor`: ultimo valore calcolato dal probe (aggiornato ogni 0.3 s
///   o quando il player si sposta di più di 1 m). È **rumoroso**: pilastri,
///   cornici, archi possono far "vedere" o "non vedere" un muro a seconda
///   del pixel, quindi piccoli spostamenti producono salti.
/// - `smoothed_factor`: valore inseguito esponenzialmente ogni frame da
///   `update_sky` con costante di tempo `FACTOR_SMOOTH_TAU`. È **questo**
///   che viene applicato alla luce: assorbe il rumore del probe e rende
///   morbide le transizioni stanza→corridoio.
///
/// **Deve essere `pub`**: appare nella firma di `update_sky` come
/// `Local<RoomLightCache>` e Bevy richiede che il tipo sia visibile dal
/// codice generato. I campi possono restare privati.
#[derive(Default)]
pub struct RoomLightCache {
    pos: Vec3,
    raw_factor: f32,
    smoothed_factor: f32,
    timer: f32,
    initialized: bool,
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

#[derive(Resource)]
pub struct SkyHandles {
    pub dome: Handle<Mesh>,
    pub stars: Handle<StandardMaterial>,
    pub clouds: Handle<StandardMaterial>,
    pub dream_overlay_mat: Handle<StandardMaterial>,
    pub dream_overcast_mat: Handle<StandardMaterial>,
}

// =============================================================================
// Setup
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

    commands.init_resource::<SkyLightTuning>();

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
// Calcolo del fattore stanza (dungeon)
// =============================================================================

/// Risultato del probe stanza.
struct RoomProbe {
    /// Dimensione della stanza (m): la corda di rango `ROOM_CHORD_RANK`.
    size: f32,
    /// La corda piu' corta (m), solo per il log di debug.
    shortest: f32,
    /// La corda piu' lunga (m), solo per il log di debug.
    longest: f32,
}

/// Distanza libera (m) dal punto (x, z) lungo la direzione `dir`: avanza a passo
/// grossolano finche' `clearance` dice "muro", poi affina l'ultimo tratto.
fn ray_free_distance(
    d: &crate::data::dungeon_builder::BuiltDungeon,
    x: f32,
    z: f32,
    feet_y: f32,
    dir: (f32, f32),
) -> f32 {
    let blocked = |t: f32| {
        d.clearance(x + dir.0 * t, z + dir.1 * t, feet_y, ROOM_PROBE_CLEARANCE_CAP)
            < ROOM_PROBE_BLOCK_THRESHOLD
    };
    let mut free = 0.0f32;
    let mut t = ROOM_PROBE_COARSE;
    while t <= ROOM_PROBE_MAX {
        if blocked(t) {
            let mut f = free + ROOM_PROBE_FINE;
            while f < t {
                if blocked(f) {
                    break;
                }
                free = f;
                f += ROOM_PROBE_FINE;
            }
            return free;
        }
        free = t;
        t += ROOM_PROBE_COARSE;
    }
    free
}

/// Misura la dimensione dell'ambiente con `ROOM_RAYS` raggi orizzontali.
///
/// Ogni raggio viene accoppiato a quello opposto in una **corda** (somma delle due
/// distanze libere): la corda non dipende da dove sta il player nella stanza (con il
/// vecchio `dist+ + dist-` valeva solo sugli assi). La dimensione e' la seconda corda
/// piu' corta: un pilastro che blocca una direzione, o una porta aperta che ne
/// allunga un'altra, non spostano il risultato.
fn room_probe(
    d: &crate::data::dungeon_builder::BuiltDungeon,
    x: f32,
    z: f32,
    feet_y: f32,
) -> RoomProbe {
    let half = ROOM_RAYS / 2;
    let mut dist = [0.0f32; ROOM_RAYS];
    for (i, slot) in dist.iter_mut().enumerate() {
        let a = i as f32 / ROOM_RAYS as f32 * std::f32::consts::TAU;
        *slot = ray_free_distance(d, x, z, feet_y, (a.cos(), a.sin()));
    }
    let mut chords: Vec<f32> = (0..half).map(|i| dist[i] + dist[i + half]).collect();
    chords.sort_by(|a, b| a.total_cmp(b));
    RoomProbe {
        size: chords[ROOM_CHORD_RANK.min(half - 1)],
        shortest: chords[0],
        longest: chords[half - 1],
    }
}

/// Curva stanza → fattore, con esponente configurabile.
///
/// Formula: `(smoothstep(x))^exponent`, dove `x = clamp((dim - min)/(max - min), 0, 1)`.
/// Con `exponent = 1.0` è lo smoothstep classico; con `> 1` la curva è più
/// ripida nella parte alta (solo stanze grandi ricevono luce piena).
fn room_light_factor(t: &SkyLightTuning, small_dim: f32) -> f32 {
    if !t.dungeon_room_scale_enabled {
        return 1.0;
    }
    let lo = t.dungeon_room_dim_min;
    let hi = t.dungeon_room_dim_max;
    if hi <= lo {
        return 1.0;
    }
    let x = ((small_dim - lo) / (hi - lo)).clamp(0.0, 1.0);
    let smooth = x * x * (3.0 - 2.0 * x);
    let exp = t.dungeon_room_curve_exponent.max(0.1);
    let curved = smooth.powf(exp);
    curved.max(t.dungeon_room_factor_min).clamp(0.0, 1.0)
}

// =============================================================================
// Update
// =============================================================================

fn srgb3(c: [f32; 3]) -> [f32; 3] {
    [lin([c[0], 0.0, 0.0])[0], lin([c[1], 0.0, 0.0])[0], lin([c[2], 0.0, 0.0])[0]]
}

#[allow(clippy::too_many_arguments)]
pub fn update_sky(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mut clock: ResMut<SkyClock>,
    mut ambient: ResMut<AmbientLight>,
    mut clear: ResMut<ClearColor>,
    // `Without<Player>` sul `light` è necessario perché più sotto c'è
    // `player: Query<(&Player, &Transform)>`: senza il filtro Bevy non può
    // provare che le due query sono disgiunte su `Transform` e va in panic
    // con `error[B0001]`.
    mut light: Query<(&mut DirectionalLight, &mut Transform), (With<SunLight>, Without<Player>)>,
                  player: Query<(&Player, &Transform)>,
                  mut fog: Query<&mut DistanceFog>,
                  preset: Option<Res<CurrentPreset>>,
                  interior: Option<Res<crate::interior::InteriorState>>,
                  dungeon: Option<Res<crate::dungeon::DungeonState>>,
                  tuning: Option<Res<SkyLightTuning>>,
                  mut weather_light: Local<WeatherLightState>,
                  mut room_cache: Local<RoomLightCache>,
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

    let t = tuning.as_deref().cloned().unwrap_or_default();

    // --- BUIO DEL DUNGEON ---------------------------------------------------
    let in_dungeon = dungeon.as_ref().is_some_and(|d| d.is_inside());
    if in_dungeon {
        // 1. Calcola (o riusa dalla cache) il fattore stanza.
        //
        // `room_cache` è `Local<RoomLightCache>`: DerefMut porta a
        // `RoomLightCache`, quindi `.timer` è già `f32` (niente `*`).
        let player_fwd = player.get_single().ok().map(|(_, tf)| tf.rotation * Vec3::NEG_Z);
        let factor = match (dungeon.as_ref(), player.get_single()) {
            (Some(d), Ok((p, player_tf))) => match d.active.as_ref() {
                Some(active) => {
                    room_cache.timer += dt;
                    let moved_sq = player_tf.translation.distance_squared(room_cache.pos);
                    let stale = room_cache.timer >= ROOM_CACHE_INTERVAL_SECS
                    || moved_sq > ROOM_CACHE_MOVE_SQ
                    || !room_cache.initialized;
                    if stale {
                        let local_feet = p.feet_y - active.origin.y;
                        let local_x = player_tf.translation.x - active.origin.x;
                        let local_z = player_tf.translation.z - active.origin.z;
                        let probe = room_probe(&active.dungeon, local_x, local_z, local_feet);
                        let small_dim = probe.size;
                        let raw = room_light_factor(&t, small_dim);
                        if !room_cache.initialized {
                            // Primo frame in dungeon: parti direttamente dal
                            // valore grezzo, così non c'è un fade-in.
                            room_cache.smoothed_factor = raw;
                        }
                        room_cache.raw_factor = raw;
                        room_cache.timer = 0.0;
                        room_cache.pos = player_tf.translation;
                        room_cache.initialized = true;
                        if std::env::var_os("DF_DUNGEON_LIGHT_DEBUG").is_some() {
                            info!(
                                "Dungeon light: dim={:.1} (corda min {:.1}, max {:.1}) raw={:.3} smoothed={:.3}",
                                small_dim, probe.shortest, probe.longest, raw, room_cache.smoothed_factor
                            );
                        }
                    }
                    // Smoothing esponenziale ogni frame: assorbe il rumore
                    // del probe, così la luce non "vibra" quando la cache
                    // si aggiorna e il probe misura un valore diverso.
                    let k_smooth = 1.0 - (-dt / FACTOR_SMOOTH_TAU).exp();
                    room_cache.smoothed_factor +=
                    (room_cache.raw_factor - room_cache.smoothed_factor) * k_smooth;
                    room_cache.smoothed_factor
                }
                None => 1.0,
            },
            _ => 1.0,
        };

        // 2. Applica i valori dungeon_* scalati per `factor`.
        if let Ok((mut l, mut tf)) = light.get_single_mut() {
            l.illuminance = t.dungeon_sun_illuminance * factor;
            // La luce segue lo sguardo (solo l'orientamento orizzontale) con un'inclinazione
            // fissa verso il basso: niente piu' facce chiare/scure in base a una direzione
            // arbitraria del sole all'aperto.
            if t.dungeon_headlamp {
                if let Some(fwd) = player_fwd {
                    let flat = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero();
                    if flat != Vec3::ZERO {
                        let pitch = t.dungeon_headlamp_pitch.clamp(0.0, 89.0).to_radians();
                        let dir = flat * pitch.cos() + Vec3::NEG_Y * pitch.sin();
                        *tf = Transform::default().looking_to(dir, Vec3::Y);
                    }
                }
            }
        }
        ambient.brightness = t.dungeon_ambient_brightness * factor;
        ambient.color = Color::srgb(
            t.dungeon_ambient_color[0],
            t.dungeon_ambient_color[1],
            t.dungeon_ambient_color[2],
        );
        clear.0 = Color::BLACK;
        for mut fog in &mut fog {
            fog.color = Color::BLACK;
            if let FogFalloff::Linear { start, end } = &mut fog.falloff {
                *start = t.dungeon_fog_start;
                *end = t.dungeon_fog_end;
            }
        }
        weather_light.snapped = false;
        return;
    }

    room_cache.initialized = false;

    let st = sky_state(clock.hours);
    let lit = lighting(&st);

    // --- Meteo --------------------------------------------------------------
    let outside = !interior.as_ref().is_some_and(|i| i.is_inside());
    let target = match (&preset, outside) {
        (Some(p), true) => p.current.scene_lighting(),
        _ => WeatherLighting::NEUTRAL,
    };
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

    // --- Sole ---------------------------------------------------------------
    if let Ok((mut l, mut tf)) = light.get_single_mut() {
        let c = lit.light_color;
        let computed_ill = lit.illuminance * weather_light.sun;
        let computed_color = [c[0] * tint[0], c[1] * tint[1], c[2] * tint[2]];
        let final_ill = t.sun_illuminance_override.unwrap_or(computed_ill);
        let final_color = match t.sun_color_override {
            Some(srgb) => srgb3(srgb),
            None => computed_color,
        };
        l.illuminance = final_ill;
        l.color = Color::linear_rgb(final_color[0], final_color[1], final_color[2]);
        *tf = Transform::default().looking_to(-Vec3::from(lit.light_from), Vec3::Y);
    }

    // --- Ambient ------------------------------------------------------------
    let a = lit.ambient_color;
    let computed_amb_bright = lit.ambient_brightness * weather_light.ambient;
    let computed_amb_color = [a[0] * tint[0], a[1] * tint[1], a[2] * tint[2]];
    let final_amb_bright = t.ambient_brightness_override.unwrap_or(computed_amb_bright);
    let final_amb_color = match t.ambient_color_override {
        Some(srgb) => srgb3(srgb),
        None => computed_amb_color,
    };
    ambient.brightness = final_amb_bright;
    ambient.color = Color::linear_rgb(final_amb_color[0], final_amb_color[1], final_amb_color[2]);

    // --- Nebbia -------------------------------------------------------------
    let f = lit.fog_color;
    let preset_horizon = match &preset {
        Some(p) => crate::sky::presets::preset_for(p.current).fog_day,
        None => f,
    };
    let mix = weather_light.fog_gray.clamp(0.0, 1.0);
    let blended = [
        f[0] * (1.0 - mix) + preset_horizon[0] * mix,
        f[1] * (1.0 - mix) + preset_horizon[1] * mix,
        f[2] * (1.0 - mix) + preset_horizon[2] * mix,
    ];
    let b = weather_light.fog_bright;
    let fog_rgb = [blended[0] * b, blended[1] * b, blended[2] * b];

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
                    if let Some(base) = weather_light.base_fog {
                        *start = base.0;
                        *end = base.1;
                    }
                    weather_light.fog_touched = false;
                }
            }
        }
    }
}

// =============================================================================
// No-op retro-compat
// =============================================================================

pub fn drive_overcast_from_weather(
    _data: Res<GameData>,
    mut overcast: ResMut<SkyOvercast>,
) {
    overcast.target = 0.0;
    overcast.level = 0.0;
    overcast.manual_override = None;
}

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

pub fn season_from_config(config: &SkyConfig) -> Season {
    GameDate::new(405, config.start_month, config.start_day, config.hour).season()
}
