//! Dialogo con gli NPC: avvicinati e premi E per vedere il ritratto dell'NPC su
//! sfondo nero semitrasparente con la scritta gialla. E o Esc chiudono.
//!
//! Funziona sia con gli NPC all'esterno che con quelli all'interno delle taverne/negozi.
//! Il ritratto cerca prima la versione HD nella mod (es. `DREAM - Portraits`),
//! altrimenti ripiega sui volti 64x64 di `TFAC00I0.RCI` (ARENA2) o sullo sprite.

use crate::data::{find_file_ci, Palette};
use crate::interior::{InteriorNpc, InteriorState};
use crate::mod_textures::ModTextures;
use crate::player::Player;
use crate::tiles::NpcWalker;
use crate::world::GameData;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;

const DIALOG_FONT: &str = "fonts/Sherwood.ttf";
const DIALOG_TEXT: &str = "Hello!";
const DIALOG_FONT_SIZE: f32 = 72.0;
const DIALOG_TEXT_COLOR: Color = Color::srgb(1.0, 0.9, 0.0);
const DIALOG_BACKGROUND: Color = Color::srgba(0.0, 0.0, 0.0, 0.75);

const DIALOG_DISTANCE: f32 = 2.4;
const FACE_DISPLAY_SIZE: f32 = 256.0;
const FACES_FILE: &str = "TFAC00I0.RCI";
const FACES_PALETTE: &str = "PAL.PAL";
const FACE_PX: usize = 64;
const FACE_BYTES: usize = FACE_PX * FACE_PX;

const PORTRAIT_WIDTH: f32 = 260.0;
const PORTRAIT_CROP: f32 = 0.45;

#[derive(Resource, Default)]
pub struct DialogState {
    pub open: bool,
}

#[derive(Resource, Default)]
pub struct FacePortraits {
    source: FaceSource,
    cache: HashMap<usize, (Handle<Image>, bool)>,
}

#[derive(Default)]
enum FaceSource {
    #[default]
    NotLoaded,
    Missing,
    Loaded { rci: Vec<u8>, palette: Palette },
}

fn face_rgba(rci: &[u8], record: usize, palette: &Palette) -> Option<Vec<u8>> {
    let start = record.checked_mul(FACE_BYTES)?;
    let pixels = rci.get(start..start.checked_add(FACE_BYTES)?)?;
    let mut rgba = Vec::with_capacity(FACE_BYTES * 4);
    for &index in pixels {
        let c = palette.color(index);
        rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
    }
    Some(rgba)
}

impl FacePortraits {
    fn load(data: &GameData) -> FaceSource {
        let Some(path) = find_file_ci(&data.dir, FACES_FILE) else {
            warn!("{FACES_FILE} non trovato in {}: i dialoghi useranno lo sprite", data.dir.display());
            return FaceSource::Missing;
        };
        let Ok(rci) = std::fs::read(&path) else {
            warn!("impossibile leggere {}: i dialoghi useranno lo sprite", path.display());
            return FaceSource::Missing;
        };
        let palette = find_file_ci(&data.dir, FACES_PALETTE)
        .and_then(|p| Palette::open(p).ok())
        .unwrap_or_else(|| (*data.palette).clone());
        info!("Volti dei dialoghi: {} record da {FACES_FILE}", rci.len() / FACE_BYTES);
        FaceSource::Loaded { rci, palette }
    }

    fn get(
        &mut self,
        record: usize,
        data: &GameData,
        mods: &mut ModTextures,
        images: &mut Assets<Image>,
    ) -> Option<(Handle<Image>, bool)> {
        if let Some(entry) = self.cache.get(&record) {
            return Some(entry.clone());
        }

        // 1. Cerca prima il ritratto in HD nelle mod .dfmod (DREAM - Portraits)
        let candidates = [
            format!("tfac00i0.rci_{record}-0"),
                format!("tfac00i0.rci_{record}"),
                    format!("tfac00i0_{record}-0"),
                        format!("tfac00i0_{record}"),
        ];

        for name in &candidates {
            if let Some(handle) = mods.image_by_name(name, images) {
                let entry = (handle, true);
                self.cache.insert(record, entry.clone());
                return Some(entry);
            }
        }

        // 2. Se non presente nella mod, carica da TFAC00I0.RCI originale
        if matches!(self.source, FaceSource::NotLoaded) {
            self.source = Self::load(data);
        }
        let FaceSource::Loaded { rci, palette } = &self.source else { return None };
        let rgba = face_rgba(rci, record, palette)?;
        let mut image = Image::new(
            Extent3d { width: FACE_PX as u32, height: FACE_PX as u32, depth_or_array_layers: 1 },
            TextureDimension::D2,
            rgba,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        image.sampler = bevy::image::ImageSampler::nearest();
        let handle = images.add(image);
        let entry = (handle, false);
        self.cache.insert(record, entry.clone());
        Some(entry)
    }
}

#[derive(Component)]
pub struct DialogRoot;

#[derive(Component)]
pub(crate) struct DialogPortrait;

pub struct DialogPlugin;

impl Plugin for DialogPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DialogState>()
        .init_resource::<FacePortraits>()
        .add_systems(Startup, setup_dialog_ui)
        .add_systems(
            Update,
            update_dialog
            .before(crate::interior::interior_enter_exit)
            .before(crate::interior::interior_toggle_doors),
        );
    }
}

fn setup_dialog_ui(mut commands: Commands, assets: Res<AssetServer>) {
    let font: Handle<Font> = assets.load(DIALOG_FONT);
    commands
    .spawn((
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            column_gap: Val::Px(56.0),
            ..default()
        },
        BackgroundColor(DIALOG_BACKGROUND),
            GlobalZIndex(100),
            Visibility::Hidden,
            DialogRoot,
    ))
    .with_children(|root| {
        root.spawn((
            Node {
                width: Val::Px(PORTRAIT_WIDTH),
                    height: Val::Px(PORTRAIT_WIDTH),
                    ..default()
            },
            ImageNode::default(),
                    DialogPortrait,
        ));
        root.spawn((
            Text::new(DIALOG_TEXT),
                    TextFont { font, font_size: DIALOG_FONT_SIZE, ..default() },
                    TextColor(DIALOG_TEXT_COLOR),
        ));
    });
}

fn close_dialog(state: &mut DialogState, root: &mut Query<&mut Visibility, With<DialogRoot>>) {
    state.open = false;
    for mut visibility in root.iter_mut() {
        *visibility = Visibility::Hidden;
    }
}

pub fn update_dialog(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut state: ResMut<DialogState>,
    data: Res<GameData>,
    mut mods: ResMut<ModTextures>,
    mut faces: ResMut<FacePortraits>,
    mut images: ResMut<Assets<Image>>,
    interior: Res<InteriorState>,
    player: Query<&Transform, With<Player>>,
    exterior_npcs: Query<(&NpcWalker, &GlobalTransform)>,
                     interior_npcs: Query<(&InteriorNpc, &GlobalTransform)>,
                     mut root: Query<&mut Visibility, With<DialogRoot>>,
                     mut portrait: Query<(&mut ImageNode, &mut Node), With<DialogPortrait>>,
) {
    let pressed_e = keys.just_pressed(KeyCode::KeyE);

    if state.open {
        if pressed_e || keys.just_pressed(KeyCode::Escape) {
            close_dialog(&mut state, &mut root);
            if pressed_e {
                keys.clear_just_pressed(KeyCode::KeyE);
            }
        }
        return;
    }
    if !pressed_e {
        return;
    }

    let Ok(player_tf) = player.get_single() else { return };
    let player_pos = player_tf.translation;

    // Se siamo all'interno, cerca tra gli InteriorNpc (taverne e negozi)
    if interior.is_inside() {
        let mut nearest_int: Option<(f32, &InteriorNpc)> = None;
        for (npc, gtf) in &interior_npcs {
            let pos = gtf.translation();
            if (pos.y - player_pos.y).abs() > 4.0 {
                continue;
            }
            let distance = Vec2::new(pos.x - player_pos.x, pos.z - player_pos.z).length();
            if distance <= DIALOG_DISTANCE && nearest_int.map_or(true, |(d, _)| distance < d) {
                nearest_int = Some((distance, npc));
            }
        }

        if let Some((_, npc)) = nearest_int {
            if let Some((face, is_hd)) = faces.get(npc.face_record, &data, &mut mods, &mut images) {
                if let Ok((mut image_node, mut node)) = portrait.get_single_mut() {
                    image_node.image = face;
                    image_node.rect = None;
                    image_node.flip_y = is_hd;
                    node.width = Val::Px(FACE_DISPLAY_SIZE);
                    node.height = Val::Px(FACE_DISPLAY_SIZE);
                }

                state.open = true;
                for mut visibility in root.iter_mut() {
                    *visibility = Visibility::Visible;
                }
                keys.clear_just_pressed(KeyCode::KeyE);
            }
        }
        return;
    }

    // Se siamo all'esterno, cerca tra gli NpcWalker che camminano
    let mut nearest_ext: Option<(f32, &NpcWalker)> = None;
    for (npc, gtf) in &exterior_npcs {
        let pos = gtf.translation();
        if (pos.y - player_pos.y).abs() > 4.0 {
            continue;
        }
        let distance = Vec2::new(pos.x - player_pos.x, pos.z - player_pos.z).length();
        if distance <= DIALOG_DISTANCE && nearest_ext.map_or(true, |(d, _)| distance < d) {
            nearest_ext = Some((distance, npc));
        }
    }
    let Some((_, npc)) = nearest_ext else { return };

    let (image, crop, width, height, is_hd) = match npc.face_record().and_then(|r| faces.get(r, &data, &mut mods, &mut images)) {
        Some((face, is_hd)) => (face, None, FACE_DISPLAY_SIZE, FACE_DISPLAY_SIZE, is_hd),
        None => {
            let (sprite, size) = npc.portrait();
            let crop = Rect::new(0.0, 0.0, size.x, (size.y * PORTRAIT_CROP).max(1.0));
            (sprite, Some(crop), PORTRAIT_WIDTH, PORTRAIT_WIDTH * crop.height() / crop.width().max(1.0), false)
        }
    };
    if let Ok((mut image_node, mut node)) = portrait.get_single_mut() {
        image_node.image = image;
        image_node.rect = crop;
        image_node.flip_y = is_hd;
        node.width = Val::Px(width);
        node.height = Val::Px(height);
    }

    state.open = true;
    for mut visibility in root.iter_mut() {
        *visibility = Visibility::Visible;
    }
    keys.clear_just_pressed(KeyCode::KeyE);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        let mut bytes = Vec::new();
        for i in 0..256usize {
            bytes.extend_from_slice(&[i as u8, (i + 1) as u8, (i + 2) as u8]);
        }
        Palette::from_bytes("PAL.PAL", &bytes).unwrap()
    }

    #[test]
    fn face_is_converted_with_palette_and_is_opaque() {
        let mut rci = vec![0u8; FACE_BYTES * 3];
        rci[FACE_BYTES * 2] = 7;
        rci[FACE_BYTES * 2 + 1] = 0;
        let rgba = face_rgba(&rci, 2, &palette()).unwrap();
        assert_eq!(rgba.len(), FACE_BYTES * 4);
        assert_eq!(&rgba[0..4], &[7, 8, 9, 255]);
        assert_eq!(&rgba[4..8], &[0, 1, 2, 255]);
        assert!(rgba.chunks(4).all(|p| p[3] == 255));
    }

    #[test]
    fn record_outside_file_gives_none() {
        let rci = vec![0u8; FACE_BYTES * 3];
        assert!(face_rgba(&rci, 2, &palette()).is_some());
        assert!(face_rgba(&rci, 3, &palette()).is_none());
        assert!(face_rgba(&rci, usize::MAX, &palette()).is_none());
        let short = vec![0u8; FACE_BYTES + FACE_BYTES / 2];
        assert!(face_rgba(&short, 1, &palette()).is_none());
    }
}
