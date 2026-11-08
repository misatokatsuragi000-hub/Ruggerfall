//! Colla Bevy per il runtime QBN (`qbn.rs`): main quest di Daggerfall.
//!
//! Flusso:
//!   gioco/dialogo --QbnInput--> `handle_inputs` --> `QbnRuntime` --Effect--> `QbnEffectEvent`
//!                                                                    \--> `apply_effects`
//!   ogni 0.5 s `poll_runtime` controlla orologi, osservatori di reputazione, `when`, `level`.
//!   I testi (`Effect::Say`) finiscono in `QbnSayQueue`, che `dialog.rs` mostra come risposta del PNG.
//!
//! In `main.rs`:
//!   mod qbn; mod qbn_npc; mod main_quest;
//!   .add_plugins((main_quest::MainQuestPlugin, qbn_npc::QbnNpcPlugin))
//!
//! Cio' che resta da collegare a codice che non ho visto e' marcato `TODO(hook)`.

use crate::dfquest::{pretty_name, Library, PersonDef, PlaceDef, QuestPhase, QuestStates};
use crate::dialog::{DialogState, ScriptedOffer};
use crate::journal::{rep_key, QuestJournal, Reputation};
use crate::quest_items::{QuestItemPickup, QuestItems};
use crate::toast::Toasts;
use crate::qbn::{offer_questor, script_id_for_number, unsupported, Effect, QbnRuntime, QbnWorld, SymbolResolver};
use crate::sky::SkyClock;
use crate::{QuestLibRes, QuestStatesRes};
use bevy::prelude::*;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

pub const MAIN_QUEST_ID: &str = "S0000001";
const RUNTIME_FILE: &str = "quest_runtime.txt";
const STATES_FILE: &str = "quest_states.txt";

// =============================================================================
// Risorse ed eventi
// =============================================================================

#[derive(Resource)]
pub struct QbnRuntimeRes(pub QbnRuntime);

/// Tempo di gioco assoluto in secondi, derivato da `SkyClock` (rispetta pausa e scala del giorno).
#[derive(Resource, Default)]
pub struct QbnClock {
    pub seconds: f64,
}

/// Fotografia del giocatore per le condizioni del QBN.
/// TODO(hook): riempirla da inventario / reputazione / livello reali.
#[derive(Resource, Default)]
pub struct QbnWorldInfo {
    /// Simboli QBN degli oggetti di quest nell'inventario (es. `"deathCertificate"`).
    pub items: HashSet<String>,
    /// Reputazione per simbolo persona (es. `"_akorithi_"`).
    pub repute: HashMap<String, i32>,
    pub level: u32,
}

impl QbnWorld for QbnWorldInfo {
    fn has_item(&self, s: &str) -> bool {
        self.items.contains(s)
    }
    fn repute(&self, s: &str) -> i32 {
        *self.repute.get(s).unwrap_or(&0)
    }
    fn player_level(&self) -> u32 {
        self.level
    }
}

/// Testi di quest in attesa di essere mostrati nel dialogo aperto.
#[derive(Resource, Default)]
pub struct QbnSayQueue(pub VecDeque<String>);

/// Ingressi verso il runtime. Li inviano dialog.rs, l'inventario, ecc.
#[derive(Event, Debug, Clone)]
pub enum QbnInput {
    StartQuest(String),
    /// Simbolo QBN del PNG cliccato (es. `"_lhotun_"`).
    NpcClicked(String),
    ItemClicked(String),
    /// Lettura di un oggetto: serve la quest perche' si puo' rileggere a quest conclusa.
    ItemUsed { quest: String, item: String },
    /// Risposta al `prompt` della quest: `label` = "yes" / "no".
    PromptAnswer { quest: String, label: String },
    /// Forza un task (debug o comandi di gioco).
    FireTask { quest: String, task: String },
    /// Un nemico creato dalla quest (`create foe`) e' stato ferito: simbolo QBN (`_F.00_`).
    FoeInjured(String),
    /// Accende una variabile globale (`ElysannaSatisfied`): la usano i task `Global` delle quest.
    SetGlobal(String),
}

/// Ogni effetto prodotto dal runtime, per chi deve applicarlo.
#[derive(Event, Debug, Clone)]
pub struct QbnEffectEvent(pub Effect);

// =============================================================================
// Nemici delle quest (`create foe`)
// =============================================================================

/// Marker sui nemici creati da una quest (`create foe _F.00_ ...`).
/// Serve a `combat::apply_player_strike` per emettere `QbnInput::FoeInjured(sym)` al primo colpo.
#[derive(Component, Clone, Debug)]
pub struct QbnFoe {
    pub quest: String,
    pub symbol: String,
}

/// Template testuale di un `Foe` QBN -> (archivio, record) dello sprite.
///
/// I nomi sono quelli di `MobileTypes` di DFU. Gli umanoidi (128..=146) condividono
/// `TEXTURE.334` con record variabile. Se il template non è noto si ricade sul Rat.
fn template_to_sprite(template: &str) -> (u16, u8) {
    let t = template.to_ascii_lowercase().replace([' ', '-'], "_");
    match t.as_str() {
        "rat" | "giant_rat" => (257, 0),
        "imp" => (259, 0),
        "spriggan" => (260, 0),
        "giant_bat" => (261, 0),
        "grizzly_bear" | "bear" => (262, 0),
        "sabertooth_tiger" | "tiger" => (263, 0),
        "spider" | "giant_spider" => (264, 0),
        "orc" => (267, 0),
        "centaur" => (269, 0),
        "werewolf" => (270, 0),
        "nymph" => (271, 0),
        "slaughterfish" => (272, 0),
        "orc_sergeant" => (273, 0),
        "harpy" => (274, 0),
        "wereboar" => (275, 0),
        "skeletal_warrior" | "skeleton" => (276, 0),
        "giant" => (277, 0),
        "zombie" => (278, 0),
        "ghost" => (279, 0),
        "mummy" => (280, 0),
        "giant_scorpion" | "scorpion" => (281, 0),
        "gargoyle" => (282, 0),
        "wraith" => (283, 0),
        "daedroth" | "daedra" => (285, 0),
        "fire_atronach" => (287, 0),
        "iron_atronach" => (288, 0),
        "flesh_atronach" => (289, 0),
        "ice_atronach" => (290, 0),
        // Umanoidi: TEXTURE.334, record = id - 128
        "knight" => (334, 145 - 128),
        "warrior" => (334, 144 - 128),
        "barbarian" => (334, 143 - 128),
        "ranger" => (334, 142 - 128),
        "archer" => (334, 141 - 128),
        "monk" => (334, 140 - 128),
        "assassin" => (334, 139 - 128),
        "thief" => (334, 138 - 128),
        "acrobat" => (334, 137 - 128),
        "rogue" => (334, 136 - 128),
        "burglar" => (334, 135 - 128),
        "bard" => (334, 134 - 128),
        "nightblade" => (334, 133 - 128),
        "healer" => (334, 132 - 128),
        "sorcerer" => (334, 131 - 128),
        "battle_mage" | "battlemage" => (334, 130 - 128),
        "spellsword" => (334, 129 - 128),
        "mage" => (334, 128 - 128),
        _ => (257, 0),
    }
}

/// Crea un billboard nemico vicino al player per `Effect::SpawnFoe`.
///
/// Il nemico è un `Health` + `QbnFoe` + billboard statico con la texture della
/// specie. Stesso schema dei civili (`tiles.rs`): `combat::apply_player_strike`
/// lo colpisce (via `Health`) e, grazie a `QbnFoe`, emette `FoeInjured`.
/// Se lo sprite non è caricabile, usa un rettangolo rosso (così il nemico è
/// comunque visibile e cliccabile).
#[allow(clippy::too_many_arguments)]
fn spawn_quest_foes(
    mut commands: Commands,
    mut events: EventReader<QbnEffectEvent>,
    player: Query<&GlobalTransform, With<crate::player::Player>>,
    data: Res<crate::world::GameData>,
    mut wall_textures: ResMut<crate::wall_textures::WallTextures>,
    mut mods: ResMut<crate::mod_textures::ModTextures>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let Ok(player_gtf) = player.get_single() else {
        for _ in events.read() {}
        return;
    };
    let origin = player_gtf.translation();
    let fwd = player_gtf.forward();
    let flat = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero();
    let base = if flat == Vec3::ZERO { Vec3::NEG_Z } else { flat };

    for QbnEffectEvent(e) in events.read() {
        let Effect::SpawnFoe { quest, foe, template } = e else { continue };

        let (archive, record) = template_to_sprite(template);
        let (mat, w_px, h_px) = match wall_textures.flat_material(
            archive, record, &data, &mut mods, &mut images, &mut materials,
        ) {
            Some(t) => t,
            None => {
                warn!(
                    "[quest] template `{template}` senza sprite (TEXTURE.{archive}): uso fallback rosso"
                );
                let mat = materials.add(StandardMaterial {
                    base_color: Color::srgb(0.75, 0.15, 0.15),
                                        unlit: true,
                                        cull_mode: None,
                                        double_sided: true,
                                        ..default()
                });
                (mat, 64.0, 64.0)
            }
        };
        let size = Vec2::new(w_px * 0.025, h_px * 0.025).max(Vec2::splat(1.0));

        // Offset "spallato" per non sovrapporre nemici spawnati insieme.
        // Offset "spallato" per non sovrapporre nemici spawnati insieme.
        // Tutto in XZ: `base` è Vec3 (ma la Y è 0), `side` è Vec2.
        let base2 = Vec2::new(base.x, base.z);
        let side2 = Vec2::new(-base.z, base.x);
        let hash: u32 = quest.bytes().chain(foe.bytes()).map(|b| b as u32).sum();
        let lateral = ((hash % 5) as f32 - 2.0) * 0.6;
        let offset2 = base2 * 2.5 + side2 * lateral;

        let feet = origin.y - crate::player::EYE_HEIGHT;
        let pos = Vec3::new(
            origin.x + offset2.x,
            feet + size.y * 0.5,
            origin.z + offset2.y,
        );
        commands.spawn((
            Name::new(format!("Quest foe {foe} ({quest})")),
                        Mesh3d(meshes.add(Rectangle::new(size.x, size.y))),
                        MeshMaterial3d(mat),
                        Transform::from_translation(pos),
                        crate::interior::FlatBillboard,
                        crate::combat::Health::new(40.0),
                        QbnFoe { quest: quest.clone(), symbol: foe.clone() },
        ));
        info!("[quest] nemico `{foe}` ({template}) creato vicino al player");
    }
}

// =============================================================================
// Scelta dei nomi all'avvio della quest
// =============================================================================

const REDGUARD_MALE: [&str; 8] =
["Cassim", "Hamid", "Rashad", "Tahir", "Idris", "Yusuf", "Zahir", "Karim"];

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
}

/// Risolutore minimo: nomi da una lista, luogo scelto a caso tra i `targets`.
/// TODO(hook): per i `Place ... remote tavern` scegliere una vera taverna in una citta' reale.
pub struct DefaultResolver;

impl SymbolResolver for DefaultResolver {
    fn person_name(&mut self, quest: &str, def: &PersonDef) -> String {
        let i = fnv(&format!("{quest}:{}", def.symbol)) as usize % REDGUARD_MALE.len();
        REDGUARD_MALE[i].to_string()
    }

    fn place(&mut self, quest: &str, def: &PlaceDef) -> (String, Option<String>) {
        if def.targets.is_empty() {
            return ("the place".to_string(), Some(crate::qbn_npc::HOME_TOWN.to_string()));
        }
        let i = fnv(&format!("{quest}:{}", def.symbol)) as usize % def.targets.len();
        (pretty_name(&def.targets[i]), Some(crate::qbn_npc::HOME_TOWN.to_string()))
    }
}

// =============================================================================
// Plugin
// =============================================================================

pub struct MainQuestPlugin;

impl Plugin for MainQuestPlugin {
    fn build(&self, app: &mut App) {
        let rt = std::fs::read_to_string(RUNTIME_FILE)
        .map(|t| QbnRuntime::from_text(&t))
        .unwrap_or_default();
        app.insert_resource(QbnClock { seconds: rt.last_now() })
        .insert_resource(QbnRuntimeRes(rt))
        .init_resource::<QbnWorldInfo>()
        .init_resource::<QbnSayQueue>()
        .add_event::<QbnInput>()
        .add_event::<QbnEffectEvent>()
        .add_systems(Startup, (bootstrap_main_quest, audit_quest_scripts))
        .add_systems(
            Update,
            (sync_clock, handle_inputs, poll_runtime, apply_effects).chain(),
        )
        .add_systems(Update, spawn_quest_foes.after(apply_effects));
        if cfg!(debug_assertions) {
            app.add_systems(Update, debug_quest_keys);
        }
    }
}

/// Per ogni script caricato elenca le istruzioni e le definizioni che il runtime non riconosce.
/// Serve a sapere, prima di giocarle, cosa manca alle quest successive: copia qui l'output
/// (righe `[quest-audit]`) per dire a chi sviluppa quali istruzioni aggiungere.
fn audit_quest_scripts(lib: Res<QuestLibRes>) {
    let mut heads: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;
    for script in lib.0.scripts() {
        let unknown = unsupported(script);
        if unknown.is_empty() && script.unknown.is_empty() {
            continue;
        }
        warn!(
            "[quest-audit] {}: {} istruzioni e {} definizioni non riconosciute",
            script.id,
            unknown.len(),
              script.unknown.len()
        );
        for u in &unknown {
            info!("[quest-audit]   {}: {u}", script.id);
            let head = u.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
            *heads.entry(head).or_default() += 1;
            total += 1;
        }
        for d in &script.unknown {
            info!("[quest-audit]   {}: definizione `{d}`", script.id);
        }
    }
    if total == 0 {
        info!("[quest-audit] tutte le istruzioni degli script caricati sono supportate");
    } else {
        warn!("[quest-audit] riepilogo ({total} istruzioni non supportate): {heads:?}");
    }
}

/// Avvia la main quest al primo avvio (se non e' gia' in corso).
fn bootstrap_main_quest(
    rt: Res<QbnRuntimeRes>,
    lib: Res<QuestLibRes>,
    mut inputs: EventWriter<QbnInput>,
) {
    if lib.0.get(MAIN_QUEST_ID).is_some() && !rt.0.is_running(MAIN_QUEST_ID) {
        inputs.send(QbnInput::StartQuest(MAIN_QUEST_ID.to_string()));
    }
}

fn sync_clock(sky: Res<SkyClock>, mut clock: ResMut<QbnClock>) {
    let d = sky.date;
    clock.seconds = d.absolute_day() as f64 * 86_400.0 + d.hour as f64 * 3_600.0;
}

fn persist(rt: &QbnRuntime, states: &QuestStates) {
    if let Err(e) = std::fs::write(RUNTIME_FILE, rt.to_text()) {
        warn!("Quest: impossibile salvare {RUNTIME_FILE}: {e}");
    }
    if let Err(e) = std::fs::write(STATES_FILE, states.to_text()) {
        warn!("Quest: impossibile salvare {STATES_FILE}: {e}");
    }
}

fn publish(
    fx: Vec<Effect>,
    rt: &QbnRuntime,
    states: &QuestStates,
    out: &mut EventWriter<QbnEffectEvent>,
) {
    if fx.is_empty() {
        return;
    }
    for e in fx {
        out.send(QbnEffectEvent(e));
    }
    persist(rt, states);
}

#[allow(clippy::too_many_arguments)]
pub fn handle_inputs(
    mut inputs: EventReader<QbnInput>,
    mut rt: ResMut<QbnRuntimeRes>,
    lib: Res<QuestLibRes>,
    mut states: ResMut<QuestStatesRes>,
    mut world: ResMut<QbnWorldInfo>,
    items: Res<QuestItems>,
    clock: Res<QbnClock>,
    mut out: EventWriter<QbnEffectEvent>,
) {
    for ev in inputs.read() {
        let now = clock.seconds;
        world.items = items.symbols();
        let fx = match ev {
            QbnInput::StartQuest(id) => {
                rt.0.start_quest(&lib.0, &mut states.0, &mut DefaultResolver, id, now)
            }
            QbnInput::NpcClicked(sym) => {
                let fx = rt.0.on_click_npc(&lib.0, &mut states.0, &*world, sym, now);
                info!("[quest] clic su `{sym}`: {} effetti", fx.len());
                fx
            }
            QbnInput::ItemClicked(sym) => rt.0.on_click_item(&lib.0, &states.0, sym),
            QbnInput::ItemUsed { quest, item } => rt.0.on_use_item(&lib.0, &mut states.0, quest, item, now),
            QbnInput::FoeInjured(foe) => rt.0.on_foe_injured(&lib.0, &mut states.0, foe, now),
            QbnInput::SetGlobal(name) => {
                rt.0.set_global(name);
                Vec::new()
            }
            QbnInput::PromptAnswer { quest, label } => {
                let pending = rt.0.pending_prompt_quests().contains(quest);
                let fx = rt.0.answer_prompt(&lib.0, &mut states.0, quest, label, now);
                info!(
                    "[quest] risposta `{label}` a `{quest}`: offerta in attesa={pending}, effetti={}, fase={:?}",
                    fx.len(),
                      states.0.phase(quest)
                );
                fx
            }
            QbnInput::FireTask { quest, task } => rt.0.fire_task(&lib.0, &mut states.0, quest, task, now),
        };
        publish(fx, &rt.0, &states.0, &mut out);
    }
}

#[allow(clippy::too_many_arguments)]
fn poll_runtime(
    time: Res<Time>,
    mut acc: Local<f32>,
    clock: Res<QbnClock>,
    mut world: ResMut<QbnWorldInfo>,
    rep: Res<Reputation>,
    items: Res<QuestItems>,
    lib: Res<QuestLibRes>,
    mut rt: ResMut<QbnRuntimeRes>,
    mut states: ResMut<QuestStatesRes>,
    mut out: EventWriter<QbnEffectEvent>,
) {
    *acc += time.delta_secs();
    if *acc < 0.5 {
        return;
    }
    *acc = 0.0;
    world.items = items.symbols();
    refresh_world_repute(&mut world, &rep, &rt.0, &lib.0, &states.0);
    let fx = rt.0.poll(&lib.0, &mut states.0, &*world, clock.seconds);
    publish(fx, &rt.0, &states.0, &mut out);
}

/// Copia la reputazione del giocatore nella vista che il runtime interroga
/// (`repute with _akorithi_ exceeds 0`): simbolo -> valore, per le persone delle quest attive.
/// Se due quest usano lo stesso simbolo per persone diverse vince l'ultima (limite noto).
fn refresh_world_repute(
    world: &mut QbnWorldInfo,
    rep: &Reputation,
    rt: &QbnRuntime,
    lib: &Library,
    states: &QuestStates,
) {
    world.repute.clear();
    for script in lib.scripts().filter(|s| rt.is_running(&s.id)) {
        for p in &script.persons {
            let (key, _) = rep_key(lib, states, &script.id, &p.symbol);
            world.repute.insert(p.symbol.clone(), rep.get(&key));
        }
    }
}

/// Applica gli effetti. Qui c'e' solo cio' che si puo' fare senza conoscere il resto del
/// progetto; i PNG sono gestiti da `qbn_npc` (legge `placements()` dal runtime).
fn apply_effects(
    mut events: EventReader<QbnEffectEvent>,
    mut journal: ResMut<QuestJournal>,
    mut rep: ResMut<Reputation>,
    mut say: ResMut<QbnSayQueue>,
    mut inputs: EventWriter<QbnInput>,
    mut toasts: ResMut<Toasts>,
    dialog: Res<DialogState>,
    lib: Res<QuestLibRes>,
    states: Res<QuestStatesRes>,
) {
    for QbnEffectEvent(e) in events.read() {
        match e {
            Effect::QuestStarted { quest } => info!("Quest `{quest}` avviata"),
            Effect::LogAdd { quest, step, text } => journal.set(quest, *step, text),
            Effect::LogRemove { quest, step } => journal.remove(quest, *step),
            Effect::QuestEnded { quest, failed } => {
                info!("Quest `{quest}` conclusa (fallita: {failed})");
                journal.clear_quest(quest);
            }
            Effect::StartQuest { number, .. } => match script_id_for_number(&lib.0, *number) {
                Some(id) => {
                    inputs.send(QbnInput::StartQuest(id));
                }
                None => warn!("Quest: `start quest {number}` ma lo script non e' nella libreria"),
            },
            Effect::Say { text, .. } => {
                if dialog.open {
                    say.0.push_back(text.clone());
                } else {
                    toasts.push(text.clone());
                }
            }
            // I PNG: `qbn_npc` li ricava da `QbnRuntime::placements()`; nessuna azione qui.
            Effect::PlaceNpc { person, place, .. } => info!("[quest] `{person}` piazzato in `{place}`"),
            Effect::MuteNpc { person, .. } => info!("[quest] `{person}` ora e' muto"),
            Effect::Questor { quest, person } => info!("[quest] questor di `{quest}`: {person:?}"),
            Effect::Prompt { quest, .. } => info!("[quest] offerta pronta per `{quest}`"),
            // Oro e permanenza li gestisce `quest_items`; qui solo l'avviso al giocatore.
            Effect::GiveGold { amount, .. } => toasts.push(format!("You receive {amount} gold pieces.")),
            Effect::MakePermanent { quest, item } => info!("[quest] `{item}` ({quest}) e' permanente"),
            // Il nemico viene creato da `spawn_quest_foes` (in `main_quest.rs`).
            Effect::SpawnFoe { foe, template, .. } => info!("[quest] nemico `{foe}` ({template}) in arrivo"),
            // L'oggetto entra nello zaino da `quest_items`; qui solo l'avviso (`notify N`).
            Effect::GiveItem { item, notify, .. } => {
                info!("[quest] ricevuto `{item}`");
                if let Some(n) = notify {
                    toasts.push(n.clone());
                }
            }
            // Lo stato lo tiene `quest_items`; il pickup 3D nel dungeon e' da agganciare.
            Effect::PlaceItem { item, place, .. } => info!("[quest] `{item}` piazzato in `{place}`"),
            Effect::Read { item, paragraphs, .. } => {
                let title = crate::quest_items::display_name(item, "letter");
                if journal.add_document(&title, paragraphs.clone()) {
                    toasts.push(format!("{title}: aggiunto al giornale (J)."));
                } else {
                    toasts.push(format!("{title}: aggiornato nel giornale (J)."));
                }
            }
            // TODO(hook): segna il luogo sulla mappa (`map.rs` / `local_map.rs`).
            Effect::Reveal { place, detail, .. } => info!("[quest] rivela `{place}` ({detail})"),
            Effect::Repute { quest, who, delta } => {
                let (key, name) = rep_key(&lib.0, &states.0, quest, who);
                let now = rep.add(&key, &name, *delta);
                info!("[quest] reputazione con {name}: {delta:+} -> {now}");
            }
            Effect::LegalRepute { delta, .. } => {
                let now = rep.add_legal(*delta);
                info!("[quest] reputazione legale: {delta:+} -> {now}");
            }
        }
    }
}

/// Per dialog.rs: l'offerta pendente di una quest QBN, se `npc_symbol` e' chi la offre.
pub fn main_quest_offer(
    rt: &QbnRuntime,
    lib: &Library,
    states: &QuestStates,
    npc_symbol: &str,
) -> Option<ScriptedOffer> {
    // 1. Quest attive con prompt pendente.
    for quest in rt.pending_prompt_quests() {
        if states.phase(&quest) != QuestPhase::Active {
            continue;
        }
        let Some(questor) = offer_questor(lib, &quest) else { continue };
        if !questor.eq_ignore_ascii_case(npc_symbol) {
            continue;
        }
        if let Some((text, _)) = rt.pending_prompt(lib, states, &quest) {
            let name = lib.get(&quest).map(|s| s.display_name.clone()).unwrap_or_default();
            return Some(ScriptedOffer { quest_id: quest, _quest_name: name, offer_text: text });
        }
    }

    // 2. Quest non ancora avviate: se il PNG è il questor atteso e ha un
    //    `QuestorOffer`, mostra l'offerta. `dialog::handle_scripted_offer`
    //    avvierà la quest quando il giocatore accetta/rifiuta.
    for script in lib.scripts() {
        if states.phase(&script.id) != QuestPhase::NotStarted {
            continue;
        }
        if script.message_named("QuestorOffer").is_none() {
            continue;
        }
        let Some(questor) = offer_questor(lib, &script.id) else { continue };
        if !questor.eq_ignore_ascii_case(npc_symbol) {
            continue;
        }
        let offer_msg = script.message_named("QuestorOffer")?;
        let variants = offer_msg.plain_variants();
        let first = variants.first()?;
        let offer_text = lib.expand(&script.id, states, first);
        return Some(ScriptedOffer {
            quest_id: script.id.clone(),
                    _quest_name: script.display_name.clone(),
                    offer_text,
        });
    }
    None
}

// =============================================================================
// Debug (solo build debug)
// =============================================================================

/// F6  = fa scattare `_S.13_` (arriva la lettera senza aspettare 3-7 giorni)
/// F7  = clic sull'agente dell'Underking (`_S.14_`: rivela la cripta)
/// F8  = raccogli il certificato di morte (simula il pickup nella cripta; prima serve F10)
/// F9  = +10 reputazione con la regina Akorithi e livello 7 (serve a far partire la quest 17)
/// F10 = accetta l'offerta (`yes`)
/// F12 = clic su Lhotun (consegna il certificato se ce l'hai)
/// (F1, F2 e F11 sono gia' usati altrove nel progetto: cambia i tasti se collidono.)
fn debug_quest_keys(
    keys: Res<ButtonInput<KeyCode>>,
    mut inputs: EventWriter<QbnInput>,
    mut pickups: EventWriter<QuestItemPickup>,
    mut world: ResMut<QbnWorldInfo>,
    mut rep: ResMut<Reputation>,
) {
    if keys.just_pressed(KeyCode::F6) {
        inputs.send(QbnInput::FireTask { quest: MAIN_QUEST_ID.into(), task: "_S.13_".into() });
    }
    if keys.just_pressed(KeyCode::F7) {
        inputs.send(QbnInput::NpcClicked("_agentuk_".into()));
    }
    if keys.just_pressed(KeyCode::F8) {
        pickups.send(QuestItemPickup { symbol: "deathCertificate".into() });
    }
    if keys.just_pressed(KeyCode::F9) {
        rep.add("Queen_Akorithi", "Queen Akorithi", 10);
        world.level = 7;
    }
    if keys.just_pressed(KeyCode::F10) {
        inputs.send(QbnInput::PromptAnswer { quest: MAIN_QUEST_ID.into(), label: "yes".into() });
    }
    if keys.just_pressed(KeyCode::F12) {
        inputs.send(QbnInput::NpcClicked("_lhotun_".into()));
    }
}
