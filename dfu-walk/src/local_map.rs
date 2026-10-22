//! Mappa locale della città (tasto L): edifici visti dall'alto, con le taverne in verde,
//! i negozi in rosso e le gilde in blu, e la tua posizione. Disegnata in una texture
//! (come la mappa del mondo), quindi non serve `bevy_ui`.

use crate::player::Player;
use crate::world::*;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::city_map::{collect_city_map, image_size, render_city_map, render_notice, MapView, MAP_PX};

/// Altezza del pannello davanti alla camera (la larghezza segue le proporzioni dell'immagine).
const HUD_HEIGHT: f32 = 0.90;
/// Un filo più vicino della mappa del mondo (-0.85), così se sono aperte insieme vince questa.
const HUD_Z: f32 = -0.80;

#[derive(Component)]
pub struct LocalMapUi;

#[derive(Component)]
pub struct LocalMapMarker;

#[derive(Resource)]
pub struct LocalMapState {
    pub is_open: bool,
    /// map pixel per cui e' disegnata la texture attuale
    shown: Option<(i32, i32)>,
    view: Option<MapView>,
    material: Handle<StandardMaterial>,
}

fn hud_width() -> f32 {
    let (w, h) = image_size();
    HUD_HEIGHT * w as f32 / h as f32
}

fn make_image(pixels: Vec<u8>) -> Image {
    let (w, h) = image_size();
    Image::new(
        Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

fn create_marker_arrow_mesh() -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    let positions = vec![[0.0, 0.024, 0.0], [-0.014, -0.016, 0.0], [0.0, -0.008, 0.0], [0.014, -0.016, 0.0]];
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 4]);
    mesh.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));
    mesh
}

pub fn setup_local_map(
    mut commands: Commands,
    player_query: Query<Entity, With<Player>>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Ok(player_entity) = player_query.get_single() else { return };

    let placeholder = images.add(make_image(render_notice("PREMI L IN CITTA")));
    let material = materials.add(StandardMaterial {
        base_color_texture: Some(placeholder),
        unlit: true,
        ..default()
    });
    let frame_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.08, 0.06, 0.05),
        unlit: true,
        ..default()
    });
    let marker_material = materials.add(StandardMaterial {
        base_color: Color::srgb(1.0, 0.95, 0.2),
        unlit: true,
        ..default()
    });

    let hw = hud_width();
    let map_quad = meshes.add(Rectangle::new(hw, HUD_HEIGHT));
    let frame_quad = meshes.add(Rectangle::new(hw + 0.03, HUD_HEIGHT + 0.03));
    let marker_mesh = meshes.add(create_marker_arrow_mesh());

    commands.entity(player_entity).with_children(|parent| {
        parent
            .spawn((
                Mesh3d(frame_quad),
                MeshMaterial3d(frame_material),
                Transform::from_xyz(0.0, 0.0, HUD_Z),
                Visibility::Hidden,
                LocalMapUi,
            ))
            .with_children(|p| {
                p.spawn((
                    Mesh3d(map_quad),
                    MeshMaterial3d(material.clone()),
                    Transform::from_xyz(0.0, 0.0, 0.001),
                ));
                p.spawn((
                    Mesh3d(marker_mesh),
                    MeshMaterial3d(marker_material),
                    Transform::from_xyz(0.0, 0.0, 0.003).with_scale(Vec3::ZERO),
                    LocalMapMarker,
                ));
            });
    });

    commands.insert_resource(LocalMapState { is_open: false, shown: None, view: None, material });
}

pub fn toggle_local_map(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<LocalMapState>,
    mut roots: Query<&mut Visibility, With<LocalMapUi>>,
) {
    let toggle = keyboard.just_pressed(KeyCode::KeyL)
        || (state.is_open && keyboard.just_pressed(KeyCode::Escape));
    if !toggle {
        return;
    }
    state.is_open = !state.is_open;
    if state.is_open {
        state.shown = None; // ridisegna alla prossima update
    }
    for mut vis in &mut roots {
        *vis = if state.is_open { Visibility::Visible } else { Visibility::Hidden };
    }
}

fn build_map_pixels(data: &GameData, mx: i32, my: i32) -> (Vec<u8>, Option<MapView>) {
    let Some(loc) = data.locations.get(&(mx, my)) else {
        return (render_notice("NESSUNA CITTA QUI"), None);
    };
    let (Ok(blocks), Ok(arch)) = (BsaArchive::parse(&data.blocks_buf), BsaArchive::parse(&data.arch3d_buf)) else {
        return (render_notice("ERRORE LETTURA DATI"), None);
    };
    let map = collect_city_map(loc, &blocks, &arch);
    let view = map.view();
    (render_city_map(&map), Some(view))
}

pub fn update_local_map(
    time: Res<Time>,
    origin: Res<Origin>,
    data: Res<GameData>,
    mut state: ResMut<LocalMapState>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    player_query: Query<&Transform, With<Player>>,
    mut marker_query: Query<&mut Transform, (With<LocalMapMarker>, Without<Player>)>,
) {
    if !state.is_open {
        return;
    }
    let Ok(player_tf) = player_query.get_single() else { return };
    let Ok(mut marker_tf) = marker_query.get_single_mut() else { return };

    let (mx, my) = map_pixel_of(&origin, player_tf.translation);

    // (ri)disegna quando si apre o quando si cambia map pixel
    if state.shown != Some((mx, my)) {
        let (pixels, view) = build_map_pixels(&data, mx, my);
        let handle = images.add(make_image(pixels));
        if let Some(mat) = materials.get_mut(&state.material) {
            mat.base_color_texture = Some(handle);
        }
        state.view = view;
        state.shown = Some((mx, my));
    }

    let Some(view) = state.view else {
        marker_tf.scale = Vec3::ZERO;
        return;
    };

    // posizione del giocatore nelle coordinate locali del map pixel (x est, z = -nord)
    let tile = tile_translation(&origin, mx, my);
    let u = (player_tf.translation.x - tile.x - view.x_min) / view.side_m;
    let v = (player_tf.translation.z - tile.z - view.z_min) / view.side_m;
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        marker_tf.scale = Vec3::ZERO;
        return;
    }

    // la mappa occupa la parte alta dell'immagine; sotto c'e' la legenda
    let (_, ih) = image_size();
    let map_frac = MAP_PX as f32 / ih as f32;
    marker_tf.translation.x = (u - 0.5) * hud_width();
    marker_tf.translation.y = (0.5 - v * map_frac) * HUD_HEIGHT;

    let forward = player_tf.forward();
    let angle = forward.x.atan2(-forward.z);
    let pulse = 1.0 + 0.15 * (time.elapsed_secs() * 6.0).sin();
    marker_tf.rotation = Quat::from_rotation_z(-angle);
    marker_tf.scale = Vec3::splat(pulse);
}
