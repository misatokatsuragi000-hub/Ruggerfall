//! Bussola HUD di Daggerfall in basso a destra.
//! Include uno sfondo ellittico in pergamena antica con cornice in ottone
//! e 32 tacche radiali per le direzioni, su cui ruotano i 32 frame di `cmpa00i0.bss`.
//!
//! ## Etichetta "N" rossa con snap ai 32 frame, sotto il disco
//!
//! La N è disegnata con il font **Sherwood** (`fonts/Sherwood.ttf`) come
//! componente `Text`, montata su un pivot 0×0 al centro del compasso
//! (`NorthLabelPivot`).
//!
//! L'ago della bussola cambia immagine in 32 posizioni discrete (una per
//! ogni frame `cmpa00i0.bss`). Anche la N usa lo **stesso `frame_idx`**:
//! l'angolo di rotazione è `frame_idx * (TAU / 32)` + offset di calibrazione.
//! Così la N scatta insieme all'ago e resta sempre allineata.
//!
//! **Z-order**: la N è spawnata *prima* del disco nel setup, quindi sta
//! **sotto** l'ago (in Bevy UI l'ultimo figlio spawnato sta sopra). Così
//! l'ago copre la N con il suo disegno, e la N resta visibile solo nelle
//! zone dove il disco è trasparente (o semitrasparente).
//!
//! `NORTH_ANGLE_OFFSET_DEG` compensa un disallineamento del disegno originale
//! del disco. Un valore negativo ruota la N in senso antiorario.

use crate::interior::InteriorState;
use crate::mod_textures::ModTextures;
use crate::player::Player;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::f32::consts::{FRAC_PI_2, TAU};

const COMPASS_WIDTH: f32 = 110.0;
const COMPASS_HEIGHT: f32 = 110.0;
const TOTAL_FRAMES: usize = 32;

// --- Parametri dell'etichetta "N" ---
/// Raggio dell'orbita: distanza in pixel del centro della N dal centro del compasso.
const N_ORBIT_RADIUS: f32 = 38.0;
/// Semi-larghezza del box che contiene la N (24 px di larghezza).
const N_LABEL_HALF_WIDTH: f32 = 12.0;
/// Altezza del box della N.
const N_LABEL_HEIGHT: f32 = 26.0;
/// Dimensione del carattere Sherwood.
const N_FONT_SIZE: f32 = 24.0;
/// Colore rosso scuro della N.
const N_COLOR: Color = Color::srgb(0.78, 0.05, 0.05);
/// Offset angolare in gradi. Negativo = antiorario.
const NORTH_ANGLE_OFFSET_DEG: f32 = -2.0;

#[derive(Component)]
pub struct CompassRoot;

#[derive(Component)]
pub struct CompassUi;

/// Pivot 0×0 al centro del compasso. Ruota col `frame_idx` corrente:
/// la N, sua figlia, orbita e si orienta a scatti con l'ago.
#[derive(Component)]
pub struct NorthLabelPivot;

#[derive(Resource, Default)]
pub struct CompassFrames {
    frames: Vec<Option<(Handle<Image>, bool)>>,
    initialized: bool,
}

pub struct CompassPlugin;

impl Plugin for CompassPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CompassFrames>()
        .add_systems(Startup, setup_compass_ui)
        .add_systems(
            Update,
            (
                update_compass.after(crate::player::player_look),
             update_compass_visibility,
            ),
        );
    }
}

/// Genera la texture ellittica in pergamena con bordo in ottone e 32 tacche.
/// La N **non** è qui: è un `Text` con font Sherwood, montato su un pivot che ruota.
fn generate_parchment_dial_texture() -> Image {
    let width = 792;
    let height = 432;
    let mut rgba = vec![0u8; width * height * 4];

    let cx = width as f32 * 0.5;
    let cy = height as f32 * 0.5;
    let rx = cx - 6.0;
    let ry = cy - 6.0;

    struct TickSegment {
        p_start: Vec2,
        p_end: Vec2,
        thick_sq: f32,
        is_cardinal: bool,
    }

    let mut ticks = Vec::with_capacity(32);
    for k in 0..32 {
        let angle = -FRAC_PI_2 + (k as f32 / 32.0) * TAU;
        let (sin_a, cos_a) = angle.sin_cos();

        let (r_min, r_max, thickness) = if k % 8 == 0 {
            (0.64, 0.86, 3.4)
        } else if k % 4 == 0 {
            (0.72, 0.86, 2.4)
        } else {
            (0.79, 0.86, 1.5)
        };

        let p_start = Vec2::new(cx + rx * r_min * cos_a, cy + ry * r_min * sin_a);
        let p_end = Vec2::new(cx + rx * r_max * cos_a, cy + ry * r_max * sin_a);
        ticks.push(TickSegment {
            p_start,
            p_end,
            thick_sq: (thickness * 0.5) * (thickness * 0.5),
                   is_cardinal: k % 8 == 0,
        });
    }

    for y in 0..height {
        for x in 0..width {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let dx = (px - cx) / rx;
            let dy = (py - cy) / ry;
            let dist_sq = dx * dx + dy * dy;
            let dist = dist_sq.sqrt();

            if dist > 1.01 {
                continue;
            }

            let alpha = if dist > 0.98 {
                ((1.0 - dist) / 0.02).clamp(0.0, 1.0)
            } else {
                1.0
            };

            let mut color = if dist >= 0.905 {
                let t = (dist - 0.905) / 0.08;
                let shine = 1.0 - (t - 0.5).abs() * 1.6;
                [
                    (115.0 + shine * 55.0).clamp(0.0, 255.0),
                    (85.0 + shine * 45.0).clamp(0.0, 255.0),
                    (40.0 + shine * 25.0).clamp(0.0, 255.0),
                ]
            } else if dist >= 0.865 {
                [65.0, 48.0, 26.0]
            } else {
                let v = (dist / 0.865).powi(2);
                let base_r = 215.0 * (1.0 - v * 0.30);
                let base_g = 180.0 * (1.0 - v * 0.35);
                let base_b = 105.0 * (1.0 - v * 0.45);
                let grain = ((x * 37 + y * 61) % 19) as f32 - 9.0;
                [
                    (base_r + grain).clamp(0.0, 255.0),
                    (base_g + grain).clamp(0.0, 255.0),
                    (base_b + grain * 0.6).clamp(0.0, 255.0),
                ]
            };

            if dist >= 0.60 && dist <= 0.88 {
                let p = Vec2::new(px, py);
                let mut min_d_sq = f32::MAX;
                let mut is_cardinal = false;
                let mut tick_thick_sq = 1.0;

                for tick in &ticks {
                    let seg = tick.p_end - tick.p_start;
                    let len_sq = seg.length_squared();
                    let t = ((p - tick.p_start).dot(seg) / len_sq).clamp(0.0, 1.0);
                    let proj = tick.p_start + seg * t;
                    let d_sq = (p - proj).length_squared();
                    if d_sq < tick.thick_sq && d_sq < min_d_sq {
                        min_d_sq = d_sq;
                        is_cardinal = tick.is_cardinal;
                        tick_thick_sq = tick.thick_sq;
                    }
                }

                if min_d_sq < tick_thick_sq {
                    let factor = (1.0 - (min_d_sq / tick_thick_sq).sqrt()).clamp(0.0, 1.0);
                    let tick_col = if is_cardinal {
                        [60.0, 25.0, 15.0]
                    } else {
                        [52.0, 38.0, 20.0]
                    };
                    color[0] = color[0] * (1.0 - factor) + tick_col[0] * factor;
                    color[1] = color[1] * (1.0 - factor) + tick_col[1] * factor;
                    color[2] = color[2] * (1.0 - factor) + tick_col[2] * factor;
                }
            }

            let idx = (y * width + x) * 4;
            rgba[idx] = color[0] as u8;
            rgba[idx + 1] = color[1] as u8;
            rgba[idx + 2] = color[2] as u8;
            rgba[idx + 3] = (alpha * 255.0) as u8;
        }
    }

    Image::new(
        Extent3d {
            width: width as u32,
            height: height as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    )
}

fn setup_compass_ui(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    assets: Res<AssetServer>,
) {
    let parchment_texture = images.add(generate_parchment_dial_texture());
    let sherwood: Handle<Font> = assets.load("fonts/Sherwood.ttf");

    commands
    .spawn((
        Name::new("HUD Compass Container"),
            CompassRoot,
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(8.0),
            bottom: Val::Px(6.0),
            width: Val::Px(COMPASS_WIDTH),
            height: Val::Px(COMPASS_HEIGHT),
            ..default()
            },
            Visibility::Visible,
            GlobalZIndex(2),
    ))
    .with_children(|parent| {
        // 1. Sfondo pergamena ellittico con le tacche (strato inferiore, statico)
        parent.spawn((
            Name::new("Compass Parchment Dial"),
                      ImageNode::new(parchment_texture),
                      Node {
                          position_type: PositionType::Absolute,
                          width: Val::Percent(100.0),
                      height: Val::Percent(100.0),
                      ..default()
                      },
        ));

        // 2. Pivot 0×0 al centro del compasso, con la N come figlia.
        //    Spawnato PRIMA del disco: in Bevy UI, l'ultimo figlio sta sopra.
        //    Così la N sta SOTTO l'ago `cmpa00i0.bss`.
        parent
        .spawn((
            Name::new("Compass North Pivot"),
                NorthLabelPivot,
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(COMPASS_WIDTH * 0.5),
                top: Val::Px(COMPASS_HEIGHT * 0.5),
                width: Val::Px(0.0),
                height: Val::Px(0.0),
                ..default()
                },
                Transform::IDENTITY,
                Visibility::Visible,
        ))
        .with_children(|pivot| {
            // Container della N: centrato sul pivot e spostato in alto
            // a distanza `N_ORBIT_RADIUS`.
            pivot
            .spawn((
                Name::new("Compass North Label Box"),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(-N_LABEL_HALF_WIDTH),
                    top: Val::Px(-N_ORBIT_RADIUS - N_LABEL_HEIGHT * 0.5),
                    width: Val::Px(N_LABEL_HALF_WIDTH * 2.0),
                    height: Val::Px(N_LABEL_HEIGHT),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                    },
            ))
            .with_children(|text_parent| {
                text_parent.spawn((
                    Name::new("Compass North Label"),
                                   Text::new("N"),
                                   TextFont {
                                       font: sherwood.clone(),
                                   font_size: N_FONT_SIZE,
                                   ..default()
                                   },
                                   TextColor(N_COLOR),
                ));
            });
        });

        // 3. Disco rotante originale `cmpa00i0.bss` (frame swapping, non Transform).
        //    Spawnato DOPO la N: sta sopra la N.
        parent.spawn((
            Name::new("Compass Needle/Disc"),
                      CompassUi,
                      ImageNode::default(),
                      Node {
                          position_type: PositionType::Absolute,
                          width: Val::Percent(100.0),
                      height: Val::Percent(100.0),
                      ..default()
                      },
                      Visibility::Visible,
        ));
    });
}

fn update_compass(
    mut compass_cache: ResMut<CompassFrames>,
    mut mods: ResMut<ModTextures>,
    mut images: ResMut<Assets<Image>>,
    player_query: Query<&Player>,
    mut compass_ui: Query<(&mut ImageNode, &mut Visibility), With<CompassUi>>,
                  mut pivot: Query<&mut Transform, With<NorthLabelPivot>>,
) {
    if !compass_cache.initialized {
        compass_cache.frames = Vec::with_capacity(TOTAL_FRAMES);

        for i in 0..TOTAL_FRAMES {
            let candidates = [
                format!("cmpa00i0.bss_0-{i}"),
                    format!("cmpa00i0.bss_0-{i}.png"),
                        format!("cmpa00i0_{i}"),
            ];

            let found = candidates.iter().find_map(|name| {
                mods.image_by_name(name, &mut images).map(|handle| {
                    let flip = mods.needs_v_flip(&handle);
                    (handle, flip)
                })
            });

            compass_cache.frames.push(found);
        }
        compass_cache.initialized = true;
    }

    let Ok(player) = player_query.get_single() else { return };
    let Ok((mut img_node, mut vis)) = compass_ui.get_single_mut() else { return };

    let normalized_yaw = player.yaw.rem_euclid(TAU);
    let frame_idx =
    ((normalized_yaw / TAU) * TOTAL_FRAMES as f32).floor() as usize % TOTAL_FRAMES;

    // --- 1. Cambia il frame del disco (rotazione a scatti). ---
    if let Some(Some((handle, flip_y))) = compass_cache.frames.get(frame_idx) {
        img_node.image = handle.clone();
        img_node.flip_y = *flip_y;
        *vis = Visibility::Visible;
    } else if let Some(Some((handle, flip_y))) = compass_cache.frames.first() {
        img_node.image = handle.clone();
        img_node.flip_y = *flip_y;
        *vis = Visibility::Visible;
    }

    // --- 2. Snap della N allo STESSO frame_idx. ---
    let snap_angle = (frame_idx as f32 / TOTAL_FRAMES as f32) * TAU;
    let offset_rad = NORTH_ANGLE_OFFSET_DEG.to_radians();
    let final_angle = snap_angle + offset_rad;
    for mut tf in &mut pivot {
        tf.rotation = Quat::from_rotation_z(final_angle);
    }
}

/// Nasconde l'intero quadrante quando ci si trova all'interno di taverne o negozi.
fn update_compass_visibility(
    interior: Res<InteriorState>,
    mut roots: Query<&mut Visibility, With<CompassRoot>>,
) {
    let desired_vis = if interior.is_inside() {
        Visibility::Hidden
    } else {
        Visibility::Visible
    };

    for mut vis in &mut roots {
        if *vis != desired_vis {
            *vis = desired_vis;
        }
    }
}
