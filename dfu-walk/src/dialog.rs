//! Dialogo con gli NPC: avvicinati e premi E per vedere il ritratto dell'NPC su
//! sfondo nero semitrasparente con la scritta gialla. E o Esc chiudono.
//!
//! Il font va messo in `assets/` (vedi `DIALOG_FONT`). In `main.rs` servono:
//!   `mod dialog;`  e  `.add_plugins(dialog::DialogPlugin)`

use crate::interior::InteriorState;
use crate::player::Player;
use crate::tiles::NpcWalker;
use bevy::prelude::*;

/// Percorso del font, relativo alla cartella `assets/` del progetto.
const DIALOG_FONT: &str = "fonts/Sherwood.ttf";
const DIALOG_TEXT: &str = "Hello!";
const DIALOG_FONT_SIZE: f32 = 72.0;
/// Giallo della scritta.
const DIALOG_TEXT_COLOR: Color = Color::srgb(1.0, 0.9, 0.0);
/// Nero dello sfondo con la sua opacità.
const DIALOG_BACKGROUND: Color = Color::srgba(0.0, 0.0, 0.0, 0.75);

/// Distanza massima (m) a cui si può parlare con un NPC. Resta sotto i 2,5 m a cui gli NPC
/// si fermano quando il giocatore sta fermo, così durante il dialogo non se ne vanno.
const DIALOG_DISTANCE: f32 = 2.4;
const PORTRAIT_WIDTH: f32 = 260.0; // larghezza del ritratto sullo schermo (px)
/// Quota di sprite mostrata dall'alto: testa e busto.
const PORTRAIT_CROP: f32 = 0.45;

#[derive(Resource, Default)]
pub struct DialogState {
    pub open: bool,
}

#[derive(Component)]
struct DialogRoot;

#[derive(Component)]
struct DialogPortrait;

pub struct DialogPlugin;

impl Plugin for DialogPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DialogState>()
            .add_systems(Startup, setup_dialog_ui)
            // prima dei sistemi interni, così la pressione di E non apre anche una porta
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
    interior: Res<InteriorState>,
    player: Query<&Transform, With<Player>>,
    npcs: Query<(&NpcWalker, &GlobalTransform)>,
    mut root: Query<&mut Visibility, With<DialogRoot>>,
    mut portrait: Query<(&mut ImageNode, &mut Node), With<DialogPortrait>>,
) {
    // dentro una taverna non si parla con i passanti
    if interior.is_inside() {
        if state.open {
            close_dialog(&mut state, &mut root);
        }
        return;
    }

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

    // NPC più vicino entro la distanza di dialogo.
    let mut nearest: Option<(f32, &NpcWalker)> = None;
    for (npc, gtf) in &npcs {
        let pos = gtf.translation();
        if (pos.y - player_pos.y).abs() > 4.0 {
            continue;
        }
        let distance = Vec2::new(pos.x - player_pos.x, pos.z - player_pos.z).length();
        if distance <= DIALOG_DISTANCE && nearest.map_or(true, |(d, _)| distance < d) {
            nearest = Some((distance, npc));
        }
    }
    let Some((_, npc)) = nearest else { return };

    // Ritratto: parte alta dello sprite frontale, a risoluzione nativa (filtro nearest).
    let (image, size) = npc.portrait();
    if let Ok((mut image_node, mut node)) = portrait.get_single_mut() {
        let crop = Rect::new(0.0, 0.0, size.x, (size.y * PORTRAIT_CROP).max(1.0));
        image_node.image = image;
        image_node.rect = Some(crop);
        node.width = Val::Px(PORTRAIT_WIDTH);
        node.height = Val::Px(PORTRAIT_WIDTH * crop.height() / crop.width().max(1.0));
    }

    state.open = true;
    for mut visibility in root.iter_mut() {
        *visibility = Visibility::Visible;
    }
    // la pressione di E è consumata: non deve aprire anche porte o taverne
    keys.clear_just_pressed(KeyCode::KeyE);
}
