//! Cursore personalizzato per il gioco.
//!
//! Bevy 0.15 non permette di cambiare la texture del cursore di sistema, ma
//! possiamo nasconderlo e disegnare un `ImageNode` UI che segue la posizione
//! del mouse. Lo facciamo solo quando serve un cursore libero (dialogo,
//! equipaggiamento): nel gioco normale il cursore è lockato e invisibile.
//!
//! La texture viene letta da `<textures_dir>/cursor.png` (PNG con alpha).
//! Se il file manca o non si legge, si usa una freccia bianca procedurale.
//!
//! ## Menu egui (F1 tuning, F2 salvataggio)
//!
//! Con un menu egui aperto il cursore custom non serve: bevy_egui disegna la
//! sua finestra *dopo* Bevy UI, quindi coprirebbe il nostro `ImageNode`. In
//! quel caso lasciamo il cursore di sistema visibile (lo gestiscono
//! `tuning_menu::apply_menu_state` e `savegame::apply_save_menu_state`) e
//! nascondiamo il nostro.
//!
//! Il controllo è doppio:
//! - `menu: Option<Res<TuningMenuState>>` — se il plugin del tuning menu non è
//!   caricato, il gioco funziona come prima.
//! - `save_menu: Option<Res<SaveMenuState>>` — se il plugin del salvataggio
//!   non è caricato, stessa cosa.
//!
//! Se uno dei due è `Some` e ha `open == true`, `want_custom` diventa false e
//! non tocchiamo `window.cursor_options.visible` (lo gestisce il menu stesso).

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::PrimaryWindow;

const FALLBACK_PX: u32 = 16;
/// Dimensione di visualizzazione del cursore in pixel logici. Il PNG può essere
/// più grande (32, 48, 64): viene scalato a questa dimensione.
const CURSOR_DISPLAY: f32 = 32.0;
/// Offset dell'hotspot: la punta della freccia in pixel dell'immagine
/// originale. Con un cursore standard (freccia in alto a sinistra), è (0,0).
/// Se il cursore appare spostato rispetto al punto di click, aumenta questi
/// valori (in pixel dell'immagine originale, non in pixel di display).
const HOTSPOT_X: f32 = 0.0;
const HOTSPOT_Y: f32 = 0.0;

#[derive(Component)]
struct CustomCursor {
    /// Dimensioni in pixel dell'immagine originale, per calcolare hotspot e
    /// scala di visualizzazione.
    source_size: Vec2,
}

pub struct CustomCursorPlugin {
    /// Percorso del PNG da usare come cursore. Se `None` o se il file manca,
    /// si usa la freccia procedurale.
    pub cursor_path: Option<PathBuf>,
}

impl Default for CustomCursorPlugin {
    fn default() -> Self {
        Self { cursor_path: None }
    }
}

impl Plugin for CustomCursorPlugin {
    fn build(&self, app: &mut App) {
        // Il setup vuole il percorso: lo mettiamo in una risorsa interna.
        app.insert_resource(CursorSource {
            path: self.cursor_path.clone(),
        })
        .add_systems(Startup, setup_custom_cursor)
        .add_systems(PostUpdate, update_custom_cursor);
    }
}

#[derive(Resource)]
struct CursorSource {
    path: Option<PathBuf>,
}

fn load_cursor_image(path: Option<&PathBuf>) -> Option<Image> {
    let path = path?;
    let reader = image::ImageReader::open(path).ok()?;
    let dyn_img = reader.decode().ok()?.to_rgba8();
    let (w, h) = (dyn_img.width(), dyn_img.height());
    if w == 0 || h == 0 {
        warn!("cursore: immagine vuota, uso la freccia procedurale");
        return None;
    }
    let mut image = Image::new(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        dyn_img.into_raw(),
                               TextureFormat::Rgba8UnormSrgb,
                               RenderAssetUsages::default(),
    );
    image.sampler = bevy::image::ImageSampler::linear();
    info!("cursore: caricato {} ({}x{})", path.display(), w, h);
    Some(image)
}

fn make_fallback_image() -> Image {
    let mut image = Image::new(
        Extent3d {
            width: FALLBACK_PX,
            height: FALLBACK_PX,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        generate_arrow_rgba(),
                               TextureFormat::Rgba8UnormSrgb,
                               RenderAssetUsages::default(),
    );
    image.sampler = bevy::image::ImageSampler::nearest();
    image
}

fn setup_custom_cursor(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    source: Res<CursorSource>,
) {
    // Prova a caricare il PNG, altrimenti fallback procedurale.
    let (image, source_size) = if let Some(img) = load_cursor_image(source.path.as_ref()) {
        let size = img.texture_descriptor.size;
        (img, Vec2::new(size.width as f32, size.height as f32))
    } else {
        if source.path.is_some() {
            warn!(
                "cursore: impossibile leggere {:?}, uso la freccia procedurale",
                source.path
            );
        }
        (
            make_fallback_image(),
         Vec2::splat(FALLBACK_PX as f32),
        )
    };

    let handle = images.add(image);
    commands.spawn((
        Name::new("Custom cursor"),
                    ImageNode::new(handle),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Px(CURSOR_DISPLAY),
                    height: Val::Px(CURSOR_DISPLAY),
                    ..default()
                    },
                    GlobalZIndex(9999),
                    Visibility::Hidden,
                    CustomCursor { source_size },
    ));
}

fn update_custom_cursor(
    dialog: Res<crate::dialog::DialogState>,
    equipment: Res<crate::equipment::EquipmentState>,
    // Opzionale: se il plugin del menu F1 non è caricato, il gioco funziona come prima.
    menu: Option<Res<crate::tuning_menu::TuningMenuState>>,
    // Anche la finestra Salva/Carica (F2) usa egui, quindi ha gli stessi
    // problemi del menu F1: cursore custom coperto, cursore di sistema da
    // lasciare visibile.
    save_menu: Option<Res<crate::savegame::SaveMenuState>>,
    // Un'unica query mutabile sulla finestra: `cursor_position()` ritorna
    // `Option<Vec2>` per valore, quindi il borrow finisce subito e possiamo
    // scrivere `cursor_options.visible` sullo stesso handle senza conflitti.
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut cursor_q: Query<(&CustomCursor, &mut Node, &mut Visibility)>,
) {
    // Il cursore custom non va mostrato quando c'è un menu egui aperto
    // (F1 tuning o F2 salvataggio): bevy_egui disegna la sua finestra *dopo*
    // Bevy UI, quindi coprirebbe il nostro `ImageNode` e resteremmo senza
    // cursore visibile. In quel caso lasciamo il cursore di sistema visibile
    // (gestito da `tuning_menu::apply_menu_state` o `savegame::apply_save_menu_state`).
    let tuning_open = menu.as_ref().is_some_and(|m| m.open);
    let save_open = save_menu.as_ref().is_some_and(|m| m.open);
    let egui_open = tuning_open || save_open;
    let want_custom = (dialog.open || equipment.open) && !egui_open;

    let Ok(mut window) = windows.get_single_mut() else { return };
    let mouse = window.cursor_position();

    for (cursor, mut node, mut vis) in &mut cursor_q {
        if !want_custom || mouse.is_none() {
            *vis = Visibility::Hidden;
            continue;
        }
        let pos = mouse.unwrap();
        // L'hotspot è in pixel dell'immagine originale, ma il cursore è
        // scalato a CURSOR_DISPLAY: convertiamo.
        let scale = CURSOR_DISPLAY / cursor.source_size.x.max(1.0);
        let hx = HOTSPOT_X * scale;
        let hy = HOTSPOT_Y * scale;
        node.left = Val::Px(pos.x - hx);
        node.top = Val::Px(pos.y - hy);
        *vis = Visibility::Visible;
    }

    if want_custom {
        window.cursor_options.visible = false;
    }
    // Se `want_custom` è false NON tocchiamo `visible`: lo gestisce chi di
    // dovere — `player::grab_cursor` nel gioco normale, `tuning_menu` con il
    // menu F1 aperto, `savegame` con la finestra F2 aperta. Questo evita che
    // i sistemi si combattano a vicenda.
}

// =============================================================================
// Fallback procedurale: freccia bianca con bordo nero
// =============================================================================

fn generate_arrow_rgba() -> Vec<u8> {
    const W: usize = FALLBACK_PX as usize;
    const H: usize = FALLBACK_PX as usize;

    let pts: &[(f32, f32)] = &[
        (1.0, 1.0),
        (1.0, 13.0),
        (5.0, 9.0),
        (8.0, 13.0),
        (10.0, 11.0),
        (7.0, 7.0),
        (13.0, 7.0),
        (13.0, 5.0),
        (7.0, 5.0),
        (10.0, 1.0),
    ];

    let inside = |px: f32, py: f32| -> bool {
        let mut hit = false;
        let n = pts.len();
        let mut j = n - 1;
        for i in 0..n {
            let (xi, yi) = pts[i];
            let (xj, yj) = pts[j];
            if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
                hit = !hit;
            }
            j = i;
        }
        hit
    };

    let mut rgba = vec![0u8; W * H * 4];
    for y in 0..H {
        for x in 0..W {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            if !inside(px, py) {
                continue;
            }
            let on_border = [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0)]
            .iter()
            .any(|(dx, dy)| !inside(px + dx, py + dy));
            let color = if on_border {
                [0u8, 0, 0, 255]
            } else {
                [255u8, 255, 255, 255]
            };
            let p = (y * W + x) * 4;
            rgba[p..p + 4].copy_from_slice(&color);
        }
    }
    rgba
}
