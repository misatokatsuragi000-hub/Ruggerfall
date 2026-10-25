//! GUI Browser per esplorare le texture dei file .dfmod (Lista a sinistra, anteprima a destra).
//! Uso: cargo run --release --bin view_mod -- <file.dfmod> [filtro] [--list]
use std::path::Path;
use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat as BevyFormat};
use bevy::window::PrimaryWindow;
use dfmod::{ModArchive, TextureFormat as DfFormat};

const VISIBLE_ROWS: usize = 22;

#[derive(Clone)]
enum AssetItem {
    Tex2D {
        name: String,
        width: u32,
        height: u32,
        format: DfFormat,
            mip_count: u32,
            data: Vec<u8>,
            is_decoded_rgba8: bool,
    },
}

impl AssetItem {
    fn name(&self) -> &str {
        match self {
            Self::Tex2D { name, .. } => name,
        }
    }
    fn width(&self) -> u32 {
        match self {
            Self::Tex2D { width, .. } => *width,
        }
    }
    fn height(&self) -> u32 {
        match self {
            Self::Tex2D { height, .. } => *height,
        }
    }
    fn mips(&self) -> u32 {
        match self {
            Self::Tex2D { mip_count, .. } => *mip_count,
        }
    }
    fn format_info(&self) -> DfFormat {
        match self {
            Self::Tex2D { format, .. } => *format,
        }
    }
}

#[derive(Resource)]
struct BrowserState {
    items: Vec<AssetItem>,
    selected_idx: usize,
    scroll_top: usize,
    flip_v: bool,
    needs_update: bool,
}

#[derive(Component)]
struct ListRowSlot(usize);
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

// =============================================================================
// Utilità di validazione e mapping formati
// =============================================================================

/// Dimensione in byte di un singolo mip per un formato compresso noto.
fn mip_size_compressed(w: u32, h: u32, format: DfFormat) -> Option<usize> {
    let block_size: usize = match format {
        // BC7 standard (sRGB) e BC7 lineare (Unity ID 108) usano entrambi 16 byte/blocco
        DfFormat::BC7 | DfFormat(108) => 16,
        DfFormat::DXT1 | DfFormat::BC4 => 8,
        DfFormat::DXT5 | DfFormat::BC5 | DfFormat::BC6H => 16,
        _ => return None,
    };
    let bw = ((w + 3) / 4) as usize;
    let bh = ((h + 3) / 4) as usize;
    Some(bw * bh * block_size)
}

fn mip_size_rgba8(w: u32, h: u32) -> usize {
    (w as usize) * (h as usize) * 4
}

/// Mappa DfFormat → BevyFormat per upload diretto GPU.
/// Gestisce esplicitamente il formato 108 (BC7_UNorm lineare) usato da DREAM Textures.
fn bevy_format_direct(format: DfFormat) -> Option<BevyFormat> {
    // Il formato 108 in Unity è BC7_UNorm (lineare). WGPU lo supporta come Bc7RgbaUnorm.
    if format == DfFormat(108) {
        return Some(BevyFormat::Bc7RgbaUnorm);
    }

    match format {
        DfFormat::BC7 => Some(BevyFormat::Bc7RgbaUnormSrgb),
        DfFormat::DXT1 => Some(BevyFormat::Bc1RgbaUnormSrgb),
        DfFormat::DXT5 => Some(BevyFormat::Bc3RgbaUnormSrgb),
        DfFormat::BC4 => Some(BevyFormat::Bc4RUnorm),
        DfFormat::BC5 => Some(BevyFormat::Bc5RgUnorm),
        DfFormat::BC6H => Some(BevyFormat::Bc6hRgbUfloat),
        DfFormat::RGBA32 => Some(BevyFormat::Rgba8UnormSrgb),
        _ => None,
    }
}

/// Costruisce un Image Bevy sicuro da dati RAW (comprimi o RGBA8).
fn safe_image_raw(
    w: u32,
    h: u32,
    format: DfFormat,
        mips: u32,
        mut data: Vec<u8>,
        is_rgba8: bool,
) -> Option<Image> {
    let bevy_fmt = if is_rgba8 {
        BevyFormat::Rgba8UnormSrgb
    } else {
        bevy_format_direct(format)?
    };

    let expected_mip0 = if is_rgba8 {
        mip_size_rgba8(w, h)
    } else {
        mip_size_compressed(w, h, format)?
    };

    if expected_mip0 == 0 || data.is_empty() {
        return None;
    }

    // Padding/truncation base
    if data.len() < expected_mip0 {
        data.resize(expected_mip0, 0);
    }

    let mut final_mips = 1u32;
    if !is_rgba8 && mips > 1 {
        // Calcola quanti mipmap stanno realmente nei dati per formati compressi
        let mut acc = 0usize;
        let (mut mw, mut mh) = (w, h);
        for i in 0..mips {
            let ms = mip_size_compressed(mw, mh, format)?;
            if acc + ms > data.len() { break; }
            acc += ms;
            final_mips = i + 1;
            mw = (mw / 2).max(1);
            mh = (mh / 2).max(1);
        }
        data.truncate(acc);
    } else if is_rgba8 {
        // Per RGBA8 assumiamo 1 mip o tronchiamo ai limiti
        data.truncate(expected_mip0);
    }

    let mut img = Image::new(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        bevy_fmt,
        RenderAssetUsages::default(),
    );
    img.texture_descriptor.mip_level_count = final_mips;
    Some(img)
}

// Helper per calcolare la dimensione totale dei mipmap per formati compressi
fn total_mip_size_compressed(w: u32, h: u32, mips: u32, format: DfFormat) -> Option<usize> {
    let mut total = 0usize;
    let (mut mw, mut mh) = (w, h);
    for _ in 0..mips.max(1) {
        total += mip_size_compressed(mw, mh, format)?;
        mw = (mw / 2).max(1);
        mh = (mh / 2).max(1);
    }
    Some(total)
}

// =============================================================================
// main
// =============================================================================

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // Modalità "solo lista": cargo run --release --bin view_mod -- <file.dfmod> --list
    if args.len() >= 3 && args[2] == "--list" {
        let mod_path = Path::new(&args[1]);
        let archive = match ModArchive::open(mod_path) {
            Ok(a) => a,
            Err(e) => {
                eprintln!("Errore apertura mod {}: {e}", mod_path.display());
                std::process::exit(1);
            }
        };

        println!("\n--- TEXTURE 2D ---");
        for name in archive.texture_names() {
            println!("{}", name);
        }

        println!("\n--- TEXTURE ARRAY (esplose in slice) ---");
        for name in archive.texture_array_names() {
            if let Ok(Some(arr)) = archive.texture_array(name) {
                // Verifica supporto formato
                if bevy_format_direct(arr.format).is_none() {
                    eprintln!("Skipping array '{}': format {:?} not supported", name, arr.format);
                    continue;
                }

                let slice_bytes = total_mip_size_compressed(arr.width, arr.height, arr.mip_count, arr.format).unwrap_or(0);
                if slice_bytes == 0 { continue; }

                for s_idx in 0..arr.depth as usize {
                    println!("{}[{}] (format {:?}, {}x{})", name, s_idx, arr.format, arr.width, arr.height);
                }
            }
        }

        std::process::exit(0);
    }

    // --- Caricamento Normale con GUI ---
    if args.len() < 2 {
        eprintln!("Uso: cargo run --release --bin view_mod -- <file.dfmod> [filtro] [--list]");
        std::process::exit(1);
    }
    let mod_path = Path::new(&args[1]);
    let filter = args.get(2).map(|s| s.to_ascii_lowercase()).filter(|f| f != "--list");

    let archive = ModArchive::open(mod_path).unwrap_or_else(|e| {
        eprintln!("Errore apertura mod {}: {e}", mod_path.display());
        std::process::exit(1);
    });

    let mut items = Vec::new();
    let mut skipped = 0usize;

    // --- Texture2D Singole ---
    for name in archive.texture_names() {
        if let Some(ref f) = filter {
            if !name.to_ascii_lowercase().contains(f) { continue; }
        }

        let tex_res = archive.texture(name);
        let tex = match tex_res {
            Ok(Some(t)) => t,
            Ok(None) => continue,
            Err(e) => {
                eprintln!("Skipping '{}': {e}", name);
                skipped += 1;
                continue;
            }
        };

        let can_gpu_load = bevy_format_direct(tex.format).is_some();

        let (format, data, is_decoded) = if can_gpu_load {
            (tex.format, tex.data, false)
        } else {
            match tex.to_rgba8_top_down() {
                Ok(rgba) => (DfFormat::RGBA32, rgba, true),
                Err(_) => {
                    eprintln!("Skipping '{}': format {:?} unsupported and CPU decode failed", name, tex.format);
                    skipped += 1;
                    continue;
                }
            }
        };

        items.push(AssetItem::Tex2D {
            name: name.to_owned(),
                   width: tex.width,
                   height: tex.height,
                   format,
                       mip_count: tex.mip_count,
                       data,
                       is_decoded_rgba8: is_decoded,
        });
    }

    // --- Texture2DArray: ESPANSE IN SLICE INDIVIDUALI ---
    // Questo è il fix cruciale per vedere tutte le texture del terreno di DREAM.
    for name in archive.texture_array_names() {
        if let Some(ref f) = filter {
            if !name.to_ascii_lowercase().contains(f) { continue; }
        }

        let arr_res = archive.texture_array(name);
        let arr = match arr_res {
            Ok(Some(a)) => a,
            Ok(None) => continue,
            Err(e) => {
                eprintln!("Skipping array '{}': {e}", name);
                skipped += 1;
                continue;
            }
        };

        // Verifica supporto GPU per il formato dell'array
        if bevy_format_direct(arr.format).is_none() {
            eprintln!("Skipping array '{}': format {:?} not natively supported by WGPU", name, arr.format);
            skipped += 1;
            continue;
        }

        // Calcola la dimensione di una singola slice (con tutti i suoi mipmap)
        let slice_size_bytes = total_mip_size_compressed(arr.width, arr.height, arr.mip_count, arr.format).unwrap_or(0);

        if slice_size_bytes == 0 || arr.data.len() < slice_size_bytes * arr.depth as usize {
            eprintln!("Skipping array '{}': data size mismatch or zero slice size", name);
            skipped += 1;
            continue;
        }

        // Estraiamo ogni slice e la aggiungiamo come Tex2D separata
        for s_idx in 0..arr.depth as usize {
            let start = s_idx * slice_size_bytes;
            let end = start + slice_size_bytes;
            let slice_data = arr.data[start..end].to_vec();

            // Nome della slice: es. "302-texarray[0]", "302-texarray[1]"...
            let slice_name = format!("{}[{}]", name, s_idx);

            items.push(AssetItem::Tex2D {
                name: slice_name,
                width: arr.width,
                height: arr.height,
                format: arr.format,
                    mip_count: arr.mip_count,
                    data: slice_data,
                    is_decoded_rgba8: false, // Sono dati BC7/DXT grezzi
            });
        }
    }

    items.sort_by(|a, b| a.name().cmp(b.name()));

    if items.is_empty() {
        eprintln!("Nessuna texture trovata ({} saltate).", skipped);
        std::process::exit(0);
    }

    println!(
        "Trovati {} asset ({} saltati). Avvio interfaccia...",
             items.len(),
             skipped
    );

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
        flip_v: true,
        needs_update: true,
    })
    .add_systems(Startup, setup_gui)
    .add_systems(Update, (handle_input, handle_clicks, update_ui_system))
    .run();
}

// =============================================================================
// GUI
// =============================================================================

fn setup_gui(mut commands: Commands) {
    commands.spawn(Camera2d::default());
    commands
    .spawn(Node {
        width: Val::Percent(100.0),
           height: Val::Percent(100.0),
           flex_direction: FlexDirection::Row,
           ..default()
    })
    .with_children(|root| {
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
            preview_panel
            .spawn((
                Node {
                    width: Val::Percent(100.0),
                    padding: UiRect::all(Val::Px(12.0)),
                    flex_direction: FlexDirection::Column,
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.13, 0.13, 0.17)),
                    BorderColor(Color::srgb(0.22, 0.22, 0.28)),
            ))
            .with_children(|info_box| {
                info_box.spawn((
                    Text::new("Caricamento..."),
                                TextFont { font_size: 14.0, ..default() },
                                TextColor(Color::srgb(0.95, 0.95, 0.95)),
                                InfoHeaderText,
                ));
            });

            preview_panel
            .spawn(Node {
                width: Val::Percent(100.0),
                   height: Val::Percent(78.0),
                   justify_content: JustifyContent::Center,
                   align_items: AlignItems::Center,
                   ..default()
            })
            .with_children(|img_container| {
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

            preview_panel.spawn((
                Text::new(
                    "Controlli: [Click/Rotella] Sfoglia | [Su/Giù] Seleziona | \
[V] Capovolgi | [Esc] Esci",
                ),
                TextFont { font_size: 12.0, ..default() },
                                 TextColor(Color::srgb(0.55, 0.55, 0.6)),
            ));
        });
    });
}

// =============================================================================
// Input
// =============================================================================

fn handle_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut mouse_wheel: EventReader<MouseWheel>,
    mut state: ResMut<BrowserState>,
) {
    let len = state.items.len();
    let mut changed = false;

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

    if keyboard.just_pressed(KeyCode::ArrowDown) || keyboard.just_pressed(KeyCode::KeyS) {
        if state.selected_idx + 1 < len {
            state.selected_idx += 1;
            if state.selected_idx >= state.scroll_top + VISIBLE_ROWS {
                state.scroll_top = state.selected_idx - VISIBLE_ROWS + 1;
            }
            changed = true;
        }
    }
    if keyboard.just_pressed(KeyCode::ArrowUp) || keyboard.just_pressed(KeyCode::KeyW) {
        if state.selected_idx > 0 {
            state.selected_idx -= 1;
            if state.selected_idx < state.scroll_top {
                state.scroll_top = state.selected_idx;
            }
            changed = true;
        }
    }

    if keyboard.just_pressed(KeyCode::KeyV) {
        state.flip_v = !state.flip_v;
        changed = true;
    }
    if keyboard.just_pressed(KeyCode::Escape) {
        std::process::exit(0);
    }
    if changed {
        state.needs_update = true;
    }
}

fn handle_clicks(
    mut interaction_query: Query<(&Interaction, &ListRowSlot), (Changed<Interaction>, With<Button>)>,
                 mut state: ResMut<BrowserState>,
) {
    for (interaction, slot) in &mut interaction_query {
        if *interaction == Interaction::Pressed {
            let actual_idx = state.scroll_top + slot.0;
            if actual_idx < state.items.len() && actual_idx != state.selected_idx {
                state.selected_idx = actual_idx;
                state.needs_update = true;
            }
        }
    }
}

// =============================================================================
// Aggiornamento UI
// =============================================================================

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
    if !state.needs_update { return; }
    state.needs_update = false;

    let len = state.items.len();
    let sel = state.selected_idx;
    let top = state.scroll_top;

    if let Ok(mut txt) = page_query.get_single_mut() {
        **txt = format!("Totale: {} | Da {} a {}", len, top + 1, (top + VISIBLE_ROWS).min(len));
    }

    for (slot, mut bg, mut vis) in &mut row_buttons {
        let item_idx = top + slot.0;
        if item_idx < len {
            *vis = Visibility::Inherited;
            let is_sel = item_idx == sel;
            *bg = if is_sel {
                BackgroundColor(Color::srgb(0.18, 0.38, 0.75))
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
        if item_idx < len { **txt = format!("#{:04}", item_idx + 1); }
    }

    for (slot_name, mut txt) in &mut name_texts {
        let item_idx = top + slot_name.0;
        if item_idx < len {
            let item = &state.items[item_idx];
            **txt = item.name().to_string();
        }
    }

    let item = &state.items[sel];

    // LOGICA DI VISUALIZZAZIONE SICURA
    let image_handle = match item {
        AssetItem::Tex2D { width, height, format, mip_count, data, is_decoded_rgba8, .. } => {
            safe_image_raw(*width, *height, *format, *mip_count, data.clone(), *is_decoded_rgba8)
            .map(|img| images.add(img))
        }
    };

    if let Some(handle) = image_handle {
        if let Ok((mut img_node, mut node)) = image_node_query.get_single_mut() {
            img_node.image = handle;
            img_node.flip_y = state.flip_v;
            node.aspect_ratio = if item.height() > 0 { Some(item.width() as f32 / item.height() as f32) } else { None };
        }
    } else {
        // Placeholder errore
        let placeholder = Image::new(
            Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            TextureDimension::D2,
            vec![255, 0, 0, 255], // Rosso errore
            BevyFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        if let Ok((mut img_node, _)) = image_node_query.get_single_mut() {
            img_node.image = images.add(placeholder);
        }
    }

    if let Ok(mut txt) = info_query.get_single_mut() {
        let fmt_name = format!("{:?}", item.format_info());
        **txt = format!(
            "Nome: {}  |  Tipo: Texture2D\n\
Risoluzione: {} x {}  |  Formato: {}  |  Mipmaps: {}",
item.name(),
                        item.width(),
                        item.height(),
                        fmt_name,
                        item.mips(),
        );
    }

    if let Ok(mut win) = windows.get_single_mut() {
        win.title = format!("DFMod Browser - [{}/{}] {}", sel + 1, len, item.name());
    }
}
