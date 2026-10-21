//! Mappa del mondo fedele di Daggerfall (1000x500 pixel), basata sulle tabelle
//! ufficiali di CLIMATE.PAK e WOODS.WLD.

use crate::player::Player;
use crate::world::*;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

const MAP_WIDTH: f32 = 1.30;
const MAP_HEIGHT: f32 = 0.65; // Proporzione 2:1 esatta (1000x500 map pixel)

// I colori ufficiali dei 10 climi da dump_world.rs
const CLIMATE_COLORS: [(i32, [u8; 3]); 10] = [
    (223, [20, 45, 120]),   // Oceano (unico specchio d'acqua)
    (224, [230, 200, 120]), // Deserto
    (225, [210, 170, 90]),  // Deserto 2
    (226, [140, 140, 140]), // Montagna
    (227, [0, 120, 40]),    // Foresta pluviale
    (228, [90, 110, 60]),   // Palude
    (229, [200, 190, 60]),  // Sub-tropicale
    (230, [80, 120, 100]),  // Boschi di montagna
    (231, [60, 160, 60]),   // Boschi temperati
    (232, [120, 60, 140]),  // Boschi infestati (Haunted woodlands)
];

#[derive(Component)]
pub struct WorldMapUi;

#[derive(Component)]
pub struct MapMarker;

#[derive(Resource, Default)]
pub struct MapState {
    pub is_open: bool,
}

pub fn climate_name(climate: i32) -> &'static str {
    match climate {
        223 => "Oceano",
        224 => "Deserto",
        225 => "Deserto roccioso",
        226 => "Montagna",
        227 => "Foresta pluviale",
        228 => "Palude",
        229 => "Subtropicale",
        230 => "Boschi di montagna",
        231 => "Boschi temperati",
        232 => "Boschi infestati",
        _ => "Sconosciuto",
    }
}

/// Genera la texture della mappa rispettando fedelmente CLIMATE.PAK e WOODS.WLD.
pub fn generate_world_map_texture(data: &GameData) -> Image {
    let w = MAP_WIDTH_PIXELS as usize; // 1000
    let h = MAP_HEIGHT_PIXELS as usize; // 500
    let mut pixels = Vec::with_capacity(w * h * 4);

    for y in 0..h {
        for x in 0..w {
            let climate = data.climate.map_pixel_value(x as i32, y as i32).unwrap_or(223) as i32;
            let height = data.woods.height_value(x as i32, y as i32);

            let base_rgb = match CLIMATE_COLORS.iter().find(|(c, _)| *c == climate) {
                Some((_, rgb)) => *rgb,
                None => [20, 45, 120], // Default oceano se non valido
            };

            let color: [u8; 4] = if climate == 223 {
                // Oceano: leggero gradiente in base alla profondità senza mai confonderlo con la terra
                let depth = (height as f32 / 30.0).clamp(0.0, 1.0);
                [
                    (base_rgb[0] as f32 * (0.8 + depth * 0.2)) as u8,
                    (base_rgb[1] as f32 * (0.8 + depth * 0.3)) as u8,
                    (base_rgb[2] as f32 * (0.8 + depth * 0.2)) as u8,
                    255,
                ]
            } else {
                // Terraferma: colore del clima modulato dalla quota per evidenziare rilievi e valli
                let relief = (height as f32 / 120.0).clamp(0.0, 0.5);
                let shade = 0.85 + relief;
                [
                    ((base_rgb[0] as f32 * shade).min(255.0)) as u8,
                    ((base_rgb[1] as f32 * shade).min(255.0)) as u8,
                    ((base_rgb[2] as f32 * shade).min(255.0)) as u8,
                    255,
                ]
            };

            pixels.extend_from_slice(&color);
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

/// Freccia indicatore del giocatore (punta in alto).
fn create_marker_arrow_mesh() -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    let positions = vec![
        [0.0, 0.024, 0.0],     // Punta
        [-0.014, -0.016, 0.0], // Sinistra
        [0.0, -0.008, 0.0],    // Rientranza
        [0.014, -0.016, 0.0],  // Destra
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

    // Indicatore del giocatore: rosso vivace per contrastare con qualsiasi clima
    let marker_material = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.1, 0.1),
        unlit: true,
        ..default()
    });

    let map_quad = meshes.add(Rectangle::new(MAP_WIDTH, MAP_HEIGHT));
    let frame_quad = meshes.add(Rectangle::new(MAP_WIDTH + 0.03, MAP_HEIGHT + 0.03));
    let marker_mesh = meshes.add(create_marker_arrow_mesh());

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
                // Mappa a colori
                map_parent.spawn((
                    Mesh3d(map_quad),
                    MeshMaterial3d(map_material),
                    Transform::from_xyz(0.0, 0.0, 0.001),
                ));
                // Freccia giocatore
                map_parent.spawn((
                    Mesh3d(marker_mesh),
                    MeshMaterial3d(marker_material),
                    Transform::from_xyz(0.0, 0.0, 0.003),
                    MapMarker,
                ));
            });
    });

    commands.insert_resource(MapState::default());
}

pub fn toggle_map(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut map_state: ResMut<MapState>,
    mut map_query: Query<&mut Visibility, With<WorldMapUi>>,
) {
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
    mut marker_query: Query<&mut Transform, (With<MapMarker>, Without<Player>)>,
) {
    if !map_state.is_open {
        return;
    }
    let Ok(player_tf) = player_query.get_single() else { return };
    let Ok(mut marker_tf) = marker_query.get_single_mut() else { return };

    let (mx, my) = map_pixel_of(&origin, player_tf.translation);

    let u = (mx as f32 / MAP_WIDTH_PIXELS as f32).clamp(0.0, 1.0);
    let v = (my as f32 / MAP_HEIGHT_PIXELS as f32).clamp(0.0, 1.0);

    let x = (u - 0.5) * MAP_WIDTH;
    let y = (0.5 - v) * MAP_HEIGHT;

    let forward = player_tf.forward();
    let angle = forward.x.atan2(-forward.z);

    let pulse = 1.0 + 0.15 * (time.elapsed_secs() * 6.0).sin();

    marker_tf.translation.x = x;
    marker_tf.translation.y = y;
    marker_tf.rotation = Quat::from_rotation_z(-angle);
    marker_tf.scale = Vec3::splat(pulse);
}
