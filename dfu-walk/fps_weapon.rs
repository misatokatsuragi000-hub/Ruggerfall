//! Arma in prima persona (FPS Weapon).
//!
//! Integra le texture della mod "Diverse Weapons" con posizionamento ingrandito,
//! reset immediato alla posa di guardia a fine animazione e le 3 varianti di attacco:
//!   - Trascina a sinistra -> Fendente sinistro (Swing)
//!   - Trascina a destra -> Fendente destro, simmetrico (SwingRight)
//!   - Trascina verso il basso -> Colpo verticale dall'alto (VerticalHit)
//!   - Trascina verso l'alto (o clic destro) -> Affondo (Thrust), frame cif_6-0 ... cif_6-4
//!   - Clic rapido -> Combo ciclica fluida (Fendente -> Verticale -> Affondo)
//!   - A tasto sinistro ancora premuto si possono concatenare colpi DIVERSI senza rilasciarlo:
//!     basta un nuovo trascinamento in un'altra direzione (vedi `CHAIN_WINDOW_START` e
//!     `CHAIN_GESTURE_THRESHOLD`). Lo stesso colpo di seguito richiede di rilasciare il tasto.
//!
//! Regolazione rapida dimensione: tasti [ e ] o TN+ e TN-.

use crate::equipment::{EquipSlot, EquipmentState, InventoryItem, PlayerInventory};
use crate::mod_textures::ModTextures;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use std::collections::HashMap;

// =========================================================================
// PARAMETRI DI DIMENSIONE E POSIZIONAMENTO
// =========================================================================
// Scala di base (~92% dell'altezza della finestra)
const DEFAULT_WEAPON_HEIGHT_VH: f32 = 93.0;

// Posizionamento di riposo (ancorata in basso a destra)
const WEAPON_RIGHT_PERCENT: f32 = -4.0;
const WEAPON_RESTING_BOTTOM_VH: f32 = -4.0;

// =========================================================================
// FINE-TUNING FENDENTE DALL'ALTO (VerticalHit)
// =========================================================================
// Più aumenti questo valore, più lo sprite si sposta verso il centro (sinistra).
// Valori consigliati: tra 14.0 e 24.0.
const VERTICAL_HIT_RIGHT_PERCENT: f32 = 18.0;

// Offset verticale: valori positivi alzano lo sprite, negativi lo abbassano.
const VERTICAL_HIT_BOTTOM_OFFSET_VH: f32 = 0.0;

// =========================================================================
// FINE-TUNING AFFONDO (Thrust): archivio 6 della mod (cif_6-0 ... cif_6-4)
// =========================================================================
// Stessa logica del fendente verticale: piu' aumenti `RIGHT_PERCENT`, piu' lo sprite va verso
// il centro; `BOTTOM_OFFSET_VH` positivo alza lo sprite, negativo lo abbassa.
const THRUST_RIGHT_PERCENT: f32 = WEAPON_RIGHT_PERCENT;
const THRUST_BOTTOM_OFFSET_VH: f32 = 0.0;
/// Ingrandimento dei SOLI frame dell'affondo (1.0 = dimensione normale, 1.5 = +50%).
/// Si somma alla scala relativa alla posa di riposo e non tocca gli altri attacchi.
const THRUST_SCALE: f32 = 1.5;

/// Spostamento minimo del mouse (in pixel) per riconoscere un gesto di attacco.
const GESTURE_THRESHOLD: f32 = 14.0;

// =========================================================================
// ATTACCHI CONCATENATI (tasto sinistro tenuto premuto)
// =========================================================================
// Quando un attacco finisce e il tasto e' ancora premuto, il gesto successivo parte subito
// (a `GESTURE_THRESHOLD`). Un gesto fatto MENTRE l'attacco e' in corso viene messo in coda e
// parte appena finisce quello attuale.
// Il colpo concatenato deve essere DIVERSO dal precedente (su -> giu', lato -> su, ...): un
// movimento nella stessa direzione e' la coda dello stesso gesto e viene ignorato. Per
// ripetere lo stesso colpo si rilascia il tasto e si ricomincia.
/// Frazione dell'animazione (0.0 .. 1.0) dopo la quale il movimento del mouse conta come
/// "prossimo gesto". Prima viene ignorato: e' la coda del trascinamento che ha lanciato l'attacco
/// in corso e non deve contare come un secondo colpo. Metti 0.0 per accettarlo da subito.
const CHAIN_WINDOW_START: f32 = 0.5;
/// Spostamento del mouse (in pixel) per mettere in coda il colpo successivo durante un attacco.
const CHAIN_GESTURE_THRESHOLD: f32 = 20.0;

const ATTACK_DURATION: f32 = 0.35;

/// Il fendente verso destra e' lo specchio di quello verso sinistra (stessi frame, immagine
/// ribaltata e ancorata a sinistra). Con `false` si usano i frame originali dello StrikeRight
/// (record 4 della mod), sempre ancorati a sinistra.
const RIGHT_SWING_MIRRORS_LEFT: bool = true;

#[derive(Resource)]
pub struct FpsWeaponState {
    pub drawn: bool,
    pub progress: f32, // 0.0 = nascosta, 1.0 = sguainata

    pub current_action: WeaponAction,
    pub action_timer: f32,
    pub is_attacking: bool,
    pub has_texture: bool,

    // Gesti mouse e combo
    pub gesture_active: bool,
    /// Il gesto in corso e' iniziato con la pressione del tasto (e non e' stato riarmato dopo
    /// un attacco): solo in quel caso rilasciare senza trascinare conta come clic rapido.
    pub from_press: bool,
    /// Ultimo attacco lanciato a tasto sinistro tenuto premuto: il prossimo gesto deve essere
    /// diverso. Si azzera quando il tasto viene rilasciato o premuto di nuovo.
    pub last_chain: Option<WeaponAction>,
    pub gesture_delta: Vec2,
    pub combo_step: usize,
    pub queued_attack: Option<WeaponAction>,

    // Scala dinamica regolabile
    pub height_vh: f32,

    /// Altezza del frame mostrato rispetto a quella della posa di riposo (1.0 = uguale).
    /// I frame di Daggerfall hanno dimensioni diverse: se tutti fossero portati alla stessa
    /// altezza l'arma cambierebbe misura a ogni frame.
    pub frame_scale: f32,
}

impl Default for FpsWeaponState {
    fn default() -> Self {
        Self {
            drawn: false,
            progress: 0.0,
            current_action: WeaponAction::Idle,
            action_timer: 0.0,
            is_attacking: false,
            has_texture: false,
            gesture_active: false,
            from_press: false,
            last_chain: None,
            gesture_delta: Vec2::ZERO,
            combo_step: 0,
            queued_attack: None,
            height_vh: DEFAULT_WEAPON_HEIGHT_VH,
            frame_scale: 1.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WeaponAction {
    Idle,
    Draw,
    Holster,
    Swing,       // Fendente da destra verso sinistra (mouse a sinistra)
    SwingRight,  // Fendente simmetrico da sinistra verso destra (mouse a destra)
    VerticalHit, // Colpo verticale dall'alto verso il basso
    Thrust,      // Affondo
}

#[derive(Component)]
pub struct FpsWeaponSprite;

#[derive(Clone, Default)]
struct WeaponFrames {
    by_archive: HashMap<u8, Vec<(u8, String)>>,
    first_name: Option<String>,
}

#[derive(Resource, Default)]
pub struct ModWeaponCache {
    map: HashMap<String, WeaponFrames>,
    /// Immagine di ogni frame con le sue dimensioni in pixel (lette subito, non da `Assets`).
    loaded_handles: HashMap<String, (Handle<Image>, Vec2)>,
    scanned: bool,
}

struct ParsedName {
    base: String,
    archive: u8,
    record: u8,
    material: String,
}

pub struct FpsWeaponPlugin;

impl Plugin for FpsWeaponPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FpsWeaponState>()
        .init_resource::<ModWeaponCache>()
        .add_systems(Startup, setup_fps_weapon)
        .add_systems(
            Update,
            (
                scan_mod_weapons,
             handle_weapon_input,
             update_weapon_animation_logic,
             update_weapon_texture,
            )
            .chain(),
        );
    }
}

fn setup_fps_weapon(mut commands: Commands) {
    commands.spawn((
        Name::new("FPS Weapon"),
                    FpsWeaponSprite,
                    ImageNode::default(),
                    Node {
                        position_type: PositionType::Absolute,
                        right: Val::Percent(WEAPON_RIGHT_PERCENT),
                    bottom: Val::Vh(-DEFAULT_WEAPON_HEIGHT_VH - 10.0),
                    height: Val::Vh(DEFAULT_WEAPON_HEIGHT_VH),
                    width: Val::Auto,
                    ..default()
                    },
                    Visibility::Hidden,
                    GlobalZIndex(5),
    ));
}

fn scan_mod_weapons(
    mut cache: ResMut<ModWeaponCache>,
    mods: Res<ModTextures>,
) {
    if cache.scanned {
        return;
    }

    info!("Scansione metadati armi dalla mod...");

    for name in mods.names() {
        let lower = name.to_lowercase();

        if !lower.contains(".cif_") {
            continue;
        }

        if lower.contains("normal")
            || lower.contains("metallic")
            || lower.contains("gloss")
            || lower.contains("emission")
            || lower.contains("mask")
            || lower.contains("roughness")
            {
                continue;
            }

            if !is_weapon_name(&lower) {
                continue;
            }

            let Some(parsed) = parse_mod_weapon_name(&lower) else {
                continue;
            };

            let key = format!("{}_{}", parsed.base, parsed.material);
            let frames = cache.map.entry(key).or_default();

            frames
            .by_archive
            .entry(parsed.archive)
            .or_default()
            .push((parsed.record, name.to_string()));
    }

    for frames in cache.map.values_mut() {
        for list in frames.by_archive.values_mut() {
            list.sort_by_key(|(record, _)| *record);
        }

        frames.first_name = None;

        // Posa di guardia Idle a riposo: Archivio 0, Frame 0
        if let Some(list) = frames.by_archive.get(&0) {
            if let Some((_, name)) = list.first() {
                frames.first_name = Some(name.clone());
            }
        }

        if frames.first_name.is_none() {
            'outer: for archive in [5, 2, 1, 3, 4, 6] {
                if let Some(list) = frames.by_archive.get(&archive) {
                    if let Some((_, name)) = list.first() {
                        frames.first_name = Some(name.clone());
                        break 'outer;
                    }
                }
            }
        }
    }

    info!("Indicizzate {} combinazioni arma/materiale dalla mod.", cache.map.len());
    cache.scanned = true;
}

fn is_weapon_name(lower: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "sword", "axe", "dagger", "staff", "bow", "mace", "hammer",
        "flail", "katana", "wakazashi", "tanto", "saber", "broadsword",
        "longsword", "shortsword", "claymore", "battleaxe", "waraxe",
        "warhammer", "weapon",
    ];

    KEYWORDS.iter().any(|k| lower.contains(k))
}

fn parse_mod_weapon_name(raw_name: &str) -> Option<ParsedName> {
    let no_ext = raw_name
    .strip_suffix(".png")
    .or_else(|| raw_name.strip_suffix(".tga"))
    .unwrap_or(raw_name);

    let clean = no_ext.trim_start_matches("w_");

    let parts: Vec<&str> = clean.splitn(2, ".cif_").collect();
    if parts.len() != 2 {
        return None;
    }

    let base = parts[0].to_string();
    let meta = parts[1];

    let last_underscore = meta.rfind('_')?;
    let material = meta[last_underscore + 1..].to_string();
    let nums = &meta[..last_underscore];

    let mut it = nums.split('-');
    let archive: u8 = it.next()?.parse().ok()?;
    let record: u8 = it.next()?.parse().ok()?;

    if it.next().is_some() {
        return None;
    }

    Some(ParsedName {
        base,
         archive,
         record,
         material,
    })
}

fn handle_weapon_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    eq_state: Res<EquipmentState>,
    dialog: Res<crate::dialog::DialogState>,   // <-- aggiunto
    mut state: ResMut<FpsWeaponState>,
) {
    if eq_state.open || dialog.open {          // <-- aggiunto `|| dialog.open`
        return;
    }


    // Regolazione al volo della scala con tasti [ e ] o TN+ e TN-
    if keys.just_pressed(KeyCode::NumpadAdd) || keys.just_pressed(KeyCode::BracketRight) {
        state.height_vh = (state.height_vh + 4.0).min(115.0);
        info!("Dimensione arma: {:.0}vh", state.height_vh);
    }
    if keys.just_pressed(KeyCode::NumpadSubtract) || keys.just_pressed(KeyCode::BracketLeft) {
        state.height_vh = (state.height_vh - 4.0).max(35.0);
        info!("Dimensione arma: {:.0}vh", state.height_vh);
    }

    if keys.just_pressed(KeyCode::KeyZ) {
        state.drawn = !state.drawn;
        state.current_action = if state.drawn {
            WeaponAction::Draw
        } else {
            WeaponAction::Holster
        };
        state.action_timer = 0.0;
        state.is_attacking = false;
        state.gesture_active = false;
        state.from_press = false;
        state.last_chain = None;
        state.gesture_delta = Vec2::ZERO;
        state.queued_attack = None;
    }

    if !state.drawn {
        return;
    }

    // Pressione iniziale del tasto sinistro: avvia il rilevamento del gesto
    if mouse.just_pressed(MouseButton::Left) {
        if state.is_attacking {
            // Se stiamo già attaccando, pre-accoda il prossimo colpo combo per non perdere il clic
            state.combo_step += 1;
            let next_combo = match state.combo_step % 3 {
                0 => WeaponAction::Swing,
                1 => WeaponAction::VerticalHit,
                _ => WeaponAction::Thrust,
            };
            state.queued_attack = Some(next_combo);
        } else {
            state.gesture_active = true;
            state.from_press = true;
            state.last_chain = None;
            state.gesture_delta = Vec2::ZERO;
        }
    }

    // Tasto sinistro ancora premuto: si puo' concatenare un altro attacco senza rilasciarlo.
    if mouse.pressed(MouseButton::Left) {
        if state.is_attacking {
            // Durante l'attacco il movimento del mouse prepara il colpo successivo, ma solo
            // nell'ultima parte dell'animazione (la prima e' ancora il gesto che l'ha lanciato).
            if state.queued_attack.is_none() {
                if state.action_timer / ATTACK_DURATION >= CHAIN_WINDOW_START {
                    state.gesture_delta += motion.delta;
                    if state.gesture_delta.length() >= CHAIN_GESTURE_THRESHOLD {
                        // None se e' lo stesso colpo in corso: si ignora e si riparte da zero
                        state.queued_attack = chained_action(state.gesture_delta, Some(state.current_action));
                        state.gesture_delta = Vec2::ZERO;
                    }
                } else {
                    state.gesture_delta = Vec2::ZERO;
                }
            }
        } else if !state.gesture_active {
            // Attacco finito con il tasto ancora premuto: si e' di nuovo pronti a riconoscere
            // il gesto, senza dover rilasciare e ricliccare.
            state.gesture_active = true;
            state.gesture_delta = Vec2::ZERO;
        }
    }

    // Accumulo del trascinamento del mouse a tasto premuto (gesti Daggerfall)
    if state.gesture_active && !state.is_attacking {
        state.gesture_delta += motion.delta;

        if state.gesture_delta.length() >= GESTURE_THRESHOLD {
            match chained_action(state.gesture_delta, state.last_chain) {
                Some(action) => {
                    state.is_attacking = true;
                    state.action_timer = 0.0;
                    state.gesture_active = false;
                    state.from_press = false;
                    state.current_action = action;
                    state.last_chain = Some(action);
                }
                // stesso colpo dell'ultimo (coda dello stesso movimento): non e' un nuovo gesto,
                // si continua ad ascoltare e serve una direzione diversa
                None => {}
            }
            state.gesture_delta = Vec2::ZERO;
        }
    }

    // Clic rapido (rilascio senza trascinamento marcato):
    // Esegue una combo ciclica fluida (Fendente -> Verticale -> Affondo)
    if mouse.just_released(MouseButton::Left)
        && state.gesture_active
        && state.from_press
        && !state.is_attacking
    {
        state.is_attacking = true;
        state.action_timer = 0.0;
        state.gesture_active = false;
        state.gesture_delta = Vec2::ZERO;

        state.current_action = match state.combo_step % 3 {
            0 => WeaponAction::Swing,
            1 => WeaponAction::VerticalHit,
            _ => WeaponAction::Thrust,
        };
        state.combo_step += 1;
    }

    // Tasto rilasciato: il gesto non e' piu' attivo (e il movimento del mouse non attacca)
    if mouse.just_released(MouseButton::Left) {
        state.gesture_active = false;
        state.from_press = false;
        state.last_chain = None;
        if !state.is_attacking {
            state.gesture_delta = Vec2::ZERO;
        }
    }

    // Clic destro: scorciatoia per l'affondo (Thrust)
    if mouse.just_pressed(MouseButton::Right) {
        if state.is_attacking {
            state.queued_attack = Some(WeaponAction::Thrust);
        } else {
            state.is_attacking = true;
            state.current_action = WeaponAction::Thrust;
            state.action_timer = 0.0;
            state.gesture_active = false;
            state.from_press = false;
            state.gesture_delta = Vec2::ZERO;
        }
    }
}

/// Attacco scelto da un trascinamento del mouse (`delta` in pixel di schermo, y verso il basso):
///   - asse orizzontale dominante: fendente a sinistra / a destra;
///   - verso il basso: colpo verticale dall'alto (fendente);
///   - verso l'alto, dal basso verso l'alto: affondo (e' l'opposto del fendente verticale).
fn gesture_action(delta: Vec2) -> WeaponAction {
    if delta.x.abs() > delta.y.abs() {
        if delta.x > 0.0 { WeaponAction::SwingRight } else { WeaponAction::Swing }
    } else if delta.y > 0.0 {
        WeaponAction::VerticalHit
    } else {
        WeaponAction::Thrust
    }
}

/// Attacco concatenato dal trascinamento `delta`, oppure None se e' lo stesso colpo di `previous`
/// (la coda dello stesso movimento, che non deve contare come un secondo colpo).
fn chained_action(delta: Vec2, previous: Option<WeaponAction>) -> Option<WeaponAction> {
    let action = gesture_action(delta);
    (Some(action) != previous).then_some(action)
}

fn update_weapon_animation_logic(
    time: Res<Time>,
    mut state: ResMut<FpsWeaponState>,
    inventory: Res<PlayerInventory>,
    mut sprites: Query<(&mut Node, &mut Visibility), With<FpsWeaponSprite>>,
) {
    let dt = time.delta_secs().min(0.1);

    const SPEED: f32 = 8.0;

    let target_progress = if state.drawn { 1.0 } else { 0.0 };
    state.progress += (target_progress - state.progress) * dt * SPEED;
    state.progress = state.progress.clamp(0.0, 1.0);

    // Gestione del timer di attacco e reset immediato a riposo (Idle)
    if state.is_attacking {
        state.action_timer += dt;
        if state.action_timer >= ATTACK_DURATION {
            state.action_timer = 0.0;
            if let Some(next) = state.queued_attack.take() {
                state.current_action = next;
                state.last_chain = Some(next);
                state.is_attacking = true;
                state.gesture_delta = Vec2::ZERO;
            } else {
                state.is_attacking = false;
                state.current_action = WeaponAction::Idle;
            }
        }
    }

    let has_weapon = inventory.equipped_in(EquipSlot::RightHand).is_some();
    let current_h = state.height_vh;

    // Altezza del frame mostrato: la posa di riposo vale `height_vh`, gli altri frame in proporzione.
    // L'affondo ha in piu' il suo ingrandimento (`THRUST_SCALE`).
    let action_scale = match state.current_action {
        WeaponAction::Thrust => THRUST_SCALE,
        WeaponAction::VerticalHit => 1.3,
        _ => 1.0,
    };
    let frame_h = current_h * state.frame_scale * action_scale;

    // Seleziona la coordinata orizzontale e l'offset verticale in base all'azione
    let (right_pct, bottom_offset) = match state.current_action {
        WeaponAction::VerticalHit => (VERTICAL_HIT_RIGHT_PERCENT, VERTICAL_HIT_BOTTOM_OFFSET_VH),
        WeaponAction::Thrust => (THRUST_RIGHT_PERCENT, THRUST_BOTTOM_OFFSET_VH),
        _ => (WEAPON_RIGHT_PERCENT, 0.0),
    };

    for (mut node, mut visibility) in &mut sprites {
        let bottom_vh = (state.progress - 1.0) * (frame_h + 10.0) + WEAPON_RESTING_BOTTOM_VH + bottom_offset;
        node.bottom = Val::Vh(bottom_vh);

        // Il fendente destro e' specchiato: l'arma parte dal lato sinistro dello schermo.
        if mirrored_swing(state.current_action) {
            node.left = Val::Percent(right_pct);
            node.right = Val::Auto;
        } else {
            node.right = Val::Percent(right_pct);
            node.left = Val::Auto;
        }
        node.height = Val::Vh(frame_h);

        *visibility = if has_weapon && state.has_texture && state.progress > 0.01 {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
    }
}

/// Carica (o prende dalla cache) l'immagine di un frame con le sue dimensioni in pixel.
///
/// Le dimensioni vengono dalla mod e non da `Assets<Image>`: le immagini caricate con
/// `RenderAssetUsages::RENDER_WORLD` vengono tolte da `Assets<Image>` dopo il primo upload sulla
/// GPU, quindi `images.get(&handle)` restituisce Some solo la prima volta che un frame appare.
fn load_frame(
    name: &str,
    cache: &mut ModWeaponCache,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
) -> Option<(Handle<Image>, Vec2)> {
    if let Some((handle, size)) = cache.loaded_handles.get(name) {
        return Some((handle.clone(), *size));
    }
    let handle = mods.image_by_name(name, images)?;
    let size = mods
    .size_by_name(name)
    .map(|(w, h)| Vec2::new(w as f32, h as f32))
    .unwrap_or(Vec2::ZERO);
    cache.loaded_handles.insert(name.to_string(), (handle.clone(), size));
    Some((handle, size))
}

/// Rapporto larghezza/altezza di un frame, se le dimensioni sono note.
fn aspect_of(size: Vec2) -> Option<f32> {
    (size.x > 0.0 && size.y > 0.0).then(|| size.x / size.y)
}

/// Altezza di un frame rispetto alla posa di riposo. 1.0 se una delle due e' sconosciuta.
fn relative_scale(frame: Vec2, idle: Vec2) -> f32 {
    if frame.y > 0.0 && idle.y > 0.0 {
        (frame.y / idle.y).clamp(0.25, 4.0)
    } else {
        1.0
    }
}

/// Il fendente destro e' disegnato ribaltato (e ancorato a sinistra).
fn mirrored_swing(action: WeaponAction) -> bool {
    action == WeaponAction::SwingRight && RIGHT_SWING_MIRRORS_LEFT
}

/// Nomi alternativi con cui una mod puo' chiamare lo stesso tipo di arma (nomi di file corti).
fn base_aliases(base: &str) -> Vec<String> {
    let extra: &[&str] = match base {
        "battleaxe" => &["battleax", "battle_axe", "battle-axe"],
        "waraxe" => &["warax", "war_axe", "war-axe"],
        "warhammer" => &["war_hammer"],
        "shortsword" => &["short_sword"],
        "broadsword" => &["broad_sword"],
        "longsword" => &["long_sword"],
        _ => &[],
    };
    std::iter::once(base).chain(extra.iter().copied()).map(str::to_string).collect()
}

/// Frame dell'arma: prima il materiale esatto, poi lo stesso tipo in un altro materiale
/// (cosi' le animazioni si vedono anche se la mod non ha quel metallo).
fn find_frames<'a>(
    map: &'a HashMap<String, WeaponFrames>,
    base: &str,
    material: &str,
) -> Option<&'a WeaponFrames> {
    let aliases = base_aliases(base);
    for alias in &aliases {
        if let Some(f) = map.get(&format!("{alias}_{material}")) {
            return Some(f);
        }
    }
    for alias in &aliases {
        let prefix = format!("{alias}_");
        let mut keys: Vec<&String> = map.keys().filter(|k| k.starts_with(&prefix)).collect();
        keys.sort();
        // si preferiscono i metalli comuni
        let pick = ["steel", "iron"]
        .iter()
        .find_map(|m| keys.iter().find(|k| k.ends_with(m)))
        .or_else(|| keys.first());
        if let Some(k) = pick {
            return map.get(*k);
        }
    }
    None
}

fn update_weapon_texture(
    inventory: Res<PlayerInventory>,
    mut state: ResMut<FpsWeaponState>,
    mut cache: ResMut<ModWeaponCache>,
    mut mods: ResMut<ModTextures>,
    mut images: ResMut<Assets<Image>>,
    mut sprites: Query<(&mut ImageNode, &mut Node), With<FpsWeaponSprite>>,
                         mut last_loaded: Local<Option<(u32, WeaponAction, usize)>>,
                         mut warned_item: Local<Option<u32>>,
) {
    let mirrored = mirrored_swing(state.current_action);
    let weapon = inventory
    .items
    .iter()
    .find(|i| i.equipped && i.slot == EquipSlot::RightHand);

    let item_id = weapon.map(|w| w.id).unwrap_or(0);

    let frame_idx = if state.is_attacking {
        let t = (state.action_timer / ATTACK_DURATION).clamp(0.0, 0.99);
        (t * 5.0).floor() as usize
    } else {
        0
    };

    let key = (item_id, state.current_action, frame_idx);
    if *last_loaded == Some(key) {
        return;
    }

    // (immagine, rapporto d'aspetto, altezza relativa alla posa di riposo)
    let mut shown: Option<(Handle<Image>, Option<f32>, f32)> = None;

    if let Some(w) = weapon {
        let (base, material) = lookup_keys(w);

        // Nomi dei file (copiati, cosi' la cache si puo' riscrivere subito dopo).
        let names = find_frames(&cache.map, &base, &material).map(|frames| {
            (
                pick_frame_name(frames, WeaponAction::Idle, 0).map(str::to_string),
             pick_frame_name(frames, state.current_action, frame_idx).map(str::to_string),
            )
        });

        if names.is_none() && *warned_item != Some(item_id) {
            *warned_item = Some(item_id);
            let mut available: Vec<&str> = cache.map.keys().map(String::as_str).filter(|k| k.contains("ax")).collect();
            available.sort();
            available.truncate(12);
            warn!(
                "Nessun frame in prima persona per '{}' (cercato '{base}', materiale '{material}'). Armi con 'ax' nella mod: {:?}",
                  w.name, available
            );
        }

        if let Some((idle_name, frame_name)) = names {
            let idle = idle_name.and_then(|n| load_frame(&n, &mut cache, &mut mods, &mut images));
            // se il frame richiesto non si carica si ripiega sulla posa di riposo
            let frame = frame_name
            .and_then(|n| load_frame(&n, &mut cache, &mut mods, &mut images))
            .or_else(|| idle.clone());
            if let Some((handle, size)) = frame {
                let idle_size = idle.map(|(_, s)| s).unwrap_or(size);
                shown = Some((handle, aspect_of(size), relative_scale(size, idle_size)));
            }
        }
    }

    state.has_texture = shown.is_some();

    if let Some((handle, aspect, scale)) = shown {
        state.frame_scale = scale;
        for (mut img_node, mut node) in &mut sprites {
            img_node.image = handle.clone();
            img_node.flip_x = mirrored;
            if aspect.is_some() {
                node.aspect_ratio = aspect;
            }
        }
    }

    *last_loaded = Some(key);
}

fn pick_frame_name<'a>(
    frames: &'a WeaponFrames,
    action: WeaponAction,
    frame_idx: usize,
) -> Option<&'a str> {
    // Archivi della mod (cif_<archivio>-<frame>):
    // - Idle: Archivio 0 (guardia a riposo eretta), poi 5, 2
    // - VerticalHit: Archivio 1 (fendente verticale dall'alto)
    // - Swing: Archivio 3 (fendente orizzontale/obliquo)
    // - Thrust: Archivio 6 (affondo, frame 0..4); se manca si ripiega sul 2
    // L'archivio 6 non e' un ripiego degli altri attacchi: mostrerebbe un affondo al posto di un fendente.
    let fallback_order: &[u8] = match action {
        WeaponAction::Idle | WeaponAction::Draw | WeaponAction::Holster => &[0, 5, 2, 1, 3, 4],
        WeaponAction::VerticalHit => &[1, 0, 4, 3, 2, 5],
        WeaponAction::Swing => &[3, 5, 1, 2, 4],
        WeaponAction::SwingRight if RIGHT_SWING_MIRRORS_LEFT => &[3, 5, 1, 2, 4],
        WeaponAction::SwingRight => &[4, 3, 5, 1, 2],
        WeaponAction::Thrust => &[6, 2, 4, 0, 1, 3, 5],
    };

    for archive in fallback_order {
        if let Some(list) = frames.by_archive.get(archive) {
            if !list.is_empty() {
                let idx = if matches!(action, WeaponAction::Idle | WeaponAction::Draw | WeaponAction::Holster) {
                    0
                } else {
                    frame_idx.min(list.len() - 1)
                };
                return Some(&list[idx].1);
            }
        }
    }

    frames.first_name.as_deref()
}

fn lookup_keys(item: &InventoryItem) -> (String, String) {
    let base = infer_base_from_name(&item.name);
    let material = infer_material_from_name(&item.name);
    (base.to_lowercase(), material.to_lowercase())
}

fn infer_base_from_name(name: &str) -> String {
    let n = name.to_lowercase().replace(' ', "");

    const BASES: &[&str] = &[
        "daikatana", "claymore", "broadsword", "longsword", "shortsword",
        "wakazashi", "tanto", "dagger", "saber", "katana", "battleaxe",
        "waraxe", "warhammer", "flail", "mace", "staff", "bow", "arrow",
    ];

    for base in BASES {
        if n.contains(base) {
            return (*base).to_string();
        }
    }

    "longsword".to_string()
}

fn infer_material_from_name(name: &str) -> String {
    let n = name.to_lowercase().replace(' ', "");

    const MATERIALS: &[&str] = &[
        "daedric", "orcish", "ebony", "adamantium", "mithril",
        "dwarven", "elven", "silver", "steel", "iron",
    ];

    for material in MATERIALS {
        if n.contains(material) {
            return (*material).to_string();
        }
    }

    "iron".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames() -> WeaponFrames {
        let mut f = WeaponFrames::default();
        for archive in [0u8, 1, 3] {
            let list = (0..5u8).map(|r| (r, format!("w_longsword.cif_{archive}-{r}_steel.png"))).collect();
            f.by_archive.insert(archive, list);
        }
        f
    }

    #[test]
    fn idle_uses_first_frame_of_rest_pose_for_every_non_attack_action() {
        let f = frames();
        for action in [WeaponAction::Idle, WeaponAction::Draw, WeaponAction::Holster] {
            assert_eq!(pick_frame_name(&f, action, 4), Some("w_longsword.cif_0-0_steel.png"));
        }
    }

    #[test]
    fn swing_and_vertical_hit_use_their_own_records_and_clamp_the_frame() {
        let f = frames();
        assert_eq!(pick_frame_name(&f, WeaponAction::Swing, 2), Some("w_longsword.cif_3-2_steel.png"));
        assert_eq!(pick_frame_name(&f, WeaponAction::VerticalHit, 9), Some("w_longsword.cif_1-4_steel.png"));
    }

    #[test]
    fn mouse_drag_picks_the_attack() {
        // dal basso verso l'alto (y negativa sullo schermo) = affondo, il contrario del fendente
        assert_eq!(gesture_action(Vec2::new(2.0, -20.0)), WeaponAction::Thrust);
        assert_eq!(gesture_action(Vec2::new(-3.0, 18.0)), WeaponAction::VerticalHit);
        assert_eq!(gesture_action(Vec2::new(-20.0, 4.0)), WeaponAction::Swing);
        assert_eq!(gesture_action(Vec2::new(20.0, -4.0)), WeaponAction::SwingRight);
        // quasi in diagonale vince l'asse piu' lungo
        assert_eq!(gesture_action(Vec2::new(10.0, -12.0)), WeaponAction::Thrust);
    }

    #[test]
    fn chained_attacks_must_differ_from_the_previous_one() {
        let up = Vec2::new(0.0, -30.0);
        let down = Vec2::new(0.0, 30.0);
        // stesso colpo di prima: ignorato
        assert_eq!(chained_action(up, Some(WeaponAction::Thrust)), None);
        assert_eq!(chained_action(down, Some(WeaponAction::VerticalHit)), None);
        // colpo diverso: concatenato
        assert_eq!(chained_action(down, Some(WeaponAction::Thrust)), Some(WeaponAction::VerticalHit));
        assert_eq!(chained_action(up, Some(WeaponAction::Swing)), Some(WeaponAction::Thrust));
        // nessun colpo precedente (nuova pressione): qualsiasi gesto vale
        assert_eq!(chained_action(up, None), Some(WeaponAction::Thrust));
    }

    #[test]
    fn thrust_uses_archive_6_frames_0_to_4() {
        let mut f = frames();
        f.by_archive.insert(6, (0..5u8).map(|r| (r, format!("w_longsword.cif_6-{r}_steel.png"))).collect());
        f.by_archive.insert(2, (0..5u8).map(|r| (r, format!("w_longsword.cif_2-{r}_steel.png"))).collect());
        for r in 0..5usize {
            assert_eq!(
                pick_frame_name(&f, WeaponAction::Thrust, r),
                Some(format!("w_longsword.cif_6-{r}_steel.png").as_str())
            );
        }
        // l'animazione dura 5 frame: un indice oltre il quarto resta sull'ultimo
        assert_eq!(pick_frame_name(&f, WeaponAction::Thrust, 9), Some("w_longsword.cif_6-4_steel.png"));
    }

    #[test]
    fn thrust_falls_back_to_archive_2_and_other_attacks_never_use_6() {
        // senza archivio 6 l'affondo usa il 2, come prima
        let mut f = frames();
        f.by_archive.insert(2, (0..5u8).map(|r| (r, format!("w_longsword.cif_2-{r}_steel.png"))).collect());
        assert_eq!(pick_frame_name(&f, WeaponAction::Thrust, 1), Some("w_longsword.cif_2-1_steel.png"));
        // un'arma che ha SOLO l'archivio 6 (piu' la guardia): i fendenti non lo usano come ripiego
        let mut only6 = WeaponFrames::default();
        only6.by_archive.insert(0, vec![(0, "w_x.cif_0-0_iron.png".into())]);
        only6.by_archive.insert(6, vec![(0, "w_x.cif_6-0_iron.png".into())]);
        for a in [WeaponAction::Swing, WeaponAction::SwingRight, WeaponAction::VerticalHit] {
            assert_ne!(pick_frame_name(&only6, a, 0), Some("w_x.cif_6-0_iron.png"), "{a:?}");
        }
        assert_eq!(pick_frame_name(&only6, WeaponAction::Thrust, 0), Some("w_x.cif_6-0_iron.png"));
    }

    #[test]
    fn right_swing_is_the_mirror_of_the_left_swing() {
        let f = frames();
        assert!(mirrored_swing(WeaponAction::SwingRight));
        assert!(!mirrored_swing(WeaponAction::Swing));
        // stessi frame del fendente sinistro, ribaltati
        assert_eq!(
            pick_frame_name(&f, WeaponAction::SwingRight, 3),
                   pick_frame_name(&f, WeaponAction::Swing, 3)
        );
    }

    #[test]
    fn frames_are_found_by_alias_and_by_other_material() {
        let mut map: HashMap<String, WeaponFrames> = HashMap::new();
        map.insert("battleax_iron".into(), frames());
        map.insert("longsword_steel".into(), frames());
        // alias del nome di file corto
        assert!(find_frames(&map, "battleaxe", "iron").is_some());
        // stesso tipo, materiale diverso
        assert!(find_frames(&map, "battleaxe", "daedric").is_some());
        assert!(find_frames(&map, "longsword", "elven").is_some());
        // tipo assente
        assert!(find_frames(&map, "mace", "iron").is_none());
    }

    #[test]
    fn frame_scale_is_relative_to_the_rest_pose() {
        let idle = Vec2::new(300.0, 600.0);
        assert_eq!(relative_scale(Vec2::new(500.0, 600.0), idle), 1.0); // stessa altezza
        assert_eq!(relative_scale(Vec2::new(300.0, 900.0), idle), 1.5);
        assert_eq!(relative_scale(Vec2::new(300.0, 100.0), idle), 0.25); // limite inferiore
        assert_eq!(relative_scale(Vec2::ZERO, idle), 1.0); // dimensioni sconosciute
        assert_eq!(aspect_of(Vec2::new(300.0, 600.0)), Some(0.5));
        assert_eq!(aspect_of(Vec2::ZERO), None);
    }
}
