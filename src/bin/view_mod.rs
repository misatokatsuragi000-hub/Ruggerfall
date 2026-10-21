//! GUI Browser per esplorare le texture dei file .dfmod (Lista a sinistra, anteprima a destra).
//! Uso: cargo run --release --bin view_mod -- <file.dfmod> [filtro]

use std::path::Path;
use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat as BevyFormat};
use bevy::window::PrimaryWindow;
use dfmod::{ModArchive, TextureFormat as DfFormat};

const VISIBLE_ROWS: usize = 22; // Numero di righe visibili contemporaneamente nel pannello sinistro

#[derive(Clone)]
enum AssetItem {
    Tex2D {
        name: String,
        width: u32,
        height: u32,
        format: DfFormat,
        mip_count: u32,
        data: Vec<u8>,
    },
    TexArray {
        name: String,
        width: u32,
        height: u32,
        depth: u32,
        format: DfFormat,
        mip_count: u32,
        data: Vec<u8>,
    },
}

impl AssetItem {
    fn name(&self) -> &str {
        match self {
            Self::Tex2D { name, .. } | Self::TexArray { name, .. } => name,
        }
    }
}

#[derive(Resource)]
struct BrowserState {
    items: Vec<AssetItem>,
    selected_idx: usize,
    scroll_top: usize,
    current_slice: usize,
    flip_v: bool,
    needs_update: bool,
}

#[derive(Component)]
struct ListRowSlot(usize); // 0..VISIBLE_ROWS-1

#[derive(Component)]
struct RowIndexText(usize);

#[derive(Component)]
struct RowNameText(usize);

#[derive(Component)]
struct PreviewImageNode;

#[derive(Component)]
struct InfoHeaderText;

#[derive(Component)]
struct PageIndicatorText;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Uso: cargo run --release --bin view_mod -- <file.dfmod> [filtro]");
        std::process::exit(1);
    }

    let mod_path = Path::new(&args[1]);
    let filter = args.get(2).map(|s| s.to_ascii_lowercase());

    let archive = ModArchive::open(mod_path).unwrap_or_else(|e| {
        eprintln!("Errore apertura mod {}: {e}", mod_path.display());
        std::process::exit(1);
    });

    let mut items = Vec::new();

    for name in archive.texture_names() {
        if let Some(ref f) = filter {
            if !name.to_ascii_lowercase().contains(f) {
                continue;
            }
        }
        if let Ok(Some(tex)) = archive.texture(name) {
            let (format, data) = if tex.format == DfFormat::BC7 {
                (tex.format, tex.data)
            } else {
                match tex.to_rgba8_top_down() {
                    Ok(rgba) => (DfFormat::RGBA32, rgba),
                    Err(_) => (tex.format, tex.data),
                }
            };

            items.push(AssetItem::Tex2D {
                name: name.to_owned(),
                width: tex.width,
                height: tex.height,
                format,
                mip_count: tex.mip_count,
                data,
            });
        }
    }

    for name in archive.texture_array_names() {
        if let Some(ref f) = filter {
            if !name.to_ascii_lowercase().contains(f) {
                continue;
            }
        }
        if let Ok(Some(arr)) = archive.texture_array(name) {
            items.push(AssetItem::TexArray {
                name: name.to_owned(),
                width: arr.width,
                height: arr.height,
                depth: arr.depth,
                format: arr.format,
                mip_count: arr.mip_count,
                data: arr.data,
            });
        }
    }

    items.sort_by(|a, b| a.name().cmp(b.name()));

    if items.is_empty() {
        eprintln!("Nessuna texture trovata che corrisponda al filtro.");
        std::process::exit(0);
    }

    println!("Trovati {} asset. Avvio interfaccia...", items.len());

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "DFMod Texture Browser".into(),
                resolution: (1360.0_f32, 820.0_f32).into(),
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(Color::srgb(0.09, 0.09, 0.11)))
        .insert_resource(BrowserState {
            items,
            selected_idx: 0,
            scroll_top: 0,
            current_slice: 0,
            flip_v: true, // Unity esporta le texture bottom-up
            needs_update: true,
        })
        .add_systems(Startup, setup_gui)
        .add_systems(Update, (handle_input, handle_clicks, update_ui_system))
        .run();
}

fn setup_gui(mut commands: Commands) {
    commands.spawn(Camera2d::default());

    // Root Container a schermo intero
    commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                flex_direction: FlexDirection::Row,
                ..default()
            },
        ))
        .with_children(|root| {
            // ==========================================
            // PANNELLO SINISTRO: LISTA TEXTURE (350px)
            // ==========================================
            root.spawn((
                Node {
                    width: Val::Px(350.0),
                    height: Val::Percent(100.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(10.0)),
                    row_gap: Val::Px(4.0),
                    border: UiRect::right(Val::Px(2.0)),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.12, 0.12, 0.15)),
                BorderColor(Color::srgb(0.2, 0.2, 0.25)),
            ))
            .with_children(|sidebar| {
                // Header della sidebar
                sidebar.spawn((
                    Text::new("TEXTURES EXPLORER"),
                    TextFont { font_size: 16.0, ..default() },
                    TextColor(Color::srgb(0.9, 0.75, 0.3)),
                ));

                sidebar.spawn((
                    Text::new(""),
                    TextFont { font_size: 12.0, ..default() },
                    TextColor(Color::srgb(0.6, 0.6, 0.65)),
                    PageIndicatorText,
                ));

                // Slot delle righe visibili
                for i in 0..VISIBLE_ROWS {
                    sidebar
                        .spawn((
                            Button,
                            ListRowSlot(i),
                            Node {
                                width: Val::Percent(100.0),
                                height: Val::Px(28.0),
                                padding: UiRect::horizontal(Val::Px(8.0)),
                                align_items: AlignItems::Center,
                                justify_content: JustifyContent::SpaceBetween,
                                border: UiRect::all(Val::Px(1.0)),
                                ..default()
                            },
                            BackgroundColor(Color::srgb(0.15, 0.15, 0.19)),
                            BorderColor(Color::NONE),
                        ))
                        .with_children(|row| {
                            row.spawn((
                                Text::new(""),
                                TextFont { font_size: 11.0, ..default() },
                                TextColor(Color::srgb(0.5, 0.5, 0.55)),
                                RowIndexText(i),
                            ));

                            row.spawn((
                                Text::new(""),
                                TextFont { font_size: 13.0, ..default() },
                                TextColor(Color::WHITE),
                                RowNameText(i),
                            ));
                        });
                }
            });

            // ==========================================
            // PANNELLO DESTRO: ANTEPRIMA & SCHEDA DATI
            // ==========================================
            root.spawn((
                Node {
                    flex_grow: 1.0,
                    height: Val::Percent(100.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(16.0)),
                    justify_content: JustifyContent::SpaceBetween,
                    ..default()
                },
                BackgroundColor(Color::srgb(0.08, 0.08, 0.10)),
            ))
            .with_children(|preview_panel| {
                // Barra superiore con metadati dell'asset selezionato
                preview_panel.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        padding: UiRect::all(Val::Px(12.0)),
                        flex_direction: FlexDirection::Column,
                        border: UiRect::all(Val::Px(1.0)),
                        ..default()
                    },
                    BackgroundColor(Color::srgb(0.13, 0.13, 0.17)),
                    BorderColor(Color::srgb(0.22, 0.22, 0.28)),
                )).with_children(|info_box| {
                    info_box.spawn((
                        Text::new("Caricamento..."),
                        TextFont { font_size: 14.0, ..default() },
                        TextColor(Color::srgb(0.95, 0.95, 0.95)),
                        InfoHeaderText,
                    ));
                });

                // Area centrale con l'immagine
                preview_panel.spawn((
                    Node {
                        width: Val::Percent(100.0),
                        height: Val::Percent(78.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                )).with_children(|img_container| {
                    img_container.spawn((
                        ImageNode::default(),
                        Node {
                            max_width: Val::Percent(92.0),
                            max_height: Val::Percent(92.0),
                            align_self: AlignSelf::Center,
                            ..default()
                        },
                        PreviewImageNode,
                    ));
                });

                // Barra inferiore con istruzioni comandi
                preview_panel.spawn((
                    Text::new("Controlli: [Click / Rotella] Sfoglia lista | [Freccia Su/Giù] Seleziona | [V] Capovolgi | [ [ / ] ] Layer array | [Esc] Esci"),
                    TextFont { font_size: 12.0, ..default() },
                    TextColor(Color::srgb(0.55, 0.55, 0.6)),
                ));
            });
        });
}

fn handle_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut mouse_wheel: EventReader<MouseWheel>,
    mut state: ResMut<BrowserState>,
) {
    let len = state.items.len();
    let mut changed = false;

    // Scroll con rotella del mouse
    for ev in mouse_wheel.read() {
        if ev.y > 0.0 {
            if state.scroll_top > 0 {
                state.scroll_top = state.scroll_top.saturating_sub(3);
                changed = true;
            }
        } else if ev.y < 0.0 {
            if state.scroll_top + VISIBLE_ROWS < len {
                state.scroll_top = (state.scroll_top + 3).min(len.saturating_sub(VISIBLE_ROWS));
                changed = true;
            }
        }
    }

    // Navigazione frecce Su/Giù
    if keyboard.just_pressed(KeyCode::ArrowDown) || keyboard.just_pressed(KeyCode::KeyS) {
        if state.selected_idx + 1 < len {
            state.selected_idx += 1;
            if state.selected_idx >= state.scroll_top + VISIBLE_ROWS {
                state.scroll_top = state.selected_idx - VISIBLE_ROWS + 1;
            }
            state.current_slice = 0;
            changed = true;
        }
    }
    if keyboard.just_pressed(KeyCode::ArrowUp) || keyboard.just_pressed(KeyCode::KeyW) {
        if state.selected_idx > 0 {
            state.selected_idx -= 1;
            if state.selected_idx < state.scroll_top {
                state.scroll_top = state.selected_idx;
            }
            state.current_slice = 0;
            changed = true;
        }
    }

    // Navigazione fette di Texture2DArray (tasti [ e ])
    if let AssetItem::TexArray { depth, .. } = state.items[state.selected_idx] {
        if keyboard.just_pressed(KeyCode::BracketRight) || keyboard.just_pressed(KeyCode::Equal) {
            state.current_slice = (state.current_slice + 1) % depth as usize;
            changed = true;
        }
        if keyboard.just_pressed(KeyCode::BracketLeft) || keyboard.just_pressed(KeyCode::Minus) {
            state.current_slice = if state.current_slice == 0 { depth as usize - 1 } else { state.current_slice - 1 };
            changed = true;
        }
    }

    // Tasto V: Capovolgi orientamento verticale
    if keyboard.just_pressed(KeyCode::KeyV) {
        state.flip_v = !state.flip_v;
        changed = true;
    }

    if changed {
        state.needs_update = true;
    }
}

fn handle_clicks(
    mut interaction_query: Query<
        (&Interaction, &ListRowSlot),
        (Changed<Interaction>, With<Button>),
    >,
    mut state: ResMut<BrowserState>,
) {
    for (interaction, slot) in &mut interaction_query {
        if *interaction == Interaction::Pressed {
            let actual_idx = state.scroll_top + slot.0;
            if actual_idx < state.items.len() && actual_idx != state.selected_idx {
                state.selected_idx = actual_idx;
                state.current_slice = 0;
                state.needs_update = true;
            }
        }
    }
}

fn update_ui_system(
    mut state: ResMut<BrowserState>,
    mut images: ResMut<Assets<Image>>,
    mut image_node_query: Query<(&mut ImageNode, &mut Node), With<PreviewImageNode>>,
    mut info_query: Query<&mut Text, (With<InfoHeaderText>, Without<RowIndexText>, Without<RowNameText>, Without<PageIndicatorText>)>,
    mut page_query: Query<&mut Text, (With<PageIndicatorText>, Without<InfoHeaderText>, Without<RowIndexText>, Without<RowNameText>)>,
    mut row_buttons: Query<(&ListRowSlot, &mut BackgroundColor, &mut Visibility), With<Button>>,
    mut idx_texts: Query<(&RowIndexText, &mut Text), (Without<InfoHeaderText>, Without<RowNameText>, Without<PageIndicatorText>)>,
    mut name_texts: Query<(&RowNameText, &mut Text), (Without<InfoHeaderText>, Without<RowIndexText>, Without<PageIndicatorText>)>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if !state.needs_update {
        return;
    }
    state.needs_update = false;

    let len = state.items.len();
    let sel = state.selected_idx;
    let top = state.scroll_top;

    // 1. Aggiorna indicatore di pagina della sidebar
    if let Ok(mut page_txt) = page_query.get_single_mut() {
        **page_txt = format!("Totale: {} texture | Da {} a {}", len, top + 1, (top + VISIBLE_ROWS).min(len));
    }

    // 2. Aggiorna i 22 pulsanti della lista a sinistra
    for (slot, mut bg, mut vis) in &mut row_buttons {
        let item_idx = top + slot.0;
        if item_idx < len {
            *vis = Visibility::Inherited;
            let is_selected = item_idx == sel;
            *bg = if is_selected {
                BackgroundColor(Color::srgb(0.18, 0.38, 0.75)) // Blu selezione
            } else if slot.0 % 2 == 0 {
                BackgroundColor(Color::srgb(0.15, 0.15, 0.19))
            } else {
                BackgroundColor(Color::srgb(0.13, 0.13, 0.16))
            };
        } else {
            *vis = Visibility::Hidden;
        }
    }

    for (slot_idx, mut txt) in &mut idx_texts {
        let item_idx = top + slot_idx.0;
        if item_idx < len {
            **txt = format!("#{:04}", item_idx + 1);
        }
    }

    for (slot_name, mut txt) in &mut name_texts {
        let item_idx = top + slot_name.0;
        if item_idx < len {
            let item = &state.items[item_idx];
            let tag = match item {
                AssetItem::Tex2D { .. } => "",
                AssetItem::TexArray { .. } => " [ARR]",
            };
            **txt = format!("{}{}", item.name(), tag);
        }
    }

    // 3. Estrai e crea la texture selezionata per l'anteprima
    let item = &state.items[sel];
    let (w, h, format, mips, img_data, type_desc) = match item {
        AssetItem::Tex2D { width, height, format, mip_count, data, .. } => {
            (*width, *height, *format, *mip_count, data.clone(), "Texture2D".to_string())
        }
        AssetItem::TexArray { width, height, depth, format, mip_count, data, .. } => {
            let slice_size = data.len() / (*depth as usize);
            let s_idx = state.current_slice.min(*depth as usize - 1);
            let slice_data = data[s_idx * slice_size..(s_idx + 1) * slice_size].to_vec();
            (
                *width,
                *height,
                *format,
                *mip_count,
                slice_data,
                format!("Texture2DArray [Fetta {} / {}]", s_idx + 1, depth),
            )
        }
    };

    let image = if format == DfFormat::BC7 {
        let mut img = Image::new(
            Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            TextureDimension::D2,
            img_data,
            BevyFormat::Bc7RgbaUnormSrgb,
            RenderAssetUsages::default(),
        );
        img.texture_descriptor.mip_level_count = mips;
        img
    } else {
        Image::new(
            Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            TextureDimension::D2,
            img_data,
            BevyFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        )
    };

    let handle = images.add(image);

    // 4. Aggiorna l'anteprima grafica nel pannello destro
    if let Ok((mut img_node, mut node)) = image_node_query.get_single_mut() {
        img_node.image = handle;
        img_node.flip_y = state.flip_v;
        node.aspect_ratio = Some(w as f32 / h as f32);
    }

    // 5. Aggiorna la scheda metadati in alto a destra
    if let Ok(mut info_txt) = info_query.get_single_mut() {
        **info_txt = format!(
            "Nome: {}  |  Tipo: {}\nRisoluzione: {} x {} px  |  Formato: {}  |  Mipmaps: {}",
            item.name(),
            type_desc,
            w,
            h,
            format.name(),
            mips
        );
    }

    if let Ok(mut win) = windows.get_single_mut() {
        win.title = format!("DFMod Browser - [{}/{}] {}", sel + 1, len, item.name());
    }
}
