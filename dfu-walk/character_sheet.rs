//! Scheda del personaggio (tasto `K`): manichino dell'equipaggiamento +
//! statistiche del personaggio + valori vitali correnti.
//!
//! ## Cosa mostra
//!
//! - A sinistra: il **manichino** dell'equipaggiamento (stesso sistema di
//!   `equipment.rs`: prima le texture HD della mod, altrimenti il composito
//!   vanilla `BODY{g}{r}I0.IMG` + `FACE{g}{r}I0.CIF`), con i pezzi equipaggiati
//!   sovrapposti (`Elven Plate Cuirass`, `Steel Greaves`, ecc.). I rect dei
//!   layer vengono NORMALIZZATI sul bounding box della loro unione (vedi
//!   `refresh_sheet_doll`): il manichino riempie esattamente il proprio
//!   contenitore e risulta CENTRATO nel riquadro, senza ereditare i margini
//!   vuoti del canvas HD (che lo facevano apparire spostato a destra).
//!   Sotto, la razza e il numero di volto (`appearance_label`). Nessun titolo.
//! - A destra: nome, classe, livello, poi:
//!   - **Vitali correnti** (da `PlayerVitals`): HP / Magicka / Fatica con
//!     `attuale / massimo` arrotondati (arrotondamento a interi per evitare
//!     rebuild della UI ogni volta che la stamina rigenera).
//!   - **Statistiche derivate** (da `DerivedStats`): bonus danno, bonus
//!     colpire, carico massimo.
//!   - **Armatura equipaggiata** (`PlayerInventory::total_armor`).
//!   - **Attributi** (8 valori interi).
//!   - **Abilità di classe** (1 primaria, 3 maggiori, 6 minori) con il valore
//!     corrente e il tag (★ primaria, ◆ maggiore, ◇ minore).
//!
//! ## Controlli
//!
//! - `K` apre / chiude
//! - `Esc` chiude
//! - Non apre sopra dialog, equipaggiamento o mappa locale
//!
//! ## Layout
//!
//! La colonna sinistra usa `align_items: AlignItems::FlexStart`, così la card
//! del manichino sta al bordo sinistro della colonna. Dentro la card,
//! `align_items: AlignItems::Center` + `justify_content: JustifyContent::Center`
//! centrano il contenitore del manichino orizzontalmente e verticalmente.
//!
//! ADATTAMENTO FINESTRA: l'altezza del manichino NON è fissa a
//! `DOLL_HEIGHT_VH` (usata solo come stima al primo frame): `refresh_sheet_doll`
//! la ricalcola dalla `Window` primaria come
//! `min(altezza_finestra - DOLL_RESERVED_V, (larghezza_finestra - DOLL_RESERVED_H) / aspect_ratio)`,
//! così il manichino riempie lo spazio verticale disponibile senza debordare
//! orizzontalmente sopra la colonna statistiche, anche dopo un resize.
//!
//! ## Perché `pub` su alcune cose di `equipment.rs`
//!
//! `build_doll`, `build_hd_layers`, `BuiltDoll`, `HdLayer`,
//! `layer_rect_percent` e `Doll` sono private in `equipment.rs`. Questo
//! modulo le rende pubbliche (patch sotto) e le usa direttamente, senza
//! duplicare 500 righe di logica di composizione. La `SheetDollCache` di
//! questo modulo è separata da quella di `equipment.rs`: i due pannelli
//! possono essere aperti indipendentemente e ognuno ha la sua cache.
//!
//! ## Cosa NON fa
//!
//! - Non permette di equipaggiare/disequipaggiare (per quello c'è il
//!   pannello `C`). Solo visualizzazione.
//! - Non mostra gli oggetti nello zaino, né le skill "varie" (35-10):
//!   solo le abilità di classe, che sono quelle che contano per salire di
//!   livello.
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::{CursorGrabMode, PrimaryWindow};

use crate::character::{Attribute, Character, ClassCatalog, DerivedStats, Skill, SkillRole};
use crate::dialog::DialogState;
use crate::equipment::{
    appearance_label, build_doll, build_hd_layers, layer_rect_percent, BuiltDoll, HdLayer,
    PlayerAppearance, PlayerInventory, DOLL_HEIGHT, DOLL_WIDTH,
};
use crate::mod_textures::ModTextures;
use crate::vitals::PlayerVitals;
use crate::world::GameData;

// Palette coerente con `equipment.rs` e `journal.rs`.
const OVERLAY: Color = Color::srgba(0.02, 0.015, 0.01, 0.45);
const PANEL: Color = Color::srgba(0.10, 0.075, 0.055, 0.90);
const PANEL_BORDER: Color = Color::srgba(0.75, 0.58, 0.28, 0.50);
const CARD: Color = Color::srgba(0.12, 0.09, 0.065, 0.75);
const ACCENT: Color = Color::srgb(0.96, 0.78, 0.32);
const TEXT: Color = Color::srgb(0.96, 0.93, 0.85);
const MUTED: Color = Color::srgb(0.74, 0.67, 0.54);
const GOOD: Color = Color::srgb(0.55, 0.85, 0.45);

/// Stima iniziale dell'altezza manichino (primo frame), poi sostituita
/// dal calcolo adattivo basato sulla finestra in `refresh_sheet_doll`.
const DOLL_HEIGHT_VH: f32 = 65.0;
/// Spazio verticale riservato: padding pannello (20*2) + border (1*2) +
/// padding card (12*2) + border card (1*2) + row_gap (8) + riga etichetta
/// aspetto (~18) + margine di sicurezza.
const DOLL_RESERVED_V: f32 = 110.0;
/// Spazio orizzontale riservato: colonna statistiche (400) + padding sue
/// (16*2) + border (2) + column_gap (20) + padding pannello (20*2) +
/// padding card sinistra (12*2) + border (2) + margine di sicurezza.
const DOLL_RESERVED_H: f32 = 540.0;
const STATS_PANEL_WIDTH: f32 = 400.0;

// =============================================================================
// Stato e cache
// =============================================================================

#[derive(Resource, Default)]
pub struct CharacterSheetState {
    pub open: bool,
    was_locked: bool,
}

#[derive(Resource, Default)]
struct SheetDollCache {
    key: Option<(PlayerAppearance, Vec<(u32, bool)>)>,
    built: Option<BuiltDoll>,
    layered: Option<(Vec2, Vec<HdLayer>)>,
    /// Rect dei layer NORMALIZZATI sul bbox della loro unione, in percentuale
    /// `(left, top, width, height)`: l'unione riempie esattamente il
    /// contenitore, quindi il manichino risulta centrato nel riquadro.
    norm_rects: Vec<(f32, f32, f32, f32)>,
    /// Dimensione del bbox dell'unione dei layer (aspect ratio reale).
    union_size: Option<Vec2>,
    layers_dirty: bool,
}

// =============================================================================
// Componenti
// =============================================================================

#[derive(Component)]
struct CharacterSheetRoot;

#[derive(Component)]
struct SheetDollImage;

#[derive(Component)]
struct SheetDollLayerNode;

#[derive(Component)]
struct SheetAppearanceText;

#[derive(Component)]
struct SheetStatsText;

#[derive(Component)]
struct SheetSkillsText;

// =============================================================================
// Plugin
// =============================================================================

pub struct CharacterSheetPlugin;

impl Plugin for CharacterSheetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CharacterSheetState>()
        .init_resource::<SheetDollCache>()
        .add_systems(Startup, setup_character_sheet_ui)
        .add_systems(
            Update,
            (
                toggle_character_sheet,
             refresh_sheet_doll,
             update_sheet_text,
            )
            .chain()
            // Blocca l'input del player e della camera quando è aperta.
            .before(crate::player::grab_cursor)
            .before(crate::player::player_look)
            .before(crate::player::player_move)
            .before(crate::equipment::toggle_equipment)
            .before(crate::dialog::update_dialog),
        );
    }
}

// =============================================================================
// Setup UI
// =============================================================================

fn setup_character_sheet_ui(mut commands: Commands) {
    commands
    .spawn((
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(OVERLAY),
            GlobalZIndex(88), // sotto equipment (90) e journal (91)
    Visibility::Hidden,
    CharacterSheetRoot,
    ))
    .with_children(|root| {
        root.spawn((
            Node {
                flex_direction: FlexDirection::Row,
                padding: UiRect::all(Val::Px(20.0)),
                    column_gap: Val::Px(20.0),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
            },
            BackgroundColor(PANEL),
                    BorderColor(PANEL_BORDER),
                    BorderRadius::all(Val::Px(18.0)),
        ))
        .with_children(|panel| {
            // -------------------------------------------------------------
            // Colonna sinistra: card del manichino + aspetto (niente titolo).
            // -------------------------------------------------------------
            panel
            .spawn(Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexStart,
                ..default()
            })
            .with_children(|left| {
                left.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        // CENTRAGGIO: il contenitore del manichino e
                        // l'etichetta aspetto sono centrati
                        // orizzontalmente...
                        align_items: AlignItems::Center,
                        // ...e verticalmente nel riquadro.
                        justify_content: JustifyContent::Center,
                        padding: UiRect::all(Val::Px(12.0)),
                            row_gap: Val::Px(8.0),
                            border: UiRect::all(Val::Px(1.0)),
                            ..default()
                    },
                    BackgroundColor(CARD),
                            BorderColor(PANEL_BORDER),
                            BorderRadius::all(Val::Px(14.0)),
                ))
                .with_children(|card| {
                    // Contenitore manichino: i layer HD sono figli
                    // assoluti con inset percentuali NORMALIZZATI sul
                    // bbox dell'unione, quindi riempiono esattamente
                    // questo nodo (centrato dalla card).
                    card.spawn((
                        Node {
                            position_type: PositionType::Relative,
                            height: Val::Vh(DOLL_HEIGHT_VH),
                                aspect_ratio: Some(DOLL_WIDTH as f32 / DOLL_HEIGHT as f32),
                                ..default()
                        },
                        ImageNode::default(),
                                Visibility::Hidden,
                                SheetDollImage,
                    ));
                    card.spawn((
                        Text::new(appearance_label(PlayerAppearance::default())),
                                TextFont {
                                    font_size: 13.0,
                                    ..default()
                                },
                                TextColor(ACCENT),
                                SheetAppearanceText,
                    ));
                });
            });
            // -------------------------------------------------------------
            // Colonna destra: statistiche + abilità
            // -------------------------------------------------------------
            panel
            .spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    width: Val::Px(STATS_PANEL_WIDTH),
                    padding: UiRect::all(Val::Px(16.0)),
                    row_gap: Val::Px(10.0),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(CARD),
                    BorderColor(PANEL_BORDER),
                    BorderRadius::all(Val::Px(14.0)),
            ))
            .with_children(|card| {
                card.spawn((
                    Text::new("Statistiche"),
                            TextFont {
                                font_size: 16.0,
                                ..default()
                            },
                            TextColor(ACCENT),
                ));
                card.spawn((
                    Text::new(""),
                            TextFont {
                                font_size: 13.0,
                                ..default()
                            },
                            TextColor(TEXT),
                            SheetStatsText,
                ));
                card.spawn((
                    Node {
                        width: Val::Percent(100.0),
                            height: Val::Px(1.0),
                            ..default()
                    },
                    BackgroundColor(PANEL_BORDER),
                ));
                card.spawn((
                    Text::new("Abilità di classe"),
                            TextFont {
                                font_size: 14.0,
                                ..default()
                            },
                            TextColor(ACCENT),
                ));
                card.spawn((
                    Text::new(""),
                            TextFont {
                                font_size: 12.5,
                                ..default()
                            },
                            TextColor(MUTED),
                            SheetSkillsText,
                ));
                card.spawn((
                    Text::new("[K / Esc] Chiudi"),
                            TextFont {
                                font_size: 11.0,
                                ..default()
                            },
                            TextColor(MUTED),
                ));
            });
        });
    });
}

// =============================================================================
// Toggle (K / Esc)
// =============================================================================

fn toggle_character_sheet(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut state: ResMut<CharacterSheetState>,
    dialog: Res<DialogState>,
    equipment: Res<crate::equipment::EquipmentState>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut root: Query<&mut Visibility, With<CharacterSheetRoot>>,
) {
    let toggle = if state.open {
        keys.just_pressed(KeyCode::KeyK) || keys.just_pressed(KeyCode::Escape)
    } else {
        keys.just_pressed(KeyCode::KeyK) && !dialog.open && !equipment.open
    };

    if toggle {
        let open = !state.open;
        state.open = open;
        if let Ok(mut window) = windows.get_single_mut() {
            if open {
                state.was_locked = window.cursor_options.grab_mode == CursorGrabMode::Locked;
                window.cursor_options.grab_mode = CursorGrabMode::None;
                window.cursor_options.visible = true;
            } else if state.was_locked {
                window.cursor_options.grab_mode = CursorGrabMode::Locked;
                window.cursor_options.visible = false;
            }
        }
        for mut vis in &mut root {
            *vis = if open {
                Visibility::Visible
            } else {
                Visibility::Hidden
            };
        }
    }
    if toggle || state.open {
        keys.reset_all();
    }
}

// =============================================================================
// Manichino (stesso flusso di `equipment::refresh_doll`, rect normalizzati)
// =============================================================================

#[allow(clippy::too_many_arguments)]
fn refresh_sheet_doll(
    mut commands: Commands,
    state: Res<CharacterSheetState>,
    look: Res<PlayerAppearance>,
    inventory: Res<PlayerInventory>,
    data: Res<GameData>,
    mut cache: ResMut<SheetDollCache>,
    mut images: ResMut<Assets<Image>>,
    mut mods: ResMut<ModTextures>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut doll: Query<
    (Entity, &mut ImageNode, &mut Node, &mut Visibility),
                      (With<SheetDollImage>, Without<SheetDollLayerNode>),
                      >,
                      layer_nodes: Query<Entity, With<SheetDollLayerNode>>,
) {
    if !state.open {
        return;
    }

    let equipped_key: Vec<(u32, bool)> = inventory.items.iter().map(|i| (i.id, i.equipped)).collect();
    let current_key = Some((*look, equipped_key));
    if cache.key != current_key {
        cache.layered = build_hd_layers(&data, *look, &inventory, &mut mods, &mut images);
        cache.built = if cache.layered.is_some() {
            None
        } else {
            build_doll(&data, *look, &inventory).map(|d| {
                let mut image = Image::new(
                    Extent3d {
                        width: d.width as u32,
                        height: d.height as u32,
                        depth_or_array_layers: 1,
                    },
                    TextureDimension::D2,
                    d.rgba,
                    TextureFormat::Rgba8UnormSrgb,
                    RenderAssetUsages::default(),
                );
                image.sampler = bevy::image::ImageSampler::nearest();
                BuiltDoll {
                    handle: images.add(image),
                                                     size: Vec2::new(d.width as f32, d.height as f32),
                }
            })
        };

        // -----------------------------------------------------------------
        // CENTRAGGIO: normalizzo i rect dei layer sul bounding box della
        // loro unione. I rect originali sono relativi al canvas completo
        // della mod, che include margini vuoti (soprattutto a sinistra):
        // senza normalizzazione il manichino appare spostato a destra nel
        // riquadro. Dopo la normalizzazione l'unione riempie esattamente il
        // contenitore, che la card centra orizzontalmente e verticalmente.
        // -----------------------------------------------------------------
        cache.norm_rects.clear();
        cache.union_size = None;
        if let Some((size, layers)) = &cache.layered {
            if !layers.is_empty() {
                let mut rects_px: Vec<(f32, f32, f32, f32)> = Vec::with_capacity(layers.len());
                let (mut minx, mut miny) = (f32::MAX, f32::MAX);
                let (mut maxx, mut maxy) = (f32::MIN, f32::MIN);
                for layer in layers {
                    let (lp, tp, wp, hp) = layer_rect_percent(layer, *size);
                    let l = lp / 100.0 * size.x;
                    let t = tp / 100.0 * size.y;
                    let w = wp / 100.0 * size.x;
                    let h = hp / 100.0 * size.y;
                    minx = minx.min(l);
                    miny = miny.min(t);
                    maxx = maxx.max(l + w);
                    maxy = maxy.max(t + h);
                    rects_px.push((l, t, w, h));
                }
                let union_w = (maxx - minx).max(1.0);
                let union_h = (maxy - miny).max(1.0);
                cache.norm_rects = rects_px
                .iter()
                .map(|(l, t, w, h)| {
                    (
                        (l - minx) / union_w * 100.0,
                     (t - miny) / union_h * 100.0,
                     w / union_w * 100.0,
                     h / union_h * 100.0,
                    )
                })
                .collect();
                cache.union_size = Some(Vec2::new(union_w, union_h));
            }
        }

        cache.layers_dirty = true;
        cache.key = current_key;
    }

    if cache.layers_dirty {
        for entity in &layer_nodes {
            commands.entity(entity).despawn_recursive();
        }
        let container = doll.iter().next().map(|(entity, _, _, _)| entity);
        if let (Some((_, layers)), Some(container)) = (&cache.layered, container) {
            if !cache.norm_rects.is_empty() {
                commands.entity(container).with_children(|parent| {
                    for (layer, (l, t, w, h)) in layers.iter().zip(cache.norm_rects.iter()) {
                        parent.spawn((
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Percent(*l),
                                      top: Val::Percent(*t),
                                      width: Val::Percent(*w),
                                      height: Val::Percent(*h),
                                      ..default()
                            },
                            ImageNode {
                                image: layer.image.clone(),
                                      flip_y: layer.flip_y,
                                      ..default()
                            },
                            SheetDollLayerNode,
                        ));
                    }
                });
            }
        }
        cache.layers_dirty = false;
    }

    for (_, mut image, mut node, mut visibility) in &mut doll {
        if let Some(union) = &cache.union_size {
            image.color = Color::NONE;
            node.aspect_ratio = Some(union.x / union.y);
            *visibility = Visibility::Inherited;

            // -----------------------------------------------------------------
            // ADATTAMENTO FINESTRA: il manichino prende tutta l'altezza
            // disponibile (finestra - spazio riservato verticale), ma senza
            // eccedere la larghezza disponibile (finestra - colonna
            // statistiche e padding), rispettando l'aspect ratio reale
            // dell'unione. Aggiorna anche a ogni resize.
            // -----------------------------------------------------------------
            if let Ok(window) = windows.get_single() {
                let ratio = node
                .aspect_ratio
                .unwrap_or(DOLL_WIDTH as f32 / DOLL_HEIGHT as f32)
                .max(0.1);
                let max_h = (window.height() - DOLL_RESERVED_V).max(140.0);
                let max_w = (window.width() - DOLL_RESERVED_H).max(140.0);
                let h = max_h.min(max_w / ratio);
                let needs_update = match node.height {
                    Val::Px(prev) => (prev - h).abs() > 0.5,
                    _ => true,
                };
                if needs_update {
                    node.height = Val::Px(h);
                }
            }
        } else if let Some(built) = &cache.built {
            image.color = Color::WHITE;
            image.image = built.handle.clone();
            node.aspect_ratio = Some(built.size.x / built.size.y);
            *visibility = Visibility::Inherited;

            if let Ok(window) = windows.get_single() {
                let ratio = node
                .aspect_ratio
                .unwrap_or(DOLL_WIDTH as f32 / DOLL_HEIGHT as f32)
                .max(0.1);
                let max_h = (window.height() - DOLL_RESERVED_V).max(140.0);
                let max_w = (window.width() - DOLL_RESERVED_H).max(140.0);
                let h = max_h.min(max_w / ratio);
                let needs_update = match node.height {
                    Val::Px(prev) => (prev - h).abs() > 0.5,
                    _ => true,
                };
                if needs_update {
                    node.height = Val::Px(h);
                }
            }
        } else {
            *visibility = Visibility::Hidden;
        }
    }
}

// =============================================================================
// Testi (statistiche + abilità + aspetto)
// =============================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
struct SheetHash(u64);

fn compute_hash(
    ch: &Character,
    derived: &DerivedStats,
    vitals: &PlayerVitals,
    armor: i32,
    look: PlayerAppearance,
) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut mix = |x: u64| {
        h = (h ^ x).wrapping_mul(0x100_0000_01b3);
    };
    mix(ch.level as u64);
    mix(ch.class as u64);
    mix(ch.level_points.to_bits() as u64);
    for a in ch.attrs {
        mix(a as u64);
    }
    for s in ch.skills {
        mix(s as u64);
    }
    mix(derived.max_health as u64);
    mix(derived.max_magicka as u64);
    mix(derived.max_fatigue as u64);
    mix(derived.damage_mod as i64 as u64);
    mix(derived.to_hit_mod as i64 as u64);
    mix(derived.max_encumbrance as i32 as u64);
    mix(armor as i64 as u64);
    // Vitali arrotondati a interi: rigenerazione fluida non triggera rebuild.
    mix(vitals.health.round() as i64 as u64);
    mix(vitals.stamina.round() as i64 as u64);
    mix(vitals.mana.round() as i64 as u64);
    mix(look.race as u64);
    mix(look.female as u64);
    mix(look.face as u64);
    h
}

#[allow(clippy::too_many_arguments)]
fn update_sheet_text(
    state: Res<CharacterSheetState>,
    character: Option<Res<Character>>,
    catalog: Option<Res<ClassCatalog>>,
    derived: Option<Res<DerivedStats>>,
    vitals: Option<Res<PlayerVitals>>,
    inventory: Res<PlayerInventory>,
    look: Res<PlayerAppearance>,
    mut stats_q: Query<
    &mut Text,
    (With<SheetStatsText>, Without<SheetSkillsText>, Without<SheetAppearanceText>),
                     >,
                     mut skills_q: Query<
                     &mut Text,
                     (With<SheetSkillsText>, Without<SheetStatsText>, Without<SheetAppearanceText>),
                     >,
                     mut app_q: Query<
                     &mut Text,
                     (With<SheetAppearanceText>, Without<SheetStatsText>, Without<SheetSkillsText>),
                     >,
                     mut last_hash: Local<Option<SheetHash>>,
                     mut was_open: Local<bool>,
) {
    if !state.open {
        *was_open = false;
        return;
    }

    let (Some(ch), Some(cat), Some(der), Some(vit)) =
    (character.as_deref(), catalog.as_deref(), derived.as_deref(), vitals.as_deref())
    else {
        // Mancano i moduli `CharacterPlugin` / `VitalsPlugin`: mostra un messaggio.
        if let Ok(mut txt) = stats_q.get_single_mut() {
            txt.0 = "Modulo personaggio non caricato.\n\nAssicurati che CharacterPlugin e VitalsPlugin siano registrati in main.rs.".into();
        }
        if let Ok(mut txt) = skills_q.get_single_mut() {
            txt.0 = String::new();
        }
        if let Ok(mut txt) = app_q.get_single_mut() {
            txt.0 = appearance_label(*look);
        }
        *was_open = true;
        return;
    };

    let armor = inventory.total_armor();
    let hash = SheetHash(compute_hash(ch, der, vit, armor, *look));
    let force = !*was_open;
    *was_open = true;
    if !force && Some(hash) == *last_hash {
        return;
    }
    *last_hash = Some(hash);

    // --- Aspetto ---
    if let Ok(mut txt) = app_q.get_single_mut() {
        txt.0 = appearance_label(*look);
    }

    // --- Statistiche ---
    let class = cat.class(ch.class);
    let mut s = String::new();
    s.push_str(&format!(
        "{}\n",
        if ch.name.is_empty() { "Senza nome" } else { &ch.name }
    ));
    s.push_str(&format!("{} — Livello {}\n", class.name, ch.level));
    s.push_str(&format!("Avanzamento: {:.0}%\n", ch.level_progress() * 100.0));
    if ch.pending_attr_points > 0 {
        s.push_str(&format!(
            "Punti attributo da spendere: {}\n",
            ch.pending_attr_points
        ));
    }
    s.push('\n');
    // Vitali
    s.push_str(&format!(
        "Punti ferita:  {:>4} / {}\n",
        vit.health.round() as i32,
                        der.max_health
    ));
    s.push_str(&format!(
        "Punti magia:   {:>4} / {}\n",
        vit.mana.round() as i32,
                        der.max_magicka
    ));
    s.push_str(&format!(
        "Fatica:        {:>4} / {}\n\n",
        vit.stamina.round() as i32,
                        der.max_fatigue
    ));
    // Derivate
    s.push_str(&format!("Bonus danno:   {:+}\n", der.damage_mod));
    s.push_str(&format!("Bonus colpire: {:+}\n", der.to_hit_mod));
    s.push_str(&format!("Carico max:    {:.0} kg\n", der.max_encumbrance));
    s.push_str(&format!("Armatura:      +{}\n", armor));
    s.push_str(&format!(
        "Peso zaino:    {:.1} kg\n\n",
        inventory.total_weight()
    ));
    // Attributi (griglia 4 colonne)
    s.push_str("Attributi\n");
    for pair in Attribute::ALL.chunks(4) {
        let mut line = String::from("  ");
        for a in pair {
            line.push_str(&format!("{:14} {:>3}   ", a.name(), ch.attr(*a)));
        }
        s.push_str(line.trim_end());
        s.push('\n');
    }
    if let Ok(mut txt) = stats_q.get_single_mut() {
        txt.0 = s;
    }

    // --- Abilità di classe ---
    let mut sk = String::new();
    // Raccogli in ordine: primaria, maggiori, minori
    let mut ordered: Vec<(Skill, SkillRole)> = Vec::new();
    for role in [SkillRole::Primary, SkillRole::Major, SkillRole::Minor] {
        for skill in Skill::all() {
            if class.role(skill) == role {
                ordered.push((skill, role));
            }
        }
    }
    if ordered.is_empty() {
        sk.push_str("  (nessuna abilità di classe)\n");
    } else {
        for (skill, role) in ordered {
            let marker = match role {
                SkillRole::Primary => "★",
                SkillRole::Major => "◆",
                SkillRole::Minor => "◇",
                SkillRole::Misc => " ",
            };
            sk.push_str(&format!(
                "  {} {:18} {:>3}{}\n",
                marker,
                skill.name(),
                                 ch.skill(skill),
                                 if ch.skill(skill) >= 100 { " MAX" } else { "" },
            ));
        }
    }
    let _ = GOOD;
    if let Ok(mut txt) = skills_q.get_single_mut() {
        txt.0 = sk;
    }
}
