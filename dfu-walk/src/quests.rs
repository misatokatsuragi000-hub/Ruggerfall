//! Quest generate da un LLM locale e voci (rumors) da ascoltare nei dialoghi.
//!
//! Porta in Rust il comportamento di `quest_lab.py`: per ogni citta' si genera UNA volta sola
//! un seed di quest (dungeon e committente scelti da un catalogo, nome esotico del luogo e
//! dell'artefatto, lore) e cinque voci, una per tipo di PNG (nobile, mercante, soldato,
//! popolano, ubriaco), con gli stessi prompt e gli stessi validatori dello script.
//!
//! In aggiunta, per ogni citta' vengono generate in background delle voci "flavor" specifiche
//! per ciascun PNG interno (taverne, negozi, gilde) identificato dalla sua descrizione esatta
//! in FLATS.CFG. L'LLM decide il tono e la personalita' basandosi sulla descrizione.
//! Queste voci sono salvate insieme alla quest in `quests.txt`.
//!
//! Per tutti gli altri PNG (quelli all'aperto e quelli interni senza voce flavor) la voce si
//! genera SU RICHIESTA, quando il giocatore sceglie "Ascolta rumori": una sola chiamata breve al
//! LLM con la descrizione di quel PNG (la caption di FLATS.CFG, oppure razza e sesso ricavati dal
//! suo volto). Il testo si salva in `quest_npc_voices.txt` e non si rigenera piu'.
//!
//! File nella cartella da cui si lancia il gioco (la cartella del progetto):
//!   - `quest_llm.conf`          configurazione (modello, llama-server, porta...). Se manca
//!                               viene creata con i valori di default.
//!   - `quests.txt`              le quest generate, una per riga in JSON. Si ricaricano
//!                               all'avvio: una citta' gia' presente non viene rigenerata.
//!   - `quest_seed_prompt.txt`, `quest_rumors_prompt.txt`, `quest_npc_flavor_prompt.txt`
//!                               (facoltativi) sostituiscono i prompt incorporati.
//!   - `quest_npc_voices.txt`    le voci generate su richiesta, una per riga in JSON.
//!   - `quest_npc_voice_prompt.txt` (facoltativo) prompt delle voci su richiesta.
//!   - `quest_npc_ask_prompt.txt` (facoltativo) prompt delle domande libere al PNG.
//!
//! Per non rallentare il rendering:
//!   - tutto il lavoro con il LLM (avvio di llama-server, richieste HTTP, validazione, scrittura
//!     del file) sta in UN thread dedicato; Bevy lo interroga solo con `try_recv`, mai in attesa;
//!   - llama-server parte solo alla prima quest da generare, con priorita' bassa (`nice`) e pochi
//!     thread CPU, e si spegne dopo `idle_stop_secs` di inattivita' (libera la VRAM);
//!   - una citta' genera una sola volta: nelle visite successive non c'e' nessun lavoro.
//!
//! Se nella porta configurata c'e' gia' un server che risponde (per esempio quello di
//! quest_lab.py) lo si riusa e non lo si spegne.
//!
//! VRAM: con il gioco aperto la scheda ha meno memoria libera di quando usi quest_lab.py. Con
//! `ngl=auto` (default) llama-server decide da solo quanti strati mettere sulla GPU; se la
//! memoria non basta il gioco riprova da solo con meno strati (fino a 0 = solo CPU).
//!
//! In `main.rs` servono:  `mod quests;`  e  `.add_plugins(quests::QuestPlugin)`

use crate::data::blocks::RmbBlock;
use crate::data::bsa::BsaArchive;
use crate::data::city_map::PoiKind;
use crate::data::flats::FlatCatalog;
use crate::data::maps::DFLocation;
use crate::player::Player;
use crate::world::{map_pixel_of, GameData, Origin};
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// =============================================================================
// Configurazione
// =============================================================================

pub const CONFIG_FILE: &str = "quest_llm.conf";
pub const QUESTS_FILE: &str = "quests.txt";
pub const SEED_PROMPT_FILE: &str = "quest_seed_prompt.txt";
pub const RUMORS_PROMPT_FILE: &str = "quest_rumors_prompt.txt";
pub const NPC_FLAVOR_PROMPT_FILE: &str = "quest_npc_flavor_prompt.txt";
pub const NPC_VOICE_PROMPT_FILE: &str = "quest_npc_voice_prompt.txt";
pub const VOICES_FILE: &str = "quest_npc_voices.txt";
pub const NPC_ASK_PROMPT_FILE: &str = "quest_npc_ask_prompt.txt";

const DEFAULT_SERVER_BIN: &str = "/home/actarus/.lmstudio/extensions/backends/llama.cpp-linux-x86_64-nvidia-cuda12-avx2-2.55.0/llama-server";
const DEFAULT_MODEL: &str = "/mnt/1CE16DBE48849731/Bionic-1.1.8-10-linux-x64.AppImage.home/models/0bserverx/Qwen3.8-27B-Heretic-Abliterated-Uncensored-GGUF/RVN-Q3_K_S.gguf";

/// Una localita' conta come citta'/villaggio se ha solo blocchi RMB e almeno questi blocchi
/// (esclude taverne, templi e fattorie, che sono un blocco solo).
const MIN_TOWN_BLOCKS: usize = 2;

/// Numero massimo di descrizioni distinte di FLATS.CFG per cui generare una voce flavor.
const MAX_NPC_FLAVORS: usize = 12;

#[derive(Clone, Debug, PartialEq)]
pub struct QuestConfig {
    /// false = nessuna quest, nessun thread, nessun server.
    pub enabled: bool,
    pub server_bin: String,
    pub model_path: String,
    pub port: u16,
    pub ctx: u32,
    /// Strati del modello sulla GPU (`-ngl`). `None` = automatico: llama-server sceglie in base
    /// alla VRAM libera in quel momento (il gioco ne usa una parte). Un numero fissa gli strati:
    /// meno strati = meno VRAM e meno concorrenza con il rendering, ma generazione piu' lenta;
    /// 0 = solo CPU. Se la VRAM non basta si riprova con meno strati.
    pub ngl: Option<u32>,
    /// Thread CPU di llama-server (`-t`); 0 = default di llama.cpp.
    pub threads: u32,
    /// Priorita' bassa del processo (`nice -n N`); 0 = nessuna.
    pub nice: u32,
    /// Passa `--reasoning-budget 0` (come "Disabilita thinking" di quest_lab.py).
    pub disable_thinking: bool,
    pub timeout_secs: u64,
    pub max_tokens: u32,
    pub startup_timeout_secs: u64,
    /// Dopo questo tempo senza lavoro il server (se l'abbiamo avviato noi) viene spento.
    pub idle_stop_secs: u64,
}

impl Default for QuestConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            server_bin: DEFAULT_SERVER_BIN.to_string(),
            model_path: DEFAULT_MODEL.to_string(),
            port: 8080,
            ctx: 8192,
            ngl: None,
            threads: 4,
            nice: 10,
            disable_thinking: true,
            timeout_secs: 300,
            max_tokens: 3072,
            startup_timeout_secs: 180,
            idle_stop_secs: 60,
        }
    }
}

fn parse_bool(v: &str, default: bool) -> bool {
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "si" | "sì" => true,
        "0" | "false" | "no" | "off" => false,
        _ => default,
    }
}

/// `auto` (o vuoto) = `None`; altrimenti un numero; un valore illeggibile lascia `default`.
fn parse_ngl(v: &str, default: Option<u32>) -> Option<u32> {
    if v.is_empty() || v.eq_ignore_ascii_case("auto") {
        None
    } else {
        v.parse().map(Some).unwrap_or(default)
    }
}

fn set_parsed<T: std::str::FromStr>(v: &str, slot: &mut T) {
    if let Ok(x) = v.parse() {
        *slot = x;
    }
}

impl QuestConfig {
    /// Legge il testo del file di configurazione (`chiave=valore`, `#` per i commenti).
    /// Chiavi sconosciute e valori non validi vengono ignorati: restano i default.
    pub fn parse(text: &str) -> Self {
        let mut c = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "enabled" => c.enabled = parse_bool(v, c.enabled),
                "server_bin" => c.server_bin = v.to_string(),
                "model_path" => c.model_path = v.to_string(),
                "port" => set_parsed(v, &mut c.port),
                "ctx" => set_parsed(v, &mut c.ctx),
                "ngl" => c.ngl = parse_ngl(v, c.ngl),
                "threads" => set_parsed(v, &mut c.threads),
                "nice" => set_parsed(v, &mut c.nice),
                "disable_thinking" => c.disable_thinking = parse_bool(v, c.disable_thinking),
                "timeout_secs" => set_parsed(v, &mut c.timeout_secs),
                "max_tokens" => set_parsed(v, &mut c.max_tokens),
                "startup_timeout_secs" => set_parsed(v, &mut c.startup_timeout_secs),
                "idle_stop_secs" => set_parsed(v, &mut c.idle_stop_secs),
                _ => {}
            }
        }
        c
    }

    pub fn to_text(&self) -> String {
        format!(
            "# Generatore di quest con LLM locale (llama-server). Modificalo e riavvia il gioco.\n\
# enabled=false disattiva tutto (nessun thread, nessun server).\n\
enabled={}\n\
\n\
# Eseguibile llama-server (le librerie CUDA accanto ad esso vengono trovate da sole)\n\
server_bin={}\n\
# Modello .gguf\n\
model_path={}\n\
\n\
port={}\n\
ctx={}\n\
# Strati sulla GPU: auto = li sceglie llama-server in base alla VRAM libera; oppure un\n\
# numero (0 = solo CPU). Se la VRAM non basta il gioco riprova con meno strati.\n\
ngl={}\n\
# Thread CPU di llama-server (0 = default di llama.cpp)\n\
threads={}\n\
# Priorita' bassa del processo (0 = disattivata)\n\
nice={}\n\
disable_thinking={}\n\
\n\
timeout_secs={}\n\
max_tokens={}\n\
startup_timeout_secs={}\n\
# Il server avviato dal gioco si spegne dopo tanti secondi senza lavoro\n\
idle_stop_secs={}\n",
self.enabled,
self.server_bin,
self.model_path,
self.port,
self.ctx,
self.ngl.map_or_else(|| "auto".to_string(), |n| n.to_string()),
                self.threads,
                self.nice,
                self.disable_thinking,
                self.timeout_secs,
                self.max_tokens,
                self.startup_timeout_secs,
                self.idle_stop_secs,
        )
    }

    /// Legge il file; se non esiste lo crea con i valori di default.
    pub fn load_or_create(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(_) => {
                let cfg = Self::default();
                if let Err(e) = std::fs::write(path, cfg.to_text()) {
                    warn!("Quest: impossibile creare {}: {e}", path.display());
                }
                cfg
            }
        }
    }
}

// =============================================================================
// JSON minimale (il progetto non ha serde_json)
// =============================================================================

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

const JSON_MAX_DEPTH: usize = 64;

struct JsonParser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> JsonParser<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("atteso '{}' alla posizione {}", c as char, self.i))
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(value)
        } else {
            Err(format!("valore non valido alla posizione {}", self.i))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > JSON_MAX_DEPTH {
            return Err("JSON troppo annidato".into());
        }
        self.ws();
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            Some(c) => Err(format!("carattere inatteso '{}' alla posizione {}", c as char, self.i)),
            None => Err("JSON incompleto".into()),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, String> {
        self.expect(b'{')?;
        let mut fields = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Json::Obj(fields));
        }
        loop {
            self.ws();
            let key = self.string()?;
            self.ws();
            self.expect(b':')?;
            let v = self.value(depth + 1)?;
            fields.push((key, v));
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Json::Obj(fields));
                }
                _ => return Err(format!("atteso ',' o '}}' alla posizione {}", self.i)),
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(format!("atteso ',' o ']' alla posizione {}", self.i)),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.b.get(self.i..self.i + 4).ok_or("escape \\u incompleto")?;
        let s = std::str::from_utf8(h).map_err(|_| "escape \\u non valido")?;
        let v = u32::from_str_radix(s, 16).map_err(|_| "escape \\u non valido".to_string())?;
        self.i += 4;
        Ok(v)
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(c) = self.peek() else { return Err("stringa non terminata".into()) };
            self.i += 1;
            match c {
                b'"' => return Ok(String::from_utf8_lossy(&out).into_owned()),
                b'\\' => {
                    let Some(e) = self.peek() else { return Err("escape incompleto".into()) };
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            if (0xD800..0xDC00).contains(&hi) && self.b[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                let cp = 0x10000 + ((hi - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                                char::from_u32(cp).unwrap_or('\u{FFFD}')
                            } else {
                                char::from_u32(hi).unwrap_or('\u{FFFD}')
                            }
                        }
                        other => return Err(format!("escape sconosciuto '\\{}'", other as char)),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                // gli a capo veri dentro una stringa sono accettati (i modelli ne producono)
                _ => out.push(c),
            }
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        while self.i < self.b.len() && matches!(self.b[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
            self.i += 1;
        }
        let text = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| "numero non valido")?;
        text.parse::<f64>().map(Json::Num).map_err(|_| format!("numero non valido '{text}'"))
    }
}

impl Json {
    pub fn parse(text: &str) -> Result<Json, String> {
        let mut p = JsonParser { b: text.as_bytes(), i: 0 };
        let v = p.value(0)?;
        p.ws();
        if p.i != text.len() {
            return Err(format!("testo dopo il JSON alla posizione {}", p.i));
        }
        Ok(v)
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(f) => f.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }

    /// Intero come lo leggerebbe `int()` di Python: numero (troncato) o stringa numerica.
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Json::Num(n) if n.is_finite() => Some(*n as i64),
            Json::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    fn write(&self, out: &mut String, indent: Option<usize>, level: usize) {
        let nl = |out: &mut String, lvl: usize| {
            if let Some(step) = indent {
                out.push('\n');
                out.extend(std::iter::repeat(' ').take(step * lvl));
            }
        };
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    out.push_str(&format!("{}", *n as i64));
                } else {
                    out.push_str(&format!("{n}"));
                }
            }
            Json::Str(s) => write_json_string(out, s),
            Json::Arr(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    nl(out, level + 1);
                    v.write(out, indent, level + 1);
                }
                nl(out, level);
                out.push(']');
            }
            Json::Obj(fields) => {
                if fields.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    nl(out, level + 1);
                    write_json_string(out, k);
                    out.push(':');
                    if indent.is_some() {
                        out.push(' ');
                    }
                    v.write(out, indent, level + 1);
                }
                nl(out, level);
                out.push('}');
            }
        }
    }

    /// Su una riga sola (formato di `quests.txt` e dei corpi HTTP).
    pub fn compact(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, None, 0);
        s
    }

    /// Indentato di 2 spazi, come `json.dumps(indent=2, ensure_ascii=False)`.
    pub fn pretty(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, Some(2), 0);
        s
    }
}

fn write_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn jstr(s: &str) -> Json {
    Json::Str(s.to_string())
}

// =============================================================================
// Catalogo, fazioni e prompt (estratti da quest_lab.py)
// =============================================================================

// Temi, nello stesso ordine di quest_lab.py
const THEMES: [&str; 8] = ["artifact_discovery", "rescue", "assassination", "escort", "investigation", "monster_hunt", "theft", "diplomacy"];

/// (id, nome tecnico inglese, nome italiano generico, regione, distanza in km)
const DUNGEONS: [(&str, &str, &str, &str, i64); 15] = [
    ("dgf_crypt_ash", "Crypt of Ash", "Cripta di Cenere", "Daggerfall", 12),
    ("dgf_barrow_mourn", "Mournhold Barrow", "Tumulo di Mournhold", "Daggerfall", 18),
    ("dgf_ruin_weeping", "The Weeping Hollow", "Cavo Piangente", "Daggerfall", 25),
    ("dgf_hollow_kings", "Hollow of the Kings", "Cavo dei Re", "Daggerfall", 30),
    ("sen_catacomb_tide", "Catacombs of the Tide", "Catacombe della Marea", "Sentinel", 10),
    ("sen_hall_silence", "The Silent Hall", "Sala del Silenzio", "Sentinel", 15),
    ("sen_tomb_drowned", "Tomb of the Drowned", "Tomba degli Annegati", "Sentinel", 22),
    ("way_crypt_pale", "The Pale Crypt", "Cripta Pallida", "Wayrest", 14),
    ("way_ruin_grinning", "Ruin of the Grinning God", "Rovina del Dio Ridente", "Wayrest", 19),
    ("way_vault_sorrow", "Vault of Sorrows", "Volta dei Dolori", "Wayrest", 28),
    ("ors_deep_forge", "The Deep Forge", "Fucina Profonda", "Orsinium", 16),
    ("ors_tomb_blood", "Blood-Tomb of Malacath", "Tomba di Sangue di Malacath", "Orsinium", 21),
    ("ors_cavern_echo", "The Echoing Caverns", "Caverne Risonanti", "Orsinium", 32),
    ("wild_tomb_seven", "Tomb of the Seven", "Tomba dei Sette", "Wilderness", 40),
    ("wild_crypt_last", "The Last Crypt", "Ultima Cripta", "Wilderness", 45),
];

/// (id, nome, tipo, sede). Nomi senza articolo: sono nomi propri.
const FACTIONS: [(&str, &str, &str, &str); 4] = [
    ("mages_guild", "Mages Guild", "guild", "Daggerfall"),
    ("fighters_guild", "Fighters Guild", "guild", "Daggerfall"),
    ("thieves_guild", "Thieves Guild", "guild", "Wayrest"),
    ("knights_dragon", "Knights of the Dragon", "order", "Daggerfall"),
];

/// Phrases that imply the artifact has already been recovered (forbidden in rumors).
const RECOVERED_HINTS: [&str; 10] = [
    "already recovered", "already found", "already taken", "already carried away",
"has been recovered", "has been found", "now in the hands of",
"already selling", "big deals with the", "already have",
];

// =============================================================================
// Prompt incorporati (IN INGLESE)
// =============================================================================

const DEFAULT_SEED_PROMPT: &str = r####"You are the author of Daggerfall quest seeds, a medieval dark fantasy world.
A quest seed is the "ground truth" behind a quest, before any NPC or
rumor tells it. You must choose a dungeon and a quest-giver from the provided lists.
Do NOT invent dungeons or factions: pick ONE exact id from each list.

Required theme: {theme}

AVAILABLE DUNGEONS (pick ONE by id):
{dungeon_list}

AVAILABLE QUEST-GIVERS (pick ONE by id):
{faction_list}

Your output must enrich the seed with:
- A short, evocative title (3-6 words), in English.
- An exotic name for the tomb/crypt/ruin (this will be the REAL name of the place,
the one NPCs who know it will mention). Must differ from the name in the dungeon list.
- An exotic name for the artifact / object at the center of the quest.
If the theme has no artifact (e.g. rescue, assassination), invent a name
for the "prize" or "target".
    - A 1-2 sentence lore, in English, explaining why this place and object matter.
    - A difficulty from 1 (trivial) to 5 (legendary).

    IMPORTANT CONSTRAINT: the quest location is ONLY the chosen dungeon. Do not mention
    other dungeons from the list. The "technical" name in parentheses is just a dev
    label: the world does not know it. NPCs who know the place will use the
    `tomb_name` you are about to invent.

    Reply ONLY with a JSON object with these fields:
    {{
    "theme": "<same as input>",
"title": "<short title in English>",
"dungeon_id": "<id from the list>",
"quest_giver_id": "<id from the list>",
"tomb_name": "<exotic name>",
"artifact_name": "<exotic name>",
"difficulty": <1-5>,
"lore": "<1-2 sentences in English>"
}}
Start your reply with '{{' and end with '}}'. No prose, no markdown, no reasoning."####;

const DEFAULT_RUMORS_PROMPT: &str = r####"You are generating RUMORS about a quest that exists in the world of Daggerfall.
The quest seed is the ground truth (this is what actually happened):
{seed_json}

CRITICAL RULE ON THE PLACE NAME:
- The quest location has ONE name in the seed: `tomb_name`. It is the only name
that exists for this place in the world.
- There is NO other "generic English" name for the place: if an NPC does not know
`tomb_name`, they must say "that place", "the crypt", "those ruins", "the hollow
outside town", not another proper name.
- If an NPC is a drunk, they may get `tomb_name` wrong and call it by a made-up
name, but must not use any other name from the catalog.
- Faction names are proper nouns WITHOUT an article:
write "Mages Guild", not "The Mages Guild".

CRITICAL RULE ON THE ARTIFACT:
- The artifact is still HIDDEN in the location. Nobody has recovered it.
- No NPC should talk about "deals" already underway with the artifact: only
speculation about what will happen IF it is recovered.

Generate FIVE rumor variants, one per tone. Each rumor is a short piece of
dialogue that a specific type of NPC would say if asked about the quest. They must:
- ALL refer to the SAME event, location and object from the seed.
- Differ in tone, reliability and level of detail.
- NEVER contradict the seed on facts (location, artifact, quest-giver).
- Be 1-2 sentences, spoken language, IN ENGLISH.
- Sound like gossip, not a formal briefing.

TONES, THEIR CHARACTER AND WHAT THEY KNOW:

- noble:
Knows: `tomb_name` (the exact exotic name), `artifact_name`, `quest_giver_name`.
Does NOT know / does not care: exact distance, number of mercenaries,
practical details. Does not bring up kilometer figures.
Tone: formal, slightly haughty, words like "Your Grace", "this matter",
"it behooves". Talks about politics, stability, honor, and the "handling" of
such discoveries.

- merchant:
Knows: `tomb_name`, `artifact_name`, approximate distance ("just outside",
"a couple of hours on foot"). Knows the region the place is in (`dungeon_region`).
Does NOT know / does not care: political or magical details.
Tone: practical, worried about prices, roads, requisitions, markets
that will close if the situation worsens. Does NOT talk about artifacts already
being traded: the artifact is still hidden.

- soldier:
Knows: the mission (who is going, roughly where), `artifact_name`. Has seen
the soldiers or mages leave. Does NOT know `tomb_name`: uses "that place",
"the crypt", "those ruins".
Tone: direct, martial, respects facts but does not embellish. Talks about
orders, patrols, "they won't come back until they have it".

- commoner:
Knows: only vague rumors. Does NOT know `tomb_name` (uses "the crypt outside town",
"that cursed place"). Does NOT know `artifact_name` (says "something that pulses",
"a strange object"). Does NOT know `quest_giver_name` (says "some lords",
"important folk", "the mages" at most). Has superstitions.
Tone: fearful, "they say", "I heard", "it scares me".

- drunk:
Knows: something, but all wrong. Gets `tomb_name` wrong (calls it by a made-up
name or another random name), gets the distance wrong ("ten or twenty
kilometers, dunno"), gets `quest_giver_name` wrong (calls the guild by another
name, or says "the squires", "the templars"). May also invent a non-existent
historical figure instead of the right one.
Tone: confused, repetitive, cheerful or whiny. Uses "let's drink!", "can't
remember", "dunno".

Reply ONLY with a JSON object:
{{
"rumors": [
{{"tone": "noble",    "text": "..." }},
{{"tone": "merchant", "text": "..." }},
{{"tone": "soldier",  "text": "..." }},
{{"tone": "commoner", "text": "..." }},
{{"tone": "drunk",    "text": "..." }}
]
}}
Start your reply with '{{' and end with '}}'. No prose, no markdown, no reasoning."####;

const DEFAULT_NPC_FLAVOR_PROMPT: &str = r####"You are generating RUMORS about a Daggerfall quest, each told by a SPECIFIC NPC.

The quest seed (ground truth):
{seed_json}

Below is a list of NPCs. Each line gives the EXACT description from FLATS.CFG.
You MUST write each rumor as if spoken BY that exact person. The description
defines who they are: their gender, appearance, profession, social standing,
and personality. Let the description fully determine the voice, vocabulary,
attitude, and what details they would know or care about.

Do NOT assign a generic "tone". A "beautiful maiden" speaks differently from
an "old beggar", a "guard", or a "King of Worms". Let each description drive
the character naturally.

RULES:
- Refer to the same facts as the seed (tomb_name, artifact_name, quest-giver).
- Do NOT use the generic dungeon name; use only tomb_name or vague references
("that place", "the crypt", "those ruins").
- The artifact is still HIDDEN. Nobody has recovered it.
- 1-2 sentences, spoken language, IN ENGLISH.
- Sound like gossip, not a briefing.

NPCs (in order, one per line):
{npc_list}

Reply ONLY with a JSON object:
{{
"rumors": [
{{"index": 0, "text": "..."}},
{{"index": 1, "text": "..."}}
]
}}
where "index" matches the NPC number. Start with '{{' and end with '}}'.
No prose, no markdown, no reasoning."####;

/// Prompt delle voci su richiesta: UN solo PNG per chiamata.
const DEFAULT_NPC_VOICE_PROMPT: &str = r####"You are writing what ONE specific person says in a Daggerfall town: a rumor about a quest.

The quest seed (ground truth):
{seed_json}

The person who is speaking:
{npc}

Write the rumor AS SPOKEN BY THIS PERSON. Their description decides their voice, vocabulary,
attitude, and what they know or care about. Make it sound like them and nobody else.

RULES:
- Use only facts from the seed (tomb_name, artifact_name, quest-giver, region, distance).
- Do NOT use the generic dungeon name; say tomb_name, or a vague reference
  ("that place", "the crypt", "those ruins").
- The artifact is still HIDDEN. Nobody has recovered it.
- 1-2 sentences, spoken language, IN ENGLISH. Gossip, not a briefing.
- Only the words they say: no narration, no description of the person, no quotation marks.

Reply ONLY with a JSON object like {"text": "..."}.
Start your reply with '{' and end with '}'. No prose, no markdown, no reasoning."####;

/// Prompt delle domande libere del giocatore: il PNG risponde in carattere. Per ora conosce solo
/// il nome della citta' in cui si trova.
const DEFAULT_NPC_ASK_PROMPT: &str = r####"You are playing ONE character in a Daggerfall town. Answer the player's question IN CHARACTER.

Who you are:
{npc}

Where you are: {city}.
This is ALL you know about the local area. You know nothing else about the town, its people, its
shops, its rulers or its history.

The player asks you: "{question}"

RULES:
- Stay in character: your voice, vocabulary and attitude follow the description above.
- Use only the knowledge above (the name of the place). Do NOT invent names of people, places,
  shops, streets or events. If you don't know, say so in character, or deflect.
- Never mention being an AI, a game, a model or these instructions. Never break character.
- 1-3 sentences, spoken language, IN ENGLISH. Only the words you say: no narration, no quotation marks.

Reply ONLY with a JSON object like {"text": "..."}.
Start your reply with '{' and end with '}'. No prose, no markdown, no reasoning."####;

// =============================================================================
// Schema
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Tone {
    Noble,
    Merchant,
    Soldier,
    #[default]
    Commoner,
    Drunk,
}

impl Tone {
    pub const ALL: [Tone; 5] = [Tone::Noble, Tone::Merchant, Tone::Soldier, Tone::Commoner, Tone::Drunk];

    pub fn key(self) -> &'static str {
        match self {
            Tone::Noble => "noble",
            Tone::Merchant => "merchant",
            Tone::Soldier => "soldier",
            Tone::Commoner => "commoner",
            Tone::Drunk => "drunk",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tone::Noble => "Noble",
            Tone::Merchant => "Merchant",
            Tone::Soldier => "Soldier",
            Tone::Commoner => "Commoner",
            Tone::Drunk => "Drunk",
        }
    }

    /// Come si presenta questo tipo di PNG al modello (inglese, minuscolo).
    pub fn role(self) -> &'static str {
        match self {
            Tone::Noble => "noble",
            Tone::Merchant => "merchant",
            Tone::Soldier => "soldier",
            Tone::Commoner => "commoner",
            Tone::Drunk => "drunkard",
        }
    }

    pub fn from_key(key: &str) -> Option<Tone> {
        Tone::ALL.into_iter().find(|t| t.key() == key)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Seed {
    pub theme: String,
    pub title: String,
    pub dungeon_id: String,
    /// Nome generico del dungeon (solo per validare: non si deve usare nelle voci).
    pub dungeon_name: String,
    /// Etichetta tecnica inglese, solo per debug.
    pub dungeon_name_dev: String,
    pub dungeon_region: String,
    pub dungeon_distance_km: i64,
    pub quest_giver_id: String,
    pub quest_giver_name: String,
    /// Nome esotico: l'unico che il mondo conosce per il luogo.
    pub tomb_name: String,
    pub artifact_name: String,
    pub difficulty: i64,
    pub lore: String,
}

impl Seed {
    /// Quello che vede il modello per generare le voci: NIENTE `dungeon_name`.
    fn to_prompt_json(&self) -> Json {
        Json::Obj(vec![
            ("theme".into(), jstr(&self.theme)),
                  ("title".into(), jstr(&self.title)),
                  ("tomb_name".into(), jstr(&self.tomb_name)),
                  ("artifact_name".into(), jstr(&self.artifact_name)),
                  ("dungeon_region".into(), jstr(&self.dungeon_region)),
                  ("dungeon_distance_km".into(), Json::Num(self.dungeon_distance_km as f64)),
                  ("quest_giver_id".into(), jstr(&self.quest_giver_id)),
                  ("quest_giver_name".into(), jstr(&self.quest_giver_name)),
                  ("difficulty".into(), Json::Num(self.difficulty as f64)),
                  ("lore".into(), jstr(&self.lore)),
        ])
    }

    fn to_full_json(&self) -> Json {
        Json::Obj(vec![
            ("theme".into(), jstr(&self.theme)),
                  ("title".into(), jstr(&self.title)),
                  ("dungeon_id".into(), jstr(&self.dungeon_id)),
                  ("dungeon_name".into(), jstr(&self.dungeon_name)),
                  ("dungeon_name_dev".into(), jstr(&self.dungeon_name_dev)),
                  ("dungeon_region".into(), jstr(&self.dungeon_region)),
                  ("dungeon_distance_km".into(), Json::Num(self.dungeon_distance_km as f64)),
                  ("quest_giver_id".into(), jstr(&self.quest_giver_id)),
                  ("quest_giver_name".into(), jstr(&self.quest_giver_name)),
                  ("tomb_name".into(), jstr(&self.tomb_name)),
                  ("artifact_name".into(), jstr(&self.artifact_name)),
                  ("difficulty".into(), Json::Num(self.difficulty as f64)),
                  ("lore".into(), jstr(&self.lore)),
        ])
    }

    fn from_full_json(j: &Json) -> Option<Seed> {
        let s = |k: &str| j.get(k).and_then(Json::as_str).map(str::to_string);
        Some(Seed {
            theme: s("theme")?,
             title: s("title")?,
             dungeon_id: s("dungeon_id")?,
             dungeon_name: s("dungeon_name")?,
             dungeon_name_dev: s("dungeon_name_dev").unwrap_or_default(),
             dungeon_region: s("dungeon_region")?,
             dungeon_distance_km: j.get("dungeon_distance_km")?.as_int()?,
             quest_giver_id: s("quest_giver_id")?,
             quest_giver_name: s("quest_giver_name")?,
             tomb_name: s("tomb_name")?,
             artifact_name: s("artifact_name")?,
             difficulty: j.get("difficulty")?.as_int()?,
             lore: s("lore")?,
        })
    }
}

/// Voce flavor di un NPC specifico, generata dalla sua descrizione esatta in FLATS.CFG.
#[derive(Clone, Debug, PartialEq)]
pub struct NpcFlavor {
    pub archive: u16,
    pub record: u8,
    pub caption: String,
    pub text: String,
}

/// Una quest di una citta': il seed, le cinque voci per tono e le voci flavor per NPC.
#[derive(Clone, Debug, PartialEq)]
pub struct Quest {
    pub city: (i32, i32),
    pub city_name: String,
    pub seed: Seed,
    pub rumors: Vec<(Tone, String)>,
    pub npc_flavors: Vec<NpcFlavor>,
}

impl Quest {
    pub fn rumor(&self, tone: Tone) -> Option<&str> {
        self.rumors.iter().find(|(t, _)| *t == tone).map(|(_, text)| text.as_str())
    }

    fn to_json(&self) -> Json {
        Json::Obj(vec![
            ("city_x".into(), Json::Num(self.city.0 as f64)),
                  ("city_y".into(), Json::Num(self.city.1 as f64)),
                  ("city_name".into(), jstr(&self.city_name)),
                  ("seed".into(), self.seed.to_full_json()),
                  (
                      "rumors".into(),
                   Json::Arr(
                       self.rumors
                       .iter()
                       .map(|(t, text)| Json::Obj(vec![("tone".into(), jstr(t.key())), ("text".into(), jstr(text))]))
                       .collect(),
                   ),
                  ),
                  (
                      "npc_flavors".into(),
                   Json::Arr(
                       self.npc_flavors
                       .iter()
                       .map(|nf| {
                           Json::Obj(vec![
                               ("archive".into(), Json::Num(nf.archive as f64)),
                                     ("record".into(), Json::Num(nf.record as f64)),
                                     ("caption".into(), jstr(&nf.caption)),
                                     ("text".into(), jstr(&nf.text)),
                           ])
                       })
                       .collect(),
                   ),
                  ),
        ])
    }

    fn from_json(j: &Json) -> Option<Quest> {
        let rumors = j
        .get("rumors")?
        .as_array()?
        .iter()
        .filter_map(|r| Some((Tone::from_key(r.get("tone")?.as_str()?)?, r.get("text")?.as_str()?.to_string())))
        .collect();
        let npc_flavors = j
        .get("npc_flavors")
        .and_then(Json::as_array)
        .map(|arr| {
            arr.iter()
            .filter_map(|o| {
                Some(NpcFlavor {
                    archive: o.get("archive")?.as_int()? as u16,
                     record: o.get("record")?.as_int()? as u8,
                     caption: o.get("caption")?.as_str()?.to_string(),
                     text: o.get("text")?.as_str()?.to_string(),
                })
            })
            .collect()
        })
        .unwrap_or_default();
        Some(Quest {
            city: (j.get("city_x")?.as_int()? as i32, j.get("city_y")?.as_int()? as i32),
             city_name: j.get("city_name")?.as_str()?.to_string(),
             seed: Seed::from_full_json(j.get("seed")?)?,
             rumors,
             npc_flavors,
        })
    }
}

// =============================================================================
// Prompt e validatori
// =============================================================================

fn dungeon_list() -> String {
    DUNGEONS
    .iter()
    .map(|(id, _dev, it, region, km)| format!("- id={id} | {it} | region={region} | {km}km"))
    .collect::<Vec<_>>()
    .join("\n")
}

fn faction_list() -> String {
    FACTIONS
    .iter()
    .filter(|(_, _, kind, _)| *kind == "guild" || *kind == "order")
    .map(|(id, name, kind, home)| format!("- id={id} | {name} | type={kind} | based in {home}"))
    .collect::<Vec<_>>()
    .join("\n")
}

pub fn build_seed_prompt(template: &str, theme: &str) -> String {
    template
    .replace("{theme}", theme)
    .replace("{dungeon_list}", &dungeon_list())
    .replace("{faction_list}", &faction_list())
}

pub fn build_rumors_prompt(template: &str, seed: &Seed) -> String {
    template.replace("{seed_json}", &seed.to_prompt_json().pretty())
}

pub fn build_npc_flavor_prompt(template: &str, seed: &Seed, npc_list: &str) -> String {
    template
    .replace("{seed_json}", &seed.to_prompt_json().pretty())
    .replace("{npc_list}", npc_list)
}

fn validate_seed(ev: &Json) -> Result<(), String> {
    if !matches!(ev, Json::Obj(_)) {
        return Err("output is not a JSON object".into());
    }
    for k in ["theme", "title", "dungeon_id", "quest_giver_id", "tomb_name", "artifact_name", "difficulty", "lore"] {
        if ev.get(k).is_none() {
            return Err(format!("missing field: {k}"));
        }
    }
    let text = |k: &str| ev.get(k).and_then(Json::as_str).unwrap_or("").trim().to_string();
    let theme = text("theme");
    if !THEMES.contains(&theme.as_str()) {
        return Err(format!("theme '{theme}' not in {THEMES:?}"));
    }
    let dungeon_id = text("dungeon_id");
    if !DUNGEONS.iter().any(|d| d.0 == dungeon_id) {
        return Err(format!("dungeon_id '{dungeon_id}' not in catalog"));
    }
    let giver = text("quest_giver_id");
    match FACTIONS.iter().find(|f| f.0 == giver) {
        None => return Err(format!("quest_giver_id '{giver}' not in factions")),
        Some((_, _, kind, _)) if *kind != "guild" && *kind != "order" => {
            return Err(format!("quest-giver '{giver}' is not a guild/order"))
        }
        _ => {}
    }
    match ev.get("difficulty").and_then(Json::as_int) {
        None => return Err("difficulty not an integer".into()),
        Some(d) if !(1..=5).contains(&d) => return Err(format!("difficulty {d} out of range 1..5")),
        _ => {}
    }
    for (k, what) in [("title", "title empty"), ("tomb_name", "tomb_name empty"), ("artifact_name", "artifact_name empty"), ("lore", "lore empty")] {
        if text(k).is_empty() {
            return Err(what.into());
        }
    }
    Ok(())
}

fn enrich_seed(ev: &Json) -> Result<Seed, String> {
    let text = |k: &str| ev.get(k).and_then(Json::as_str).unwrap_or("").trim().to_string();
    let dungeon_id = text("dungeon_id");
    let (_, name_dev, name_it, region, km) = DUNGEONS
    .iter()
    .find(|d| d.0 == dungeon_id)
    .ok_or_else(|| format!("dungeon_id '{dungeon_id}' not in catalog"))?;
    let giver_id = text("quest_giver_id");
    let (_, giver_name, _, _) = FACTIONS
    .iter()
    .find(|f| f.0 == giver_id)
    .ok_or_else(|| format!("quest_giver_id '{giver_id}' not in factions"))?;
    Ok(Seed {
        theme: text("theme"),
       title: text("title"),
       dungeon_id,
       dungeon_name: name_it.to_string(),
       dungeon_name_dev: name_dev.to_string(),
       dungeon_region: region.to_string(),
       dungeon_distance_km: *km,
       quest_giver_id: giver_id,
       quest_giver_name: giver_name.to_string(),
       tomb_name: text("tomb_name"),
       artifact_name: text("artifact_name"),
       difficulty: ev.get("difficulty").and_then(Json::as_int).unwrap_or(1),
       lore: text("lore"),
    })
}

/// Lowercase with spaces collapsed: for searching a name inside text.
fn normalized(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn validate_rumors(ev: &Json, seed: &Seed) -> Result<(), String> {
    let list = ev
    .get("rumors")
    .ok_or("missing 'rumors' field")?
    .as_array()
    .ok_or("expected 5 rumors, got non-list")?;
    if list.len() != 5 {
        return Err(format!("expected 5 rumors, got {}", list.len()));
    }
    let mut seen: HashSet<&'static str> = HashSet::new();
    let dungeon_name = normalized(&seed.dungeon_name);
    let artifact = seed.artifact_name.to_lowercase();
    for r in list {
        if !matches!(r, Json::Obj(_)) {
            return Err("rumor is not an object".into());
        }
        let tone_key = r.get("tone").and_then(Json::as_str).unwrap_or("");
        let text = r.get("text").and_then(Json::as_str).unwrap_or("").trim();
        let Some(tone) = Tone::from_key(tone_key) else {
            return Err(format!("invalid tone '{tone_key}'"));
        };
        if !seen.insert(tone.key()) {
            return Err(format!("duplicate tone '{tone_key}'"));
        }
        if text.is_empty() {
            return Err(format!("tone '{tone_key}' has empty text"));
        }
        let len = text.chars().count();
        if len > 500 {
            return Err(format!("tone '{tone_key}' text too long ({len} chars)"));
        }
        if normalized(text).contains(&dungeon_name) {
            return Err(format!(
                "tone '{tone_key}' uses generic name '{}' instead of tomb_name '{}'. The place has one name: '{}' (or a vague 'the crypt/that place' if the NPC doesn't know it).",
                               seed.dungeon_name, seed.tomb_name, seed.tomb_name
            ));
        }
        let lower = text.to_lowercase();
        for hint in RECOVERED_HINTS {
            if lower.contains(hint) && lower.contains(&artifact) {
                return Err(format!(
                    "tone '{tone_key}' implies the artifact was already recovered ('{hint}'). The seed says it is still hidden."
                ));
            }
        }
    }
    Ok(())
}

fn rumors_from_json(ev: &Json) -> Result<Vec<(Tone, String)>, String> {
    ev.get("rumors")
    .and_then(Json::as_array)
    .ok_or("missing 'rumors' field")?
    .iter()
    .map(|r| {
        let tone = r.get("tone").and_then(Json::as_str).and_then(Tone::from_key).ok_or("invalid tone")?;
        let text = r.get("text").and_then(Json::as_str).ok_or("missing text")?;
        Ok((tone, text.trim().to_string()))
    })
    .collect()
}

fn validate_npc_flavors(ev: &Json) -> Result<(), String> {
    let arr = ev.get("rumors").and_then(Json::as_array).ok_or("missing 'rumors' field")?;
    if arr.is_empty() {
        return Err("no rumors generated".into());
    }
    Ok(())
}

// =============================================================================
// Raccolta NPC e generazione flavor
// =============================================================================

/// Building types the player can enter (taverns, guilds, shops).
fn is_enterable_type(bt: u8) -> bool {
    matches!(bt, 0 | 2 | 5 | 6 | 7 | 8 | 9 | 11 | 12 | 13 | 14 | 15)
}

/// Collects NPC flats from enterable building interiors.
/// Searches both `people` (interactive NPCs) and `flats` with faction_id != 0
/// (static NPCs like beggars).
fn collect_npc_flats(
    location: &DFLocation,
    blocks_buf: &[u8],
    catalog: &FlatCatalog,
) -> Vec<(u16, u8, String)> {
    let Ok(blocks_bsa) = BsaArchive::parse(blocks_buf) else { return Vec::new() };
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for by in 0..location.height {
        for bx in 0..location.width {
            let Some(name) = location.block_names.get(by * location.width + bx) else { continue };
            let Some(raw) = blocks_bsa.get_by_name(name) else { continue };
            let Ok(rmb) = RmbBlock::parse(raw) else { continue };
            for (i, it) in rmb.interiors.iter().enumerate() {
                let enterable = rmb.buildings.get(i).map_or(false, |b| is_enterable_type(b.building_type));
                if !enterable { continue; }
                // 1. People (interactive NPCs: innkeeper, customers, merchants)
                for p in &it.people {
                    let key = (p.texture_archive, p.texture_record);
                    if !seen.insert(key) { continue; }
                    if let Some(info) = catalog.get(p.texture_archive, p.texture_record) {
                        if !info.caption.is_empty() {
                            out.push((p.texture_archive, p.texture_record, info.caption.clone()));
                        }
                    }
                }
                // 2. Flats with faction_id != 0 (static NPCs: beggars, guards)
                for f in &it.flats {
                    if f.faction_id == 0 { continue; }
                    let key = (f.texture_archive, f.texture_record);
                    if !seen.insert(key) { continue; }
                    if let Some(info) = catalog.get(f.texture_archive, f.texture_record) {
                        if !info.caption.is_empty() {
                            out.push((f.texture_archive, f.texture_record, info.caption.clone()));
                        }
                    }
                }
            }
        }
    }
    out.truncate(128);
    out
}

/// Generates one flavored rumor per distinct FLATS.CFG caption (single LLM call).
/// Non-fatal: if it fails, the quest is saved without flavors and falls back to tones.
fn generate_npc_flavors(
    cfg: &QuestConfig,
    tpl: &Templates,
    seed: &Seed,
    npc_flats: &[(u16, u8, String)],
) -> Vec<NpcFlavor> {
    if npc_flats.is_empty() { return Vec::new(); }
    // Group by exact caption: NPCs with the same description share the same rumor.
    let mut order: Vec<String> = Vec::new();
    let mut by_cap: HashMap<String, Vec<(u16, u8)>> = HashMap::new();
    for (a, r, cap) in npc_flats {
        if !by_cap.contains_key(cap) {
            order.push(cap.clone());
        }
        by_cap.entry(cap.clone()).or_default().push((*a, *r));
    }
    order.truncate(MAX_NPC_FLAVORS);
    // The exact caption, quoted, is all the LLM sees as the character.
    let npc_list = order
    .iter()
    .enumerate()
    .map(|(i, c)| format!("[{i}] \"{c}\""))
    .collect::<Vec<_>>()
    .join("\n");
    let prompt = build_npc_flavor_prompt(&tpl.npc_flavor, seed, &npc_list);
    let ev = match try_json(cfg, &prompt, "NPC_FLAVORS", validate_npc_flavors) {
        Ok(ev) => ev,
        Err(e) => { warn!("Quest [NPC_FLAVORS]: {e}"); return Vec::new(); }
    };
    let Some(arr) = ev.get("rumors").and_then(Json::as_array) else { return Vec::new() };
    let mut out = Vec::new();
    for item in arr {
        let idx = item.get("index").and_then(Json::as_int).unwrap_or(-1);
        if idx < 0 || (idx as usize) >= order.len() { continue; }
        let cap = &order[idx as usize];
        let text = item.get("text").and_then(Json::as_str).unwrap_or("").trim().to_string();
        if text.is_empty() || text.chars().count() > 500 { continue; }
        for &(a, r) in &by_cap[cap] {
            out.push(NpcFlavor { archive: a, record: r, caption: cap.clone(), text: text.clone() });
        }
    }
    out
}

// =============================================================================
// Voci dei PNG su richiesta
// =============================================================================
//
// Le voci "flavor" in blocco coprono poche descrizioni. Qui ogni PNG con cui si parla riceve la
// sua voce quando serve, con una chiamata breve (un PNG per volta: molto piu' affidabile con i
// modelli piccoli), e il testo si conserva in `quest_npc_voices.txt`.

/// Lunghezza massima (caratteri) di una voce.
pub const VOICE_MAX_CHARS: usize = 400;

/// I volti di `TFAC00I0.RCI` sono a blocchi di 24 per razza e sesso.
const FACE_BLOCK: usize = 24;

const TEMPERAMENTS: [&str; 12] = [
    "nervous", "cheerful", "weary", "proud", "curious", "suspicious", "friendly", "gloomy", "talkative", "blunt",
    "superstitious", "greedy",
];

/// Richiesta di una voce per un PNG, con il seed della quest della sua citta'.
struct VoiceJob {
    city: (i32, i32),
    key: String,
    description: String,
    seed: Seed,
}

/// Chiave di un PNG interno riconosciuto da FLATS.CFG (tutti i flat uguali parlano allo stesso modo).
pub fn flat_key(archive: u16, record: u8) -> String {
    format!("flat:{archive}:{record}")
}

/// Chiave di un PNG interno senza caption: in base al suo volto.
pub fn face_key(face_record: usize) -> String {
    format!("face:{face_record}")
}

/// Chiave di un passante all'aperto: in base al suo volto (stesso volto = stessa persona).
pub fn walker_key(face_record: usize) -> String {
    format!("walker:{face_record}")
}

/// Razza e sesso di un passante, dal blocco di volti (`face_record / 24`). I blocchi sono quelli
/// degli archivi persona di DFU; dove piu' razze condividono un blocco si dice solo il sesso.
pub fn walker_look(face_record: usize) -> &'static str {
    match face_record / FACE_BLOCK {
        0 | 2 => "Nord woman",
        1 => "Breton woman",
        3 => "woman",
        4..=6 => "Redguard woman",
        8 => "Breton man",
        9 | 10 => "man",
        11 => "Nord man",
        13 | 14 => "Redguard man",
        _ => "townsperson",
    }
}

pub fn place_label(place: Option<PoiKind>) -> &'static str {
    match place {
        Some(PoiKind::Tavern) => "inside a tavern",
        Some(PoiKind::Shop) => "inside a shop",
        Some(_) => "inside a building",
        None => "out on the street",
    }
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
}

/// Carattere del PNG, stabile (stessa chiave e citta' = stesso carattere): da' varieta' alle voci
/// di persone con lo stesso aspetto.
pub fn temperament_for(key: &str, city: (i32, i32)) -> &'static str {
    let h = splitmix(fnv1a(key) ^ ((city.0 as u64) << 20) ^ (city.1 as u64).wrapping_mul(0x1_0001));
    TEMPERAMENTS[(h % TEMPERAMENTS.len() as u64) as usize]
}

/// Il blocco `{npc}` del prompt: chi e' e come appare.
pub fn describe_npc(who: &str, role: Option<&str>, place: Option<PoiKind>, temperament: &str) -> String {
    let mut s = format!("- Appearance: {who}\n");
    if let Some(r) = role {
        s.push_str(&format!("- Standing: {r}\n"));
    }
    s.push_str(&format!("- Location: {}\n", place_label(place)));
    s.push_str(&format!("- Temperament (not visible, shapes how they talk): {temperament}"));
    s
}

pub fn build_npc_voice_prompt(template: &str, seed: &Seed, npc: &str) -> String {
    template.replace("{seed_json}", &seed.to_prompt_json().pretty()).replace("{npc}", npc)
}

/// Le stesse regole delle voci per tono: niente nome generico del dungeon, artefatto ancora nascosto.
fn validate_voice(ev: &Json, seed: &Seed) -> Result<(), String> {
    let text = ev.get("text").and_then(Json::as_str).ok_or("missing 'text' field")?.trim();
    if text.is_empty() {
        return Err("empty text".into());
    }
    let len = text.chars().count();
    if len > VOICE_MAX_CHARS {
        return Err(format!("text too long ({len} chars, max {VOICE_MAX_CHARS})"));
    }
    if normalized(text).contains(&normalized(&seed.dungeon_name)) {
        return Err(format!(
            "uses the generic name '{}' instead of '{}'. Say '{}' (or a vague 'that place').",
            seed.dungeon_name, seed.tomb_name, seed.tomb_name
        ));
    }
    let lower = text.to_lowercase();
    let artifact = seed.artifact_name.to_lowercase();
    for hint in RECOVERED_HINTS {
        if lower.contains(hint) && lower.contains(&artifact) {
            return Err(format!("implies the artifact was already recovered ('{hint}'): it is still hidden"));
        }
    }
    Ok(())
}

/// Una voce per un PNG: fino a 3 tentativi con il motivo del rifiuto.
fn generate_voice(cfg: &QuestConfig, tpl: &Templates, seed: &Seed, npc: &str) -> Result<String, String> {
    let prompt = build_npc_voice_prompt(&tpl.voice, seed, npc);
    let ev = try_json(cfg, &prompt, "NPC_VOICE", |ev| validate_voice(ev, seed))?;
    let text = ev.get("text").and_then(Json::as_str).unwrap_or("").trim();
    // il dialogo mette le virgolette: se il modello le ha messe lo stesso, si tolgono
    Ok(text.trim_matches('"').trim().to_string())
}

fn append_voice(path: &Path, city: (i32, i32), key: &str, description: &str, text: &str) -> std::io::Result<()> {
    let line = Json::Obj(vec![
        ("city_x".into(), Json::Num(city.0 as f64)),
        ("city_y".into(), Json::Num(city.1 as f64)),
        ("key".into(), jstr(key)),
        ("description".into(), jstr(description)),
        ("text".into(), jstr(text)),
    ])
    .compact();
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{line}")
}

// =============================================================================
// Domande libere del giocatore
// =============================================================================

/// Lunghezza massima (caratteri) di una domanda.
pub const QUESTION_MAX_CHARS: usize = 200;
const ANSWER_MAX_CHARS: usize = 500;

/// Una domanda del giocatore a un PNG.
struct AskJob {
    id: u64,
    city_name: String,
    description: String,
    question: String,
}

/// La domanda come la vede il modello: spazi ridotti a uno, niente caratteri di controllo, al
/// massimo `QUESTION_MAX_CHARS` caratteri. None se resta vuota.
pub fn clean_question(raw: &str) -> Option<String> {
    let no_control: String = raw.chars().filter(|c| !c.is_control() || c.is_whitespace()).collect();
    let single_spaced = no_control.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = single_spaced.chars().take(QUESTION_MAX_CHARS).collect();
    let cut = cut.trim().to_string();
    (!cut.is_empty()).then_some(cut)
}

/// La domanda va sostituita per ultima: se contiene `{npc}` o `{city}` non viene riespansa.
pub fn build_npc_ask_prompt(template: &str, city_name: &str, npc: &str, question: &str) -> String {
    let city = if city_name.trim().is_empty() { "an unnamed place" } else { city_name.trim() };
    template.replace("{city}", city).replace("{npc}", npc).replace("{question}", question)
}

fn validate_answer(ev: &Json) -> Result<(), String> {
    let text = ev.get("text").and_then(Json::as_str).ok_or("missing 'text' field")?.trim();
    if text.is_empty() {
        return Err("empty text".into());
    }
    let len = text.chars().count();
    if len > ANSWER_MAX_CHARS {
        return Err(format!("text too long ({len} chars, max {ANSWER_MAX_CHARS})"));
    }
    let lower = text.to_lowercase();
    for phrase in ["as an ai", "language model", "i am an ai", "i'm an ai", "as a language", "i cannot assist"] {
        if lower.contains(phrase) {
            return Err(format!("breaks character ('{phrase}'): stay in character, you are a person in the town"));
        }
    }
    Ok(())
}

fn generate_answer(cfg: &QuestConfig, tpl: &Templates, job: &AskJob) -> Result<String, String> {
    let prompt = build_npc_ask_prompt(&tpl.ask, &job.city_name, &job.description, &job.question);
    let ev = try_json(cfg, &prompt, "NPC_ASK", validate_answer)?;
    let text = ev.get("text").and_then(Json::as_str).unwrap_or("").trim();
    Ok(text.trim_matches('"').trim().to_string())
}

/// Una domanda del giocatore: avvia il server se serve, genera la risposta e la restituisce.
fn run_ask(cfg: &QuestConfig, tpl: &Templates, server: &SharedServer, a: &AskJob) -> QuestMsg {
    let started = Instant::now();
    match ensure_server(cfg, server).and_then(|_| generate_answer(cfg, tpl, a)) {
        Ok(text) => {
            info!("Quest: answer #{} ready in {:?}", a.id, started.elapsed());
            QuestMsg::Answer { id: a.id, text }
        }
        Err(reason) => {
            warn!("Quest: answer #{}: {reason}", a.id);
            QuestMsg::AnswerFailed { id: a.id }
        }
    }
}

// =============================================================================
// Estrazione del JSON dalla risposta del modello
// =============================================================================

/// Strips "reasoning" blocks (<think>...</think> and similar).
fn strip_thinking(text: &str) -> String {
    const PAIRS: [(&str, &str, bool); 4] = [
        ("<think", "</think", true),
        ("<reasoning", "</reasoning", true),
        ("<thought", "</thought", true),
        ("<|thinking|>", "<|/thinking|>", false),
    ];
    let mut out = text.to_string();
    for (open, close, close_with_gt) in PAIRS {
        loop {
            let lower = out.to_ascii_lowercase();
            let Some(start) = lower.find(open) else { break };
            let Some(rel) = lower[start..].find(close) else { break };
            let close_start = start + rel;
            let end = if close_with_gt {
                match lower[close_start..].find('>') {
                    Some(g) => close_start + g + 1,
                    None => break,
                }
            } else {
                close_start + close.len()
            };
            out.replace_range(start..end, "");
        }
    }
    out
}

pub fn extract_json(text: &str) -> String {
    let stripped = strip_thinking(text);
    let mut t = stripped.trim();
    t = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")).unwrap_or(t);
    t = t.strip_suffix("```").unwrap_or(t).trim();
    match (t.find('{'), t.rfind('}')) {
        (Some(s), Some(e)) if e > s => t[s..=e].to_string(),
        _ => t.to_string(),
    }
}

// =============================================================================
// HTTP minimale verso llama-server (127.0.0.1, niente TLS)
// =============================================================================

fn decode_chunked(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let Some(nl) = data[i..].windows(2).position(|w| w == b"\r\n") else { break };
        let size_line = String::from_utf8_lossy(&data[i..i + nl]);
        let size_hex = size_line.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_hex, 16) else { break };
        i += nl + 2;
        if size == 0 || i + size > data.len() {
            if size > 0 {
                out.extend_from_slice(&data[i..]);
            }
            break;
        }
        out.extend_from_slice(&data[i..i + size]);
        i += size + 2;
    }
    out
}

/// (status code, body) from a complete HTTP/1.x response.
fn parse_http_response(raw: &[u8]) -> Result<(u16, String), String> {
    let split = raw
    .windows(4)
    .position(|w| w == b"\r\n\r\n")
    .ok_or("incomplete HTTP response")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let body = &raw[split + 4..];
    let status: u16 = head
    .lines()
    .next()
    .and_then(|l| l.split_whitespace().nth(1))
    .and_then(|c| c.parse().ok())
    .ok_or("invalid HTTP status line")?;
    let chunked = head
    .lines()
    .skip(1)
    .any(|l| l.to_ascii_lowercase().replace(' ', "").starts_with("transfer-encoding:chunked"));
    let body = if chunked { decode_chunked(body) } else { body.to_vec() };
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
}

fn http_request(port: u16, method: &str, path: &str, body: Option<&str>, read_timeout: Duration) -> Result<(u16, String), String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream =
    TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| format!("connection to {addr}: {e}"))?;
    stream.set_read_timeout(Some(read_timeout)).ok();
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).map_err(|e| format!("sending request: {e}"))?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| format!("reading response: {e}"))?;
    parse_http_response(&raw)
}

fn server_health(port: u16) -> bool {
    for path in ["/health", "/v1/models"] {
        if let Ok((status, _)) = http_request(port, "GET", path, None, Duration::from_millis(1500)) {
            if (200..300).contains(&status) {
                return true;
            }
        }
    }
    false
}

fn chat_body(cfg: &QuestConfig, prompt: &str, with_template_kwargs: bool) -> String {
    let mut fields = vec![
        ("model".to_string(), jstr("llm")),
        (
            "messages".to_string(),
         Json::Arr(vec![Json::Obj(vec![("role".into(), jstr("user")), ("content".into(), jstr(prompt))])]),
        ),
        ("temperature".to_string(), Json::Num(0.85)),
        ("max_tokens".to_string(), Json::Num(cfg.max_tokens as f64)),
    ];
    if with_template_kwargs {
        fields.push((
            "chat_template_kwargs".to_string(),
                     Json::Obj(vec![("enable_thinking".into(), Json::Bool(false))]),
        ));
    }
    Json::Obj(fields).compact()
}

/// One completion request. Like the script: if the server rejects `chat_template_kwargs`
/// (HTTP 400/422) retry without.
fn call_llm(cfg: &QuestConfig, prompt: &str) -> Result<String, String> {
    let timeout = Duration::from_secs(cfg.timeout_secs.max(1));
    let mut with_kwargs = true;
    loop {
        let body = chat_body(cfg, prompt, with_kwargs);
        let (status, text) = http_request(cfg.port, "POST", "/v1/chat/completions", Some(&body), timeout)?;
        if matches!(status, 400 | 422) && with_kwargs {
            with_kwargs = false;
            continue;
        }
        if status != 200 {
            let head: String = text.chars().take(200).collect();
            return Err(format!("HTTP {status}: {head}"));
        }
        let json = Json::parse(&text).map_err(|e| format!("invalid server response: {e}"))?;
        return json
        .get("choices")
        .and_then(Json::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Json::as_str)
        .map(str::to_string)
        .ok_or_else(|| "response without choices[0].message.content".to_string());
    }
}

// =============================================================================
// Gestione del processo llama-server
// =============================================================================

fn collect_lib_dirs(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 3 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut subdirs = Vec::new();
    let mut has_cuda_lib = false;
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        match entry.file_type() {
            Ok(t) if t.is_dir() => subdirs.push(entry.path()),
            _ => {
                if ["libcudart.so", "libcublas.so", "libcublasLt.so", "libcudnn.so", "libnvJitLink.so"]
                    .iter()
                    .any(|p| name.starts_with(p))
                    {
                        has_cuda_lib = true;
                    }
            }
        }
    }
    if has_cuda_lib && !out.iter().any(|d| d == dir) {
        out.push(dir.to_path_buf());
    }
    subdirs.sort();
    for sub in subdirs {
        collect_lib_dirs(&sub, depth + 1, out);
    }
}

fn llama_lib_dirs(exe: &Path) -> Vec<PathBuf> {
    let exe_dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut dirs = vec![exe_dir.clone()];
    collect_lib_dirs(&exe_dir, 0, &mut dirs);
    if let Some(parent) = exe_dir.parent() {
        let vendor = parent.join("vendor");
        if vendor.is_dir() {
            let is_cuda12 = exe_dir.file_name().map_or(false, |n| n.to_string_lossy().contains("cuda12"));
            let mut vs: Vec<PathBuf> = std::fs::read_dir(&vendor)
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
            .unwrap_or_default();
            vs.sort();
            for v in vs {
                let name = v.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if is_cuda12 && !name.contains("cuda12") {
                    continue;
                }
                if !dirs.contains(&v) {
                    dirs.push(v);
                }
            }
        }
    }
    dirs
}

fn ld_library_path(dirs: &[PathBuf], existing: Option<&str>) -> String {
    let mut parts: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
    if let Some(e) = existing.filter(|e| !e.is_empty()) {
        parts.push(e.to_string());
    }
    parts.join(":")
}

fn server_args(cfg: &QuestConfig, ngl: Option<u32>) -> Vec<String> {
    let mut a: Vec<String> = vec!["-m".into(), cfg.model_path.clone(), "-c".into(), cfg.ctx.to_string()];
    if let Some(n) = ngl {
        a.push("-ngl".into());
        a.push(n.to_string());
    }
    a.extend([
        "--host".into(),
             "127.0.0.1".into(),
             "--port".into(),
             cfg.port.to_string(),
    ]);
    if cfg.threads > 0 {
        a.push("-t".into());
        a.push(cfg.threads.to_string());
    }
    if cfg.disable_thinking {
        a.push("--reasoning-budget".into());
        a.push("0".into());
    }
    a
}

fn log_tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(12)..].join("\n")
}

fn spawn_server(cfg: &QuestConfig, ngl: Option<u32>) -> Result<(Child, PathBuf), String> {
    let bin = Path::new(&cfg.server_bin);
    if !bin.exists() {
        return Err(format!("llama-server not found: {}", cfg.server_bin));
    }
    if !Path::new(&cfg.model_path).exists() {
        return Err(format!("model not found: {}", cfg.model_path));
    }
    let exe_dir = bin.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let ld = ld_library_path(&llama_lib_dirs(bin), std::env::var("LD_LIBRARY_PATH").ok().as_deref());
    let log_path = std::env::temp_dir().join("dfu_walk_llama_server.log");
    let log = File::create(&log_path).map_err(|e| format!("server log: {e}"))?;
    let args = server_args(cfg, ngl);
    let spawn = |with_nice: bool| -> std::io::Result<Child> {
        let mut cmd = if with_nice {
            let mut c = Command::new("nice");
            c.arg("-n").arg(cfg.nice.to_string()).arg(bin);
            c
        } else {
            Command::new(bin)
        };
        cmd.args(&args)
        .env("LD_LIBRARY_PATH", &ld)
        .current_dir(&exe_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log.try_clone()?));
        cmd.spawn()
    };
    let child = if cfg.nice > 0 {
        match spawn(true) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => spawn(false),
            other => other,
        }
    } else {
        spawn(false)
    }
    .map_err(|e| format!("starting llama-server: {e}"))?;
    Ok((child, log_path))
}

type SharedServer = Arc<Mutex<Option<Child>>>;

fn stop_server(server: &SharedServer) {
    if let Some(mut child) = server.lock().ok().and_then(|mut g| g.take()) {
        info!("Quest: stopping llama-server");
        let _ = child.kill();
        let _ = child.wait();
    }
}

static WORKING_NGL: AtomicI64 = AtomicI64::new(-1);

fn looks_like_oom(log: &str) -> bool {
    let l = log.to_lowercase();
    ["out of memory", "failed to allocate", "cudamalloc failed", "unable to allocate", "cuda error: out of memory"]
    .iter()
    .any(|p| l.contains(p))
}

fn ngl_candidates(configured: Option<u32>, working: Option<u32>) -> Vec<Option<u32>> {
    let mut chain: Vec<Option<u32>> = Vec::new();
    match configured {
        None => {
            chain.push(None);
            chain.extend([32, 16, 8, 0].map(Some));
        }
        Some(n) => {
            chain.push(Some(n));
            let mut k = n / 2;
            while k > 0 {
                chain.push(Some(k));
                k /= 2;
            }
            if n > 0 {
                chain.push(Some(0));
            }
        }
    }
    if let Some(w) = working {
        chain.retain(|c| c.map_or(false, |n| n < w));
        chain.insert(0, Some(w));
    }
    chain
}

enum StartError {
    OutOfMemory(String),
    Other(String),
}

fn start_and_wait(cfg: &QuestConfig, server: &SharedServer, ngl: Option<u32>) -> Result<(), StartError> {
    let (child, log_path) = spawn_server(cfg, ngl).map_err(StartError::Other)?;
    *server.lock().map_err(|_| StartError::Other("poisoned mutex".into()))? = Some(child);
    let deadline = Instant::now() + Duration::from_secs(cfg.startup_timeout_secs.max(1));
    loop {
        {
            let mut guard = server.lock().map_err(|_| StartError::Other("poisoned mutex".into()))?;
            if let Some(child) = guard.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    guard.take();
                    let full_log = std::fs::read_to_string(&log_path).unwrap_or_default();
                    let msg = format!("llama-server terminated ({status}). Log:\n{}", log_tail(&log_path));
                    return Err(if looks_like_oom(&full_log) { StartError::OutOfMemory(msg) } else { StartError::Other(msg) });
                }
            }
        }
        if server_health(cfg.port) {
            return Ok(());
        }
        if Instant::now() > deadline {
            stop_server(server);
            return Err(StartError::Other(format!("server did not start within {}s", cfg.startup_timeout_secs)));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn ensure_server(cfg: &QuestConfig, server: &SharedServer) -> Result<(), String> {
    if server_health(cfg.port) {
        return Ok(());
    }
    stop_server(server);
    let working = u32::try_from(WORKING_NGL.load(Ordering::Relaxed)).ok();
    let mut last_error = String::new();
    for ngl in ngl_candidates(cfg.ngl, working) {
        let label = ngl.map_or("auto".to_string(), |n| n.to_string());
        info!("Quest: starting llama-server (ngl {label}) with {}", cfg.model_path);
        match start_and_wait(cfg, server, ngl) {
            Ok(()) => {
                info!("Quest: llama-server ready (ngl {label})");
                if let Some(n) = ngl.filter(|_| ngl != cfg.ngl) {
                    WORKING_NGL.store(i64::from(n), Ordering::Relaxed);
                    warn!("Quest: server starts with ngl={n}; to skip retries set ngl={n} in {CONFIG_FILE}");
                }
                return Ok(());
            }
            Err(StartError::OutOfMemory(msg)) => {
                warn!("Quest: out of memory with ngl={label}, retrying with fewer layers");
                last_error = msg;
            }
            Err(StartError::Other(msg)) => return Err(msg),
        }
    }
    Err(format!("out of memory even with fewer GPU layers. Last error: {last_error}"))
}

// =============================================================================
// Generazione
// =============================================================================

#[derive(Clone)]
struct Templates {
    seed: String,
    rumors: String,
    npc_flavor: String,
    voice: String,
    ask: String,
}

struct Job {
    city: (i32, i32),
    city_name: String,
    theme: &'static str,
    location: DFLocation,
    blocks_buf: Arc<Vec<u8>>,
    arena_dir: PathBuf,
}

/// Up to 3 attempts: on each rejection, repeat the prompt with the reason.
fn try_json(cfg: &QuestConfig, prompt: &str, label: &str, validate: impl Fn(&Json) -> Result<(), String>) -> Result<Json, String> {
    let mut last_err = String::new();
    for attempt in 1..=3 {
        let p = if attempt > 1 {
            format!("{prompt}\nWARNING: the previous output was rejected ({last_err}). Reply ONLY with the JSON object.")
        } else {
            prompt.to_string()
        };
        info!("Quest [{label}] attempt {attempt}/3");
        let raw = match call_llm(cfg, &p) {
            Ok(r) => r,
            Err(e) => {
                warn!("Quest [{label}] LLM error: {e}");
                last_err = e;
                continue;
            }
        };
        let parsed = match Json::parse(&extract_json(&raw)) {
            Ok(v) => v,
            Err(e) => {
                last_err = format!("invalid JSON: {e}");
                warn!("Quest [{label}] {last_err}");
                continue;
            }
        };
        if let Err(msg) = validate(&parsed) {
            warn!("Quest [{label}] rejected: {msg}");
            last_err = msg;
            continue;
        }
        return Ok(parsed);
    }
    Err(format!("{label}: failed after 3 attempts ({last_err})"))
}

fn generate_quest(cfg: &QuestConfig, tpl: &Templates, job: &Job, npc_flats: &[(u16, u8, String)]) -> Result<Quest, String> {
    let seed_json = try_json(cfg, &build_seed_prompt(&tpl.seed, job.theme), "SEED", validate_seed)?;
    let seed = enrich_seed(&seed_json)?;
    info!(
        "Quest [SEED] '{}' | location: {} | artifact: {} | quest-giver: {}",
        seed.title, seed.tomb_name, seed.artifact_name, seed.quest_giver_name
    );
    let rumors_json = try_json(cfg, &build_rumors_prompt(&tpl.rumors, &seed), "RUMORS", |ev| validate_rumors(ev, &seed))?;
    let rumors = rumors_from_json(&rumors_json)?;
    let npc_flavors = generate_npc_flavors(cfg, tpl, &seed, npc_flats);
    Ok(Quest {
        city: job.city,
       city_name: job.city_name.clone(),
       seed,
       rumors,
       npc_flavors,
    })
}

fn append_quest(path: &Path, quest: &Quest) -> std::io::Result<()> {
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", quest.to_json().compact())
}

// =============================================================================
// Thread di lavoro
// =============================================================================

enum QuestMsg {
    Done(Quest),
    Failed { city: (i32, i32), reason: String },
    /// Voce di un PNG generata su richiesta.
    Voice { city: (i32, i32), key: String, text: String },
    VoiceFailed { city: (i32, i32), key: String },
    /// Risposta a una domanda libera del giocatore.
    Answer { id: u64, text: String },
    AnswerFailed { id: u64 },
}

/// Ogni quanto il thread controlla le richieste di voce mentre aspetta una quest.
const WORKER_POLL: Duration = Duration::from_millis(100);

/// Una voce su richiesta: avvia il server se serve, genera, salva e risponde.
fn run_voice(cfg: &QuestConfig, tpl: &Templates, server: &SharedServer, voices_path: &Path, v: &VoiceJob) -> QuestMsg {
    let started = Instant::now();
    let outcome = ensure_server(cfg, server)
        .and_then(|_| generate_voice(cfg, tpl, &v.seed, &v.description))
        .and_then(|text| {
            append_voice(voices_path, v.city, &v.key, &v.description, &text)
                .map_err(|e| format!("writing {}: {e}", voices_path.display()))?;
            Ok(text)
        });
    match outcome {
        Ok(text) => {
            info!("Quest: voice '{}' ready in {:?}", v.key, started.elapsed());
            QuestMsg::Voice { city: v.city, key: v.key.clone(), text }
        }
        Err(reason) => {
            warn!("Quest: voice '{}': {reason}", v.key);
            QuestMsg::VoiceFailed { city: v.city, key: v.key.clone() }
        }
    }
}

/// Solo per i test: un thread di lavoro senza richieste di voce.
#[cfg(test)]
fn worker_main(
    cfg: QuestConfig,
    tpl: Templates,
    quests_path: PathBuf,
    jobs: Receiver<Job>,
    results: Sender<QuestMsg>,
    server: SharedServer,
) {
    let (_voice_tx, voice_rx) = mpsc::channel();
    let (_ask_tx, ask_rx) = mpsc::channel();
    worker_loop(cfg, tpl, quests_path, PathBuf::from("quest_npc_voices.test.txt"), jobs, voice_rx, ask_rx, results, server);
}

/// Il thread di lavoro: genera le quest (prioritarie) e le voci su richiesta, una cosa per volta.
#[allow(clippy::too_many_arguments)]
fn worker_loop(
    cfg: QuestConfig,
    tpl: Templates,
    quests_path: PathBuf,
    voices_path: PathBuf,
    jobs: Receiver<Job>,
    voice_jobs: Receiver<VoiceJob>,
    ask_jobs: Receiver<AskJob>,
    results: Sender<QuestMsg>,
    server: SharedServer,
) {
    let idle = Duration::from_secs(cfg.idle_stop_secs.max(1));
    let mut catalog_cache: Option<FlatCatalog> = None;
    let mut last_work = Instant::now();
    loop {
        match jobs.recv_timeout(WORKER_POLL) {
            Ok(job) => {
                let started = Instant::now();
                let npc_flats = {
                    let catalog = catalog_cache.get_or_insert_with(|| FlatCatalog::load(&job.arena_dir));
                    collect_npc_flats(&job.location, &job.blocks_buf, catalog)
                };
                let outcome = ensure_server(&cfg, &server)
                .and_then(|_| generate_quest(&cfg, &tpl, &job, &npc_flats))
                .and_then(|quest| {
                    append_quest(&quests_path, &quest).map_err(|e| format!("writing {}: {e}", quests_path.display()))?;
                    Ok(quest)
                });
                let msg = match outcome {
                    Ok(quest) => {
                        info!(
                            "Quest for {} generated in {:?} ({} NPC flavors)",
                              job.city_name,
                              started.elapsed(),
                              quest.npc_flavors.len()
                        );
                        QuestMsg::Done(quest)
                    }
                    Err(reason) => {
                        warn!("Quest for {}: {reason}", job.city_name);
                        QuestMsg::Failed { city: job.city, reason }
                    }
                };
                if results.send(msg).is_err() {
                    break;
                }
                last_work = Instant::now();
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        // le domande del giocatore (interattive) hanno la precedenza sulle voci; una cosa per giro,
        // e se nel frattempo arriva una quest, ha la precedenza su tutto
        if let Ok(a) = ask_jobs.try_recv() {
            let msg = run_ask(&cfg, &tpl, &server, &a);
            if results.send(msg).is_err() {
                break;
            }
            last_work = Instant::now();
        } else if let Ok(v) = voice_jobs.try_recv() {
            let msg = run_voice(&cfg, &tpl, &server, &voices_path, &v);
            if results.send(msg).is_err() {
                break;
            }
            last_work = Instant::now();
        }
        // nessun lavoro da un po': si libera la VRAM
        if last_work.elapsed() >= idle {
            stop_server(&server);
            last_work = Instant::now();
        }
    }
    stop_server(&server);
}

// =============================================================================
// Archivio delle quest
// =============================================================================

#[derive(Resource, Default)]
pub struct QuestBook {
    quests: HashMap<(i32, i32), Quest>,
    pending: HashSet<(i32, i32)>,
    failed: HashSet<(i32, i32)>,
    /// Voci generate su richiesta: (citta', chiave del PNG) -> testo.
    voices: HashMap<((i32, i32), String), String>,
    voices_pending: HashSet<((i32, i32), String)>,
    voices_failed: HashSet<((i32, i32), String)>,
    /// Risposte alle domande libere, in attesa che il dialogo le ritiri.
    answers: HashMap<u64, Result<String, ()>>,
}

/// Stato della voce su richiesta di un PNG.
#[derive(Debug, PartialEq)]
pub enum VoiceState<'a> {
    Ready(&'a str),
    /// Il thread la sta generando.
    Pending,
    /// La generazione e' fallita in questa sessione.
    Failed,
    /// Mai richiesta.
    Unknown,
}

/// What the player can hear from an NPC.
#[derive(Debug, PartialEq)]
pub enum Hint<'a> {
    Text(&'a str),
    Pending,
    Nothing,
}

impl QuestBook {
    /// Reads `quests.txt`: one JSON quest per line; invalid lines are ignored.
    pub fn load(path: &Path) -> Self {
        let mut book = Self::default();
        let Ok(text) = std::fs::read_to_string(path) else { return book };
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            match Json::parse(line).ok().as_ref().and_then(Quest::from_json) {
                Some(q) => {
                    book.quests.insert(q.city, q);
                }
                None => warn!("Quest: line {} of {} invalid, ignored", n + 1, path.display()),
            }
        }
        book
    }

    pub fn len(&self) -> usize {
        self.quests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.quests.is_empty()
    }

    pub fn quest(&self, city: (i32, i32)) -> Option<&Quest> {
        self.quests.get(&city)
    }

    fn knows(&self, city: (i32, i32)) -> bool {
        self.quests.contains_key(&city) || self.pending.contains(&city) || self.failed.contains(&city)
    }

    /// The rumor an NPC of the given tone tells in the given city.
    pub fn hint(&self, city: Option<(i32, i32)>, tone: Tone) -> Hint<'_> {
        let Some(city) = city else { return Hint::Nothing };
        if let Some(q) = self.quests.get(&city) {
            return q.rumor(tone).map_or(Hint::Nothing, Hint::Text);
        }
        if self.pending.contains(&city) {
            Hint::Pending
        } else {
            Hint::Nothing
        }
    }

    /// Legge `quest_npc_voices.txt`: una voce JSON per riga; le righe non valide si ignorano.
    pub fn load_voices(&mut self, path: &Path) {
        let Ok(text) = std::fs::read_to_string(path) else { return };
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let Ok(j) = Json::parse(line) else { continue };
            let (Some(x), Some(y), Some(key), Some(text)) = (
                j.get("city_x").and_then(Json::as_int),
                j.get("city_y").and_then(Json::as_int),
                j.get("key").and_then(Json::as_str),
                j.get("text").and_then(Json::as_str),
            ) else {
                continue;
            };
            self.voices.insert(((x as i32, y as i32), key.to_string()), text.to_string());
        }
    }

    pub fn voice_state(&self, city: (i32, i32), key: &str) -> VoiceState<'_> {
        let k = (city, key.to_string());
        if let Some(text) = self.voices.get(&k) {
            VoiceState::Ready(text)
        } else if self.voices_pending.contains(&k) {
            VoiceState::Pending
        } else if self.voices_failed.contains(&k) {
            VoiceState::Failed
        } else {
            VoiceState::Unknown
        }
    }

    /// Registra una voce arrivata dal thread (e toglie la richiesta in corso).
    pub fn store_voice(&mut self, city: (i32, i32), key: String, text: String) {
        self.voices_pending.remove(&(city, key.clone()));
        self.voices.insert((city, key), text);
    }

    pub fn fail_voice(&mut self, city: (i32, i32), key: String) {
        self.voices_pending.remove(&(city, key.clone()));
        self.voices_failed.insert((city, key));
    }

    pub fn store_answer(&mut self, id: u64, text: String) {
        self.answers.insert(id, Ok(text));
    }

    pub fn fail_answer(&mut self, id: u64) {
        self.answers.insert(id, Err(()));
    }

    /// La risposta alla domanda `id`, se e' arrivata (viene consumata).
    pub fn take_answer(&mut self, id: u64) -> Option<Result<String, ()>> {
        self.answers.remove(&id)
    }

    #[cfg(test)]
    pub(crate) fn mark_voice_pending(&mut self, city: (i32, i32), key: &str) {
        self.voices_pending.insert((city, key.to_string()));
    }

    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    /// NPC-specific flavor rumor, looked up by flat (archive, record).
    pub fn npc_flavor(&self, city: (i32, i32), archive: u16, record: u8) -> Option<&str> {
        let q = self.quests.get(&city)?;
        q.npc_flavors
        .iter()
        .find(|nf| nf.archive == archive && nf.record == record)
        .map(|nf| nf.text.as_str())
    }
}

// =============================================================================
// Chi e' il PNG: che voce racconta
// =============================================================================

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// NPC type (and therefore rumor) stable for the same NPC: depends on their face
/// (`key`), the city and the place. In shops they are merchants, in taverns mostly
/// drunks and commoners, outdoors mostly commoners, merchants and soldiers.
pub fn tone_for_npc(key: u64, city: (i32, i32), place: Option<PoiKind>) -> Tone {
    let h = splitmix(key ^ ((city.0 as u64) << 20) ^ (city.1 as u64).wrapping_mul(0x1_0001));
    let r = h % 100;
    match place {
        Some(PoiKind::Shop) => Tone::Merchant,
        Some(PoiKind::Tavern) => match r {
            0..=44 => Tone::Drunk,
            45..=74 => Tone::Commoner,
            75..=84 => Tone::Soldier,
            85..=94 => Tone::Merchant,
            _ => Tone::Noble,
        },
        _ => match r {
            0..=39 => Tone::Commoner,
            40..=59 => Tone::Merchant,
            60..=79 => Tone::Soldier,
            80..=94 => Tone::Noble,
            _ => Tone::Drunk,
        },
    }
}

/// City, village or town: only RMB blocks and at least `MIN_TOWN_BLOCKS` blocks.
pub fn is_town(loc: &DFLocation) -> bool {
    loc.block_names.len() >= MIN_TOWN_BLOCKS
    && loc.block_names.iter().all(|n| n.to_ascii_uppercase().ends_with(".RMB"))
}

fn theme_for(city: (i32, i32)) -> &'static str {
    THEMES[(splitmix(((city.0 as u64) << 32) ^ (city.1 as u64 & 0xFFFF_FFFF)) % THEMES.len() as u64) as usize]
}

// =============================================================================
// Plugin Bevy
// =============================================================================

#[derive(Resource)]
pub struct QuestWorker {
    jobs: Sender<Job>,
    voice_jobs: Sender<VoiceJob>,
    ask_jobs: Sender<AskJob>,
    next_ask: AtomicU64,
    results: Mutex<Receiver<QuestMsg>>,
    server: SharedServer,
}

impl QuestWorker {
    /// Manda al LLM la domanda del giocatore: il PNG risponde in carattere, sapendo solo il nome
    /// della citta'. Restituisce l'id con cui ritirare la risposta (`QuestBook::take_answer`), o
    /// None se la domanda e' vuota o il thread di lavoro non c'e' piu'.
    pub fn ask(&self, city_name: &str, description: &str, question: &str) -> Option<u64> {
        let question = clean_question(question)?;
        let id = self.next_ask.fetch_add(1, Ordering::Relaxed);
        let job = AskJob { id, city_name: city_name.to_string(), description: description.to_string(), question };
        self.ask_jobs.send(job).is_ok().then_some(id)
    }

    /// Chiede al thread la voce di un PNG. Serve la quest della citta' (il seed) e la voce non
    /// deve essere gia' pronta, in corso o fallita. Restituisce true se la richiesta e' partita.
    pub fn request_voice(&self, book: &mut QuestBook, city: (i32, i32), key: &str, description: &str) -> bool {
        let Some(seed) = book.quests.get(&city).map(|q| q.seed.clone()) else { return false };
        if !matches!(book.voice_state(city, key), VoiceState::Unknown) {
            return false;
        }
        let job = VoiceJob { city, key: key.to_string(), description: description.to_string(), seed };
        if self.voice_jobs.send(job).is_ok() {
            book.voices_pending.insert((city, key.to_string()));
            true
        } else {
            false
        }
    }
}

impl Drop for QuestWorker {
    fn drop(&mut self) {
        stop_server(&self.server);
    }
}

fn read_prompt(file: &str, default: &str) -> String {
    match std::fs::read_to_string(file) {
        Ok(t) if !t.trim().is_empty() => {
            info!("Quest: using prompt from {file}");
            t
        }
        _ => default.to_string(),
    }
}

pub struct QuestPlugin;

impl Plugin for QuestPlugin {
    fn build(&self, app: &mut App) {
        let cfg = QuestConfig::load_or_create(Path::new(CONFIG_FILE));
        let mut book = QuestBook::load(Path::new(QUESTS_FILE));
        book.load_voices(Path::new(VOICES_FILE));
        info!(
            "Quest: {} already saved in {QUESTS_FILE} (+{} NPC voices), generator {}",
            book.len(),
            book.voice_count(),
              if cfg.enabled { "active" } else { "disabled" }
        );
        app.insert_resource(book);
        if !cfg.enabled {
            return;
        }
        let tpl = Templates {
            seed: read_prompt(SEED_PROMPT_FILE, DEFAULT_SEED_PROMPT),
            rumors: read_prompt(RUMORS_PROMPT_FILE, DEFAULT_RUMORS_PROMPT),
            npc_flavor: read_prompt(NPC_FLAVOR_PROMPT_FILE, DEFAULT_NPC_FLAVOR_PROMPT),
            voice: read_prompt(NPC_VOICE_PROMPT_FILE, DEFAULT_NPC_VOICE_PROMPT),
            ask: read_prompt(NPC_ASK_PROMPT_FILE, DEFAULT_NPC_ASK_PROMPT),
        };
        let (job_tx, job_rx) = mpsc::channel();
        let (voice_tx, voice_rx) = mpsc::channel();
        let (ask_tx, ask_rx) = mpsc::channel();
        let (res_tx, res_rx) = mpsc::channel();
        let server: SharedServer = Arc::new(Mutex::new(None));
        let thread_server = Arc::clone(&server);
        let spawned = std::thread::Builder::new()
        .name("quest-llm".into())
        .spawn(move || {
            worker_loop(
                cfg,
                tpl,
                PathBuf::from(QUESTS_FILE),
                PathBuf::from(VOICES_FILE),
                job_rx,
                voice_rx,
                ask_rx,
                res_tx,
                thread_server,
            )
        });
        if let Err(e) = spawned {
            warn!("Quest: cannot start generator thread: {e}");
            return;
        }
        app.insert_resource(QuestWorker {
            jobs: job_tx,
            voice_jobs: voice_tx,
            ask_jobs: ask_tx,
            next_ask: AtomicU64::new(1),
            results: Mutex::new(res_rx),
            server,
        })
        .add_systems(Update, (quest_trigger, quest_poll));
    }
}

/// When the player enters a city without a quest, request one from the worker thread.
fn quest_trigger(
    data: Res<GameData>,
    origin: Res<Origin>,
    player: Query<&Transform, With<Player>>,
    worker: Res<QuestWorker>,
    mut book: ResMut<QuestBook>,
    mut last: Local<Option<(i32, i32)>>,
) {
    let Ok(tf) = player.get_single() else { return };
    let here = map_pixel_of(&origin, tf.translation);
    if *last == Some(here) {
        return;
    }
    *last = Some(here);
    let Some(loc) = data.locations.get(&here) else { return };
    if !is_town(loc) || book.knows(here) {
        return;
    }
    let job = Job {
        city: here,
        city_name: loc.name.clone(),
        theme: theme_for(here),
        location: loc.clone(),
        blocks_buf: Arc::clone(&data.blocks_buf),
        arena_dir: data.dir.clone(),
    };
    if worker.jobs.send(job).is_ok() {
        info!("Quest: generating quest for {} (map pixel {}, {})", loc.name, here.0, here.1);
        book.pending.insert(here);
    }
}

/// Collects thread results without ever blocking.
fn quest_poll(worker: Res<QuestWorker>, mut book: ResMut<QuestBook>) {
    let Ok(results) = worker.results.try_lock() else { return };
    while let Ok(msg) = results.try_recv() {
        match msg {
            QuestMsg::Done(quest) => {
                book.pending.remove(&quest.city);
                book.quests.insert(quest.city, quest);
            }
            QuestMsg::Failed { city, .. } => {
                book.pending.remove(&city);
                book.failed.insert(city);
            }
            QuestMsg::Voice { city, key, text } => book.store_voice(city, key, text),
            QuestMsg::VoiceFailed { city, key } => book.fail_voice(city, key),
            QuestMsg::Answer { id, text } => book.store_answer(id, text),
            QuestMsg::AnswerFailed { id } => book.fail_answer(id),
        }
    }
}

// =============================================================================
// Test
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ---------------------------------------------------------------- JSON

    #[test]
    fn json_round_trip_with_escapes_and_unicode() {
        let src = r#"{"a":"line\nnew \"quoted\" \\ \u00e8 \ud83d\ude00","n":[1,2.5,-3,true,false,null],"o":{"x":{}} }"#;
        let v = Json::parse(src).unwrap();
        assert_eq!(v.get("a").and_then(Json::as_str), Some("line\nnew \"quoted\" \\ è 😀"));
        assert_eq!(v.get("n").and_then(Json::as_array).map(|a| a.len()), Some(6));
        assert_eq!(Json::parse(&v.compact()).unwrap(), v);
        assert_eq!(Json::parse(&v.pretty()).unwrap(), v);
    }

    #[test]
    fn json_rejects_garbage_and_deep_nesting() {
        assert!(Json::parse("").is_err());
        assert!(Json::parse("{").is_err());
        assert!(Json::parse(r#"{"a":1,}"#).is_err());
        assert!(Json::parse(r#"{"a":1} extra"#).is_err());
        assert!(Json::parse(r#""\x""#).is_err());
        let deep = format!("{}1{}", "[".repeat(500), "]".repeat(500));
        assert!(Json::parse(&deep).is_err());
        assert_eq!(Json::parse("\"a\nb\"").unwrap(), Json::Str("a\nb".into()));
    }

    #[test]
    fn json_pretty_matches_python_indent_2() {
        let v = Json::Obj(vec![
            ("a".into(), Json::Num(3.0)),
                          ("b".into(), Json::Arr(vec![])),
                          ("c".into(), Json::Obj(vec![("x".into(), jstr("è"))])),
        ]);
        assert_eq!(v.pretty(), "{\n  \"a\": 3,\n  \"b\": [],\n  \"c\": {\n    \"x\": \"è\"\n  }\n}");
        assert_eq!(v.compact(), "{\"a\":3,\"b\":[],\"c\":{\"x\":\"è\"}}");
    }

    #[test]
    fn as_int_reads_numbers_and_numeric_strings() {
        assert_eq!(Json::Num(4.0).as_int(), Some(4));
        assert_eq!(Json::Str(" 3 ".into()).as_int(), Some(3));
        assert_eq!(Json::Str("three".into()).as_int(), None);
        assert_eq!(Json::Bool(true).as_int(), None);
    }

    // -------------------------------------------------------- configurazione

    #[test]
    fn config_defaults_match_the_requested_paths() {
        let c = QuestConfig::default();
        assert!(c.model_path.ends_with("Qwen3.8-27B-Heretic-Abliterated-Uncensored-GGUF/RVN-Q3_K_S.gguf"));
        assert!(c.model_path.starts_with("/mnt/1CE16DBE48849731/Bionic-1.1.8-10-linux-x64.AppImage.home/models/"));
        assert!(c.server_bin.ends_with("llama.cpp-linux-x86_64-nvidia-cuda12-avx2-2.55.0/llama-server"));
        assert_eq!((c.port, c.ctx, c.ngl), (8080, 8192, None));
    }

    #[test]
    fn config_round_trips_and_ignores_bad_lines() {
        let mut c = QuestConfig::default();
        c.port = 9000;
        c.ngl = Some(20);
        c.enabled = false;
        c.model_path = "/tmp/a b/model.gguf".into();
        assert_eq!(QuestConfig::parse(&c.to_text()), c);
        let p = QuestConfig::parse("# comment\nngl = 7\nport=not_a_number\nunknown=1\nline without equals\nenabled=no\n");
        assert_eq!(p.ngl, Some(7));
        assert_eq!(p.port, 8080);
        assert_eq!(QuestConfig::parse("ngl=auto").ngl, None);
        assert_eq!(QuestConfig::parse("ngl=AUTO").ngl, None);
        assert_eq!(QuestConfig::parse("ngl=0").ngl, Some(0));
        assert_eq!(QuestConfig::parse("ngl=blah").ngl, None);
        assert!(QuestConfig::default().to_text().contains("\nngl=auto\n"));
        assert!(!p.enabled);
    }

    #[test]
    fn config_file_is_created_when_missing() {
        let dir = std::env::temp_dir().join(format!("quest_cfg_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("quest_llm.conf");
        let _ = std::fs::remove_file(&path);
        let c = QuestConfig::load_or_create(&path);
        assert_eq!(c, QuestConfig::default());
        assert!(path.exists());
        assert_eq!(QuestConfig::load_or_create(&path), c);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------- risposta del modello

    #[test]
    fn extract_json_handles_fences_and_thinking() {
        let raw = "<think>\nreasoning {with braces}\n</think>\n```json\n{\"a\": 1}\n```";
        assert_eq!(extract_json(raw), "{\"a\": 1}");
        assert_eq!(extract_json("Here: {\"a\": {\"b\": 2}} end"), "{\"a\": {\"b\": 2}}");
        assert_eq!(extract_json("<THINK foo>x</THINK >{\"k\":1}"), "{\"k\":1}");
        assert_eq!(extract_json("<|thinking|>x<|/thinking|>{\"k\":1}"), "{\"k\":1}");
        assert_eq!(extract_json("no json"), "no json");
        assert_eq!(extract_json("<think>{\"k\":1}"), "{\"k\":1}");
    }

    // ------------------------------------------------------------ validatori

    fn good_seed_json() -> Json {
        Json::parse(
            r#"{"theme":"artifact_discovery","title":"The Whisper of Vael","dungeon_id":"dgf_crypt_ash",
            "quest_giver_id":"mages_guild","tomb_name":"Nekhar-Zul","artifact_name":"Eye of Vael",
            "difficulty":3,"lore":"An ancient glass eye sleeps beneath the crypt."}"#,
        )
        .unwrap()
    }

    #[test]
    fn seed_validation_follows_the_script() {
        assert!(validate_seed(&good_seed_json()).is_ok());
        let tweak = |key: &str, value: Json| {
            let Json::Obj(fields) = good_seed_json() else { unreachable!() };
            Json::Obj(fields.into_iter().map(|(k, v)| if k == key { (k, value.clone()) } else { (k, v) }).collect())
        };
        assert!(validate_seed(&tweak("theme", jstr("pirates"))).unwrap_err().contains("theme"));
        assert!(validate_seed(&tweak("dungeon_id", jstr("invented"))).unwrap_err().contains("catalog"));
        assert!(validate_seed(&tweak("quest_giver_id", jstr("king"))).unwrap_err().contains("factions"));
        assert!(validate_seed(&tweak("difficulty", Json::Num(9.0))).unwrap_err().contains("out of range"));
        assert!(validate_seed(&tweak("difficulty", jstr("high"))).unwrap_err().contains("integer"));
        assert!(validate_seed(&tweak("tomb_name", jstr("  "))).unwrap_err().contains("tomb_name"));
        assert!(validate_seed(&Json::Arr(vec![])).is_err());
        assert!(validate_seed(&tweak("difficulty", jstr("4"))).is_ok());
    }

    fn seed() -> Seed {
        enrich_seed(&good_seed_json()).unwrap()
    }

    fn rumors_json(texts: [&str; 5]) -> Json {
        let items: Vec<Json> = Tone::ALL
        .iter()
        .zip(texts)
        .map(|(t, text)| Json::Obj(vec![("tone".into(), jstr(t.key())), ("text".into(), jstr(text))]))
        .collect();
        Json::Obj(vec![("rumors".into(), Json::Arr(items))])
    }

    const OK_TEXTS: [&str; 5] = [
        "Your Grace, it is said the Mages Guild watches over Nekhar-Zul and the Eye of Vael.",
        "The roads to Nekhar-Zul are a couple of hours out, and prices are rising.",
        "I saw the mages leave for those ruins. They seek the Eye of Vael.",
        "They say there's something pulsing in the crypt outside town. It scares me.",
        "Dunno, Nekhar... Nekhal? Ten or twenty kilometers, let's drink!",
    ];

    #[test]
    fn enrich_seed_adds_catalog_data() {
        let s = seed();
        assert_eq!(s.dungeon_name, "Cripta di Cenere");
        assert_eq!(s.dungeon_name_dev, "Crypt of Ash");
        assert_eq!((s.dungeon_region.as_str(), s.dungeon_distance_km), ("Daggerfall", 12));
        assert_eq!(s.quest_giver_name, "Mages Guild");
        let prompt = build_rumors_prompt(DEFAULT_RUMORS_PROMPT, &s);
        assert!(prompt.contains("Nekhar-Zul") && prompt.contains("\"difficulty\": 3"));
        assert!(!prompt.contains("Cripta di Cenere") && !prompt.contains("Crypt of Ash") && !prompt.contains("dgf_crypt_ash"));
    }

    #[test]
    fn rumors_validation_follows_the_script() {
        let s = seed();
        assert!(validate_rumors(&rumors_json(OK_TEXTS), &s).is_ok());
        assert!(rumors_from_json(&rumors_json(OK_TEXTS)).unwrap().len() == 5);
        let mut t = OK_TEXTS;
        t[1] = "The  CRYPT   of Ash is nearby.";
        assert!(validate_rumors(&rumors_json(t), &s).unwrap_err().contains("generic name"));
        let mut t = OK_TEXTS;
        t[1] = "They already recovered the Eye of Vael and merchants are making deals.";
        assert!(validate_rumors(&rumors_json(t), &s).unwrap_err().contains("recovered"));
        let mut t = OK_TEXTS;
        t[2] = "They found the road closed.";
        assert!(validate_rumors(&rumors_json(t), &s).is_ok());
        let mut t = OK_TEXTS;
        t[0] = "  ";
        assert!(validate_rumors(&rumors_json(t), &s).unwrap_err().contains("empty"));
        let long = "x".repeat(501);
        let mut t = OK_TEXTS;
        t[3] = &long;
        assert!(validate_rumors(&rumors_json(t), &s).unwrap_err().contains("too long"));
        assert!(validate_rumors(&Json::parse(r#"{"rumors":[]}"#).unwrap(), &s).unwrap_err().contains("expected 5"));
        assert!(validate_rumors(&Json::parse("{}").unwrap(), &s).is_err());
        let dup = Json::parse(
            r#"{"rumors":[{"tone":"noble","text":"a"},{"tone":"noble","text":"b"},{"tone":"soldier","text":"c"},{"tone":"drunk","text":"d"},{"tone":"merchant","text":"e"}]}"#,
        )
        .unwrap();
        assert!(validate_rumors(&dup, &s).unwrap_err().contains("duplicate"));
        let bad = Json::parse(
            r#"{"rumors":[{"tone":"king","text":"a"},{"tone":"noble","text":"b"},{"tone":"soldier","text":"c"},{"tone":"drunk","text":"d"},{"tone":"merchant","text":"e"}]}"#,
        )
        .unwrap();
        assert!(validate_rumors(&bad, &s).unwrap_err().contains("invalid"));
    }

    #[test]
    fn seed_prompt_lists_catalog_and_theme() {
        let p = build_seed_prompt(DEFAULT_SEED_PROMPT, "rescue");
        assert!(p.contains("Required theme: rescue"));
        assert!(p.contains("- id=dgf_crypt_ash | Cripta di Cenere | region=Daggerfall | 12km"));
        assert!(p.contains("- id=knights_dragon | Knights of the Dragon | type=order | based in Daggerfall"));
        assert!(!p.contains("{theme}") && !p.contains("{dungeon_list}") && !p.contains("{faction_list}"));
    }

    // ------------------------------------------------------------------ HTTP

    #[test]
    fn http_responses_are_parsed() {
        let plain = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 7\r\n\r\n{\"a\":1}";
        assert_eq!(parse_http_response(plain).unwrap(), (200, "{\"a\":1}".to_string()));
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\n\r\n";
        assert_eq!(parse_http_response(chunked).unwrap(), (200, "{\"a\":1}".to_string()));
        let err = b"HTTP/1.1 503 Service Unavailable\r\n\r\nLoading model";
        assert_eq!(parse_http_response(err).unwrap(), (503, "Loading model".to_string()));
        assert!(parse_http_response(b"HTTP/1.1 200 OK\r\n").is_err());
        assert!(parse_http_response(b"garbage\r\n\r\n").is_err());
    }

    // -------------------------------------------------------- server finto

    struct FakeServer {
        port: u16,
        bodies: Arc<Mutex<Vec<String>>>,
    }

    fn fake_server(replies: Vec<(u16, String)>) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&bodies);
        let replies = Arc::new(Mutex::new(replies.into_iter().collect::<std::collections::VecDeque<_>>()));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap_or(0);
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                reader.read_exact(&mut body).ok();
                let (status, payload) = if request_line.starts_with("GET /health") {
                    (200, "{\"status\":\"ok\"}".to_string())
                } else {
                    seen.lock().unwrap().push(String::from_utf8_lossy(&body).into_owned());
                    let (st, content) = replies.lock().unwrap().pop_front().unwrap_or((500, "done".into()));
                    if st == 200 {
                        let json = Json::Obj(vec![(
                            "choices".into(),
                                                   Json::Arr(vec![Json::Obj(vec![
                                                       ("message".into(), Json::Obj(vec![("content".into(), jstr(&content))])),
                                                                            ("finish_reason".into(), jstr("stop")),
                                                   ])]),
                        )]);
                        (200, json.compact())
                    } else {
                        (st, content)
                    }
                };
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                stream.write_all(response.as_bytes()).ok();
            }
        });
        FakeServer { port, bodies }
    }

    fn test_cfg(port: u16) -> QuestConfig {
        QuestConfig { port, server_bin: "/does/not/exist/llama-server".into(), idle_stop_secs: 1, ..QuestConfig::default() }
    }

    fn tpl() -> Templates {
        Templates {
            seed: DEFAULT_SEED_PROMPT.to_string(),
            rumors: DEFAULT_RUMORS_PROMPT.to_string(),
            npc_flavor: DEFAULT_NPC_FLAVOR_PROMPT.to_string(),
            voice: DEFAULT_NPC_VOICE_PROMPT.to_string(),
            ask: DEFAULT_NPC_ASK_PROMPT.to_string(),
        }
    }

    fn empty_location() -> DFLocation {
        DFLocation {
            name: "X".into(),
            region_index: 0,
            world_x: 0,
            world_y: 0,
            width: 0,
            height: 0,
            block_names: Vec::new(),
            letter1_for_rmb_name: None,
        }
    }

    #[test]
    fn llm_call_sends_chat_body_and_reads_content() {
        let srv = fake_server(vec![(200, "hello".into())]);
        let cfg = test_cfg(srv.port);
        assert_eq!(call_llm(&cfg, "question \"with quotes\"").unwrap(), "hello");
        let sent = srv.bodies.lock().unwrap()[0].clone();
        let body = Json::parse(&sent).unwrap();
        assert_eq!(body.get("model").and_then(Json::as_str), Some("llm"));
        assert_eq!(body.get("max_tokens").and_then(Json::as_int), Some(3072));
        let content = body.get("messages").and_then(Json::as_array).unwrap()[0].get("content").and_then(Json::as_str);
        assert_eq!(content, Some("question \"with quotes\""));
        assert!(body.get("chat_template_kwargs").is_some());
    }

    #[test]
    fn llm_call_retries_without_template_kwargs_on_400() {
        let srv = fake_server(vec![(400, "no kwargs".into()), (200, "ok".into())]);
        assert_eq!(call_llm(&test_cfg(srv.port), "x").unwrap(), "ok");
        let bodies = srv.bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 2);
        assert!(bodies[0].contains("chat_template_kwargs") && !bodies[1].contains("chat_template_kwargs"));
        let srv = fake_server(vec![(500, "broken".into())]);
        assert!(call_llm(&test_cfg(srv.port), "x").unwrap_err().contains("HTTP 500"));
        assert!(call_llm(&test_cfg(1), "x").is_err());
    }

    fn seed_reply() -> String {
        format!("```json\n{}\n```", good_seed_json().compact())
    }

    fn rumors_reply() -> String {
        rumors_json(OK_TEXTS).compact()
    }

    #[test]
    fn quest_is_generated_with_retries_and_feedback() {
        let invalid_seed = r#"{"theme":"artifact_discovery","title":"x","dungeon_id":"invented","quest_giver_id":"mages_guild","tomb_name":"a","artifact_name":"b","difficulty":3,"lore":"c"}"#;
        let srv = fake_server(vec![
            (200, "<think>hmm</think> I don't know".into()),
                              (200, invalid_seed.into()),
                              (200, seed_reply()),
                              (200, rumors_reply()),
        ]);
        let job = Job {
            city: (207, 213),
            city_name: "Daggerfall".into(),
            theme: "rescue",
            location: empty_location(),
            blocks_buf: Arc::new(Vec::new()),
            arena_dir: PathBuf::from("/tmp"),
        };
        let quest = generate_quest(&test_cfg(srv.port), &tpl(), &job, &[]).unwrap();
        assert_eq!(quest.city, (207, 213));
        assert_eq!(quest.seed.tomb_name, "Nekhar-Zul");
        assert_eq!(quest.rumors.len(), 5);
        assert_eq!(quest.rumor(Tone::Drunk), Some(OK_TEXTS[4]));
        let bodies = srv.bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 4);
        let prompt_of = |i: usize| {
            let b = Json::parse(&bodies[i]).unwrap();
            b.get("messages").and_then(Json::as_array).unwrap()[0].get("content").and_then(Json::as_str).unwrap().to_string()
        };
        assert!(prompt_of(0).contains("Required theme: rescue") && !prompt_of(0).contains("WARNING"));
        assert!(prompt_of(1).contains("WARNING") && prompt_of(1).contains("invalid JSON"));
        assert!(prompt_of(2).contains("WARNING") && prompt_of(2).contains("dungeon_id 'invented' not in catalog"));
        assert!(prompt_of(3).contains("Nekhar-Zul"));
    }

    #[test]
    fn generation_gives_up_after_three_attempts() {
        let srv = fake_server(vec![(200, "no".into()), (200, "no".into()), (200, "no".into())]);
        let job = Job {
            city: (1, 1),
            city_name: "X".into(),
            theme: "rescue",
            location: empty_location(),
            blocks_buf: Arc::new(Vec::new()),
            arena_dir: PathBuf::from("/tmp"),
        };
        let err = generate_quest(&test_cfg(srv.port), &tpl(), &job, &[]).unwrap_err();
        assert!(err.contains("SEED") && err.contains("3 attempts"), "{err}");
        assert_eq!(srv.bodies.lock().unwrap().len(), 3);
    }

    // ---------------------------------------------------------------- archivio

    fn sample_quest() -> Quest {
        Quest {
            city: (207, 213),
            city_name: "Daggerfall".into(),
            seed: seed(),
            rumors: OK_TEXTS.iter().zip(Tone::ALL).map(|(t, tone)| (tone, t.to_string())).collect(),
            npc_flavors: Vec::new(),
        }
    }

    #[test]
    fn quest_survives_a_round_trip_through_the_text_file() {
        let q = sample_quest();
        assert_eq!(Quest::from_json(&Json::parse(&q.to_json().compact()).unwrap()), Some(q.clone()));
        let dir = std::env::temp_dir().join(format!("quest_book_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("quests.txt");
        let _ = std::fs::remove_file(&path);
        append_quest(&path, &q).unwrap();
        let mut q2 = sample_quest();
        q2.city = (10, 20);
        q2.city_name = "Wayrest".into();
        append_quest(&path, &q2).unwrap();
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"\n{not json\n{\"a\":1}\n").unwrap();
        let book = QuestBook::load(&path);
        assert_eq!(book.len(), 2);
        assert_eq!(book.quest((10, 20)).map(|q| q.city_name.as_str()), Some("Wayrest"));
        assert!(book.knows((207, 213)) && !book.knows((1, 1)));
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().filter(|l| l.starts_with('{') && l.contains("city_x")).count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hints_depend_on_city_state_and_tone() {
        let mut book = QuestBook::default();
        assert_eq!(book.hint(Some((1, 1)), Tone::Noble), Hint::Nothing);
        assert_eq!(book.hint(None, Tone::Noble), Hint::Nothing);
        book.pending.insert((1, 1));
        assert_eq!(book.hint(Some((1, 1)), Tone::Noble), Hint::Pending);
        book.pending.remove(&(1, 1));
        book.failed.insert((1, 1));
        assert_eq!(book.hint(Some((1, 1)), Tone::Noble), Hint::Nothing);
        book.quests.insert((207, 213), sample_quest());
        assert_eq!(book.hint(Some((207, 213)), Tone::Merchant), Hint::Text(OK_TEXTS[1]));
        assert_eq!(book.hint(Some((207, 213)), Tone::Drunk), Hint::Text(OK_TEXTS[4]));
    }

    #[test]
    fn npc_flavors_round_trip_and_lookup() {
        let mut q = sample_quest();
        q.npc_flavors.push(NpcFlavor {
            archive: 183,
            record: 2,
            caption: "guard".into(),
                           text: "Guard's voice about the quest.".into(),
        });
        let parsed = Quest::from_json(&Json::parse(&q.to_json().compact()).unwrap()).unwrap();
        assert_eq!(parsed.npc_flavors.len(), 1);
        let mut book = QuestBook::default();
        book.quests.insert(q.city, parsed);
        assert_eq!(book.npc_flavor((207, 213), 183, 2), Some("Guard's voice about the quest."));
        assert_eq!(book.npc_flavor((207, 213), 999, 0), None);
    }

    // ------------------------------------------------------------- thread

    #[test]
    fn worker_generates_saves_and_reuses_an_external_server() {
        let srv = fake_server(vec![(200, seed_reply()), (200, rumors_reply())]);
        let dir = std::env::temp_dir().join(format!("quest_worker_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("quests.txt");
        let _ = std::fs::remove_file(&path);
        let (job_tx, job_rx) = mpsc::channel();
        let (res_tx, res_rx) = mpsc::channel();
        let server: SharedServer = Arc::new(Mutex::new(None));
        let (cfg, t, p, s) = (test_cfg(srv.port), tpl(), path.clone(), Arc::clone(&server));
        let handle = std::thread::spawn(move || worker_main(cfg, t, p, job_rx, res_tx, s));
        job_tx
        .send(Job {
            city: (207, 213),
              city_name: "Daggerfall".into(),
              theme: "escort",
              location: empty_location(),
              blocks_buf: Arc::new(Vec::new()),
              arena_dir: dir.clone(),
        })
        .unwrap();
        match res_rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            QuestMsg::Done(q) => assert_eq!((q.city, q.rumors.len()), ((207, 213), 5)),
            QuestMsg::Failed { reason, .. } => panic!("failed: {reason}"),
        }
        assert_eq!(QuestBook::load(&path).len(), 1);
        assert!(server.lock().unwrap().is_none());
        job_tx
        .send(Job {
            city: (5, 5),
              city_name: "Elsewhere".into(),
              theme: "theft",
              location: empty_location(),
              blocks_buf: Arc::new(Vec::new()),
              arena_dir: dir.clone(),
        })
        .unwrap();
        match res_rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            QuestMsg::Failed { city, .. } => assert_eq!(city, (5, 5)),
            QuestMsg::Done(_) => panic!("should have failed"),
        }
        assert_eq!(QuestBook::load(&path).len(), 1);
        drop(job_tx);
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_binary_or_model_fails_without_hanging() {
        let mut cfg = test_cfg(1);
        let err = ensure_server(&cfg, &Arc::new(Mutex::new(None))).unwrap_err();
        assert!(err.contains("llama-server not found"), "{err}");
        cfg.server_bin = std::env::current_exe().unwrap().display().to_string();
        cfg.model_path = "/does/not/exist.gguf".into();
        let err = ensure_server(&cfg, &Arc::new(Mutex::new(None))).unwrap_err();
        assert!(err.contains("model not found"), "{err}");
    }

    // ---------------------------------------------------- processo llama-server

    #[test]
    fn server_command_line_matches_the_script() {
        let cfg = QuestConfig { model_path: "/m/x.gguf".into(), ..QuestConfig::default() };
        assert_eq!(
            server_args(&cfg, Some(99)).join(" "),
                   "-m /m/x.gguf -c 8192 -ngl 99 --host 127.0.0.1 --port 8080 -t 4 --reasoning-budget 0"
        );
        assert_eq!(
            server_args(&cfg, None).join(" "),
                   "-m /m/x.gguf -c 8192 --host 127.0.0.1 --port 8080 -t 4 --reasoning-budget 0"
        );
        let cfg = QuestConfig { threads: 0, disable_thinking: false, ..cfg };
        assert_eq!(server_args(&cfg, Some(0)).join(" "), "-m /m/x.gguf -c 8192 -ngl 0 --host 127.0.0.1 --port 8080");
    }

    #[test]
    fn out_of_memory_is_recognised_in_the_server_log() {
        assert!(looks_like_oom("E ggml_backend_cuda_buffer_type_alloc_buffer: cudaMalloc failed: out of memory"));
        assert!(looks_like_oom("failed to allocate buffer for kv cache"));
        assert!(!looks_like_oom("I srv load_model: loading model\nE srv: file not found"));
    }

    #[test]
    fn fewer_gpu_layers_are_tried_after_an_oom() {
        assert_eq!(ngl_candidates(None, None), vec![None, Some(32), Some(16), Some(8), Some(0)]);
        assert_eq!(ngl_candidates(Some(99), None), vec![Some(99), Some(49), Some(24), Some(12), Some(6), Some(3), Some(1), Some(0)]);
        assert_eq!(ngl_candidates(Some(0), None), vec![Some(0)]);
        assert_eq!(ngl_candidates(None, Some(16)), vec![Some(16), Some(8), Some(0)]);
        assert_eq!(ngl_candidates(Some(99), Some(12)), vec![Some(12), Some(6), Some(3), Some(1), Some(0)]);
    }

    #[test]
    fn library_dirs_include_cuda_folders_and_vendor() {
        static N: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!("quest_libs_{}_{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&root);
        let exe_dir = root.join("llama.cpp-linux-x86_64-nvidia-cuda12-avx2-2.55.0");
        let cuda = exe_dir.join("a").join("b");
        std::fs::create_dir_all(&cuda).unwrap();
        std::fs::write(cuda.join("libcudart.so.12"), b"").unwrap();
        std::fs::write(exe_dir.join("llama-server"), b"").unwrap();
        let deep = exe_dir.join("1").join("2").join("3").join("4");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("libcublas.so.12"), b"").unwrap();
        for v in ["vendor-cuda12", "vendor-cuda13", "vendor-cpu"] {
            std::fs::create_dir_all(root.join("vendor").join(v)).unwrap();
        }
        let dirs = llama_lib_dirs(&exe_dir.join("llama-server"));
        assert_eq!(dirs[0], exe_dir);
        assert!(dirs.contains(&cuda));
        assert!(!dirs.contains(&deep));
        assert!(dirs.contains(&root.join("vendor").join("vendor-cuda12")));
        assert!(!dirs.contains(&root.join("vendor").join("vendor-cuda13")));
        assert!(!dirs.contains(&root.join("vendor").join("vendor-cpu")));
        assert_eq!(ld_library_path(&[PathBuf::from("/a"), PathBuf::from("/b")], Some("/x")), "/a:/b:/x");
        assert_eq!(ld_library_path(&[PathBuf::from("/a")], Some("")), "/a");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---------------------------------------------------------- PNG e citta'

    #[test]
    fn npc_tone_is_stable_and_follows_the_place() {
        let city = (207, 213);
        for key in 0..40u64 {
            assert_eq!(tone_for_npc(key, city, None), tone_for_npc(key, city, None));
            assert_eq!(tone_for_npc(key, city, Some(PoiKind::Shop)), Tone::Merchant);
        }
        let count = |place: Option<PoiKind>, tone: Tone| (0..2000u64).filter(|k| tone_for_npc(*k, city, place) == tone).count();
        assert!(count(Some(PoiKind::Tavern), Tone::Drunk) > count(Some(PoiKind::Tavern), Tone::Commoner));
        assert!(count(None, Tone::Commoner) > count(None, Tone::Merchant));
        for tone in Tone::ALL {
            assert!(count(None, tone) > 0, "{tone:?}");
        }
        let a: Vec<Tone> = (0..30u64).map(|k| tone_for_npc(k, (1, 1), None)).collect();
        let b: Vec<Tone> = (0..30u64).map(|k| tone_for_npc(k, (2, 9), None)).collect();
        assert_ne!(a, b);
    }

    fn location(blocks: &[&str]) -> DFLocation {
        DFLocation {
            name: "Place".into(),
            region_index: 0,
            world_x: 0,
            world_y: 0,
            width: blocks.len(),
            height: 1,
            block_names: blocks.iter().map(|s| s.to_string()).collect(),
            letter1_for_rmb_name: None,
        }
    }

    #[test]
    fn only_towns_get_quests() {
        assert!(is_town(&location(&["MAGEAA13.RMB", "N0000001.rmb", "TEMPAA01.RMB"])));
        assert!(!is_town(&location(&["TAVNAA01.RMB"])));
        assert!(!is_town(&location(&["N0000001.RDB", "N0000002.RDB", "N0000003.RDB"])));
        assert!(!is_town(&location(&["A.RMB", "B.RDB"])));
        assert!(!is_town(&location(&[])));
    }

    #[test]
    fn each_city_gets_a_stable_valid_theme() {
        for x in 0..50 {
            let t = theme_for((x, 7));
            assert!(THEMES.contains(&t));
            assert_eq!(t, theme_for((x, 7)));
        }
        let distinct: HashSet<_> = (0..100).map(|x| theme_for((x, 3))).collect();
        assert!(distinct.len() >= 4);
    }

    // ------------------------------------------------- voci su richiesta

    #[test]
    fn walker_description_follows_the_face_blocks() {
        assert_eq!(walker_look(0), "Nord woman");
        assert_eq!(walker_look(24 + 5), "Breton woman");
        assert_eq!(walker_look(72), "woman"); // blocco condiviso da Nord e Breton
        assert_eq!(walker_look(96 + 23), "Redguard woman");
        assert_eq!(walker_look(192), "Breton man");
        assert_eq!(walker_look(240 + 3), "man");
        assert_eq!(walker_look(264), "Nord man");
        assert_eq!(walker_look(336 + 23), "Redguard man");
        assert_eq!(walker_look(168), "townsperson"); // blocchi che nessun archivio usa
        assert_eq!(walker_look(10_000), "townsperson");
        assert_eq!(walker_key(37), "walker:37");
        assert_eq!(flat_key(175, 0), "flat:175:0");
        assert_eq!(face_key(9), "face:9");
    }

    #[test]
    fn npc_description_is_stable_and_complete() {
        let t1 = temperament_for("walker:37", (207, 213));
        assert_eq!(t1, temperament_for("walker:37", (207, 213)));
        assert!(TEMPERAMENTS.contains(&t1));
        let all: HashSet<_> = (0..200).map(|i| temperament_for(&format!("walker:{i}"), (1, 1))).collect();
        assert!(all.len() >= 8, "poca varieta': {all:?}");

        let d = describe_npc("beautiful maiden", None, Some(PoiKind::Tavern), "cheerful");
        assert_eq!(
            d,
            "- Appearance: beautiful maiden\n- Location: inside a tavern\n- Temperament (not visible, shapes how they talk): cheerful"
        );
        let d = describe_npc("Nord woman", Some("merchant"), None, "weary");
        assert!(d.contains("- Standing: merchant\n") && d.contains("out on the street"));
        assert_eq!(Tone::Drunk.role(), "drunkard");
    }

    #[test]
    fn voice_prompt_has_the_seed_and_the_npc_but_not_the_generic_dungeon() {
        let p = build_npc_voice_prompt(DEFAULT_NPC_VOICE_PROMPT, &seed(), "- Appearance: old beggar");
        assert!(p.contains("Nekhar-Zul") && p.contains("- Appearance: old beggar"));
        assert!(!p.contains("{seed_json}") && !p.contains("{npc}"));
        assert!(!p.contains("Cripta di Cenere") && !p.contains("dgf_crypt_ash"));
    }

    #[test]
    fn voice_validation_follows_the_rumor_rules() {
        let s = seed();
        let ok = |t: &str| validate_voice(&Json::Obj(vec![("text".into(), jstr(t))]), &s);
        assert!(ok("They say the Mages Guild sniffs around Nekhar-Zul. Bad business.").is_ok());
        assert!(ok("").unwrap_err().contains("empty"));
        assert!(ok(&"a".repeat(VOICE_MAX_CHARS + 1)).unwrap_err().contains("too long"));
        assert!(ok("Stay away from the  CRIPTA  di cenere!").unwrap_err().contains("generic name"));
        assert!(ok("Someone has already found the Occhio di Vael.").unwrap_err().contains("already recovered"));
        assert!(validate_voice(&Json::Obj(vec![]), &s).is_err());
    }

    #[test]
    fn voices_are_saved_and_reloaded() {
        let dir = std::env::temp_dir().join(format!("quest_voices_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("quest_npc_voices.txt");
        let _ = std::fs::remove_file(&path);
        append_voice(&path, (207, 213), "walker:37", "- Appearance: Nord woman", "Psst, listen \"here\".").unwrap();
        append_voice(&path, (10, 20), "flat:175:0", "- Appearance: maiden", "Hush.").unwrap();
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"\n{rotto\n{\"a\":1}\n").unwrap();

        let mut book = QuestBook::default();
        book.load_voices(&path);
        assert_eq!(book.voice_count(), 2);
        assert_eq!(book.voice_state((207, 213), "walker:37"), VoiceState::Ready("Psst, listen \"here\"."));
        assert_eq!(book.voice_state((10, 20), "flat:175:0"), VoiceState::Ready("Hush."));
        assert_eq!(book.voice_state((10, 20), "walker:37"), VoiceState::Unknown); // la citta' conta
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn voice_requests_need_a_quest_and_are_not_repeated() {
        let (job_tx, _job_rx) = mpsc::channel();
        let (voice_tx, voice_rx) = mpsc::channel();
        let (_res_tx, res_rx) = mpsc::channel();
        let (ask_tx, _ask_rx) = mpsc::channel();
        let worker = QuestWorker {
            jobs: job_tx,
            voice_jobs: voice_tx,
            ask_jobs: ask_tx,
            next_ask: AtomicU64::new(1),
            results: Mutex::new(res_rx),
            server: Arc::new(Mutex::new(None)),
        };
        let mut book = QuestBook::default();
        // nessuna quest in citta': niente richiesta
        assert!(!worker.request_voice(&mut book, (207, 213), "walker:1", "- Appearance: x"));
        assert!(voice_rx.try_recv().is_err());
        book.quests.insert((207, 213), sample_quest());
        assert!(worker.request_voice(&mut book, (207, 213), "walker:1", "- Appearance: x"));
        assert_eq!(book.voice_state((207, 213), "walker:1"), VoiceState::Pending);
        let job = voice_rx.try_recv().unwrap();
        assert_eq!((job.city, job.key.as_str(), job.seed.tomb_name.as_str()), ((207, 213), "walker:1", "Nekhar-Zul"));
        // gia' in corso: non si ripete
        assert!(!worker.request_voice(&mut book, (207, 213), "walker:1", "- Appearance: x"));
        assert!(voice_rx.try_recv().is_err());
        // arrivata: pronta; e un altro PNG puo' chiedere la sua
        book.store_voice((207, 213), "walker:1".into(), "Hush.".into());
        assert_eq!(book.voice_state((207, 213), "walker:1"), VoiceState::Ready("Hush."));
        assert!(!worker.request_voice(&mut book, (207, 213), "walker:1", "- Appearance: x"));
        assert!(worker.request_voice(&mut book, (207, 213), "walker:2", "- Appearance: y"));
        // fallita: non si ritenta nella sessione
        book.fail_voice((207, 213), "walker:2".into());
        assert_eq!(book.voice_state((207, 213), "walker:2"), VoiceState::Failed);
        assert!(!worker.request_voice(&mut book, (207, 213), "walker:2", "- Appearance: y"));
        std::mem::forget(worker); // niente server da spegnere
    }

    #[test]
    fn worker_generates_voices_on_demand_and_quests_keep_priority() {
        let srv = fake_server(vec![
            (200, "no json".into()),                                                // 1° tentativo: non e' JSON
            (200, r#"{"text": "Folks whisper about Nekhar-Zul, mind you."}"#.into()), // ok
            (200, "no".into()), (200, "no".into()), (200, "no".into()),            // seconda voce: 3 rifiuti
        ]);
        let dir = std::env::temp_dir().join(format!("quest_voice_worker_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (quests, voices) = (dir.join("quests.txt"), dir.join("quest_npc_voices.txt"));
        let _ = std::fs::remove_file(&voices);

        let (job_tx, job_rx) = mpsc::channel();
        let (voice_tx, voice_rx) = mpsc::channel();
        let (res_tx, res_rx) = mpsc::channel();
        let server: SharedServer = Arc::new(Mutex::new(None));
        let (cfg, t, q, v, s) = (test_cfg(srv.port), tpl(), quests.clone(), voices.clone(), Arc::clone(&server));
        let (_ask_tx, ask_rx) = mpsc::channel();
        let handle = std::thread::spawn(move || worker_loop(cfg, t, q, v, job_rx, voice_rx, ask_rx, res_tx, s));

        let job = |key: &str| VoiceJob {
            city: (207, 213),
            key: key.into(),
            description: "- Appearance: Nord woman".into(),
            seed: seed(),
        };
        voice_tx.send(job("walker:1")).unwrap();
        match res_rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            QuestMsg::Voice { city, key, text } => {
                assert_eq!((city, key.as_str()), ((207, 213), "walker:1"));
                assert_eq!(text, "Folks whisper about Nekhar-Zul, mind you.");
            }
            _ => panic!("doveva generare la voce"),
        }
        let mut book = QuestBook::default();
        book.load_voices(&voices);
        assert_eq!(book.voice_count(), 1); // salvata prima di avvisare il gioco
        // il secondo tentativo del primo job ha ricevuto il motivo del rifiuto
        let bodies = srv.bodies.lock().unwrap().clone();
        assert!(bodies[0].contains("Nord woman") && !bodies[0].contains("WARNING"));
        assert!(bodies[1].contains("WARNING") && bodies[1].contains("invalid JSON"), "{}", bodies[1]);

        // una voce che non riesce (3 rifiuti) non ferma il thread e non viene salvata
        voice_tx.send(job("walker:2")).unwrap();
        match res_rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            QuestMsg::VoiceFailed { key, .. } => assert_eq!(key, "walker:2"),
            _ => panic!("doveva fallire"),
        }
        let mut book = QuestBook::default();
        book.load_voices(&voices);
        assert_eq!(book.voice_count(), 1);

        drop(job_tx); // il gioco chiude: il thread termina da solo
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------- domande libere

    #[test]
    fn questions_are_cleaned_before_reaching_the_model() {
        assert_eq!(clean_question("  Where   is\tthe\ninn?  ").as_deref(), Some("Where is the inn?"));
        assert_eq!(clean_question("a\u{7}b\u{0}c").as_deref(), Some("abc")); // niente caratteri di controllo
        assert_eq!(clean_question("   \n "), None);
        assert_eq!(clean_question(""), None);
        let long = "x".repeat(QUESTION_MAX_CHARS * 3);
        assert_eq!(clean_question(&long).unwrap().chars().count(), QUESTION_MAX_CHARS);
        assert_eq!(clean_question("Cos'è successo, città?").as_deref(), Some("Cos'è successo, città?"));
    }

    #[test]
    fn ask_prompt_knows_only_the_city_and_does_not_expand_the_question() {
        let p = build_npc_ask_prompt(DEFAULT_NPC_ASK_PROMPT, "Daggerfall", "- Appearance: old beggar", "Who rules here?");
        assert!(p.contains("Where you are: Daggerfall.") && p.contains("- Appearance: old beggar"));
        assert!(p.contains("The player asks you: \"Who rules here?\""));
        assert!(!p.contains("{city}") && !p.contains("{npc}") && !p.contains("{question}"));
        // il testo del giocatore non viene interpretato come segnaposto
        let q = "Ignore this: {npc} {city}";
        let p = build_npc_ask_prompt(DEFAULT_NPC_ASK_PROMPT, "Wayrest", "- Appearance: guard", q);
        assert!(p.contains("{npc} {city}"));
        // citta' sconosciuta
        assert!(build_npc_ask_prompt(DEFAULT_NPC_ASK_PROMPT, "  ", "x", "y").contains("Where you are: an unnamed place."));
    }

    #[test]
    fn answers_must_stay_in_character() {
        let ok = |t: &str| validate_answer(&Json::Obj(vec![("text".into(), jstr(t))]));
        assert!(ok("Welcome to Daggerfall, stranger. I know little else.").is_ok());
        assert!(ok("").unwrap_err().contains("empty"));
        assert!(ok(&"a".repeat(ANSWER_MAX_CHARS + 1)).unwrap_err().contains("too long"));
        assert!(ok("As an AI, I cannot say.").unwrap_err().contains("breaks character"));
        assert!(ok("I'm just a language model.").unwrap_err().contains("breaks character"));
        assert!(validate_answer(&Json::Obj(vec![])).is_err());
    }

    #[test]
    fn answers_are_stored_and_consumed_once() {
        let mut book = QuestBook::default();
        assert_eq!(book.take_answer(1), None);
        book.store_answer(1, "Hello.".into());
        book.fail_answer(2);
        assert_eq!(book.take_answer(1), Some(Ok("Hello.".to_string())));
        assert_eq!(book.take_answer(1), None); // gia' ritirata
        assert_eq!(book.take_answer(2), Some(Err(())));
    }

    #[test]
    fn asking_sends_a_cleaned_question_with_increasing_ids() {
        let (job_tx, _job_rx) = mpsc::channel();
        let (voice_tx, _voice_rx) = mpsc::channel();
        let (ask_tx, ask_rx) = mpsc::channel();
        let (_res_tx, res_rx) = mpsc::channel();
        let worker = QuestWorker {
            jobs: job_tx,
            voice_jobs: voice_tx,
            ask_jobs: ask_tx,
            next_ask: AtomicU64::new(7),
            results: Mutex::new(res_rx),
            server: Arc::new(Mutex::new(None)),
        };
        assert_eq!(worker.ask("Daggerfall", "- Appearance: x", "   "), None); // domanda vuota: niente
        assert!(ask_rx.try_recv().is_err());
        assert_eq!(worker.ask("Daggerfall", "- Appearance: x", "  Who  are you? "), Some(7));
        assert_eq!(worker.ask("Daggerfall", "- Appearance: x", "Where is the inn?"), Some(8));
        let first = ask_rx.try_recv().unwrap();
        assert_eq!((first.id, first.question.as_str(), first.city_name.as_str()), (7, "Who are you?", "Daggerfall"));
        assert_eq!(ask_rx.try_recv().unwrap().id, 8);
        // thread di lavoro chiuso: nessuna risposta in arrivo
        drop(ask_rx);
        assert_eq!(worker.ask("Daggerfall", "x", "hello?"), None);
    }

    #[test]
    fn worker_answers_questions_before_voices_and_retries_out_of_character_replies() {
        let srv = fake_server(vec![
            (200, r#"{"text": "I am an AI and cannot help."}"#.into()),                  // fuori carattere
            (200, r#"{"text": "Daggerfall? Aye, that's where we are, friend."}"#.into()), // ok
        ]);
        let dir = std::env::temp_dir().join(format!("quest_ask_worker_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (job_tx, job_rx) = mpsc::channel();
        let (voice_tx, voice_rx) = mpsc::channel();
        let (ask_tx, ask_rx) = mpsc::channel();
        let (res_tx, res_rx) = mpsc::channel();
        let server: SharedServer = Arc::new(Mutex::new(None));
        let (cfg, t, q, v, s) = (test_cfg(srv.port), tpl(), dir.join("q.txt"), dir.join("v.txt"), Arc::clone(&server));
        let handle = std::thread::spawn(move || worker_loop(cfg, t, q, v, job_rx, voice_rx, ask_rx, res_tx, s));

        ask_tx
            .send(AskJob {
                id: 3,
                city_name: "Daggerfall".into(),
                description: "- Appearance: old beggar".into(),
                question: "What is this place?".into(),
            })
            .unwrap();
        match res_rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            QuestMsg::Answer { id, text } => {
                assert_eq!(id, 3);
                assert_eq!(text, "Daggerfall? Aye, that's where we are, friend.");
            }
            _ => panic!("doveva rispondere"),
        }
        let bodies = srv.bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 2);
        assert!(bodies[0].contains("What is this place?") && bodies[0].contains("old beggar"));
        assert!(bodies[1].contains("WARNING") && bodies[1].contains("breaks character"), "{}", bodies[1]);
        // la risposta non si salva su file: e' una conversazione, non una voce
        assert!(!dir.join("v.txt").exists());

        drop(job_tx);
        drop(voice_tx);
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
