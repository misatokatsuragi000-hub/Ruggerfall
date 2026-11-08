//! Calendario di Daggerfall con i nomi originali dei mesi e della settimana,
//! e catena del meteo:
//!
//! `SkyClock.date ─► WeatherPreset (meteo) ─► cielo / pioggia / neve`
//! `       │`
//! `       └─► Season (dal MESE) ─► terreno / alberi / tiles`
//!
//! `sync_season_from_calendar` è l'unico sistema che decide il preset
//! corrente e la stagione. Il cielo (`dream_sky`) legge `CurrentPreset.current`
//! e non sceglie niente da solo; `WeatherOverride` (F4) forza un preset.
//!
//! ## Fix: stagione disaccoppiata dal meteo
//!
//! Prima la `Season` era derivata dal meteo (`target.to_season()`), quindi le
//! texture a terra (neve/estate) erano schiave del `WeatherPreset` corrente:
//! un cambio automatico Neve→Sole (~ogni `MIN_DWELL_HOURS`, ≈1 minuto reale
//! con `day_minutes = 12`) invalidava tutti i tile "di botto" e faceva
//! sparire la neve al suolo. Rendeva anche impossibile il sole d'inverno con
//! le texture innevate (sole ⇒ Summer ⇒ verde).
//!
//! Ora:
//! - **Stagione** = dal mese (`clock.date.season()`: Winter per i mesi
//!   11/0/1, Summer per gli altri). Cambia solo al cambio mese.
//! - **Meteo** = dal rumore (`WeatherPreset::from_date`), indipendente:
//!   tocca nuvole, luce, pioggia/neve che cade, ma NON le texture.
//! - Risultato: inverno sereno = neve a terra + sole visibile + cielo limpido.
//!
//! Nota: in inverno `from_date` ha `cold = true`, quindi il ramo umido produce
//! `Snow` e mai `Rain`/`Thunder`: d'inverno o nevica o è sereno/coperto/nebbia.
//! Coerente con la neve al suolo.
//!
//! Conseguenza: `Season::Rain` non è più prodotto dalla stagione (il mese non
//! lo genera). Se qualche sistema leggeva `data.season == Season::Rain` per
//! bagnare il terreno, quello stato ora non compare più: il "bagnato" andrebbe
//! guidato da `CurrentPreset` (meteo) a parte.
//!
//! ## Fix: meteo consapevole del clima locale
//!
//! `WeatherPreset::from_date` riceve anche il **clima locale** del
//! giocatore (`ClimateBase`). Il deserto (`ClimateBase::Desert`) non può
//! produrre neve: prima Sentinel (Hammerfell) mostrava la neve in Evening
//! Star / Morning Star / Sun's Dawn, nonostante `apply_climate` disattivasse
//! già le texture invernali del suolo.
//!
//! Inoltre, se il clima locale cambia (viaggio rapido, camminata tra map
//! pixel con climi diversi), il meteo automatico si riallinea **subito**
//! (senza aspettare `MIN_DWELL_HOURS`). Senza questo, chi viaggiava in
//! deserto mentre fuori nevicava vedeva la neve nel deserto per ore di gioco.
use bevy::prelude::*;
use crate::data::climate::{world_climate_settings, ClimateBase, Season};
use crate::dialog::DialogState;
use crate::equipment::EquipmentState;
use crate::player::Player;
use crate::sky::dream_sky::CurrentPreset;
use crate::sky::presets::WeatherPreset;
use crate::sky::SkyClock;
use crate::tiles::{climate_of, Streaming};
use crate::world::{map_pixel_of, GameData, HeightField, Origin, MAP_HEIGHT_PIXELS};

pub const MONTH_NAMES: [&str; 12] = [
    "Morning Star", "Sun's Dawn", "First Seed", "Rain's Hand", "Second Seed",
"Mid Year", "Sun's Height", "Last Seed", "Hearthfire", "Frostfall",
"Sun's Dusk", "Evening Star",
];

pub const WEEKDAY_NAMES: [&str; 7] = [
    "Sundas", "Morndas", "Tirdas", "Middas", "Turdas", "Fredas", "Loredas",
];

pub const HOURS_PER_DAY: f64 = 24.0;
pub const DAYS_PER_MONTH: i64 = 30;
pub const MONTHS_PER_YEAR: i64 = 12;
pub const DAYS_PER_YEAR: i64 = DAYS_PER_MONTH * MONTHS_PER_YEAR;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GameDate {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: f32,
}

impl Default for GameDate {
    fn default() -> Self {
        Self::new(405, 0, 5, 10.5)
    }
}

impl GameDate {
    pub fn new(year: i32, month: u8, day: u8, hour: f32) -> Self {
        Self {
            year,
            month: month % 12,
            day: day.clamp(1, 30),
            hour: hour.rem_euclid(24.0),
        }
    }

    pub fn season(&self) -> Season {
        match self.month {
            11 | 0 | 1 => Season::Winter,
            _ => Season::Summer,
        }
    }

    pub fn month_name(&self) -> &'static str {
        MONTH_NAMES[self.month as usize]
    }

    pub fn absolute_day(&self) -> i64 {
        self.year as i64 * DAYS_PER_YEAR
        + self.month as i64 * DAYS_PER_MONTH
        + (self.day as i64 - 1)
    }

    pub fn weekday(&self) -> u8 {
        (self.absolute_day().rem_euclid(7)) as u8
    }

    pub fn weekday_name(&self) -> &'static str {
        WEEKDAY_NAMES[self.weekday() as usize]
    }

    pub fn format_full(&self) -> String {
        format!(
            "{}, {} {} 3E {}",
            self.weekday_name(),
                self.day,
                self.month_name(),
                self.year
        )
    }

    pub fn format_date(&self) -> String {
        format!("{} {} 3E {}", self.day, self.month_name(), self.year)
    }

    pub fn format_time(&self) -> String {
        let h = self.hour.floor() as u32;
        let m = ((self.hour - h as f32) * 60.0).floor() as u32;
        format!("{h:02}:{m:02}")
    }

    pub fn add_hours(&mut self, hours: f64) {
        let mut total_hours = self.hour as f64 + hours;
        while total_hours >= HOURS_PER_DAY {
            total_hours -= HOURS_PER_DAY;
            self.add_days(1);
        }
        while total_hours < 0.0 {
            total_hours += HOURS_PER_DAY;
            self.add_days(-1);
        }
        self.hour = total_hours as f32;
    }

    pub fn add_days(&mut self, days: i64) {
        let total = self.absolute_day() + days;
        let year = total.div_euclid(DAYS_PER_YEAR);
        let rem = total.rem_euclid(DAYS_PER_YEAR);
        self.year = year as i32;
        self.month = (rem / DAYS_PER_MONTH) as u8;
        self.day = ((rem % DAYS_PER_MONTH) + 1) as u8;
    }
}

#[derive(Resource, Default, Debug)]
pub struct WeatherOverride(pub Option<WeatherPreset>);

impl WeatherOverride {
    pub fn label(&self) -> &'static str {
        match self.0 {
            None => "auto (calendario)",
            Some(p) => p.label(),
        }
    }
}

/// Ore di gioco minime tra due cambi di meteo automatici (evita scatti e,
/// soprattutto, ricostruzioni continue del terreno quando il rumore oscilla
/// attorno a una soglia).
///
/// **Eccezione**: un cambio di **clima locale** (`last_climate`) forza il
/// riallineamento immediato, indipendentemente dal dwell. Vedi `AutoWeather`
/// e `sync_season_from_calendar`.
const MIN_DWELL_HOURS: f64 = 2.0;

/// F4: forza i preset a rotazione (Soleggiato → ... → Neve → automatico).
pub fn cycle_weather(keys: Res<ButtonInput<KeyCode>>, mut weather: ResMut<WeatherOverride>) {
    if !keys.just_pressed(KeyCode::F4) {
        return;
    }
    weather.0 = match weather.0 {
        None => Some(WeatherPreset::Sunny),
        Some(WeatherPreset::Snow) => None,
        Some(p) => Some(p.next_manual()),
    };
    info!("Meteo: {}", weather.label());
}

/// Stato interno del meteo automatico.
pub struct AutoWeather {
    held: WeatherPreset,
    since_hours: f64,
    /// Il prossimo meteo automatico va applicato subito (primo avvio o
    /// ritorno da un override).
    snap: bool,
    initialized: bool,
    /// Ultimo clima visto: se cambia (es. temperato → deserto in viaggio
    /// rapido) il meteo automatico si riallinea subito, senza aspettare
    /// `MIN_DWELL_HOURS`. Serve a non far nevicare nel deserto.
    last_climate: Option<ClimateBase>,
}

impl Default for AutoWeather {
    fn default() -> Self {
        Self {
            held: WeatherPreset::Sunny,
            since_hours: 0.0,
            snap: true,
            initialized: false,
            last_climate: None,
        }
    }
}

/// `SkyClock.date ─► WeatherPreset` (meteo) e `clock.date.season()` (stagione).
///
/// 1. sceglie il preset: override (F4) oppure `WeatherPreset::from_date`,
///    con un tempo minimo di permanenza tra due cambi automatici **e**
///    riallineamento immediato al cambio di clima locale;
/// 2. lo scrive in `CurrentPreset` (da lì lo legge il cielo);
/// 3. ricava la `Season` **dal mese** (`clock.date.season()`, NON dal meteo)
///    e, se cambia, invalida i tile così terreno e alberi vengono ricostruiti
///    con la stagione giusta. Il meteo resta indipendente: un cambio
///    Neve→Sole non tocca più le texture.
pub fn sync_season_from_calendar(
    clock: Res<SkyClock>,
    weather: Res<WeatherOverride>,
    mut preset: ResMut<CurrentPreset>,
    mut data: ResMut<GameData>,
    mut streaming: ResMut<Streaming>,
    mut field: ResMut<HeightField>,
    mut commands: Commands,
    origin: Res<Origin>,
    player: Query<&Transform, With<Player>>,
    mut state: Local<AutoWeather>,
) {
    let now_hours = clock.date.absolute_day() as f64 * 24.0 + clock.date.hour as f64;

    // Clima locale del giocatore. Se il player non esiste ancora (primo
    // frame) o è in un interno/dungeon, `origin` descrive il map pixel
    // corrente, quindi il calcolo resta corretto.
    let (mx, my) = player
    .get_single()
    .map(|tf| map_pixel_of(&origin, tf.translation))
    .unwrap_or((origin.east, MAP_HEIGHT_PIXELS - origin.north));
    let climate = world_climate_settings(climate_of(&data, mx, my)).climate_type;

    let target = if let Some(forced) = weather.0 {
        state.snap = true; // quando l'override finisce, torna subito all'automatico
        forced
    } else {
        let auto = WeatherPreset::from_date(clock.date, climate);
        if now_hours < state.since_hours {
            state.since_hours = now_hours; // il tempo è tornato indietro (tasto ',')
        }
        // Cambio di clima: forza il riallineamento immediato del meteo
        // automatico. Senza questo, chi viaggia in deserto mentre fuori
        // nevica vedrebbe neve nel deserto per MIN_DWELL_HOURS ore di gioco.
        if state.last_climate != Some(climate) {
            state.last_climate = Some(climate);
            state.snap = true;
        }
        if state.snap || (auto != state.held && now_hours - state.since_hours >= MIN_DWELL_HOURS) {
            state.held = auto;
            state.since_hours = now_hours;
            state.snap = false;
        }
        state.held
    };

    if preset.current != target {
        info!(
            "Meteo: {} ({})",
              target.label(),
              if weather.0.is_some() { "forzato" } else { "calendario" }
        );
        preset.current = target;
    }

    // FIX: la stagione segue il CALENDARIO (mese), NON il meteo.
    // Prima `target.to_season()` legava le texture (neve/estate) al meteo
    // corrente: un cambio Neve→Sole ricostruiva tutti i tile "di botto"
    // (~1 min reale) e faceva sparire la neve a terra. Ora inverno = neve
    // a terra sempre, estate = verde sempre; il meteo (sole/pioggia/neve
    // che cade) è indipendente, quindi si può avere il sole d'inverno con
    // le texture innevate.
    let season = clock.date.season();
    if data.season != season {
        info!(
            "Stagione: {:?} (mese {})",
              season,
              clock.date.month_name()
        );
        data.season = season;
        // Al primo frame i tile non esistono ancora: niente da ricostruire.
        if state.initialized {
            streaming.invalidate_all_with(&mut commands);
            field.tiles.clear();
        }
    }

    state.initialized = true;
}

// =============================================================================
// UI
// =============================================================================

const FONT: &str = "fonts/Sherwood.ttf";
const PANEL_BG: Color = Color::srgba(0.05, 0.04, 0.03, 0.92);
const PANEL_BORDER: Color = Color::srgb(0.75, 0.58, 0.28);
const TITLE_COLOR: Color = Color::srgb(0.96, 0.78, 0.32);
const TEXT_COLOR: Color = Color::srgb(0.95, 0.93, 0.85);
const MUTED_COLOR: Color = Color::srgb(0.7, 0.65, 0.55);
const WINTER_COLOR: Color = Color::srgb(0.65, 0.82, 1.0);
const SUMMER_COLOR: Color = Color::srgb(0.98, 0.82, 0.42);
const RAIN_COLOR: Color = Color::srgb(0.55, 0.72, 0.85);
const OVERRIDE_COLOR: Color = Color::srgb(1.0, 0.65, 0.30);

#[derive(Component)]
pub struct CalendarRoot;
#[derive(Component)]
struct CalendarWeekdayText;
#[derive(Component)]
struct CalendarDateText;
#[derive(Component)]
struct CalendarSeasonText;
#[derive(Component)]
struct CalendarWeatherText;
#[derive(Component)]
struct CalendarTimeText;

#[derive(Resource, Default)]
pub struct CalendarUiState {
    pub open: bool,
}

fn setup_calendar_ui(mut commands: Commands, assets: Res<AssetServer>) {
    let font: Handle<Font> = assets.load(FONT);
    commands
    .spawn((
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        Visibility::Hidden,
        GlobalZIndex(150),
            CalendarRoot,
    ))
    .with_children(|root| {
        root.spawn((
            Node {
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(32.0)),
                    row_gap: Val::Px(10.0),
                    border: UiRect::all(Val::Px(2.0)),
                    align_items: AlignItems::Center,
                    ..default()
            },
            BackgroundColor(PANEL_BG),
                    BorderColor(PANEL_BORDER),
                    BorderRadius::all(Val::Px(12.0)),
        ))
        .with_children(|panel| {
            panel.spawn((
                Text::new("CALENDARIO"),
                         TextFont { font: font.clone(), font_size: 18.0, ..default() },
                         TextColor(TITLE_COLOR),
            ));
            panel.spawn((
                Node { width: Val::Px(80.0), height: Val::Px(2.0), ..default() },
                         BackgroundColor(TITLE_COLOR),
                         BorderRadius::all(Val::Px(1.0)),
            ));
            panel.spawn((
                Text::new(""),
                         TextFont { font: font.clone(), font_size: 30.0, ..default() },
                         TextColor(TEXT_COLOR),
                         CalendarWeekdayText,
            ));
            panel.spawn((
                Text::new(""),
                         TextFont { font: font.clone(), font_size: 20.0, ..default() },
                         TextColor(TEXT_COLOR),
                         CalendarDateText,
            ));
            panel.spawn((
                Text::new(""),
                         TextFont { font: font.clone(), font_size: 16.0, ..default() },
                         TextColor(SUMMER_COLOR),
                         CalendarSeasonText,
            ));
            panel.spawn((
                Text::new(""),
                         TextFont { font: font.clone(), font_size: 14.0, ..default() },
                         TextColor(OVERRIDE_COLOR),
                         Visibility::Hidden,
                         CalendarWeatherText,
            ));
            panel.spawn((
                Text::new(""),
                         TextFont { font: font.clone(), font_size: 14.0, ..default() },
                         TextColor(MUTED_COLOR),
                         CalendarTimeText,
            ));
            panel.spawn((
                Text::new("[O] chiudi  ·  [F4] cambia meteo"),
                         TextFont { font: font.clone(), font_size: 12.0, ..default() },
                         TextColor(MUTED_COLOR),
            ));
        });
    });
}

fn toggle_calendar_ui(
    keys: Res<ButtonInput<KeyCode>>,
    dialog: Res<DialogState>,
    equipment: Res<EquipmentState>,
    save_menu: Option<Res<crate::savegame::SaveMenuState>>,
    tuning_menu: Option<Res<crate::tuning_menu::TuningMenuState>>,
    mut state: ResMut<CalendarUiState>,
    mut roots: Query<&mut Visibility, With<CalendarRoot>>,
) {
    if dialog.open || equipment.open {
        return;
    }
    if save_menu.as_ref().is_some_and(|m| m.open) {
        return;
    }
    if tuning_menu.as_ref().is_some_and(|m| m.open) {
        return;
    }
    if !keys.just_pressed(KeyCode::KeyO) {
        return;
    }
    state.open = !state.open;
    for mut vis in &mut roots {
        *vis = if state.open { Visibility::Visible } else { Visibility::Hidden };
    }
}

#[allow(clippy::type_complexity)]
fn update_calendar_ui(
    clock: Res<SkyClock>,
    state: Res<CalendarUiState>,
    weather: Res<WeatherOverride>,
    preset: Res<CurrentPreset>,
    mut weekday_q: Query<
    &mut Text,
    (
        With<CalendarWeekdayText>,
     Without<CalendarDateText>,
     Without<CalendarSeasonText>,
     Without<CalendarWeatherText>,
     Without<CalendarTimeText>,
    ),
    >,
    mut date_q: Query<
    &mut Text,
    (
        With<CalendarDateText>,
     Without<CalendarWeekdayText>,
     Without<CalendarSeasonText>,
     Without<CalendarWeatherText>,
     Without<CalendarTimeText>,
    ),
    >,
    mut season_q: Query<
    (&mut Text, &mut TextColor),
                      (
                          With<CalendarSeasonText>,
                       Without<CalendarWeekdayText>,
                       Without<CalendarDateText>,
                       Without<CalendarWeatherText>,
                       Without<CalendarTimeText>,
                      ),
                      >,
                      mut weather_q: Query<
                      (&mut Text, &mut TextColor, &mut Visibility),
                      (
                          With<CalendarWeatherText>,
                       Without<CalendarWeekdayText>,
                       Without<CalendarDateText>,
                       Without<CalendarSeasonText>,
                       Without<CalendarTimeText>,
                      ),
                      >,
                      mut time_q: Query<
                      &mut Text,
                      (
                          With<CalendarTimeText>,
                       Without<CalendarWeekdayText>,
                       Without<CalendarDateText>,
                       Without<CalendarSeasonText>,
                       Without<CalendarWeatherText>,
                      ),
                      >,
                      mut last: Local<Option<(GameDate, Option<WeatherPreset>, WeatherPreset)>>,
) {
    if !state.open {
        return;
    }
    let date = clock.date;
    let key = (date, weather.0, preset.current);
    if Some(key) == *last {
        return;
    }
    *last = Some(key);

    if let Ok(mut txt) = weekday_q.get_single_mut() {
        txt.0 = date.weekday_name().to_string();
    }
    if let Ok(mut txt) = date_q.get_single_mut() {
        txt.0 = date.format_date();
    }
    if let Ok((mut txt, mut color)) = season_q.get_single_mut() {
        // FIX: la stagione mostrata segue il calendario (mese), coerente con
        // le texture a terra: non dipende più dal meteo corrente.
        let (name, col) = match clock.date.season() {
            Season::Winter => ("Inverno", WINTER_COLOR),
            Season::Summer => ("Estate", SUMMER_COLOR),
            // Non prodotto da date.season() (il mese genera solo Winter/Summer),
            // tenuto per esaustività del match.
            Season::Rain => ("Pioggia", RAIN_COLOR),
        };
        txt.0 = name.to_string();
        *color = TextColor(col);
    }
    if let Ok((mut txt, mut color, mut vis)) = weather_q.get_single_mut() {
        if weather.0.is_some() {
            txt.0 = format!("Meteo: {} (forzato, F4)", weather.label());
            *color = TextColor(OVERRIDE_COLOR);
            *vis = Visibility::Inherited;
        } else {
            txt.0 = format!("Meteo: {} (auto)", preset.current.label());
            *color = TextColor(MUTED_COLOR);
            *vis = Visibility::Inherited;
        }
    }
    if let Ok(mut txt) = time_q.get_single_mut() {
        txt.0 = format!("Ore {}", date.format_time());
    }
}

pub struct CalendarPlugin;

impl Plugin for CalendarPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CalendarUiState>()
        .init_resource::<WeatherOverride>()
        .add_systems(Startup, setup_calendar_ui)
        .add_systems(
            Update,
            (
                toggle_calendar_ui,
             cycle_weather,
             // Decide preset e stagione prima dello streaming dei tile.
             sync_season_from_calendar
             .after(cycle_weather)
             .before(crate::tiles::stream_tiles),
             update_calendar_ui,
            ),
        );
    }
}
