//! Mappa del mondo fedele di Daggerfall (1000x500 pixel), basata sulle tabelle
//! ufficiali di CLIMATE.PAK, WOODS.WLD e le città/villaggi da MAPS.BSA.
//!
//! ## Marker
//!
//! - **Freccia rossa** (`MapMarker`): posizione del giocatore, ruota con lo
//!   yaw della camera.
//! - **Puntino viola** (`ShipMarker`): posizione della nave, pulsante. Se non
//!   c'è nessuna nave in scena (o è fuori dalla porzione visibile con lo
//!   zoom), il puntino è invisibile.
//!
//! ## Zoom
//!
//! Con la mappa aperta, **+** (anche tastierino) ingrandisce del 20% per
//! pressione e **-** rimpicciolisce del 20%. Lo zoom ritaglia la texture
//! (UV) centrandola sul giocatore; la cornice resta della stessa
//! dimensione e i marker mantengono dimensione costante.

use crate::player::Player;
use crate::ship::Ship;
use crate::world::*;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

const MAP_WIDTH: f32 = 1.30;
const MAP_HEIGHT: f32 = 0.65;

/// Fattore moltiplicativo per ogni pressione di + / - (20%).
const ZOOM_STEP: f32 = 1.2;
const ZOOM_MIN: f32 = 1.0;
const ZOOM_MAX: f32 = 8.0;

const CLIMATE_COLORS: [(i32, [u8; 3]); 10] = [
    (223, [20, 45, 120]),
    (224, [230, 200, 120]),
    (225, [210, 170, 90]),
    (226, [140, 140, 140]),
    (227, [0, 120, 40]),
    (228, [90, 110, 60]),
    (229, [200, 190, 60]),
    (230, [80, 120, 100]),
    (231, [60, 160, 60]),
    (232, [120, 60, 140]),
];

#[derive(Component)]
pub struct WorldMapUi;

#[derive(Component)]
pub struct MapMarker;

/// Marker della nave sulla mappa del mondo.
#[derive(Component)]
pub struct ShipMarker;

/// Quad che mostra la texture della mappa (le UV vengono ritagliate con lo zoom).
#[derive(Component)]
pub struct MapQuad;

#[derive(Resource)]
pub struct MapState {
    pub is_open: bool,
    /// Fattore di zoom (1.0 = mappa intera).
    pub zoom: f32,
}

impl Default for MapState {
    fn default() -> Self {
        Self { is_open: false, zoom: 1.0 }
    }
}

/// Centro (u, v) della porzione di texture visibile: segue `(u, v)` ma
/// resta dentro la mappa.
fn view_center(u: f32, v: f32, zoom: f32) -> (f32, f32) {
    let h = 0.5 / zoom;
    (u.clamp(h, 1.0 - h), v.clamp(h, 1.0 - h))
}

/// Converte (u, v) della mappa in coordinate locali del marker, dato il
/// centro della vista. `None` se il punto è fuori dalla porzione visibile.
fn map_to_local(u: f32, v: f32, center: (f32, f32), zoom: f32) -> Option<(f32, f32)> {
    let du = (u - center.0) * zoom;
    let dv = (v - center.1) * zoom;
    if du.abs() > 0.5 || dv.abs() > 0.5 {
        return None;
    }
    Some((du * MAP_WIDTH, -dv * MAP_HEIGHT))
}

pub fn generate_world_map_texture(data: &GameData) -> Image {
    let w = MAP_WIDTH_PIXELS as usize;
    let h = MAP_HEIGHT_PIXELS as usize;
    let mut pixels = vec![0u8; w * h * 4];

    for y in 0..h {
        for x in 0..w {
            let climate = data.climate.map_pixel_value(x as i32, y as i32).unwrap_or(223) as i32;
            let height = data.woods.height_value(x as i32, y as i32);

            let base_rgb = match CLIMATE_COLORS.iter().find(|(c, _)| *c == climate) {
                Some((_, rgb)) => *rgb,
                None => [20, 45, 120],
            };

            let color: [u8; 4] = if climate == 223 {
                let depth = (height as f32 / 30.0).clamp(0.0, 1.0);
                [
                    (base_rgb[0] as f32 * (0.8 + depth * 0.2)) as u8,
                    (base_rgb[1] as f32 * (0.8 + depth * 0.3)) as u8,
                    (base_rgb[2] as f32 * (0.8 + depth * 0.2)) as u8,
                    255,
                ]
            } else {
                let relief = (height as f32 / 120.0).clamp(0.0, 0.5);
                let shade = 0.85 + relief;
                [
                    ((base_rgb[0] as f32 * shade).min(255.0)) as u8,
                    ((base_rgb[1] as f32 * shade).min(255.0)) as u8,
                    ((base_rgb[2] as f32 * shade).min(255.0)) as u8,
                    255,
                ]
            };

            let idx = (y * w + x) * 4;
            pixels[idx..idx + 4].copy_from_slice(&color);
        }
    }

    // Punti bianchi per tutte le città e villaggi
    for (&(mx, my), loc) in data.locations.iter() {
        if mx < 0 || mx >= w as i32 || my < 0 || my >= h as i32 {
            continue;
        }

        let cx = mx as usize;
        let cy = my as usize;
        let radius = if loc.width >= 4 { 1isize } else { 0isize };

        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let px = cx as isize + dx;
                let py = cy as isize + dy;

                if px >= 0 && px < w as isize && py >= 0 && py < h as isize {
                    let p_idx = (py as usize * w + px as usize) * 4;
                    pixels[p_idx..p_idx + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
    }

    Image::new(
        Extent3d {
            width: w as u32,
            height: h as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

fn create_marker_arrow_mesh() -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    let positions = vec![
        [0.0, 0.024, 0.0],
        [-0.014, -0.016, 0.0],
        [0.0, -0.008, 0.0],
        [0.014, -0.016, 0.0],
    ];
    let normals = vec![[0.0, 0.0, 1.0]; 4];
    let indices = vec![0, 1, 2, 0, 2, 3];

    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

pub fn setup_map_ui(
    mut commands: Commands,
    data: Res<GameData>,
    player_query: Query<Entity, With<Player>>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Ok(player_entity) = player_query.get_single() else { return };

    let map_image = generate_world_map_texture(&data);
    let map_handle = images.add(map_image);

    let map_material = materials.add(StandardMaterial {
        base_color_texture: Some(map_handle),
                                     unlit: true,
                                     ..default()
    });

    let frame_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.08, 0.06, 0.05),
                                       unlit: true,
                                       ..default()
    });

    // Freccia rossa del giocatore.
    let marker_material = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.1, 0.1),
                                        unlit: true,
                                        ..default()
    });

    // Puntino viola della nave.
    let ship_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.6, 0.1, 0.9),
                                      unlit: true,
                                      ..default()
    });

    let map_quad = meshes.add(Rectangle::new(MAP_WIDTH, MAP_HEIGHT));
    let frame_quad = meshes.add(Rectangle::new(MAP_WIDTH + 0.03, MAP_HEIGHT + 0.03));
    let marker_mesh = meshes.add(create_marker_arrow_mesh());
    // Puntino nave: quadratino di ~6 pixel sulla mappa visiva.
    let ship_marker_mesh = meshes.add(Rectangle::new(0.008, 0.008));

    commands.entity(player_entity).with_children(|parent| {
        parent
        .spawn((
            Mesh3d(frame_quad),
                MeshMaterial3d(frame_material),
                Transform::from_xyz(0.0, 0.0, -0.85),
                Visibility::Hidden,
                WorldMapUi,
        ))
        .with_children(|map_parent| {
            map_parent.spawn((
                Mesh3d(map_quad),
                              MeshMaterial3d(map_material),
                              Transform::from_xyz(0.0, 0.0, 0.001),
                              MapQuad,
            ));
            map_parent.spawn((
                Mesh3d(marker_mesh),
                              MeshMaterial3d(marker_material),
                              Transform::from_xyz(0.0, 0.0, 0.003),
                              MapMarker,
            ));
            // Puntino nave, sotto la freccia del player (z = 0.002) così se
            // si sovrappongono la freccia resta visibile sopra.
            map_parent.spawn((
                Mesh3d(ship_marker_mesh),
                              MeshMaterial3d(ship_material),
                              Transform::from_xyz(0.0, 0.0, 0.002).with_scale(Vec3::ZERO),
                              ShipMarker,
            ));
        });
    });

    commands.insert_resource(MapState::default());
}

pub fn toggle_map(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut key_events: EventReader<KeyboardInput>,
    mut map_state: ResMut<MapState>,
    mut map_query: Query<&mut Visibility, With<WorldMapUi>>,
) {
    // Zoom del 20% a pressione. Si leggono i tasti *logici* ("+" / "-"), così
    // funziona con qualsiasi layout di tastiera (italiano compreso).
    // Gli eventi vanno sempre consumati, anche a mappa chiusa.
    for ev in key_events.read() {
        if ev.state != ButtonState::Pressed || !map_state.is_open {
            continue;
        }
        if let Key::Character(c) = &ev.logical_key {
            match c.as_str() {
                "+" => map_state.zoom = (map_state.zoom * ZOOM_STEP).min(ZOOM_MAX),
                "-" => map_state.zoom = (map_state.zoom / ZOOM_STEP).max(ZOOM_MIN),
                _ => {}
            }
        }
    }

    let toggle = keyboard.just_pressed(KeyCode::KeyM)
    || (map_state.is_open && keyboard.just_pressed(KeyCode::Escape));

    if toggle {
        map_state.is_open = !map_state.is_open;
        for mut vis in &mut map_query {
            *vis = if map_state.is_open {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
        }
    }
}

pub fn update_map_marker(
    time: Res<Time>,
    origin: Res<Origin>,
    map_state: Res<MapState>,
    player_query: Query<&Transform, With<Player>>,
    mut marker_query: Query<&mut Transform, (With<MapMarker>, Without<Player>, Without<ShipMarker>)>,
    quad_query: Query<&Mesh3d, With<MapQuad>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if !map_state.is_open {
        return;
    }
    let Ok(player_tf) = player_query.get_single() else { return };
    let Ok(mut marker_tf) = marker_query.get_single_mut() else { return };

    let (mx, my) = map_pixel_of(&origin, player_tf.translation);

    let u = (mx as f32 / MAP_WIDTH_PIXELS as f32).clamp(0.0, 1.0);
    let v = (my as f32 / MAP_HEIGHT_PIXELS as f32).clamp(0.0, 1.0);

    let zoom = map_state.zoom;
    let center = view_center(u, v, zoom);

    // Ritaglia la texture attorno al giocatore (UV ricalcolate dalle posizioni).
    if let Ok(quad) = quad_query.get_single() {
        if let Some(mesh) = meshes.get_mut(&quad.0) {
            let uvs: Option<Vec<[f32; 2]>> = mesh
                .attribute(Mesh::ATTRIBUTE_POSITION)
                .and_then(|a| a.as_float3())
                .map(|ps| {
                    ps.iter()
                        .map(|p| {
                            [
                                center.0 + (p[0] / MAP_WIDTH) / zoom,
                                center.1 - (p[1] / MAP_HEIGHT) / zoom,
                            ]
                        })
                        .collect()
                });
            if let Some(uvs) = uvs {
                mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
            }
        }
    }

    let (x, y) = map_to_local(u, v, center, zoom).unwrap_or((0.0, 0.0));

    let forward = player_tf.forward();
    let angle = forward.x.atan2(-forward.z);
    let pulse = 1.0 + 0.15 * (time.elapsed_secs() * 6.0).sin();

    marker_tf.translation.x = x;
    marker_tf.translation.y = y;
    marker_tf.rotation = Quat::from_rotation_z(-angle);
    marker_tf.scale = Vec3::splat(pulse);
}

/// Aggiorna il puntino rosso della nave sulla mappa del mondo.
///
/// - Se non c'è nessuna nave in scena (`Ship` assente), il puntino viene
///   nascosto (`scale = Vec3::ZERO`).
/// - Altrimenti converte la posizione mondo della nave in map pixel con
///   `map_pixel_of`, poi in coordinate mappa (u, v) in [0, 1] con lo stesso
///   mapping di `update_map_marker`, e posiziona il marker.
/// - La pulsazione usa la stessa frequenza della freccia del giocatore ma è
///   sfasata di 1.5 rad, così i due marker non pulsano in sincrono.
pub fn update_ship_marker(
    time: Res<Time>,
    origin: Res<Origin>,
    map_state: Res<MapState>,
    player_query: Query<&Transform, With<Player>>,
    ship_query: Query<&Transform, With<Ship>>,
    mut marker_query: Query<
    &mut Transform,
    (With<ShipMarker>, Without<Ship>, Without<Player>, Without<MapMarker>),
                          >,
) {
    if !map_state.is_open {
        return;
    }
    let zoom = map_state.zoom;
    let center = match player_query.get_single() {
        Ok(ptf) => {
            let (pmx, pmy) = map_pixel_of(&origin, ptf.translation);
            view_center(
                (pmx as f32 / MAP_WIDTH_PIXELS as f32).clamp(0.0, 1.0),
                (pmy as f32 / MAP_HEIGHT_PIXELS as f32).clamp(0.0, 1.0),
                zoom,
            )
        }
        Err(_) => (0.5, 0.5),
    };
    for mut marker_tf in &mut marker_query {
        let Ok(ship_tf) = ship_query.get_single() else {
            marker_tf.scale = Vec3::ZERO;
            continue;
        };

        let (mx, my) = map_pixel_of(&origin, ship_tf.translation);
        let u = (mx as f32 / MAP_WIDTH_PIXELS as f32).clamp(0.0, 1.0);
        let v = (my as f32 / MAP_HEIGHT_PIXELS as f32).clamp(0.0, 1.0);

        let Some((x, y)) = map_to_local(u, v, center, zoom) else {
            marker_tf.scale = Vec3::ZERO; // fuori dalla porzione visibile
            continue;
        };

        let pulse = 1.0 + 0.15 * (time.elapsed_secs() * 6.0 + 1.5).sin();

        marker_tf.translation.x = x;
        marker_tf.translation.y = y;
        marker_tf.scale = Vec3::splat(pulse);
    }
}
