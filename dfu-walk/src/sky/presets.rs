//! Preset del cielo Dynamic Skies / DREAM-SKY.
//!
//! ## Pulsazione dell'opacità delle nuvole
//!
//! Ogni `CloudLayer` ha un campo `opacity_pulse_speed`:
//!
//! - `0.0` → nessuna pulsazione (l'opacità resta il valore `opacity` fissato);
//! - `> 0.0` → l'opacità oscilla fra 0 e il valore base con una sinusoide
//!   `0.5 + 0.5 * cos(world_time * speed)`, con periodo `2π / speed` secondi.
//!
//! Nel preset `SUNNY` è impostato a `0.05` (~126 s per ciclo), negli altri è
//! `0.0` (comportamento invariato).
//!
//! ## Illuminazione derivata (`WeatherLighting`)
//!
//! `WeatherLighting` è un **set di moltiplicatori** applicati sopra la luce
//! "astronomica" (`sky_state`/`lighting`) per scurire e colorare la scena 3D
//! a seconda del meteo. Il tuo `sky.rs::update_sky` lo usa così:
//!
//! ```ignore
//! let target = match (&preset, outside) {
//!     (Some(p), true) => p.current.scene_lighting(),
//!     _ => WeatherLighting::NEUTRAL,
//! };
//! // ... lerp esponenziale di ogni campo verso `target` ...
//! l.illuminance = lit.illuminance * weather_light.sun;
//! l.color = Color::linear_rgb(c[0] * tint[0], c[1] * tint[1], c[2] * tint[2]);
//! ambient.brightness = lit.ambient_brightness * weather_light.ambient;
//! // ... fog color/bright/gray/range ...
//! ```
//!
//! `NEUTRAL` è il valore "nessun meteo" (moltiplicatori a 1, nessun tint):
//! quando non sei all'aperto o `CurrentPreset` non è disponibile, la scena
//! torna al colore astronomico puro.
//!
//! **Scurimento sotto maltempo**:
//! - Snow: `sun 0.20`, `ambient 0.55` — grigio invernale, neve leggibile;
//! - Rain: `sun 0.10`, `ambient 0.35` — cupo;
//! - Thunder: `sun 0.04`, `ambient 0.15` — quasi crepuscolare.
//!
//! ## Parametri notturni: `sky_fade_start` e `sky_end_start`
//!
//! Nel WGSL di DreamSky:
//! - `night_start` / `night_end`: elevazione solare in cui il cielo passa al
//!   colore notturno (stelle, lune visibili);
//! - `sky_fade_start` / `sky_end_start`: elevazione solare in cui il **cielo
//!   base** (`sky_tint`) inizia a sfumare verso il nero.
//!
//! Con `sky_fade_start: 1.0` il fade **non parte mai**: il cielo rimane chiaro
//! anche a mezzanotte. Questo era il bug di `RAIN`, `SNOW`, `THUNDER` e
//! `OVERCAST`. Ora tutti i preset usano i valori di `SUNNY`
//! (`sky_fade_start: -0.01`, `sky_end_start: -0.04`), così il cielo si
//! scurisce correttamente di notte con qualsiasi meteo.
//!
//! ## Nebbia: `fog_gray` come "quanto il meteo copre il cielo"
//!
//! **Colori della nebbia e del cielo coperto**:
//! - Neve: bianco lattiginoso, `fog_day 0xE2E6EC`, `fog_bright 1.25`;
//! - Temporale: grigio scuro `0x2E323A`, `fog_bright 0.65`;
//! - Pioggia: grigio-ardesia `0x4A525E`.
//!
//! ## Clima e `from_date`
//!
//! `WeatherPreset::from_date` riceve anche il clima locale
//! (`ClimateBase`). Il deserto non ha inverno: con `ClimateBase::Desert` la
//! funzione si limita a scegliere fra `Sunny`, `Cloudy` e `Overcast`, così
//! città come Sentinel (Hammerfell) non mostrano neve in Evening Star /
//! Morning Star / Sun's Dawn. Vedi il doc della funzione.

use crate::calendar::GameDate;
use crate::data::climate::{ClimateBase, Season};

fn hex_rgb(hex: &str) -> [f32; 3] {
    let h = hex.trim_start_matches('#');
    let n = u32::from_str_radix(h, 16).unwrap_or(0xFFFFFF);
    let (r, g, b) = if h.len() >= 8 {
        (((n >> 24) & 0xFF) as f32 / 255.0, ((n >> 16) & 0xFF) as f32 / 255.0, ((n >> 8) & 0xFF) as f32 / 255.0)
    } else {
        (((n >> 16) & 0xFF) as f32 / 255.0, ((n >> 8) & 0xFF) as f32 / 255.0, (n & 0xFF) as f32 / 255.0)
    };
    [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)]
}

fn srgb_to_linear(x: f32) -> f32 {
    if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
}

fn hash01(i: i64, seed: u64) -> f32 {
    let mut s = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ seed.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    s ^= s >> 33;
    s = s.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    s ^= s >> 33;
    s = s.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    s ^= s >> 33;
    (s >> 40) as f32 / 16_777_216.0
}

pub(crate) fn value_noise(t: f64, seed: u64) -> f32 {
    let i = t.floor();
    let f = (t - i) as f32;
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash01(i as i64, seed);
    let b = hash01(i as i64 + 1, seed);
    a + (b - a) * u
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WeatherPreset {
    Sunny,
    Cloudy,
    Overcast,
    Fog,
    Rain,
    Thunder,
    Snow,
}

impl WeatherPreset {
    pub fn file_stem(self) -> &'static str {
        match self {
            WeatherPreset::Sunny => "SkyboxSunny",
            WeatherPreset::Cloudy => "SkyboxCloudy",
            WeatherPreset::Overcast => "SkyboxOvercast",
            WeatherPreset::Fog => "SkyboxFog",
            WeatherPreset::Rain => "SkyboxRain",
            WeatherPreset::Thunder => "SkyboxThunder",
            WeatherPreset::Snow => "SkyboxSnow",
        }
    }

    pub const ALL: [WeatherPreset; 7] = [
        WeatherPreset::Sunny, WeatherPreset::Cloudy, WeatherPreset::Overcast,
        WeatherPreset::Fog, WeatherPreset::Rain, WeatherPreset::Thunder, WeatherPreset::Snow,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            WeatherPreset::Sunny => "Soleggiato",
            WeatherPreset::Cloudy => "Nuvoloso",
            WeatherPreset::Overcast => "Coperto",
            WeatherPreset::Fog => "Nebbia",
            WeatherPreset::Rain => "Pioggia",
            WeatherPreset::Thunder => "Temporale",
            WeatherPreset::Snow => "Neve",
        }
    }

    /// Sceglie il meteo automatico dalla data **e dal clima locale** del
    /// giocatore.
    ///
    /// Il rumore di base (cloud, wet, mist) è lo stesso di prima; la novità è
    /// che il **deserto non ha inverno**: in `ClimateBase::Desert` la funzione
    /// ritorna solo `Sunny`, `Cloudy` o `Overcast`. Senza questo, città come
    /// Sentinel (Hammerfell, climate `223..=229` con `climate_type = Desert` in
    /// `world_climate_settings`) mostravano la neve in Evening Star / Morning
    /// Star / Sun's Dawn, nonostante `apply_climate` disattivi già le texture
    /// invernali del suolo.
    ///
    /// La pioggia nel deserto è rara ma possibile in natura; qui la escludiamo
    /// per semplicità (più realistico della neve, ma meno interessante). Se
    /// vuoi reintrodurla, basta togliere il ramo `Desert` e alzare la soglia
    /// `wet` per quel clima.
    ///
    /// L'override manuale (F4 in `calendar::cycle_weather`) non passa di qui:
    /// è una scelta esplicita del giocatore e resta possibile anche nel deserto.
    pub fn from_date(date: GameDate, climate: ClimateBase) -> Self {
        let (base_cloud, base_wet, cold) = match date.month {
            11 | 0 | 1 => (0.55_f32, 0.45_f32, true),
            3 | 8 => (0.65, 0.70, false),
            2 | 4 | 9 | 10 => (0.45, 0.38, false),
            _ => (0.28, 0.12, false),
        };
        let t = (date.absolute_day() as f64 * 24.0 + date.hour as f64) / 8.0;
        let cloud = (base_cloud + (value_noise(t, 0xA11CE) - 0.5)).clamp(0.0, 1.0);
        let wet = (base_wet + (value_noise(t * 0.5, 0xB0B) - 0.5)).clamp(0.0, 1.0);
        let mist = value_noise(t, 0xF06);
        let morning = date.hour >= 3.0 && date.hour < 10.0;

        // Il deserto non ha inverno: niente neve. Anche la pioggia è rara,
        // quindi ci limitiamo a sole / qualche nuvola / cielo coperto. Questo
        // evita che città come Sentinel (Hammerfell) mostrino la neve in
        // Evening Star / Morning Star / Sun's Dawn.
        if matches!(climate, ClimateBase::Desert) {
            return if cloud >= 0.62 {
                Self::Overcast
            } else if cloud >= 0.40 {
                Self::Cloudy
            } else {
                Self::Sunny
            };
        }

        if cloud >= 0.55 && wet >= 0.60 {
            if cold {
                Self::Snow
            } else if wet >= 0.93 && cloud >= 0.70 {
                Self::Thunder
            } else {
                Self::Rain
            }
        } else if morning && mist > 0.75 && cloud < 0.70 && wet > 0.35 {
            Self::Fog
        } else if cloud >= 0.62 {
            Self::Overcast
        } else if cloud >= 0.40 {
            Self::Cloudy
        } else {
            Self::Sunny
        }
    }

    pub fn to_season(self) -> Season {
        match self {
            Self::Sunny | Self::Cloudy | Self::Overcast | Self::Fog => Season::Summer,
            Self::Rain | Self::Thunder => Season::Rain,
            Self::Snow => Season::Winter,
        }
    }

    pub fn is_rain(self) -> bool {
        matches!(self, Self::Rain | Self::Thunder)
    }

    pub fn is_snow(self) -> bool {
        matches!(self, Self::Snow)
    }

    pub fn next_manual(self) -> Self {
        match self {
            WeatherPreset::Sunny => WeatherPreset::Cloudy,
            WeatherPreset::Cloudy => WeatherPreset::Overcast,
            WeatherPreset::Overcast => WeatherPreset::Fog,
            WeatherPreset::Fog => WeatherPreset::Rain,
            WeatherPreset::Rain => WeatherPreset::Thunder,
            WeatherPreset::Thunder => WeatherPreset::Snow,
            WeatherPreset::Snow => WeatherPreset::Sunny,
        }
    }

    /// Moltiplicatori di illuminazione per questo preset.
    ///
    /// Il tuo `sky.rs::update_sky` li applica sopra la luce astronomica
    /// (`lit` di `sky_state`/`lighting`), non li usa come valori assoluti.
    ///
    /// **Scurimento sotto maltempo**:
    /// - Snow: `sun 0.20`, `ambient 0.55` — grigio invernale, neve leggibile;
    /// - Rain: `sun 0.10`, `ambient 0.35` — cupo;
    /// - Thunder: `sun 0.04`, `ambient 0.15` — quasi crepuscolare.
    pub fn scene_lighting(&self) -> WeatherLighting {
        match self {
            WeatherPreset::Sunny => WeatherLighting {
                sun: 1.00,
                ambient: 1.00,
                tint: [1.00, 1.00, 1.00],
                fog_bright: 1.00,
                fog_gray: 0.00,
                fog_range: 1.00,
            },
            WeatherPreset::Cloudy => WeatherLighting {
                sun: 0.65,
                ambient: 0.85,
                tint: [0.95, 0.95, 0.97],
                fog_bright: 1.00,
                fog_gray: 0.30,
                fog_range: 0.95,
            },
            WeatherPreset::Overcast => WeatherLighting {
                sun: 0.35,
                ambient: 0.60,
                tint: [0.85, 0.87, 0.90],
                fog_bright: 0.95,
                fog_gray: 0.70,
                fog_range: 0.85,
            },
            WeatherPreset::Fog => WeatherLighting {
                sun: 0.20,
                ambient: 0.45,
                tint: [0.85, 0.87, 0.88],
                fog_bright: 1.05,
                fog_gray: 0.90,
                fog_range: 0.35,
            },
            WeatherPreset::Rain => WeatherLighting {
                sun: 0.10,
                ambient: 0.35,
                tint: [0.65, 0.70, 0.80],
                fog_bright: 0.75,
                fog_gray: 0.85,
                fog_range: 0.60,
            },
            WeatherPreset::Thunder => WeatherLighting {
                sun: 0.04,
                ambient: 0.15,
                tint: [0.50, 0.55, 0.70],
                fog_bright: 0.65,
                fog_gray: 0.90,
                fog_range: 0.40,
            },
            WeatherPreset::Snow => WeatherLighting {
                sun: 0.04,
                ambient: 0.55,
                tint: [0.22, 0.25, 0.30],
                fog_bright: 0.10,
                fog_gray: 0.85,
                fog_range: 0.55,
            },
        }
    }
}

/// Moltiplicatori di illuminazione, applicati sopra `sky_state`/`lighting`.
#[derive(Clone, Copy, Debug)]
pub struct WeatherLighting {
    /// Fattore per `lit.illuminance` (0..1).
    pub sun: f32,
    /// Fattore per `lit.ambient_brightness` (0..1).
    pub ambient: f32,
    /// Fattore RGB per i colori di luce e ambiente.
    pub tint: [f32; 3],
    /// Fattore per la luminosità del fog (0..1).
    pub fog_bright: f32,
    /// "Quanto il meteo copre il cielo": 0 = nebbia = colore dell'ora,
    /// 1 = nebbia = `fog_day` del preset.
    pub fog_gray: f32,
    /// Fattore per la distanza del fog (0.5 = nebbia più vicina).
    pub fog_range: f32,
}

impl WeatherLighting {
    pub const NEUTRAL: Self = Self {
        sun: 1.0,
        ambient: 1.0,
        tint: [1.0, 1.0, 1.0],
        fog_bright: 1.0,
        fog_gray: 0.0,
        fog_range: 1.0,
    };
}

#[derive(Clone, Copy, Debug)]
pub struct CloudLayer {
    pub texture: &'static str,
    pub normal_texture: &'static str,
    pub tiling: [f32; 2],
    pub offset: [f32; 2],
    pub day_color: [f32; 3],
    pub night_color: [f32; 3],
    pub alpha_threshold: f32,
    pub alpha_max: f32,
    pub color_boost: f32,
    pub normal_effect: f32,
    pub opacity: f32,
    pub bending: f32,
    pub sun_color: [f32; 3],
    pub sun_color_scale: f32,
    pub sun_color_lerp_scale: f32,
    pub speed: f32,
    pub opacity_pulse_speed: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct StarLayer {
    pub texture: &'static str,
    pub twinkle_mask: &'static str,
    pub twinkle_noise: &'static str,
    pub tiling: [f32; 2],
    pub bending: f32,
    pub twinkle_boost: f32,
    pub twinkle_speed: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct MoonOrbit {
    pub color: [f32; 3],
    pub texture: &'static str,
    pub min_size: f32,
    pub max_size: f32,
    pub orbit_angle: [f32; 3],
    pub orbit_offset: f32,
    pub orbit_speed: f32,
    pub semi_major: f32,
    pub semi_minor: f32,
    pub auto_phase: bool,
    pub tidal_angle: [f32; 3],
    pub spin_speed: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct SkyboxPreset {
    pub sun_size: f32,
    pub sun_convergence: f32,
    pub atmosphere_lerp_duration: f32,
    pub atmosphere_normal: f32,
    pub atmosphere_dawn_dusk: f32,
    pub atmosphere_lerp: f32,
    pub sky_tint: [f32; 3],
    pub ground_color: [f32; 3],
    pub exposure: f32,
    pub night_start: f32,
    pub night_end: f32,
    pub sky_fade_start: f32,
    pub sky_end_start: f32,
    pub fog_day: [f32; 3],
    pub fog_night: [f32; 3],
    pub fog_distance: f32,
    pub moon_night_color: [f32; 3],
    pub step_size: f32,
    pub cloud_fade_height: f32,
    pub top_clouds: CloudLayer,
    pub bottom_clouds: CloudLayer,
    pub stars: StarLayer,
    pub masser: MoonOrbit,
    pub secunda: MoonOrbit,
}

pub const SUNNY: SkyboxPreset = SkyboxPreset {
    sun_size: 0.1, sun_convergence: 10.0, atmosphere_lerp_duration: 0.666,
    atmosphere_normal: 0.75, atmosphere_dawn_dusk: 1.5, atmosphere_lerp: 1.0,
    sky_tint: hex_rgb_const(0x6176DC), ground_color: hex_rgb_const(0x4C4C4C), exposure: 1.5,
    night_start: 0.01, night_end: -0.01, sky_fade_start: -0.01, sky_end_start: -0.04,
    fog_day: hex_rgb_const(0x7783B7), fog_night: hex_rgb_const(0x000000),
    fog_distance: 1500.0, moon_night_color: hex_rgb_const(0x000027), step_size: 0.015,
    cloud_fade_height: 0.20,
    top_clouds: CloudLayer { texture: "CdMSunny", normal_texture: "CdMCloudsNormal", tiling: [0.2, 0.2], offset: [0.0, 0.0], day_color: hex_rgb_const(0xC7C7C7), night_color: hex_rgb_const(0x303030), alpha_threshold: 0.02, alpha_max: 0.757, color_boost: 0.665, normal_effect: 0.329, opacity: 0.797, bending: 0.0, sun_color: hex_rgb_const(0x191919), sun_color_scale: 1.0, sun_color_lerp_scale: 1.0, speed: 0.001, opacity_pulse_speed: 0.05 },
    bottom_clouds: CloudLayer { texture: "CdMSunny2", normal_texture: "CdMCloudsNormal", tiling: [0.25, 0.25], offset: [0.5, 0.0], day_color: hex_rgb_const(0xC7C7C7), night_color: hex_rgb_const(0x232329), alpha_threshold: 0.02, alpha_max: 0.757, color_boost: 0.665, normal_effect: 0.665, opacity: 0.557, bending: 0.15, sun_color: hex_rgb_const(0x1A1A1A), sun_color_scale: 1.0, sun_color_lerp_scale: 1.0, speed: 0.0125, opacity_pulse_speed: 0.05 },
    stars: StarLayer { texture: "VanillaStars", twinkle_mask: "VanillaStarsTwinkleMask", twinkle_noise: "NLstarsHighlight", tiling: [0.25, 0.25], bending: 1.0, twinkle_boost: 1.0, twinkle_speed: 0.05 },
    masser: MoonOrbit { color: hex_rgb_const(0xABABAB), texture: "PixelMars", min_size: 0.11, max_size: 0.11, orbit_angle: [0.0, 270.0, 90.0], orbit_offset: 0.0, orbit_speed: 0.00024, semi_major: 1.0, semi_minor: 1.0, auto_phase: false, tidal_angle: [0.0, 300.0, 0.0], spin_speed: [0.1, 0.0, 0.0] },
    secunda: MoonOrbit { color: hex_rgb_const(0xABABAB), texture: "PixelEnceladus", min_size: 0.07, max_size: 0.07, orbit_angle: [0.0, 270.0, 90.0], orbit_offset: 0.0, orbit_speed: 0.00024, semi_major: 0.6, semi_minor: 0.9, auto_phase: false, tidal_angle: [30.0, 160.0, -30.0], spin_speed: [0.1, 0.0, 0.0] },
};

pub const CLOUDY: SkyboxPreset = SkyboxPreset {
    sun_size: 0.1, sun_convergence: 10.0, atmosphere_lerp_duration: 0.66, atmosphere_normal: 0.75,
    atmosphere_dawn_dusk: 1.5, atmosphere_lerp: 1.0, sky_tint: hex_rgb_const(0x6176DC),
    ground_color: hex_rgb_const(0x4C4C4C), exposure: 1.5,
    // Fade notturno come SUNNY (funzionava già).
    night_start: 0.01, night_end: -0.01, sky_fade_start: -0.01, sky_end_start: -0.04,
    fog_day: hex_rgb_const(0x7A8494), fog_night: hex_rgb_const(0x000000),
    fog_distance: 1500.0, moon_night_color: hex_rgb_const(0x000027),
    step_size: 0.015, cloud_fade_height: 0.015,
    top_clouds: CloudLayer { texture: "CdMCloudy", normal_texture: "CdMCloudsNormal", tiling: [0.2, 0.2], offset: [0.5, 0.2], day_color: hex_rgb_const(0xFFFFFF), night_color: hex_rgb_const(0x303030), alpha_threshold: 0.0, alpha_max: 0.184, color_boost: 0.0, normal_effect: 0.5, opacity: 1.0, bending: 0.2, sun_color: hex_rgb_const(0x303030), sun_color_scale: 1.0, sun_color_lerp_scale: 1.0, speed: 0.0, opacity_pulse_speed: 0.0 },
    bottom_clouds: CloudLayer { texture: "CdMCloudy", normal_texture: "CdMCloudsNormal", tiling: [0.25, 0.25], offset: [0.5, 0.4], day_color: hex_rgb_const(0xFFFFFF), night_color: hex_rgb_const(0x232329), alpha_threshold: 0.0, alpha_max: 0.20, color_boost: 1.0, normal_effect: 0.1, opacity: 1.0, bending: 0.25, sun_color: hex_rgb_const(0x191919), sun_color_scale: 1.0, sun_color_lerp_scale: 1.0, speed: 0.00125, opacity_pulse_speed: 0.0 },
    stars: StarLayer { texture: "VanillaStars", twinkle_mask: "VanillaStarsTwinkleMask", twinkle_noise: "NLstarsHighlight", tiling: [0.25, 0.25], bending: 1.0, twinkle_boost: 1.0, twinkle_speed: 0.05 },
    masser: MoonOrbit { color: hex_rgb_const(0xABABAB), texture: "PixelMars", min_size: 0.11, max_size: 0.11, orbit_angle: [0.0, 0.0, 45.0], orbit_offset: 0.0, orbit_speed: 0.00002, semi_major: 1.0, semi_minor: 1.0, auto_phase: true, tidal_angle: [0.0, 300.0, 0.0], spin_speed: [0.1, 0.0, 0.0] },
    secunda: MoonOrbit { color: hex_rgb_const(0xABABAB), texture: "PixelEnceladus", min_size: 0.07, max_size: 0.07, orbit_angle: [0.0, 0.0, 30.0], orbit_offset: 30.0, orbit_speed: 0.00002, semi_major: 0.6, semi_minor: 0.9, auto_phase: true, tidal_angle: [30.0, 160.0, -30.0], spin_speed: [0.1, 0.0, 0.0] },
};

pub const OVERCAST: SkyboxPreset = SkyboxPreset {
    sun_size: 0.1, sun_convergence: 10.0, atmosphere_lerp_duration: 0.667, atmosphere_normal: 0.75,
    atmosphere_dawn_dusk: 2.0, atmosphere_lerp: 1.0, sky_tint: hex_rgb_const(0xFFFFFF),
    ground_color: hex_rgb_const(0x4C4C4C), exposure: 1.2,
    // FIX: prima era sky_fade_start: 1.0 (fade mai attivato, cielo chiaro
    // anche di notte). Ora come SUNNY: il cielo si scurisce al tramonto.
    night_start: 0.01, night_end: -0.01, sky_fade_start: -0.01, sky_end_start: -0.04,
    fog_day: hex_rgb_const(0x707680), fog_night: hex_rgb_const(0x000000),
    fog_distance: 320.0, moon_night_color: hex_rgb_const(0x000027),
    step_size: 0.01, cloud_fade_height: 0.06,
    top_clouds: CloudLayer { texture: "CdMRain", normal_texture: "CdMCloudsNormal", tiling: [0.3, 0.3], offset: [0.5, 0.0], day_color: hex_rgb_const(0xFFFFFF), night_color: hex_rgb_const(0x303030), alpha_threshold: 0.0, alpha_max: 0.5, color_boost: 0.0, normal_effect: 0.0, opacity: 0.25, bending: 0.15, sun_color: hex_rgb_const(0x69433B), sun_color_scale: 1.0, sun_color_lerp_scale: 1.0, speed: 0.0, opacity_pulse_speed: 0.0 },
    bottom_clouds: CloudLayer { texture: "CdMOvercast2", normal_texture: "CdMCloudsNormal", tiling: [0.1, 0.1], offset: [0.4, 0.5], day_color: hex_rgb_const(0x8E99B8), night_color: hex_rgb_const(0x2E2E32), alpha_threshold: 0.0, alpha_max: 0.6, color_boost: 0.058, normal_effect: 1.0, opacity: 0.7, bending: 0.15, sun_color: hex_rgb_const(0x1A1A1A), sun_color_scale: 1.0, sun_color_lerp_scale: 1.0, speed: 0.00125, opacity_pulse_speed: 0.0 },
    stars: StarLayer { texture: "VanillaStars", twinkle_mask: "VanillaStarsTwinkleMask", twinkle_noise: "NLstarsHighlight", tiling: [0.25, 0.25], bending: 1.0, twinkle_boost: 1.0, twinkle_speed: 0.05 },
    masser: MoonOrbit { color: hex_rgb_const(0xABABAB), texture: "PixelMars", min_size: 0.11, max_size: 0.11, orbit_angle: [0.0, 0.0, 45.0], orbit_offset: 0.0, orbit_speed: 0.00002, semi_major: 1.0, semi_minor: 1.0, auto_phase: true, tidal_angle: [0.0, 300.0, 0.0], spin_speed: [0.1, 0.0, 0.0] },
    secunda: MoonOrbit { color: hex_rgb_const(0xABABAB), texture: "PixelEnceladus", min_size: 0.07, max_size: 0.07, orbit_angle: [0.0, 0.0, 30.0], orbit_offset: 30.0, orbit_speed: 0.00002, semi_major: 0.6, semi_minor: 0.9, auto_phase: true, tidal_angle: [30.0, 160.0, -30.0], spin_speed: [0.1, 0.0, 0.0] },
};

pub const FOG: SkyboxPreset = SkyboxPreset {
    sun_size: 0.1, sun_convergence: 10.0, atmosphere_lerp_duration: 0.666, atmosphere_normal: 0.65,
    atmosphere_dawn_dusk: 1.0, atmosphere_lerp: 1.0, sky_tint: hex_rgb_const(0x87CEEB),
    ground_color: hex_rgb_const(0x4C4C4C), exposure: 1.0,
    night_start: 0.0, night_end: -0.16, sky_fade_start: 0.04, sky_end_start: -0.008,
    fog_day: hex_rgb_const(0x8A8E94), fog_night: hex_rgb_const(0x141414),
    fog_distance: 2.0, moon_night_color: hex_rgb_const(0x000027),
    step_size: 0.01, cloud_fade_height: 0.15,
    top_clouds: CloudLayer { texture: "CdMRain", normal_texture: "CdMCloudsNormal", tiling: [0.125, 0.125], offset: [0.0, 0.0], day_color: hex_rgb_const(0x808080), night_color: hex_rgb_const(0x171512), alpha_threshold: 0.0, alpha_max: 0.6, color_boost: 0.0, normal_effect: 1.0, opacity: 0.75, bending: 0.45, sun_color: hex_rgb_const(0xFFFFFF), sun_color_scale: 0.0, sun_color_lerp_scale: 0.0, speed: 0.0, opacity_pulse_speed: 0.0 },
    bottom_clouds: CloudLayer { texture: "CdMRain", normal_texture: "CdMCloudsNormal", tiling: [0.25, 0.25], offset: [0.0, 0.0], day_color: hex_rgb_const(0x606060), night_color: hex_rgb_const(0x141310), alpha_threshold: 0.0, alpha_max: 0.7, color_boost: 0.0, normal_effect: 0.25, opacity: 0.75, bending: 0.55, sun_color: hex_rgb_const(0xFFFFFF), sun_color_scale: 0.0, sun_color_lerp_scale: 0.0, speed: 0.00125, opacity_pulse_speed: 0.0 },
    stars: StarLayer { texture: "DefaultStars", twinkle_mask: "DefaultStarsTwinkleMask", twinkle_noise: "DefaultStarsTwinkleNoise", tiling: [1.0, 1.0], bending: 1.0, twinkle_boost: 1.0, twinkle_speed: 0.1 },
    masser: MoonOrbit { color: hex_rgb_const(0xABABAB), texture: "PixelMars", min_size: 0.11, max_size: 0.11, orbit_angle: [0.0, 0.0, 45.0], orbit_offset: 0.0, orbit_speed: 0.001, semi_major: 1.0, semi_minor: 1.0, auto_phase: true, tidal_angle: [0.0, 0.0, 0.0], spin_speed: [0.0, 0.0, 0.0] },
    secunda: MoonOrbit { color: hex_rgb_const(0xABABAB), texture: "PixelEnceladus", min_size: 0.07, max_size: 0.07, orbit_angle: [0.0, 0.0, 45.0], orbit_offset: 10.0, orbit_speed: 0.001, semi_major: 0.6, semi_minor: 0.9, auto_phase: true, tidal_angle: [20.0, 0.0, 0.0], spin_speed: [0.0, 0.0, 0.0] },
};

pub const RAIN: SkyboxPreset = SkyboxPreset {
    sun_size: 0.1, sun_convergence: 10.0, atmosphere_lerp_duration: 0.666, atmosphere_normal: 1.0,
    atmosphere_dawn_dusk: 1.5, atmosphere_lerp: 1.0, sky_tint: hex_rgb_const(0x676777),
    ground_color: hex_rgb_const(0x4C4C4C), exposure: 0.09,
    // FIX: prima era sky_fade_start: 1.0 / sky_end_start: -1.0 (fade mai
    // attivato, cielo chiaro anche di notte). Ora come SUNNY.
    night_start: 0.01, night_end: -0.01, sky_fade_start: -0.01, sky_end_start: -0.04,
    fog_day: hex_rgb_const(0x4A525E), fog_night: hex_rgb_const(0x000000),
    fog_distance: 384.0, moon_night_color: hex_rgb_const(0x000027),
    step_size: 0.005, cloud_fade_height: 0.20,
    top_clouds: CloudLayer { texture: "CdMRain", normal_texture: "CdMCloudsNormal", tiling: [0.3, 0.3], offset: [0.0, 0.0], day_color: hex_rgb_const(0xFFFFFF), night_color: hex_rgb_const(0x000000), alpha_threshold: 0.006, alpha_max: 0.6, color_boost: 0.0, normal_effect: 0.4, opacity: 1.0, bending: 0.2, sun_color: hex_rgb_const(0x4C3912), sun_color_scale: 1.0, sun_color_lerp_scale: 1.0, speed: 0.0, opacity_pulse_speed: 0.0 },
    bottom_clouds: CloudLayer { texture: "CdMRain", normal_texture: "CdMCloudsNormal", tiling: [0.3, 0.3], offset: [0.0, 0.0], day_color: hex_rgb_const(0x3B3D48), night_color: hex_rgb_const(0x211F1B), alpha_threshold: 0.0, alpha_max: 0.7, color_boost: 0.65, normal_effect: 0.6, opacity: 0.75, bending: 0.25, sun_color: hex_rgb_const(0x000000), sun_color_scale: 1.0, sun_color_lerp_scale: 1.0, speed: 0.002, opacity_pulse_speed: 0.0 },
    stars: StarLayer { texture: "NLStarsBlack", twinkle_mask: "NLStarsThiefTwinkleMask", twinkle_noise: "NLstarsHighlight", tiling: [1.0, 1.0], bending: 1.0, twinkle_boost: 1.0, twinkle_speed: 0.1 },
    masser: MoonOrbit { color: hex_rgb_const(0xABABAB), texture: "PixelMars", min_size: 0.11, max_size: 0.11, orbit_angle: [0.0, 0.0, 45.0], orbit_offset: 0.0, orbit_speed: 0.00002, semi_major: 1.0, semi_minor: 1.0, auto_phase: true, tidal_angle: [0.0, 0.0, 0.0], spin_speed: [0.1, 0.0, 0.0] },
    secunda: MoonOrbit { color: hex_rgb_const(0xABABAB), texture: "PixelEnceladus", min_size: 0.07, max_size: 0.07, orbit_angle: [0.0, 0.0, 45.0], orbit_offset: 30.0, orbit_speed: 0.00002, semi_major: 0.6, semi_minor: 0.9, auto_phase: true, tidal_angle: [20.0, 0.0, 0.0], spin_speed: [0.1, 0.0, 0.0] },
};

pub const SNOW: SkyboxPreset = SkyboxPreset {
    sun_size: 0.001, sun_convergence: 10.0, atmosphere_lerp_duration: 0.666, atmosphere_normal: 0.0,
    atmosphere_dawn_dusk: 1.5, atmosphere_lerp: 0.0,
    sky_tint: hex_rgb_const(0x4A5260),
    ground_color: hex_rgb_const(0x4C4C4C), exposure: 0.40,
    // FIX: prima era night_start: 0.60 / sky_fade_start: 1.0 (fade mai
    // attivato, cielo lattiginoso anche a mezzanotte). Ora come SUNNY.
    night_start: 0.01, night_end: -0.01, sky_fade_start: -0.01, sky_end_start: -0.04,
    fog_day: hex_rgb_const(0x4A5260), fog_night: hex_rgb_const(0x141414),
    fog_distance: 2.0, moon_night_color: hex_rgb_const(0x000027),
    step_size: 0.005, cloud_fade_height: 0.0,
    top_clouds: CloudLayer { texture: "CdMThunder", normal_texture: "CdMCloudsNormal", tiling: [0.125, 0.125], offset: [0.0, 0.0], day_color: hex_rgb_const(0x606060), night_color: hex_rgb_const(0x666666), alpha_threshold: 0.0, alpha_max: 0.85, color_boost: 0.0, normal_effect: 0.5, opacity: 0.95, bending: 0.1, sun_color: hex_rgb_const(0xA83C00), sun_color_scale: 0.2, sun_color_lerp_scale: 0.7, speed: 0.0, opacity_pulse_speed: 0.0 },
    bottom_clouds: CloudLayer { texture: "CdMSnow", normal_texture: "CdMCloudsNormal", tiling: [0.25, 0.25], offset: [0.0, 0.0], day_color: hex_rgb_const(0xE2E6EC), night_color: hex_rgb_const(0x000000), alpha_threshold: 0.0, alpha_max: 0.9, color_boost: 0.4, normal_effect: 0.577, opacity: 0.7, bending: 0.3, sun_color: hex_rgb_const(0x8C3900), sun_color_scale: 0.2, sun_color_lerp_scale: 0.8, speed: 0.00075, opacity_pulse_speed: 0.0 },
    stars: StarLayer { texture: "NLStarsBlack", twinkle_mask: "NLStarsThiefTwinkleMask", twinkle_noise: "NLstarsHighlight", tiling: [1.0, 1.0], bending: 1.0, twinkle_boost: 1.0, twinkle_speed: 0.1 },
    masser: MoonOrbit { color: hex_rgb_const(0xABABAB), texture: "PixelMars", min_size: 0.11, max_size: 0.11, orbit_angle: [0.0, 0.0, 45.0], orbit_offset: 0.0, orbit_speed: 0.001, semi_major: 1.0, semi_minor: 1.0, auto_phase: true, tidal_angle: [0.0, 300.0, 0.0], spin_speed: [0.1, 0.0, 0.0] },
    secunda: MoonOrbit { color: hex_rgb_const(0xABABAB), texture: "PixelEnceladus", min_size: 0.07, max_size: 0.07, orbit_angle: [0.0, 0.0, 45.0], orbit_offset: 30.0, orbit_speed: 0.002, semi_major: 0.6, semi_minor: 0.9, auto_phase: true, tidal_angle: [20.0, 0.0, 0.0], spin_speed: [0.1, 0.0, 0.0] },
};

pub const THUNDER: SkyboxPreset = SkyboxPreset {
    sun_size: 0.1, sun_convergence: 10.0, atmosphere_lerp_duration: 0.666, atmosphere_normal: 1.0,
    atmosphere_dawn_dusk: 1.0, atmosphere_lerp: 0.5, sky_tint: hex_rgb_const(0x2E323A),
    ground_color: hex_rgb_const(0x4C4C4C), exposure: 0.04,
    // FIX: prima era sky_fade_start: 0.829 (fade solo a sole altissimo,
    // praticamente mai). Ora come SUNNY.
    night_start: 0.01, night_end: -0.01, sky_fade_start: -0.01, sky_end_start: -0.04,
    fog_day: hex_rgb_const(0x2E323A), fog_night: hex_rgb_const(0x000000),
    fog_distance: 384.0, moon_night_color: hex_rgb_const(0x000027),
    step_size: 0.001, cloud_fade_height: 0.20,
    top_clouds: CloudLayer { texture: "CdMThunder", normal_texture: "CdMCloudsNormal", tiling: [0.3, 0.3], offset: [0.0, 0.0], day_color: hex_rgb_const(0x2E323A), night_color: hex_rgb_const(0x000000), alpha_threshold: 0.0, alpha_max: 0.9, color_boost: 0.0, normal_effect: 0.4, opacity: 0.97, bending: 0.2, sun_color: hex_rgb_const(0x000000), sun_color_scale: 0.5, sun_color_lerp_scale: 0.4, speed: 0.0, opacity_pulse_speed: 0.0 },
    bottom_clouds: CloudLayer { texture: "CdMThunder", normal_texture: "CdMCloudsNormal", tiling: [0.3, 0.3], offset: [0.0, 0.0], day_color: hex_rgb_const(0x2E323A), night_color: hex_rgb_const(0x000000), alpha_threshold: 0.0, alpha_max: 0.95, color_boost: 0.65, normal_effect: 0.627, opacity: 1.0, bending: 0.25, sun_color: hex_rgb_const(0x62482F), sun_color_scale: 0.5, sun_color_lerp_scale: 0.75, speed: 0.0025, opacity_pulse_speed: 0.0 },
    stars: StarLayer { texture: "NLStarsBlack", twinkle_mask: "NLStarsThiefTwinkleMask", twinkle_noise: "NLstarsHighlight", tiling: [1.0, 1.0], bending: 1.0, twinkle_boost: 1.0, twinkle_speed: 1.0 },
    masser: MoonOrbit { color: hex_rgb_const(0xABABAB), texture: "PixelMars", min_size: 0.11, max_size: 0.11, orbit_angle: [0.0, 0.0, 45.0], orbit_offset: 0.0, orbit_speed: 0.01, semi_major: 1.0, semi_minor: 1.0, auto_phase: true, tidal_angle: [0.0, 0.0, 0.0], spin_speed: [0.1, 0.0, 0.0] },
    secunda: MoonOrbit { color: hex_rgb_const(0xABABAB), texture: "PixelEnceladus", min_size: 0.07, max_size: 0.07, orbit_angle: [0.0, 0.0, 45.0], orbit_offset: 30.0, orbit_speed: 0.01, semi_major: 0.6, semi_minor: 0.9, auto_phase: true, tidal_angle: [20.0, 0.0, 0.0], spin_speed: [0.1, 0.0, 0.0] },
};

pub fn preset_for(w: WeatherPreset) -> &'static SkyboxPreset {
    match w {
        WeatherPreset::Sunny => &SUNNY,
        WeatherPreset::Cloudy => &CLOUDY,
        WeatherPreset::Overcast => &OVERCAST,
        WeatherPreset::Fog => &FOG,
        WeatherPreset::Rain => &RAIN,
        WeatherPreset::Thunder => &THUNDER,
        WeatherPreset::Snow => &SNOW,
    }
}

const fn hex_rgb_const(rgb: u32) -> [f32; 3] {
    let r = ((rgb >> 16) & 0xFF) as f32 / 255.0;
    let g = ((rgb >> 8) & 0xFF) as f32 / 255.0;
    let b = (rgb & 0xFF) as f32 / 255.0;
    [srgb_to_linear_const(r), srgb_to_linear_const(g), srgb_to_linear_const(b)]
}

const fn srgb_to_linear_const(x: f32) -> f32 {
    if x <= 0.04045 {
        x / 12.92
    } else {
        x * (x * (x * 0.305_306_01 + 0.682_171_11) + 0.012_522_878)
    }
}
