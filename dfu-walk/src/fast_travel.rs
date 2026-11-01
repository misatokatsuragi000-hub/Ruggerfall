use crate::data::generate_default_samples;
use crate::data::maps::DFLocation;
use crate::data::terrain_mesh::{terrain_height, MAP_PIXEL_SIZE, CELL_SIZE};
use crate::mod_textures::ModTextures;
use crate::player::{Player, EYE_HEIGHT};
use crate::tiles::Streaming;
use crate::world::{GameData, MAP_HEIGHT_PIXELS, MAP_WIDTH_PIXELS, Origin};
use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::{CursorGrabMode, PrimaryWindow};

const UI_HEIGHT_FRAC: f32 = 0.20;
const VIEW_BOTTOM_FRAC: f32 = 0.888_889;

// CALIBRAZIONE APPLICATA DALL'UTENTE
const WORLD_FRAC: [f32; 4] = [0.04987, 0.05387, 0.94640, 0.86286];

// Offset di spawn in metri. X = Est, Y = Nord.
const SPAWN_OFFSET_M: Vec2 = Vec2::new(300.0, 300.0);

// Limiti dello zoom con la rotella
const MIN_ZOOM: f32 = 1.0;
const MAX_ZOOM: f32 = 20.0;
const ZOOM_STEP: f32 = 1.25;

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
            show_towns: true,
            show_dungeons: true,
            show_homes: true,
            show_temples: true,
            dots_handle: Handle::default(),
            dots_dirty: true,
            zoom_level: 1.0,
            zoom_center: Vec2::new(0.5, 0.5),
            view_h_px: 960.0,
            view_h_ui: 500.0,
            world: Rect { min: Vec2::ZERO, max: Vec2::new(1920.0, 960.0) },
        }
    }
}

pub struct FastTravelPlugin;

impl Plugin for FastTravelPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FastTravelState>()
        .add_systems(Startup, setup_fast_travel)
        .add_systems(Update, (
            toggle_fast_travel,
            handle_filter_buttons,
            handle_map_click,
            handle_scroll_zoom,
            calibrate_world_rect,
            debug_cursor_world,
            update_dots_texture,
            update_map_view_rect,
            update_hover_tooltip,
        ).before(crate::player::grab_cursor));
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
            "TVRN" | "GENR" | "WEAP" | "ARMR" | "ALCH" | "BANK" | "BOOK" | "CLOT" | "FURN" | "GEMS" | "LIBR" | "PAWN" | "PALA" | "CAST" | "WALL" | "CUST" | "MARK" | "MANS" | "BLDG" => has_town = true,
            "TEMP" | "PTEM" | "SHRI" | "MAGE" | "FIGH" | "THIE" | "DARK" | "GUIL" => has_temple = true,
            "DUNG" | "RUIN" | "SHCK" | "GRVE" | "KRAV" | "KDRA" | "KOWL" | "KMOO" | "KCAN" | "KFLA" | "KHOR" | "KROS" | "KWHE" | "KSCA" | "KHAW" => has_dungeon = true,
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

    commands.spawn((
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
        parent.spawn((
            Node { width: Val::Px(ui_w), height: Val::Px(ui_h), position_type: PositionType::Relative, ..default() },
        )).with_children(|container| {
            container.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0), top: Val::Px(0.0),
                             width: Val::Px(ui_w), height: Val::Px(view_h_ui),
                             overflow: Overflow::clip(),
                             ..default()
                },
            )).with_children(|map_parent| {
                map_parent.spawn((
                    ImageNode::new(map_handle.clone()),
                                  Node { width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
                                  MapImageNode,
                ));
                map_parent.spawn((
                    ImageNode::new(dots_handle),
                                  Node { width: Val::Percent(100.0), height: Val::Percent(100.0), position_type: PositionType::Absolute, top: Val::Px(0.0), left: Val::Px(0.0), ..default() },
                                  DotsImageNode,
                ));
            });

            container.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0), bottom: Val::Px(0.0),
                             width: Val::Px(ui_w), height: Val::Px(ui_part_h),
                             ..default()
                },
            )).with_children(|ui_parent| {
                ui_parent.spawn((
                    ImageNode::new(map_handle),
                                 Node { width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
                                 UiImageNode,
                ));
            });

            let btn_style = |l, t, w, h| Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(l), top: Val::Percent(t),
                         width: Val::Percent(w), height: Val::Percent(h),
                         ..default()
            };

            let btn_y_top = 84.0;
            let btn_y_bot = 92.0;

            container.spawn((Button, FilterButton::Dungeons, btn_style(18.0, btn_y_top, 28.0, 8.0)));
            container.spawn((Button, FilterButton::Temples, btn_style(18.0, btn_y_bot, 28.0, 8.0)));
            container.spawn((Button, FilterButton::Homes, btn_style(46.5, btn_y_top, 31.5, 8.0)));
            container.spawn((Button, FilterButton::Towns, btn_style(46.5, btn_y_bot, 31.5, 8.0)));
            container.spawn((Button, FilterButton::Exit, btn_style(81.5, 85.0, 17.0, 14.0)));
            container.spawn((Button, FilterButton::Back, btn_style(1.0, 84.0, 16.0, 16.0)));

            container.spawn((
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
            )).with_children(|t| {
                t.spawn((
                    Text::new(""),
                         TextFont { font: font.clone(), font_size: 16.0, ..default() },
                         TextColor(Color::srgb(0.9, 0.8, 0.6)),
                         HoverTooltipText,
                ));
            });
        });
    });
}

fn toggle_fast_travel(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<FastTravelState>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut ui_query: Query<&mut Visibility, With<FastTravelUi>>,
    map_state: Option<Res<crate::map::MapState>>,
    dialog: Option<Res<crate::dialog::DialogState>>,
    equipment: Option<Res<crate::equipment::EquipmentState>>,
) {
    if map_state.is_some_and(|m| m.is_open) || dialog.is_some_and(|d| d.open) || equipment.is_some_and(|e| e.open) {
        return;
    }

    if keyboard.just_pressed(KeyCode::KeyT) || (state.is_open && keyboard.just_pressed(KeyCode::Escape)) {
        state.is_open = !state.is_open;

        if state.is_open {
            state.zoom_level = 1.0;
            state.zoom_center = Vec2::new(0.5, 0.5);
            state.dots_dirty = true;
        }

        for mut vis in &mut ui_query {
            *vis = if state.is_open { Visibility::Visible } else { Visibility::Hidden };
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

fn handle_filter_buttons(
    mut interactions: Query<(&Interaction, &FilterButton), Changed<Interaction>>,
                         mut state: ResMut<FastTravelState>,
                         mut ui_query: Query<&mut Visibility, With<FastTravelUi>>,
                         mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if !state.is_open { return; }

    for (interaction, button) in &mut interactions {
        if *interaction == Interaction::Pressed {
            match button {
                FilterButton::Towns => { state.show_towns = !state.show_towns; state.dots_dirty = true; }
                FilterButton::Dungeons => { state.show_dungeons = !state.show_dungeons; state.dots_dirty = true; }
                FilterButton::Homes => { state.show_homes = !state.show_homes; state.dots_dirty = true; }
                FilterButton::Temples => { state.show_temples = !state.show_temples; state.dots_dirty = true; }
                FilterButton::Exit => {
                    state.is_open = false;
                    for mut vis in &mut ui_query { *vis = Visibility::Hidden; }
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

fn handle_scroll_zoom(
    mut wheel_events: EventReader<MouseWheel>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    ui_scale: Res<bevy::ui::UiScale>,
    mut state: ResMut<FastTravelState>,
) {
    if !state.is_open { return; }

    let delta: f32 = wheel_events.read().map(|e| e.y).sum();
    if delta.abs() < 0.01 { return; }

    let Ok(mut window) = windows.get_single_mut() else { return };
    let Some(cursor) = window.cursor_position() else { return };

    let scale = ui_scale.0.max(0.01);
    let cursor = cursor / scale;
    let win_w = window.width() / scale;
    let win_h = window.height() / scale;

    let map_left = (win_w - state.map_w) / 2.0;
    let map_top = (win_h - state.map_h) / 2.0;
    let click_h = state.map_h * (1.0 - UI_HEIGHT_FRAC);

    if cursor.x < map_left || cursor.x > map_left + state.map_w
        || cursor.y < map_top || cursor.y > map_top + click_h
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

fn update_dots_texture(
    mut state: ResMut<FastTravelState>,
    data: Res<GameData>,
    origin: Res<Origin>,
    player_query: Query<&Transform, With<Player>>,
    mut images: ResMut<Assets<Image>>,
) {
    if !state.is_open || !state.dots_dirty { return; }
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
                if !draw { continue; }

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
            let offset_y = -player_tf.translation.z / MAP_PIXEL_SIZE;
            let player_mx = origin.east as f32 + offset_x;
            let player_my = (MAP_HEIGHT_PIXELS - origin.north) as f32 + offset_y;

            let pp = state.world_to_image(player_mx, player_my);
            put(pp, 2, [255, 255, 255, 255]);
            put(pp, 1, [0, 90, 255, 255]);
        }
    }
}

fn debug_cursor_world(
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    ui_scale: Res<bevy::ui::UiScale>,
    state: Res<FastTravelState>,
    origin: Res<Origin>,
) {
    if !state.is_open || !mouse.just_pressed(MouseButton::Middle) { return; }
    let Ok(window) = windows.get_single() else { return };
    let Some(cursor) = window.cursor_position() else { return };

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

fn handle_map_click(
    mouse: Res<ButtonInput<MouseButton>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    ui_scale: Res<bevy::ui::UiScale>,
    mut state: ResMut<FastTravelState>,
    data: Res<GameData>,
    mut origin: ResMut<Origin>,
    mut player: Query<(&mut Player, &mut Transform)>,
                    mut streaming: ResMut<Streaming>,
                    mut commands: Commands,
                    mut ui_query: Query<&mut Visibility, With<FastTravelUi>>,
                    interactions: Query<&Interaction, With<Button>>,
) {
    if !state.is_open { return; }

    if mouse.just_pressed(MouseButton::Right) && state.zoom_level > 1.0 {
        state.zoom_level = 1.0;
        state.zoom_center = Vec2::new(0.5, 0.5);
        state.dots_dirty = true;
        return;
    }

    if !mouse.just_pressed(MouseButton::Left) { return; }
    if interactions.iter().any(|i| *i == Interaction::Pressed) { return; }

    let Ok(mut window) = windows.get_single_mut() else { return };
    let Some(cursor) = window.cursor_position() else { return };

    let scale = ui_scale.0.max(0.01);
    let cursor = cursor / scale;
    let win_w = window.width() / scale;
    let win_h = window.height() / scale;

    let map_left = (win_w - state.map_w) / 2.0;
    let map_top = (win_h - state.map_h) / 2.0;
    let click_h = state.map_h * (1.0 - UI_HEIGHT_FRAC);

    if cursor.x < map_left || cursor.x > map_left + state.map_w
        || cursor.y < map_top || cursor.y > map_top + click_h
        {
            return;
        }

        let u_vis = (cursor.x - map_left) / state.map_w;
    let v_vis = (cursor.y - map_top) / state.view_h_ui;

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

        streaming.invalidate_all_with(&mut commands);

        state.is_open = false;
        for mut vis in &mut ui_query { *vis = Visibility::Hidden; }
        window.cursor_options.grab_mode = CursorGrabMode::Locked;
        window.cursor_options.visible = false;
    }
}

fn update_hover_tooltip(
    windows: Query<&Window, With<PrimaryWindow>>,
    ui_scale: Res<bevy::ui::UiScale>,
    state: Res<FastTravelState>,
    data: Res<GameData>,
    interactions: Query<&Interaction, With<Button>>,
    mut tooltip_query: Query<(&mut Node, &mut Visibility), With<HoverTooltip>>,
                        mut text_query: Query<&mut Text, With<HoverTooltipText>>,
) {
    let Ok((mut tooltip_node, mut tooltip_vis)) = tooltip_query.get_single_mut() else { return };
    let Ok(mut tooltip_text) = text_query.get_single_mut() else { return };

    if !state.is_open || state.zoom_level <= 1.01 {
        *tooltip_vis = Visibility::Hidden;
        return;
    }

    let Ok(window) = windows.get_single() else { return };
    let Some(cursor) = window.cursor_position() else { return };

    let scale = ui_scale.0.max(0.01);
    let cursor = cursor / scale;
    let win_w = window.width() / scale;
    let win_h = window.height() / scale;

    let map_left = (win_w - state.map_w) / 2.0;
    let map_top = (win_h - state.map_h) / 2.0;
    let click_h = state.map_h * (1.0 - UI_HEIGHT_FRAC);

    if cursor.x < map_left || cursor.x > map_left + state.map_w
        || cursor.y < map_top || cursor.y > map_top + click_h
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

fn calibrate_world_rect(
    keyboard: Res<ButtonInput<KeyCode>>,
    origin: Res<Origin>,
    mut state: ResMut<FastTravelState>,
) {
    if !state.is_open { return; }
    let ctrl = keyboard.pressed(KeyCode::ControlLeft) || keyboard.pressed(KeyCode::ControlRight);
    if !ctrl { return; }
    let alt = keyboard.pressed(KeyCode::AltLeft) || keyboard.pressed(KeyCode::AltRight);
    let shift = keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight);

    let move_step = if shift { 1.0 } else { 0.25 };
    let scale_step = if shift { 0.01 } else { 0.001 };

    let mut dir = Vec2::ZERO;
    if keyboard.just_pressed(KeyCode::ArrowLeft)  { dir.x -= 1.0; }
    if keyboard.just_pressed(KeyCode::ArrowRight) { dir.x += 1.0; }
    if keyboard.just_pressed(KeyCode::ArrowUp)    { dir.y += 1.0; }
    if keyboard.just_pressed(KeyCode::ArrowDown)  { dir.y -= 1.0; }
    let mut uniform = 0.0;
    if keyboard.just_pressed(KeyCode::PageUp)   { uniform += 1.0; }
    if keyboard.just_pressed(KeyCode::PageDown) { uniform -= 1.0; }

    if dir == Vec2::ZERO && uniform == 0.0 { return; }

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
    mut map_query: Query<&mut ImageNode, (With<MapImageNode>, Without<DotsImageNode>, Without<UiImageNode>)>,
                        mut dots_query: Query<&mut ImageNode, (With<DotsImageNode>, Without<MapImageNode>, Without<UiImageNode>)>,
                        mut ui_query: Query<&mut ImageNode, (With<UiImageNode>, Without<MapImageNode>, Without<DotsImageNode>)>,
) {
    if !state.is_open { return; }

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
    for mut img in &mut ui_query {
        img.rect = ui_rect;
    }
}
