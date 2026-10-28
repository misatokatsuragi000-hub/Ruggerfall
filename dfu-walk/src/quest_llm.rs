//! Infrastruttura LLM: HTTP, gestione llama-server, worker thread, plugin.
//! Non contiene modelli di dominio: quelli sono in `crate::quests`.

use crate::data::blocks::RmbBlock;
use crate::data::bsa::BsaArchive;
use crate::data::flats::FlatCatalog;
use crate::data::maps::DFLocation;
use crate::player::Player;
use crate::quests::{
    build_npc_ask_prompt, build_npc_flavor_prompt, build_npc_voice_prompt, build_rumors_prompt,
    build_seed_prompt, clean_question, enrich_seed, is_town, jstr, rumors_from_json,
    theme_for, validate_answer, validate_npc_flavors, validate_rumors, validate_seed, validate_voice,
    Json, NpcFlavor, Quest, QuestBook, QuestConfig, Seed, VoiceState, MAX_HISTORY_TURNS,
    MAX_NPC_FLAVORS, CONFIG_FILE, DEFAULT_NPC_ASK_PROMPT, DEFAULT_NPC_FLAVOR_PROMPT,
    DEFAULT_NPC_VOICE_PROMPT, DEFAULT_RUMORS_PROMPT, DEFAULT_SEED_PROMPT,
};
use crate::world::{map_pixel_of, GameData, Origin};
use bevy::prelude::*;
use std::collections::HashMap;
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
// Costanti di percorso
// =============================================================================

pub const QUESTS_FILE: &str = "quests.txt";
pub const SEED_PROMPT_FILE: &str = "quest_seed_prompt.txt";
pub const RUMORS_PROMPT_FILE: &str = "quest_rumors_prompt.txt";
pub const NPC_FLAVOR_PROMPT_FILE: &str = "quest_npc_flavor_prompt.txt";
pub const NPC_VOICE_PROMPT_FILE: &str = "quest_npc_voice_prompt.txt";
pub const VOICES_FILE: &str = "quest_npc_voices.txt";
pub const NPC_ASK_PROMPT_FILE: &str = "quest_npc_ask_prompt.txt";

const WORKER_POLL: Duration = Duration::from_millis(100);


// =============================================================================
// Estrazione del JSON dalla risposta del modello
// =============================================================================

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
// HTTP minimale
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
            if size > 0 { out.extend_from_slice(&data[i..]); }
            break;
        }
        out.extend_from_slice(&data[i..i + size]);
        i += size + 2;
    }
    out
}

fn parse_http_response(raw: &[u8]) -> Result<(u16, String), String> {
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("incomplete HTTP response")?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let body = &raw[split + 4..];
    let status: u16 = head.lines().next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .ok_or("invalid HTTP status line")?;
    let chunked = head.lines().skip(1)
        .any(|l| l.to_ascii_lowercase().replace(' ', "").starts_with("transfer-encoding:chunked"));
    let body = if chunked { decode_chunked(body) } else { body.to_vec() };
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
}

fn http_request(port: u16, method: &str, path: &str, body: Option<&str>, read_timeout: Duration)
    -> Result<(u16, String), String>
{
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|e| format!("connection to {addr}: {e}"))?;
    stream.set_read_timeout(Some(read_timeout)).ok();
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
            if (200..300).contains(&status) { return true; }
        }
    }
    false
}

fn chat_body(cfg: &QuestConfig, prompt: &str, with_template_kwargs: bool) -> String {
    let mut fields = vec![
        ("model".to_string(), jstr("llm")),
        ("messages".to_string(), Json::Arr(vec![Json::Obj(vec![
            ("role".into(), jstr("user")), ("content".into(), jstr(prompt)),
        ])])),
        ("temperature".to_string(), Json::Num(0.85)),
        ("max_tokens".to_string(), Json::Num(cfg.max_tokens as f64)),
    ];
    if with_template_kwargs {
        fields.push(("chat_template_kwargs".to_string(),
                     Json::Obj(vec![("enable_thinking".into(), Json::Bool(false))])));
    }
    Json::Obj(fields).compact()
}

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
        return json.get("choices").and_then(Json::as_array)
            .and_then(|c| c.first()).and_then(|c| c.get("message"))
            .and_then(|m| m.get("content")).and_then(Json::as_str).map(str::to_string)
            .ok_or_else(|| "response without choices[0].message.content".to_string());
    }
}

// =============================================================================
// Gestione llama-server
// =============================================================================

fn collect_lib_dirs(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 3 { return; }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut subdirs = Vec::new();
    let mut has_cuda_lib = false;
    for entry in rd.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        match entry.file_type() {
            Ok(t) if t.is_dir() => subdirs.push(entry.path()),
            _ => {
                if ["libcudart.so", "libcublas.so", "libcublasLt.so", "libcudnn.so", "libnvJitLink.so"]
                    .iter().any(|p| name.starts_with(p)) { has_cuda_lib = true; }
            }
        }
    }
    if has_cuda_lib && !out.iter().any(|d| d == dir) { out.push(dir.to_path_buf()); }
    subdirs.sort();
    for sub in subdirs { collect_lib_dirs(&sub, depth + 1, out); }
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
                if is_cuda12 && !name.contains("cuda12") { continue; }
                if !dirs.contains(&v) { dirs.push(v); }
            }
        }
    }
    dirs
}

fn ld_library_path(dirs: &[PathBuf], existing: Option<&str>) -> String {
    let mut parts: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
    if let Some(e) = existing.filter(|e| !e.is_empty()) { parts.push(e.to_string()); }
    parts.join(":")
}

fn server_args(cfg: &QuestConfig, ngl: Option<u32>) -> Vec<String> {
    let mut a: Vec<String> = vec!["-m".into(), cfg.model_path.clone(), "-c".into(), cfg.ctx.to_string()];
    if let Some(n) = ngl { a.push("-ngl".into()); a.push(n.to_string()); }
    a.extend(["--host".into(), "127.0.0.1".into(), "--port".into(), cfg.port.to_string()]);
    if cfg.threads > 0 { a.push("-t".into()); a.push(cfg.threads.to_string()); }
    if cfg.disable_thinking { a.push("--reasoning-budget".into()); a.push("0".into()); }
    a
}

fn log_tail(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(12)..].join("\n")
}

fn spawn_server(cfg: &QuestConfig, ngl: Option<u32>) -> Result<(Child, PathBuf), String> {
    let bin = Path::new(&cfg.server_bin);
    if !bin.exists() { return Err(format!("llama-server not found: {}", cfg.server_bin)); }
    if !Path::new(&cfg.model_path).exists() { return Err(format!("model not found: {}", cfg.model_path)); }
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
        } else { Command::new(bin) };
        cmd.args(&args).env("LD_LIBRARY_PATH", &ld).current_dir(&exe_dir)
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
    } else { spawn(false) }.map_err(|e| format!("starting llama-server: {e}"))?;
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
        .iter().any(|p| l.contains(p))
}

fn ngl_candidates(configured: Option<u32>, working: Option<u32>) -> Vec<Option<u32>> {
    let mut chain: Vec<Option<u32>> = Vec::new();
    match configured {
        None => { chain.push(None); chain.extend([32, 16, 8, 0].map(Some)); }
        Some(n) => {
            chain.push(Some(n));
            let mut k = n / 2;
            while k > 0 { chain.push(Some(k)); k /= 2; }
            if n > 0 { chain.push(Some(0)); }
        }
    }
    if let Some(w) = working {
        chain.retain(|c| c.map_or(false, |n| n < w));
        chain.insert(0, Some(w));
    }
    chain
}

enum StartError { OutOfMemory(String), Other(String) }

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
                    return Err(if looks_like_oom(&full_log) {
                        StartError::OutOfMemory(msg)
                    } else { StartError::Other(msg) });
                }
            }
        }
        if server_health(cfg.port) { return Ok(()); }
        if Instant::now() > deadline {
            stop_server(server);
            return Err(StartError::Other(format!("server did not start within {}s", cfg.startup_timeout_secs)));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn ensure_server(cfg: &QuestConfig, server: &SharedServer) -> Result<(), String> {
    if server_health(cfg.port) { return Ok(()); }
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
// Generazione (usa i prompt/validatori di quests.rs)
// =============================================================================

#[derive(Clone)]
pub struct Templates {
    pub seed: String,
    pub rumors: String,
    pub npc_flavor: String,
    pub voice: String,
    pub ask: String,
}

pub struct Job {
    pub city: (i32, i32),
    pub city_name: String,
    pub theme: &'static str,
    pub location: DFLocation,
    pub blocks_buf: Arc<Vec<u8>>,
    pub arena_dir: PathBuf,
}

struct VoiceJob { city: (i32, i32), key: String, description: String, seed: Seed }

struct AskJob {
    id: u64, city_name: String, description: String, question: String,
    history: Vec<(String, String)>,
}

fn try_json(cfg: &QuestConfig, prompt: &str, label: &str,
             validate: impl Fn(&Json) -> Result<(), String>) -> Result<Json, String>
{
    let mut last_err = String::new();
    for attempt in 1..=3 {
        let p = if attempt > 1 {
            format!("{prompt}\nWARNING: the previous output was rejected ({last_err}). Reply ONLY with the JSON object.")
        } else { prompt.to_string() };
        info!("Quest [{label}] attempt {attempt}/3");
        let raw = match call_llm(cfg, &p) {
            Ok(r) => r,
            Err(e) => { warn!("Quest [{label}] LLM error: {e}"); last_err = e; continue; }
        };
        let parsed = match Json::parse(&extract_json(&raw)) {
            Ok(v) => v,
            Err(e) => { last_err = format!("invalid JSON: {e}"); warn!("Quest [{label}] {last_err}"); continue; }
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

fn is_enterable_type(bt: u8) -> bool {
    matches!(bt, 0 | 2 | 5 | 6 | 7 | 8 | 9 | 11 | 12 | 13 | 14 | 15)
}

fn collect_npc_flats(location: &DFLocation, blocks_buf: &[u8], catalog: &FlatCatalog)
    -> Vec<(u16, u8, String)>
{
    let Ok(blocks_bsa) = BsaArchive::parse(blocks_buf) else { return Vec::new() };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for by in 0..location.height {
        for bx in 0..location.width {
            let Some(name) = location.block_names.get(by * location.width + bx) else { continue };
            let Some(raw) = blocks_bsa.get_by_name(name) else { continue };
            let Ok(rmb) = RmbBlock::parse(raw) else { continue };
            for (i, it) in rmb.interiors.iter().enumerate() {
                let enterable = rmb.buildings.get(i).map_or(false, |b| is_enterable_type(b.building_type));
                if !enterable { continue; }
                for p in &it.people {
                    let key = (p.texture_archive, p.texture_record);
                    if !seen.insert(key) { continue; }
                    if let Some(info) = catalog.get(p.texture_archive, p.texture_record) {
                        if !info.caption.is_empty() {
                            out.push((p.texture_archive, p.texture_record, info.caption.clone()));
                        }
                    }
                }
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

fn generate_npc_flavors(cfg: &QuestConfig, tpl: &Templates, seed: &Seed,
                          npc_flats: &[(u16, u8, String)]) -> Vec<NpcFlavor>
{
    if npc_flats.is_empty() { return Vec::new(); }
    let mut order: Vec<String> = Vec::new();
    let mut by_cap: HashMap<String, Vec<(u16, u8)>> = HashMap::new();
    for (a, r, cap) in npc_flats {
        if !by_cap.contains_key(cap) { order.push(cap.clone()); }
        by_cap.entry(cap.clone()).or_default().push((*a, *r));
    }
    order.truncate(MAX_NPC_FLAVORS);
    let npc_list = order.iter().enumerate().map(|(i, c)| format!("[{i}] \"{c}\""))
        .collect::<Vec<_>>().join("\n");
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

fn generate_quest(cfg: &QuestConfig, tpl: &Templates, job: &Job,
                   npc_flats: &[(u16, u8, String)]) -> Result<Quest, String>
{
    let seed_json = try_json(cfg, &build_seed_prompt(&tpl.seed, job.theme), "SEED", validate_seed)?;
    let seed = enrich_seed(&seed_json)?;
    info!("Quest [SEED] '{}' | location: {} | artifact: {} | quest-giver: {}",
          seed.title, seed.tomb_name, seed.artifact_name, seed.quest_giver_name);
    let rumors_json = try_json(cfg, &build_rumors_prompt(&tpl.rumors, &seed), "RUMORS",
                                |ev| validate_rumors(ev, &seed))?;
    let rumors = rumors_from_json(&rumors_json)?;
    let npc_flavors = generate_npc_flavors(cfg, tpl, &seed, npc_flats);
    Ok(Quest { city: job.city, city_name: job.city_name.clone(), seed, rumors, npc_flavors })
}

fn generate_voice(cfg: &QuestConfig, tpl: &Templates, seed: &Seed, npc: &str) -> Result<String, String> {
    let prompt = build_npc_voice_prompt(&tpl.voice, seed, npc);
    let ev = try_json(cfg, &prompt, "NPC_VOICE", |ev| validate_voice(ev, seed))?;
    let text = ev.get("text").and_then(Json::as_str).unwrap_or("").trim();
    Ok(text.trim_matches('"').trim().to_string())
}

fn generate_answer(cfg: &QuestConfig, tpl: &Templates, job: &AskJob) -> Result<String, String> {
    let prompt = build_npc_ask_prompt(&tpl.ask, &job.city_name, &job.description, &job.question, &job.history);
    let ev = try_json(cfg, &prompt, "NPC_ASK", validate_answer)?;
    let text = ev.get("text").and_then(Json::as_str).unwrap_or("").trim();
    Ok(text.trim_matches('"').trim().to_string())
}

fn append_quest(path: &Path, quest: &Quest) -> std::io::Result<()> {
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{}", quest.to_json().compact())
}

fn append_voice(path: &Path, city: (i32, i32), key: &str, description: &str, text: &str) -> std::io::Result<()> {
    let line = Json::Obj(vec![
        ("city_x".into(), Json::Num(city.0 as f64)),
        ("city_y".into(), Json::Num(city.1 as f64)),
        ("key".into(), jstr(key)),
        ("description".into(), jstr(description)),
        ("text".into(), jstr(text)),
    ]).compact();
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{line}")
}

// =============================================================================
// Thread di lavoro
// =============================================================================

enum QuestMsg {
    Done(Quest),
    Failed { city: (i32, i32), reason: String },
    Voice { city: (i32, i32), key: String, text: String },
    VoiceFailed { city: (i32, i32), key: String },
    Answer { id: u64, text: String },
    AnswerFailed { id: u64 },
}

fn run_voice(cfg: &QuestConfig, tpl: &Templates, server: &SharedServer,
              voices_path: &Path, v: &VoiceJob) -> QuestMsg
{
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

fn run_ask(cfg: &QuestConfig, tpl: &Templates, server: &SharedServer, a: &AskJob) -> QuestMsg {
    let started = Instant::now();
    match ensure_server(cfg, server).and_then(|_| generate_answer(cfg, tpl, a)) {
        Ok(text) => {
            info!("Quest: answer #{} ready in {:?} ({} turns)", a.id, started.elapsed(), a.history.len() / 2);
            QuestMsg::Answer { id: a.id, text }
        }
        Err(reason) => {
            warn!("Quest: answer #{}: {reason}", a.id);
            QuestMsg::AnswerFailed { id: a.id }
        }
    }
}

#[cfg(test)]
fn worker_main(cfg: QuestConfig, tpl: Templates, quests_path: PathBuf,
                jobs: Receiver<Job>, results: Sender<QuestMsg>, server: SharedServer)
{
    let (_voice_tx, voice_rx) = mpsc::channel();
    let (_ask_tx, ask_rx) = mpsc::channel();
    worker_loop(cfg, tpl, quests_path, PathBuf::from("quest_npc_voices.test.txt"),
                jobs, voice_rx, ask_rx, results, server);
}

#[allow(clippy::too_many_arguments)]
fn worker_loop(cfg: QuestConfig, tpl: Templates, quests_path: PathBuf, voices_path: PathBuf,
                jobs: Receiver<Job>, voice_jobs: Receiver<VoiceJob>, ask_jobs: Receiver<AskJob>,
                results: Sender<QuestMsg>, server: SharedServer)
{
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
                        append_quest(&quests_path, &quest)
                            .map_err(|e| format!("writing {}: {e}", quests_path.display()))?;
                        Ok(quest)
                    });
                let msg = match outcome {
                    Ok(quest) => {
                        info!("Quest for {} generated in {:?} ({} NPC flavors)",
                              job.city_name, started.elapsed(), quest.npc_flavors.len());
                        QuestMsg::Done(quest)
                    }
                    Err(reason) => {
                        warn!("Quest for {}: {reason}", job.city_name);
                        QuestMsg::Failed { city: job.city, reason }
                    }
                };
                if results.send(msg).is_err() { break; }
                last_work = Instant::now();
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if let Ok(a) = ask_jobs.try_recv() {
            let msg = run_ask(&cfg, &tpl, &server, &a);
            if results.send(msg).is_err() { break; }
            last_work = Instant::now();
        } else if let Ok(v) = voice_jobs.try_recv() {
            let msg = run_voice(&cfg, &tpl, &server, &voices_path, &v);
            if results.send(msg).is_err() { break; }
            last_work = Instant::now();
        }
        if last_work.elapsed() >= idle { stop_server(&server); last_work = Instant::now(); }
    }
    stop_server(&server);
}

// =============================================================================
// Plugin
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
    pub fn ask(&self, city_name: &str, description: &str, question: &str,
               history: Vec<(String, String)>) -> Option<u64>
    {
        let question = clean_question(question)?;
        let id = self.next_ask.fetch_add(1, Ordering::Relaxed);
        let max = MAX_HISTORY_TURNS * 2;
        let history = if history.len() > max { history[history.len() - max..].to_vec() } else { history };
        let job = AskJob { id, city_name: city_name.to_string(), description: description.to_string(),
                           question, history };
        self.ask_jobs.send(job).is_ok().then_some(id)
    }

    pub fn request_voice(&self, book: &mut QuestBook, city: (i32, i32), key: &str, description: &str) -> bool {
        let Some(seed) = book.quest(city).map(|q| q.seed.clone()) else { return false };
        if !matches!(book.voice_state(city, key), VoiceState::Unknown) { return false; }
        let job = VoiceJob { city, key: key.to_string(), description: description.to_string(), seed };
        if self.voice_jobs.send(job).is_ok() {
            book.voices_pending.insert((city, key.to_string()));
            true
        } else { false }
    }
}

impl Drop for QuestWorker {
    fn drop(&mut self) { stop_server(&self.server); }
}

fn read_prompt(file: &str, default: &str) -> String {
    match std::fs::read_to_string(file) {
        Ok(t) if !t.trim().is_empty() => { info!("Quest: using prompt from {file}"); t }
        _ => default.to_string(),
    }
}

pub struct QuestPlugin;

impl Plugin for QuestPlugin {
    fn build(&self, app: &mut App) {
        let cfg = QuestConfig::load_or_create(Path::new(CONFIG_FILE));
        let mut book = QuestBook::load(Path::new(QUESTS_FILE));
        book.load_voices(Path::new(VOICES_FILE));
        info!("Quest: {} already saved in {QUESTS_FILE} (+{} NPC voices), generator {}",
              book.len(), book.voice_count(), if cfg.enabled { "active" } else { "disabled" });
        app.insert_resource(book);
        if !cfg.enabled { return; }
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
        let spawned = std::thread::Builder::new().name("quest-llm".into()).spawn(move || {
            worker_loop(cfg, tpl, PathBuf::from(QUESTS_FILE), PathBuf::from(VOICES_FILE),
                        job_rx, voice_rx, ask_rx, res_tx, thread_server)
        });
        if let Err(e) = spawned { warn!("Quest: cannot start generator thread: {e}"); return; }
        app.insert_resource(QuestWorker {
            jobs: job_tx, voice_jobs: voice_tx, ask_jobs: ask_tx,
            next_ask: AtomicU64::new(1),
            results: Mutex::new(res_rx), server,
        })
        .add_systems(Update, (quest_trigger, quest_poll));
    }
}

fn quest_trigger(data: Res<GameData>, origin: Res<Origin>, player: Query<&Transform, With<Player>>,
                  worker: Res<QuestWorker>, mut book: ResMut<QuestBook>, mut last: Local<Option<(i32, i32)>>)
{
    let Ok(tf) = player.get_single() else { return };
    let here = map_pixel_of(&origin, tf.translation);
    if *last == Some(here) { return; }
    *last = Some(here);
    let Some(loc) = data.locations.get(&here) else { return };
    if !is_town(loc) || book.knows(here) { return; }
    let job = Job {
        city: here, city_name: loc.name.clone(), theme: theme_for(here),
        location: loc.clone(), blocks_buf: Arc::clone(&data.blocks_buf), arena_dir: data.dir.clone(),
    };
    if worker.jobs.send(job).is_ok() {
        info!("Quest: generating quest for {} (map pixel {}, {})", loc.name, here.0, here.1);
        book.mark_pending(here);
    }
}

fn quest_poll(worker: Res<QuestWorker>, mut book: ResMut<QuestBook>) {
    let Ok(results) = worker.results.try_lock() else { return };
    while let Ok(msg) = results.try_recv() {
        match msg {
            QuestMsg::Done(quest) => {
                book.clear_pending(quest.city);
                book.insert_quest(quest);
            }
            QuestMsg::Failed { city, reason } => {
                warn!("Quest: città {city:?} fallita: {reason}");
                book.mark_failed(city);
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
    use crate::quests::{jstr, Seed, Tone};
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU32, AtomicU64};

    fn test_cfg(port: u16) -> QuestConfig {
        QuestConfig { port, server_bin: "/does/not/exist/llama-server".into(),
                      idle_stop_secs: 1, ..QuestConfig::default() }
    }

    fn tpl() -> Templates {
        Templates {
            seed: DEFAULT_SEED_PROMPT.to_string(), rumors: DEFAULT_RUMORS_PROMPT.to_string(),
            npc_flavor: DEFAULT_NPC_FLAVOR_PROMPT.to_string(),
            voice: DEFAULT_NPC_VOICE_PROMPT.to_string(), ask: DEFAULT_NPC_ASK_PROMPT.to_string(),
        }
    }

    fn good_seed_json() -> Json {
        Json::parse(
            r#"{"theme":"artifact_discovery","title":"The Whisper of Vael","dungeon_id":"dgf_crypt_ash",
            "quest_giver_id":"mages_guild","tomb_name":"Nekhar-Zul","artifact_name":"Eye of Vael",
            "difficulty":3,"lore":"An ancient glass eye sleeps beneath the crypt."}"#,
        ).unwrap()
    }

    fn seed_reply() -> String { format!("```json\n{}\n```", good_seed_json().compact()) }

    fn rumors_reply() -> String {
        let texts: [&str; 5] = [
            "Your Grace, it is said the Mages Guild watches over Nekhar-Zul and the Eye of Vael.",
            "The roads to Nekhar-Zul are a couple of hours out, and prices are rising.",
            "I saw the mages leave for those ruins. They seek the Eye of Vael.",
            "They say there's something pulsing in the crypt outside town. It scares me.",
            "Dunno, Nekhar... Nekhal? Ten or twenty kilometers, let's drink!",
        ];
        let items: Vec<Json> = Tone::ALL.iter().zip(texts)
            .map(|(t, text)| Json::Obj(vec![("tone".into(), jstr(t.key())), ("text".into(), jstr(text))]))
            .collect();
        Json::Obj(vec![("rumors".into(), Json::Arr(items))]).compact()
    }

    fn empty_location() -> DFLocation {
        DFLocation { name: "X".into(), region_index: 0, world_x: 0, world_y: 0,
                     width: 0, height: 0, block_names: Vec::new(), letter1_for_rmb_name: None }
    }

    #[test]
    fn extract_json_handles_fences_and_thinking() {
        assert_eq!(extract_json("<think>x</think>```json\n{\"a\":1}\n```"), "{\"a\":1}");
        assert_eq!(extract_json("Here: {\"a\": 1} end"), "{\"a\": 1}");
    }

    #[test]
    fn http_responses_are_parsed() {
        let plain = b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\n\r\n{\"a\":1}";
        assert_eq!(parse_http_response(plain).unwrap(), (200, "{\"a\":1}".to_string()));
    }

    struct FakeServer { port: u16, bodies: Arc<Mutex<Vec<String>>> }

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
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" { break; }
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
                        let json = Json::Obj(vec![("choices".into(),
                            Json::Arr(vec![Json::Obj(vec![
                                ("message".into(), Json::Obj(vec![("content".into(), jstr(&content))])),
                                ("finish_reason".into(), jstr("stop")),
                            ])]))]);
                        (200, json.compact())
                    } else { (st, content) }
                };
                let response = format!(
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len());
                stream.write_all(response.as_bytes()).ok();
            }
        });
        FakeServer { port, bodies }
    }

    #[test]
    fn llm_call_reads_content() {
        let srv = fake_server(vec![(200, "hello".into())]);
        assert_eq!(call_llm(&test_cfg(srv.port), "test").unwrap(), "hello");
    }

    #[test]
    fn quest_is_generated_with_retries() {
        let srv = fake_server(vec![
            (200, "<think>hmm</think> no json".into()),
            (200, seed_reply()),
            (200, rumors_reply()),
        ]);
        let job = Job {
            city: (207, 213), city_name: "Daggerfall".into(), theme: "rescue",
            location: empty_location(), blocks_buf: Arc::new(Vec::new()),
            arena_dir: PathBuf::from("/tmp"),
        };
        let q = generate_quest(&test_cfg(srv.port), &tpl(), &job, &[]).unwrap();
        assert_eq!(q.seed.tomb_name, "Nekhar-Zul");
        assert_eq!(q.rumors.len(), 5);
    }

    #[test]
    fn missing_binary_fails_cleanly() {
        let cfg = test_cfg(1);
        let err = ensure_server(&cfg, &Arc::new(Mutex::new(None))).unwrap_err();
        assert!(err.contains("llama-server not found"), "{err}");
    }

    #[test]
    fn server_args_match_script() {
        let cfg = QuestConfig { model_path: "/m/x.gguf".into(), ..QuestConfig::default() };
        assert!(server_args(&cfg, Some(99)).join(" ").contains("-ngl 99"));
    }

    #[test]
    fn ngl_falls_back_after_oom() {
        assert_eq!(ngl_candidates(None, None), vec![None, Some(32), Some(16), Some(8), Some(0)]);
    }

    #[test]
    fn worker_generates_saves() {
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
        job_tx.send(Job {
            city: (207, 213), city_name: "Daggerfall".into(), theme: "escort",
            location: empty_location(), blocks_buf: Arc::new(Vec::new()), arena_dir: dir.clone(),
        }).unwrap();
        match res_rx.recv_timeout(Duration::from_secs(20)).unwrap() {
            QuestMsg::Done(q) => assert_eq!(q.rumors.len(), 5),
            _ => panic!("doveva riuscire"),
        }
        assert_eq!(QuestBook::load(&path).len(), 1);
        drop(job_tx);
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
