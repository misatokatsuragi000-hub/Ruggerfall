//! Lettore degli script dei quest di Daggerfall Unity (`S0000001.txt` ... i quest principali).
//!
//! Uno script ha due parti:
//!   - QRC: i testi. Messaggi numerati ("QuestorOffer: [1000]", "Message: 1011") con piu' varianti
//!     separate da `<--->`, le voci in giro (`RumorsDuringQuest`, `RumorsPostfailure`,
//!     `RumorsPostsuccess`) e le informazioni su persone e luoghi (`anyInfo`, `rumors`).
//!   - QBN: la logica. Definizioni (Item, Person, Place, Clock, Foe), poi i compiti (`_S.00_ task:`),
//!     le variabili e il blocco di avvio con le istruzioni (`say 1018`, `log 1019 step 0`,
//!     `dialog link for location _ukcrypt_`, `start quest 17 17`...).
//!
//! Questo modulo e' PURO (solo std, niente Bevy) e fa quattro cose:
//!   1. `QuestScript::parse` legge uno script in una struttura (messaggi, definizioni, compiti);
//!   2. `expand_macros` espande le macro dei testi (`%pcf`, `_brother_`, `___ukcrypt_`, `=1stparton_`);
//!   3. `QuestStates` tiene lo stato di ogni quest (non iniziato, attivo, fallito, completato), gli
//!      argomenti di dialogo abilitati e i nomi risolti a runtime, e si salva su testo;
//!   4. `knowledge_block` e `rumor_for` trasformano tutto questo in conoscenza per i PNG: quello che
//!      sanno e raccontano, in base allo stato delle quest. Il LLM la ripete a modo suo.
//!
//! L'ESECUZIONE dei compiti (condizioni, orologi, ricompense) non e' qui: chi la implementa legge i
//! compiti da `QuestScript::tasks` / `startup` e comunica lo stato con `QuestStates`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

pub const DEFAULT_QUESTS_DIR: &str =
"/mnt/1CE16DBE48849731/Daggerfall/DaggerfallUnity_Data/StreamingAssets/Quests";


// =============================================================================
// Messaggi (QRC)
// =============================================================================

/// Un messaggio dello script: una o piu' varianti (separate da `<--->`), ognuna fatta di righe.
#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub id: u32,
    /// "QuestorOffer", "RumorsDuringQuest"... oppure None per i `Message:` anonimi.
    pub name: Option<String>,
    pub variants: Vec<Vec<String>>,
}

impl Message {
    /// Le varianti come testo semplice: righe unite con uno spazio, spazi ridotti a uno.
    pub fn plain_variants(&self) -> Vec<String> {
        self.variants
            .iter()
            .map(|lines| lines.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" "))
            .filter(|t| !t.is_empty())
            .collect()
    }
}

/// Riferimento a un messaggio: per numero o per nome ("say QuestorOffer").
#[derive(Clone, Debug, PartialEq)]
pub enum MsgRef {
    Id(u32),
    Name(String),
}

impl MsgRef {
    fn parse(token: &str) -> MsgRef {
        token.parse().map(MsgRef::Id).unwrap_or_else(|_| MsgRef::Name(token.to_string()))
    }
}

// =============================================================================
// Definizioni (QBN)
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TopicKind {
    Person,
    Location,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct PersonDef {
    pub symbol: String,
    /// Nome fisso (`named Prince_Lhotun`); None = lo genera il gioco.
    pub name: Option<String>,
    pub face: Option<u32>,
    pub faction: Option<String>,
    pub gender: Option<String>,
    /// Messaggio che dice qualsiasi PNG quando gli si chiede di questa persona.
    pub any_info: Option<u32>,
    /// Messaggio che entra nelle voci in giro.
    pub rumors: Option<u32>,
    /// Gli altri parametri (`atHome`, `remote`...).
    pub flags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct PlaceDef {
    pub symbol: String,
    /// "permanent", "remote", "randompermanent"...
    pub kind: String,
    /// Luoghi possibili o tipo di edificio (`Castle_Faallem,Fortress_Fhojum`, `tavern`, `house1`).
    pub targets: Vec<String>,
    pub any_info: Option<u32>,
    pub rumors: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ItemDef {
    pub symbol: String,
    pub kind: String,
    /// `used 1010`: testo mostrato quando si usa l'oggetto.
    pub used_message: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct ClockDef {
    pub symbol: String,
    /// Il resto della riga (durata, flag, range): si interpreta nel runtime.
    pub spec: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct FoeDef {
    pub symbol: String,
    pub template: String,
}

// =============================================================================
// Compiti e istruzioni
// =============================================================================

#[derive(Clone, Debug, PartialEq)]
pub struct TopicRef {
    pub kind: TopicKind,
    pub symbol: String,
}

/// Un'istruzione. Quelle che servono alla conoscenza dei PNG sono tipizzate; le altre restano
/// come testo (`Other`) per il runtime.
#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Say(MsgRef),
    Log { message: MsgRef, step: u32 },
    DialogLink(TopicRef),
    AddDialog(TopicRef),
    StartQuest(Vec<u32>),
    EndQuest,
    Other(String),
}

fn parse_topic(kind: &str, symbol: &str) -> Option<TopicRef> {
    let kind = match kind {
        "person" => TopicKind::Person,
        "location" => TopicKind::Location,
        _ => return None,
    };
    Some(TopicRef { kind, symbol: symbol.to_string() })
}

impl Stmt {
    pub fn parse(raw: &str) -> Stmt {
        let toks: Vec<&str> = raw.split_whitespace().collect();
        let lower: Vec<String> = toks.iter().map(|t| t.to_ascii_lowercase()).collect();
        let l = |i: usize| lower.get(i).map(String::as_str).unwrap_or("");
        match (l(0), l(1), l(2)) {
            ("say", _, _) if toks.len() >= 2 => Stmt::Say(MsgRef::parse(toks[1])),
            ("log", _, _) if toks.len() >= 4 && l(2) == "step" => match toks[3].parse() {
                Ok(step) => Stmt::Log { message: MsgRef::parse(toks[1]), step },
                Err(_) => Stmt::Other(raw.to_string()),
            },
            ("dialog", "link", "for") if toks.len() >= 5 => {
                parse_topic(l(3), toks[4]).map_or_else(|| Stmt::Other(raw.to_string()), Stmt::DialogLink)
            }
            ("add", "dialog", "for") if toks.len() >= 5 => {
                parse_topic(l(3), toks[4]).map_or_else(|| Stmt::Other(raw.to_string()), Stmt::AddDialog)
            }
            ("start", "quest", _) => {
                let ids: Vec<u32> = toks[2..].iter().filter_map(|t| t.parse().ok()).collect();
                Stmt::StartQuest(ids)
            }
            ("end", "quest", _) => Stmt::EndQuest,
            _ => Stmt::Other(raw.to_string()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskKind {
    /// `_S.00_ task:` con le sue istruzioni.
    Task,
    /// `variable _S.01_`.
    Variable,
    /// `LiftedCurse _S.09_`: variabile (senza istruzioni) o compito (con istruzioni) con un nome
    /// globale, che il gioco puo' impostare dall'esterno.
    Global,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub symbol: String,
    pub kind: TaskKind,
    /// Il nome globale di un `Global`.
    pub global_name: Option<String>,
    pub statements: Vec<String>,
}

impl Task {
    /// Le istruzioni lette; quelle che non si riconoscono restano `Stmt::Other`.
    pub fn stmts(&self) -> Vec<Stmt> {
        self.statements.iter().map(|s| Stmt::parse(s)).collect()
    }
}

// =============================================================================
// Lo script
// =============================================================================

#[derive(Clone, Debug, PartialEq, Default)]
pub struct QuestScript {
    /// "S0000001".
    pub id: String,
    pub display_name: String,
    /// Dal commento `-- QuestId: 1`.
    pub quest_id: Option<u32>,
    pub messages: BTreeMap<u32, Message>,
    pub items: Vec<ItemDef>,
    pub persons: Vec<PersonDef>,
    pub places: Vec<PlaceDef>,
    pub clocks: Vec<ClockDef>,
    pub foes: Vec<FoeDef>,
    /// Le istruzioni del blocco di avvio (prima del primo compito).
    pub startup: Vec<String>,
    pub tasks: Vec<Task>,
    /// Righe del QBN non riconosciute (per accorgersi di sintassi non gestite).
    pub unknown: Vec<String>,
}

fn is_symbol(token: &str) -> bool {
    token.len() > 2 && token.starts_with('_') && token.ends_with('_')
}

/// "Prince_Lhotun" -> "Prince Lhotun".
pub fn pretty_name(raw: &str) -> String {
    raw.replace('_', " ").trim().to_string()
}

fn clean_line(line: &str) -> String {
    let t = line.trim();
    t.strip_prefix("<ce>").unwrap_or(t).trim().to_string()
}

/// `Name:  [1000]` o `Message:  1011`: (nome, numero).
fn parse_message_header(line: &str) -> Option<(Option<String>, u32)> {
    let (name, rest) = line.trim().split_once(':')?;
    let name = name.trim();
    let mut chars = name.chars();
    if !chars.next()?.is_ascii_alphabetic() || !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let rest = rest.trim();
    let digits = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')).unwrap_or(rest);
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let id = digits.parse().ok()?;
    Some((if name.eq_ignore_ascii_case("message") { None } else { Some(name.to_string()) }, id))
}

/// Parole chiave dopo il simbolo di Person/Place che non sono un valore da prendere.
const KEYWORDS: [&str; 8] = ["named", "face", "faction", "anyinfo", "rumors", "male", "female", "remote"];

fn parse_person(tokens: &[&str]) -> Option<PersonDef> {
    let mut p = PersonDef { symbol: tokens.get(1)?.to_string(), ..Default::default() };
    let mut i = 2;
    while i < tokens.len() {
        let t = tokens[i];
        let next = tokens.get(i + 1).copied();
        match (t.to_ascii_lowercase().as_str(), next) {
            ("named", Some(v)) => {
                p.name = Some(pretty_name(v));
                i += 1;
            }
            ("face", Some(v)) => {
                p.face = v.parse().ok();
                i += 1;
            }
            ("faction", Some(v)) => {
                p.faction = Some(pretty_name(v));
                i += 1;
            }
            ("anyinfo", Some(v)) => {
                p.any_info = v.parse().ok();
                i += 1;
            }
            ("rumors", Some(v)) => {
                p.rumors = v.parse().ok();
                i += 1;
            }
            ("male", _) | ("female", _) => p.gender = Some(t.to_ascii_lowercase()),
            _ => p.flags.push(t.to_string()),
        }
        i += 1;
    }
    Some(p)
}

fn parse_place(tokens: &[&str]) -> Option<PlaceDef> {
    let mut p = PlaceDef { symbol: tokens.get(1)?.to_string(), kind: tokens.get(2)?.to_string(), ..Default::default() };
    let mut i = 3;
    // il primo valore che non e' una parola chiave sono i luoghi possibili
    if let Some(t) = tokens.get(i) {
        if !KEYWORDS.contains(&t.to_ascii_lowercase().as_str()) {
            p.targets = t.split(',').map(str::to_string).filter(|s| !s.is_empty()).collect();
            i += 1;
        }
    }
    while i < tokens.len() {
        match (tokens[i].to_ascii_lowercase().as_str(), tokens.get(i + 1)) {
            ("anyinfo", Some(v)) => {
                p.any_info = v.parse().ok();
                i += 1;
            }
            ("rumors", Some(v)) => {
                p.rumors = v.parse().ok();
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    Some(p)
}

fn parse_item(tokens: &[&str]) -> Option<ItemDef> {
    let mut it = ItemDef { symbol: tokens.get(1)?.to_string(), kind: tokens.get(2)?.to_string(), ..Default::default() };
    if let Some(pos) = tokens.iter().position(|t| t.eq_ignore_ascii_case("used")) {
        it.used_message = tokens.get(pos + 1).and_then(|v| v.parse().ok());
    }
    Some(it)
}

#[derive(PartialEq)]
enum Section {
    Header,
    Qrc,
    Qbn,
}

impl QuestScript {
    /// Legge il testo di uno script. Tollerante: le righe non riconosciute finiscono in `unknown`.
    pub fn parse(source: &str) -> Result<QuestScript, String> {
        let mut s = QuestScript::default();
        let mut section = Section::Header;
        // messaggio in lettura
        let mut current: Option<(Option<String>, u32)> = None;
        let mut variants: Vec<Vec<String>> = Vec::new();
        let mut lines_acc: Vec<String> = Vec::new();
        // compito in lettura (indice in s.tasks)
        let mut task: Option<usize> = None;

        fn trim_blank(mut v: Vec<String>) -> Vec<String> {
            while v.last().is_some_and(|l| l.is_empty()) {
                v.pop();
            }
            let lead = v.iter().take_while(|l| l.is_empty()).count();
            v.drain(..lead);
            v
        }
        fn finish_message(
            s: &mut QuestScript,
            current: &mut Option<(Option<String>, u32)>,
            variants: &mut Vec<Vec<String>>,
            acc: &mut Vec<String>,
        ) {
            if let Some((name, id)) = current.take() {
                variants.push(trim_blank(std::mem::take(acc)));
                let vs: Vec<Vec<String>> = std::mem::take(variants).into_iter().filter(|v| !v.is_empty()).collect();
                s.messages.insert(id, Message { id, name, variants: vs });
            }
            acc.clear();
            variants.clear();
        }

        for raw in source.lines() {
            let line = raw.trim_end_matches('\r');
            let t = line.trim();
            match section {
                Section::Header => {
                    if t.eq_ignore_ascii_case("QRC:") {
                        section = Section::Qrc;
                    } else if t.eq_ignore_ascii_case("QBN:") {
                        section = Section::Qbn;
                    } else if let Some(rest) = t.strip_prefix("--") {
                        if let Some(n) = rest.trim().strip_prefix("QuestId:") {
                            s.quest_id = n.trim().parse().ok();
                        }
                    } else if let Some(v) = t.strip_prefix("Quest:") {
                        s.id = v.trim().to_string();
                    } else if let Some(v) = t.strip_prefix("DisplayName:") {
                        s.display_name = v.trim().to_string();
                    }
                }
                Section::Qrc => {
                    if t.eq_ignore_ascii_case("QBN:") {
                        finish_message(&mut s, &mut current, &mut variants, &mut lines_acc);
                        section = Section::Qbn;
                    } else if line.starts_with("--") {
                        // commento
                    } else if let Some(header) = parse_message_header(line) {
                        finish_message(&mut s, &mut current, &mut variants, &mut lines_acc);
                        current = Some(header);
                    } else if current.is_some() {
                        if t == "<--->" {
                            variants.push(trim_blank(std::mem::take(&mut lines_acc)));
                        } else {
                            lines_acc.push(clean_line(line));
                        }
                    }
                }
                Section::Qbn => {
                    if t.is_empty() || t.starts_with("--") {
                        continue;
                    }
                    let indented = line.starts_with(' ') || line.starts_with('\t');
                    if indented {
                        match task {
                            Some(i) => s.tasks[i].statements.push(t.to_string()),
                            None => s.startup.push(t.to_string()),
                        }
                        continue;
                    }
                    let tokens: Vec<&str> = t.split_whitespace().collect();
                    task = None;
                    match tokens[0] {
                        "Item" => match parse_item(&tokens) {
                            Some(d) => s.items.push(d),
                            None => s.unknown.push(t.to_string()),
                        },
                        "Person" => match parse_person(&tokens) {
                            Some(d) => s.persons.push(d),
                            None => s.unknown.push(t.to_string()),
                        },
                        "Place" => match parse_place(&tokens) {
                            Some(d) => s.places.push(d),
                            None => s.unknown.push(t.to_string()),
                        },
                        "Clock" if tokens.len() >= 2 => {
                            s.clocks.push(ClockDef { symbol: tokens[1].to_string(), spec: tokens[2..].join(" ") })
                        }
                        "Foe" if tokens.len() >= 2 => {
                            let template = tokens.iter().skip(2).filter(|t| !t.eq_ignore_ascii_case("is")).cloned().collect::<Vec<_>>().join(" ");
                            s.foes.push(FoeDef { symbol: tokens[1].to_string(), template })
                        }
                        "variable" if tokens.len() == 2 => {
                            s.tasks.push(Task { symbol: tokens[1].to_string(), kind: TaskKind::Variable, global_name: None, statements: vec![] })
                        }
                        first if t.ends_with("task:") && tokens.len() == 2 => {
                            s.tasks.push(Task { symbol: first.to_string(), kind: TaskKind::Task, global_name: None, statements: vec![] });
                            task = Some(s.tasks.len() - 1);
                        }
                        first if tokens.len() == 2 && is_symbol(tokens[1]) => {
                            s.tasks.push(Task {
                                symbol: tokens[1].to_string(),
                                kind: TaskKind::Global,
                                global_name: Some(first.to_string()),
                                statements: vec![],
                            });
                            task = Some(s.tasks.len() - 1);
                        }
                        _ => s.unknown.push(t.to_string()),
                    }
                }
            }
        }
        if section == Section::Qrc {
            finish_message(&mut s, &mut current, &mut variants, &mut lines_acc);
        }
        if s.id.is_empty() {
            return Err("manca la riga 'Quest:'".into());
        }
        Ok(s)
    }

    pub fn message(&self, id: u32) -> Option<&Message> {
        self.messages.get(&id)
    }

    pub fn message_named(&self, name: &str) -> Option<&Message> {
        self.messages.values().find(|m| m.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
    }

    pub fn resolve_ref(&self, r: &MsgRef) -> Option<&Message> {
        match r {
            MsgRef::Id(id) => self.message(*id),
            MsgRef::Name(n) => self.message_named(n),
        }
    }

    pub fn person(&self, symbol: &str) -> Option<&PersonDef> {
        self.persons.iter().find(|p| p.symbol == symbol)
    }

    pub fn place(&self, symbol: &str) -> Option<&PlaceDef> {
        self.places.iter().find(|p| p.symbol == symbol)
    }

    /// Argomenti abilitati subito all'avvio (`dialog link for ...` nel blocco di avvio).
    pub fn startup_topics(&self) -> Vec<TopicRef> {
        self.startup
            .iter()
            .filter_map(|s| match Stmt::parse(s) {
                Stmt::DialogLink(t) => Some(t),
                _ => None,
            })
            .collect()
    }

    /// Tutti gli argomenti su cui un PNG sa dire qualcosa (persone e luoghi con `anyInfo`).
    pub fn topics(&self) -> Vec<Topic> {
        let persons = self.persons.iter().filter_map(|p| {
            Some(Topic { symbol: p.symbol.clone(), kind: TopicKind::Person, message: p.any_info? })
        });
        let places = self.places.iter().filter_map(|p| {
            Some(Topic { symbol: p.symbol.clone(), kind: TopicKind::Location, message: p.any_info? })
        });
        persons.chain(places).collect()
    }

    /// Le voci in giro per lo stato della quest, con le macro ancora da espandere.
    /// Attiva: voci della quest piu' quelle di persone e luoghi; fallita o completata: le voci del dopo.
    pub fn rumor_variants(&self, phase: QuestPhase) -> Vec<String> {
        let named = |n: &str| self.message_named(n).map(Message::plain_variants).unwrap_or_default();
        match phase {
            QuestPhase::NotStarted => vec![],
            QuestPhase::Active => {
                let mut v = named("RumorsDuringQuest");
                let extra = self
                    .persons
                    .iter()
                    .filter_map(|p| p.rumors)
                    .chain(self.places.iter().filter_map(|p| p.rumors))
                    .filter_map(|id| self.message(id));
                for m in extra {
                    v.extend(m.plain_variants());
                }
                v
            }
            QuestPhase::Failed => named("RumorsPostfailure"),
            QuestPhase::Completed => named("RumorsPostsuccess"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Topic {
    pub symbol: String,
    pub kind: TopicKind,
    /// Il messaggio `anyInfo`.
    pub message: u32,
}

// =============================================================================
// Macro dei testi
// =============================================================================

/// Una macro trovata in un testo.
#[derive(Clone, Debug, PartialEq)]
pub enum Macro {
    /// `%pcf`, `%di`...: il nome senza `%`.
    Percent(String),
    /// `_brother_` (livello 1), `__castfort_` (2), `___ukcrypt_` (3): `name` e' il simbolo del QBN
    /// (`_brother_`).
    Symbol { name: String, level: u8 },
    /// `=1stparton_`: valore di un orologio; `name` e' il simbolo (`_1stparton_`).
    Clock(String),
}

fn is_sym_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.'
}

/// Espande le macro di `text` con `f`; se `f` risponde None la macro resta com'e'.
pub fn expand_macros(text: &str, f: &mut dyn FnMut(&Macro) -> Option<String>) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let prev_ok = i == 0 || !chars[i - 1].is_alphanumeric();
        // %nome
        if c == '%' && i + 1 < chars.len() && chars[i + 1].is_ascii_alphabetic() {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_ascii_alphabetic() {
                j += 1;
            }
            let name: String = chars[i + 1..j].iter().collect();
            match f(&Macro::Percent(name)) {
                Some(v) => out.push_str(&v),
                None => out.extend(&chars[i..j]),
            }
            i = j;
            continue;
        }
        // _simbolo_, __simbolo_, ___simbolo_
        if c == '_' && prev_ok {
            let mut j = i;
            while j < chars.len() && chars[j] == '_' {
                j += 1;
            }
            let level = j - i;
            let name_start = j;
            while j < chars.len() && is_sym_char(chars[j]) {
                j += 1;
            }
            if (1..=3).contains(&level) && j > name_start && j < chars.len() && chars[j] == '_' {
                let name: String = chars[name_start..j].iter().collect();
                let m = Macro::Symbol { name: format!("_{name}_"), level: level as u8 };
                j += 1;
                match f(&m) {
                    Some(v) => out.push_str(&v),
                    None => out.extend(&chars[i..j]),
                }
                i = j;
                continue;
            }
        }
        // =orologio_
        if c == '=' && prev_ok {
            let mut j = i + 1;
            while j < chars.len() && is_sym_char(chars[j]) {
                j += 1;
            }
            if j > i + 1 && j < chars.len() && chars[j] == '_' {
                let name: String = chars[i + 1..j].iter().collect();
                j += 1;
                match f(&Macro::Clock(format!("_{name}_"))) {
                    Some(v) => out.push_str(&v),
                    None => out.extend(&chars[i..j]),
                }
                i = j;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Valori delle macro `%...` quando il gioco non ne conosce uno migliore. Neutri: finiscono nei
/// prompt del LLM, che non deve ricevere segnaposto.
fn default_percent(name: &str) -> String {
    match name {
        "pcf" | "pcn" | "pct" | "ra" => "traveler".into(),
        "oth" => "Oh".into(),
        "god" => "the gods".into(),
        "di" => "a short distance".into(),
        _ => String::new(), // `%qdt` e simili: intestazioni del diario, non si pronunciano
    }
}

fn generic_place(def: &PlaceDef) -> String {
    match def.targets.first().map(|t| t.to_ascii_lowercase()) {
        Some(t) if t == "tavern" => "the tavern".into(),
        Some(t) if t.starts_with("house") => "a house".into(),
        Some(t) if t.starts_with("palace") => "the palace".into(),
        Some(t) if def.kind != "remote" => pretty_name(&t),
        _ => "the place".into(),
    }
}

// =============================================================================
// Stato delle quest
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QuestPhase {
    NotStarted,
    Active,
    Failed,
    Completed,
}

impl QuestPhase {
    pub fn key(self) -> &'static str {
        match self {
            QuestPhase::NotStarted => "notstarted",
            QuestPhase::Active => "active",
            QuestPhase::Failed => "failed",
            QuestPhase::Completed => "completed",
        }
    }

    pub fn from_key(k: &str) -> Option<QuestPhase> {
        [QuestPhase::NotStarted, QuestPhase::Active, QuestPhase::Failed, QuestPhase::Completed]
            .into_iter()
            .find(|p| p.key() == k)
    }
}

/// Lo stato di tutte le quest principali. Lo scrive chi esegue i compiti; lo leggono i dialoghi.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QuestStates {
    phases: HashMap<String, QuestPhase>,
    /// Argomenti di dialogo abilitati: (quest, simbolo).
    topics: HashSet<(String, String)>,
    /// Nomi risolti a runtime, per le macro: (quest, macro come scritta, es. "___ukcrypt_") -> testo.
    symbols: HashMap<(String, String), String>,
}

impl QuestStates {
    pub fn phase(&self, quest: &str) -> QuestPhase {
        self.phases.get(quest).copied().unwrap_or(QuestPhase::NotStarted)
    }

    /// Cambia fase. Quando la quest diventa attiva si abilitano gli argomenti del blocco di avvio.
    pub fn set_phase(&mut self, script: &QuestScript, phase: QuestPhase) {
        self.phases.insert(script.id.clone(), phase);
        if phase == QuestPhase::Active {
            for t in script.startup_topics() {
                self.topics.insert((script.id.clone(), t.symbol));
            }
        }
    }

    /// `add dialog for ...`: abilita un argomento.
    pub fn enable_topic(&mut self, quest: &str, symbol: &str) {
        self.topics.insert((quest.to_string(), symbol.to_string()));
    }

    pub fn is_topic_enabled(&self, quest: &str, symbol: &str) -> bool {
        self.topics.contains(&(quest.to_string(), symbol.to_string()))
    }

    /// Il nome scelto dal gioco per un luogo o una persona: serve per `_sym_`, `__sym_` (citta') e
    /// `___sym_` (luogo). `symbol` come nel QBN (`_ukcrypt_`).
    pub fn set_symbol(&mut self, quest: &str, symbol: &str, name: &str, town: Option<&str>) {
        let bare = symbol.trim_matches('_');
        self.symbols.insert((quest.to_string(), format!("_{bare}_")), name.to_string());
        self.symbols.insert((quest.to_string(), format!("___{bare}_")), name.to_string());
        if let Some(town) = town {
            self.symbols.insert((quest.to_string(), format!("__{bare}_")), town.to_string());
        }
    }

    pub fn active_quests(&self) -> impl Iterator<Item = &str> {
        self.phases.iter().filter(|(_, p)| **p != QuestPhase::NotStarted).map(|(k, _)| k.as_str())
    }

    /// Testo per il file di stato: `phase ID fase`, `topic ID simbolo`, `symbol ID macro valore`.
    pub fn to_text(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        for (id, p) in &self.phases {
            lines.push(format!("phase {id} {}", p.key()));
        }
        for (id, sym) in &self.topics {
            lines.push(format!("topic {id} {sym}"));
        }
        for ((id, sym), v) in &self.symbols {
            lines.push(format!("symbol {id} {sym} {v}"));
        }
        lines.sort();
        let mut out = String::from("# Stato dei quest principali (phase / topic / symbol)\n");
        out.push_str(&lines.join("\n"));
        out.push('\n');
        out
    }

    pub fn from_text(text: &str) -> QuestStates {
        let mut st = QuestStates::default();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let mut it = line.splitn(4, char::is_whitespace);
            match (it.next(), it.next(), it.next(), it.next()) {
                (Some("phase"), Some(id), Some(p), _) => {
                    if let Some(p) = QuestPhase::from_key(&p.to_ascii_lowercase()) {
                        st.phases.insert(id.to_string(), p);
                    }
                }
                (Some("topic"), Some(id), Some(sym), _) => {
                    st.topics.insert((id.to_string(), sym.to_string()));
                }
                (Some("symbol"), Some(id), Some(sym), Some(v)) => {
                    st.symbols.insert((id.to_string(), sym.to_string()), v.trim().to_string());
                }
                _ => {}
            }
        }
        st
    }
}

// =============================================================================
// Libreria di script
// =============================================================================

#[derive(Clone, Debug, Default)]
pub struct Library {
    scripts: BTreeMap<String, QuestScript>,
}

impl Library {
    pub fn insert(&mut self, script: QuestScript) {
        self.scripts.insert(script.id.clone(), script);
    }

    pub fn get(&self, id: &str) -> Option<&QuestScript> {
        self.scripts.get(id)
    }

    pub fn scripts(&self) -> impl Iterator<Item = &QuestScript> {
        self.scripts.values()
    }

    pub fn len(&self) -> usize {
        self.scripts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.scripts.is_empty()
    }

    /// Carica i quest principali `S*.txt` di una cartella (come `StreamingAssets/Quests`).
    /// Restituisce la libreria e gli avvisi (file illeggibili o non validi).
    /// Carica **tutti** i `.txt` di una cartella (come `StreamingAssets/Quests`).
    ///
    /// Non filtra per nome: sia i quest principali (`S*.txt`), sia le quest di gilda
    /// (`N*.txt`, `Q*.txt`, `A*.txt`), sia i template e i file di documentazione
    /// (`QuestComments.txt`, `_tutorial_.txt`, ...). I file che non sono quest valide
    /// (manca `Quest:` o `QRC:`) finiscono nei `warnings` e non entrano nella libreria.
    /// Il filtro "solo file S*" del vecchio `load_dir` è stato rimosso: caricare tutti i
    /// `.txt` costa pochi millisecondi e permette di usare anche le quest secondarie.
    ///
    /// Restituisce la libreria e gli avvisi (file illeggibili o non validi).
    pub fn load_dir(dir: &Path) -> (Library, Vec<String>) {
        let mut lib = Library::default();
        let mut warnings = Vec::new();
        let Ok(rd) = std::fs::read_dir(dir) else {
            warnings.push(format!("cartella dei quest non leggibile: {}", dir.display()));
            return (lib, warnings);
        };
        let mut paths: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let lower = name.to_ascii_lowercase();
            // Carica solo file .txt: la cartella Quests di DFU contiene anche
            // README.md, screenshot, ecc. che non interessano.
            if !lower.ends_with(".txt") {
                continue;
            }
            match std::fs::read(&path) {
                // gli script di DFU sono quasi sempre ASCII; i caratteri strani non fermano la lettura
                Ok(bytes) => match QuestScript::parse(&String::from_utf8_lossy(&bytes)) {
                    Ok(script) => lib.insert(script),
                    Err(e) => warnings.push(format!("{name}: {e}")),
                },
                Err(e) => warnings.push(format!("{name}: {e}")),
            }
        }
        (lib, warnings)
    }
    // ------------------------------------------------------------- macro

    /// Il valore di una macro per lo script `quest`: prima il nome scelto dal gioco, poi quello
    /// fisso dello script, poi una descrizione neutra.
    fn macro_value(&self, quest: &str, states: &QuestStates, m: &Macro) -> String {
        match m {
            Macro::Percent(n) => default_percent(n),
            Macro::Clock(_) => "a few".into(),
            Macro::Symbol { name, level } => {
                let written = format!("{}{}", "_".repeat(*level as usize), name.trim_matches('_')) + "_";
                if let Some(v) = states.symbols.get(&(quest.to_string(), written)) {
                    return v.clone();
                }
                if let Some(v) = states.symbols.get(&(quest.to_string(), name.clone())) {
                    if *level != 2 {
                        return v.clone();
                    }
                }
                let Some(script) = self.get(quest) else { return "someone".into() };
                if let Some(p) = script.person(name) {
                    return p.name.clone().unwrap_or_else(|| "a certain person".into());
                }
                if let Some(p) = script.place(name) {
                    return if *level == 2 { "the town".into() } else { generic_place(p) };
                }
                if script.items.iter().any(|i| &i.symbol == name) {
                    return "an item".into();
                }
                "something".into()
            }
        }
    }

    /// Espande le macro di un testo dello script `quest` con i valori noti.
    pub fn expand(&self, quest: &str, states: &QuestStates, text: &str) -> String {
        let cleaned = expand_macros(text, &mut |m| Some(self.macro_value(quest, states, m)));
        cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Il nome di un argomento, per riconoscerlo in una domanda.
    pub fn topic_subject(&self, quest: &str, states: &QuestStates, topic: &Topic) -> Option<String> {
        let script = self.get(quest)?;
        let subject = match topic.kind {
            TopicKind::Person => {
                let p = script.person(&topic.symbol)?;
                match &p.name {
                    Some(n) => n.clone(),
                    // senza nome fisso: il gioco ne sceglie uno (se lo ha registrato)
                    None => states.symbols.get(&(quest.to_string(), topic.symbol.clone()))?.clone(),
                }
            }
            TopicKind::Location => {
                let l = script.place(&topic.symbol)?;
                match states.symbols.get(&(quest.to_string(), topic.symbol.clone())) {
                    Some(n) => n.clone(),
                    // luogo scelto a caso tra piu' possibili: finche' il gioco non sceglie, nessun nome
                    None if l.targets.len() == 1 && l.kind != "remote" => pretty_name(&l.targets[0]),
                    None => return None,
                }
            }
        };
        Some(subject)
    }
}

// =============================================================================
// Conoscenza dei PNG
// =============================================================================

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
}

const GENERIC_WORDS: [&str; 14] = [
    "the", "prince", "princess", "queen", "king", "lord", "lady", "sir", "castle", "fortress", "tower", "temple",
    "dungeon", "crypt",
];

/// La domanda parla di `subject`? (nome intero, oppure una sua parola significativa)
pub fn mentions(question: &str, subject: &str) -> bool {
    let words = |s: &str| -> Vec<String> {
        s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
    };
    let q = words(question);
    let s = words(subject);
    if s.is_empty() {
        return false;
    }
    if q.windows(s.len()).any(|w| w == s.as_slice()) {
        return true;
    }
    s.iter().filter(|w| w.chars().count() >= 4 && !GENERIC_WORDS.contains(&w.as_str())).any(|w| q.contains(w))
}

#[derive(Clone, Debug)]
pub struct KnowledgeOptions {
    /// Quanti argomenti al massimo (nomi di persone e luoghi citati nella domanda).
    pub max_topics: usize,
    /// Quante voci in giro al massimo.
    pub max_rumors: usize,
    /// Tetto ai caratteri del blocco.
    pub max_chars: usize,
}

impl Default for KnowledgeOptions {
    fn default() -> Self {
        Self { max_topics: 2, max_rumors: 1, max_chars: 1200 }
    }
}

/// Le voci in giro che `npc_key` conosce: fisso per lo stesso PNG, diverso tra PNG diversi.
fn rumor_candidates(lib: &Library, states: &QuestStates) -> Vec<(String, String)> {
    let mut quests: Vec<&str> = states.active_quests().collect();
    quests.sort();
    let mut out = Vec::new();
    for q in quests {
        let Some(script) = lib.get(q) else { continue };
        for text in script.rumor_variants(states.phase(q)) {
            out.push((q.to_string(), text));
        }
    }
    out
}

/// Una voce in giro per questo PNG (macro espanse), se ce ne sono.
pub fn rumor_for(lib: &Library, states: &QuestStates, npc_key: &str) -> Option<String> {
    let cands = rumor_candidates(lib, states);
    if cands.is_empty() {
        return None;
    }
    let (quest, text) = &cands[(fnv1a(npc_key) % cands.len() as u64) as usize];
    Some(lib.expand(quest, states, text))
}

/// Il blocco "cose che hai sentito" da mettere nel prompt di un PNG: le informazioni sugli
/// argomenti che la domanda cita (solo quelli abilitati nelle quest attive) e una voce in giro.
/// Stringa vuota se non c'e' niente da dire.
pub fn knowledge_block(lib: &Library, states: &QuestStates, question: &str, npc_key: &str, opts: &KnowledgeOptions) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut quests: Vec<&str> = states.active_quests().collect();
    quests.sort();

    // 1. argomenti citati dalla domanda
    'topics: for q in &quests {
        if states.phase(q) != QuestPhase::Active {
            continue;
        }
        let Some(script) = lib.get(q) else { continue };
        for topic in script.topics() {
            if lines.len() >= opts.max_topics {
                break 'topics;
            }
            if !states.is_topic_enabled(q, &topic.symbol) {
                continue;
            }
            let Some(subject) = lib.topic_subject(q, states, &topic) else { continue };
            if !mentions(question, &subject) {
                continue;
            }
            let Some(msg) = script.message(topic.message) else { continue };
            let variants = msg.plain_variants();
            if variants.is_empty() {
                continue;
            }
            let pick = (fnv1a(&format!("{npc_key}|{}", topic.symbol)) % variants.len() as u64) as usize;
            lines.push(format!("About {subject}: {}", lib.expand(q, states, &variants[pick])));
        }
    }
    // 2. una voce in giro
    if opts.max_rumors > 0 {
        if let Some(r) = rumor_for(lib, states, npc_key) {
            lines.push(format!("Talk going around: {r}"));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    let mut block = String::from(
        "\nTHINGS YOU HAVE HEARD (common talk: you may repeat or paraphrase it in your own words, but add nothing beyond it):\n",
    );
    let mut budget = opts.max_chars;
    for l in lines {
        let l = format!("- {l}\n");
        if l.chars().count() > budget {
            break;
        }
        budget -= l.chars().count();
        block.push_str(&l);
    }
    if block.lines().count() <= 2 {
        return String::new(); // l'intestazione senza righe
    }
    block.push('\n');
    block
}

// =============================================================================
// Test
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "-- Quest: C:\\QUESTS\\WORKING\\S0000001.Qbn.
-- Main story quest.
-- QuestId: 1
Messages: 6
Quest: S0000001
DisplayName: Missing Prince
-- Message panels
QRC:

QuestorOffer:  [1000]
<ce>                  Welcome to Sentinel, %pcf. I guess
<ce>         you must have received my letter.

RefuseQuest:  [1001]
<ce>                         %oth, %pcf, I thought
<ce>                 you were a hero.

RumorsDuringQuest:  [1005]
Poor Prince Lhotun is in one of his moods. He's so different from Greklith.
<--->
Queen Akorithi doesn't know what to do with Prince Lhotun.

RumorsPostsuccess:  [1007]
Prince Lhotun's mood has gotten better.

Message:  1013
The Underking is supposed to be centuries old.
<--->
                                     <--->
I heard he kidnapped Prince _brother_.

Message:  1015
___ukcrypt_ used to be the Underking's crypt, but now it's a Sentilian prison.
<--->
___ukcrypt_ is just %di of here. Some say it's still a crypt of the Underking.

Message:  1016
___ukcrypt_ is directly %di of here.

Message:  1030
_brother_ was a prince of Sentinel. He died years ago.

Message:  1031
I heard that _brother_'s former nursemaid was sent to Skyrim with a chest full of gold.

Message:  1050
Lord Castellian is the most powerful of Wayrest's councilors.

-- Symbols used in the QRC file:
--
--              %di occurs 4 times.

QBN:
Item deathCertificate letter used 1010
Item _magicitem_ magic_item

Person _lhotun_ named Prince_Lhotun atHome
Person _brother_ face 5 faction Children male remote anyInfo 1030 rumors 1031
Person _underking_ face 1 named The_Underking anyInfo 1013

Place _ukcrypt_ randompermanent Castle_Faallem,Fortress_Fhojum anyInfo 1015 rumors 1016
Place _dirennitower_ permanent DirenniTowerEntry
Place _agentplace_ remote tavern

Clock _S.13_ 3.00:00 7.00:00
Clock _1stparton_ 00:00 0 flag 17 range 0 2
Foe _F.00_ is Knight

--	Quest start-up:
	dialog link for location _ukcrypt_
	prompt QuestorOffer yes yes no _no_
	place npc _agentuk_ at _agentplace_

_S.00_ task:
	toting deathCertificate and _lhotun_ clicked
	give pc _magicitem_
	add dialog for location _dirennitower_
--added additional remove log.
	start quest 17 17

variable _S.01_
yes task:
	log 1019 step 0
	say AcceptQuest

LiftedCurse _S.09_
MedoraGotHorn _S.10_
	toting horn and _medora_ clicked
	end quest
";

    fn script() -> QuestScript {
        QuestScript::parse(SAMPLE).unwrap()
    }

    #[test]
    fn header_and_messages_are_read() {
        let s = script();
        assert_eq!((s.id.as_str(), s.display_name.as_str(), s.quest_id), ("S0000001", "Missing Prince", Some(1)));
        assert_eq!(s.messages.len(), 11);
        let offer = s.message_named("QuestorOffer").unwrap();
        assert_eq!(offer.id, 1000);
        assert_eq!(offer.plain_variants(), vec!["Welcome to Sentinel, %pcf. I guess you must have received my letter."]);
        // numero senza parentesi e senza nome
        assert_eq!(s.message(1013).unwrap().name, None);
        // le varianti vuote (riga <---> in mezzo) si scartano
        assert_eq!(s.message(1013).unwrap().plain_variants().len(), 2);
        assert_eq!(s.message(1005).unwrap().plain_variants().len(), 2);
        assert_eq!(s.message_named("rumorsduringquest").unwrap().id, 1005); // nome senza badare alle maiuscole
        // il commento finale e l'elenco dei simboli non finiscono nell'ultimo messaggio
        assert_eq!(s.message(1050).unwrap().plain_variants(), vec!["Lord Castellian is the most powerful of Wayrest's councilors."]);
    }

    #[test]
    fn definitions_are_read() {
        let s = script();
        assert_eq!(s.items.len(), 2);
        assert_eq!(s.items[0], ItemDef { symbol: "deathCertificate".into(), kind: "letter".into(), used_message: Some(1010) });
        let lhotun = s.person("_lhotun_").unwrap();
        assert_eq!((lhotun.name.as_deref(), lhotun.any_info), (Some("Prince Lhotun"), None));
        assert_eq!(lhotun.flags, vec!["atHome"]);
        let brother = s.person("_brother_").unwrap();
        assert_eq!(
            (brother.name.clone(), brother.face, brother.faction.as_deref(), brother.gender.as_deref(), brother.any_info, brother.rumors),
            (None, Some(5), Some("Children"), Some("male"), Some(1030), Some(1031))
        );
        assert_eq!(brother.flags, vec!["remote"]);
        let crypt = s.place("_ukcrypt_").unwrap();
        assert_eq!(crypt.kind, "randompermanent");
        assert_eq!(crypt.targets, vec!["Castle_Faallem", "Fortress_Fhojum"]);
        assert_eq!((crypt.any_info, crypt.rumors), (Some(1015), Some(1016)));
        assert_eq!(s.place("_agentplace_").unwrap().targets, vec!["tavern"]);
        assert_eq!(s.place("_dirennitower_").unwrap().targets, vec!["DirenniTowerEntry"]);
        assert_eq!(s.clocks.len(), 2);
        assert_eq!(s.clocks[1].spec, "00:00 0 flag 17 range 0 2");
        assert_eq!(s.foes, vec![FoeDef { symbol: "_F.00_".into(), template: "Knight".into() }]);
        assert!(s.unknown.is_empty(), "{:?}", s.unknown);
    }

    #[test]
    fn startup_tasks_variables_and_globals_are_read() {
        let s = script();
        assert_eq!(s.startup.len(), 3);
        assert_eq!(s.startup_topics(), vec![TopicRef { kind: TopicKind::Location, symbol: "_ukcrypt_".into() }]);
        let kinds: Vec<(&str, TaskKind)> = s.tasks.iter().map(|t| (t.symbol.as_str(), t.kind)).collect();
        assert_eq!(
            kinds,
            vec![
                ("_S.00_", TaskKind::Task),
                ("_S.01_", TaskKind::Variable),
                ("yes", TaskKind::Task),
                ("_S.09_", TaskKind::Global),
                ("_S.10_", TaskKind::Global),
            ]
        );
        let s00 = &s.tasks[0];
        assert_eq!(s00.statements.len(), 4); // il commento a colonna 0 non conta
        assert_eq!(s00.stmts()[2], Stmt::AddDialog(TopicRef { kind: TopicKind::Location, symbol: "_dirennitower_".into() }));
        assert_eq!(s00.stmts()[3], Stmt::StartQuest(vec![17, 17]));
        assert_eq!(s.tasks[3].global_name.as_deref(), Some("LiftedCurse"));
        assert!(s.tasks[3].statements.is_empty()); // variabile globale senza corpo
        assert_eq!(s.tasks[4].global_name.as_deref(), Some("MedoraGotHorn"));
        assert_eq!(s.tasks[4].stmts().last(), Some(&Stmt::EndQuest));
    }

    #[test]
    fn statements_are_parsed() {
        assert_eq!(Stmt::parse("say 1018"), Stmt::Say(MsgRef::Id(1018)));
        assert_eq!(Stmt::parse("say AcceptQuest"), Stmt::Say(MsgRef::Name("AcceptQuest".into())));
        assert_eq!(Stmt::parse("log 1019 step 0 "), Stmt::Log { message: MsgRef::Id(1019), step: 0 });
        assert_eq!(
            Stmt::parse("dialog link for person Castellian"),
            Stmt::DialogLink(TopicRef { kind: TopicKind::Person, symbol: "Castellian".into() })
        );
        assert_eq!(Stmt::parse("start quest 11 11"), Stmt::StartQuest(vec![11, 11]));
        assert_eq!(Stmt::parse("end quest"), Stmt::EndQuest);
        assert_eq!(Stmt::parse("give pc _magicitem_"), Stmt::Other("give pc _magicitem_".into()));
        assert_eq!(Stmt::parse("log 1019 step x"), Stmt::Other("log 1019 step x".into()));
        assert_eq!(Stmt::parse(""), Stmt::Other(String::new()));
    }

    #[test]
    fn a_script_without_quest_line_is_rejected() {
        assert!(QuestScript::parse("QRC:\nQBN:\n").is_err());
        assert!(QuestScript::parse("Quest: S1\nQRC:\n").is_ok()); // senza QBN va bene
    }

    // ----------------------------------------------------------------- macro

    #[test]
    fn macros_are_found_with_their_level() {
        let mut found = Vec::new();
        let text = "Hi %pcf, ___ukcrypt_ in __castfort_ with _brother_'s letter, in =1stparton_ days. 100% sure, snake_case_x, 3=4.";
        let out = expand_macros(text, &mut |m| {
            found.push(m.clone());
            Some("X".into())
        });
        assert_eq!(
            found,
            vec![
                Macro::Percent("pcf".into()),
                Macro::Symbol { name: "_ukcrypt_".into(), level: 3 },
                Macro::Symbol { name: "_castfort_".into(), level: 2 },
                Macro::Symbol { name: "_brother_".into(), level: 1 },
                Macro::Clock("_1stparton_".into()),
            ]
        );
        assert_eq!(out, "Hi X, X in X with X's letter, in X days. 100% sure, snake_case_x, 3=4.");
        // macro non risolta: resta com'e'
        assert_eq!(expand_macros("a %pcf b", &mut |_| None), "a %pcf b");
        assert_eq!(expand_macros("", &mut |_| None), "");
        assert_eq!(expand_macros("%", &mut |_| None), "%");
        assert_eq!(expand_macros("__", &mut |_| None), "__");
    }

    fn lib_with_sample() -> Library {
        let mut lib = Library::default();
        lib.insert(script());
        lib
    }

    #[test]
    fn default_expansion_never_leaks_placeholders() {
        let lib = lib_with_sample();
        let st = QuestStates::default();
        let t = lib.expand("S0000001", &st, "%oth, %pcf! ___ukcrypt_ is just %di of here. _brother_ and _lhotun_ in __ukcrypt_. =1stparton_ days. %qdt:");
        assert_eq!(t, "Oh, traveler! Castle Faallem is just a short distance of here. a certain person and Prince Lhotun in the town. a few days. :");
        assert!(!t.contains('%') && !t.contains("_ukcrypt_") && !t.contains("_brother_"));
        // luogo "remote tavern"
        assert_eq!(lib.expand("S0000001", &st, "meet me at _agentplace_"), "meet me at the tavern");
    }

    #[test]
    fn names_chosen_by_the_game_override_the_defaults() {
        let lib = lib_with_sample();
        let mut st = QuestStates::default();
        st.set_symbol("S0000001", "_ukcrypt_", "Fortress Fhojum", Some("Sentinel"));
        st.set_symbol("S0000001", "_brother_", "Prince Zelmo", None);
        assert_eq!(
            lib.expand("S0000001", &st, "___ukcrypt_ near __ukcrypt_; _brother_ died."),
            "Fortress Fhojum near Sentinel; Prince Zelmo died."
        );
        // un altro quest non ne risente
        assert_eq!(lib.expand("S0000002", &st, "_brother_"), "someone");
    }

    // ---------------------------------------------------------------- stato

    #[test]
    fn states_round_trip_through_text() {
        let s = script();
        let mut st = QuestStates::default();
        assert_eq!(st.phase("S0000001"), QuestPhase::NotStarted);
        st.set_phase(&s, QuestPhase::Active);
        assert!(st.is_topic_enabled("S0000001", "_ukcrypt_")); // dal blocco di avvio
        assert!(!st.is_topic_enabled("S0000001", "_dirennitower_"));
        st.enable_topic("S0000001", "_dirennitower_");
        st.set_symbol("S0000001", "_ukcrypt_", "Castle Faallem", Some("Sentinel"));
        let text = st.to_text();
        assert_eq!(QuestStates::from_text(&text), st);
        // un file scritto a mano, con righe sbagliate
        let hand = "# prova\nphase S0000002 ACTIVE\nphase S0000003 boh\ntopic S0000002 Castellian\nrumore\n\nsymbol S0000002 _castfort_ Lord's Hold\n";
        let st2 = QuestStates::from_text(hand);
        assert_eq!(st2.phase("S0000002"), QuestPhase::Active);
        assert_eq!(st2.phase("S0000003"), QuestPhase::NotStarted);
        assert!(st2.is_topic_enabled("S0000002", "Castellian"));
        assert_eq!(st2.symbols.get(&("S0000002".into(), "_castfort_".into())).map(String::as_str), Some("Lord's Hold"));
    }

    #[test]
    fn rumors_depend_on_the_quest_phase() {
        let s = script();
        assert!(s.rumor_variants(QuestPhase::NotStarted).is_empty());
        let active = s.rumor_variants(QuestPhase::Active);
        // 2 voci della quest + 2 di _brother_ (1031) + 1 di _ukcrypt_ (1016)
        assert_eq!(active.len(), 2 + 1 + 1);
        assert!(active.iter().any(|r| r.contains("Greklith")));
        assert!(active.iter().any(|r| r.contains("nursemaid")));
        assert_eq!(s.rumor_variants(QuestPhase::Completed), vec!["Prince Lhotun's mood has gotten better."]);
        assert!(s.rumor_variants(QuestPhase::Failed).is_empty()); // questo script non ne ha
    }

    // ------------------------------------------------------------ conoscenza

    fn active_states() -> (Library, QuestStates) {
        let lib = lib_with_sample();
        let mut st = QuestStates::default();
        st.set_phase(lib.get("S0000001").unwrap(), QuestPhase::Active);
        (lib, st)
    }

    #[test]
    fn questions_are_matched_to_topics() {
        assert!(mentions("Do you know anything about the Underking?", "The Underking"));
        assert!(mentions("what happened to lhotun", "Prince Lhotun"));
        assert!(mentions("Who is PRINCE Lhotun?", "Prince Lhotun"));
        assert!(!mentions("Where is the prince?", "Prince Lhotun")); // "prince" e' un titolo, non il nome
        assert!(!mentions("Where is the inn?", "Castle Faallem"));
        assert!(mentions("Is Castle Faallem far?", "Castle Faallem"));
        assert!(mentions("tell me about faallem", "Castle Faallem"));
        assert!(!mentions("anything", ""));
    }

    #[test]
    fn knowledge_needs_an_active_quest_and_an_enabled_topic() {
        let lib = lib_with_sample();
        let opts = KnowledgeOptions::default();
        // nessuna quest attiva: niente
        assert_eq!(knowledge_block(&lib, &QuestStates::default(), "Tell me about the Underking", "npc1", &opts), "");
        let (lib, mut st) = active_states();
        // _underking_ non e' abilitato (non c'e' un `dialog link` per lui nell'avvio)
        let k = knowledge_block(&lib, &st, "Tell me about the Underking", "npc1", &opts);
        assert!(!k.contains("About The Underking"), "{k}");
        // abilitato: si
        st.enable_topic("S0000001", "_underking_");
        let k = knowledge_block(&lib, &st, "Tell me about the Underking", "npc1", &opts);
        assert!(k.starts_with("\nTHINGS YOU HAVE HEARD"), "{k}");
        assert!(k.contains("- About The Underking: "), "{k}");
        assert!(k.contains("centuries old") || k.contains("kidnapped Prince a certain person"), "{k}");
        assert!(!k.contains('%') && !k.contains("_brother_"));
        // la domanda non lo cita: restano solo le voci in giro
        let k = knowledge_block(&lib, &st, "Where is the inn?", "npc1", &opts);
        assert!(!k.contains("About "), "{k}");
        assert!(k.contains("Talk going around: "), "{k}");
    }

    #[test]
    fn a_place_chosen_at_random_is_a_topic_only_once_the_game_names_it() {
        let (lib, st) = active_states();
        let opts = KnowledgeOptions::default();
        // _ukcrypt_ e' abilitato dall'avvio ma ha due luoghi possibili: finche' il gioco non sceglie, nessun nome
        let k = knowledge_block(&lib, &st, "Where is Castle Faallem?", "npc1", &opts);
        assert!(!k.contains("About Castle Faallem"), "{k}");
        let mut st = st;
        st.set_symbol("S0000001", "_ukcrypt_", "Castle Faallem", Some("Sentinel"));
        let k = knowledge_block(&lib, &st, "Where is Castle Faallem?", "npc1", &opts);
        assert!(k.contains("About Castle Faallem: Castle Faallem "), "{k}");
    }

    #[test]
    fn same_npc_same_answer_different_npcs_vary() {
        let (lib, mut st) = active_states();
        st.enable_topic("S0000001", "_underking_");
        let opts = KnowledgeOptions::default();
        let q = "What about the Underking?";
        assert_eq!(knowledge_block(&lib, &st, q, "npc7", &opts), knowledge_block(&lib, &st, q, "npc7", &opts));
        let all: HashSet<String> = (0..40).map(|i| knowledge_block(&lib, &st, q, &format!("npc{i}"), &opts)).collect();
        assert!(all.len() >= 2, "tutti i PNG dicono la stessa cosa");
        let rumors: HashSet<String> = (0..40).filter_map(|i| rumor_for(&lib, &st, &format!("npc{i}"))).collect();
        assert!(rumors.len() >= 3, "{rumors:?}");
    }

    #[test]
    fn knowledge_is_capped_and_empty_when_nothing_fits() {
        let (lib, mut st) = active_states();
        st.enable_topic("S0000001", "_underking_");
        let tiny = KnowledgeOptions { max_chars: 20, ..KnowledgeOptions::default() };
        assert_eq!(knowledge_block(&lib, &st, "the Underking", "npc1", &tiny), ""); // niente intestazione sola
        let none = KnowledgeOptions { max_rumors: 0, max_topics: 0, ..KnowledgeOptions::default() };
        assert_eq!(knowledge_block(&lib, &st, "the Underking", "npc1", &none), "");
    }

    #[test]
    fn rumors_after_the_quest_come_from_the_post_messages() {
        let lib = lib_with_sample();
        let mut st = QuestStates::default();
        st.set_phase(lib.get("S0000001").unwrap(), QuestPhase::Completed);
        assert_eq!(rumor_for(&lib, &st, "x").as_deref(), Some("Prince Lhotun's mood has gotten better."));
        st.set_phase(lib.get("S0000001").unwrap(), QuestPhase::Failed);
        assert_eq!(rumor_for(&lib, &st, "x"), None);
        // le informazioni su persone e luoghi servono solo mentre la quest e' attiva
        st.enable_topic("S0000001", "_underking_");
        st.set_phase(lib.get("S0000001").unwrap(), QuestPhase::Completed);
        assert_eq!(knowledge_block(&lib, &st, "the Underking", "x", &KnowledgeOptions { max_rumors: 0, ..Default::default() }), "");
    }

    #[test]
    fn library_loads_only_main_quest_files() {
        let dir = std::env::temp_dir().join(format!("dfquest_lib_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("S0000001.txt"), SAMPLE).unwrap();
        std::fs::write(dir.join("s0000002.TXT"), SAMPLE.replace("S0000001", "S0000002")).unwrap();
        std::fs::write(dir.join("_tutorial_.txt"), SAMPLE).unwrap(); // non e' un quest principale
        std::fs::write(dir.join("Q0C00Y00.txt"), SAMPLE).unwrap();
        std::fs::write(dir.join("S0000003.txt"), "niente di valido").unwrap();
        let (lib, warnings) = Library::load_dir(&dir);
        assert_eq!(lib.len(), 2);
        assert!(lib.get("S0000001").is_some() && lib.get("S0000002").is_some());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("S0000003.txt"), "{warnings:?}");
        let (empty, w) = Library::load_dir(&dir.join("non_esiste"));
        assert!(empty.is_empty() && w.len() == 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
