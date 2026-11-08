//! Mappa del viaggio rapido con ricerca (tasto S).
//!
//! ## Ricerca
//!
//! Con la mappa aperta (T), **S** apre un pannello di ricerca in alto a
//! destra. Si digita parte del nome di una località e la lista filtra in
//! tempo reale su `data.locations`. **Click** su una riga (o **Invio** per
//! la prima) → viaggio immediato alla località. **Esc** chiude solo la
//! ricerca (la mappa resta aperta); **Esc** di nuovo chiude la mappa.
//!
//! La ricerca ignora i filtri (Taverne/Negozi/Gilde/Palazzo): cercare
//! "sent" trova Sentinel anche se hai filtrato via le città.
//!
//! ## Consumo dei tasti "menu" (`suppress_menu_keys`)
//!
//! Quando la mappa è aperta, i caratteri digitati nella query (M, L, C, J,
//! O, X, Z, N, E, ...) sono normali caratteri di testo, ma gli altri
//! sistemi (`map::toggle_map`, `local_map::toggle_local_map`,
//! `equipment::toggle_equipment`, `journal::journal_input`,
//! `calendar::toggle_calendar_ui`, `calendar::cycle_weather`,
//! `savegame::detect_f2_save_menu`, `tuning_menu::detect_f1`,
//! `torch::torch_cycle`, `ship::cycle_ship_camera`,
//! `ship::ship_debug_recall`, `fps_weapon::update_weapon_input`,
//! `dialog::update_dialog`) li leggerebbero come comandi.
//!
//! `suppress_menu_keys` gira in `PreUpdate` dopo `handle_search_keyboard` e
//! prima di `dialog::update_dialog_followers` / `dialog::update_dialog`.
//! Chiama `ButtonInput::clear_just_pressed` su ogni tasto "menu" così gli
//! altri sistemi non li vedono. `T` (chiude la mappa), `S`, `Esc`, `Enter`
//! sono esclusi: li gestiscono `toggle_fast_travel`, `handle_search_keyboard`
//! e `handle_search_result_clicks`.
//!
//! Il `.before(crate::dialog::update_dialog_followers)` è necessario perché
//! `dialog::update_dialog_followers` e `dialog::update_dialog` sono in
//! `.chain()`: senza il `.before()` esplicito, il chain potrebbe far girare
//! `update_dialog_followers` prima di `suppress_menu_keys`, e quel sistema
//! aprirebbe un dialogo con un follower leggendo `E`.
//!
//! ## Visibilità di default
//!
//! Solo le **città** (`show_towns`) sono segnalate di default. Dungeon,
//! case e templi partono spenti: l'utente li attiva coi pulsanti in basso.
//! Player e nave (marker blu e viola) sono **sempre** visibili a
//! prescindere dai filtri.
//!
//! ## Esclusioni e priorità
//!
//! - Il tasto S del frame di apertura viene "consumato": non finisce nella
//!   query di ricerca. Un secondo S (pressione successiva) aggiunge 's'
//!   alla query come un carattere normale.
//! - Con il pannello di ricerca aperto, il click sulla mappa **non** viene
//!   catturato: `handle_map_click` ritorna early su `search_open` e il
//!   viaggio è gestito da `handle_search_result_clicks`.
//! - L'hint in alto a sinistra ("[S] Cerca") è visibile quando il pannello
//!   di ricerca è chiuso, nascosto quando è aperto.
//!
//! ## Dove è il giocatore sulla mappa
//!
//! Il marker "player" segue il map pixel (uguale a prima): se ti muovi
//! sulla mappa, la ricerca resta centrata su di te.

use crate::data::generate_default_samples;
use crate::data::heightmap::OCEAN_ELEVATION;
use crate::data::maps::DFLocation;
use crate::data::terrain_mesh::{
    terrain_height, CELL_SIZE, MAP_PIXEL_SIZE, TERRAIN_HEIGHT_SCALE,
};
use crate::mod_textures::ModTextures;
use crate::player::{Player, EYE_HEIGHT};
use crate::ship::Ship;
use crate::tiles::Streaming;
use crate::world::{
    map_pixel_of, tile_translation, GameData, Origin, MAP_HEIGHT_PIXELS, MAP_WIDTH_PIXELS,
};
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::MouseWheel;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::{CursorGrabMode, PrimaryWindow};

const UI_HEIGHT_FRAC: f32 = 0.20;
const VIEW_BOTTOM_FRAC: f32 = 0.888_889;

// CALIBRAZIONE APPLICATA DALL'UTENTE
const WORLD_FRAC: [f32; 4] = [0.04987, 0.05387, 0.94640, 0.86286];

// Offset di spawn in metri. X = Est, Y = Nord.
const SPAWN_OFFSET_M: Vec2 = Vec2::new(3.0, 3.0);

// Limiti dello zoom con la rotella
const MIN_ZOOM: f32 = 1.0;
const MAX_ZOOM: f32 = 20.0;
const ZOOM_STEP: f32 = 1.25;

// Colori dei marker
const PLAYER_OUTLINE: [u8; 4] = [255, 255, 255, 255];
const PLAYER_FILL: [u8; 4] = [0, 90, 255, 255];
const SHIP_OUTLINE: [u8; 4] = [255, 255, 255, 255];
const SHIP_FILL: [u8; 4] = [160, 40, 200, 255];

// Ricerca
const SEARCH_ROWS: usize = 12;
const SEARCH_MAX_QUERY: usize = 40;

/// Tasti "menu" che non devono essere visti dagli altri sistemi quando la
/// mappa del viaggio rapido è aperta.
///
/// `T` è escluso: chiude la mappa (`toggle_fast_travel`). `S`, `Escape`,
/// `Enter` sono esclusi: sono i tasti della ricerca (`handle_search_keyboard`
/// e `handle_search_result_clicks`). `Escape` in particolare deve arrivare a
/// `toggle_fast_travel` quando la ricerca è chiusa, per chiudere la mappa.
const MENU_KEYS: &[KeyCode] = &[
    KeyCode::KeyE,      // dialog::update_dialog / update_dialog_followers
KeyCode::KeyM,      // map::toggle_map (world map)
KeyCode::KeyL,      // local_map::toggle_local_map
KeyCode::KeyC,      // equipment::toggle_equipment
KeyCode::KeyJ,      // journal::journal_input
KeyCode::KeyO,      // calendar::toggle_calendar_ui
KeyCode::KeyX,      // torch::torch_cycle
KeyCode::KeyZ,      // ship::cycle_ship_camera / fps_weapon
KeyCode::KeyN,      // ship::ship_debug_recall
KeyCode::KeyF,      // player::player_move (toggle volo)
KeyCode::KeyP,      // sky::update_sky (pausa tempo)
KeyCode::F1,        // tuning_menu::detect_f1
KeyCode::F2,        // savegame::detect_f2_save_menu
KeyCode::F4,        // calendar::cycle_weather
KeyCode::F5,        // savegame::quicksave_f5
KeyCode::Comma,     // sky::update_sky (ora indietro)
KeyCode::Period,    // sky::update_sky (ora avanti)
KeyCode::Space,     // player_move / dungeon_move (salto)
KeyCode::Backspace, // dungeon_enter_exit / interior_enter_exit (esci)
];

#[derive(Component)]
pub struct FastTravelUi;

#[derive(Component)]
pub struct MapImageNode;

#[derive(Component)]
pub struct DotsImageNode;

#[derive(Component)]
pub struct UiImageNode;

#[derive(Component)]
pub struct HoverTooltip;

#[derive(Component)]
pub struct HoverTooltipText;

#[derive(Component)]
pub struct MapHint;

#[derive(Component)]
pub struct SearchPanel;

#[derive(Component)]
pub struct SearchInputText;

#[derive(Component)]
pub struct SearchResultButton(pub usize);

#[derive(Component)]
pub struct SearchResultLabel(pub usize);

#[derive(Component)]
pub struct SearchResultHint(pub usize);

#[derive(Component, Clone, Copy, PartialEq, Eq)]
pub enum FilterButton {
    Dungeons,
    Homes,
    Temples,
    Towns,
    Exit,
    Back,
}

#[derive(Resource)]
pub struct FastTravelState {
    pub is_open: bool,
    pub map_w: f32,
    pub map_h: f32,
    pub img_w: f32,
    pub img_h: f32,
    pub show_towns: bool,
    pub show_dungeons: bool,
    pub show_homes: bool,
    pub show_temples: bool,
    pub dots_handle: Handle<Image>,
    pub dots_dirty: bool,
    pub zoom_level: f32,
    pub zoom_center: Vec2,
    pub view_h_px: f32,
    pub view_h_ui: f32,
    pub world: Rect,
    pub search_open: bool,
    pub search_query: String,
    /// Risultati della ricerca: `(map pixel, nome)`. Riempito da
    /// `update_search_panel`, letto da `handle_search_result_clicks`.
    pub search_results: Vec<((i32, i32), String)>,
}

impl FastTravelState {
    fn view_rect(&self) -> Rect {
        let full = Vec2::new(self.img_w, self.view_h_px);
        if self.zoom_level <= 1.01 {
            return Rect { min: Vec2::ZERO, max: full };
        }
        let half = 0.5 / self.zoom_level;
        let cx = self.zoom_center.x.clamp(half, 1.0 - half);
        let cy = self.zoom_center.y.clamp(half, 1.0 - half);
        Rect {
            min: Vec2::new((cx - half) * full.x, (cy - half) * full.y),
            max: Vec2::new((cx + half) * full.x, (cy + half) * full.y),
        }
    }

    fn world_to_image(&self, wx: f32, wy: f32) -> Vec2 {
        let size = self.world.size();
        Vec2::new(
            self.world.min.x + wx / MAP_WIDTH_PIXELS as f32 * size.x,
            self.world.min.y + wy / MAP_HEIGHT_PIXELS as f32 * size.y,
        )
    }

    fn image_to_world(&self, p: Vec2) -> Vec2 {
        let size = self.world.size();
        Vec2::new(
            (p.x - self.world.min.x) / size.x * MAP_WIDTH_PIXELS as f32,
                  (p.y - self.world.min.y) / size.y * MAP_HEIGHT_PIXELS as f32,
        )
    }
}

impl Default for FastTravelState {
    fn default() -> Self {
        Self {
            is_open: false,
            map_w: 1000.0,
            map_h: 562.5,
            img_w: 1920.0,
            img_h: 1080.0,
            // Di default solo le città sono visibili. Dungeon, case e templi
            // partono spenti: si attivano coi pulsanti in basso. Player e
            // nave sono sempre visibili, indipendentemente dai filtri.
            show_towns: true,
            show_dungeons: false,
            show_homes: false,
            show_temples: false,
            dots_handle: Handle::default(),
            dots_dirty: true,
            zoom_level: 1.0,
            zoom_center: Vec2::new(0.5, 0.5),
            view_h_px: 960.0,
            view_h_ui: 500.0,
            world: Rect { min: Vec2::ZERO, max: Vec2::new(1920.0, 960.0) },
            search_open: false,
            search_query: String::new(),
            search_results: Vec::new(),
        }
    }
}

pub struct FastTravelPlugin;

impl Plugin for FastTravelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FastTravelState>()
        .add_systems(Startup, setup_fast_travel)
        // PreUpdate: `handle_search_keyboard` deve girare PRIMA di
        // `suppress_menu_keys` (che pulisce `ButtonInput`) e PRIMA di
        // `crate::dialog::update_dialog` (che altrimenti aprirebbe un
        // dialogo leggendo `E` durante la digitazione).
        .add_systems(
            PreUpdate,
            (
                handle_search_keyboard,
             // Pulisce i tasti "menu" per gli altri sistemi quando la
             // mappa è aperta. Vedi il doc del modulo.
             suppress_menu_keys
             .after(handle_search_keyboard)
             .before(crate::dialog::update_dialog_followers)
             .before(crate::dialog::update_dialog),
            )
            .after(bevy::input::InputSystem),
        )
        .add_systems(
            Update,
            (
                // `handle_search_keyboard` è in PreUpdate (vedi sopra), quindi
                // `toggle_fast_travel` può girare direttamente in Update:
                // `state.search_open` è già aggiornato.
                toggle_fast_travel,
             handle_filter_buttons,
             handle_map_click,
             handle_search_result_clicks.after(handle_map_click),
             update_search_panel,
             handle_scroll_zoom,
             calibrate_world_rect,
             debug_cursor_world,
             update_dots_texture,
             update_map_view_rect,
             update_hover_tooltip,
            )
            .before(crate::player::grab_cursor),
        );
    }
}

/// Consuma i tasti "menu" quando la mappa del viaggio è aperta.
///
/// `ButtonInput::clear_just_pressed(k)` rimuove `k` dalla lista
/// `just_pressed` del frame corrente. Gli altri sistemi (`map`,
/// `local_map`, `equipment`, `journal`, `calendar`, `torch`, `ship`,
/// `dialog`, `fps_weapon`, `player`, `sky`) usano `just_pressed`, quindi
/// con questa pulizia non reagiscono ai caratteri digitati nella query.
///
/// Gira in `PreUpdate` `.after(handle_search_keyboard)`, così i tasti sono
/// ancora visibili alla ricerca (che li legge come caratteri) ma non agli
/// altri sistemi. `.before(update_dialog_followers)` e
/// `.before(update_dialog)` evitano che il dialogo si apra leggendo `E`.
fn suppress_menu_keys(mut keys: ResMut<ButtonInput<KeyCode>>, state: Res<FastTravelState>) {
    if !state.is_open {
        return;
    }
    for key in MENU_KEYS {
        keys.clear_just_pressed(*key);
    }
}

fn classify_location(loc: &DFLocation) -> usize {
    let mut has_town = false;
    let mut has_temple = false;
    let mut has_dungeon = false;
    let mut has_home = false;

    for name in &loc.block_names {
        let prefix = if name.len() >= 4 { &name[..4] } else { "" };
        match prefix {
            "TVRN" | "GENR" | "WEAP" | "ARMR" | "ALCH" | "BANK" | "BOOK" | "CLOT" | "FURN"
            | "GEMS" | "LIBR" | "PAWN" | "PALA" | "CAST" | "WALL" | "CUST" | "MARK" | "MANS"
            | "BLDG" => has_town = true,
            "TEMP" | "PTEM" | "SHRI" | "MAGE" | "FIGH" | "THIE" | "DARK" | "GUIL" => {
                has_temple = true
            }
            "DUNG" | "RUIN" | "SHCK" | "GRVE" | "KRAV" | "KDRA" | "KOWL" | "KMOO" | "KCAN"
            | "KFLA" | "KHOR" | "KROS" | "KWHE" | "KSCA" | "KHAW" => has_dungeon = true,
            "RESI" | "MANR" | "FARM" | "HUT" => has_home = true,
            _ => {}
        }
    }

    if loc.width > 1 || loc.height > 1 {
        has_town = true;
    }

    if has_town {
        0
    } else if has_temple {
        2
    } else if has_dungeon {
        1
    } else if has_home {
        3
    } else {
        0
    }
}

fn setup_fast_travel(
    mut commands: Commands,
    mut mods: ResMut<ModTextures>,
    mut images: ResMut<Assets<Image>>,
    mut state: ResMut<FastTravelState>,
    asset_server: Res<AssetServer>,
) {
    let map_handle = mods.image_by_name("daggerfall", &mut images).unwrap_or_else(|| {
        let img = Image::new(
            Extent3d { width: 100, height: 100, depth_or_array_layers: 1 },
            TextureDimension::D2,
            vec![255, 0, 0, 255].repeat(100 * 100),
                             TextureFormat::Rgba8UnormSrgb,
                             RenderAssetUsages::default(),
        );
        images.add(img)
    });

    let (img_w, img_h) = if let Some(img) = images.get(&map_handle) {
        (img.width() as f32, img.height() as f32)
    } else {
        (1920.0, 1080.0)
    };
    state.img_w = img_w;
    state.img_h = img_h;

    let mut dots_img = Image::new(
        Extent3d { width: img_w as u32, height: img_h as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        vec![0; (img_w * img_h * 4.0) as usize],
                                  TextureFormat::Rgba8UnormSrgb,
                                  RenderAssetUsages::default(),
    );
    dots_img.sampler = bevy::image::ImageSampler::nearest();

    let dots_handle = images.add(dots_img);
    state.dots_handle = dots_handle.clone();

    state.world = Rect {
        min: Vec2::new(WORLD_FRAC[0] * img_w, WORLD_FRAC[1] * img_h),
        max: Vec2::new(WORLD_FRAC[2] * img_w, WORLD_FRAC[3] * img_h),
    };
    state.view_h_px = VIEW_BOTTOM_FRAC * img_h;

    let ui_w = 1000.0;
    let ui_h = ui_w * (img_h / img_w);
    let view_h_ui = ui_w * (state.view_h_px / img_w);
    let ui_part_h = ui_h * UI_HEIGHT_FRAC;
    state.map_w = ui_w;
    state.map_h = ui_h;
    state.view_h_ui = view_h_ui;

    let font: Handle<Font> = asset_server.load("fonts/Sherwood.ttf");

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
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.98)),
            Visibility::Hidden,
            GlobalZIndex(100),
            FastTravelUi,
    ))
    .with_children(|parent| {
        parent
        .spawn(Node {
            width: Val::Px(ui_w),
               height: Val::Px(ui_h),
               position_type: PositionType::Relative,
               ..default()
        })
        .with_children(|container| {
            container
            .spawn(Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                   top: Val::Px(0.0),
                   width: Val::Px(ui_w),
                   height: Val::Px(view_h_ui),
                   overflow: Overflow::clip(),
                   ..default()
            })
            .with_children(|map_parent| {
                map_parent.spawn((
                    ImageNode::new(map_handle.clone()),
                                  Node {
                                      width: Val::Percent(100.0),
                                  height: Val::Percent(100.0),
                                  ..default()
                                  },
                                  MapImageNode,
                ));
                map_parent.spawn((
                    ImageNode::new(dots_handle),
                                  Node {
                                      width: Val::Percent(100.0),
                                  height: Val::Percent(100.0),
                                  position_type: PositionType::Absolute,
                                  top: Val::Px(0.0),
                                  left: Val::Px(0.0),
                                  ..default()
                                  },
                                  DotsImageNode,
                ));
            });

            container
            .spawn(Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                   bottom: Val::Px(0.0),
                   width: Val::Px(ui_w),
                   height: Val::Px(ui_part_h),
                   ..default()
            })
            .with_children(|ui_parent| {
                ui_parent.spawn((
                    ImageNode::new(map_handle),
                                 Node {
                                     width: Val::Percent(100.0),
                                 height: Val::Percent(100.0),
                                 ..default()
                                 },
                                 UiImageNode,
                ));
            });

            let btn_style = |l, t, w, h| Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(l),
                       top: Val::Percent(t),
                       width: Val::Percent(w),
                       height: Val::Percent(h),
                       ..default()
            };

            let btn_y_top = 84.0;
            let btn_y_bot = 92.0;

            container.spawn((
                Button,
                FilterButton::Dungeons,
                btn_style(18.0, btn_y_top, 28.0, 8.0),
            ));
            container.spawn((
                Button,
                FilterButton::Temples,
                btn_style(18.0, btn_y_bot, 28.0, 8.0),
            ));
            container.spawn((
                Button,
                FilterButton::Homes,
                btn_style(46.5, btn_y_top, 31.5, 8.0),
            ));
            container.spawn((
                Button,
                FilterButton::Towns,
                btn_style(46.5, btn_y_bot, 31.5, 8.0),
            ));
            container.spawn((
                Button,
                FilterButton::Exit,
                btn_style(81.5, 85.0, 17.0, 14.0),
            ));
            container.spawn((
                Button,
                FilterButton::Back,
                btn_style(1.0, 84.0, 16.0, 16.0),
            ));

            container
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    padding: UiRect::all(Val::Px(4.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.1, 0.08, 0.05, 0.95)),
                    BorderColor(Color::srgb(0.8, 0.6, 0.2)),
                    Visibility::Hidden,
                    HoverTooltip,
                    GlobalZIndex(101),
            ))
            .with_children(|t| {
                t.spawn((
                    Text::new(""),
                         TextFont { font: font.clone(), font_size: 16.0, ..default() },
                         TextColor(Color::srgb(0.9, 0.8, 0.6)),
                         HoverTooltipText,
                ));
            });

            // Hint "[S] Cerca · Click mappa = viaggia · Esc = chiudi".
            container
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(16.0),
                    top: Val::Px(16.0),
                    padding: UiRect::all(Val::Px(6.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.78)),
                    BorderColor(Color::srgb(0.55, 0.45, 0.18)),
                    BorderRadius::all(Val::Px(4.0)),
                    MapHint,
            ))
            .with_children(|h| {
                h.spawn((
                    Text::new("[S] Cerca"),
                         TextFont { font: font.clone(), font_size: 13.0, ..default() },
                         TextColor(Color::srgb(0.95, 0.90, 0.70)),
                ));
            });

            // Pannello di ricerca (top-right, nascosto di default).
            container
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    right: Val::Px(20.0),
                    top: Val::Px(20.0),
                    width: Val::Px(360.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(10.0)),
                    row_gap: Val::Px(4.0),
                    border: UiRect::all(Val::Px(2.0)),
                    ..default()
                },
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.92)),
                    BorderColor(Color::srgb(0.80, 0.60, 0.20)),
                    BorderRadius::all(Val::Px(6.0)),
                    Visibility::Hidden,
                    SearchPanel,
                    GlobalZIndex(102),
            ))
            .with_children(|panel| {
                panel.spawn((
                    Text::new("Cerca località"),
                             TextFont { font: font.clone(), font_size: 15.0, ..default() },
                             TextColor(Color::srgb(0.95, 0.85, 0.40)),
                ));

                panel
                .spawn((
                    Node {
                        padding: UiRect::all(Val::Px(6.0)),
                        border: UiRect::all(Val::Px(1.0)),
                        min_height: Val::Px(28.0),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.05, 0.05, 0.10, 0.90)),
                        BorderColor(Color::srgb(0.40, 0.40, 0.50)),
                        BorderRadius::all(Val::Px(3.0)),
                ))
                .with_children(|input| {
                    input.spawn((
                        Text::new("_"),
                                 TextFont {
                                     font: font.clone(),
                                 font_size: 14.0,
                                 ..default()
                                 },
                                 TextColor(Color::srgb(0.95, 0.95, 0.95)),
                                 SearchInputText,
                    ));
                });

                panel.spawn((
                    Node {
                        width: Val::Percent(100.0),
                             height: Val::Px(1.0),
                             ..default()
                    },
                    BackgroundColor(Color::srgb(0.35, 0.35, 0.45)),
                ));

                for i in 0..SEARCH_ROWS {
                    panel
                    .spawn((
                        Button,
                        SearchResultButton(i),
                            Node {
                                width: Val::Percent(100.0),
                            padding: UiRect::new(
                                Val::Px(4.0),
                                                 Val::Px(4.0),
                                                 Val::Px(3.0),
                                                 Val::Px(3.0),
                            ),
                            min_height: Val::Px(22.0),
                            flex_direction: FlexDirection::Row,
                            justify_content: JustifyContent::SpaceBetween,
                            align_items: AlignItems::Center,
                            ..default()
                            },
                            BackgroundColor(Color::NONE),
                            Visibility::Hidden,
                    ))
                    .with_children(|row| {
                        row.spawn((
                            Text::new(""),
                                   TextFont {
                                       font: font.clone(),
                                   font_size: 13.0,
                                   ..default()
                                   },
                                   TextColor(Color::srgb(0.95, 0.93, 0.85)),
                                   SearchResultLabel(i),
                        ));
                        row.spawn((
                            Text::new(""),
                                   TextFont {
                                       font: font.clone(),
                                   font_size: 11.0,
                                   ..default()
                                   },
                                   TextColor(Color::srgb(0.65, 0.65, 0.70)),
                                   SearchResultHint(i),
                        ));
                    });
                }

                panel.spawn((
                    Text::new("Click / Invio = viaggia  ·  Esc = chiudi"),
                             TextFont { font, font_size: 11.0, ..default() },
                             TextColor(Color::srgb(0.55, 0.55, 0.60)),
                ));
            });
        });
    });
}

// ---------------------------------------------------------------------------
// Ricerca: input (gira in PreUpdate, prima di `suppress_menu_keys`)
// ---------------------------------------------------------------------------

fn handle_search_keyboard(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut key_events: EventReader<KeyboardInput>,
    mut state: ResMut<FastTravelState>,
) {
    if !state.is_open {
        for _ in key_events.read() {}
        state.search_open = false;
        state.search_query.clear();
        state.search_results.clear();
        return;
    }

    // Escape con ricerca aperta: chiude SOLO la ricerca.
    if state.search_open && keys.just_pressed(KeyCode::Escape) {
        state.search_open = false;
        state.search_query.clear();
        state.search_results.clear();
        keys.clear_just_pressed(KeyCode::Escape);
        for _ in key_events.read() {}
        return;
    }

    // S (ricerca chiusa) apre la ricerca. Il frame di apertura consuma gli
    // eventi, così 's' non finisce nella query.
    if !state.search_open && keys.just_pressed(KeyCode::KeyS) {
        state.search_open = true;
        state.search_query.clear();
        state.search_results.clear();
        for _ in key_events.read() {}
        return;
    }

    if !state.search_open {
        for _ in key_events.read() {}
        return;
    }

    // Input di testo.
    for ev in key_events.read() {
        if ev.state != ButtonState::Pressed {
            continue;
        }
        match &ev.logical_key {
            Key::Backspace => {
                state.search_query.pop();
            }
            Key::Space => {
                if state.search_query.chars().count() < SEARCH_MAX_QUERY {
                    state.search_query.push(' ');
                }
            }
            Key::Character(s) => {
                for c in s.chars() {
                    if !c.is_control()
                        && state.search_query.chars().count() < SEARCH_MAX_QUERY
                        {
                            state.search_query.push(c);
                        }
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Ricerca: pannello UI (visibilità, query, risultati)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn update_search_panel(
    time: Res<Time>,
    data: Res<GameData>,
    mut state: ResMut<FastTravelState>,
    mut panel_q: Query<
    &mut Visibility,
    (
        With<SearchPanel>,
     Without<MapHint>,
     Without<SearchResultButton>,
    ),
    >,
    mut hint_q: Query<
    &mut Visibility,
    (
        With<MapHint>,
     Without<SearchPanel>,
     Without<SearchResultButton>,
    ),
    >,
    mut input_q: Query<
    &mut Text,
    (
        With<SearchInputText>,
     Without<SearchResultLabel>,
     Without<SearchResultHint>,
    ),
    >,
    mut label_q: Query<
    (&SearchResultLabel, &mut Text),
                       (Without<SearchInputText>, Without<SearchResultHint>),
                       >,
                       mut row_vis_q: Query<
                       (&SearchResultButton, &mut Visibility),
                       (
                           Without<SearchPanel>,
                        Without<SearchInputText>,
                        Without<MapHint>,
                       ),
                       >,
                       mut last_query: Local<String>,
) {
    for mut vis in &mut panel_q {
        *vis = if state.search_open {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    for mut vis in &mut hint_q {
        *vis = if state.search_open {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }

    if !state.search_open {
        if !state.search_results.is_empty() {
            state.search_results.clear();
        }
        *last_query = String::new();
        return;
    }

    // Ricalcola i risultati quando la query cambia.
    if state.search_query != *last_query {
        *last_query = state.search_query.clone();
        state.search_results.clear();
        let q = state.search_query.to_ascii_lowercase();
        if !q.is_empty() {
            let mut matches: Vec<((i32, i32), String)> = data
            .locations
            .iter()
            .filter(|(_, loc)| loc.name.to_ascii_lowercase().contains(&q))
            .map(|(&k, loc)| (k, loc.name.clone()))
            .collect();
            matches.sort_by(|a, b| {
                a.1.to_ascii_lowercase().cmp(&b.1.to_ascii_lowercase())
            });
            matches.truncate(SEARCH_ROWS);
            state.search_results = matches;
        }
    }

    // Cursore lampeggiante.
    let cursor = if (time.elapsed_secs() * 2.0) as u32 % 2 == 0 {
        "_"
    } else {
        " "
    };
    let display = format!("{}{}", state.search_query, cursor);
    if let Ok(mut t) = input_q.get_single_mut() {
        t.0 = display;
    }

    for (btn, mut vis) in &mut row_vis_q {
        *vis = if btn.0 < state.search_results.len() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }

    for (lbl, mut txt) in &mut label_q {
        txt.0 = state
        .search_results
        .get(lbl.0)
        .map(|(_, n)| n.clone())
        .unwrap_or_default();
    }
}

// ---------------------------------------------------------------------------
// Ricerca: click su un risultato / Invio
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn handle_search_result_clicks(
    keys: Res<ButtonInput<KeyCode>>,
    mut interactions: Query<
    (&Interaction, &SearchResultButton),
                               (Changed<Interaction>, With<Button>),
                               >,
                               mut state: ResMut<FastTravelState>,
                               mut origin: ResMut<Origin>,
                               mut player: Query<(&mut Player, &mut Transform), Without<Ship>>,
                               mut streaming: ResMut<Streaming>,
                               mut commands: Commands,
                               data: Res<GameData>,
                               mut ui_query: Query<&mut Visibility, With<FastTravelUi>>,
                               mut windows: Query<&mut Window, With<PrimaryWindow>>,
                               mut respawn_ship: EventWriter<crate::ship::RespawnShipRequest>,
) {
    if !state.is_open || !state.search_open {
        return;
    }

    // 1. Click su una riga.
    let mut chosen: Option<usize> = None;
    for (interaction, btn) in &mut interactions {
        if *interaction == Interaction::Pressed && btn.0 < state.search_results.len() {
            chosen = Some(btn.0);
        }
    }
    // 2. Invio = primo risultato.
    if chosen.is_none()
        && keys.just_pressed(KeyCode::Enter)
        && !state.search_results.is_empty()
        {
            chosen = Some(0);
        }

        let Some(idx) = chosen else { return };
    let (mx, my) = state.search_results[idx].0;
    let Some(loc) = data.locations.get(&(mx, my)).cloned() else {
        return;
    };

    teleport_to_location(
        &mut origin,
        &mut streaming,
        &mut commands,
        &data,
        &mut player,
        &mut respawn_ship,
        mx,
        my,
        &loc,
    );

    state.is_open = false;
    state.search_open = false;
    state.search_query.clear();
    state.search_results.clear();
    for mut vis in &mut ui_query {
        *vis = Visibility::Hidden;
    }
    if let Ok(mut window) = windows.get_single_mut() {
        window.cursor_options.grab_mode = CursorGrabMode::Locked;
        window.cursor_options.visible = false;
    }
    info!(
        "Fast travel (ricerca): viaggio a '{}' ({}, {})",
          loc.name, mx, my
    );
}

// ---------------------------------------------------------------------------
// Viaggio (condiviso da mappa e ricerca)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn teleport_to_location(
    origin: &mut Origin,
    streaming: &mut Streaming,
    commands: &mut Commands,
    data: &GameData,
    player: &mut Query<(&mut Player, &mut Transform), Without<Ship>>,
                        respawn_ship: &mut EventWriter<crate::ship::RespawnShipRequest>,
                        target_mx: i32,
                        target_my: i32,
                        loc: &DFLocation,
) {
    origin.east = target_mx;
    origin.north = MAP_HEIGHT_PIXELS - target_my;

    let local_units_x = loc.world_x % 32768;
    let local_units_y = loc.world_y % 32768;

    let local_m_x = local_units_x as f32 * 0.025;
    let local_m_y = local_units_y as f32 * 0.025;

    let spawn_x = local_m_x + SPAWN_OFFSET_M.x;
    let spawn_y = local_m_y + SPAWN_OFFSET_M.y;

    let gx = spawn_x / CELL_SIZE;
    let gy = spawn_y / CELL_SIZE;

    let ground = match generate_default_samples(&data.woods, target_mx, target_my) {
        Ok(samples) => terrain_height(&samples, gx, gy),
        Err(_) => 0.0,
    };

    if let Ok((mut p, mut tf)) = player.get_single_mut() {
        p.feet_y = ground;
        p.vel_y = 0.0;
        p.on_ground = true;
        p.fly = false;
        tf.translation = Vec3::new(spawn_x, ground + EYE_HEIGHT, -spawn_y);
    }

    streaming.invalidate_all_with(commands);
    respawn_ship.send(crate::ship::RespawnShipRequest { remove_old: true });
}

// ---------------------------------------------------------------------------
// Toggle mappa (T) — in Update, dopo che `handle_search_keyboard` ha
// aggiornato `state.search_open` in PreUpdate.
// ---------------------------------------------------------------------------

fn toggle_fast_travel(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<FastTravelState>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut ui_query: Query<&mut Visibility, With<FastTravelUi>>,
    map_state: Option<Res<crate::map::MapState>>,
    dialog: Option<Res<crate::dialog::DialogState>>,
    equipment: Option<Res<crate::equipment::EquipmentState>>,
) {
    if map_state.is_some_and(|m| m.is_open)
        || dialog.is_some_and(|d| d.open)
        || equipment.is_some_and(|e| e.open)
        {
            return;
        }

        // Con la ricerca aperta, T è disabilitato: l'utente deve prima
        // chiudere la ricerca con Escape (gestito in handle_search_keyboard)
        // oppure col pulsante "Chiudi" del pannello. Anche Escape qui è
        // inerte, perché handle_search_keyboard lo consuma nello stesso
        // frame in cui chiude la ricerca.
        if state.search_open {
            return;
        }

        if keyboard.just_pressed(KeyCode::KeyT)
            || (state.is_open && keyboard.just_pressed(KeyCode::Escape))
            {
                state.is_open = !state.is_open;

                if state.is_open {
                    state.zoom_level = 1.0;
                    state.zoom_center = Vec2::new(0.5, 0.5);
                    state.dots_dirty = true;
                }

                // Reset della ricerca (aperta o chiusa che sia).
                state.search_open = false;
                state.search_query.clear();
                state.search_results.clear();

                for mut vis in &mut ui_query {
                    *vis = if state.is_open {
                        Visibility::Visible
                    } else {
                        Visibility::Hidden
                    };
                }
                if let Ok(mut window) = windows.get_single_mut() {
                    if state.is_open {
                        window.cursor_options.grab_mode = CursorGrabMode::None;
                        window.cursor_options.visible = true;
                    } else {
                        window.cursor_options.grab_mode = CursorGrabMode::Locked;
                        window.cursor_options.visible = false;
                    }
                }
            }
}

// ---------------------------------------------------------------------------
// Filtri
// ---------------------------------------------------------------------------

fn handle_filter_buttons(
    mut interactions: Query<(&Interaction, &FilterButton), Changed<Interaction>>,
                         mut state: ResMut<FastTravelState>,
                         mut ui_query: Query<&mut Visibility, With<FastTravelUi>>,
                         mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if !state.is_open {
        return;
    }

    for (interaction, button) in &mut interactions {
        if *interaction == Interaction::Pressed {
            match button {
                FilterButton::Towns => {
                    state.show_towns = !state.show_towns;
                    state.dots_dirty = true;
                }
                FilterButton::Dungeons => {
                    state.show_dungeons = !state.show_dungeons;
                    state.dots_dirty = true;
                }
                FilterButton::Homes => {
                    state.show_homes = !state.show_homes;
                    state.dots_dirty = true;
                }
                FilterButton::Temples => {
                    state.show_temples = !state.show_temples;
                    state.dots_dirty = true;
                }
                FilterButton::Exit => {
                    state.is_open = false;
                    state.search_open = false;
                    state.search_query.clear();
                    state.search_results.clear();
                    for mut vis in &mut ui_query {
                        *vis = Visibility::Hidden;
                    }
                    if let Ok(mut window) = windows.get_single_mut() {
                        window.cursor_options.grab_mode = CursorGrabMode::Locked;
                        window.cursor_options.visible = false;
                    }
                }
                FilterButton::Back => {
                    if state.zoom_level > 1.0 {
                        state.zoom_level = 1.0;
                        state.zoom_center = Vec2::new(0.5, 0.5);
                        state.dots_dirty = true;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Zoom con rotella
// ---------------------------------------------------------------------------

fn handle_scroll_zoom(
    mut wheel_events: EventReader<MouseWheel>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    ui_scale: Res<bevy::ui::UiScale>,
    mut state: ResMut<FastTravelState>,
) {
    if !state.is_open {
        return;
    }

    // Ricerca aperta: la rotella è per la lista, non per lo zoom. Consuma
    // gli eventi, altrimenti si accumulano e allo zoom 1.0 cambierebbe di
    // colpo appena chiudi la ricerca.
    if state.search_open {
        for _ in wheel_events.read() {}
        return;
    }

    let delta: f32 = wheel_events.read().map(|e| e.y).sum();
    if delta.abs() < 0.01 {
        return;
    }

    let Ok(mut window) = windows.get_single_mut() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };

    let scale = ui_scale.0.max(0.01);
    let cursor = cursor / scale;
    let win_w = window.width() / scale;
    let win_h = window.height() / scale;

    let map_left = (win_w - state.map_w) / 2.0;
    let map_top = (win_h - state.map_h) / 2.0;
    let click_h = state.map_h * (1.0 - UI_HEIGHT_FRAC);

    if cursor.x < map_left
        || cursor.x > map_left + state.map_w
        || cursor.y < map_top
        || cursor.y > map_top + click_h
        {
            return;
        }

        let u_vis = (cursor.x - map_left) / state.map_w;
    let v_vis = (cursor.y - map_top) / state.view_h_ui;

    let view = state.view_rect();
    let img_pt = view.min + Vec2::new(u_vis, v_vis) * view.size();
    let target_world = state.image_to_world(img_pt);

    if delta > 0.0 {
        state.zoom_level = (state.zoom_level * ZOOM_STEP).min(MAX_ZOOM);
    } else {
        state.zoom_level = (state.zoom_level / ZOOM_STEP).max(MIN_ZOOM);
    }

    if state.zoom_level > 1.01 {
        state.zoom_center = Vec2::new(
            target_world.x / MAP_WIDTH_PIXELS as f32,
            target_world.y / MAP_HEIGHT_PIXELS as f32,
        );
    } else {
        state.zoom_center = Vec2::new(0.5, 0.5);
    }

    state.dots_dirty = true;
}

// ---------------------------------------------------------------------------
// Puntini sulla mappa
// ---------------------------------------------------------------------------

fn update_dots_texture(
    mut state: ResMut<FastTravelState>,
    data: Res<GameData>,
    origin: Res<Origin>,
    player_query: Query<&Transform, With<Player>>,
    ship_query: Query<&Transform, With<Ship>>,
    mut images: ResMut<Assets<Image>>,
    mut last_ship_pos: Local<Option<Vec3>>,
) {
    if !state.is_open {
        return;
    }

    let ship_pos_now = ship_query.iter().next().map(|t| t.translation);
    let ship_changed = match (ship_pos_now, *last_ship_pos) {
        (Some(a), Some(b)) => a.distance_squared(b) > 0.01,
        (None, Some(_)) | (Some(_), None) => true,
        (None, None) => false,
    };
    if ship_changed {
        state.dots_dirty = true;
        *last_ship_pos = ship_pos_now;
    }

    if !state.dots_dirty {
        return;
    }
    state.dots_dirty = false;

    if let Some(dots_img) = images.get_mut(&state.dots_handle) {
        let w = dots_img.width() as usize;
        let h = dots_img.height() as usize;
        let data_img = dots_img.data.as_mut_slice();
        data_img.fill(0);

        let mut put = |p: Vec2, radius: isize, color: [u8; 4]| {
            let px = p.x.floor() as isize;
            let py = p.y.floor() as isize;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let x = px + dx;
                    let y = py + dy;
                    if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                        let idx = ((y as usize) * w + (x as usize)) * 4;
                        data_img[idx..idx + 4].copy_from_slice(&color);
                    }
                }
            }
        };

        if state.zoom_level > 1.01 {
            for (&(mx, my), loc) in data.locations.iter() {
                let loc_type = classify_location(loc);
                let draw = match loc_type {
                    0 => state.show_towns,
                    1 => state.show_dungeons,
                    2 => state.show_temples,
                    3 => state.show_homes,
                    _ => false,
                };
                if !draw {
                    continue;
                }

                let p = state.world_to_image(mx as f32 + 0.5, my as f32 + 0.5);
                let color = match loc_type {
                    1 => [255, 50, 50, 255],
                    2 => [255, 255, 50, 255],
                    3 => [50, 255, 50, 255],
                    _ => [255, 255, 255, 255],
                };
                put(p, 1, color);
            }
        }

        if let Ok(player_tf) = player_query.get_single() {
            let offset_x = player_tf.translation.x / MAP_PIXEL_SIZE;
            let offset_y = player_tf.translation.z / MAP_PIXEL_SIZE;
            let player_mx = origin.east as f32 + offset_x;
            let player_my = (MAP_HEIGHT_PIXELS - origin.north) as f32 + offset_y;

            let pp = state.world_to_image(player_mx, player_my);
            put(pp, 2, PLAYER_OUTLINE);
            put(pp, 1, PLAYER_FILL);
        }

        for ship_tf in &ship_query {
            let offset_x = ship_tf.translation.x / MAP_PIXEL_SIZE;
            let offset_y = ship_tf.translation.z / MAP_PIXEL_SIZE;
            let ship_mx = origin.east as f32 + offset_x;
            let ship_my = (MAP_HEIGHT_PIXELS - origin.north) as f32 + offset_y;

            let sp = state.world_to_image(ship_mx, ship_my);
            put(sp, 2, SHIP_OUTLINE);
            put(sp, 1, SHIP_FILL);
        }
    }
}

// ---------------------------------------------------------------------------
// Debug cursore / mappa
// ---------------------------------------------------------------------------

fn debug_cursor_world(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    ui_scale: Res<bevy::ui::UiScale>,
    state: Res<FastTravelState>,
    origin: Res<Origin>,
) {
    if !state.is_open || !mouse.just_pressed(MouseButton::Middle) {
        return;
    }
    // Ricerca aperta: il tasto centrale è disabilitato.
    if state.search_open {
        return;
    }
    let Ok(window) = windows.get_single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };

    let scale = ui_scale.0.max(0.01);
    let cursor = cursor / scale;
    let map_left = (window.width() / scale - state.map_w) / 2.0;
    let map_top = (window.height() / scale - state.map_h) / 2.0;
    let u_vis = (cursor.x - map_left) / state.map_w;
    let v_vis = (cursor.y - map_top) / state.view_h_ui;

    let view = state.view_rect();
    let img_pt = view.min + Vec2::new(u_vis, v_vis) * view.size();
    let world = state.image_to_world(img_pt);
    info!(
        "fast travel: cursore -> immagine ({:.1}, {:.1}) = frazione ({:.5}, {:.5}) | pixel mondo ({:.1}, {:.1}) | giocatore pixel mondo ({}, {})",
          img_pt.x, img_pt.y, img_pt.x / state.img_w, img_pt.y / state.img_h,
          world.x, world.y,
          origin.east, MAP_HEIGHT_PIXELS - origin.north
    );
}

// ---------------------------------------------------------------------------
// Click sulla mappa (esclude click sui bottoni)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn handle_map_click(
    mouse: Res<ButtonInput<MouseButton>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    ui_scale: Res<bevy::ui::UiScale>,
    mut state: ResMut<FastTravelState>,
    data: Res<GameData>,
    mut origin: ResMut<Origin>,
    mut player: Query<(&mut Player, &mut Transform), Without<Ship>>,
                    mut streaming: ResMut<Streaming>,
                    mut commands: Commands,
                    mut ui_query: Query<&mut Visibility, With<FastTravelUi>>,
                    interactions: Query<&Interaction, With<Button>>,
                    mut ship_state: Option<ResMut<crate::ship::ShipState>>,
                    mut ships: Query<(Entity, &mut Transform), (With<Ship>, Without<Player>)>,
                    mut respawn_ship: EventWriter<crate::ship::RespawnShipRequest>,
) {
    if !state.is_open {
        return;
    }

    // Ricerca aperta: nessun click sulla mappa viene processato (né il
    // tasto destro per resettare lo zoom, né il sinistro per nave/località).
    // I click sui risultati della ricerca sono gestiti da
    // `handle_search_result_clicks`, che gira dopo.
    if state.search_open {
        return;
    }

    if mouse.just_pressed(MouseButton::Right) && state.zoom_level > 1.0 {
        state.zoom_level = 1.0;
        state.zoom_center = Vec2::new(0.5, 0.5);
        state.dots_dirty = true;
        return;
    }

    if !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    // Se un qualsiasi Button è Pressed in questo frame, il click è per la UI
    // (filtri, exit, oppure una riga della ricerca): non è un click sulla mappa.
    if interactions.iter().any(|i| *i == Interaction::Pressed) {
        return;
    }

    let Ok(mut window) = windows.get_single_mut() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };

    let scale = ui_scale.0.max(0.01);
    let cursor = cursor / scale;
    let win_w = window.width() / scale;
    let win_h = window.height() / scale;

    let map_left = (win_w - state.map_w) / 2.0;
    let map_top = (win_h - state.map_h) / 2.0;
    let click_h = state.map_h * (1.0 - UI_HEIGHT_FRAC);

    if cursor.x < map_left
        || cursor.x > map_left + state.map_w
        || cursor.y < map_top
        || cursor.y > map_top + click_h
        {
            return;
        }

        let u_vis = (cursor.x - map_left) / state.map_w;
    let v_vis = (cursor.y - map_top) / state.view_h_ui;

    let view = state.view_rect();
    let img_pt = view.min + Vec2::new(u_vis, v_vis) * view.size();

    // --- Click sul puntino nave ---
    if let Some((ship_entity, mut ship_tf)) = ships.iter_mut().next() {
        let offset_x = ship_tf.translation.x / MAP_PIXEL_SIZE;
        let offset_y = ship_tf.translation.z / MAP_PIXEL_SIZE;
        let ship_mx = origin.east as f32 + offset_x;
        let ship_my = (MAP_HEIGHT_PIXELS - origin.north) as f32 + offset_y;
        let ship_img = state.world_to_image(ship_mx, ship_my);

        let threshold = (25.0 / state.zoom_level.sqrt()).max(10.0);
        if img_pt.distance(ship_img) < threshold {
            info!("Fast travel: teletrasporto alla nave");

            let (ship_px, ship_py) = map_pixel_of(&origin, ship_tf.translation);
            let old_tile = tile_translation(&origin, ship_px, ship_py);
            let offset_in_tile = ship_tf.translation - old_tile;

            origin.east = ship_px;
            origin.north = MAP_HEIGHT_PIXELS - ship_py;

            let new_ship_pos = offset_in_tile;
            ship_tf.translation = new_ship_pos;

            let player_offset = ship_tf.rotation * Vec3::new(20.0, 0.0, 0.0);
            let player_pos = new_ship_pos + player_offset;
            let ground = OCEAN_ELEVATION * TERRAIN_HEIGHT_SCALE;

            if let Ok((mut p, mut ptf)) = player.get_single_mut() {
                p.feet_y = ground;
                p.vel_y = 0.0;
                p.on_ground = true;
                p.fly = false;
                ptf.translation = Vec3::new(player_pos.x, ground + EYE_HEIGHT, player_pos.z);
            }

            if let Some(st) = ship_state.as_mut() {
                st.player_aboard = Some(ship_entity);
            }

            streaming.invalidate_all_with(&mut commands);

            state.is_open = false;
            state.search_open = false;
            state.search_query.clear();
            state.search_results.clear();
            for mut vis in &mut ui_query {
                *vis = Visibility::Hidden;
            }
            window.cursor_options.grab_mode = CursorGrabMode::Locked;
            window.cursor_options.visible = false;
            return;
        }
    }

    // --- Zoom-in al primo click se a zoom 1.0 ---
    if state.zoom_level <= 1.01 {
        state.zoom_center = Vec2::new(u_vis, v_vis);
        state.zoom_level = 4.0;
        state.dots_dirty = true;
        return;
    }

    let view = state.view_rect();
    let img_pt = view.min + Vec2::new(u_vis, v_vis) * view.size();
    let target = state.image_to_world(img_pt);

    let mut best_loc: Option<((i32, i32), &DFLocation)> = None;
    let mut min_dist = f32::MAX;

    for (&(mx, my), loc) in data.locations.iter() {
        let dx = mx as f32 + 0.5 - target.x;
        let dy = my as f32 + 0.5 - target.y;
        let dist = (dx * dx + dy * dy).sqrt();

        if dist < 30.0 {
            let loc_type = classify_location(loc);
            let draw = match loc_type {
                0 => state.show_towns,
                1 => state.show_dungeons,
                2 => state.show_temples,
                3 => state.show_homes,
                _ => false,
            };
            if draw && dist < min_dist {
                min_dist = dist;
                best_loc = Some(((mx, my), loc));
            }
        }
    }

    if let Some(((target_mx, target_my), loc)) = best_loc {
        teleport_to_location(
            &mut origin,
            &mut streaming,
            &mut commands,
            &data,
            &mut player,
            &mut respawn_ship,
            target_mx,
            target_my,
            loc,
        );

        state.is_open = false;
        state.search_open = false;
        state.search_query.clear();
        state.search_results.clear();
        for mut vis in &mut ui_query {
            *vis = Visibility::Hidden;
        }
        window.cursor_options.grab_mode = CursorGrabMode::Locked;
        window.cursor_options.visible = false;
    }
}

// ---------------------------------------------------------------------------
// Tooltip hover
// ---------------------------------------------------------------------------

fn update_hover_tooltip(
    windows: Query<&Window, With<PrimaryWindow>>,
    ui_scale: Res<bevy::ui::UiScale>,
    state: Res<FastTravelState>,
    data: Res<GameData>,
    interactions: Query<&Interaction, With<Button>>,
    mut tooltip_query: Query<(&mut Node, &mut Visibility), With<HoverTooltip>>,
                        mut text_query: Query<&mut Text, With<HoverTooltipText>>,
) {
    let Ok((mut tooltip_node, mut tooltip_vis)) = tooltip_query.get_single_mut() else {
        return;
    };
    let Ok(mut tooltip_text) = text_query.get_single_mut() else {
        return;
    };

    if !state.is_open || state.zoom_level <= 1.01 || state.search_open {
        *tooltip_vis = Visibility::Hidden;
        return;
    }

    let Ok(window) = windows.get_single() else {
        return;
    };
    let Some(cursor) = window.cursor_position() else {
        return;
    };

    let scale = ui_scale.0.max(0.01);
    let cursor = cursor / scale;
    let win_w = window.width() / scale;
    let win_h = window.height() / scale;

    let map_left = (win_w - state.map_w) / 2.0;
    let map_top = (win_h - state.map_h) / 2.0;
    let click_h = state.map_h * (1.0 - UI_HEIGHT_FRAC);

    if cursor.x < map_left
        || cursor.x > map_left + state.map_w
        || cursor.y < map_top
        || cursor.y > map_top + click_h
        || interactions.iter().any(|i| *i == Interaction::Pressed)
        {
            *tooltip_vis = Visibility::Hidden;
            return;
        }

        let u_vis = (cursor.x - map_left) / state.map_w;
    let v_vis = (cursor.y - map_top) / state.view_h_ui;

    let view = state.view_rect();
    let img_pt = view.min + Vec2::new(u_vis, v_vis) * view.size();
    let target = state.image_to_world(img_pt);

    let mut best_name: Option<String> = None;
    let mut min_dist = f32::MAX;

    for (&(mx, my), loc) in data.locations.iter() {
        let dx = mx as f32 + 0.5 - target.x;
        let dy = my as f32 + 0.5 - target.y;
        let dist = (dx * dx + dy * dy).sqrt();

        if dist < 30.0 {
            let loc_type = classify_location(loc);
            let draw = match loc_type {
                0 => state.show_towns,
                1 => state.show_dungeons,
                2 => state.show_temples,
                3 => state.show_homes,
                _ => false,
            };
            if draw && dist < min_dist {
                min_dist = dist;
                best_name = Some(loc.name.clone());
            }
        }
    }

    if let Some(name) = best_name {
        tooltip_text.0 = name;
        *tooltip_vis = Visibility::Visible;

        tooltip_node.left = Val::Px(cursor.x + 15.0);
        tooltip_node.top = Val::Px(cursor.y + 15.0);
    } else {
        *tooltip_vis = Visibility::Hidden;
    }
}

// ---------------------------------------------------------------------------
// Calibrazione / view
// ---------------------------------------------------------------------------

fn calibrate_world_rect(
    keyboard: Res<ButtonInput<KeyCode>>,
    origin: Res<Origin>,
    mut state: ResMut<FastTravelState>,
) {
    if !state.is_open {
        return;
    }
    // Ricerca aperta: Ctrl+frecce e PageUp/PageDown sono disabilitati.
    if state.search_open {
        return;
    }
    let ctrl =
    keyboard.pressed(KeyCode::ControlLeft) || keyboard.pressed(KeyCode::ControlRight);
    if !ctrl {
        return;
    }
    let alt = keyboard.pressed(KeyCode::AltLeft) || keyboard.pressed(KeyCode::AltRight);
    let shift = keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight);

    let move_step = if shift { 1.0 } else { 0.25 };
    let scale_step = if shift { 0.01 } else { 0.001 };

    let mut dir = Vec2::ZERO;
    if keyboard.just_pressed(KeyCode::ArrowLeft) {
        dir.x -= 1.0;
    }
    if keyboard.just_pressed(KeyCode::ArrowRight) {
        dir.x += 1.0;
    }
    if keyboard.just_pressed(KeyCode::ArrowUp) {
        dir.y += 1.0;
    }
    if keyboard.just_pressed(KeyCode::ArrowDown) {
        dir.y -= 1.0;
    }
    let mut uniform = 0.0;
    if keyboard.just_pressed(KeyCode::PageUp) {
        uniform += 1.0;
    }
    if keyboard.just_pressed(KeyCode::PageDown) {
        uniform -= 1.0;
    }

    if dir == Vec2::ZERO && uniform == 0.0 {
        return;
    }

    let mut f = Vec2::ONE;
    if alt {
        f.x += dir.x * scale_step;
        f.y += dir.y * scale_step;
    } else {
        let d = Vec2::new(dir.x * move_step, -dir.y * move_step);
        state.world.min += d;
        state.world.max += d;
    }
    if uniform != 0.0 {
        f += Vec2::splat(uniform * scale_step);
    }

    if f != Vec2::ONE {
        let anchor = state.world_to_image(
            origin.east as f32 + 0.5,
            (MAP_HEIGHT_PIXELS - origin.north) as f32 + 0.5,
        );
        let (min, max) = (state.world.min, state.world.max);
        state.world.min = anchor + (min - anchor) * f;
        state.world.max = anchor + (max - anchor) * f;
    }
    state.dots_dirty = true;

    let (w, h) = (state.img_w, state.img_h);
    let size = state.world.size();
    info!(
        "fast travel: scala X={:.4} Y={:.4} | const WORLD_FRAC: [f32; 4] = [{:.5}, {:.5}, {:.5}, {:.5}];",
        size.x / w, size.y / (w * 0.5),
          state.world.min.x / w, state.world.min.y / h,
          state.world.max.x / w, state.world.max.y / h
    );
}

fn update_map_view_rect(
    state: Res<FastTravelState>,
    mut map_query: Query<
    &mut ImageNode,
    (With<MapImageNode>, Without<DotsImageNode>, Without<UiImageNode>),
                        >,
                        mut dots_query: Query<
                        &mut ImageNode,
                        (With<DotsImageNode>, Without<MapImageNode>, Without<UiImageNode>),
                        >,
                        mut ui_query: Query<
                        &mut ImageNode,
                        (With<UiImageNode>, Without<MapImageNode>, Without<DotsImageNode>),
                        >,
) {
    if !state.is_open {
        return;
    }

    let map_rect = Some(state.view_rect());

    let ui_rect = Some(Rect {
        min: Vec2::new(0.0, state.img_h * (1.0 - UI_HEIGHT_FRAC)),
                       max: Vec2::new(state.img_w, state.img_h),
    });

    for mut img in &mut map_query {
        img.rect = map_rect;
    }
    for mut img in &mut dots_query {
        img.rect = map_rect;
    }
    for mut ui in &mut ui_query {
        ui.rect = ui_rect;
    }
}
