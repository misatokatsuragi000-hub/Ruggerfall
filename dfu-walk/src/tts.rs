//! Voci parlate dei PNG con Kokoro TTS.
//!
//! Il dialogo manda un `SpeakEvent` con il testo e un `Speaker` (chi parla: genere, ruolo,
//! carattere, chiave stabile). Qui si sceglie la voce di Kokoro e la velocita' in base a quei
//! descrittori, si sintetizza in un thread dedicato e si riproduce il WAV appena pronto.
//!
//! Server: `api_server.py` (FastAPI sopra la libreria `kokoro`), su `127.0.0.1:8880`.
//! Accetta sia `input` sia `text` nel corpo della richiesta, oltre a `model` e
//! `response_format`. Il formato di risposta e' sempre WAV PCM16 a 24 kHz (oppure `pcm`
//! s16le mono grezzo se richiesto); la lingua (`lang`) e' dedotta dal prefisso della voce
//! (`af_*` → americano, `bf_*` → britannico, `if_*` → italiano, ecc.).
//!
//! File nella cartella da cui si lancia il gioco:
//!   - `tts.conf`      configurazione (server, formato, elenchi di voci). Se manca viene creata.
//!   - `voice_audio/`  cache dei WAV, un file per (voce, velocita', testo): una frase gia'
//!                     sintetizzata, in questa sessione o in una precedente, parte subito.
//!
//! Audio: la voce usa un tipo di sorgente suo (`SpeechSource`), non l'`AudioPlayer` standard,
//! perche' `music::play_music` despawna tutti gli `AudioPlayer<AudioSource>` a ogni cambio
//! di musica. In `Cargo.toml` servono le feature `wav` e `vorbis` di Bevy.
//!
//! In `main.rs` servono:  `mod tts;`  e  `.add_plugins(tts::TtsPlugin)`

use crate::dialog::DialogState;
use crate::quests::Tone;
use bevy::audio::{AddAudioSource, AudioPlayer, AudioSource, Decodable, PlaybackSettings, Volume};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const TTS_CONFIG_FILE: &str = "tts.conf";

/// Dopo un errore (server spento, ecc.) si riprova a sintetizzare la stessa frase solo dopo tanto.
const RETRY_AFTER: Duration = Duration::from_secs(20);
/// Lunghezza massima per il parlato (sufficiente per i testi integrali dei quest principali).
const MAX_SPOKEN_CHARS: usize = 4000;

// =============================================================================
// Configurazione
// =============================================================================

#[derive(Clone, Debug, PartialEq)]
pub struct TtsConfig {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub path: String,
    pub model: String,
    /// "wav" (consigliato) oppure "pcm" (s16 mono grezzo a `sample_rate`).
    pub response_format: String,
    pub sample_rate: u32,
    pub volume: f32,
    pub timeout_secs: u64,
    pub cache_dir: String,
    pub voices_female: Vec<String>,
    pub voices_male: Vec<String>,
    /// I nobili parlano con voci britanniche.
    pub voices_noble_female: Vec<String>,
    pub voices_noble_male: Vec<String>,
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            host: "127.0.0.1".into(),
            port: 8880,
            path: "/v1/audio/speech".into(),
            model: "kokoro".into(),
            response_format: "wav".into(),
            sample_rate: 24_000,
            volume: 1.0,
            timeout_secs: 120,
            cache_dir: "voice_audio".into(),
            voices_female: names(&["af_bella", "af_nicole", "af_sarah", "af_sky", "af_nova", "af_jessica", "af_heart"]),
            voices_male: names(&["am_adam", "am_michael", "am_fenrir", "am_puck", "am_eric", "am_liam", "am_onyx"]),
            voices_noble_female: names(&["bf_emma", "bf_isabella", "bf_alice", "bf_lily"]),
            voices_noble_male: names(&["bm_george", "bm_lewis", "bm_daniel", "bm_fable"]),
        }
    }
}

fn parse_list(v: &str) -> Vec<String> {
    v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

impl TtsConfig {
    /// Legge il testo del file (`chiave=valore`, `#` per i commenti). Chiavi sconosciute e valori
    /// illeggibili vengono ignorati; un elenco di voci vuoto lascia quello di default.
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
                "enabled" => c.enabled = !matches!(v.to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"),
                "host" if !v.is_empty() => c.host = v.to_string(),
                "port" => c.port = v.parse().unwrap_or(c.port),
                "path" if !v.is_empty() => c.path = v.to_string(),
                "model" if !v.is_empty() => c.model = v.to_string(),
                "response_format" if !v.is_empty() => c.response_format = v.to_ascii_lowercase(),
                "sample_rate" => c.sample_rate = v.parse().unwrap_or(c.sample_rate),
                "volume" => c.volume = v.parse().unwrap_or(c.volume),
                "timeout_secs" => c.timeout_secs = v.parse().unwrap_or(c.timeout_secs),
                "cache_dir" if !v.is_empty() => c.cache_dir = v.to_string(),
                "voices_female" if !parse_list(v).is_empty() => c.voices_female = parse_list(v),
                "voices_male" if !parse_list(v).is_empty() => c.voices_male = parse_list(v),
                "voices_noble_female" if !parse_list(v).is_empty() => c.voices_noble_female = parse_list(v),
                "voices_noble_male" if !parse_list(v).is_empty() => c.voices_noble_male = parse_list(v),
                _ => {}
            }
        }
        c
    }

    pub fn to_text(&self) -> String {
        format!(
            "# Voci parlate dei PNG con Kokoro TTS. Modificalo e riavvia il gioco.\n\
# enabled=false disattiva le voci (nessun thread, nessuna richiesta).\n\
enabled={}\n\
\n\
# Server Kokoro (api_server.py). Avvialo a parte.\n\
host={}\n\
port={}\n\
path={}\n\
model={}\n\
# wav (consigliato) oppure pcm (s16 mono grezzo a sample_rate)\n\
response_format={}\n\
sample_rate={}\n\
volume={}\n\
timeout_secs={}\n\
# Cache dei WAV gia' sintetizzati\n\
cache_dir={}\n\
\n\
# Voci per genere (separate da virgola). Ogni PNG ne ha sempre la stessa.\n\
voices_female={}\n\
voices_male={}\n\
# Voci dei nobili (Kokoro ha voci britanniche)\n\
voices_noble_female={}\n\
voices_noble_male={}\n",
self.enabled,
self.host,
self.port,
self.path,
self.model,
self.response_format,
self.sample_rate,
self.volume,
self.timeout_secs,
self.cache_dir,
self.voices_female.join(","),
                self.voices_male.join(","),
                self.voices_noble_female.join(","),
                self.voices_noble_male.join(","),
        )
    }

    pub fn load_or_create(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(_) => {
                let cfg = Self::default();
                if let Err(e) = std::fs::write(path, cfg.to_text()) {
                    warn!("Voci: impossibile creare {}: {e}", path.display());
                }
                cfg
            }
        }
    }
}

// =============================================================================
// Chi parla e quale voce usa
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gender {
    Female,
    Male,
}

/// I descrittori di un PNG che decidono la sua voce.
#[derive(Clone, Debug, PartialEq)]
pub struct Speaker {
    /// Chiave stabile del PNG (la stessa delle voci su richiesta): stessa chiave, stessa voce.
    pub key: String,
    /// None = non si sa: si sceglie in modo stabile dalla chiave.
    pub gender: Option<Gender>,
    pub role: Option<Tone>,
    pub temperament: &'static str,
    /// Voce imposta (es. `af_sarah`, dal campo `voice=` del .txt di un NPC custom):
    /// se presente sostituisce la scelta automatica per genere/ruolo.
    pub voice: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VoiceSpec {
    pub voice: String,
    pub speed: f32,
}

/// Il dialogo manda questo evento quando un PNG dice qualcosa.
#[derive(Event, Clone, Debug)]
pub struct SpeakEvent {
    pub text: String,
    pub speaker: Speaker,
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
}

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphabetic()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// "Nord woman", "Redguard man", "woman", "man": il genere dalla descrizione di un passante.
pub fn gender_from_look(look: &str) -> Option<Gender> {
    gender_from_caption(look)
}

/// Il genere dalla caption di FLATS.CFG ("beautiful maiden", "old man", "guard"...).
pub fn gender_from_caption(caption: &str) -> Option<Gender> {
    const FEMALE: [&str; 39] = [
        "woman", "women", "maiden", "lady", "girl", "wife", "mother", "matron", "madam",
        "priestess", "queen", "princess", "sister", "barmaid", "waitress", "wench",
        "noblewoman", "witch", "female", "grandmother",
        "blonde", "brunette", "damsel", "lass", "lassie", "widow", "bride", "nun",
        "mistress", "sorceress", "enchantress", "seamstress", "hag", "crone", "goddess",
        "vampiress", "heroine", "adventuress", "mermaid",
    ];
    const MALE: [&str; 31] = [
        "man", "men", "boy", "knight", "king", "prince", "priest", "father", "brother",
        "lord", "gentleman", "fisherman", "grandfather", "male", "barman", "innkeeper",
        "sailor",
        "lad", "sir", "master", "monk", "friar", "wizard", "sorcerer", "necromancer",
        "duke", "count", "baron", "earl", "jester", "watchman",
    ];
    let ws = words(caption);
    if ws.iter().any(|w| FEMALE.contains(&w.as_str())) {
        Some(Gender::Female)
    } else if ws.iter().any(|w| MALE.contains(&w.as_str())) {
        Some(Gender::Male)
    } else {
        None
    }
}

fn temperament_speed(t: &str) -> f32 {
    match t {
        "nervous" => 0.08,
        "cheerful" => 0.04,
        "weary" => -0.08,
        "proud" => -0.04,
        "curious" => 0.02,
        "suspicious" => -0.03,
        "gloomy" => -0.10,
        "talkative" => 0.08,
        "superstitious" => -0.02,
        "greedy" => 0.03,
        _ => 0.0,
    }
}

fn role_speed(role: Option<Tone>) -> f32 {
    match role {
        Some(Tone::Drunk) => -0.12,
        Some(Tone::Noble) => -0.03,
        Some(Tone::Merchant) => 0.05,
        _ => 0.0,
    }
}

/// Riconosce se un discorso appartiene a un quest o alla trama principale.
pub fn is_main_quest_speech(sp: &Speaker, text: &str) -> bool {
    // 1. I monologhi di quest e le spiegazioni di trama sono tipicamente lunghe
    if text.chars().count() >= 220 {
        return true;
    }
    // 2. Chiavi generate dai moduli di quest
    let k = sp.key.to_ascii_lowercase();
    if k.contains("quest") || k.contains("s000") || k.contains("n0b") {
        return true;
    }
    // 3. Parole chiave legate alle figure cardine della trama principale di Daggerfall
    let lower = text.to_lowercase();
    const MAIN_QUEST_KEYWORDS: &[&str] = &[
        "underking", "lysandus", "totem", "mantella", "numidium",
        "lhotun", "gothryd", "barenziah", "brisienna", "nulfaga",
        "medora", "mannimarco", "king of worms", "emperor",
        "uriel septim", "blades", "missing prince", "daggerfall",
        "wayrest", "sentinel",
    ];
    MAIN_QUEST_KEYWORDS.iter().any(|kw| lower.contains(kw))
}

/// Calcola la voce di base del PNG.
pub fn voice_for(cfg: &TtsConfig, sp: &Speaker) -> VoiceSpec {
    let h = mix(fnv1a(&sp.key));
    let gender = sp.gender.unwrap_or(if h & 1 == 0 { Gender::Female } else { Gender::Male });
    let noble = sp.role == Some(Tone::Noble);
    let (preferred, fallback) = match (gender, noble) {
        (Gender::Female, true) => (&cfg.voices_noble_female, &cfg.voices_female),
        (Gender::Male, true) => (&cfg.voices_noble_male, &cfg.voices_male),
        (Gender::Female, false) => (&cfg.voices_female, &cfg.voices_female),
        (Gender::Male, false) => (&cfg.voices_male, &cfg.voices_male),
    };
    let list = if preferred.is_empty() { fallback } else { preferred };
    let auto = list.get((h >> 8) as usize % list.len().max(1)).cloned().unwrap_or_else(|| "af_bella".to_string());
    let voice = match sp.voice.as_deref().map(str::trim) {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => auto,
    };
    let speed = (1.0 + role_speed(sp.role) + temperament_speed(sp.temperament)).clamp(0.7, 1.3);
    VoiceSpec { voice, speed: (speed * 100.0).round() / 100.0 }
}

/// Calcola la voce applicando un leggero rallentamento (effetto "Gravitas") se si tratta di una quest principale.
pub fn voice_for_speech(cfg: &TtsConfig, sp: &Speaker, text: &str) -> VoiceSpec {
    let mut spec = voice_for(cfg, sp);
    if is_main_quest_speech(sp, text) {
        // Riduce la velocità di circa il 12% per conferire maggiore solennità e autorevolezza narrativa
        spec.speed = (spec.speed * 0.70).clamp(0.72, 1.15);
        spec.speed = (spec.speed * 100.0).round() / 100.0;
    }
    spec
}

/// Identificativo di una frase sintetizzata: voce + velocita' + testo.
pub fn clip_key(spec: &VoiceSpec, text: &str) -> u64 {
    fnv1a(&format!("{}|{:.2}|{}", spec.voice, spec.speed, text))
}

// =============================================================================
// WAV: normalizzazione
// =============================================================================

/// Un WAV canonico con l'intestazione corretta.
pub fn normalize_wav(data: &[u8], pcm_rate: u32, raw_pcm: bool) -> Result<Vec<u8>, String> {
    fn build(fmt: &[u8], pcm: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(44 + pcm.len());
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((36 + pcm.len()) as u32).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(fmt);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        out.extend_from_slice(pcm);
        out
    }
    if data.len() >= 12 && &data[0..4] == b"RIFF" && &data[8..12] == b"WAVE" {
        let mut fmt: Option<[u8; 16]> = None;
        let mut i = 12usize;
        while i + 8 <= data.len() {
            let id = &data[i..i + 4];
            let size = u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]) as usize;
            let body = i + 8;
            if id == b"fmt " && body + 16 <= data.len() {
                let mut f = [0u8; 16];
                f.copy_from_slice(&data[body..body + 16]);
                fmt = Some(f);
            } else if id == b"data" {
                let fmt = fmt.ok_or("WAV senza chunk 'fmt '")?;
                let end = if size == 0 || body.saturating_add(size) > data.len() { data.len() } else { body + size };
                if end <= body {
                    return Err("WAV senza campioni".into());
                }
                return Ok(build(&fmt, &data[body..end]));
            }
            i = body.saturating_add(size).saturating_add(size & 1);
        }
        return Err("WAV senza chunk 'data'".into());
    }
    if raw_pcm && !data.is_empty() {
        let mut fmt = [0u8; 16];
        fmt[0..2].copy_from_slice(&1u16.to_le_bytes()); // PCM
        fmt[2..4].copy_from_slice(&1u16.to_le_bytes()); // mono
        fmt[4..8].copy_from_slice(&pcm_rate.to_le_bytes());
        fmt[8..12].copy_from_slice(&(pcm_rate * 2).to_le_bytes());
        fmt[12..14].copy_from_slice(&2u16.to_le_bytes());
        fmt[14..16].copy_from_slice(&16u16.to_le_bytes());
        return Ok(build(&fmt, &data[..data.len() & !1]));
    }
    let head: String = data.iter().take(4).map(|b| format!("{b:02x}")).collect();
    Err(format!("risposta non WAV (inizia con {head}): imposta response_format=wav in {TTS_CONFIG_FILE}"))
}

// =============================================================================
// HTTP minimale (binario) verso Kokoro
// =============================================================================

fn decode_chunked(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let Some(nl) = data[i..].windows(2).position(|w| w == b"\r\n") else { break };
        let size_line = String::from_utf8_lossy(&data[i..i + nl]).to_string();
        let Ok(size) = usize::from_str_radix(size_line.split(';').next().unwrap_or("").trim(), 16) else { break };
        i += nl + 2;
        if size == 0 {
            break;
        }
        let end = (i + size).min(data.len());
        out.extend_from_slice(&data[i..end]);
        i = end + 2;
    }
    out
}

fn parse_http_response(raw: &[u8]) -> Result<(u16, Vec<u8>), String> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("risposta HTTP incompleta")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let body = &raw[split + 4..];
    let status: u16 = head
    .lines()
    .next()
    .and_then(|l| l.split_whitespace().nth(1))
    .and_then(|c| c.parse().ok())
    .ok_or("riga di stato HTTP non valida")?;
    let chunked =
    head.lines().skip(1).any(|l| l.to_ascii_lowercase().replace(' ', "").starts_with("transfer-encoding:chunked"));
    Ok((status, if chunked { decode_chunked(body) } else { body.to_vec() }))
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
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
    out
}

fn lang_for_voice(voice: &str) -> &'static str {
    match voice.chars().next() {
        Some('a') => "a",
        Some('b') => "b",
        Some('e') => "e",
        Some('f') => "f",
        Some('i') => "i",
        Some('j') => "j",
        Some('z') => "z",
        _ => "a",
    }
}

fn speech_body(cfg: &TtsConfig, text: &str, spec: &VoiceSpec) -> String {
    format!(
        "{{\"model\":{},\"input\":{},\"text\":{},\"voice\":{},\"lang\":{},\"response_format\":{},\"speed\":{}}}",
        json_string(&cfg.model),
            json_string(text),
            json_string(text),
            json_string(&spec.voice),
            json_string(lang_for_voice(&spec.voice)),
            json_string(&cfg.response_format),
            spec.speed
    )
}

fn http_post(cfg: &TtsConfig, body: &str) -> Result<(u16, Vec<u8>), String> {
    let addr: SocketAddr = (cfg.host.as_str(), cfg.port)
    .to_socket_addrs()
    .map_err(|e| format!("indirizzo {}:{}: {e}", cfg.host, cfg.port))?
    .next()
    .ok_or("indirizzo non risolto")?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
    .map_err(|e| format!("Kokoro non raggiungibile su {addr}: {e}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(cfg.timeout_secs.max(60)))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(15))).ok();
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nAccept: audio/wav, */*\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        cfg.path,
        cfg.host,
        cfg.port,
        body.len()
    );
    stream.write_all(request.as_bytes()).map_err(|e| format!("invio richiesta: {e}"))?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| format!("lettura audio: {e}"))?;
    parse_http_response(&raw)
}

/// Sintetizza `text` con `spec` e restituisce un WAV valido.
fn synthesize(cfg: &TtsConfig, text: &str, spec: &VoiceSpec) -> Result<Vec<u8>, String> {
    let (status, body) = http_post(cfg, &speech_body(cfg, text, spec))?;
    if status != 200 {
        let head: String = String::from_utf8_lossy(&body).chars().take(200).collect();
        return Err(format!("Kokoro: HTTP {status}: {head}"));
    }
    normalize_wav(&body, cfg.sample_rate, cfg.response_format == "pcm")
}

// =============================================================================
// Thread di sintesi e cache
// =============================================================================

struct TtsJob {
    key: u64,
    text: String,
    spec: VoiceSpec,
}

enum TtsMsg {
    Ready(u64),
    Failed(u64, String),
}

fn clip_path(dir: &Path, key: u64) -> PathBuf {
    dir.join(format!("{key:016x}.wav"))
}

fn tts_worker(cfg: TtsConfig, dir: PathBuf, jobs: Receiver<TtsJob>, results: Sender<TtsMsg>) {
    let mut last_warning: Option<Instant> = None;
    while let Ok(job) = jobs.recv() {
        let path = clip_path(&dir, job.key);
        let outcome = if path.exists() {
            Ok(())
        } else {
            synthesize(&cfg, &job.text, &job.spec).and_then(|wav| {
                let tmp = path.with_extension("tmp");
                std::fs::write(&tmp, &wav).map_err(|e| format!("scrittura {}: {e}", tmp.display()))?;
                std::fs::rename(&tmp, &path).map_err(|e| format!("rinomina {}: {e}", path.display()))
            })
        };
        let msg = match outcome {
            Ok(()) => TtsMsg::Ready(job.key),
            Err(reason) => {
                if last_warning.map_or(true, |t| t.elapsed() > Duration::from_secs(60)) {
                    warn!("Voci: {reason}");
                    last_warning = Some(Instant::now());
                }
                TtsMsg::Failed(job.key, reason)
            }
        };
        if results.send(msg).is_err() {
            break;
        }
    }
}

enum ClipState {
    Pending,
    Ready,
    Failed(Instant),
}

#[derive(Resource)]
pub struct Speech {
    cfg: TtsConfig,
    dir: PathBuf,
    jobs: Sender<TtsJob>,
    results: Mutex<Receiver<TtsMsg>>,
    state: HashMap<u64, ClipState>,
}

impl Speech {
    /// Chiede la sintesi di una frase. Se supera `MAX_SPOKEN_CHARS`, tronca sull'ultimo punto fermo.
    fn request(&mut self, text: &str, spec: &VoiceSpec) -> u64 {
        let text: String = if text.chars().count() > MAX_SPOKEN_CHARS {
            let truncated: String = text.chars().take(MAX_SPOKEN_CHARS).collect();
            if let Some(last_punct) = truncated.rfind(['.', '!', '?']) {
                truncated[..=last_punct].to_string()
            } else {
                truncated
            }
        } else {
            text.to_string()
        };

        let key = clip_key(spec, &text);
        match self.state.get(&key) {
            Some(ClipState::Pending | ClipState::Ready) => return key,
            Some(ClipState::Failed(at)) if at.elapsed() < RETRY_AFTER => return key,
            _ => {}
        }
        if clip_path(&self.dir, key).exists() {
            self.state.insert(key, ClipState::Ready);
        } else if self.jobs.send(TtsJob { key, text, spec: spec.clone() }).is_ok() {
            self.state.insert(key, ClipState::Pending);
        }
        key
    }

    fn is_ready(&self, key: u64) -> bool {
        matches!(self.state.get(&key), Some(ClipState::Ready))
    }

    fn poll(&mut self) {
        let Ok(results) = self.results.try_lock() else { return };
        while let Ok(msg) = results.try_recv() {
            match msg {
                TtsMsg::Ready(key) => {
                    self.state.insert(key, ClipState::Ready);
                }
                TtsMsg::Failed(key, reason) => {
                    debug!("Voci: sintesi fallita per {key:016x}: {reason}");
                    self.state.insert(key, ClipState::Failed(Instant::now()));
                }
            }
        }
    }
}

// =============================================================================
// Riproduzione
// =============================================================================

#[derive(Asset, TypePath, Clone)]
pub struct SpeechSource {
    bytes: Arc<[u8]>,
}

impl Decodable for SpeechSource {
    type DecoderItem = <<AudioSource as Decodable>::Decoder as Iterator>::Item;
    type Decoder = <AudioSource as Decodable>::Decoder;

    fn decoder(&self) -> Self::Decoder {
        AudioSource { bytes: Arc::clone(&self.bytes) }.decoder()
    }
}

#[derive(Component)]
pub(crate) struct SpeechVoice;

#[derive(Resource, Default)]
struct Playback {
    wanted: Option<u64>,
}

fn handle_speak_events(mut events: EventReader<SpeakEvent>, mut speech: ResMut<Speech>, mut playback: ResMut<Playback>) {
    if let Some(event) = events.read().last() {
        let spec = voice_for_speech(&speech.cfg, &event.speaker, &event.text);
        playback.wanted = Some(speech.request(&event.text, &spec));
    }
}

#[allow(clippy::too_many_arguments)]
fn speech_playback(
    mut commands: Commands,
    mut speech: ResMut<Speech>,
    mut playback: ResMut<Playback>,
    dialog: Res<DialogState>,
    mut sources: ResMut<Assets<SpeechSource>>,
    voices: Query<Entity, With<SpeechVoice>>,
) {
    speech.poll();

    // Se il giocatore chiude volontariamente la schermata di dialogo, interrompe la voce corrente
    if dialog.is_changed() && !dialog.open {
        playback.wanted = None;
        for e in &voices {
            commands.entity(e).despawn();
        }
        return;
    }

    let Some(key) = playback.wanted else { return };
    if !speech.is_ready(key) {
        return;
    }
    playback.wanted = None;
    for e in &voices {
        commands.entity(e).despawn();
    }
    match std::fs::read(clip_path(&speech.dir, key)) {
        Ok(bytes) => {
            let handle = sources.add(SpeechSource { bytes: bytes.into() });
            commands.spawn((
                AudioPlayer::<SpeechSource>(handle),
                            PlaybackSettings::DESPAWN.with_volume(Volume::new(speech.cfg.volume)),
                            SpeechVoice,
            ));
        }
        Err(e) => warn!("Voci: impossibile leggere la cache audio: {e}"),
    }
}

// =============================================================================
// Plugin
// =============================================================================

pub struct TtsPlugin;

impl Plugin for TtsPlugin {
    fn build(&self, app: &mut App) {
        app.add_event::<SpeakEvent>();
        let cfg = TtsConfig::load_or_create(Path::new(TTS_CONFIG_FILE));
        if !cfg.enabled {
            info!("Voci: disattivate ({TTS_CONFIG_FILE})");
            return;
        }
        let dir = PathBuf::from(&cfg.cache_dir);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            warn!("Voci: impossibile creare {}: {e}", dir.display());
            return;
        }
        let (job_tx, job_rx) = mpsc::channel();
        let (res_tx, res_rx) = mpsc::channel();
        let (worker_cfg, worker_dir) = (cfg.clone(), dir.clone());
        if let Err(e) = std::thread::Builder::new()
            .name("tts".into())
            .spawn(move || tts_worker(worker_cfg, worker_dir, job_rx, res_tx))
            {
                warn!("Voci: impossibile avviare il thread di sintesi: {e}");
                return;
            }
            info!("Voci: Kokoro su {}:{}{}", cfg.host, cfg.port, cfg.path);
        app.add_audio_source::<SpeechSource>()
        .init_resource::<Playback>()
        .insert_resource(Speech { cfg, dir, jobs: job_tx, results: Mutex::new(res_rx), state: HashMap::new() })
        .add_systems(Update, (handle_speak_events, speech_playback).chain());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    fn speaker(key: &str, gender: Option<Gender>, role: Option<Tone>, temperament: &'static str) -> Speaker {
        Speaker { key: key.into(), gender, role, temperament, voice: None }
    }

    #[test]
    fn config_defaults_match_the_kokoro_server() {
        let c = TtsConfig::default();
        assert_eq!((c.host.as_str(), c.port, c.path.as_str()), ("127.0.0.1", 8880, "/v1/audio/speech"));
        assert_eq!(c.response_format, "wav");
        assert!(c.voices_noble_male.iter().all(|v| v.starts_with("bm_")));
        assert!(c.voices_female.iter().all(|v| v.starts_with("af_")));
    }

    #[test]
    fn config_round_trips_and_ignores_bad_lines() {
        let mut c = TtsConfig::default();
        c.port = 9000;
        c.enabled = false;
        c.voices_male = names(&["am_adam", "bm_lewis"]);
        c.volume = 0.8;
        assert_eq!(TtsConfig::parse(&c.to_text()), c);

        let p = TtsConfig::parse("# x\nport=boh\nvoices_female=\nenabled=off\nresponse_format=PCM\nsconosciuta=1\n");
        assert_eq!(p.port, 8880);
        assert_eq!(p.voices_female, TtsConfig::default().voices_female);
        assert!(!p.enabled);
        assert_eq!(p.response_format, "pcm");
    }

    #[test]
    fn gender_comes_from_explicit_words_only() {
        assert_eq!(gender_from_look("Nord woman"), Some(Gender::Female));
        assert_eq!(gender_from_look("Redguard man"), Some(Gender::Male));
        assert_eq!(gender_from_look("townsperson"), None);
        assert_eq!(gender_from_caption("beautiful maiden"), Some(Gender::Female));
        assert_eq!(gender_from_caption("old man with a cane"), Some(Gender::Male));
        assert_eq!(gender_from_caption("Barmaid, tired"), Some(Gender::Female));
        assert_eq!(gender_from_caption("city guard"), None);
        assert_eq!(gender_from_caption("old beggar"), None);
        assert_eq!(gender_from_caption("rich woman"), Some(Gender::Female));
        assert_eq!(gender_from_caption("womanly"), None);
    }

    #[test]
    fn blonde_and_similar_words_are_female() {
        assert_eq!(gender_from_caption("dazzling blonde"), Some(Gender::Female));
        assert_eq!(gender_from_caption("tall blonde"), Some(Gender::Female));
        assert_eq!(gender_from_caption("pretty brunette"), Some(Gender::Female));
        assert_eq!(gender_from_caption("young damsel"), Some(Gender::Female));
        assert_eq!(gender_from_caption("an old crone"), Some(Gender::Female));
        assert_eq!(gender_from_caption("the local seamstress"), Some(Gender::Female));
        assert_eq!(gender_from_caption("village bride"), Some(Gender::Female));
        assert_eq!(gender_from_caption("tall guard"), None);
        assert_eq!(gender_from_caption("city merchant"), None);
        assert_eq!(gender_from_caption("masked thief"), None);
        assert_eq!(gender_from_caption("a noble person"), None);
    }

    #[test]
    fn male_words_added_are_recognised() {
        assert_eq!(gender_from_caption("young lad"), Some(Gender::Male));
        assert_eq!(gender_from_caption("the old wizard"), Some(Gender::Male));
        assert_eq!(gender_from_caption("a proud duke"), Some(Gender::Male));
        assert_eq!(gender_from_caption("a jester"), Some(Gender::Male));
    }

    #[test]
    fn voice_is_stable_per_npc_and_follows_gender_and_role() {
        let cfg = TtsConfig::default();
        let s = speaker("walker:37", Some(Gender::Female), Some(Tone::Commoner), "weary");
        assert_eq!(voice_for(&cfg, &s), voice_for(&cfg, &s));
        assert!(cfg.voices_female.contains(&voice_for(&cfg, &s).voice));
        let m = speaker("walker:37", Some(Gender::Male), Some(Tone::Commoner), "weary");
        assert!(cfg.voices_male.contains(&voice_for(&cfg, &m).voice));
        let nf = speaker("walker:5", Some(Gender::Female), Some(Tone::Noble), "proud");
        let nm = speaker("walker:5", Some(Gender::Male), Some(Tone::Noble), "proud");
        assert!(voice_for(&cfg, &nf).voice.starts_with("bf_"));
        assert!(voice_for(&cfg, &nm).voice.starts_with("bm_"));
        let voices: std::collections::HashSet<String> = (0..60)
        .map(|i| voice_for(&cfg, &speaker(&format!("walker:{i}"), Some(Gender::Male), None, "")).voice)
        .collect();
        assert!(voices.len() >= 5, "{voices:?}");
    }

    #[test]
    fn unknown_gender_is_decided_by_the_key_and_both_genders_appear() {
        let cfg = TtsConfig::default();
        let a = voice_for(&cfg, &speaker("face:9", None, None, ""));
        assert_eq!(a, voice_for(&cfg, &speaker("face:9", None, None, "")));
        let genders: std::collections::HashSet<bool> = (0..40)
        .map(|i| cfg.voices_female.contains(&voice_for(&cfg, &speaker(&format!("face:{i}"), None, None, "")).voice))
        .collect();
        assert_eq!(genders.len(), 2);
    }

    #[test]
    fn explicit_voice_overrides_the_automatic_choice() {
        let cfg = TtsConfig::default();
        let mut s = speaker("custom_Bartolo", Some(Gender::Male), None, "");
        s.voice = Some("af_sarah".to_string());
        assert_eq!(voice_for(&cfg, &s).voice, "af_sarah");
        s.voice = Some("  ".to_string());
        assert!(cfg.voices_male.contains(&voice_for(&cfg, &s).voice));
    }

    #[test]
    fn empty_noble_lists_fall_back_to_the_normal_ones() {
        let mut cfg = TtsConfig::default();
        cfg.voices_noble_female.clear();
        let v = voice_for(&cfg, &speaker("x", Some(Gender::Female), Some(Tone::Noble), ""));
        assert!(cfg.voices_female.contains(&v.voice));
    }

    #[test]
    fn speed_depends_on_role_and_temperament_within_limits() {
        let cfg = TtsConfig::default();
        let speed = |role, t| voice_for(&cfg, &speaker("k", Some(Gender::Male), role, t)).speed;
        assert_eq!(speed(None, ""), 1.0);
        assert_eq!(speed(Some(Tone::Drunk), "gloomy"), 0.78);
        assert_eq!(speed(Some(Tone::Merchant), "talkative"), 1.13);
        assert!(speed(Some(Tone::Drunk), "weary") < speed(Some(Tone::Merchant), "nervous"));
        assert!((0.7..=1.3).contains(&speed(Some(Tone::Drunk), "gloomy")));
    }

    #[test]
    fn gravitas_slows_down_main_quest_speech() {
        let cfg = TtsConfig::default();
        let sp = speaker("walker:10", Some(Gender::Male), Some(Tone::Noble), "proud");
        let normal_spec = voice_for(&cfg, &sp);
        let quest_spec = voice_for_speech(&cfg, &sp, "Welcome to Sentinel, traveler. The Underking and Prince Lhotun are involved in this mystery.");
        assert!(quest_spec.speed < normal_spec.speed);
    }

    #[test]
    fn clip_key_changes_with_voice_speed_and_text() {
        let a = VoiceSpec { voice: "af_bella".into(), speed: 1.0 };
        let b = VoiceSpec { voice: "am_adam".into(), speed: 1.0 };
        let c = VoiceSpec { voice: "af_bella".into(), speed: 0.9 };
        let keys = [clip_key(&a, "hi"), clip_key(&b, "hi"), clip_key(&c, "hi"), clip_key(&a, "hello")];
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), 4);
        assert_eq!(clip_key(&a, "hi"), clip_key(&a.clone(), "hi"));
    }

    // ------------------------------------------------------------------- WAV

    fn riff(data_size_field: u32, pcm: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&u32::MAX.to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&[1, 0, 1, 0]);
        v.extend_from_slice(&24_000u32.to_le_bytes());
        v.extend_from_slice(&48_000u32.to_le_bytes());
        v.extend_from_slice(&[2, 0, 16, 0]);
        v.extend_from_slice(b"data");
        v.extend_from_slice(&data_size_field.to_le_bytes());
        v.extend_from_slice(pcm);
        v
    }

    #[test]
    fn wav_headers_from_streaming_servers_are_repaired() {
        let pcm = [1u8, 0, 2, 0, 3, 0, 4, 0];
        for placeholder in [0u32, u32::MAX, 0x7fff_ffff] {
            let out = normalize_wav(&riff(placeholder, &pcm), 24_000, false).unwrap();
            assert_eq!(out.len(), 44 + pcm.len());
            assert_eq!(&out[0..4], b"RIFF");
            assert_eq!(u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize, 36 + pcm.len());
            assert_eq!(&out[36..40], b"data");
            assert_eq!(u32::from_le_bytes(out[40..44].try_into().unwrap()) as usize, pcm.len());
            assert_eq!(&out[44..], &pcm);
            assert_eq!(u32::from_le_bytes(out[24..28].try_into().unwrap()), 24_000);
        }
        let good = normalize_wav(&riff(8, &pcm), 24_000, false).unwrap();
        assert_eq!(normalize_wav(&good, 24_000, false).unwrap(), good);
        let mut with_tail = riff(8, &pcm);
        with_tail.extend_from_slice(b"LIST\x04\x00\x00\x00abcd");
        assert_eq!(normalize_wav(&with_tail, 24_000, false).unwrap(), good);
    }

    #[test]
    fn raw_pcm_is_wrapped_and_other_formats_are_rejected() {
        let wav = normalize_wav(&[1, 0, 2, 0, 9], 22_050, true).unwrap();
        assert_eq!(wav.len(), 44 + 4);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 22_050);
        assert!(normalize_wav(&[1, 0, 2, 0], 24_000, false).unwrap_err().contains("response_format=wav"));
        assert!(normalize_wav(b"ID3\x04", 24_000, false).is_err());
        assert!(normalize_wav(&[], 24_000, true).is_err());
        assert!(normalize_wav(b"RIFF\x00\x00\x00\x00WAVE", 24_000, false).is_err());
    }

    #[test]
    fn http_binary_bodies_are_parsed_plain_and_chunked() {
        let body = [0u8, 255, 1, 254, 13, 10, 7];
        let mut plain = b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: 7\r\n\r\n".to_vec();
        plain.extend_from_slice(&body);
        assert_eq!(parse_http_response(&plain).unwrap(), (200, body.to_vec()));
        let mut chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        chunked.extend_from_slice(b"3\r\n");
        chunked.extend_from_slice(&body[..3]);
        chunked.extend_from_slice(b"\r\n4\r\n");
        chunked.extend_from_slice(&body[3..]);
        chunked.extend_from_slice(b"\r\n0\r\n\r\n");
        assert_eq!(parse_http_response(&chunked).unwrap(), (200, body.to_vec()));
        assert_eq!(parse_http_response(b"HTTP/1.1 500 X\r\n\r\nerr").unwrap(), (500, b"err".to_vec()));
        assert!(parse_http_response(b"HTTP/1.1 200").is_err());
    }

    #[test]
    fn request_body_is_valid_json_with_escaped_text_and_lang() {
        let cfg = TtsConfig::default();
        let spec = VoiceSpec { voice: "af_bella".into(), speed: 0.95 };
        let body = speech_body(&cfg, "He said \"hi\"\nand left.", &spec);
        assert_eq!(
            body,
            "{\"model\":\"kokoro\",\"input\":\"He said \\\"hi\\\"\\nand left.\",\"text\":\"He said \\\"hi\\\"\\nand left.\",\"voice\":\"af_bella\",\"lang\":\"a\",\"response_format\":\"wav\",\"speed\":0.95}"
        );
        let spec_b = VoiceSpec { voice: "bf_emma".into(), speed: 1.0 };
        assert!(speech_body(&cfg, "x", &spec_b).contains("\"lang\":\"b\""));
        let spec_i = VoiceSpec { voice: "if_sara".into(), speed: 1.0 };
        assert!(speech_body(&cfg, "x", &spec_i).contains("\"lang\":\"i\""));
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn make_speech(cfg: TtsConfig, dir: &Path) -> Speech {
        let (job_tx, job_rx) = mpsc::channel();
        let (res_tx, res_rx) = mpsc::channel();
        let (c, d) = (cfg.clone(), dir.to_path_buf());
        std::thread::spawn(move || tts_worker(c, d, job_rx, res_tx));
        Speech { cfg, dir: dir.to_path_buf(), jobs: job_tx, results: Mutex::new(res_rx), state: HashMap::new() }
    }

    #[test]
    fn very_long_text_is_cut_before_synthesis() {
        let dir = temp_dir("tts_long");
        let (job_tx, job_rx) = mpsc::channel();
        let (_res_tx, res_rx) = mpsc::channel();
        let mut speech = Speech {
            cfg: TtsConfig::default(),
            dir: dir.clone(),
            jobs: job_tx,
            results: Mutex::new(res_rx),
            state: HashMap::new(),
        };
        let spec = VoiceSpec { voice: "af_bella".into(), speed: 1.0 };
        speech.request(&"a".repeat(6000), &spec);
        assert_eq!(job_rx.try_recv().unwrap().text.chars().count(), MAX_SPOKEN_CHARS);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
