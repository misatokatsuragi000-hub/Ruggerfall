//! Dialogo con gli NPC: avvicinati e premi E per vedere il ritratto dell'NPC su
//! sfondo nero semitrasparente con la scritta gialla. E o Esc chiudono.
//!
//! Funziona sia con gli NPC all'esterno che con quelli all'interno delle taverne/negozi.
//! Il ritratto cerca prima la versione HD nella mod (es. `DREAM - Portraits`),
//! altrimenti ripiega sui volti 64x64 di `TFAC00I0.RCI` (ARENA2) o sullo sprite.
//!
//! ## Testo scrollabile
//!
//! Il testo della conversazione sta in un container con `Overflow::scroll_y()` e una
//! barra di scorrimento verticale. Si scorre con la rotella, con PageUp/PageDown, o
//! con Home/End. Ogni volta che il testo cambia (nuovo messaggio, offerta, risposta
//! del LLM) lo scroll torna in cima.
//!
//! ## Menu e offerte scriptate
//!
//! Il dialogo ha sempre un campo di testo in fondo (click o tasto 2 per scrivere).
//! Se l'NPC è in una gilda e c'è una quest scriptata disponibile (per ora `N0B00Y16`),
//! il dialogo mostra l'offerta originale con due opzioni `[A] Accetta` e `[R] Rifiuta`.
//!
//! ## Knowledge block (da dfquest)
//!
//! Quando il dialogo si apre su un PNG, viene calcolato un `knowledge_block` con le
//! informazioni che quel PNG "ha sentito in giro" sulle quest principali attive.
//! Il blocco viene iniettato nel prompt LLM, così il PNG risponde coerentemente.
//!
//! ## Reclutamento follower via LLM
//!
//! Il giocatore può convincere un NPC a seguirlo tramite la conversazione libera.
//! L'LLM decide se l'NPC è stato persuaso a sufficienza e risponde con
//! `{"text": "...", "follow": true}`. Il gioco intercetta il flag e converte
//! l'NPC in un `Follower` che segue il giocatore.
use crate::data::city_map::PoiKind;
use crate::data::flats::FlatCatalog;
use crate::data::{find_file_ci, Palette};
use crate::dfquest::{knowledge_block, KnowledgeOptions, Library, QuestStates};
use crate::interior::{InteriorNpc, InteriorState};
use crate::mod_textures::ModTextures;
use crate::player::Player;
use crate::quest_llm::QuestWorker;
use crate::quests::{
    clean_question, describe_npc, face_key, flat_key, temperament_for, tone_for_npc, walker_key,
    walker_look, Hint, QuestBook, Tone, VoiceState, QUESTION_MAX_CHARS,
};
use crate::tiles::{Follower, NpcWalker};
use crate::tts::{gender_from_caption, gender_from_look, Gender, SpeakEvent, Speaker};
use crate::world::{map_pixel_of, GameData, Origin};
use crate::{QuestLibRes, QuestStatesRes};

use bevy::ecs::system::SystemParam;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::MouseWheel;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui::{ComputedNode, ScrollPosition};
use std::collections::HashMap;
use bevy::ecs::query::{QueryEntityError, QueryFilter};

const DIALOG_FONT: &str = "fonts/Sherwood.ttf";
const DIALOG_TEXT: &str = "Hello!";
const DIALOG_FONT_SIZE: f32 = 44.0;
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

// Menu e campo di testo
const DIALOG_TEXT_MAX_WIDTH: f32 = 760.0;
const DIALOG_TEXT_MIN_HEIGHT: f32 = 280.0;
const DIALOG_TEXT_MAX_HEIGHT: f32 = 360.0;
const MENU_TEXT: &str = "[1] Ascolta rumori      [E] Congeda";
const OFFER_MENU: &str = "[A] Accetta      [R] Rifiuta";
const MENU_FONT_SIZE: f32 = 28.0;
const MENU_COLOR: Color = Color::srgb(0.85, 0.8, 0.55);
const RUMOR_FONT_SIZE: f32 = 28.0;
const INPUT_PLACEHOLDER: &str = "Clicca qui per parlare...";
const INPUT_FONT_SIZE: f32 = 26.0;
const INPUT_COLOR: Color = Color::srgb(0.95, 0.95, 0.85);
const INPUT_BG: Color = Color::srgba(0.05, 0.05, 0.08, 0.7);
const INPUT_BORDER: Color = Color::srgb(0.6, 0.5, 0.2);
const INPUT_MIN_HEIGHT: f32 = 44.0;
const RUMOR_FONT: &str = "fonts/Sherwood.ttf";

// Scroll
const WHEEL_STEP: f32 = 40.0;
const SCROLLBAR_WIDTH: f32 = 8.0;
const SCROLLBAR_MIN_HANDLE: f32 = 30.0;

const SCRIPTED_QUEST_ID: &str = "N0B00Y16";

// =============================================================================
// Evento di reclutamento
// =============================================================================

/// Evento emesso quando l'LLM decide che l'NPC segue il giocatore.
#[derive(Event)]
pub struct RecruitFollowerEvent {
    pub entity: Entity,
}

// =============================================================================
// Risorse e tipi
// =============================================================================

/// Bundle `SystemParam` per raggruppare le due risorse `dfquest` (con wrapper):
/// riduce il numero di parametri di `update_dialog` sotto il limite di 16 di Bevy.
#[derive(SystemParam)]
pub struct QuestResources<'w> {
    pub lib: Res<'w, QuestLibRes>,
    pub states: Res<'w, QuestStatesRes>,
}

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
        let candidates = [
            format!("tfac00i0.rci_{record}-0"),
                format!("tfac00i0.rci_{record}"),
                    format!("tfac00i0_{record}-0"),
                        format!("tfac00i0_{record}"),
        ];
        for name in &candidates {
            if let Some(handle) = mods.image_by_name(name, images) {
                let flip = mods.needs_v_flip(&handle);
                let entry = (handle, flip);
                self.cache.insert(record, entry.clone());
                return Some(entry);
            }
        }
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

// =============================================================================
// Componenti UI
// =============================================================================

#[derive(Component)]
pub struct DialogRoot;

#[derive(Component)]
struct DialogLine;

#[derive(Component)]
struct DialogOptions;

#[derive(Component)]
struct DialogInput;

#[derive(Component)]
struct DialogInputText;

/// Container scrollabile che ospita `DialogLine`.
#[derive(Component)]
struct DialogScroll;

/// Maniglia visiva della barra di scorrimento.
#[derive(Component)]
struct DialogScrollBar;

#[derive(Component)]
pub(crate) struct DialogPortrait;

// =============================================================================
// Offerte scriptate
// =============================================================================

#[derive(Clone)]
pub struct ScriptedOffer {
    pub quest_id: String,
    pub _quest_name: String,
    pub offer_text: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ScriptedQuestState {
    #[default]
    None,
    Offering,
}

// =============================================================================
// Contesto del dialogo
// =============================================================================

#[derive(Resource, Default)]
pub struct DialogContext {
    tone: Tone,
    city: Option<(i32, i32)>,
    npc_flat: Option<(u16, u8)>,
    npc_caption: Option<String>,
    dirty: bool,
    voice: Option<NpcVoiceInfo>,
    awaiting: bool,
    city_name: String,
    typing: bool,
    buffer: String,
    asking: Option<u64>,
    conversation: Vec<(String, String)>,
    knowledge: String,
    pub scripted_offer: Option<ScriptedOffer>,
    pub scripted_state: ScriptedQuestState,
    scripted_response: Option<String>,
    /// Entità dell'NPC con cui si sta parlando (per il reclutamento).
    npc_entity: Option<Entity>,
}

#[derive(Clone, Default)]
struct NpcVoiceInfo {
    key: String,
    label: String,
    description: String,
    gender: Option<Gender>,
    role: Option<Tone>,
    temperament: &'static str,
}

impl DialogContext {
    fn with_voice(mut self, key: String, label: String, description: String) -> Self {
        self.voice = Some(NpcVoiceInfo { key, label, description, ..Default::default() });
        self
    }

    fn with_city_name(mut self, name: String) -> Self {
        self.city_name = name;
        self
    }

    fn with_speaker(mut self, gender: Option<Gender>, role: Option<Tone>, temperament: &'static str) -> Self {
        if let Some(voice) = self.voice.as_mut() {
            voice.gender = gender;
            voice.role = role;
            voice.temperament = temperament;
        }
        self
    }

    fn with_knowledge(mut self, knowledge: String) -> Self {
        self.knowledge = knowledge;
        self
    }

    fn new(
        npc_key: u64,
        city: (i32, i32),
           place: Option<PoiKind>,
           npc_flat: Option<(u16, u8)>,
           npc_caption: Option<String>,
    ) -> Self {
        Self {
            tone: tone_for_npc(npc_key, city, place),
            city: Some(city),
            npc_flat,
            npc_caption,
            dirty: true,
            voice: None,
            awaiting: false,
            city_name: String::new(),
            typing: false,
            buffer: String::new(),
            asking: None,
            conversation: Vec::new(),
            knowledge: String::new(),
            scripted_offer: None,
            scripted_state: ScriptedQuestState::None,
            scripted_response: None,
            npc_entity: None,
        }
    }

    fn prompt_history(&self) -> Vec<(String, String)> {
        let mut h = Vec::with_capacity(self.conversation.len() + 1);
        if !self.knowledge.is_empty() {
            h.push(("system".to_string(), self.knowledge.clone()));
        }
        h.extend(self.conversation.iter().cloned());
        h
    }
}

// =============================================================================
// Utility testo
// =============================================================================

fn ascii_punctuation(text: &str) -> String {
    text.chars()
    .map(|c| match c {
        '\u{2019}' | '\u{2018}' | '`' => "'".to_string(),
         '\u{201C}' | '\u{201D}' | '\u{00AB}' | '\u{00BB}' => "\"".to_string(),
         '\u{2026}' => "...".to_string(),
         '\u{2013}' | '\u{2014}' => "-".to_string(),
         c => c.to_string(),
    })
    .collect()
}

fn flavor_message(book: &QuestBook, ctx: &DialogContext) -> Option<String> {
    let (city, (a, r)) = (ctx.city?, ctx.npc_flat?);
    let text = book.npc_flavor(city, a, r)?;
    let label = ctx.npc_caption.clone().unwrap_or_else(|| "NPC".to_string());
    Some(format!("{}: \"{}\"", label, ascii_punctuation(text)))
}

fn spoken_part(message: &str) -> Option<String> {
    let start = message.find(": \"")? + 3;
    let end = message.rfind('"')?;
    (end > start).then(|| message[start..end].to_string())
}

enum Reply {
    Say(String),
    Wait(String),
}

fn waiting_text(voice: &NpcVoiceInfo) -> String {
    format!("{} is thinking about it...", voice.label)
}

fn npc_reply(book: &mut QuestBook, worker: Option<&QuestWorker>, ctx: &DialogContext) -> Reply {
    if let Some(message) = flavor_message(book, ctx) {
        return Reply::Say(message);
    }
    if let (Some(city), Some(voice)) = (ctx.city, ctx.voice.as_ref()) {
        enum Known {
            Ready(String),
            Pending,
            Unknown,
            Failed,
        }
        let known = match book.voice_state(city, &voice.key) {
            VoiceState::Ready(t) => Known::Ready(t.to_string()),
            VoiceState::Pending => Known::Pending,
            VoiceState::Unknown => Known::Unknown,
            VoiceState::Failed => Known::Failed,
        };
        match known {
            Known::Ready(text) => {
                return Reply::Say(format!("{}: \"{}\"", voice.label, ascii_punctuation(&text)));
            }
            Known::Pending => return Reply::Wait(waiting_text(voice)),
            Known::Unknown => {
                if let Some(worker) = worker {
                    if worker.request_voice(book, city, &voice.key, &voice.description) {
                        return Reply::Wait(waiting_text(voice));
                    }
                }
            }
            Known::Failed => {}
        }
    }
    Reply::Say(rumor_message(book, ctx))
}

fn rumor_message(book: &QuestBook, ctx: &DialogContext) -> String {
    if let Some(message) = flavor_message(book, ctx) {
        return message;
    }
    if ctx.npc_flat.is_some() {
        return "They shrug. They don't seem to know anything about it.".to_string();
    }
    hint_message(book, ctx.city, ctx.tone)
}

fn hint_message(book: &QuestBook, city: Option<(i32, i32)>, tone: Tone) -> String {
    match book.hint(city, tone) {
        Hint::Text(t) => format!("{}: \"{}\"", tone.label(), ascii_punctuation(t)),
        Hint::Pending => "Something is being whispered in town, but nobody can tell you anything specific yet. Try again later.".to_string(),
        Hint::Nothing => "You don't hear anything interesting.".to_string(),
    }
}

/// Formatta la risposta LLM per la visualizzazione.
/// Il risultato è `(testo, follow)`: il flag follow viene ignorato qui
/// (è gestito separatamente in `dialog_menu`).
fn answer_message(label: &str, result: &Result<(String, bool), ()>) -> String {
    match result {
        Ok((text, _)) => format!("{}: \"{}\"", label, ascii_punctuation(text)),
        Err(()) => format!("{label} says nothing."),
    }
}

// =============================================================================
// Plugin
// =============================================================================

pub struct DialogPlugin;

impl Plugin for DialogPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DialogState>()
        .init_resource::<FacePortraits>()
        .init_resource::<DialogContext>()
        .add_event::<RecruitFollowerEvent>()
        .add_systems(Startup, setup_dialog_ui)
        .add_systems(
            PreUpdate,
            (update_dialog, dialog_menu, reset_keys_when_dialog_open)
            .chain()
            .after(bevy::input::InputSystem),
        )
        .add_systems(
            Update,
            (
                dialog_input_click,
             dialog_scroll_wheel,
             dialog_scroll_keys,
             dialog_scroll_reset_on_text_change,
             dialog_scroll_bar_update,
             apply_recruit,
            ),
        );
    }
}

// =============================================================================
// Setup UI
// =============================================================================

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
        root.spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(14.0),
                   width: Val::Px(DIALOG_TEXT_MAX_WIDTH),
                   max_width: Val::Px(DIALOG_TEXT_MAX_WIDTH),
                   ..default()
        })
        .with_children(|column| {
            // --- Container scrollabile con il testo della conversazione ---
            column
            .spawn((
                Node {
                    width: Val::Percent(100.0),
                    min_height: Val::Px(DIALOG_TEXT_MIN_HEIGHT),
                    max_height: Val::Px(DIALOG_TEXT_MAX_HEIGHT),
                    overflow: Overflow::scroll_y(),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::new(
                        Val::Px(0.0),
                                         Val::Px(SCROLLBAR_WIDTH + 10.0),
                                         Val::Px(0.0),
                                         Val::Px(0.0),
                    ),
                    ..default()
                },
                ScrollPosition::default(),
                    DialogScroll,
            ))
            .with_children(|scroll| {
                scroll.spawn((
                    Text::new(DIALOG_TEXT),
                              TextFont { font: font.clone(), font_size: DIALOG_FONT_SIZE, ..default() },
                              TextColor(DIALOG_TEXT_COLOR),
                              TextLayout::default(),
                              DialogLine,
                ));
                scroll.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        right: Val::Px(0.0),
                              top: Val::Px(0.0),
                              width: Val::Px(SCROLLBAR_WIDTH),
                              height: Val::Px(0.0),
                              ..default()
                    },
                    BackgroundColor(Color::NONE),
                              DialogScrollBar,
                ));
            });
            // --- Menu e input restano fissi sotto ---
            column.spawn((
                Text::new(MENU_TEXT),
                          TextFont { font: font.clone(), font_size: MENU_FONT_SIZE, ..default() },
                          TextColor(MENU_COLOR),
                          DialogOptions,
            ));
            column.spawn((
                Button,
                DialogInput,
                Node {
                    width: Val::Percent(100.0),
                          min_height: Val::Px(INPUT_MIN_HEIGHT),
                          padding: UiRect::all(Val::Px(8.0)),
                          border: UiRect::all(Val::Px(2.0)),
                          justify_content: JustifyContent::FlexStart,
                          align_items: AlignItems::Center,
                          flex_wrap: FlexWrap::Wrap,
                          ..default()
                },
                BackgroundColor(INPUT_BG),
                          BorderColor(INPUT_BORDER),
                          BorderRadius::all(Val::Px(4.0)),
            ))
            .with_children(|parent| {
                parent.spawn((
                    Text::new(INPUT_PLACEHOLDER),
                              TextFont { font, font_size: INPUT_FONT_SIZE, ..default() },
                              TextColor(INPUT_COLOR),
                              DialogInputText,
                ));
            });
        });
    });
}

// =============================================================================
// Chiusura dialogo
// =============================================================================

fn close_dialog(state: &mut DialogState, root: &mut Query<&mut Visibility, With<DialogRoot>>) {
    state.open = false;
    for mut visibility in root.iter_mut() {
        *visibility = Visibility::Hidden;
    }
}

// =============================================================================
// Scroll
// =============================================================================

fn dialog_scroll_wheel(
    state: Res<DialogState>,
    mut wheel: EventReader<MouseWheel>,
    mut scroll: Query<&mut ScrollPosition, With<DialogScroll>>,
) {
    if !state.open {
        wheel.clear();
        return;
    }
    let delta: f32 = wheel.read().map(|e| e.y).sum();
    if delta == 0.0 {
        return;
    }
    for mut sp in &mut scroll {
        sp.offset_y = (sp.offset_y - delta * WHEEL_STEP).max(0.0);
    }
}

fn dialog_scroll_keys(
    state: Res<DialogState>,
    keys: Res<ButtonInput<KeyCode>>,
    mut scroll: Query<&mut ScrollPosition, With<DialogScroll>>,
) {
    if !state.open {
        return;
    }
    let mut delta = 0.0f32;
    if keys.just_pressed(KeyCode::PageDown) {
        delta = -DIALOG_TEXT_MAX_HEIGHT * 0.9;
    } else if keys.just_pressed(KeyCode::PageUp) {
        delta = DIALOG_TEXT_MAX_HEIGHT * 0.9;
    } else if keys.just_pressed(KeyCode::Home) {
        for mut sp in &mut scroll {
            sp.offset_y = 0.0;
        }
        return;
    } else if keys.just_pressed(KeyCode::End) {
        for mut sp in &mut scroll {
            sp.offset_y = f32::MAX;
        }
        return;
    }
    if delta == 0.0 {
        return;
    }
    for mut sp in &mut scroll {
        sp.offset_y = (sp.offset_y + delta).max(0.0);
    }
}

fn dialog_scroll_reset_on_text_change(
    mut scroll: Query<&mut ScrollPosition, With<DialogScroll>>,
    line_changed: Query<&Text, (Changed<Text>, With<DialogLine>)>,
) {
    if line_changed.is_empty() {
        return;
    }
    for mut sp in &mut scroll {
        sp.offset_y = 0.0;
    }
}

fn dialog_scroll_bar_update(
    scroll: Query<(&ScrollPosition, &ComputedNode), With<DialogScroll>>,
                            content: Query<&ComputedNode, (With<DialogLine>, Without<DialogScroll>, Without<DialogScrollBar>)>,
                            mut bar: Query<(&mut Node, &mut BackgroundColor), With<DialogScrollBar>>,
) {
    let Ok((sp, view_node)) = scroll.get_single() else { return };
    let Ok(content_node) = content.get_single() else { return };
    let Ok((mut bar_node, mut bar_bg)) = bar.get_single_mut() else { return };
    let view_h = view_node.size().y;
    let content_h = content_node.size().y;
    if content_h <= view_h + 1.0 || view_h <= 0.0 {
        bar_node.height = Val::Px(0.0);
        *bar_bg = BackgroundColor(Color::NONE);
        return;
    }
    let ratio = view_h / content_h;
    let handle_h = (view_h * ratio).max(SCROLLBAR_MIN_HANDLE);
    let max_offset = (content_h - view_h).max(1.0);
    let scroll_ratio = (sp.offset_y / max_offset).clamp(0.0, 1.0);
    let top = (view_h - handle_h) * scroll_ratio;
    bar_node.height = Val::Px(handle_h);
    bar_node.top = Val::Px(top);
    *bar_bg = BackgroundColor(Color::srgba(0.85, 0.8, 0.55, 0.7));
}

// =============================================================================
// Offerte scriptate
// =============================================================================

#[allow(dead_code)]
fn player_race_label(race: u8) -> &'static str {
    match race % 8 {
        0 => "Breton",
        1 => "Redguard",
        2 => "Nord",
        3 => "Dark Elf",
        4 => "High Elf",
        5 => "Wood Elf",
        6 => "Khajiit",
        7 => "Argonian",
        _ => "Breton",
    }
}

fn find_quest_offer(
    lib: &Library,
    states: &QuestStates,
    interior: &InteriorState,
    _data: &GameData,
    _player_pos: Vec3,
    ctx: &DialogContext,
) -> Option<ScriptedOffer> {
    if ctx.scripted_state != ScriptedQuestState::None {
        return None;
    }
    let place = interior.kind()?;
    if !matches!(place, PoiKind::Guild) {
        return None;
    }
    let script = lib.get(SCRIPTED_QUEST_ID)?;
    let offer_msg = script.message_named("QuestorOffer")?;
    let variants = offer_msg.plain_variants();
    let first = variants.first()?;
    let offer_text = lib.expand(SCRIPTED_QUEST_ID, states, first);
    Some(ScriptedOffer {
        quest_id: script.id.clone(),
         _quest_name: script.display_name.clone(),
         offer_text,
    })
}

// =============================================================================
// Apertura dialogo (E)
// =============================================================================

#[allow(clippy::too_many_arguments)]
pub fn update_dialog(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut state: ResMut<DialogState>,
    data: Res<GameData>,
    mut mods: ResMut<ModTextures>,
    mut faces: ResMut<FacePortraits>,
    mut images: ResMut<Assets<Image>>,
    interior: Res<InteriorState>,
    origin: Res<Origin>,
    quests: QuestResources,
    mut ctx: ResMut<DialogContext>,
    player: Query<&Transform, With<Player>>,
    exterior_npcs: Query<(Entity, &NpcWalker, &GlobalTransform)>,
                     interior_npcs: Query<(Entity, &InteriorNpc, &GlobalTransform)>,
                     mut root: Query<&mut Visibility, With<DialogRoot>>,
                     mut portrait: Query<(&mut ImageNode, &mut Node), With<DialogPortrait>>,
                     mut flats_catalog: Local<Option<FlatCatalog>>,
) {
    let pressed_e = keys.just_pressed(KeyCode::KeyE);
    if state.open {
        if ctx.typing {
            return;
        }
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

    // --- NPC interni ---
    if interior.is_inside() {
        let mut nearest_int: Option<(f32, Entity, &InteriorNpc)> = None;
        for (entity, npc, gtf) in &interior_npcs {
            let pos = gtf.translation();
            if (pos.y - player_pos.y).abs() > 4.0 {
                continue;
            }
            let distance = Vec2::new(pos.x - player_pos.x, pos.z - player_pos.z).length();
            if distance <= DIALOG_DISTANCE && nearest_int.map_or(true, |(d, _, _)| distance < d) {
                nearest_int = Some((distance, entity, npc));
            }
        }
        if let Some((_, npc_entity, npc)) = nearest_int {
            if let Some((face, is_hd)) = faces.get(npc.face_record, &data, &mut mods, &mut images) {
                let npc_flat = Some((npc.flat_archive, npc.flat_record));
                let catalog = flats_catalog.get_or_insert_with(|| FlatCatalog::load(&data.dir));
                let flat_info = catalog.get(npc.flat_archive, npc.flat_record);
                let npc_caption = flat_info.map(|i| i.caption.clone());
                let city = map_pixel_of(&origin, player_pos);
                let city_name = data.locations.get(&city).map(|l| l.name.clone()).unwrap_or_default();
                let place = interior.kind();
                let tone = tone_for_npc(npc.face_record as u64, city, place);
                let (key, who, role) = match &npc_caption {
                    Some(caption) => (flat_key(npc.flat_archive, npc.flat_record), caption.clone(), None),
                    None => (face_key(npc.face_record), "an ordinary townsperson".to_string(), Some(tone.role())),
                };
                let temperament = temperament_for(&key, city);
                let description = describe_npc(&who, role, place, temperament);
                let gender = flat_info
                .and_then(|i| match i.gender {
                    1 => Some(Gender::Male),
                          2 => Some(Gender::Female),
                          _ => None,
                })
                .or_else(|| npc_caption.as_deref().and_then(gender_from_caption));
                let knowledge = knowledge_block(
                    &quests.lib.0,
                    &quests.states.0,
                    "",
                    &key,
                    &KnowledgeOptions::default(),
                );
                *ctx = DialogContext::new(npc.face_record as u64, city, place, npc_flat, npc_caption)
                .with_voice(key, who, description)
                .with_speaker(gender, role.map(|_| tone), temperament)
                .with_city_name(city_name)
                .with_knowledge(knowledge);
                ctx.npc_entity = Some(npc_entity);
                if let Some(offer) = find_quest_offer(&quests.lib.0, &quests.states.0, &*interior, &*data, player_pos, &ctx) {
                    ctx.scripted_offer = Some(offer);
                    ctx.scripted_state = ScriptedQuestState::Offering;
                }
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

    // --- NPC esterni ---
    let mut nearest_ext: Option<(f32, Entity, &NpcWalker)> = None;
    for (entity, npc, gtf) in &exterior_npcs {
        let pos = gtf.translation();
        if (pos.y - player_pos.y).abs() > 4.0 {
            continue;
        }
        let distance = Vec2::new(pos.x - player_pos.x, pos.z - player_pos.z).length();
        if distance <= DIALOG_DISTANCE && nearest_ext.map_or(true, |(d, _, _)| distance < d) {
            nearest_ext = Some((distance, entity, npc));
        }
    }
    let Some((_, npc_entity, npc)) = nearest_ext else { return };
    let city = map_pixel_of(&origin, player_pos);
    let city_name = data.locations.get(&city).map(|l| l.name.clone()).unwrap_or_default();
    let face = npc.face_record().unwrap_or(0);
    let tone = tone_for_npc(face as u64, city, None);
    let look = walker_look(face);
    let key = walker_key(face);
    let temperament = temperament_for(&key, city);
    let description = describe_npc(look, Some(tone.role()), None, temperament);
    let knowledge = knowledge_block(
        &quests.lib.0,
        &quests.states.0,
        "",
        &key,
        &KnowledgeOptions::default(),
    );
    *ctx = DialogContext::new(face as u64, city, None, None, None)
    .with_voice(key, format!("{look} ({})", tone.role()), description)
    .with_speaker(gender_from_look(look), Some(tone), temperament)
    .with_city_name(city_name)
    .with_knowledge(knowledge);
    ctx.npc_entity = Some(npc_entity);

    let (image, crop, width, height, is_hd) =
    match npc.face_record().and_then(|r| faces.get(r, &data, &mut mods, &mut images)) {
        Some((face, is_hd)) => (face, None, FACE_DISPLAY_SIZE, FACE_DISPLAY_SIZE, is_hd),
        None => {
            let (sprite, size) = npc.portrait();
            let flip = mods.needs_v_flip(&sprite);
            let crop_h = (size.y * PORTRAIT_CROP).max(1.0);
            let crop = if flip {
                Rect::new(0.0, size.y - crop_h, size.x, size.y)
            } else {
                Rect::new(0.0, 0.0, size.x, crop_h)
            };
            (sprite, Some(crop), PORTRAIT_WIDTH, PORTRAIT_WIDTH * crop.height() / crop.width().max(1.0), flip)
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

fn reset_keys_when_dialog_open(state: Res<DialogState>, mut keys: ResMut<ButtonInput<KeyCode>>) {
    if state.open {
        keys.reset_all();
    }
}

// =============================================================================
// Input testo
// =============================================================================

fn push_typed(buffer: &mut String, c: char) {
    if !c.is_control() && buffer.chars().count() < QUESTION_MAX_CHARS {
        buffer.push(c);
    }
}

fn dialog_input_click(
    state: Res<DialogState>,
    mut ctx: ResMut<DialogContext>,
    interactions: Query<&Interaction, (Changed<Interaction>, With<DialogInput>)>,
) {
    if !state.open || ctx.typing {
        return;
    }
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
            ctx.typing = true;
            ctx.buffer.clear();
        }
    }
}

fn set_input_text<F: QueryFilter>(texts: &mut Query<&mut Text, F>, s: String) {
    for mut text in texts.iter_mut() {
        text.0 = s.clone();
    }
}

// =============================================================================
// Offerta scriptata (A/R)
// =============================================================================

fn handle_scripted_offer(
    keys: &ButtonInput<KeyCode>,
    ctx: &mut DialogContext,
    lib: &Library,
    states: &QuestStates,
    assets: &AssetServer,
    line: &mut Query<(&mut Text, &mut TextFont), (With<DialogLine>, Without<DialogOptions>, Without<DialogInputText>)>,
                         options: &mut Query<&mut Text, (With<DialogOptions>, Without<DialogLine>, Without<DialogInputText>)>,
) -> bool {
    if ctx.scripted_state != ScriptedQuestState::Offering {
        return false;
    }
    let Some(offer) = ctx.scripted_offer.clone() else {
        ctx.scripted_state = ScriptedQuestState::None;
        return false;
    };
    for (mut text, mut font) in line.iter_mut() {
        text.0 = offer.offer_text.clone();
        font.font = assets.load(RUMOR_FONT);
        font.font_size = RUMOR_FONT_SIZE;
    }
    for mut text in options.iter_mut() {
        text.0 = OFFER_MENU.to_string();
    }
    let response = if keys.just_pressed(KeyCode::KeyA) {
        Some(true)
    } else if keys.just_pressed(KeyCode::KeyR) {
        Some(false)
    } else {
        None
    };
    if let Some(accepted) = response {
        let section = if accepted { "AcceptQuest" } else { "RefuseQuest" };
        let fallback = if accepted { "You accept." } else { "You decline." };
        let raw = lib
        .get(&offer.quest_id)
        .and_then(|s| s.message_named(section))
        .and_then(|m| m.plain_variants().into_iter().next())
        .unwrap_or_else(|| fallback.to_string());
        let resolved = lib.expand(&offer.quest_id, states, &raw);
        ctx.scripted_state = ScriptedQuestState::None;
        ctx.scripted_response = Some(resolved);
    }
    true
}

// =============================================================================
// Menu dialogo principale
// =============================================================================

#[allow(clippy::too_many_arguments)]
fn dialog_menu(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    state: Res<DialogState>,
    mut ctx: ResMut<DialogContext>,
    mut book: ResMut<QuestBook>,
    worker: Option<Res<QuestWorker>>,
    quests: QuestResources,
    mut speak: EventWriter<SpeakEvent>,
    mut typed: EventReader<KeyboardInput>,
    mut recruit_events: EventWriter<RecruitFollowerEvent>,
    assets: Res<AssetServer>,
    mut line: Query<(&mut Text, &mut TextFont), (With<DialogLine>, Without<DialogOptions>, Without<DialogInputText>)>,
               mut options: Query<&mut Text, (With<DialogOptions>, Without<DialogLine>, Without<DialogInputText>)>,
               mut input_text: Query<&mut Text, (With<DialogInputText>, Without<DialogLine>, Without<DialogOptions>)>,
) {
    if !state.open {
        typed.clear();
        return;
    }
    if ctx.dirty {
        ctx.dirty = false;
        ctx.awaiting = false;
        ctx.typing = false;
        ctx.buffer.clear();
        ctx.asking = None;
        ctx.conversation.clear();
        ctx.scripted_response = None;
        typed.clear();
        for (mut text, mut font) in &mut line {
            text.0 = DIALOG_TEXT.to_string();
            font.font = assets.load(DIALOG_FONT);
            font.font_size = DIALOG_FONT_SIZE;
        }
        for mut text in &mut options {
            text.0 = MENU_TEXT.to_string();
        }
        set_input_text(&mut input_text, INPUT_PLACEHOLDER.to_string());
    }

    if handle_scripted_offer(&keys, &mut ctx, &quests.lib.0, &quests.states.0, &assets, &mut line, &mut options) {
        if let Some(response) = ctx.scripted_response.take() {
            for (mut text, mut font) in &mut line {
                text.0 = response.clone();
                font.font = assets.load(RUMOR_FONT);
                font.font_size = RUMOR_FONT_SIZE;
            }
            for mut text in &mut options {
                text.0 = MENU_TEXT.to_string();
            }
            let label = ctx.voice.as_ref().map_or_else(|| "NPC".to_string(), |v| v.label.clone());
            let spoken = format!("{label}: \"{}\"", ascii_punctuation(&response));
            if let (Some(s), Some(v)) = (spoken_part(&spoken), ctx.voice.as_ref()) {
                speak.send(SpeakEvent {
                    text: s,
                    speaker: Speaker {
                        key: v.key.clone(),
                           gender: v.gender,
                           role: v.role,
                           temperament: v.temperament,
                    },
                });
            }
        }
        return;
    }

    let label = ctx.voice.as_ref().map_or_else(|| "NPC".to_string(), |v| v.label.clone());
    let mut shown: Option<String> = None;

    if ctx.typing {
        let (mut submit, mut cancel) = (false, false);
        for ev in typed.read() {
            if ev.state != ButtonState::Pressed {
                continue;
            }
            match &ev.logical_key {
                Key::Enter => submit = true,
                Key::Escape => cancel = true,
                Key::Backspace => {
                    ctx.buffer.pop();
                }
                Key::Space => push_typed(&mut ctx.buffer, ' '),
                Key::Character(s) => {
                    for c in s.chars() {
                        push_typed(&mut ctx.buffer, c);
                    }
                }
                _ => {}
            }
        }
        let mut finished = cancel;
        if submit && !cancel {
            match clean_question(&ctx.buffer) {
                None => {}
                Some(question) => {
                    finished = true;
                    let description = ctx.voice.as_ref().map(|v| v.description.clone()).unwrap_or_default();
                    let key = ctx.voice.as_ref().map(|v| v.key.clone()).unwrap_or_default();
                    ctx.knowledge = knowledge_block(&quests.lib.0, &quests.states.0, &question, &key, &KnowledgeOptions::default());
                    ctx.conversation.push(("player".to_string(), question.clone()));
                    let asked = worker
                    .as_deref()
                    .and_then(|w| w.ask(&ctx.city_name, &description, &question, ctx.prompt_history()));
                    match asked {
                        Some(id) => {
                            ctx.asking = Some(id);
                            shown = Some(format!("{label} considers your question..."));
                        }
                        None => {
                            ctx.conversation.pop();
                            shown = Some(format!("{label} says nothing."));
                        }
                    }
                }
            }
        }
        if finished {
            ctx.typing = false;
            ctx.buffer.clear();
            set_input_text(&mut input_text, INPUT_PLACEHOLDER.to_string());
        } else {
            let cursor = if (time.elapsed_secs() * 2.0) as u32 % 2 == 0 { "_" } else { " " };
            set_input_text(&mut input_text, format!("> {}{cursor}", ctx.buffer));
        }
    } else {
        typed.clear();
        let idle = ctx.asking.is_none();
        let pressed = |a: KeyCode, b: KeyCode| keys.just_pressed(a) || keys.just_pressed(b);
        if idle && pressed(KeyCode::Digit2, KeyCode::Numpad2) {
            if worker.is_none() {
                shown = Some(format!("{label} doesn't seem to want to talk."));
            } else {
                ctx.typing = true;
                ctx.awaiting = false;
                ctx.buffer.clear();
            }
        } else if idle && pressed(KeyCode::Digit1, KeyCode::Numpad1) {
            match npc_reply(&mut book, worker.as_deref(), &ctx) {
                Reply::Say(message) => {
                    ctx.awaiting = false;
                    shown = Some(message);
                }
                Reply::Wait(message) => {
                    ctx.awaiting = true;
                    shown = Some(message);
                }
            }
        } else if ctx.awaiting {
            if let Reply::Say(message) = npc_reply(&mut book, worker.as_deref(), &ctx) {
                ctx.awaiting = false;
                shown = Some(message);
            }
        }

        // --- Risposta LLM (con eventuale reclutamento) ---
        if let Some(id) = ctx.asking {
            if let Some(result) = book.take_answer(id) {
                ctx.asking = None;
                match &result {
                    Ok((text, follow)) => {
                        ctx.conversation.push(("npc".to_string(), text.clone()));
                        // ★ L'LLM ha deciso che l'NPC segue il giocatore.
                        if *follow {
                            if let Some(entity) = ctx.npc_entity {
                                recruit_events.send(RecruitFollowerEvent { entity });
                            }
                        }
                    }
                    Err(()) => {}
                }
                shown = Some(answer_message(&label, &result));
            }
        }
    }

    if let Some(message) = shown {
        for (mut text, mut font) in &mut line {
            text.0 = message.clone();
            font.font = assets.load(RUMOR_FONT);
            font.font_size = RUMOR_FONT_SIZE;
        }
        if let (Some(spoken), Some(voice)) = (spoken_part(&message), ctx.voice.as_ref()) {
            speak.send(SpeakEvent {
                text: spoken,
                speaker: Speaker {
                    key: voice.key.clone(),
                       gender: voice.gender,
                       role: voice.role,
                       temperament: voice.temperament,
                },
            });
        }
    }
}

// =============================================================================
// Conversione NPC → Follower
// =============================================================================

/// Sistema che applica il reclutamento: rimuove `NpcWalker`, inserisce
/// `Follower`, reparenta l'entità a root (così sopravvive allo scarico dei
/// tile) e imposta il Transform in coordinate mondo.
fn apply_recruit(
    mut commands: Commands,
    mut events: EventReader<RecruitFollowerEvent>,
    walkers: Query<(&NpcWalker, &Transform, &GlobalTransform)>,
) {
    for ev in events.read() {
        info!("RECRUIT: ricevuto evento {:?}", ev.entity);
        match walkers.get(ev.entity) {
            Ok((walker, local_tf, gtf)) => {
                let follower = Follower::from_walker(walker);
                let world_pos = gtf.translation();
                let scale = local_tf.scale;
                commands.entity(ev.entity)
                .remove::<NpcWalker>()
                .insert(follower)
                .insert(Transform::from_translation(world_pos).with_scale(scale))
                .remove_parent();
                info!("RECRUIT: convertito {:?} in follower a {:?}", ev.entity, world_pos);
            }
            Err(QueryEntityError::NoSuchEntity(_)) => {
                warn!("RECRUIT: entità {:?} NON ESISTE più → despawnata (tile streaming durante l'attesa LLM?)", ev.entity);
            }
            Err(other) => {
                warn!("RECRUIT: entità {:?} esiste ma non ha NpcWalker+Transform+GlobalTransform → {}", ev.entity, other);
            }
        }
    }
}

// =============================================================================
// Test
// =============================================================================

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
    }

    #[test]
    fn typographic_punctuation_becomes_ascii() {
        assert_eq!(
            ascii_punctuation("L\u{2019}oste dice: \u{00AB}vieni\u{2026}\u{00BB} \u{2013} o no?"),
                   "L'oste dice: \"vieni...\" - o no?"
        );
    }

    #[test]
    fn only_quoted_speech_is_spoken() {
        assert_eq!(spoken_part("guard: \"He said \"no\" to me.\"").as_deref(), Some("He said \"no\" to me."));
        assert_eq!(spoken_part("They shrug. They don't seem to know anything about it."), None);
        assert_eq!(spoken_part("x: \"\""), None);
    }

    #[test]
    fn typed_characters_are_filtered_and_limited() {
        let mut b = String::new();
        for c in "Where is the inn?\n\t\u{7}è".chars() {
            push_typed(&mut b, c);
        }
        assert_eq!(b, "Where is the inn?è");
        let mut long = String::new();
        for _ in 0..QUESTION_MAX_CHARS + 50 {
            push_typed(&mut long, 'a');
        }
        assert_eq!(long.chars().count(), QUESTION_MAX_CHARS);
    }

    #[test]
    fn answer_message_is_spoken_only_when_there_is_an_answer() {
        let ok = answer_message("old beggar", &Ok(("Daggerfall, friend.".to_string(), false)));
        assert_eq!(ok, "old beggar: \"Daggerfall, friend.\"");
        assert_eq!(spoken_part(&ok).as_deref(), Some("Daggerfall, friend."));
        let fail = answer_message("old beggar", &Err(()));
        assert_eq!(fail, "old beggar says nothing.");
        assert_eq!(spoken_part(&fail), None);
    }

    #[test]
    fn answer_message_with_follow_true() {
        let ok = answer_message("barmaid", &Ok(("I'll follow you!".to_string(), true)));
        assert_eq!(ok, "barmaid: \"I'll follow you!\"");
    }

    #[test]
    fn placeholder_is_shown_when_not_typing() {
        assert!(INPUT_PLACEHOLDER.contains("Clicca"));
        assert!(MENU_TEXT.contains("[1]") && MENU_TEXT.contains("[E]"));
        assert!(OFFER_MENU.contains("[A]") && OFFER_MENU.contains("[R]"));
    }

    #[test]
    fn waiting_text_names_the_npc() {
        let v = NpcVoiceInfo { key: "k".into(), label: "old beggar".into(), ..Default::default() };
        assert_eq!(waiting_text(&v), "old beggar is thinking about it...");
    }

    #[test]
    fn scripted_context_starts_empty() {
        let ctx = DialogContext::new(1, (207, 213), None, None, None);
        assert!(ctx.scripted_offer.is_none());
        assert_eq!(ctx.scripted_state, ScriptedQuestState::None);
    }

    #[test]
    fn scripted_offer_state_transitions() {
        let mut ctx = DialogContext::new(1, (1, 1), Some(PoiKind::Guild), None, None);
        ctx.scripted_offer = Some(ScriptedOffer {
            quest_id: "N0B00Y16".into(),
                                  _quest_name: "Open a chest".into(),
                                  offer_text: "Offer text".into(),
        });
        ctx.scripted_state = ScriptedQuestState::Offering;
        assert_eq!(ctx.scripted_state, ScriptedQuestState::Offering);
        ctx.scripted_state = ScriptedQuestState::None;
        ctx.scripted_response = Some("You accept.".into());
        assert!(ctx.scripted_response.is_some());
    }

    #[test]
    fn player_race_label_covers_all() {
        for r in 0..8u8 {
            assert!(!player_race_label(r).is_empty());
        }
    }

    #[test]
    fn conversation_starts_empty_on_new_context() {
        let ctx = DialogContext::new(1, (207, 213), None, None, None);
        assert!(ctx.conversation.is_empty());
        assert!(ctx.npc_entity.is_none());
    }

    #[test]
    fn prompt_history_includes_knowledge_when_present() {
        let mut ctx = DialogContext::new(1, (207, 213), None, None, None);
        ctx.conversation.push(("player".into(), "hi".into()));
        ctx.conversation.push(("npc".into(), "hello".into()));
        let h = ctx.prompt_history();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].0, "player");
        ctx.knowledge = "\nTHINGS YOU HAVE HEARD:\n- x".to_string();
        let h = ctx.prompt_history();
        assert_eq!(h.len(), 3);
        assert_eq!(h[0].0, "system");
        assert!(h[0].1.contains("THINGS YOU HAVE HEARD"));
        assert_eq!(h[1].0, "player");
        assert_eq!(h[2].0, "npc");
    }
}
