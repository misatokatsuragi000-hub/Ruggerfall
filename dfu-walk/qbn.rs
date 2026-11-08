//! Interprete dei task QBN (esecuzione delle quest principali `S*.txt`).
//!
//! Modulo PURO: solo `std` + `crate::dfquest`. Niente Bevy, niente I/O di rete.
//! Il mondo di gioco gli parla tramite due trait (`QbnWorld`, `SymbolResolver`) e
//! riceve indietro una lista di `Effect` da applicare (giornale, inventario, PNG, mappa...).
//!
//! ## Modello (come in Daggerfall Unity)
//!
//! - Un task e' un elenco di azioni. Le azioni che sono *condizioni* (`toting X and Y clicked`,
//!   `clicked npc X`, `when A and B`, `level N completed`) sono il suo innesco; le altre sono
//!   *effetti*. Un task con condizioni scatta quando UNA qualsiasi e' vera; senza condizioni
//!   scatta solo se lo avvia qualcuno (un orologio con lo stesso simbolo, una risposta a un
//!   `prompt`, un osservatore `repute ... do`).
//! - Un task scatta una sola volta. Una `variable` e' un task vuoto: serve solo a essere "acceso".
//! - Gli orologi (`Clock _S.13_ 3.00:00 7.00:00`) hanno una durata casuale tra min e max e,
//!   scaduti, avviano il task con lo stesso simbolo.
//!
//! ## Assunzioni da verificare contro il sorgente di DFU
//!
//! 1. Le condizioni di un task sono in OR.
//! 2. `repute with X exceeds N do T` registra un osservatore che avvia T quando la reputazione
//!    supera N (e non e' una condizione del task che lo contiene).
//! 3. `end quest` = completata, tranne nei task "di fallimento" (vedi `is_failure_task`).
//! 4. `QuestComplete` non e' citato da nessuna istruzione: lo mostro quando si consegna la
//!    ricompensa (`give pc` di un oggetto di tipo `magic_item`).
//! 5. `clicked item X say N` e' persistente (non marca il task come scattato).
//! 6. `when A and not B`, `when A or B`: espressione in forma "OR di AND" (vedi `WhenExpr`).
//! 7. `get item X` dà l'oggetto NON permanente (a fine quest va tolto), `give pc X` lo dà
//!    permanente; `make X permanent` lo rende permanente. Un `get item` di un oggetto `gold`
//!    (`Item _I.06_ gold range 100 to 100`) dà oro, non un oggetto.
//! 8. Orologi con `flag` e `range`: se il flag ha il bit 16 (es. `flag 17`) alla durata si somma
//!    il tempo di viaggio (`set_default_travel_days`, 4 giorni se nessuno lo imposta) piu'
//!    `range a b` giorni a caso. Senza il bit 16 il `range` si ignora (`flag 1` = tempo esatto).
//!    `=1stparton_` nei testi diventa i giorni dell'orologio.
//! 9. Un `end quest` e' un FALLIMENTO se lo scatena un orologio senza che il giocatore riceva
//!    nulla (tempo scaduto), oltre ai casi di `is_failure_task`.
//! 10. Variabili globali (`ElysannaSatisfied _S.06_`): le accende il gioco con `set_global`.

use crate::dfquest::{
    Library, MsgRef, PersonDef, PlaceDef, QuestPhase, QuestScript, QuestStates, Stmt, TaskKind,
    TopicRef,
};
use std::collections::{HashMap, HashSet};

// =============================================================================
// Azioni
// =============================================================================

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    // ---- condizioni
    Toting { item: String, person: String },
    ClickedNpc(String),
    ClickedItem { item: String, say: Option<MsgRef> },
    When(WhenExpr),
    PlayerLevel(u32),
    /// `injured _F.00_ saying 1025`: il nemico viene ferito (lo segnala il gioco con `on_foe_injured`).
    Injured { foe: String, say: Option<MsgRef> },
    // ---- effetti
    Say(MsgRef),
    Log { message: MsgRef, step: u32 },
    RemoveLog { step: u32 },
    GiveItem { item: String, notify: Option<MsgRef> },
    PlaceItem { item: String, place: String },
    PlaceNpc { person: String, place: String },
    Mute(String),
    Reveal { place: String, detail: String },
    ChangeRepute { who: String, delta: i32 },
    LegalRepute(i32),
    AddDialog(TopicRef),
    DialogLink(TopicRef),
    AddQuestor(String),
    DropQuestor(String),
    StartTimer(String),
    StartQuest(Vec<u32>),
    EndQuest,
    WatchRepute { who: String, above: i32, task: String },
    /// `prompt QuestorOffer yes yes no _no_`: messaggio + coppie (etichetta, task).
    Prompt { message: MsgRef, options: Vec<(String, String)> },
    /// `pick one of _S.13_ _S.24_`: ne accende uno a caso.
    PickOneOf(Vec<String>),
    /// `clear _S.01_ _clearclick_`: spegne task/variabili, che potranno scattare di nuovo.
    Clear(Vec<String>),
    /// `letter39 used do _S.07_`: quando l'oggetto viene letto, parte il task.
    UsedDo { item: String, task: String },
    /// `get item X`: l'oggetto passa al giocatore (NON permanente, a differenza di `give pc`).
    GetItem { item: String },
    /// `make X permanent`: l'oggetto resta al giocatore a quest finita.
    MakePermanent(String),
    /// `create npc at P`: un residente anonimo; ignorato (serve solo il `place npc` che segue).
    CreateNpc { place: String },
    /// `create foe F every N minutes M times with P% success`.
    CreateFoe { foe: String, every_minutes: f64, times: u32, success_pct: u32 },
    Unknown(String),
}

fn msg_ref(token: &str) -> MsgRef {
    token.parse().map(MsgRef::Id).unwrap_or_else(|_| MsgRef::Name(token.to_string()))
}

fn msg_ref_key(r: &MsgRef) -> String {
    match r {
        MsgRef::Id(n) => n.to_string(),
        MsgRef::Name(s) => s.clone(),
    }
}

/// Espressione di `when`: OR di AND di (negato, simbolo).
/// `when A and not B` = [[(false,A),(true,B)]]; `when A or B` = [[(false,A)],[(false,B)]].
#[derive(Clone, Debug, PartialEq)]
pub struct WhenExpr(pub Vec<Vec<(bool, String)>>);

impl WhenExpr {
    pub fn eval(&self, fired: &HashSet<String>) -> bool {
        self.0
            .iter()
            .any(|group| !group.is_empty() && group.iter().all(|(neg, s)| fired.contains(s) != *neg))
    }
}

fn parse_when(tokens: &[&str]) -> Option<WhenExpr> {
    let mut groups: Vec<Vec<(bool, String)>> = vec![Vec::new()];
    let mut negate = false;
    for t in tokens {
        match t.to_ascii_lowercase().as_str() {
            "and" => {}
            "or" => groups.push(Vec::new()),
            "not" => negate = !negate,
            _ => {
                if let Some(g) = groups.last_mut() {
                    g.push((negate, t.to_string()));
                }
                negate = false;
            }
        }
    }
    groups.retain(|g| !g.is_empty());
    if groups.is_empty() {
        None
    } else {
        Some(WhenExpr(groups))
    }
}

impl Action {
    pub fn is_condition(&self) -> bool {
        matches!(
            self,
            Action::Toting { .. }
                | Action::ClickedNpc(_)
                | Action::ClickedItem { .. }
                | Action::When(_)
                | Action::PlayerLevel(_)
                | Action::Injured { .. }
        )
    }

    pub fn parse(raw: &str) -> Action {
        let toks: Vec<&str> = raw.split_whitespace().collect();
        let low: Vec<String> = toks.iter().map(|t| t.to_ascii_lowercase()).collect();
        let w = |i: usize| low.get(i).map(String::as_str).unwrap_or("");
        let tk = |i: usize| toks.get(i).copied().unwrap_or("").to_string();
        let n = toks.len();

        let parsed: Option<Action> = match w(0) {
            "toting" if n >= 4 && w(2) == "and" => {
                Some(Action::Toting { item: tk(1), person: tk(3) })
            }
            "clicked" if n >= 3 && w(1) == "npc" => Some(Action::ClickedNpc(tk(2))),
            "clicked" if n >= 3 && w(1) == "item" => Some(Action::ClickedItem {
                item: tk(2),
                say: if n >= 5 && w(3) == "say" { Some(msg_ref(toks[4])) } else { None },
            }),
            "when" if n >= 2 => parse_when(&toks[1..]).map(Action::When),
            "level" if n >= 2 => toks[1].parse().ok().map(Action::PlayerLevel),
            "repute" if n >= 7 && w(1) == "with" && w(3) == "exceeds" && w(5) == "do" => toks[4]
                .parse()
                .ok()
                .map(|above| Action::WatchRepute { who: tk(2), above, task: tk(6) }),
            "give" if n >= 3 && w(1) == "pc" => Some(Action::GiveItem {
                item: tk(2),
                notify: if n >= 5 && w(3) == "notify" { Some(msg_ref(toks[4])) } else { None },
            }),
            "remove" if n >= 4 && w(1) == "log" && w(2) == "step" => {
                toks[3].parse().ok().map(|step| Action::RemoveLog { step })
            }
            "place" if n >= 5 && w(1) == "item" && w(3) == "at" => {
                Some(Action::PlaceItem { item: tk(2), place: tk(4) })
            }
            "place" if n >= 5 && w(1) == "npc" && w(3) == "at" => {
                Some(Action::PlaceNpc { person: tk(2), place: tk(4) })
            }
            "mute" if n >= 3 && w(1) == "npc" => Some(Action::Mute(tk(2))),
            "reveal" if n >= 2 => {
                Some(Action::Reveal { place: tk(1), detail: toks[2..].join(" ") })
            }
            "change" if n >= 6 && w(1) == "repute" && w(2) == "with" && w(4) == "by" => {
                toks[5].parse().ok().map(|delta| Action::ChangeRepute { who: tk(3), delta })
            }
            "legal" if n >= 3 && w(1) == "repute" => toks[2].parse().ok().map(Action::LegalRepute),
            "add" if n >= 4 && w(2) == "as" && w(3) == "questor" => Some(Action::AddQuestor(tk(1))),
            "drop" if n >= 4 && w(2) == "as" && w(3) == "questor" => {
                Some(Action::DropQuestor(tk(1)))
            }
            "start" if n >= 3 && w(1) == "timer" => Some(Action::StartTimer(tk(2))),
            "prompt" if n >= 2 => {
                let options: Vec<(String, String)> = toks[2..]
                    .chunks(2)
                    .filter(|c| c.len() == 2)
                    .map(|c| (c[0].to_string(), c[1].to_string()))
                    .collect();
                Some(Action::Prompt { message: msg_ref(toks[1]), options })
            }
            "pick" if n >= 4 && w(1) == "one" && w(2) == "of" => {
                Some(Action::PickOneOf(toks[3..].iter().map(|x| x.to_string()).collect()))
            }
            "clear" if n >= 2 => Some(Action::Clear(toks[1..].iter().map(|x| x.to_string()).collect())),
            "get" if n >= 3 && w(1) == "item" => Some(Action::GetItem { item: tk(2) }),
            "make" if n >= 3 && w(2) == "permanent" => Some(Action::MakePermanent(tk(1))),
            "create" if n >= 4 && w(1) == "npc" && w(2) == "at" => Some(Action::CreateNpc { place: tk(3) }),
            "create" if n >= 3 && w(1) == "foe" => {
                let find = |key: &str| low.iter().position(|x| x == key);
                // unita' dopo il numero: minutes (default), hours, days, seconds -> minuti
                let unit = |num_idx: usize| match w(num_idx + 1) {
                    "hours" | "hour" => 60.0,
                    "days" | "day" => 1440.0,
                    "seconds" | "second" => 1.0 / 60.0,
                    _ => 1.0,
                };
                let every = find("every")
                    .and_then(|i| toks.get(i + 1).and_then(|v| v.parse::<f64>().ok()).map(|v| v * unit(i + 1)));
                let times = find("times")
                    .and_then(|i| i.checked_sub(1))
                    .and_then(|j| toks.get(j))
                    .and_then(|v| v.parse::<u32>().ok());
                let pct = toks
                    .iter()
                    .find(|t| t.ends_with('%'))
                    .and_then(|t| t.trim_end_matches('%').parse::<u32>().ok());
                Some(Action::CreateFoe {
                    foe: tk(2),
                    every_minutes: every.unwrap_or(0.0),
                    times: times.unwrap_or(1),
                    success_pct: pct.unwrap_or(100),
                })
            }
            "injured" if n >= 2 => Some(Action::Injured {
                foe: tk(1),
                say: if n >= 4 && w(2) == "saying" { Some(msg_ref(toks[3])) } else { None },
            }),
            _ if n >= 4 && w(1) == "used" && w(2) == "do" => Some(Action::UsedDo { item: tk(0), task: tk(3) }),
            _ => None,
        };
        if let Some(a) = parsed {
            return a;
        }
        match Stmt::parse(raw) {
            Stmt::Say(m) => Action::Say(m),
            Stmt::Log { message, step } => Action::Log { message, step },
            Stmt::DialogLink(t) => Action::DialogLink(t),
            Stmt::AddDialog(t) => Action::AddDialog(t),
            Stmt::StartQuest(ids) => Action::StartQuest(ids),
            Stmt::EndQuest => Action::EndQuest,
            Stmt::Other(s) => Action::Unknown(s),
        }
    }
}

fn parse_all(stmts: &[String]) -> Vec<Action> {
    stmts.iter().map(|s| Action::parse(s)).collect()
}

/// Istruzioni di uno script che l'interprete non riconosce (da loggare all'avvio).
pub fn unsupported(script: &QuestScript) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    let all = script.startup.iter().chain(script.tasks.iter().flat_map(|t| t.statements.iter()));
    for s in all {
        if let Action::Unknown(raw) = Action::parse(s) {
            v.push(raw);
        }
    }
    v
}

// =============================================================================
// Effetti verso il mondo
// =============================================================================

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Testo da mostrare al giocatore (popup / dialogo).
    Say { quest: String, text: String },
    LogAdd { quest: String, step: u32, text: String },
    LogRemove { quest: String, step: u32 },
    /// `permanent`: `give pc` (o ricompensa magica) = true; `get item` = false (si toglie a fine quest).
    GiveItem { quest: String, item: String, notify: Option<String>, permanent: bool },
    /// Oro al giocatore (`get item _I.06_` con un oggetto di tipo `gold`).
    GiveGold { quest: String, amount: u32 },
    /// `make X permanent`: l'oggetto non va tolto a quest finita.
    MakePermanent { quest: String, item: String },
    /// Un nemico da far comparire vicino al giocatore (`create foe ...`); `template` e' il tipo
    /// dello script (`Knight`, `Battle_mage`).
    SpawnFoe { quest: String, foe: String, template: String },
    PlaceItem { quest: String, item: String, place: String },
    PlaceNpc { quest: String, person: String, place: String },
    MuteNpc { quest: String, person: String },
    Reveal { quest: String, place: String, detail: String },
    Repute { quest: String, who: String, delta: i32 },
    LegalRepute { quest: String, delta: i32 },
    Questor { quest: String, person: Option<String> },
    /// Offerta da mostrare (di solito quando si parla con il questor).
    Prompt { quest: String, text: String, options: Vec<(String, String)> },
    /// Testo di un oggetto letto dal giocatore (lettera, certificato...), a paragrafi.
    Read { quest: String, item: String, paragraphs: Vec<String> },
    /// La quest e' stata istanziata.
    QuestStarted { quest: String },
    /// `start quest N ...`: chi sta in ascolto chiama `start_quest(script_id_for_number(N))`.
    StartQuest { quest: String, number: u32 },
    QuestEnded { quest: String, failed: bool },
}

// =============================================================================
// Trait verso il mondo
// =============================================================================

pub trait QbnWorld {
    /// Il giocatore ha in inventario l'oggetto della quest con questo simbolo?
    fn has_item(&self, item_symbol: &str) -> bool;
    /// Reputazione corrente del giocatore con la persona (simbolo QBN).
    fn repute(&self, person_symbol: &str) -> i32;
    fn player_level(&self) -> u32;
}

/// Sceglie i nomi concreti all'avvio della quest (prima che si espandano le macro).
pub trait SymbolResolver {
    /// Nome per una `Person` senza `named` (es. `_brother_`).
    fn person_name(&mut self, quest: &str, def: &PersonDef) -> String;
    /// Luogo scelto per un `Place`: (nome, citta' opzionale per `__sym_`).
    fn place(&mut self, quest: &str, def: &PlaceDef) -> (String, Option<String>);
}

// =============================================================================
// Runtime
// =============================================================================

/// `create foe ... every N minutes M times with P% success` in corso.
#[derive(Clone, Debug, PartialEq)]
struct Spawner {
    foe: String,
    next: f64,
    every: f64,
    left: u32,
    pct: u32,
}

#[derive(Clone, Debug, Default)]
struct Instance {
    /// Task / variabili scattati (e il marcatore interno "!reward").
    fired: HashSet<String>,
    /// Orologi attivi: simbolo -> scadenza (secondi di gioco assoluti).
    clocks: HashMap<String, f64>,
    /// Osservatori di reputazione: (persona, soglia, task da avviare).
    watchers: Vec<(String, i32, String)>,
    questor: Option<String>,
    prompt: Option<(MsgRef, Vec<(String, String)>)>,
    /// `place npc X at P` gia' eseguiti: (persona, luogo). Servono a ri-piazzare i PNG dopo un load.
    placed: Vec<(String, String)>,
    /// PNG a cui e' stato fatto `mute npc`.
    muted: HashSet<String>,
    /// `X used do T`: quando l'oggetto X viene letto, parte il task T.
    used_watch: Vec<(String, String)>,
    /// `create foe ...` ancora in corso.
    spawners: Vec<Spawner>,
    /// Giorni degli orologi, per `=1stparton_` nei testi: simbolo -> "5".
    clock_text: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct QbnRuntime {
    inst: HashMap<String, Instance>,
    rng: u64,
    last_now: f64,
    /// Variabili globali accese dal gioco (`ElysannaSatisfied`...).
    globals: HashSet<String>,
    /// Giorni di viaggio da sommare agli orologi col bit 16 (`flag 17`).
    travel_days: f64,
}

impl Default for QbnRuntime {
    fn default() -> Self {
        QbnRuntime::new(0x9E37_79B9_7F4A_7C15)
    }
}

fn parse_dhm(tok: &str) -> Option<f64> {
    let (d, rest) = match tok.split_once('.') {
        Some((d, r)) => (d.parse::<f64>().ok()?, r),
        None => (0.0, tok),
    };
    let (h, m) = rest.split_once(':')?;
    Some(((d * 24.0 + h.parse::<f64>().ok()?) * 60.0 + m.parse::<f64>().ok()?) * 60.0)
}

/// Una riga `Clock`: durata (min..max), flag e range.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ClockSpec {
    pub lo: f64,
    pub hi: f64,
    pub flags: u32,
    /// `range a b`: giorni in piu' a caso (contano solo col bit 16 dei flag).
    pub range: Option<(f64, f64)>,
}

/// `"1.00:00 0 flag 1 range 0 1"`, `"00:00 0 flag 17 range 0 2"`, `"3.00:00 7.00:00"`.
pub fn parse_clock(spec: &str) -> ClockSpec {
    let toks: Vec<&str> = spec.split_whitespace().collect();
    let times: Vec<f64> = toks.iter().map_while(|t| parse_dhm(t)).take(2).collect();
    let (lo, hi) = match times.as_slice() {
        [a, b] => (*a, *b),
        [a] => (*a, *a),
        _ => (0.0, 0.0),
    };
    let after = |key: &str, k: usize| {
        toks.iter().position(|t| t.eq_ignore_ascii_case(key)).and_then(|i| toks.get(i + k)).copied()
    };
    let flags = after("flag", 1).and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
    let range = match (
        after("range", 1).and_then(|v| v.parse::<f64>().ok()),
        after("range", 2).and_then(|v| v.parse::<f64>().ok()),
    ) {
        (Some(a), Some(b)) => Some((a.min(b), a.max(b))),
        _ => None,
    };
    ClockSpec { lo: lo.min(hi), hi: lo.max(hi), flags, range }
}

/// `"3.00:00 7.00:00"` -> (min, max) in secondi.
pub fn clock_range(spec: &str) -> (f64, f64) {
    let c = parse_clock(spec);
    (c.lo, c.hi)
}

/// `=1stparton_` -> i giorni dell'orologio (prima di `lib.expand`, che altrimenti scrive "a few").
fn subst_clocks(text: &str, clocks: &HashMap<String, String>) -> String {
    let mut out = text.to_string();
    for (sym, val) in clocks {
        out = out.replace(&format!("={}_", sym.trim_matches('_')), val);
    }
    out
}

fn msg_text(
    lib: &Library,
    states: &QuestStates,
    quest: &str,
    r: &MsgRef,
    clocks: Option<&HashMap<String, String>>,
) -> Option<String> {
    let script = lib.get(quest)?;
    let m = script.resolve_ref(r)?;
    let mut first = m.plain_variants().into_iter().next()?;
    if let Some(c) = clocks {
        first = subst_clocks(&first, c);
    }
    Some(lib.expand(quest, states, &first))
}

/// Task il cui `end quest` e' un fallimento: l'opzione "no" di un `prompt`, un task che abbassa
/// la reputazione / il livello di ricercato, o un orologio scaduto senza ricompensa.
fn is_failure_task(script: &QuestScript, task: &str) -> bool {
    for a in parse_all(&script.startup) {
        if let Action::Prompt { options, .. } = a {
            if options.iter().any(|(l, t)| l.eq_ignore_ascii_case("no") && t == task) {
                return true;
            }
        }
    }
    let Some(t) = script.tasks.iter().find(|t| t.symbol == task) else { return false };
    let acts = parse_all(&t.statements);
    let negative = acts.iter().any(|a| {
        matches!(a, Action::ChangeRepute { delta, .. } if *delta < 0) || matches!(a, Action::LegalRepute(d) if *d < 0)
    });
    // scaduto un orologio senza che il giocatore abbia ricevuto nulla: tempo scaduto
    let timed_out = script.clocks.iter().any(|c| c.symbol == task)
        && !acts.iter().any(|a| matches!(a, Action::GiveItem { .. } | Action::GetItem { .. }));
    negative || timed_out
}

/// Il messaggio `id` come paragrafi: le righe a capo fisso si uniscono, le righe vuote separano.
/// Le macro (`_brother_`, `___ukcrypt_`...) sono espanse.
pub fn message_paragraphs(lib: &Library, states: &QuestStates, quest: &str, id: u32) -> Vec<String> {
    let Some(script) = lib.get(quest) else { return Vec::new() };
    let Some(m) = script.resolve_ref(&MsgRef::Id(id)) else { return Vec::new() };
    let Some(lines) = m.variants.first() else { return Vec::new() };
    let mut paras: Vec<String> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for l in lines {
        let t = l.trim().trim_start_matches("<ce>").trim();
        if t.is_empty() {
            if !cur.is_empty() {
                paras.push(cur.join(" "));
                cur.clear();
            }
        } else {
            cur.push(t);
        }
    }
    if !cur.is_empty() {
        paras.push(cur.join(" "));
    }
    paras.into_iter().map(|p| lib.expand(quest, states, &p)).collect()
}

/// Chi fa l'offerta: la persona che il task `yes` aggiunge come questor
/// (`add _lhotun_ as questor`). Prima dell'accettazione `QbnRuntime::questor` e' ancora None.
pub fn offer_questor(lib: &Library, quest: &str) -> Option<String> {
    let script = lib.get(quest)?;
    let task = script.tasks.iter().find(|t| t.symbol.eq_ignore_ascii_case("yes"))?;
    parse_all(&task.statements).into_iter().find_map(|a| match a {
        Action::AddQuestor(p) => Some(p),
        _ => None,
    })
}

/// Da `start quest 17 17` al simbolo dello script (`S0000017`).
pub fn script_id_for_number(lib: &Library, number: u32) -> Option<String> {
    if let Some(s) = lib.scripts().find(|s| s.quest_id == Some(number)) {
        return Some(s.id.clone());
    }
    let guess = format!("S{number:07}");
    lib.get(&guess).map(|s| s.id.clone())
}

impl QbnRuntime {
    pub fn new(seed: u64) -> Self {
        QbnRuntime { inst: HashMap::new(), rng: seed.max(1), last_now: 0.0, globals: HashSet::new(), travel_days: 4.0 }
    }

    pub fn is_running(&self, quest: &str) -> bool {
        self.inst.contains_key(quest)
    }

    pub fn last_now(&self) -> f64 {
        self.last_now
    }

    pub fn questor(&self, quest: &str) -> Option<&str> {
        self.inst.get(quest)?.questor.as_deref()
    }

    pub fn has_fired(&self, quest: &str, task: &str) -> bool {
        self.inst.get(quest).map_or(false, |i| i.fired.contains(task))
    }

    /// Tutti i PNG piazzati da `place npc`: (quest, persona, luogo).
    pub fn placements(&self) -> Vec<(String, String, String)> {
        let mut v = Vec::new();
        for (q, i) in &self.inst {
            for (p, pl) in &i.placed {
                v.push((q.clone(), p.clone(), pl.clone()));
            }
        }
        v.sort();
        v
    }

    pub fn is_muted(&self, quest: &str, person: &str) -> bool {
        self.inst.get(quest).map_or(false, |i| i.muted.contains(person))
    }

    /// Quest istanziate con un `prompt` ancora senza risposta.
    pub fn pending_prompt_quests(&self) -> Vec<String> {
        let mut v: Vec<String> =
            self.inst.iter().filter(|(_, i)| i.prompt.is_some()).map(|(q, _)| q.clone()).collect();
        v.sort();
        v
    }

    /// Variabile globale (`ElysannaSatisfied`): la accende il gioco; i task `Global` collegati
    /// scattano al primo `poll` (anche per quest avviate dopo).
    pub fn set_global(&mut self, name: &str) {
        self.globals.insert(name.to_string());
    }

    pub fn is_global_set(&self, name: &str) -> bool {
        self.globals.contains(name)
    }

    /// Giorni di viaggio per gli orologi col bit 16 (`flag 17`). Default 4.
    pub fn set_default_travel_days(&mut self, days: f64) {
        self.travel_days = days.max(0.0);
    }

    fn clock_text(&self, quest: &str) -> Option<&HashMap<String, String>> {
        self.inst.get(quest).map(|i| &i.clock_text)
    }

    fn next_unit(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Inserisce la chiave; `true` se era nuova.
    fn once(&mut self, quest: &str, key: &str) -> bool {
        self.inst.entry(quest.to_string()).or_default().fired.insert(key.to_string())
    }

    // ------------------------------------------------------------------ avvio

    /// Istanzia una quest: sceglie i nomi, la rende attiva ed esegue il blocco di avvio.
    pub fn start_quest(
        &mut self,
        lib: &Library,
        states: &mut QuestStates,
        resolver: &mut dyn SymbolResolver,
        quest: &str,
        now: f64,
    ) -> Vec<Effect> {
        let mut out = Vec::new();
        let Some(script) = lib.get(quest) else { return out };
        if self.inst.contains_key(quest) {
            return out;
        }
        self.inst.insert(quest.to_string(), Instance::default());
        for p in &script.persons {
            if p.name.is_none() {
                let name = resolver.person_name(quest, p);
                states.set_symbol(quest, &p.symbol, &name, None);
            }
        }
        for pl in &script.places {
            let (name, town) = resolver.place(quest, pl);
            states.set_symbol(quest, &pl.symbol, &name, town.as_deref());
        }
        states.set_phase(script, QuestPhase::Active);
        out.push(Effect::QuestStarted { quest: quest.to_string() });
        for a in parse_all(&script.startup) {
            self.exec(lib, states, quest, "", a, now, &mut out);
        }
        out
    }

    // ------------------------------------------------------------------ esecuzione

    #[allow(clippy::too_many_arguments)]
    fn exec(
        &mut self,
        lib: &Library,
        states: &mut QuestStates,
        quest: &str,
        task: &str,
        action: Action,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        let Some(script) = lib.get(quest) else { return };
        let q = quest.to_string();
        match action {
            Action::Say(r) => {
                if let Some(text) = msg_text(lib, states, quest, &r, self.clock_text(quest)) {
                    out.push(Effect::Say { quest: q, text });
                }
            }
            Action::Log { message, step } => {
                if let Some(text) = msg_text(lib, states, quest, &message, self.clock_text(quest)) {
                    out.push(Effect::LogAdd { quest: q, step, text });
                }
            }
            Action::RemoveLog { step } => out.push(Effect::LogRemove { quest: q, step }),
            Action::GiveItem { item, notify } => self.give_item(lib, states, quest, item, notify, true, out),
            Action::GetItem { item } => self.give_item(lib, states, quest, item, None, false, out),
            Action::MakePermanent(item) => out.push(Effect::MakePermanent { quest: q, item }),
            Action::CreateNpc { .. } => {}
            Action::PickOneOf(list) => {
                if !list.is_empty() {
                    let i = ((self.next_unit() * list.len() as f64) as usize).min(list.len() - 1);
                    self.fire_into(lib, states, quest, &list[i], now, out);
                }
            }
            Action::Clear(list) => {
                if let Some(inst) = self.inst.get_mut(quest) {
                    for sym in &list {
                        inst.fired.remove(sym);
                    }
                }
            }
            Action::UsedDo { item, task } => {
                let i = self.inst.entry(q).or_default();
                if !i.used_watch.iter().any(|(a, b)| *a == item && *b == task) {
                    i.used_watch.push((item, task));
                }
            }
            Action::CreateFoe { foe, every_minutes, times, success_pct } => {
                self.inst.entry(q).or_default().spawners.push(Spawner {
                    foe,
                    next: now,
                    every: every_minutes * 60.0,
                    left: times.max(1),
                    pct: success_pct,
                });
            }
            Action::PlaceItem { item, place } => out.push(Effect::PlaceItem { quest: q, item, place }),
            Action::PlaceNpc { person, place } => {
                let i = self.inst.entry(q.clone()).or_default();
                if !i.placed.iter().any(|(p, _)| *p == person) {
                    i.placed.push((person.clone(), place.clone()));
                }
                out.push(Effect::PlaceNpc { quest: q, person, place });
            }
            Action::Mute(person) => {
                self.inst.entry(q.clone()).or_default().muted.insert(person.clone());
                out.push(Effect::MuteNpc { quest: q, person });
            }
            Action::Reveal { place, detail } => out.push(Effect::Reveal { quest: q, place, detail }),
            Action::ChangeRepute { who, delta } => out.push(Effect::Repute { quest: q, who, delta }),
            Action::LegalRepute(delta) => out.push(Effect::LegalRepute { quest: q, delta }),
            Action::AddDialog(t) | Action::DialogLink(t) => states.enable_topic(quest, &t.symbol),
            Action::AddQuestor(p) => {
                self.inst.entry(q.clone()).or_default().questor = Some(p.clone());
                out.push(Effect::Questor { quest: q, person: Some(p) });
            }
            Action::DropQuestor(_) => {
                self.inst.entry(q.clone()).or_default().questor = None;
                out.push(Effect::Questor { quest: q, person: None });
            }
            Action::StartTimer(c) => {
                let spec = script
                    .clocks
                    .iter()
                    .find(|k| k.symbol == c)
                    .map(|k| parse_clock(&k.spec))
                    .unwrap_or_default();
                let mut secs = spec.lo + (spec.hi - spec.lo) * self.next_unit();
                if spec.flags & 16 != 0 {
                    // durata legata al viaggio: tempo di viaggio + range a caso (giorni)
                    secs += self.travel_days * 86_400.0;
                    if let Some((a, b)) = spec.range {
                        secs += (a + (b - a) * self.next_unit()) * 86_400.0;
                    }
                }
                let days = (secs / 86_400.0).ceil().max(1.0) as u64;
                let inst = self.inst.entry(q).or_default();
                inst.clock_text.insert(c.clone(), days.to_string());
                inst.clocks.insert(c, now + secs);
            }
            Action::StartQuest(ids) => {
                if let Some(&number) = ids.first() {
                    out.push(Effect::StartQuest { quest: q, number });
                }
            }
            Action::EndQuest => {
                let failed = is_failure_task(script, task);
                states.set_phase(script, if failed { QuestPhase::Failed } else { QuestPhase::Completed });
                if let Some(i) = self.inst.get_mut(quest) {
                    i.clocks.clear();
                    i.watchers.clear();
                    i.prompt = None;
                    i.spawners.clear();
                    i.used_watch.clear();
                }
                out.push(Effect::QuestEnded { quest: q, failed });
            }
            Action::WatchRepute { who, above, task } => {
                self.inst.entry(q).or_default().watchers.push((who, above, task));
            }
            Action::Prompt { message, options } => {
                let text = msg_text(lib, states, quest, &message, self.clock_text(quest)).unwrap_or_default();
                self.inst.entry(q.clone()).or_default().prompt = Some((message, options.clone()));
                out.push(Effect::Prompt { quest: q, text, options });
            }
            // condizioni: sono l'innesco, non si eseguono
            Action::Toting { .. }
            | Action::ClickedNpc(_)
            | Action::ClickedItem { .. }
            | Action::When(_)
            | Action::PlayerLevel(_)
            | Action::Injured { .. }
            | Action::Unknown(_) => {}
        }
    }

    /// `give pc` (`permanent`) e `get item` (non permanente). Un oggetto `gold` da' oro.
    #[allow(clippy::too_many_arguments)]
    fn give_item(
        &mut self,
        lib: &Library,
        states: &mut QuestStates,
        quest: &str,
        item: String,
        notify: Option<MsgRef>,
        permanent: bool,
        out: &mut Vec<Effect>,
    ) {
        let Some(script) = lib.get(quest) else { return };
        let def = script.items.iter().find(|i| i.symbol == item);
        let kind = def.map(|d| d.kind.to_ascii_lowercase()).unwrap_or_default();
        if kind == "gold" {
            let (lo, hi) = def.and_then(|d| d.range).unwrap_or((0, 0));
            let amount = lo + (hi.saturating_sub(lo) as f64 * self.next_unit()) as u32;
            out.push(Effect::GiveGold { quest: quest.to_string(), amount });
            return;
        }
        let reward = kind == "magic_item";
        if reward && self.once(quest, "!reward") {
            let name = MsgRef::Name("QuestComplete".to_string());
            if let Some(text) = msg_text(lib, states, quest, &name, self.clock_text(quest)) {
                out.push(Effect::Say { quest: quest.to_string(), text });
            }
        }
        let notify = notify.and_then(|r| msg_text(lib, states, quest, &r, self.clock_text(quest)));
        out.push(Effect::GiveItem { quest: quest.to_string(), item, notify, permanent: permanent || reward });
    }

    fn fire_into(
        &mut self,
        lib: &Library,
        states: &mut QuestStates,
        quest: &str,
        task_sym: &str,
        now: f64,
        out: &mut Vec<Effect>,
    ) {
        let Some(script) = lib.get(quest) else { return };
        if states.phase(quest) != QuestPhase::Active {
            return;
        }
        let Some(task) = script.tasks.iter().find(|t| t.symbol == task_sym) else { return };
        if !self.once(quest, task_sym) {
            return;
        }
        for a in parse_all(&task.statements) {
            if a.is_condition() {
                continue;
            }
            self.exec(lib, states, quest, task_sym, a, now, out);
        }
    }

    /// Fa scattare un task a mano.
    pub fn fire_task(
        &mut self,
        lib: &Library,
        states: &mut QuestStates,
        quest: &str,
        task_sym: &str,
        now: f64,
    ) -> Vec<Effect> {
        let mut out = Vec::new();
        self.fire_into(lib, states, quest, task_sym, now, &mut out);
        out
    }

    // ------------------------------------------------------------------ prompt

    /// L'offerta in attesa di risposta: (testo espanso, opzioni `(etichetta, task)`).
    pub fn pending_prompt(
        &self,
        lib: &Library,
        states: &QuestStates,
        quest: &str,
    ) -> Option<(String, Vec<(String, String)>)> {
        let (msg, opts) = self.inst.get(quest)?.prompt.as_ref()?;
        let text = msg_text(lib, states, quest, msg, self.clock_text(quest)).unwrap_or_default();
        Some((text, opts.clone()))
    }

    /// Risposta a un `prompt` (`"yes"` / `"no"`): avvia il task collegato.
    pub fn answer_prompt(
        &mut self,
        lib: &Library,
        states: &mut QuestStates,
        quest: &str,
        label: &str,
        now: f64,
    ) -> Vec<Effect> {
        let task = self
            .inst
            .get_mut(quest)
            .and_then(|i| i.prompt.take())
            .and_then(|(_, opts)| opts.into_iter().find(|(l, _)| l.eq_ignore_ascii_case(label)))
            .map(|(_, t)| t);
        match task {
            Some(t) => self.fire_task(lib, states, quest, &t, now),
            None => Vec::new(),
        }
    }

    // ------------------------------------------------------------------ eventi di gioco

    /// Da chiamare a ogni tick (basta ogni mezzo secondo): orologi, osservatori, `when`, `level`.
    pub fn poll(
        &mut self,
        lib: &Library,
        states: &mut QuestStates,
        world: &dyn QbnWorld,
        now: f64,
    ) -> Vec<Effect> {
        self.last_now = now;
        let mut out = Vec::new();
        for _ in 0..16 {
            let mut progressed = false;
            let quests: Vec<String> = self.inst.keys().cloned().collect();
            for q in quests {
                if states.phase(&q) != QuestPhase::Active {
                    continue;
                }
                let Some(script) = lib.get(&q) else { continue };

                // 0. variabili globali accese dal gioco (`ElysannaSatisfied _S.06_`)
                for task in script.tasks.iter().filter(|t| t.kind == TaskKind::Global) {
                    let Some(name) = task.global_name.as_deref() else { continue };
                    if self.globals.contains(name) && !self.inst[&q].fired.contains(&task.symbol) {
                        self.fire_into(lib, states, &q, &task.symbol, now, &mut out);
                        progressed = true;
                    }
                }

                // 0b. nemici da creare (`create foe ... every N minutes M times with P% success`)
                let pending = self.inst.get_mut(&q).map(|i| std::mem::take(&mut i.spawners)).unwrap_or_default();
                let mut keep: Vec<Spawner> = Vec::new();
                for mut sp in pending {
                    if sp.next <= now {
                        if self.next_unit() * 100.0 < f64::from(sp.pct) {
                            let template = script
                                .foes
                                .iter()
                                .find(|f| f.symbol == sp.foe)
                                .map(|f| f.template.clone())
                                .unwrap_or_default();
                            out.push(Effect::SpawnFoe { quest: q.clone(), foe: sp.foe.clone(), template });
                        }
                        sp.left = sp.left.saturating_sub(1);
                        sp.next = now + sp.every;
                        if sp.left == 0 {
                            continue;
                        }
                    }
                    keep.push(sp);
                }
                if let Some(i) = self.inst.get_mut(&q) {
                    i.spawners = keep;
                }

                // 1. orologi scaduti
                let due: Vec<String> = self.inst[&q]
                    .clocks
                    .iter()
                    .filter(|(_, t)| **t <= now)
                    .map(|(s, _)| s.clone())
                    .collect();
                for c in due {
                    if let Some(i) = self.inst.get_mut(&q) {
                        i.clocks.remove(&c);
                    }
                    self.fire_into(lib, states, &q, &c, now, &mut out);
                    progressed = true;
                }

                // 2. osservatori di reputazione
                let watchers = self.inst[&q].watchers.clone();
                for (who, above, task) in watchers {
                    if world.repute(&who) > above {
                        if let Some(i) = self.inst.get_mut(&q) {
                            i.watchers.retain(|w| !(w.0 == who && w.2 == task));
                        }
                        self.fire_into(lib, states, &q, &task, now, &mut out);
                        progressed = true;
                    }
                }

                // 3. task con condizioni di stato (when / level)
                for task in script.tasks.iter().filter(|t| t.kind == TaskKind::Task) {
                    if self.inst[&q].fired.contains(&task.symbol) {
                        continue;
                    }
                    let hit = parse_all(&task.statements).iter().any(|c| match c {
                        Action::When(expr) => expr.eval(&self.inst[&q].fired),
                        Action::PlayerLevel(n) => world.player_level() >= *n,
                        _ => false,
                    });
                    if hit {
                        self.fire_into(lib, states, &q, &task.symbol, now, &mut out);
                        progressed = true;
                    }
                }
            }
            if !progressed {
                break;
            }
        }
        out
    }

    /// Il giocatore ha cliccato un PNG (simbolo QBN, es. `_lhotun_`).
    pub fn on_click_npc(
        &mut self,
        lib: &Library,
        states: &mut QuestStates,
        world: &dyn QbnWorld,
        person: &str,
        now: f64,
    ) -> Vec<Effect> {
        let mut out = Vec::new();
        let quests: Vec<String> = self.inst.keys().cloned().collect();
        for q in quests {
            if states.phase(&q) != QuestPhase::Active {
                continue;
            }
            let Some(script) = lib.get(&q) else { continue };
            for task in script.tasks.iter().filter(|t| t.kind == TaskKind::Task) {
                if self.inst[&q].fired.contains(&task.symbol) {
                    continue;
                }
                let hit = parse_all(&task.statements).into_iter().any(|a| match a {
                    Action::ClickedNpc(p) => p.eq_ignore_ascii_case(person),
                    Action::Toting { item, person: p } => {
                        p.eq_ignore_ascii_case(person) && world.has_item(&item)
                    }
                    _ => false,
                });
                if hit {
                    self.fire_into(lib, states, &q, &task.symbol, now, &mut out);
                }
            }
        }
        out
    }

    /// Clic su un oggetto della quest nell'inventario (`clicked item X say N`, persistente).
    pub fn on_click_item(&self, lib: &Library, states: &QuestStates, item: &str) -> Vec<Effect> {
        let mut out = Vec::new();
        for (q, _) in self.inst.iter().filter(|(q, _)| states.phase(q) == QuestPhase::Active) {
            let Some(script) = lib.get(q) else { continue };
            for task in &script.tasks {
                for a in parse_all(&task.statements) {
                    if let Action::ClickedItem { item: i, say: Some(r) } = a {
                        if i.eq_ignore_ascii_case(item) {
                            if let Some(text) = msg_text(lib, states, q, &r, self.clock_text(q)) {
                                out.push(Effect::Say { quest: q.clone(), text });
                            }
                        }
                    }
                }
            }
        }
        out
    }

    /// Il giocatore legge un oggetto di quest: il testo e' il messaggio `used N` della sua
    /// definizione (anche a quest conclusa: si puo' rileggere la lettera) e, se un task ha
    /// registrato `ITEM used do TASK`, quel task parte (una volta).
    pub fn on_use_item(
        &mut self,
        lib: &Library,
        states: &mut QuestStates,
        quest: &str,
        item: &str,
        now: f64,
    ) -> Vec<Effect> {
        let mut out = Vec::new();
        let used = lib
            .get(quest)
            .and_then(|s| s.items.iter().find(|d| d.symbol == item))
            .and_then(|d| d.used_message);
        if let Some(id) = used {
            let paragraphs = message_paragraphs(lib, states, quest, id);
            if !paragraphs.is_empty() {
                out.push(Effect::Read { quest: quest.to_string(), item: item.to_string(), paragraphs });
            }
        }
        let tasks: Vec<String> = self
            .inst
            .get_mut(quest)
            .map(|i| {
                let (hit, rest): (Vec<(String, String)>, Vec<(String, String)>) =
                    std::mem::take(&mut i.used_watch).into_iter().partition(|(it, _)| it.as_str() == item);
                i.used_watch = rest;
                hit.into_iter().map(|(_, t)| t).collect()
            })
            .unwrap_or_default();
        for t in tasks {
            self.fire_into(lib, states, quest, &t, now, &mut out);
        }
        out
    }

    /// Un nemico creato dalla quest e' stato ferito (`injured _F.00_ saying 1025`).
    pub fn on_foe_injured(&mut self, lib: &Library, states: &mut QuestStates, foe: &str, now: f64) -> Vec<Effect> {
        let mut out = Vec::new();
        let quests: Vec<String> = self.inst.keys().cloned().collect();
        for q in quests {
            if states.phase(&q) != QuestPhase::Active {
                continue;
            }
            let Some(script) = lib.get(&q) else { continue };
            for task in script.tasks.iter().filter(|t| t.kind == TaskKind::Task) {
                if self.inst[&q].fired.contains(&task.symbol) {
                    continue;
                }
                let said = parse_all(&task.statements).into_iter().find_map(|a| match a {
                    Action::Injured { foe: f, say } if f.eq_ignore_ascii_case(foe) => Some(say),
                    _ => None,
                });
                if let Some(say) = said {
                    self.fire_into(lib, states, &q, &task.symbol, now, &mut out);
                    if let Some(r) = say {
                        if let Some(text) = msg_text(lib, states, &q, &r, self.clock_text(&q)) {
                            out.push(Effect::Say { quest: q.clone(), text });
                        }
                    }
                }
            }
        }
        out
    }

    // ------------------------------------------------------------------ persistenza

    pub fn to_text(&self) -> String {
        let mut lines: Vec<String> = vec![format!("now {}", self.last_now)];
        for (q, i) in &self.inst {
            lines.push(format!("inst {q}"));
            for t in &i.fired {
                lines.push(format!("fired {q} {t}"));
            }
            for (c, d) in &i.clocks {
                lines.push(format!("clock {q} {c} {d}"));
            }
            for (w, a, t) in &i.watchers {
                lines.push(format!("watch {q} {w} {a} {t}"));
            }
            if let Some(p) = &i.questor {
                lines.push(format!("questor {q} {p}"));
            }
            for (p, pl) in &i.placed {
                lines.push(format!("placed {q} {p} {pl}"));
            }
            for p in &i.muted {
                lines.push(format!("muted {q} {p}"));
            }
            if let Some((m, opts)) = &i.prompt {
                let o: Vec<String> = opts.iter().map(|(l, t)| format!("{l}={t}")).collect();
                lines.push(format!("prompt {q} {} {}", msg_ref_key(m), o.join(",")));
            }
            for (it, t) in &i.used_watch {
                lines.push(format!("used {q} {it} {t}"));
            }
            for sp in &i.spawners {
                lines.push(format!("spawn {q} {} {} {} {} {}", sp.foe, sp.next, sp.every, sp.left, sp.pct));
            }
            for (c, d) in &i.clock_text {
                lines.push(format!("clocktext {q} {c} {d}"));
            }
        }
        for g in &self.globals {
            lines.push(format!("global {g}"));
        }
        lines.sort();
        let mut s = String::from("# Stato runtime QBN (now / inst / fired / clock / watch / questor / prompt)\n");
        s.push_str(&lines.join("\n"));
        s.push('\n');
        s
    }

    pub fn from_text(text: &str) -> QbnRuntime {
        let mut rt = QbnRuntime::default();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let p: Vec<&str> = line.split_whitespace().collect();
            match p.as_slice() {
                ["now", v] => rt.last_now = v.parse().unwrap_or(0.0),
                ["inst", q] => {
                    rt.inst.entry(q.to_string()).or_default();
                }
                ["fired", q, t] => {
                    rt.inst.entry(q.to_string()).or_default().fired.insert(t.to_string());
                }
                ["clock", q, c, d] => {
                    if let Ok(d) = d.parse::<f64>() {
                        rt.inst.entry(q.to_string()).or_default().clocks.insert(c.to_string(), d);
                    }
                }
                ["watch", q, w, a, t] => {
                    if let Ok(a) = a.parse::<i32>() {
                        rt.inst
                            .entry(q.to_string())
                            .or_default()
                            .watchers
                            .push((w.to_string(), a, t.to_string()));
                    }
                }
                ["questor", q, p] => {
                    rt.inst.entry(q.to_string()).or_default().questor = Some(p.to_string());
                }
                ["placed", q, p, pl] => {
                    rt.inst
                        .entry(q.to_string())
                        .or_default()
                        .placed
                        .push((p.to_string(), pl.to_string()));
                }
                ["muted", q, p] => {
                    rt.inst.entry(q.to_string()).or_default().muted.insert(p.to_string());
                }
                ["used", q, it, t] => {
                    rt.inst
                        .entry(q.to_string())
                        .or_default()
                        .used_watch
                        .push((it.to_string(), t.to_string()));
                }
                ["spawn", q, foe, next, every, left, pct] => {
                    if let (Ok(next), Ok(every), Ok(left), Ok(pct)) =
                        (next.parse::<f64>(), every.parse::<f64>(), left.parse::<u32>(), pct.parse::<u32>())
                    {
                        rt.inst.entry(q.to_string()).or_default().spawners.push(Spawner {
                            foe: foe.to_string(),
                            next,
                            every,
                            left,
                            pct,
                        });
                    }
                }
                ["clocktext", q, c, d] => {
                    rt.inst.entry(q.to_string()).or_default().clock_text.insert(c.to_string(), d.to_string());
                }
                ["global", g] => {
                    rt.globals.insert(g.to_string());
                }
                ["prompt", q, m, opts] => {
                    let options: Vec<(String, String)> = opts
                        .split(',')
                        .filter_map(|o| o.split_once('='))
                        .map(|(l, t)| (l.to_string(), t.to_string()))
                        .collect();
                    rt.inst.entry(q.to_string()).or_default().prompt = Some((msg_ref(m), options));
                }
                _ => {}
            }
        }
        rt
    }
}

// =============================================================================
// Test
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::dfquest::pretty_name;

    const SAMPLE: &str = r#"-- QuestId: 1
Quest: S0000001
DisplayName: Missing Prince
QRC:

QuestorOffer:  [1000]
<ce> Welcome to Sentinel, %pcf. Will you help me?

RefuseQuest:  [1001]
<ce> %oth, %pcf, I thought you were a hero.

AcceptQuest:  [1002]
<ce> Thank you. Find proof of what happened to _brother_.

QuestComplete:  [1004]
<ce> Poor _brother_. Take this _magicitem_.

Message:  1011
<ce> A thin woman hands you a note.

Message:  1018
<ce> Blackmail? Poor Prince _brother_ died of a fever.

Message:  1019
Lhotun asked me to investigate the death of _brother_.

Message:  1020
<ce> You pick up a death certificate.

Message:  1032
Dear %pcf, meet me at _agentplace_ in __agentplace_ of Sentinel.

Message:  1035
<ce> Check into ___ukcrypt_. I'll mark it on your map.

Message:  1040
I met an agent. I am supposed to check out ___ukcrypt_.

QBN:
Item deathCertificate letter used 1010
Item letter38 letter used 1032
Item _magicitem_ magic_item

Person _lhotun_ named Prince_Lhotun atHome
Person _akorithi_ named Queen_Akorithi atHome
Person _greklith_ face 88 named Prince_Greklith atHome
Person Vhosek face 88 named Lord_Vhosek atHome
Person _brother_ face 5 faction Children male remote anyInfo 1030 rumors 1031
Person _agentuk_ face 1 faction Agents_of_The_Underking remote

Place _ukcrypt_ randompermanent Castle_Faallem,Fortress_Fhojum anyInfo 1015 rumors 1016
Place _dirennitower_ permanent DirenniTowerEntry
Place _agentplace_ remote tavern

Clock _S.13_ 3.00:00 7.00:00

    dialog link for location _ukcrypt_
    dialog link for location _dirennitower_
    prompt QuestorOffer yes yes no _no_
    place npc _agentuk_ at _agentplace_

_S.00_ task:
    toting deathCertificate and _lhotun_ clicked
    give pc _magicitem_
    add dialog for location _dirennitower_
    remove log step 0
    remove log step 1
    reveal _dirennitower_ in province 9 at 204718
    repute with _akorithi_ exceeds 0 do _S.16_
    change repute with _lhotun_ by +15
    change repute with _agentuk_ by +25
    mute npc _lhotun_
    mute npc _greklith_
    mute npc _akorithi_
    mute npc Vhosek
    drop _lhotun_ as questor

variable _S.01_
_S.05_ task:
    toting deathCertificate and _akorithi_ clicked
    toting deathCertificate and Vhosek clicked
    toting deathCertificate and _greklith_ clicked
    say 1018
    change repute with _akorithi_ by -20
    legal repute -20
    end quest

_S.10_ task:
    clicked item deathCertificate say 1020

yes task:
    place item deathCertificate at _ukcrypt_
    start timer _S.13_
    log 1019 step 0
    say AcceptQuest
    add _lhotun_ as questor

_no_ task:
    say RefuseQuest
    drop _lhotun_ as questor
    end quest

_S.13_ task:
    give pc letter38 notify 1011

_S.14_ task:
    clicked npc _agentuk_
    add dialog for location _ukcrypt_
    reveal _ukcrypt_
    log 1040 step 1
    say 1035

_S.15_ task:
    level 7 completed

variable _S.16_
_S.17_ task:
    when _S.16_ and _S.15_
    start quest 17 17
    end quest
"#;

    #[derive(Default)]
    struct W {
        items: HashSet<String>,
        rep: HashMap<String, i32>,
        level: u32,
    }
    impl QbnWorld for W {
        fn has_item(&self, s: &str) -> bool {
            self.items.contains(s)
        }
        fn repute(&self, s: &str) -> i32 {
            *self.rep.get(s).unwrap_or(&0)
        }
        fn player_level(&self) -> u32 {
            self.level
        }
    }

    struct R;
    impl SymbolResolver for R {
        fn person_name(&mut self, _q: &str, _d: &PersonDef) -> String {
            "Cassim".into()
        }
        fn place(&mut self, _q: &str, d: &PlaceDef) -> (String, Option<String>) {
            let n = d.targets.first().map(|t| pretty_name(t)).unwrap_or_else(|| "Somewhere".into());
            (n, Some("Dragontail".into()))
        }
    }

    fn setup() -> (Library, QuestStates, QbnRuntime) {
        let script = QuestScript::parse(SAMPLE).expect("parse");
        let mut lib = Library::default();
        lib.insert(script);
        (lib, QuestStates::default(), QbnRuntime::default())
    }

    const Q: &str = "S0000001";

    #[test]
    fn every_statement_is_recognised() {
        let script = QuestScript::parse(SAMPLE).unwrap();
        let unknown = unsupported(&script);
        assert!(unknown.is_empty(), "istruzioni non riconosciute: {unknown:?}");
    }

    #[test]
    fn clock_range_parses_days() {
        assert_eq!(clock_range("3.00:00 7.00:00"), (259_200.0, 604_800.0));
        assert_eq!(clock_range("00:30"), (1800.0, 1800.0));
    }

    #[test]
    fn happy_path() {
        let (lib, mut st, mut rt) = setup();
        let mut w = W::default();

        let fx = rt.start_quest(&lib, &mut st, &mut R, Q, 0.0);
        assert_eq!(st.phase(Q), QuestPhase::Active);
        assert!(fx.iter().any(|e| matches!(e, Effect::Prompt { options, .. } if options.len() == 2)));
        assert!(fx.iter().any(|e| matches!(e, Effect::PlaceNpc { person, .. } if person == "_agentuk_")));
        assert!(rt.pending_prompt(&lib, &st, Q).is_some());

        // accetta
        let fx = rt.answer_prompt(&lib, &mut st, Q, "yes", 10.0);
        assert!(fx.iter().any(|e| matches!(e, Effect::PlaceItem { item, place, .. }
            if item == "deathCertificate" && place == "_ukcrypt_")));
        assert!(fx.iter().any(|e| matches!(e, Effect::LogAdd { step: 0, .. })));
        assert_eq!(rt.questor(Q), Some("_lhotun_"));
        assert!(rt.pending_prompt(&lib, &st, Q).is_none());

        // troppo presto: niente lettera
        assert!(rt.poll(&lib, &mut st, &w, 11.0).is_empty());
        // dopo 8 giorni: lettera di Lhotun
        let fx = rt.poll(&lib, &mut st, &w, 10.0 + 8.0 * 86_400.0);
        assert!(fx.iter().any(|e| matches!(e, Effect::GiveItem { item, notify: Some(_), .. }
            if item == "letter38")));

        // l'agente
        let fx = rt.on_click_npc(&lib, &mut st, &w, "_agentuk_", 1.0e6);
        assert!(st.is_topic_enabled(Q, "_ukcrypt_"));
        assert!(fx.iter().any(|e| matches!(e, Effect::Say { text, .. } if text.contains("Faallem"))));

        // consegna il certificato a Lhotun: prima QuestComplete, poi la ricompensa
        w.items.insert("deathCertificate".into());
        let fx = rt.on_click_npc(&lib, &mut st, &w, "_lhotun_", 1.0e6);
        assert!(matches!(fx[0], Effect::Say { .. }));
        assert!(fx.iter().any(|e| matches!(e, Effect::GiveItem { item, .. } if item == "_magicitem_")));
        assert_eq!(rt.questor(Q), None);

        // livello 7 + Akorithi benevola -> parte la quest 17 e questa si chiude
        w.level = 7;
        w.rep.insert("_akorithi_".into(), 5);
        let fx = rt.poll(&lib, &mut st, &w, 1.0e6 + 1.0);
        assert!(fx.iter().any(|e| matches!(e, Effect::StartQuest { number: 17, .. })));
        assert_eq!(st.phase(Q), QuestPhase::Completed);
    }

    #[test]
    fn refusing_fails_the_quest() {
        let (lib, mut st, mut rt) = setup();
        rt.start_quest(&lib, &mut st, &mut R, Q, 0.0);
        let fx = rt.answer_prompt(&lib, &mut st, Q, "no", 1.0);
        assert!(fx.iter().any(|e| matches!(e, Effect::QuestEnded { failed: true, .. })));
        assert_eq!(st.phase(Q), QuestPhase::Failed);
    }

    #[test]
    fn blackmail_fails_the_quest() {
        let (lib, mut st, mut rt) = setup();
        let mut w = W::default();
        w.items.insert("deathCertificate".into());
        rt.start_quest(&lib, &mut st, &mut R, Q, 0.0);
        rt.answer_prompt(&lib, &mut st, Q, "yes", 1.0);
        let fx = rt.on_click_npc(&lib, &mut st, &w, "_akorithi_", 2.0);
        assert!(fx.iter().any(|e| matches!(e, Effect::QuestEnded { failed: true, .. })));
        assert_eq!(st.phase(Q), QuestPhase::Failed);
    }

    #[test]
    fn click_item_is_persistent() {
        let (lib, mut st, mut rt) = setup();
        rt.start_quest(&lib, &mut st, &mut R, Q, 0.0);
        assert_eq!(rt.on_click_item(&lib, &st, "deathCertificate").len(), 1);
        assert_eq!(rt.on_click_item(&lib, &st, "deathCertificate").len(), 1);
    }

    #[test]
    fn placements_and_questor() {
        let (lib, mut st, mut rt) = setup();
        rt.start_quest(&lib, &mut st, &mut R, Q, 0.0);
        // lo startup piazza l'agente
        assert_eq!(rt.placements(), vec![(Q.to_string(), "_agentuk_".to_string(), "_agentplace_".to_string())]);
        // prima di accettare il questor e' ignoto, ma l'offerta sa chi lo sara'
        assert_eq!(rt.questor(Q), None);
        assert_eq!(offer_questor(&lib, Q).as_deref(), Some("_lhotun_"));
        assert_eq!(rt.pending_prompt_quests(), vec![Q.to_string()]);
        // il piazzamento sopravvive al salvataggio
        let back = QbnRuntime::from_text(&rt.to_text());
        assert_eq!(back.placements(), rt.placements());
    }

    #[test]
    fn mute_is_remembered() {
        let (lib, mut st, mut rt) = setup();
        let mut w = W::default();
        w.items.insert("deathCertificate".into());
        rt.start_quest(&lib, &mut st, &mut R, Q, 0.0);
        rt.answer_prompt(&lib, &mut st, Q, "yes", 1.0);
        rt.on_click_npc(&lib, &mut st, &w, "_lhotun_", 2.0);
        assert!(rt.is_muted(Q, "_lhotun_"));
        assert!(QbnRuntime::from_text(&rt.to_text()).is_muted(Q, "_lhotun_"));
    }

    #[test]
    fn reading_an_item_returns_paragraphs() {
        let (lib, mut st, mut rt) = setup();
        rt.start_quest(&lib, &mut st, &mut R, Q, 0.0);
        let fx = rt.on_use_item(&lib, &mut st, Q, "letter38", 0.0);
        assert!(matches!(&fx[..], [Effect::Read { paragraphs, .. }] if !paragraphs.is_empty()));
        // un oggetto senza `used` non produce nulla
        assert!(rt.on_use_item(&lib, &mut st, Q, "_magicitem_", 0.0).is_empty());
    }

    #[test]
    fn persistence_roundtrip() {
        let (lib, mut st, mut rt) = setup();
        rt.start_quest(&lib, &mut st, &mut R, Q, 0.0);
        rt.answer_prompt(&lib, &mut st, Q, "yes", 10.0);
        let a = rt.to_text();
        let b = QbnRuntime::from_text(&a).to_text();
        assert_eq!(a, b);
    }


    // ========================================================================
    // S0000002 "Blackmailing Prince Helseth": le istruzioni in piu' del primo script
    // ========================================================================

    const Q2: &str = "S0000002";

    const SAMPLE2: &str = r#"-- QuestId: 2
Quest: S0000002
DisplayName: Blackmailing Prince Helseth
QRC:

QuestorOffer:  [1000]
<ce> I need a courier, %pcf. Does this sound agreeable?

RefuseQuest:  [1001]
<ce> Dullard.

AcceptQuest:  [1002]
<ce> Excellent. Meet me here this time tomorrow.

QuestComplete:  [1004]
<ce> Here is the _magicitem_ I promised.

Message:  1010
My Dear Lord Castellian,
 
 First paragraph of the letter, which is
 wrapped by hand.
 
 Second paragraph.

Message:  1011
<ce> Well done.

Message:  1012
<ce> Too bad you were not faster.

Message:  1018
<ce> A courier shoves a leather pouch into your hands.

Message:  1019
Prince Helseth wants me to meet him tomorrow.

Message:  1020
I have =1stparton_ days to deliver the letter and return.

Message:  1024
Prince Helseth, I will meet you shortly.

Message:  1025
<ce> Lord Castellian has ordered your death.

Message:  1026
My Dear Lord Castellian, forgive this rough delivery.

Message:  1027
<ce> Here is the letter. You have =1stparton_ days.

Message:  1028
<ce> You wait while Lord Castellian reads the letter.

Message:  1030
<ce> This letter has been opened.

Message:  1031
<ce> So you like to read the missives of your betters.

Message:  1040
Dear friend, here are 100 gold pieces.

QBN:
Item letter39 letter used 1010
Item _magicitem_ magic_item
Item letter41 letter used 1024
Item letter42 letter used 1026
Item letter43 letter used 1040
Item _I.06_ gold range 100 to 100

Person _helseth_ named Prince_Helseth atHome
Person Castellian face 1 named Lord_Castellian anyInfo 1050
Person Eadwyre face 80 named King_Eadwyre atHome

Place _castfort_ remote house1

Clock _S.00_ 1.00:00 0 flag 1 range 0 1
Clock _1stparton_ 00:00 0 flag 17 range 0 2
Clock _S.15_ 3.00:00 7.00:00

Foe _F.00_ is Knight
Foe _battlemage_ is Battle_mage

    start timer _S.00_ 
    log 1019 step 0 
    prompt QuestorOffer yes yes no _no_ 
    pick one of _S.13_ _S.24_ 

_S.00_ task:
    create npc at _castfort_ 
    place npc Castellian at _castfort_ 

_S.01_ task:
    clicked npc _helseth_ 

_S.02_ task:
    toting letter42 and Castellian clicked 
    toting letter39 and Castellian clicked 

_S.03_ task:
    when _S.01_ and _S.00_ 
    add dialog for person Castellian 
    start timer _1stparton_ 
    log 1020 step 1 
    remove log step 0 
    say 1027 

_S.04_ task:
    toting letter41 and _helseth_ clicked 
    give pc _magicitem_ 
    change repute with _helseth_ by +10 
    drop Eadwyre as questor 
    drop _helseth_ as questor 

_S.05_ task:
    toting letter42 and Eadwyre clicked 
    toting letter39 and Eadwyre clicked 
    say 1014 
    change repute with Eadwyre by +5 

ElysannaSatisfied _S.06_
    start timer _S.15_ 

variable _S.07_
_S.08_ task:
    injured _F.00_ saying 1025 
    injured _battlemage_ saying 1025 

_clearclick_ task:
    when _S.01_ and not _S.03_ 
    clear _S.01_ _clearclick_ 

yes task:
    dialog link for person Castellian 
    say AcceptQuest 
    add _helseth_ as questor 
    add Eadwyre as questor 

_no_ task:
    say RefuseQuest 
    end quest 

_S.12_ task:
    when _S.03_ and _S.13_ 
    letter39 used do _S.07_ 
    get item letter39 

variable _S.13_
_S.14_ task:
    when _S.03_ and _S.24_ 
    letter42 used do _S.07_ 
    get item letter42 

_S.15_ task:
    get item letter43 
    get item _I.06_ 
    say 1018 
    make letter43 permanent 
    make _I.06_ permanent 
    drop Eadwyre as questor 
    drop _helseth_ as questor 
    end quest 

_1stparton_ task:
    drop Eadwyre as questor 
    drop _helseth_ as questor 
    end quest 

_S.17_ task:
    when _S.04_ and _S.06_ 
    say 1012 

_S.18_ task:
    when _S.04_ and not _S.06_ 
    say 1011 

_S.19_ task:
    when _S.02_ and _S.07_ 
    create foe _F.00_ every 4 minutes 6 times with 100% success 
    create foe _battlemage_ every 7 minutes 2 times with 100% success 
    say 1030 
    change repute with Castellian by -10 

_S.20_ task:
    when _S.02_ and not _S.07_ 
    letter41 used do _S.21_ 
    get item letter41 
    say 1028 

variable _S.21_
_S.22_ task:
    when _S.04_ and _S.21_ 
    say 1031 
    change repute with _helseth_ by -25 

_S.23_ task:
    when _S.04_ and not _S.21_ 
    start quest 11 11 

variable _S.24_
_S.25_ task:
    when _S.22_ or _S.23_ 
    end quest 
"#;

    fn setup2() -> (Library, QuestStates, QbnRuntime) {
        let script = QuestScript::parse(SAMPLE2).expect("parse");
        let mut lib = Library::default();
        lib.insert(script);
        (lib, QuestStates::default(), QbnRuntime::default())
    }

    /// Avvia la quest, accetta, aspetta il giorno, clicca Helseth e lascia scattare i task:
    /// restituisce (tempo, simbolo della lettera ricevuta, effetti dell'ultimo poll).
    fn s2_until_letter(
        lib: &Library,
        st: &mut QuestStates,
        rt: &mut QbnRuntime,
        w: &W,
    ) -> (f64, &'static str, Vec<Effect>) {
        rt.start_quest(lib, st, &mut R, Q2, 0.0);
        rt.answer_prompt(lib, st, Q2, "yes", 1.0);
        let t = 90_000.0; // oltre il giorno di attesa
        rt.poll(lib, st, w, t); // `_S.00_`: Castellian al maniero
        rt.on_click_npc(lib, st, w, "_helseth_", t + 1.0);
        let fx = rt.poll(lib, st, w, t + 2.0); // `_S.03_`, poi `_S.12_` o `_S.14_`
        let letter = if rt.has_fired(Q2, "_S.13_") { "letter39" } else { "letter42" };
        (t + 2.0, letter, fx)
    }

    #[test]
    fn s2_every_statement_is_recognised() {
        let script = QuestScript::parse(SAMPLE2).unwrap();
        let unknown = unsupported(&script);
        assert!(unknown.is_empty(), "istruzioni non riconosciute: {unknown:?}");
    }

    #[test]
    fn when_supports_not_and_or() {
        let fired: HashSet<String> = ["A".to_string(), "B".to_string()].into_iter().collect();
        let eval = |s: &str| match Action::parse(s) {
            Action::When(e) => e.eval(&fired),
            other => panic!("non e' un when: {other:?}"),
        };
        assert!(eval("when A and B"));
        assert!(eval("when A and not C"));
        assert!(!eval("when A and not B"));
        assert!(eval("when C or B"));
        assert!(!eval("when C or D"));
        assert!(eval("when C and D or A and B"));
    }

    #[test]
    fn new_instructions_parse() {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(Action::parse("pick one of _S.13_ _S.24_"), Action::PickOneOf(v(&["_S.13_", "_S.24_"])));
        assert_eq!(Action::parse("clear _S.01_ _clearclick_"), Action::Clear(v(&["_S.01_", "_clearclick_"])));
        assert_eq!(
            Action::parse("letter39 used do _S.07_"),
            Action::UsedDo { item: "letter39".into(), task: "_S.07_".into() }
        );
        assert_eq!(Action::parse("get item letter39"), Action::GetItem { item: "letter39".into() });
        assert_eq!(Action::parse("make _I.06_ permanent"), Action::MakePermanent("_I.06_".into()));
        assert_eq!(Action::parse("create npc at _castfort_"), Action::CreateNpc { place: "_castfort_".into() });
        assert_eq!(
            Action::parse("create foe _F.00_ every 4 minutes 6 times with 100% success"),
            Action::CreateFoe { foe: "_F.00_".into(), every_minutes: 4.0, times: 6, success_pct: 100 }
        );
        assert_eq!(
            Action::parse("injured _F.00_ saying 1025"),
            Action::Injured { foe: "_F.00_".into(), say: Some(MsgRef::Id(1025)) }
        );
    }

    #[test]
    fn clock_flags_and_range() {
        let c = parse_clock("00:00 0 flag 17 range 0 2");
        assert_eq!(c.flags, 17);
        assert_eq!(c.range, Some((0.0, 2.0)));
        assert_eq!(c.lo, 0.0);
        let d = parse_clock("1.00:00 0 flag 1 range 0 1");
        assert_eq!(d.lo, 86_400.0);
        assert_eq!(d.flags & 16, 0, "flag 1: tempo esatto, il range non conta");
        let e = parse_clock("3.00:00 7.00:00");
        assert_eq!((e.lo, e.hi, e.flags, e.range), (259_200.0, 604_800.0, 0, None));
    }

    #[test]
    fn s2_start_picks_one_variant_and_offers_the_quest() {
        let (lib, mut st, mut rt) = setup2();
        let fx = rt.start_quest(&lib, &mut st, &mut R, Q2, 0.0);
        assert!(fx.iter().any(|e| matches!(e, Effect::Prompt { .. })));
        assert_ne!(rt.has_fired(Q2, "_S.13_"), rt.has_fired(Q2, "_S.24_"), "ne scatta una sola");
        assert_eq!(offer_questor(&lib, Q2).as_deref(), Some("_helseth_"));
    }

    #[test]
    fn s2_delivering_the_letter_unread() {
        let (lib, mut st, mut rt) = setup2();
        let mut w = W::default();
        let (t, letter, fx) = s2_until_letter(&lib, &mut st, &mut rt, &w);
        assert!(rt.has_fired(Q2, "_S.03_"));
        assert!(rt.placements().iter().any(|(_, p, _)| p == "Castellian"), "Castellian va al maniero");
        // `get item`: la lettera e' un oggetto di quest NON permanente
        assert!(fx
            .iter()
            .any(|e| matches!(e, Effect::GiveItem { item, permanent: false, .. } if item == letter)));

        // consegna a Castellian: non l'ho letta -> risponde con una lettera per Helseth
        w.items.insert(letter.to_string());
        rt.on_click_npc(&lib, &mut st, &w, "Castellian", t + 10.0);
        assert!(rt.has_fired(Q2, "_S.02_"));
        let fx = rt.poll(&lib, &mut st, &w, t + 11.0);
        assert!(rt.has_fired(Q2, "_S.20_") && !rt.has_fired(Q2, "_S.19_"));
        assert!(fx.iter().any(|e| matches!(e, Effect::GiveItem { item, .. } if item == "letter41")));

        // ... che porto a Helseth: ricompensa, reputazione, e la quest 11 parte
        w.items.insert("letter41".into());
        let fx = rt.on_click_npc(&lib, &mut st, &w, "_helseth_", t + 20.0);
        assert!(rt.has_fired(Q2, "_S.04_"));
        assert!(fx.iter().any(|e| matches!(e, Effect::Repute { delta: 10, .. })));
        let say = fx.iter().position(|e| matches!(e, Effect::Say { .. })).expect("QuestComplete");
        let ring = fx
            .iter()
            .position(|e| matches!(e, Effect::GiveItem { item, permanent: true, .. } if item == "_magicitem_"))
            .expect("anello permanente");
        assert!(say < ring, "prima il testo di QuestComplete, poi la ricompensa");
        let fx = rt.poll(&lib, &mut st, &w, t + 21.0);
        assert!(rt.has_fired(Q2, "_S.18_"), "senza ElysannaSatisfied: `when ... and not _S.06_`");
        assert!(!rt.has_fired(Q2, "_S.17_"));
        assert!(fx.iter().any(|e| matches!(e, Effect::StartQuest { number: 11, .. })));
        assert_eq!(st.phase(Q2), QuestPhase::Completed);
    }

    #[test]
    fn s2_reading_the_letter_makes_castellian_hostile() {
        let (lib, mut st, mut rt) = setup2();
        let mut w = W::default();
        let (t, letter, _) = s2_until_letter(&lib, &mut st, &mut rt, &w);

        // leggerla avvia `_S.07_` (lo aveva registrato `_S.12_`/`_S.14_` con `used do`)
        let fx = rt.on_use_item(&lib, &mut st, Q2, letter, t + 5.0);
        assert!(fx.iter().any(|e| matches!(e, Effect::Read { .. })));
        assert!(rt.has_fired(Q2, "_S.07_"));

        w.items.insert(letter.to_string());
        rt.on_click_npc(&lib, &mut st, &w, "Castellian", t + 10.0);
        let fx = rt.poll(&lib, &mut st, &w, t + 11.0);
        assert!(rt.has_fired(Q2, "_S.19_") && !rt.has_fired(Q2, "_S.20_"));
        assert!(fx
            .iter()
            .any(|e| matches!(e, Effect::Repute { who, delta: -10, .. } if who == "Castellian")));
        // il primo cavaliere arriva subito
        assert!(fx.iter().any(|e| matches!(e, Effect::SpawnFoe { template, .. } if template == "Knight")));

        // dopo qualche minuto: altri nemici, tra cui il mago da battaglia
        let fx = rt.poll(&lib, &mut st, &w, t + 500.0);
        assert!(fx.iter().any(|e| matches!(e, Effect::SpawnFoe { template, .. } if template == "Knight")));
        assert!(fx.iter().any(|e| matches!(e, Effect::SpawnFoe { template, .. } if template == "Battle_mage")));

        // ferirne uno fa dire la frase di Castellian, una volta sola
        let fx = rt.on_foe_injured(&lib, &mut st, "_F.00_", t + 501.0);
        assert!(rt.has_fired(Q2, "_S.08_"));
        assert!(fx.iter().any(|e| matches!(e, Effect::Say { .. })));
        assert!(rt.on_foe_injured(&lib, &mut st, "_F.00_", t + 502.0).is_empty());
    }

    #[test]
    fn s2_clicking_helseth_too_early_is_forgotten() {
        let (lib, mut st, mut rt) = setup2();
        let w = W::default();
        rt.start_quest(&lib, &mut st, &mut R, Q2, 0.0);
        rt.answer_prompt(&lib, &mut st, Q2, "yes", 1.0);
        rt.on_click_npc(&lib, &mut st, &w, "_helseth_", 100.0);
        assert!(rt.has_fired(Q2, "_S.01_"));
        rt.poll(&lib, &mut st, &w, 200.0); // `_clearclick_`: `when _S.01_ and not _S.03_`
        assert!(!rt.has_fired(Q2, "_S.01_"));
        assert!(!rt.has_fired(Q2, "_clearclick_"), "si cancella da solo: puo' scattare ancora");
        assert!(!rt.has_fired(Q2, "_S.03_"));
        // passato il giorno, il clic vale
        rt.poll(&lib, &mut st, &w, 90_000.0);
        rt.on_click_npc(&lib, &mut st, &w, "_helseth_", 90_001.0);
        rt.poll(&lib, &mut st, &w, 90_002.0);
        assert!(rt.has_fired(Q2, "_S.03_"));
    }

    #[test]
    fn s2_the_deadline_shows_real_days_and_failing_ends_the_quest() {
        let (lib, mut st, mut rt) = setup2();
        let w = W::default();
        rt.start_quest(&lib, &mut st, &mut R, Q2, 0.0);
        rt.answer_prompt(&lib, &mut st, Q2, "yes", 1.0);
        rt.poll(&lib, &mut st, &w, 90_000.0);
        rt.on_click_npc(&lib, &mut st, &w, "_helseth_", 90_001.0);
        let fx = rt.poll(&lib, &mut st, &w, 90_002.0);
        let log = fx
            .iter()
            .find_map(|e| match e {
                Effect::LogAdd { step: 1, text, .. } => Some(text.clone()),
                _ => None,
            })
            .expect("annotazione dello step 1");
        // 4 giorni di viaggio + 0..2 di range: 4, 5 o 6, mai "a few"
        assert!(!log.contains("a few") && !log.contains('='), "{log}");
        assert!(["4", "5", "6"].iter().any(|d| log.contains(&format!("have {d} days"))), "{log}");

        // senza consegnare nulla, scade: `_1stparton_` e' una fine quest FALLITA
        let fx = rt.poll(&lib, &mut st, &w, 90_002.0 + 10.0 * 86_400.0);
        assert!(fx.iter().any(|e| matches!(e, Effect::QuestEnded { failed: true, .. })));
        assert_eq!(st.phase(Q2), QuestPhase::Failed);
    }

    #[test]
    fn s2_global_variable_starts_the_reward_clock() {
        let (lib, mut st, mut rt) = setup2();
        let w = W::default();
        rt.start_quest(&lib, &mut st, &mut R, Q2, 0.0);
        rt.answer_prompt(&lib, &mut st, Q2, "yes", 1.0);
        rt.poll(&lib, &mut st, &w, 10.0);
        assert!(!rt.has_fired(Q2, "_S.06_"), "finche' nessuno accende ElysannaSatisfied");
        rt.set_global("ElysannaSatisfied");
        rt.poll(&lib, &mut st, &w, 20.0);
        assert!(rt.has_fired(Q2, "_S.06_"));

        // 3-7 giorni dopo: la lettera di ringraziamento con 100 monete, tutto permanente
        let fx = rt.poll(&lib, &mut st, &w, 20.0 + 8.0 * 86_400.0);
        assert!(fx.iter().any(|e| matches!(e, Effect::GiveItem { item, .. } if item == "letter43")));
        assert!(fx.iter().any(|e| matches!(e, Effect::GiveGold { amount: 100, .. })));
        assert!(fx.iter().any(|e| matches!(e, Effect::MakePermanent { item, .. } if item == "letter43")));
        assert!(fx.iter().any(|e| matches!(e, Effect::QuestEnded { failed: false, .. })), "ricompensa = successo");
    }

    #[test]
    fn s2_state_survives_save_and_load() {
        let (lib, mut st, mut rt) = setup2();
        let w = W::default();
        let (t, letter, _) = s2_until_letter(&lib, &mut st, &mut rt, &w);
        rt.set_global("ElysannaSatisfied");
        let a = rt.to_text();
        assert!(a.contains("used S0000002"), "{a}");
        assert!(a.contains("clocktext S0000002 _1stparton_"), "{a}");
        assert!(a.contains("global ElysannaSatisfied"), "{a}");
        assert_eq!(QbnRuntime::from_text(&a).to_text(), a);

        // dopo il ricaricamento leggere la lettera avvia ancora `_S.07_`
        let mut back = QbnRuntime::from_text(&a);
        back.on_use_item(&lib, &mut st, Q2, letter, t + 5.0);
        assert!(back.has_fired(Q2, "_S.07_"));
    }

    #[test]
    fn gold_range_is_read_from_the_script() {
        let script = QuestScript::parse(SAMPLE2).unwrap();
        let gold = script.items.iter().find(|i| i.symbol == "_I.06_").expect("oggetto oro");
        assert_eq!(gold.kind, "gold");
        assert_eq!(gold.range, Some((100, 100)));
    }
}
