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
//!  del LLM) lo scroll torna in cima.
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
//! L'LLM decide se l'NPC è stato persuaso e risponde `{"text": "...", "follow": true}`.
//! Il gioco intercetta il flag:
//!  - NPC esterno : l'entità `NpcWalker` diventa `Follower` in-place, staccata dal
//!    chunk (sopravvive allo scarico dei tile) e cammina animata dietro al giocatore.
//!  - NPC interno : l'entità originale viene despawnata e spawnata una nuova
//!    entità root che eredita la `StandardMaterial` del billboard originale (stessa
//!    immagine, nessuna camminata) e insegue il giocatore. Le dimensioni `w` / `h`
//!    vengono prese da `InteriorNpc::size` (in metri), così l'aspect ratio è preservato.
//!    L'NPC viene anche segnato in `InteriorState::recruited`, così al rientro
//!    nell'edificio non viene ri-spawnato (niente doppie copie).
//!
//! ## Comandi con i follower (durante il dialogo)
//!
//! * **W** = "Aspetta qui": aggiunge `FollowerWait::new()`. Il follower resta fermo
//!   in questa posizione (fuori) o in questo edificio (dentro), e ricompare dove
//!   l'hai lasciato se esci e rientri.
//! * **F** = "Seguimi": rimuove `FollowerWait`. Il follower riprende a inseguirti.
//!
//! Funzionano sia con follower esterni (animati) che interni (immagine fissa).
//!
//! ## Fix B0001 (Visibility)
//!
//! `DialogRoot` ha `Visibility`. `portrait` ha `ImageNode`, che in Bevy 0.15
//! richiede `Visibility`. Se un'entità può essere contemporaneamente
//! `DialogRoot` e `DialogPortrait`, Bevy non riesce a dimostrare che le due
//! query sono disgiunte e panica con B0001. Per questo `portrait` ha
//! `Without<DialogRoot>`. Stesso motivo per `followers`: `&GlobalTransform`
//! non accede a `Visibility`, ma tenerlo disgiunto da `DialogRoot` è
//! comunque più sicuro e previene futuri conflitti.

use crate::data::city_map::PoiKind;
use crate::data::flats::FlatCatalog;
use crate::data::{find_file_ci, Palette};
use crate::dfquest::{knowledge_block, KnowledgeOptions, Library, QuestStates};
use crate::interior::{CustomNpcCache, CustomNpcName, InteriorNpc, InteriorState};
use crate::mod_textures::ModTextures;
use crate::player::Player;
use crate::quest_llm::QuestWorker;
use crate::quests::{
    clean_question, describe_npc, face_key, flat_key, temperament_for, tone_for_npc, walker_key,
    walker_look, Hint, QuestBook, Tone, VoiceState, QUESTION_MAX_CHARS,
};
use crate::tiles::{Follower, FollowerWait, NpcWalker};
use crate::tts::{gender_from_caption, gender_from_look, Gender, SpeakEvent, Speaker};
use crate::world::{map_pixel_of, GameData, Origin};
use crate::{QuestLibRes, QuestStatesRes};
use bevy::ecs::query::QueryFilter;
use bevy::ecs::system::SystemParam;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::MouseWheel;
use bevy::input::ButtonState;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui::{ComputedNode, ScrollPosition};
use std::collections::HashMap;

const DIALOG_FONT: &str = "fonts/Sherwood.ttf";
const DIALOG_TEXT: &str = "Hello!";
const DIALOG_FONT_SIZE: f32 = 44.0;
const DIALOG_TEXT_COLOR: Color = Color::srgb(1.0, 0.9, 0.0);
const DIALOG_BACKGROUND: Color = Color::srgba(0.0, 0.0, 0.0, 0.75);
const DIALOG_DISTANCE: f32 = 2.4;
/// Raggio per parlare a un compagno: un po' piu' largo, perche' quando
/// recupera terreno puo' restare qualche metro indietro.
const FOLLOWER_DIALOG_DISTANCE: f32 = 3.5;
/// Per parlare a un compagno bisogna GUARDARLO: coseno dell'angolo massimo
/// (orizzontale) tra la direzione dello sguardo e il compagno (~30 gradi).
/// Cosi' E resta libero per porte e ingressi quando si guarda altrove.
const FOLLOWER_LOOK_DOT: f32 = 0.87;
/// Sotto questa distanza (m) il controllo dello sguardo e' ignorato.
const FOLLOWER_LOOK_MIN_DIST: f32 = 0.8;
const FACE_DISPLAY_SIZE: f32 = 256.0;
const FACES_FILE: &str = "TFAC00I0.RCI";
const FACES_PALETTE: &str = "PAL.PAL";
const FACE_PX: usize = 64;
const FACE_BYTES: usize = FACE_PX * FACE_PX;
const PORTRAIT_WIDTH: f32 = 260.0;
const PORTRAIT_CROP: f32 = 0.45;
// Menu e campo di testo
const DIALOG_TEXT_MAX_WIDTH: f32 = 760.0;
/// Altezza minima del box di testo per mostrare almeno 8-9 righe contemporaneamente.
const DIALOG_TEXT_MIN_HEIGHT: f32 = 280.0;
/// Altezza massima visibile del testo prima di richiedere lo scroll.
const DIALOG_TEXT_MAX_HEIGHT: f32 = 360.0;
const MENU_TEXT: &str = "[1] Ascolta rumori      [E] Congeda";
/// Menu mostrato quando si parla con un follower: oltre ad ascoltare e congedare,
/// si può ordinare di aspettare (W) o di seguire (F).
const MENU_FOLLOWER: &str =
"[1] Ascolta rumori   [2] Parla   [W] Aspetta qui   [F] Seguimi   [E] Congeda";
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
// Messaggio di reclutamento (verde)
const FOLLOWER_MSG_COLOR: Color = Color::srgb(0.2, 0.95, 0.4);
const FOLLOWER_MSG: &str = "★ is now following you! ★";

// =============================================================================
// Evento di reclutamento
// =============================================================================
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
    /// Cache degli NPC custom (serve a trovare il ritratto `por_*.png`).
    /// Sta qui per non superare il limite di 16 parametri dei sistemi.
    pub custom: Res<'w, CustomNpcCache>,
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
    /// True se l'NPC con cui si parla e' gia' un compagno (niente nuovo reclutamento).
    npc_is_follower: bool,
}

#[derive(Clone, Default)]
struct NpcVoiceInfo {
    key: String,
    label: String,
    description: String,
    gender: Option<Gender>,
    role: Option<Tone>,
    temperament: &'static str,
    /// Voce TTS imposta (campo `voice=` del .txt di un NPC custom).
    voice_override: Option<String>,
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
    fn with_voice_override(mut self, voice: Option<String>) -> Self {
        if let Some(v) = self.voice.as_mut() {
            v.voice_override = voice;
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
            npc_is_follower: false,
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
        enum Known { Ready(String), Pending, Unknown, Failed }
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
            (update_dialog_followers, update_dialog, dialog_menu, reset_keys_when_dialog_open)
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
    if !state.open { wheel.clear(); return; }
    let delta: f32 = wheel.read().map(|e| e.y).sum();
    if delta == 0.0 { return; }
    for mut sp in &mut scroll {
        sp.offset_y = (sp.offset_y - delta * WHEEL_STEP).max(0.0);
    }
}

fn dialog_scroll_keys(
    state: Res<DialogState>,
    keys: Res<ButtonInput<KeyCode>>,
    mut scroll: Query<&mut ScrollPosition, With<DialogScroll>>,
) {
    if !state.open { return; }
    let mut delta = 0.0f32;
    if keys.just_pressed(KeyCode::PageDown) {
        delta = -DIALOG_TEXT_MAX_HEIGHT * 0.9;
    } else if keys.just_pressed(KeyCode::PageUp) {
        delta = DIALOG_TEXT_MAX_HEIGHT * 0.9;
    } else if keys.just_pressed(KeyCode::Home) {
        for mut sp in &mut scroll { sp.offset_y = 0.0; }
        return;
    } else if keys.just_pressed(KeyCode::End) {
        for mut sp in &mut scroll { sp.offset_y = f32::MAX; }
        return;
    }
    if delta == 0.0 { return; }
    for mut sp in &mut scroll {
        sp.offset_y = (sp.offset_y + delta).max(0.0);
    }
}

fn dialog_scroll_reset_on_text_change(
    mut scroll: Query<&mut ScrollPosition, With<DialogScroll>>,
    line_changed: Query<&Text, (Changed<Text>, With<DialogLine>)>,
) {
    if line_changed.is_empty() { return; }
    for mut sp in &mut scroll { sp.offset_y = 0.0; }
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
        0 => "Breton", 1 => "Redguard", 2 => "Nord", 3 => "Dark Elf",
        4 => "High Elf", 5 => "Wood Elf", 6 => "Khajiit", 7 => "Argonian",
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
    if ctx.scripted_state != ScriptedQuestState::None { return None; }
    let place = interior.kind()?;
    if !matches!(place, PoiKind::Guild) { return None; }
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
    exterior_npcs: Query<(Entity, &NpcWalker, &GlobalTransform), Without<Follower>>,
                     interior_npcs: Query<(Entity, &InteriorNpc, &GlobalTransform, Option<&CustomNpcName>), Without<Follower>>,
                     mut root: Query<&mut Visibility, With<DialogRoot>>,
                     // FIX B0001: `ImageNode` (qui dentro) richiede `Visibility` in
                     // Bevy 0.15; senza `Without<DialogRoot>` il compilatore non
                     // riesce a dimostrare che le due query sono disgiunte.
                     mut portrait: Query<
                     (&mut ImageNode, &mut Node),
                     (With<DialogPortrait>, Without<DialogRoot>),
                     >,
                     mut flats_catalog: Local<Option<FlatCatalog>>,
) {
    let pressed_e = keys.just_pressed(KeyCode::KeyE);
    if state.open {
        if ctx.typing { return; }
        if pressed_e || keys.just_pressed(KeyCode::Escape) {
            close_dialog(&mut state, &mut root);
            if pressed_e { keys.clear_just_pressed(KeyCode::KeyE); }
        }
        return;
    }
    if !pressed_e { return; }
    let Ok(player_tf) = player.get_single() else { return };
    let player_pos = player_tf.translation;

    // --- NPC interni ---
    if interior.is_inside() {
        let mut nearest_int: Option<(f32, Entity, &InteriorNpc, Option<&CustomNpcName>)> = None;
        for (entity, npc, gtf, custom_name) in &interior_npcs {
            let pos = gtf.translation();
            if (pos.y - player_pos.y).abs() > 4.0 { continue; }
            let distance = Vec2::new(pos.x - player_pos.x, pos.z - player_pos.z).length();
            if distance <= DIALOG_DISTANCE && nearest_int.map_or(true, |(d, _, _, _)| distance < d) {
                nearest_int = Some((distance, entity, npc, custom_name));
            }
        }
        if let Some((_, npc_entity, npc, custom_name_opt)) = nearest_int {
            let npc_name = if let Some(custom) = custom_name_opt {
                custom.name.clone()
            } else {
                let catalog = flats_catalog.get_or_insert_with(|| FlatCatalog::load(&data.dir));
                let flat_info = catalog.get(npc.flat_archive, npc.flat_record);
                flat_info.map(|i| i.caption.clone()).unwrap_or_else(|| "an ordinary townsperson".to_string())
            };

            let face_record = npc.face_record;

            let custom_por = custom_name_opt
            .and_then(|c| custom_portrait(&c.name, &quests.custom, &mut mods, &mut images));
            let face_opt = match custom_por {
                Some((h, f, sz)) => Some((h, f, Some(sz))),
                None => faces
                .get(face_record, &data, &mut mods, &mut images)
                .map(|(h, f)| (h, f, None)),
            };
            if let Some((face, is_hd, face_size)) = face_opt {
                let npc_flat = if custom_name_opt.is_none() {
                    Some((npc.flat_archive, npc.flat_record))
                } else {
                    None
                };

                let catalog = flats_catalog.get_or_insert_with(|| FlatCatalog::load(&data.dir));
                let flat_info = if npc_flat.is_some() {
                    catalog.get(npc.flat_archive, npc.flat_record)
                } else {
                    None
                };

                let npc_caption = if custom_name_opt.is_some() {
                    Some(npc_name.clone())
                } else {
                    flat_info.map(|i| i.caption.clone())
                };

                let city = map_pixel_of(&origin, player_pos);
                let city_name = data.locations.get(&city).map(|l| l.name.clone()).unwrap_or_default();
                let place = interior.kind();

                let dialog_key = if let Some(_custom) = custom_name_opt {
                    let mut h = 0u64;
                    for b in npc_name.bytes() {
                        h = h.wrapping_mul(31).wrapping_add(b as u64);
                    }
                    h
                } else {
                    face_record as u64
                };

                let tone = tone_for_npc(dialog_key, city, place);

                let (key, who, role) = match &npc_caption {
                    Some(caption) => {
                        if npc_flat.is_some() {
                            (flat_key(npc.flat_archive, npc.flat_record), caption.clone(), None)
                        } else {
                            (format!("custom_{}", caption.replace(' ', "_")), caption.clone(), None)
                        }
                    }
                    None => (face_key(face_record), "an ordinary townsperson".to_string(), Some(tone.role())),
                };

                let temperament = temperament_for(&key, city);
                let description = custom_name_opt
                .and_then(|c| custom_description(&c.name, &quests.custom))
                .unwrap_or_else(|| describe_npc(&who, role, place, temperament));

                let gender = if custom_name_opt.is_some() {
                    gender_from_caption(&npc_name)
                } else {
                    flat_info
                    .and_then(|i| match i.gender { 1 => Some(Gender::Male), 2 => Some(Gender::Female), _ => None })
                    .or_else(|| npc_caption.as_deref().and_then(gender_from_caption))
                };

                let knowledge = knowledge_block(&quests.lib.0, &quests.states.0, "", &key, &KnowledgeOptions::default());

                *ctx = DialogContext::new(dialog_key, city, place, npc_flat, npc_caption)
                .with_voice(key, who, description)
                .with_speaker(gender, role.map(|_| tone), temperament)
                .with_city_name(city_name)
                .with_knowledge(knowledge)
                .with_voice_override(custom_name_opt.and_then(|c| custom_voice(&c.name, &quests.custom)));

                ctx.npc_entity = Some(npc_entity);
                ctx.npc_is_follower = false;

                if let Some(offer) = find_quest_offer(&quests.lib.0, &quests.states.0, &*interior, &*data, player_pos, &ctx) {
                    ctx.scripted_offer = Some(offer);
                    ctx.scripted_state = ScriptedQuestState::Offering;
                }

                if let Ok((mut image_node, mut node)) = portrait.get_single_mut() {
                    image_node.image = face;
                    image_node.rect = None;
                    image_node.flip_y = is_hd;
                    node.width = Val::Px(face_size.map_or(FACE_DISPLAY_SIZE, |d| d.x));
                    node.height = Val::Px(face_size.map_or(FACE_DISPLAY_SIZE, |d| d.y));
                }
                state.open = true;
                for mut visibility in root.iter_mut() { *visibility = Visibility::Visible; }
                keys.clear_just_pressed(KeyCode::KeyE);
            }
        }
        return;
    }

    // --- NPC esterni ---
    let mut nearest_ext: Option<(f32, Entity, &NpcWalker)> = None;
    for (entity, npc, gtf) in &exterior_npcs {
        let pos = gtf.translation();
        if (pos.y - player_pos.y).abs() > 4.0 { continue; }
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
    let knowledge = knowledge_block(&quests.lib.0, &quests.states.0, "", &key, &KnowledgeOptions::default());
    *ctx = DialogContext::new(face as u64, city, None, None, None)
    .with_voice(key, format!("{look} ({})", tone.role()), description)
    .with_speaker(gender_from_look(look), Some(tone), temperament)
    .with_city_name(city_name)
    .with_knowledge(knowledge);
    ctx.npc_entity = Some(npc_entity);
    ctx.npc_is_follower = false;
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
    for mut visibility in root.iter_mut() { *visibility = Visibility::Visible; }
    keys.clear_just_pressed(KeyCode::KeyE);
}

// =============================================================================
// Apertura dialogo con i COMPAGNI (E)
// =============================================================================
/// Se il giocatore preme E vicino a un `Follower`, apre il dialogo con lui
/// (priorita' sugli NPC non reclutati). Identita', chiave LLM, descrizione e
/// ritratto sono gli stessi che aveva prima del reclutamento, cosi' voce e
/// personalita' restano coerenti. Funziona dentro e fuori dagli edifici.
///
/// Durante il dialogo con un follower, `dialog_menu` abilita anche i comandi
/// `[W] Aspetta qui` e `[F] Seguimi` (vedi `MENU_FOLLOWER`).
#[allow(clippy::too_many_arguments)]
pub fn update_dialog_followers(
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
    // FIX B0001: `Without<DialogRoot>` rende questa query disgiunta da
    // `root: Query<&mut Visibility, With<DialogRoot>>` più sotto.
    followers: Query<
    (Entity, &Follower, &GlobalTransform, Option<&CustomNpcName>),
                               Without<DialogRoot>,
                               >,
                               mut root: Query<&mut Visibility, With<DialogRoot>>,
                               // FIX B0001: `ImageNode` (qui dentro) richiede `Visibility` in Bevy 0.15;
                               // senza `Without<DialogRoot>` il compilatore non riesce a dimostrare
                               // che le due query sono disgiunte.
                               mut portrait: Query<
                               (&mut ImageNode, &mut Node),
                               (With<DialogPortrait>, Without<DialogRoot>),
                               >,
                               mut flats_catalog: Local<Option<FlatCatalog>>,
) {
    let _ = &interior;
    if state.open || !keys.just_pressed(KeyCode::KeyE) { return; }
    let Ok(player_tf) = player.get_single() else { return };
    let player_pos = player_tf.translation;

    let fwd = player_tf.forward();
    let fwd_xz = Vec2::new(fwd.x, fwd.z).normalize_or_zero();
    let mut nearest: Option<(f32, Entity, &Follower, Option<&CustomNpcName>)> = None;
    for (entity, fol, gtf, custom) in &followers {
        let pos = gtf.translation();
        if (pos.y - player_pos.y).abs() > 4.0 { continue; }
        let to = Vec2::new(pos.x - player_pos.x, pos.z - player_pos.z);
        let distance = to.length();
        if distance > FOLLOWER_DIALOG_DISTANCE { continue; }
        if distance > FOLLOWER_LOOK_MIN_DIST && fwd_xz.dot(to / distance) < FOLLOWER_LOOK_DOT { continue; }
        if nearest.map_or(true, |(d, _, _, _)| distance < d) {
            nearest = Some((distance, entity, fol, custom));
        }
    }
    let Some((_, npc_entity, fol, custom)) = nearest else { return };

    let city = map_pixel_of(&origin, player_pos);
    let city_name = data.locations.get(&city).map(|l| l.name.clone()).unwrap_or_default();

    // ---- Compagno nato da un NPC interno (billboard statico) ----
    if let Some(id) = fol.interior_identity() {
        let custom_por = custom
        .and_then(|c| custom_portrait(&c.name, &quests.custom, &mut mods, &mut images));
        let face_opt = match custom_por {
            Some((h, f, sz)) => Some((h, f, Some(sz))),
            None => faces
            .get(id.face_record, &data, &mut mods, &mut images)
            .map(|(h, f)| (h, f, None)),
        };
        let Some((face, is_hd, face_size)) = face_opt else { return };
        let catalog = flats_catalog.get_or_insert_with(|| FlatCatalog::load(&data.dir));
        let npc_flat = custom.is_none().then_some((id.flat_archive, id.flat_record));
        let flat_info = if npc_flat.is_some() { catalog.get(id.flat_archive, id.flat_record) } else { None };
        let npc_caption = match custom {
            Some(c) => Some(c.name.clone()),
            None => flat_info.map(|i| i.caption.clone()),
        };
        let place = None;
        let dialog_key = match custom {
            Some(c) => c.name.bytes().fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64)),
            None => id.face_record as u64,
        };
        let tone = tone_for_npc(dialog_key, city, place);
        let (key, who, role) = match &npc_caption {
            Some(caption) if npc_flat.is_some() => (flat_key(id.flat_archive, id.flat_record), caption.clone(), None),
            Some(caption) => (format!("custom_{}", caption.replace(' ', "_")), caption.clone(), None),
            None => (face_key(id.face_record), "an ordinary townsperson".to_string(), Some(tone.role())),
        };
        let temperament = temperament_for(&key, city);
        let description = custom
        .and_then(|c| custom_description(&c.name, &quests.custom))
        .unwrap_or_else(|| describe_npc(&who, role, place, temperament));
        let gender = if custom.is_some() {
            npc_caption.as_deref().and_then(gender_from_caption)
        } else {
            flat_info
            .and_then(|i| match i.gender { 1 => Some(Gender::Male), 2 => Some(Gender::Female), _ => None })
            .or_else(|| npc_caption.as_deref().and_then(gender_from_caption))
        };
        let knowledge = knowledge_block(&quests.lib.0, &quests.states.0, "", &key, &KnowledgeOptions::default());
        *ctx = DialogContext::new(dialog_key, city, place, npc_flat, npc_caption)
        .with_voice(key, who, description)
        .with_speaker(gender, role.map(|_| tone), temperament)
        .with_city_name(city_name)
        .with_knowledge(knowledge)
        .with_voice_override(custom.and_then(|c| custom_voice(&c.name, &quests.custom)));
        if let Ok((mut image_node, mut node)) = portrait.get_single_mut() {
            image_node.image = face;
            image_node.rect = None;
            image_node.flip_y = is_hd;
            node.width = Val::Px(face_size.map_or(FACE_DISPLAY_SIZE, |d| d.x));
            node.height = Val::Px(face_size.map_or(FACE_DISPLAY_SIZE, |d| d.y));
        }
    } else {
        // ---- Compagno nato da un NpcWalker esterno (animato) ----
        let face = fol.face_record().unwrap_or(0);
        let tone = tone_for_npc(face as u64, city, None);
        let look = walker_look(face);
        let key = walker_key(face);
        let temperament = temperament_for(&key, city);
        let description = describe_npc(look, Some(tone.role()), None, temperament);
        let knowledge = knowledge_block(&quests.lib.0, &quests.states.0, "", &key, &KnowledgeOptions::default());
        *ctx = DialogContext::new(face as u64, city, None, None, None)
        .with_voice(key, format!("{look} ({})", tone.role()), description)
        .with_speaker(gender_from_look(look), Some(tone), temperament)
        .with_city_name(city_name)
        .with_knowledge(knowledge);
        let (image, crop, width, height, is_hd) =
        match fol.face_record().and_then(|r| faces.get(r, &data, &mut mods, &mut images)) {
            Some((face, is_hd)) => (face, None, FACE_DISPLAY_SIZE, FACE_DISPLAY_SIZE, is_hd),
            None => {
                let Some((sprite, size)) = fol.portrait() else { return };
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
    }

    ctx.npc_entity = Some(npc_entity);
    ctx.npc_is_follower = true;
    state.open = true;
    for mut visibility in root.iter_mut() { *visibility = Visibility::Visible; }
    keys.clear_just_pressed(KeyCode::KeyE);
}

/// Ritratto dedicato di un NPC custom: `por_<nome>.png` nella cartella delle
/// texture. `<nome>` e' il nome della texture dell'NPC senza il prefisso `npc_`
/// (`npc_bartolo.png` -> `por_bartolo.png`); in seconda battuta si prova col nome
/// del `.txt` in minuscolo con gli spazi sostituiti da `_`.
/// Ritorna (immagine, serve flip verticale, dimensione da mostrare in px).
/// Voce TTS imposta per l'NPC custom (riga `voice=` del .txt), se presente.
fn custom_voice(name: &str, cache: &CustomNpcCache) -> Option<String> {
    cache
    .npcs
    .iter()
    .find(|n| n.name == name)
    .and_then(|n| n.voice.clone())
}

/// Descrizione dell'NPC custom (riga `description=` del .txt), se presente.
fn custom_description(name: &str, cache: &CustomNpcCache) -> Option<String> {
    cache
    .npcs
    .iter()
    .find(|n| n.name == name)
    .and_then(|n| n.description.clone())
}

fn custom_portrait(
    name: &str,
    cache: &CustomNpcCache,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
) -> Option<(Handle<Image>, bool, Vec2)> {
    let mut candidates: Vec<String> = Vec::new();
    if let Some(d) = cache.npcs.iter().find(|n| n.name == name) {
        let base = d.texture_stem.strip_prefix("npc_").unwrap_or(&d.texture_stem);
        candidates.push(format!("por_{base}"));
    }
    candidates.push(format!("por_{}", name.trim().to_lowercase().replace(' ', "_")));
    for c in candidates {
        for stem in [c.clone(), c.to_lowercase()] {
            if let Some(handle) = mods.image_by_name(&stem, images) {
                let flip = mods.needs_v_flip(&handle);
                let px = mods
                .image_size(&handle)
                .or_else(|| images.get(&handle).map(|i| i.size_f32()))
                .unwrap_or(Vec2::splat(FACE_DISPLAY_SIZE));
                let k = FACE_DISPLAY_SIZE / px.x.max(px.y).max(1.0);
                return Some((handle, flip, px * k));
            }
        }
    }
    None
}

fn reset_keys_when_dialog_open(state: Res<DialogState>, mut keys: ResMut<ButtonInput<KeyCode>>) {
    if state.open { keys.reset_all(); }
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
    if !state.open || ctx.typing { return; }
    for interaction in &interactions {
        if *interaction == Interaction::Pressed {
            ctx.typing = true;
            ctx.buffer.clear();
        }
    }
}

fn set_input_text<F: QueryFilter>(texts: &mut Query<&mut Text, F>, s: String) {
    for mut text in texts.iter_mut() { text.0 = s.clone(); }
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
    line: &mut Query<(&mut Text, &mut TextFont, &mut TextColor), (With<DialogLine>, Without<DialogOptions>, Without<DialogInputText>)>,
                         options: &mut Query<&mut Text, (With<DialogOptions>, Without<DialogLine>, Without<DialogInputText>)>,
) -> bool {
    if ctx.scripted_state != ScriptedQuestState::Offering { return false; }
    let Some(offer) = ctx.scripted_offer.clone() else {
        ctx.scripted_state = ScriptedQuestState::None;
        return false;
    };
    for (mut text, mut font, _color) in line.iter_mut() {
        text.0 = offer.offer_text.clone();
        font.font = assets.load(RUMOR_FONT);
        font.font_size = RUMOR_FONT_SIZE;
    }
    for mut text in options.iter_mut() { text.0 = OFFER_MENU.to_string(); }
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
    mut commands: Commands,
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
    mut line: Query<(&mut Text, &mut TextFont, &mut TextColor), (With<DialogLine>, Without<DialogOptions>, Without<DialogInputText>)>,
               mut options: Query<&mut Text, (With<DialogOptions>, Without<DialogLine>, Without<DialogInputText>)>,
               mut input_text: Query<&mut Text, (With<DialogInputText>, Without<DialogLine>, Without<DialogOptions>)>,
) {
    if !state.open { typed.clear(); return; }
    if ctx.dirty {
        ctx.dirty = false;
        ctx.awaiting = false;
        ctx.typing = false;
        ctx.buffer.clear();
        ctx.asking = None;
        ctx.conversation.clear();
        ctx.scripted_response = None;
        typed.clear();
        for (mut text, mut font, mut color) in &mut line {
            text.0 = DIALOG_TEXT.to_string();
            font.font = assets.load(DIALOG_FONT);
            font.font_size = DIALOG_FONT_SIZE;
            *color = TextColor(DIALOG_TEXT_COLOR);
        }
        // Menu contestuale: con un follower mostra anche W/F.
        let menu = if ctx.npc_is_follower { MENU_FOLLOWER } else { MENU_TEXT };
        for mut text in &mut options { text.0 = menu.to_string(); }
        set_input_text(&mut input_text, INPUT_PLACEHOLDER.to_string());
    }
    if handle_scripted_offer(&keys, &mut ctx, &quests.lib.0, &quests.states.0, &assets, &mut line, &mut options) {
        if let Some(response) = ctx.scripted_response.take() {
            for (mut text, mut font, _color) in &mut line {
                text.0 = response.clone();
                font.font = assets.load(RUMOR_FONT);
                font.font_size = RUMOR_FONT_SIZE;
            }
            for mut text in &mut options { text.0 = MENU_TEXT.to_string(); }
            let label = ctx.voice.as_ref().map_or_else(|| "NPC".to_string(), |v| v.label.clone());
            let spoken = format!("{label}: \"{}\"", ascii_punctuation(&response));
            if let (Some(s), Some(v)) = (spoken_part(&spoken), ctx.voice.as_ref()) {
                speak.send(SpeakEvent {
                    text: s,
                    speaker: Speaker { key: v.key.clone(), gender: v.gender, role: v.role, temperament: v.temperament, voice: v.voice_override.clone() },
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
            if ev.state != ButtonState::Pressed { continue; }
            match &ev.logical_key {
                Key::Enter => submit = true,
                Key::Escape => cancel = true,
                Key::Backspace => { ctx.buffer.pop(); }
                Key::Space => push_typed(&mut ctx.buffer, ' '),
                Key::Character(s) => { for c in s.chars() { push_typed(&mut ctx.buffer, c); } }
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
                    let asked = worker.as_deref()
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

        // Comandi per i follower (solo se stiamo parlando con un compagno):
        // W = aspetta qui, F = seguimi.
        if idle && ctx.npc_is_follower {
            if let Some(entity) = ctx.npc_entity {
                if keys.just_pressed(KeyCode::KeyW) {
                    if let Some(mut ent) = commands.get_entity(entity) {
                        ent.insert(FollowerWait::new());
                        shown = Some(format!("{}: \"I'll wait here.\"", label));
                    }
                } else if keys.just_pressed(KeyCode::KeyF) {
                    if let Some(mut ent) = commands.get_entity(entity) {
                        ent.remove::<FollowerWait>();
                        shown = Some(format!("{}: \"I'll follow you.\"", label));
                    }
                }
            }
        }

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
                Reply::Say(message) => { ctx.awaiting = false; shown = Some(message); }
                Reply::Wait(message) => { ctx.awaiting = true; shown = Some(message); }
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
                let mut recruited = false;
                match &result {
                    Ok((text, follow)) => {
                        ctx.conversation.push(("npc".to_string(), text.clone()));
                        if *follow && !ctx.npc_is_follower {
                            if let Some(entity) = ctx.npc_entity {
                                recruit_events.send(RecruitFollowerEvent { entity });
                                recruited = true;
                                info!("RECRUIT: invio evento per {:?}", entity);
                            }
                        }
                    }
                    Err(()) => {}
                }
                if recruited {
                    let npc_text = match &result { Ok((t, _)) => t.clone(), Err(()) => String::new() };
                    shown = Some(format!("{}: \"{}\"\n\n{}", label, ascii_punctuation(&npc_text), FOLLOWER_MSG));
                } else {
                    shown = Some(answer_message(&label, &result));
                }
            }
        }
    }
    if let Some(message) = shown {
        let is_follower_msg = message.contains(FOLLOWER_MSG);
        for (mut text, mut font, mut color) in &mut line {
            text.0 = message.clone();
            font.font = assets.load(RUMOR_FONT);
            font.font_size = RUMOR_FONT_SIZE;
            *color = if is_follower_msg { TextColor(FOLLOWER_MSG_COLOR) } else { TextColor(DIALOG_TEXT_COLOR) };
        }
        if let (Some(spoken), Some(voice)) = (spoken_part(&message), ctx.voice.as_ref()) {
            speak.send(SpeakEvent {
                text: spoken,
                speaker: Speaker { key: voice.key.clone(), gender: voice.gender, role: voice.role, temperament: voice.temperament, voice: voice.voice_override.clone() },
            });
        }
    }
}

// =============================================================================
// Conversione NPC → Follower
// =============================================================================
#[allow(clippy::too_many_arguments)]
fn apply_recruit(
    mut commands: Commands,
    mut events: EventReader<RecruitFollowerEvent>,
    walkers: Query<(&NpcWalker, &Transform, &GlobalTransform)>,
                 interiors: Query<(&InteriorNpc, &MeshMaterial3d<StandardMaterial>, Option<&CustomNpcName>), With<InteriorNpc>>,
                 player: Query<&Transform, With<Player>>,
                 mut interior: ResMut<InteriorState>,
                 mut meshes: ResMut<Assets<Mesh>>,
                 mut quad: Local<Option<Handle<Mesh>>>,
) {
    for ev in events.read() {
        if let Ok((walker, local_tf, gtf)) = walkers.get(ev.entity) {
            let follower = Follower::from_walker(walker);
            let world_pos = gtf.translation();
            let scale = local_tf.scale;
            commands.entity(ev.entity)
            .remove::<NpcWalker>()
            .insert(follower)
            .insert(Transform::from_translation(world_pos).with_scale(scale))
            .remove_parent();
            info!("RECRUIT: {:?} esterno -> follower animato", ev.entity);
            continue;
        }
        if let Ok((npc, mat, custom)) = interiors.get(ev.entity) {
            commands.entity(ev.entity).despawn();
            let size = npc.size;
            match custom {
                Some(c) => interior.mark_recruited_custom(&c.name),
                None => interior.mark_recruited(npc.flat_archive, npc.flat_record),
            }
            let follower_copy = Follower::from_static_mobile_full(
                mat.0.clone(),
                                                                  size,
                                                                  npc.flat_archive,
                                                                  npc.flat_record,
                                                                  npc.face_record,
            );
            let spawn = match player.get_single() {
                Ok(tf) => {
                    let fwd = tf.forward();
                    let fxz = Vec2::new(fwd.x, fwd.z).normalize_or_zero();
                    tf.translation + Vec3::new(fxz.x, 0.0, fxz.y) * 1.5
                }
                Err(_) => Vec3::ZERO,
            };
            let q = quad.get_or_insert_with(|| meshes.add(Rectangle::new(1.0, 1.0))).clone();
            let mut e = commands.spawn((
                Mesh3d(q),
                                        MeshMaterial3d(mat.0.clone()),
                                        Transform::from_xyz(spawn.x, spawn.y, spawn.z)
                                        .with_scale(Vec3::new(size.x, size.y, 1.0)),
                                        crate::interior::FlatBillboard,
                                        follower_copy,
            ));
            if let Some(c) = custom {
                e.insert(CustomNpcName { name: c.name.clone() });
            }
            info!(
                "RECRUIT: {:?} interno -> despawn + copia mobile arch={} rec={} face={}",
                ev.entity, npc.flat_archive, npc.flat_record, npc.face_record
            );
            continue;
        }
        warn!("RECRUIT: entità {:?} non trovata (né walker né interior)", ev.entity);
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
        for c in "Where is the inn?\n\t\u{7}è".chars() { push_typed(&mut b, c); }
        assert_eq!(b, "Where is the inn?è");
        let mut long = String::new();
        for _ in 0..QUESTION_MAX_CHARS + 50 { push_typed(&mut long, 'a'); }
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
        assert!(MENU_FOLLOWER.contains("[W]") && MENU_FOLLOWER.contains("[F]"));
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
        for r in 0..8u8 { assert!(!player_race_label(r).is_empty()); }
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
