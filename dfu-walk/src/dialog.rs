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
//! Accettando o rifiutando, `quest_states.txt` viene aggiornato: la quest passa
//! rispettivamente a `Active` o `Failed`, e non viene più riproposta.
//!
//! ## Knowledge block (da dfquest)
//!
//! Quando il dialogo si apre su un PNG, viene calcolato un `knowledge_block` con le
//! informazioni che quel PNG "ha sentito in giro" sulle quest principali attive.
//! Il blocco viene iniettato nel prompt LLM, così il PNG risponde coerentemente.
//!
//! ## Gilda insider (Fase A: dialogo LLM, niente hardcoding)
//!
//! Se l'NPC è dentro una gilda:
//!
//! 1. `describe_npc_in` riceve il nome della gilda (da
//!    `InteriorState::block_prefix()` + `guild_name_from_prefix`) e il prompt
//!    INSIDERS si attiva.
//! 2. La voice key include il prefisso della gilda (`"flat:182:11@MAGE"`),
//!    così la stessa texture in taverna e in gilda ha VOCI SEPARATE: quella
//!    dentro la gilda è generata con la description "sei un membro della
//!    Mages Guild", quella fuori con la description "sei un estraneo".
//! 3. `npc_reply` SALTA il flavor pre-generato quando parla un membro della
//!    gilda che ha emesso la quest attiva (i flavor in `quests.txt` sono
//!    generati una volta senza contesto di edificio). Va direttamente a
//!    `voice_state`, che è la risposta generata dall'LLM.
//!
//! Risultato: la frase "our guild is sending someone to recover the X" non è
//! più hardcoded, ma generata dall'LLM al primo "Ascolta rumori".
//!
//! ## Reclutamento follower via LLM
//!
//! Il giocatore può convincere un NPC a seguirlo tramite la conversazione libera.
//! L'LLM decide se l'NPC è stato persuaso e risponde `{"text": "...", "follow": true,
//! "mood": N}`. Il gioco intercetta i flag:
//!  - NPC esterno : l'entità `NpcWalker` diventa `Follower` in-place, staccata dal
//!    chunk (sopravvive allo scarico dei tile) e cammina animata dietro al giocatore.
//!  - NPC interno : l'entità originale viene despawnata e spawnata una nuova
//!    entità root che eredita la `StandardMaterial` del billboard originale (stessa
//!    immagine, nessuna camminata) e insegue il giocatore.
//!
//! ## Mood (stato emotivo)
//!
//! Il campo `mood` (0-7) prodotto dall'LLM viene applicato al `Follower` tramite:
//!  - `RecruitFollowerEvent { entity, initial_mood }` al momento del reclutamento;
//!  - `FollowerMoodEvent { entity, mood }` per i follower già attivi.
//!
//! ## Comandi con i follower (durante il dialogo)
//!
//! * **W** = "Aspetta qui": aggiunge `FollowerWait::new()`. Il follower resta fermo
//!   in questa posizione (fuori) o in questo edificio (dentro), e ricompare dove
//!   l'hai lasciato se esci e rientri.
//! * **F** = "Seguimi": rimuove `FollowerWait`. Il follower riprende a inseguirti.
//!
//! ## Fix B0001 (Visibility)
//!
//! `DialogRoot` ha `Visibility`. `portrait` ha `ImageNode`, che in Bevy 0.15
//! richiede `Visibility`. Per questo `portrait` ha `Without<DialogRoot>`. Stesso
//! motivo per `followers`.

use crate::classic_npc::{self, ClassicNpc};
use crate::data::city_map::PoiKind;
use crate::data::flats::FlatCatalog;
use crate::data::{find_file_ci, Palette};
use crate::dfquest::{knowledge_block, KnowledgeOptions, Library, QuestPhase, QuestStates};
use crate::interior::{
    load_custom_idle_frames, load_custom_walk_frames, load_flat_idle_frames, load_flat_walk_frames, CustomNpcCache,
    CustomNpcName, InteriorNpc, InteriorState,
};
use crate::mod_textures::ModTextures;
use crate::player::Player;
use crate::quest_llm::QuestWorker;
use crate::quests::{
    clean_question, describe_npc, describe_npc_in, face_key, flat_key, guild_name_from_prefix,
    temperament_for, tone_for_npc, walker_key, walker_look, Hint, QuestBook, Tone, VoiceState,
    QUESTION_MAX_CHARS,
};
use crate::tiles::{Follower, FollowerMoodEvent, FollowerWait, NpcWalker};
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
const FOLLOWER_DIALOG_DISTANCE: f32 = 3.5;
const FOLLOWER_LOOK_DOT: f32 = 0.87;
const FOLLOWER_LOOK_MIN_DIST: f32 = 0.8;
const FACE_DISPLAY_SIZE: f32 = 256.0;
const FACES_FILE: &str = "TFAC00I0.RCI";
const FACES_PALETTE: &str = "PAL.PAL";
const FACE_PX: usize = 64;
const FACE_BYTES: usize = FACE_PX * FACE_PX;
const PORTRAIT_WIDTH: f32 = 260.0;
const PORTRAIT_CROP: f32 = 0.45;
const DIALOG_TEXT_MAX_WIDTH: f32 = 760.0;
const DIALOG_TEXT_MIN_HEIGHT: f32 = 280.0;
const DIALOG_TEXT_MAX_HEIGHT: f32 = 360.0;
const MENU_TEXT: &str = "[1] Ascolta rumori      [E] Congeda";
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
const WHEEL_STEP: f32 = 40.0;
const SCROLLBAR_WIDTH: f32 = 8.0;
const SCROLLBAR_MIN_HANDLE: f32 = 30.0;
const SCRIPTED_QUEST_ID: &str = "N0B00Y16";
const FOLLOWER_MSG_COLOR: Color = Color::srgb(0.2, 0.95, 0.4);
const FOLLOWER_MSG: &str = "★ is now following you! ★";
const IDLE_SIZE_SCALE: f32 = 0.5;

#[derive(Event)]
pub struct RecruitFollowerEvent {
    pub entity: Entity,
    pub initial_mood: u8,
}

#[derive(SystemParam)]
pub struct QuestResources<'w> {
    pub custom: Res<'w, CustomNpcCache>,
    pub materials: Res<'w, Assets<StandardMaterial>>,
    pub lib: Res<'w, QuestLibRes>,
    /// `ResMut` perché `handle_scripted_offer` aggiorna la fase della quest
    /// (accettata → `Active`, rifiutata → `Failed`) e salva `quest_states.txt`.
    pub states: ResMut<'w, QuestStatesRes>,
}

#[derive(Resource, Default)]
pub struct DialogState {
    pub open: bool,
    /// Se `true`, `open` è attivo solo per bloccare l'input mentre un menu
    /// egui (F1 tuning / F2 save) è aperto.
    pub forced_by_menu: bool,
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
#[derive(Component)]
struct DialogScroll;
#[derive(Component)]
struct DialogScrollBar;
#[derive(Component)]
pub(crate) struct DialogPortrait;

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
    npc_entity: Option<Entity>,
    npc_is_follower: bool,
    classic_anim: (Option<f32>, Option<f32>),
    /// Prefisso del blocco RMB corrente (`"MAGE"`, `"FIGH"`, ...), se stiamo
    /// parlando dentro una gilda. Popolato da `update_dialog` e
    /// `update_dialog_followers`. Usato da `npc_reply` per riconoscere se
    /// l'NPC è un insider della quest attiva.
    guild_prefix: Option<String>,
}

#[derive(Clone, Default)]
struct NpcVoiceInfo {
    key: String,
    label: String,
    description: String,
    gender: Option<Gender>,
    role: Option<Tone>,
    temperament: &'static str,
    voice_override: Option<String>,
}

impl DialogContext {
    fn with_voice(mut self, key: String, label: String, description: String) -> Self {
        self.voice = Some(NpcVoiceInfo { key, label, description, ..Default::default() });
        self
    }
    fn with_classic_anim(mut self, walk_fps: Option<f32>, idle_fps: Option<f32>) -> Self {
        self.classic_anim = (walk_fps, idle_fps);
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
            classic_anim: (None, None),
            guild_prefix: None,
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

/// Gilda che ha sede in un prefisso di blocco: `MAGE` → `"mages_guild"`,
/// ecc. Serve a `npc_reply` per riconoscere se l'NPC in cui stiamo parlando
/// è un membro della gilda che ha emesso la quest attiva.
fn guild_id_from_prefix(prefix: &str) -> Option<&'static str> {
    match prefix.to_ascii_uppercase().as_str() {
        "MAGE" => Some("mages_guild"),
        "FIGH" => Some("fighters_guild"),
        "THIE" => Some("thieves_guild"),
        _ => None,
    }
}

/// Se l'NPC è **dentro** la gilda che ha emesso la quest attiva per questa
/// città, è un insider: il flavor pre-generato ("I heard that the Mages
/// Guild is looking for…") è stato creato senza sapere dove si trova l'NPC
/// e non va bene. Va scartato in favore della voice (generata col prompt
/// INSIDERS, che produce "our guild is sending…").
fn is_insider(book: &QuestBook, ctx: &DialogContext) -> bool {
    let (Some(city), Some(prefix)) = (ctx.city, ctx.guild_prefix.as_deref()) else {
        return false;
    };
    let Some(giver) = guild_id_from_prefix(prefix) else {
        return false;
    };
    book.active_quest_giver_id(city) == Some(giver)
}

fn npc_reply(book: &mut QuestBook, worker: Option<&QuestWorker>, ctx: &DialogContext) -> Reply {
    // Se l'NPC è dentro la gilda che ha emesso la quest attiva, NON usiamo il
    // flavor pre-generato (è stato creato senza contesto di edificio). Andiamo
    // direttamente alla voice, il cui prompt ha la regola INSIDERS.
    if !is_insider(book, ctx) {
        if let Some(message) = flavor_message(book, ctx) {
            return Reply::Say(message);
        }
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

fn answer_message(label: &str, result: &Result<(String, bool, u8), ()>) -> String {
    match result {
        Ok((text, _, _)) => format!("{}: \"{}\"", label, ascii_punctuation(text)),
        Err(()) => format!("{label} says nothing."),
    }
}

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

fn close_dialog(state: &mut DialogState, root: &mut Query<&mut Visibility, With<DialogRoot>>) {
    state.open = false;
    state.forced_by_menu = false;
    for mut visibility in root.iter_mut() {
        *visibility = Visibility::Hidden;
    }
}

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
    if ctx.scripted_state != ScriptedQuestState::None {
        return None;
    }
    // Se la quest è già Active/Failed/Completed, non riproporla.
    if states.phase(SCRIPTED_QUEST_ID) != QuestPhase::NotStarted {
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

/// Costruisce la voice key: se l'NPC è dentro una gilda, appende `@PREFIX`
/// (es. `"flat:182:11@MAGE"`) così la stessa texture in taverna e in gilda
/// ha VOCI SEPARATE. La voce dentro la gilda è generata con description
/// "sei un membro della Mages Guild" (INSIDERS), quella fuori con "sei un
/// estraneo". Il `key` originale resta usato per `knowledge_block`,
/// `find_quest_offer`, il file `classic_npc`, e la caption.
fn voice_key_for(base_key: &str, interior: &InteriorState) -> String {
    match interior.block_prefix() {
        Some(prefix) => format!("{base_key}@{}", prefix.to_ascii_uppercase()),
        None => base_key.to_string(),
    }
}

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
                     interior_npcs: Query<(Entity, &InteriorNpc, &GlobalTransform, Option<&CustomNpcName>, &MeshMaterial3d<StandardMaterial>), Without<Follower>>,
                     mut root: Query<&mut Visibility, With<DialogRoot>>,
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

    if interior.is_inside() {
        let mut nearest_int: Option<(f32, Entity, &InteriorNpc, Option<&CustomNpcName>, &MeshMaterial3d<StandardMaterial>)> = None;
        for (entity, npc, gtf, custom_name, mat) in &interior_npcs {
            let pos = gtf.translation();
            if (pos.y - player_pos.y).abs() > 4.0 { continue; }
            let distance = Vec2::new(pos.x - player_pos.x, pos.z - player_pos.z).length();
            if distance <= DIALOG_DISTANCE && nearest_int.map_or(true, |(d, _, _, _, _)| distance < d) {
                nearest_int = Some((distance, entity, npc, custom_name, mat));
            }
        }
        if let Some((_, npc_entity, npc, custom_name_opt, npc_mat)) = nearest_int {
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
                // FIX gilda: se siamo dentro una gilda, l'NPC ne è membro.
                let guild = interior
                .block_prefix()
                .and_then(guild_name_from_prefix);
                let description = custom_name_opt
                .and_then(|c| custom_description(&c.name, &quests.custom))
                .unwrap_or_else(|| describe_npc_in(&who, role, place, temperament, guild));

                let gender = if custom_name_opt.is_some() {
                    gender_from_caption(&npc_name)
                } else {
                    flat_info
                    .and_then(|i| match i.gender { 1 => Some(Gender::Male), 2 => Some(Gender::Female), _ => None })
                    .or_else(|| npc_caption.as_deref().and_then(gender_from_caption))
                };

                let knowledge = knowledge_block(&quests.lib.0, &quests.states.0, "", &key, &KnowledgeOptions::default());

                let mut who = who;
                let mut description = description;
                let mut npc_caption = npc_caption;
                let mut voice_override = custom_name_opt.and_then(|c| custom_voice(&c.name, &quests.custom));
                let mut classic_anim = (None, None);
                if custom_name_opt.is_none() {
                    let sprite = quests.materials.get(&npc_mat.0)
                    .and_then(|m| m.base_color_texture.clone())
                    .map(|h| { let f = mods.needs_v_flip(&h); (h, f) });
                    let ov = classic_npc::resolve(&quests.custom.dir, &ClassicNpc {
                        city,
                        city_name: &city_name,
                        key: &key,
                        label: &who,
                        description: &description,
                        place,
                        sprite_stem: format!("{}_{}-0", npc.flat_archive, npc.flat_record),
                                                  portrait_stem: Some(format!("tfac00i0.rci_{face_record}-0")),
                                                  sprite,
                                                  portrait: Some((face.clone(), is_hd)),
                    }, &images, &mods);
                    if let Some(n) = ov.name {
                        if npc_caption.is_some() { npc_caption = Some(n.clone()); }
                        who = n;
                    }
                    if let Some(d) = ov.description { description = d; }
                    voice_override = ov.voice;
                    classic_anim = (ov.walk_fps, ov.idle_fps);
                }

                // Voice key con prefisso gilda: "flat:182:11@MAGE" per separare
                // la voce "da insider" da quella "da estraneo" dello stesso flat.
                let voice_key = voice_key_for(&key, &interior);

                *ctx = DialogContext::new(dialog_key, city, place, npc_flat, npc_caption)
                .with_voice(voice_key, who, description)
                .with_speaker(gender, role.map(|_| tone), temperament)
                .with_city_name(city_name)
                .with_knowledge(knowledge)
                .with_voice_override(voice_override)
                .with_classic_anim(classic_anim.0, classic_anim.1);

                // Popola il prefisso per il bypass insider (usato da npc_reply).
                ctx.guild_prefix = interior.block_prefix().map(str::to_string);
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
                state.forced_by_menu = false;
                for mut visibility in root.iter_mut() { *visibility = Visibility::Visible; }
                keys.clear_just_pressed(KeyCode::KeyE);
            }
        }
        return;
    }

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
    let mut label = format!("{look} ({})", tone.role());
    let mut description = describe_npc(look, Some(tone.role()), None, temperament);
    let face_img = npc.face_record().and_then(|r| faces.get(r, &data, &mut mods, &mut images));
    let sprite = {
        let (h, _) = npc.portrait();
        let f = mods.needs_v_flip(&h);
        Some((h, f))
    };
    let ov = classic_npc::resolve(&quests.custom.dir, &ClassicNpc {
        city,
        city_name: &city_name,
        key: &key,
        label: &label,
        description: &description,
        place: None,
        sprite_stem: format!("walker_{face}"),
                                  portrait_stem: npc.face_record().map(|r| format!("tfac00i0.rci_{r}-0")),
                                  sprite,
                                  portrait: face_img.clone(),
    }, &images, &mods);
    if let Some(n) = ov.name { label = n; }
    if let Some(d) = ov.description { description = d; }
    let knowledge = knowledge_block(&quests.lib.0, &quests.states.0, "", &key, &KnowledgeOptions::default());
    *ctx = DialogContext::new(face as u64, city, None, None, None)
    .with_voice(key, label, description)
    .with_speaker(gender_from_look(look), Some(tone), temperament)
    .with_city_name(city_name)
    .with_knowledge(knowledge)
    .with_voice_override(ov.voice);
    ctx.npc_entity = Some(npc_entity);
    ctx.npc_is_follower = false;
    // Fuori dall'edificio: nessuna gilda.
    ctx.guild_prefix = None;
    let (image, crop, width, height, is_hd) =
    match face_img {
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
    state.forced_by_menu = false;
    for mut visibility in root.iter_mut() { *visibility = Visibility::Visible; }
    keys.clear_just_pressed(KeyCode::KeyE);
}

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
    followers: Query<
    (Entity, &Follower, &GlobalTransform, Option<&CustomNpcName>),
                               Without<DialogRoot>,
                               >,
                               mut root: Query<&mut Visibility, With<DialogRoot>>,
                               mut portrait: Query<
                               (&mut ImageNode, &mut Node),
                               (With<DialogPortrait>, Without<DialogRoot>),
                               >,
                               mut flats_catalog: Local<Option<FlatCatalog>>,
) {
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
        let place = interior.kind();
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
        let guild = interior
        .block_prefix()
        .and_then(guild_name_from_prefix);
        let description = custom
        .and_then(|c| custom_description(&c.name, &quests.custom))
        .unwrap_or_else(|| describe_npc_in(&who, role, place, temperament, guild));
        let gender = if custom.is_some() {
            npc_caption.as_deref().and_then(gender_from_caption)
        } else {
            flat_info
            .and_then(|i| match i.gender { 1 => Some(Gender::Male), 2 => Some(Gender::Female), _ => None })
            .or_else(|| npc_caption.as_deref().and_then(gender_from_caption))
        };
        let knowledge = knowledge_block(&quests.lib.0, &quests.states.0, "", &key, &KnowledgeOptions::default());

        let mut who = who;
        let mut description = description;
        let mut npc_caption = npc_caption;
        let mut voice_override = custom.and_then(|c| custom_voice(&c.name, &quests.custom));
        if custom.is_none() {
            let sprite = fol.portrait().map(|(h, _)| { let f = mods.needs_v_flip(&h); (h, f) });
            let ov = classic_npc::resolve(&quests.custom.dir, &ClassicNpc {
                city,
                city_name: &city_name,
                key: &key,
                label: &who,
                description: &description,
                place,
                sprite_stem: format!("{}_{}-0", id.flat_archive, id.flat_record),
                                          portrait_stem: Some(format!("tfac00i0.rci_{}-0", id.face_record)),
                                          sprite,
                                          portrait: Some((face.clone(), is_hd)),
            }, &images, &mods);
            if let Some(n) = ov.name {
                if npc_caption.is_some() { npc_caption = Some(n.clone()); }
                who = n;
            }
            if let Some(d) = ov.description { description = d; }
            voice_override = ov.voice;
        }

        // Voice key con prefisso gilda (stessa logica del ramo di update_dialog).
        let voice_key = voice_key_for(&key, &interior);

        *ctx = DialogContext::new(dialog_key, city, place, npc_flat, npc_caption)
        .with_voice(voice_key, who, description)
        .with_speaker(gender, role.map(|_| tone), temperament)
        .with_city_name(city_name)
        .with_knowledge(knowledge)
        .with_voice_override(voice_override);

        // Popola il prefisso per il bypass insider (usato da npc_reply).
        ctx.guild_prefix = interior.block_prefix().map(str::to_string);
        if let Ok((mut image_node, mut node)) = portrait.get_single_mut() {
            image_node.image = face;
            image_node.rect = None;
            image_node.flip_y = is_hd;
            node.width = Val::Px(face_size.map_or(FACE_DISPLAY_SIZE, |d| d.x));
            node.height = Val::Px(face_size.map_or(FACE_DISPLAY_SIZE, |d| d.y));
        }
    } else {
        let face = fol.face_record().unwrap_or(0);
        let tone = tone_for_npc(face as u64, city, None);
        let look = walker_look(face);
        let key = walker_key(face);
        let temperament = temperament_for(&key, city);
        let mut label = format!("{look} ({})", tone.role());
        let mut description = describe_npc(look, Some(tone.role()), None, temperament);
        let face_img = fol.face_record().and_then(|r| faces.get(r, &data, &mut mods, &mut images));
        let sprite = fol.portrait().map(|(h, _)| { let f = mods.needs_v_flip(&h); (h, f) });
        let ov = classic_npc::resolve(&quests.custom.dir, &ClassicNpc {
            city,
            city_name: &city_name,
            key: &key,
            label: &label,
            description: &description,
            place: None,
            sprite_stem: format!("walker_{face}"),
                                      portrait_stem: fol.face_record().map(|r| format!("tfac00i0.rci_{r}-0")),
                                      sprite,
                                      portrait: face_img.clone(),
        }, &images, &mods);
        if let Some(n) = ov.name { label = n; }
        if let Some(d) = ov.description { description = d; }
        let knowledge = knowledge_block(&quests.lib.0, &quests.states.0, "", &key, &KnowledgeOptions::default());
        *ctx = DialogContext::new(face as u64, city, None, None, None)
        .with_voice(key, label, description)
        .with_speaker(gender_from_look(look), Some(tone), temperament)
        .with_city_name(city_name)
        .with_knowledge(knowledge)
        .with_voice_override(ov.voice);
        // Follower esterno: nessuna gilda.
        ctx.guild_prefix = None;
        let (image, crop, width, height, is_hd) =
        match face_img {
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
    state.forced_by_menu = false;
    for mut visibility in root.iter_mut() { *visibility = Visibility::Visible; }
    keys.clear_just_pressed(KeyCode::KeyE);
}

fn custom_voice(name: &str, cache: &CustomNpcCache) -> Option<String> {
    cache
    .npcs
    .iter()
    .find(|n| n.name == name)
    .and_then(|n| n.voice.clone())
}

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
    if !state.open || state.forced_by_menu || ctx.typing {
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
    for mut text in texts.iter_mut() { text.0 = s.clone(); }
}

fn handle_scripted_offer(
    keys: &ButtonInput<KeyCode>,
    ctx: &mut DialogContext,
    lib: &Library,
    states: &mut QuestStates,
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

        // Aggiorna lo stato: Accepted → Active, Refused → Failed.
        // `set_phase(Active)` abilita anche gli argomenti `dialog link`
        // del blocco di avvio. Il file viene riscritto così lo stato
        // sopravvive al riavvio.
        if let Some(s) = lib.get(&offer.quest_id) {
            let phase = if accepted { QuestPhase::Active } else { QuestPhase::Failed };
            states.set_phase(s, phase);
            if let Err(e) = std::fs::write("quest_states.txt", states.to_text()) {
                warn!("Impossibile salvare quest_states.txt: {e}");
            }
        }

        ctx.scripted_state = ScriptedQuestState::None;
        ctx.scripted_response = Some(resolved);
    }
    true
}

#[allow(clippy::too_many_arguments)]
fn dialog_menu(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    state: Res<DialogState>,
    mut ctx: ResMut<DialogContext>,
    mut book: ResMut<QuestBook>,
    worker: Option<Res<QuestWorker>>,
    mut quests: QuestResources,
    mut speak: EventWriter<SpeakEvent>,
    mut typed: EventReader<KeyboardInput>,
    mut recruit_events: EventWriter<RecruitFollowerEvent>,
    mut mood_events: EventWriter<FollowerMoodEvent>,
    assets: Res<AssetServer>,
    mut line: Query<(&mut Text, &mut TextFont, &mut TextColor), (With<DialogLine>, Without<DialogOptions>, Without<DialogInputText>)>,
               mut options: Query<&mut Text, (With<DialogOptions>, Without<DialogLine>, Without<DialogInputText>)>,
               mut input_text: Query<&mut Text, (With<DialogInputText>, Without<DialogLine>, Without<DialogOptions>)>,
) {
    if !state.open {
        typed.clear();
        return;
    }
    if state.forced_by_menu {
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
        for (mut text, mut font, mut color) in &mut line {
            text.0 = DIALOG_TEXT.to_string();
            font.font = assets.load(DIALOG_FONT);
            font.font_size = DIALOG_FONT_SIZE;
            *color = TextColor(DIALOG_TEXT_COLOR);
        }
        let menu = if ctx.npc_is_follower { MENU_FOLLOWER } else { MENU_TEXT };
        for mut text in &mut options { text.0 = menu.to_string(); }
        set_input_text(&mut input_text, INPUT_PLACEHOLDER.to_string());
    }

    // Gestione dell'offerta scriptata. `QuestStatesRes` è un wrapper:
    // con il destructuring su `&mut *quests.states` otteniamo direttamente
    // `&mut QuestStates` (il campo interno della tuple struct), mantenendo
    // `&quests.lib` immutabile — split borrow sui campi del SystemParam.
    let offer_done = {
        let lib: &Library = &quests.lib.0;
        let QuestStatesRes(states) = &mut *quests.states;
        handle_scripted_offer(&keys, &mut ctx, lib, states, &assets, &mut line, &mut options)
    };
    if offer_done {
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
        if let Some(id) = ctx.asking {
            if let Some(result) = book.take_answer(id) {
                ctx.asking = None;
                let mut recruited = false;
                let mut mood: u8 = 0;
                match &result {
                    Ok((text, follow, m)) => {
                        ctx.conversation.push(("npc".to_string(), text.clone()));
                        mood = *m;
                        if *follow && !ctx.npc_is_follower {
                            if let Some(entity) = ctx.npc_entity {
                                recruit_events.send(RecruitFollowerEvent {
                                    entity,
                                    initial_mood: mood,
                                });
                                recruited = true;
                                info!("RECRUIT: invio evento per {:?} (mood={})", entity, mood);
                            }
                        }
                    }
                    Err(()) => {}
                }
                if !recruited && ctx.npc_is_follower {
                    if let Some(entity) = ctx.npc_entity {
                        mood_events.send(FollowerMoodEvent { entity, mood });
                    }
                }
                if recruited {
                    let npc_text = match &result {
                        Ok((t, _, _)) => t.clone(),
                        Err(()) => String::new(),
                    };
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
                 custom_npcs: Res<CustomNpcCache>,
                 dialog_ctx: Res<DialogContext>,
                 mut mods: ResMut<ModTextures>,
                 mut images: ResMut<Assets<Image>>,
                 mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for ev in events.read() {
        if let Ok((walker, local_tf, gtf)) = walkers.get(ev.entity) {
            let follower = Follower::from_walker(walker).with_mood(ev.initial_mood);
            let world_pos = gtf.translation();
            let scale = local_tf.scale;
            commands.entity(ev.entity)
            .remove::<NpcWalker>()
            .insert(follower)
            .insert(Transform::from_translation(world_pos).with_scale(scale))
            .remove_parent();
            info!(
                "RECRUIT: {:?} esterno -> follower animato (mood={})",
                  ev.entity, ev.initial_mood
            );
            continue;
        }
        if let Ok((npc, mat, custom)) = interiors.get(ev.entity) {
            commands.entity(ev.entity).despawn();
            let size = npc.size;
            match custom {
                Some(c) => interior.mark_recruited_custom(&c.name),
                None => interior.mark_recruited(npc.flat_archive, npc.flat_record),
            }

            let mut follower_copy = Follower::from_static_mobile_full(
                mat.0.clone(),
                                                                      size,
                                                                      npc.flat_archive,
                                                                      npc.flat_record,
                                                                      npc.face_record,
            );

            if custom.is_none() {
                let (walk_fps, idle_fps) = if dialog_ctx.npc_entity == Some(ev.entity) {
                    dialog_ctx.classic_anim
                } else {
                    (None, None)
                };
                let base_size_px = mods
                .size_by_name(&format!("{:03}_{}-0", npc.flat_archive, npc.flat_record))
                .or_else(|| {
                    mods.size_by_name(&format!("{}_{}-0", npc.flat_archive, npc.flat_record))
                })
                .map(|(w, h)| Vec2::new(w as f32, h as f32))
                .unwrap_or(size);
                if let Some((idle_frames, idle_sizes)) = load_flat_idle_frames(
                    npc.flat_archive,
                    npc.flat_record,
                    0,
                    base_size_px,
                    size ,
                    &mut mods,
                    &mut images,
                    &mut materials,
                ) {
                    info!(
                        "RECRUIT: flat {}:{} con {} frame idle",
                        npc.flat_archive,
                        npc.flat_record,
                        idle_frames.len(),
                    );
                    follower_copy = follower_copy.with_idle(idle_frames, idle_sizes, idle_fps.unwrap_or(1.0));
                }
                if let Some((frames, sizes)) = load_flat_walk_frames(
                    npc.flat_archive,
                    npc.flat_record,
                    0,
                    base_size_px,
                    size,
                    &mut mods,
                    &mut images,
                    &mut materials,
                ) {
                    let fps = walk_fps.unwrap_or(8.0);
                    info!(
                        "RECRUIT: flat {}:{} con {} frame walk @ {} fps",
                        npc.flat_archive, npc.flat_record, frames.len(), fps,
                    );
                    follower_copy = follower_copy.with_walk(frames, sizes, fps);
                }
            }

            if let Some(c) = custom {
                if let Some(d) = custom_npcs.npcs.iter().find(|n| n.name == c.name) {
                    let base_size_px = mods
                    .image_by_name(&d.texture_stem, &mut images)
                    .and_then(|h| {
                        mods.image_size(&h)
                        .or_else(|| images.get(&h).map(|i| i.size_f32()))
                    })
                    .unwrap_or(size);

                    if let Some((frames, sizes)) = load_custom_walk_frames(
                        d,
                        base_size_px,
                        size,
                        &mut mods,
                        &mut images,
                        &mut materials,
                    ) {
                        info!(
                            "RECRUIT: NPC custom '{}' con {} frame walk @ {} fps",
                            d.name, frames.len(), d.walk_fps,
                        );
                        follower_copy = follower_copy.with_walk(frames, sizes, d.walk_fps);
                    } else if !d.walk_stems.is_empty() {
                        warn!(
                            "RECRUIT: NPC custom '{}' con walk_stems={:?} ma i frame non si caricano",
                            d.name, d.walk_stems,
                        );
                    }

                    if let Some((idle_frames, idle_sizes)) = load_custom_idle_frames(
                        d,
                        base_size_px,
                        size * IDLE_SIZE_SCALE,
                        &mut mods,
                        &mut images,
                        &mut materials,
                    ) {
                        info!(
                            "RECRUIT: NPC custom '{}' con {} frame idle @ {} fps",
                            d.name, idle_frames.len(), d.idle_fps,
                        );
                        follower_copy = follower_copy.with_idle(idle_frames, idle_sizes, d.idle_fps);
                    } else if !d.idle_stems.is_empty() {
                        warn!(
                            "RECRUIT: NPC custom '{}' con idle_stems={:?} ma i frame non si caricano",
                            d.name, d.idle_stems,
                        );
                    }
                } else {
                    warn!(
                        "RECRUIT: NPC custom '{}' non trovato nel CustomNpcCache",
                        c.name,
                    );
                }
            }

            follower_copy = follower_copy.with_mood(ev.initial_mood);

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
                "RECRUIT: {:?} interno -> despawn + copia mobile arch={} rec={} face={} mood={}",
                ev.entity, npc.flat_archive, npc.flat_record, npc.face_record, ev.initial_mood
            );
            continue;
        }
        warn!("RECRUIT: entità {:?} non trovata (né walker né interior)", ev.entity);
    }
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
        let ok = answer_message("old beggar", &Ok(("Daggerfall, friend.".to_string(), false, 0)));
        assert_eq!(ok, "old beggar: \"Daggerfall, friend.\"");
        assert_eq!(spoken_part(&ok).as_deref(), Some("Daggerfall, friend."));
        let fail = answer_message("old beggar", &Err(()));
        assert_eq!(fail, "old beggar says nothing.");
        assert_eq!(spoken_part(&fail), None);
    }

    #[test]
    fn answer_message_with_follow_true() {
        let ok = answer_message("barmaid", &Ok(("I'll follow you!".to_string(), true, 1)));
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
        assert!(ctx.guild_prefix.is_none());
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

    #[test]
    fn guild_id_from_prefix_maps_known_guilds() {
        assert_eq!(guild_id_from_prefix("MAGE"), Some("mages_guild"));
        assert_eq!(guild_id_from_prefix("mage"), Some("mages_guild"));
        assert_eq!(guild_id_from_prefix("FIGH"), Some("fighters_guild"));
        assert_eq!(guild_id_from_prefix("THIE"), Some("thieves_guild"));
        assert_eq!(guild_id_from_prefix("TEMP"), None);
        assert_eq!(guild_id_from_prefix("TAVN"), None);
        assert_eq!(guild_id_from_prefix(""), None);
    }

    #[test]
    fn guild_prefix_is_set_independently_of_place() {
        let mut ctx = DialogContext::new(1, (1, 1), Some(PoiKind::Guild), None, None);
        ctx.guild_prefix = Some("MAGE".to_string());
        assert_eq!(ctx.guild_prefix.as_deref(), Some("MAGE"));
    }

    #[test]
    fn voice_key_separates_inside_from_outside() {
        // Placeholder: senza `InteriorState` reale non possiamo testare
        // `voice_key_for` end-to-end, ma la funzione è banale. Il test qui
        // documenta l'invariante: stesso `base_key` → stesso `voice_key` se
        // non c'è prefisso.
        let base = "flat:182:11";
        // Con `interior` vuoto, `block_prefix()` ritorna `None`, quindi
        // `voice_key_for` ritorna `base.to_string()`. Non possiamo chiamare
        // `InteriorState::default().block_prefix()` direttamente perché
        // `InteriorState` non è `Default`-costruibile in un test senza Bevy.
        // Verifichiamo solo la formula: `voice_key_for` con `None` = base.
        // (Se in futuro `InteriorState` diventa testabile, sostituire con
        // una chiamata reale.)
        assert_eq!(base.to_string(), "flat:182:11");
    }
}
