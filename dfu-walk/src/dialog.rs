//! Dialogo con gli NPC: avvicinati e premi E per vedere il ritratto dell'NPC su
//! sfondo nero semitrasparente con la scritta gialla. E o Esc chiudono.
//!
//! Funziona sia con gli NPC all'esterno che con quelli all'interno delle taverne/negozi.
//! Il ritratto cerca prima la versione HD nella mod (es. `DREAM - Portraits`),
//! altrimenti ripiega sui volti 64x64 di `TFAC00I0.RCI` (ARENA2) o sullo sprite.
//!
//! Il dialogo ha un piccolo menu: con `1` si ascolta una voce (rumor) sulla quest generata dal
//! LLM per la citta' (vedi `quests.rs`). Se l'NPC ha una voce flavor dedicata (generata dalla
//! sua descrizione esatta in FLATS.CFG), quella ha la precedenza.
//!
//! Per tutti gli altri PNG la voce si genera su richiesta: ogni PNG ha una descrizione (la
//! caption di FLATS.CFG per quelli interni, razza e sesso ricavati dal volto per i passanti) che
//! il LLM usa per far parlare proprio quella persona. Nell'attesa il dialogo mostra un messaggio e
//! si aggiorna da solo quando la voce e' pronta; se non si puo' generare si ripiega sul tono.
//! E o Esc chiudono.

use crate::data::city_map::PoiKind;
use crate::data::flats::FlatCatalog;
use crate::data::{find_file_ci, Palette};
use crate::interior::{InteriorNpc, InteriorState};
use crate::mod_textures::ModTextures;
use crate::player::Player;
use crate::quests::{
    describe_npc, face_key, flat_key, temperament_for, tone_for_npc, walker_key, walker_look, Hint, QuestBook,
    QuestWorker, Tone, VoiceState,
};
use crate::tiles::NpcWalker;
use crate::world::{map_pixel_of, GameData, Origin};
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

// Menu e voci
const DIALOG_TEXT_MAX_WIDTH: f32 = 760.0;
const MENU_TEXT: &str = "[1] Ascolta rumori        [E] Congeda";
const MENU_FONT_SIZE: f32 = 28.0;
const MENU_COLOR: Color = Color::srgb(0.85, 0.8, 0.55);
const RUMOR_FONT_SIZE: f32 = 30.0;

/// Font delle voci. Le voci sono in inglese: se vedi dei quadratini,
/// metti in `assets/fonts/` un font con le lettere accentate (per esempio DejaVuSans.ttf) e
/// cambia questo percorso.
const RUMOR_FONT: &str = "fonts/Sherwood.ttf";

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
                // si capovolge solo se l'immagine e' BC7 (PNG e DXT sono gia' nel verso giusto)
                let flip = mods.needs_v_flip(&handle);
                let entry = (handle, flip);
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

/// La riga di testo principale del dialogo (saluto o voce).
#[derive(Component)]
struct DialogLine;

/// La riga con le opzioni del menu.
#[derive(Component)]
struct DialogOptions;

/// Con chi si sta parlando: decide quale voce racconta.
#[derive(Resource, Default)]
pub struct DialogContext {
    tone: Tone,
    /// Map pixel della citta' in cui ci si trova.
    city: Option<(i32, i32)>,
    /// Flat (archive, record) dell'NPC interno, per cercare la voce flavor dedicata.
    npc_flat: Option<(u16, u8)>,
    /// Descrizione esatta FLATS.CFG dell'NPC (es. "beautiful maiden", "old beggar"),
    /// usata come etichetta nel dialogo.
    npc_caption: Option<String>,
    /// Il dialogo e' appena stato aperto: va rimesso il saluto.
    dirty: bool,
    /// Chi e' l'NPC per il LLM (chiave, etichetta, descrizione), per la voce su richiesta.
    voice: Option<NpcVoiceInfo>,
    /// Si sta aspettando la voce dal thread del LLM.
    awaiting: bool,
}

#[derive(Clone, Default)]
struct NpcVoiceInfo {
    /// Chiave stabile del PNG (per la cache e il file delle voci).
    key: String,
    /// Come lo si chiama nel dialogo ("beautiful maiden", "Nord woman (merchant)").
    label: String,
    /// Il blocco di descrizione che si passa al LLM.
    description: String,
}

impl DialogContext {
    fn with_voice(mut self, key: String, label: String, description: String) -> Self {
        self.voice = Some(NpcVoiceInfo { key, label, description });
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
        }
    }
}

/// Punteggiatura tipografica -> ASCII (i font dei giochi non hanno sempre ' " " « » …).
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

/// Voce flavor pre-generata in blocco per il flat dell'NPC (taverne e negozi), se c'e'.
fn flavor_message(book: &QuestBook, ctx: &DialogContext) -> Option<String> {
    let (city, (a, r)) = (ctx.city?, ctx.npc_flat?);
    let text = book.npc_flavor(city, a, r)?;
    let label = ctx.npc_caption.clone().unwrap_or_else(|| "NPC".to_string());
    Some(format!("{}: \"{}\"", label, ascii_punctuation(text)))
}

/// Cosa mostrare scegliendo "Ascolta rumori".
enum Reply {
    /// La risposta definitiva.
    Say(String),
    /// La voce e' in generazione: si mostra questo messaggio e si aspetta.
    Wait(String),
}

fn waiting_text(voice: &NpcVoiceInfo) -> String {
    format!("{} is thinking about it...", voice.label)
}

/// La risposta del PNG. Ordine: voce flavor in blocco, voce su richiesta (generandola se manca e
/// il LLM e' disponibile), e infine il ripiego di sempre (risposta neutra o voce per tono).
fn npc_reply(book: &mut QuestBook, worker: Option<&QuestWorker>, ctx: &DialogContext) -> Reply {
    if let Some(message) = flavor_message(book, ctx) {
        return Reply::Say(message);
    }
    if let (Some(city), Some(voice)) = (ctx.city, ctx.voice.as_ref()) {
        // lo stato si copia in un valore proprio: poi si puo' modificare l'archivio
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

/// Cosa dice il PNG quando si sceglie "Ascolta rumori" (senza generare nulla di nuovo).
/// Se l'NPC ha una voce flavor dedicata (da FLATS.CFG), quella ha la precedenza.
/// Altrimenti si ripiega sulla voce per tono.
fn rumor_message(book: &QuestBook, ctx: &DialogContext) -> String {
    // 1. Voce flavor dedicata: la caption esatta di FLATS.CFG è l'etichetta.
    if let Some(message) = flavor_message(book, ctx) {
        return message;
    }
    // 2. Nessun flavor per questo NPC interno: risposta neutra.
    if ctx.npc_flat.is_some() {
        return "They shrug. They don't seem to know anything about it.".to_string();
    }
    // 3. NPC esterno (nessun flat FLATS.CFG): ripiego sul tono.
    hint_message(book, ctx.city, ctx.tone)
}

/// Cosa dice il PNG in base al tono (fallback per NPC esterni).
fn hint_message(book: &QuestBook, city: Option<(i32, i32)>, tone: Tone) -> String {
    match book.hint(city, tone) {
        Hint::Text(t) => format!("{}: \"{}\"", tone.label(), ascii_punctuation(t)),
        Hint::Pending => "Something is being whispered in town, but nobody can tell you anything specific yet. Try again later.".to_string(),
        Hint::Nothing => "You don't hear anything interesting.".to_string(),
    }
}

#[derive(Component)]
pub(crate) struct DialogPortrait;

pub struct DialogPlugin;

impl Plugin for DialogPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DialogState>()
        .init_resource::<FacePortraits>()
        .init_resource::<DialogContext>()
        .add_systems(Startup, setup_dialog_ui)
        .add_systems(Update, dialog_menu.after(update_dialog))
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
        root.spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(28.0),
                   max_width: Val::Px(DIALOG_TEXT_MAX_WIDTH),
                   ..default()
        })
        .with_children(|column| {
            column.spawn((
                Text::new(DIALOG_TEXT),
                          TextFont { font: font.clone(), font_size: DIALOG_FONT_SIZE, ..default() },
                          TextColor(DIALOG_TEXT_COLOR),
                          DialogLine,
            ));
            column.spawn((
                Text::new(MENU_TEXT),
                          TextFont { font, font_size: MENU_FONT_SIZE, ..default() },
                          TextColor(MENU_COLOR),
                          DialogOptions,
            ));
        });
    });
}

fn close_dialog(state: &mut DialogState, root: &mut Query<&mut Visibility, With<DialogRoot>>) {
    state.open = false;
    for mut visibility in root.iter_mut() {
        *visibility = Visibility::Hidden;
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
    mut ctx: ResMut<DialogContext>,
    player: Query<&Transform, With<Player>>,
    exterior_npcs: Query<(&NpcWalker, &GlobalTransform)>,
                     interior_npcs: Query<(&InteriorNpc, &GlobalTransform)>,
                     mut root: Query<&mut Visibility, With<DialogRoot>>,
                     mut portrait: Query<(&mut ImageNode, &mut Node), With<DialogPortrait>>,
                     mut flats_catalog: Local<Option<FlatCatalog>>,
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
                // Cattura il flat dell'NPC per cercare la voce flavor dedicata
                let npc_flat = Some((npc.flat_archive, npc.flat_record));
                let catalog = flats_catalog.get_or_insert_with(|| FlatCatalog::load(&data.dir));
                let npc_caption = catalog
                .get(npc.flat_archive, npc.flat_record)
                .map(|i| i.caption.clone());

                let city = map_pixel_of(&origin, player_pos);
                let place = interior.kind();
                let tone = tone_for_npc(npc.face_record as u64, city, place);
                // con la caption di FLATS.CFG il PNG e' descritto da quella; altrimenti dal volto
                let (key, who, role) = match &npc_caption {
                    Some(caption) => (flat_key(npc.flat_archive, npc.flat_record), caption.clone(), None),
                    None => (face_key(npc.face_record), "an ordinary townsperson".to_string(), Some(tone.role())),
                };
                let description = describe_npc(&who, role, place, temperament_for(&key, city));
                *ctx = DialogContext::new(npc.face_record as u64, city, place, npc_flat, npc_caption)
                    .with_voice(key, who, description);
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

    let city = map_pixel_of(&origin, player_pos);
    let face = npc.face_record().unwrap_or(0);
    let tone = tone_for_npc(face as u64, city, None);
    // i passanti non hanno una riga in FLATS.CFG: razza e sesso si ricavano dal volto
    let look = walker_look(face);
    let key = walker_key(face);
    let description = describe_npc(look, Some(tone.role()), None, temperament_for(&key, city));
    *ctx = DialogContext::new(face as u64, city, None, None, None).with_voice(
        key,
        format!("{look} ({})", tone.role()),
        description,
    );
    let (image, crop, width, height, is_hd) =
    match npc.face_record().and_then(|r| faces.get(r, &data, &mut mods, &mut images)) {
        Some((face, is_hd)) => (face, None, FACE_DISPLAY_SIZE, FACE_DISPLAY_SIZE, is_hd),
        None => {
            let (sprite, size) = npc.portrait();
            // Gli sprite HD BC7 sono memorizzati con la riga 0 in basso: per avere la parte alta
            // (testa e busto) si ritagliano le ultime righe e poi si capovolge la V.
            let flip = mods.needs_v_flip(&sprite);
            let crop_h = (size.y * PORTRAIT_CROP).max(1.0);
            let crop = if flip {
                Rect::new(0.0, size.y - crop_h, size.x, size.y)
            } else {
                Rect::new(0.0, 0.0, size.x, crop_h)
            };
            (
                sprite,
             Some(crop),
             PORTRAIT_WIDTH,
             PORTRAIT_WIDTH * crop.height() / crop.width().max(1.0),
             flip,
            )
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

/// Menu del dialogo aperto: rimette il saluto all'apertura; con `1` mostra la voce del PNG e, se
/// va generata, aspetta il thread del LLM senza bloccare il gioco.
#[allow(clippy::too_many_arguments)]
fn dialog_menu(
    keys: Res<ButtonInput<KeyCode>>,
    state: Res<DialogState>,
    mut ctx: ResMut<DialogContext>,
    mut book: ResMut<QuestBook>,
    worker: Option<Res<QuestWorker>>,
    assets: Res<AssetServer>,
    mut line: Query<(&mut Text, &mut TextFont), (With<DialogLine>, Without<DialogOptions>)>,
    mut options: Query<&mut Text, (With<DialogOptions>, Without<DialogLine>)>,
) {
    if !state.open {
        return;
    }
    if ctx.dirty {
        ctx.dirty = false;
        ctx.awaiting = false;
        for (mut text, mut font) in &mut line {
            text.0 = DIALOG_TEXT.to_string();
            font.font = assets.load(DIALOG_FONT);
            font.font_size = DIALOG_FONT_SIZE;
        }
        for mut text in &mut options {
            text.0 = MENU_TEXT.to_string();
        }
    }
    let mut shown: Option<String> = None;
    if keys.just_pressed(KeyCode::Digit1) || keys.just_pressed(KeyCode::Numpad1) {
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
        // la voce arriva dal thread: appena e' pronta (o fallita) si mostra
        if let Reply::Say(message) = npc_reply(&mut book, worker.as_deref(), &ctx) {
            ctx.awaiting = false;
            shown = Some(message);
        }
    }
    if let Some(message) = shown {
        for (mut text, mut font) in &mut line {
            text.0 = message.clone();
            font.font = assets.load(RUMOR_FONT);
            font.font_size = RUMOR_FONT_SIZE;
        }
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
        assert_eq!(&rgba[4..8], &[0, 1, 2, 255]);
        assert!(rgba.chunks(4).all(|p| p[3] == 255));
    }

    #[test]
    fn typographic_punctuation_becomes_ascii() {
        assert_eq!(
            ascii_punctuation("L\u{2019}oste dice: \u{00AB}vieni\u{2026}\u{00BB} \u{2013} o no?"),
                   "L'oste dice: \"vieni...\" - o no?"
        );
        assert_eq!(ascii_punctuation("\u{00E8} gi\u{00E0} cos\u{00EC}"), "\u{00E8} gi\u{00E0} cos\u{00EC}"); // gli accenti restano
    }

    #[test]
    fn hint_message_covers_every_state() {
        let book = QuestBook::default();
        assert_eq!(hint_message(&book, None, Tone::Noble), "You don't hear anything interesting.");
        assert_eq!(hint_message(&book, Some((1, 1)), Tone::Noble), "You don't hear anything interesting.");
    }

    #[test]
    fn rumor_message_falls_back_to_neutral_for_interior_npc_without_flavor() {
        let book = QuestBook::default();
        let ctx = DialogContext {
            tone: Tone::Merchant,
            city: Some((1, 1)),
            npc_flat: Some((183, 2)),
            npc_caption: Some("guard".into()),
            dirty: false,
            ..Default::default()
        };
        // Nessun flavor salvato: risposta neutra per NPC interno
        let msg = rumor_message(&book, &ctx);
        assert_eq!(msg, "They shrug. They don't seem to know anything about it.");
    }

    #[test]
    fn rumor_message_without_flat_uses_tone() {
        let book = QuestBook::default();
        let ctx = DialogContext {
            tone: Tone::Commoner,
            city: Some((1, 1)),
            npc_flat: None,
            npc_caption: None,
            dirty: false,
            ..Default::default()
        };
        let msg = rumor_message(&book, &ctx);
        assert_eq!(msg, "You don't hear anything interesting.");
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

    fn voice_ctx(city: Option<(i32, i32)>) -> DialogContext {
        DialogContext {
            tone: Tone::Merchant,
            city,
            ..Default::default()
        }
        .with_voice("walker:37".into(), "Nord woman (merchant)".into(), "- Appearance: Nord woman".into())
    }

    #[test]
    fn reply_uses_a_stored_voice_with_the_npc_label() {
        let mut book = QuestBook::default();
        book.store_voice((1, 1), "walker:37".into(), "Hush \u{2019}now\u{2019}.".into());
        match npc_reply(&mut book, None, &voice_ctx(Some((1, 1)))) {
            Reply::Say(m) => assert_eq!(m, "Nord woman (merchant): \"Hush 'now'.\""),
            Reply::Wait(_) => panic!("la voce e' pronta"),
        }
    }

    #[test]
    fn reply_waits_while_the_voice_is_being_generated() {
        let mut book = QuestBook::default();
        book.mark_voice_pending((1, 1), "walker:37");
        let ctx = voice_ctx(Some((1, 1)));
        match npc_reply(&mut book, None, &ctx) {
            Reply::Wait(m) => assert_eq!(m, "Nord woman (merchant) is thinking about it..."),
            Reply::Say(_) => panic!("deve aspettare"),
        }
        // arrivata la voce: la stessa domanda ora riceve la risposta
        book.store_voice((1, 1), "walker:37".into(), "Psst.".into());
        assert!(matches!(npc_reply(&mut book, None, &ctx), Reply::Say(m) if m.ends_with("\"Psst.\"")));
    }

    #[test]
    fn reply_without_llm_and_without_voice_uses_the_tone_fallback() {
        let mut book = QuestBook::default();
        match npc_reply(&mut book, None, &voice_ctx(Some((2, 2)))) {
            Reply::Say(m) => assert_eq!(m, "You don't hear anything interesting."),
            Reply::Wait(_) => panic!("senza LLM non si aspetta"),
        }
    }

    #[test]
    fn reply_falls_back_when_generation_failed() {
        let mut book = QuestBook::default();
        book.fail_voice((1, 1), "walker:37".into());
        match npc_reply(&mut book, None, &voice_ctx(Some((1, 1)))) {
            Reply::Say(m) => assert_eq!(m, "You don't hear anything interesting."),
            Reply::Wait(_) => panic!("fallita: niente attesa"),
        }
    }

    #[test]
    fn batch_flavor_still_has_priority_over_on_demand_voices() {
        // un NPC interno senza flavor ne' voce ne' LLM: risposta neutra come prima
        let mut book = QuestBook::default();
        let ctx = DialogContext {
            tone: Tone::Merchant,
            city: Some((1, 1)),
            npc_flat: Some((183, 2)),
            npc_caption: Some("guard".into()),
            ..Default::default()
        };
        match npc_reply(&mut book, None, &ctx) {
            Reply::Say(m) => assert_eq!(m, "They shrug. They don't seem to know anything about it."),
            Reply::Wait(_) => panic!("senza LLM non si aspetta"),
        }
    }

    #[test]
    fn waiting_text_names_the_npc() {
        let v = NpcVoiceInfo { key: "k".into(), label: "old beggar".into(), description: String::new() };
        assert_eq!(waiting_text(&v), "old beggar is thinking about it...");
    }
}
