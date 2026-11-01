//! Descrittori degli NPC "classici" (flat di DFU, volti generici, passanti).
//!
//! Funziona come per gli NPC custom (`npc_*.txt`): la prima volta che il giocatore
//! parla con un NPC classico, in `textures_dir` viene creato un file base con nome,
//! descrizione, voce e posizione. Da quel momento il file e' la fonte di verita':
//! se lo modifichi, il dialogo usa i tuoi dati (riletto a ogni dialogo, non serve
//! riavviare). Il file non viene mai sovrascritto.
//!
//! ## Nomi dei file
//!
//! Le immagini hanno il nome della texture, quindi sono condivise da tutti gli NPC
//! che la usano (una sola copia, anche se entri in cento taverne):
//!
//! ```text
//! 182_3-0.png                 sprite del flat 182:3 (idle base)
//! tfac00i0.rci_23-0.png       ritratto del dialogo (volto 23)
//! 182_3-0_walk_0.png          (facoltativo) frame di camminata, come per i custom
//! 182_3-0_idle_0.png          (facoltativo) frame idle / mood, come per i custom
//! ```
//!
//! I txt sono uno per NPC (citta' + tipo); quelli che condividono la texture si
//! distinguono con un numero:
//!
//! ```text
//! 182_3-0.txt   182_3-0_1.txt   182_3-0_2.txt ...
//!
//! Blonde woman                              <- nome (etichetta nel dialogo)
//! id = 207,213 flat:182:3                   <- identita' (citta' + chiave): non toccare
//! description = "..."                       <- sostituisce la descrizione automatica
//! voice = af_kore:0.9,af_sky:0.1            <- vuoto = voce automatica
//! position = Tavern                         <- solo informativa
//! walk_fps = 8.0                            <- facoltativo, come per i custom
//! idle_fps = 4.0                            <- facoltativo, come per i custom
//! ```
//!
//! Identita': come per le voci in `QuestBook`, un NPC classico e' la coppia
//! (citta', chiave) dove la chiave e' `flat:A:R`, `face:N` o `walker:N`.
//! Un campo vuoto o mancante ripiega sul valore automatico.
//!
//! ## Prefetch (read-only) e profilo LLM (write-once)
//!
//! Il modulo espone due funzioni che **non** creano file solo per leggere:
//!
//! - `resolve_existing`: legge il `.txt` dell'NPC **se esiste già** e ritorna
//!   l'override, altrimenti `ClassicOverride::default()`. Usata dai sistemi di
//!   prefetch (le voci degli NPC in `interior.rs`) che non devono creare
//!   file/PNG solo per leggere una descrizione.
//!
//! - `apply_llm_background`: scrive nel `.txt` il **nome proprio** e il
//!   **background** generati dall'LLM (in `quest_llm.rs` → `NPC_VOICE`), ma
//!   **solo se il file è ancora "auto"** (prima riga == descrittore visivo,
//!   description che inizia con "Appearance (visual descriptor"). Se l'utente
//!   ha personalizzato nome o description, il file non viene toccato.
//!
//! Il flusso completo:
//!
//! 1. entri in un interno → `interior_enter_exit` emette `InteriorEntered` con
//!    `PendingVoice` per ogni NPC (voice_key, description auto, original_label,
//!    sprite_stem);
//! 2. `prefetch_interior_voices` chiama `QuestWorker::request_voice` per ognuno;
//! 3. `NPC_VOICE` produce `{name, description, text}` in un solo giro;
//! 4. `quest_poll` salva in `QuestBook` con `store_voice_full` **e** chiama
//!    `apply_llm_background` per fissare nome+background nel `.txt`;
//! 5. al riavvio `QuestBook::load_voices` rilegge `name` e `background`, e
//!    `classic_npc::resolve` (via `dialog.rs`) rilegge la `description` dal
//!    `.txt`: nessuna chiamata LLM.
use crate::data::city_map::PoiKind;
use crate::interior::parse_custom_txt;
use crate::mod_textures::ModTextures;
use bevy::math::Affine2;
use bevy::prelude::*;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Dati automatici dell'NPC con cui parla il giocatore.
pub struct ClassicNpc<'a> {
    pub city: (i32, i32),
    pub city_name: &'a str,
    /// `flat:A:R`, `face:N` o `walker:N`.
    pub key: &'a str,
    /// Etichetta automatica (quella mostrata nel dialogo senza file).
    pub label: &'a str,
    /// Descrizione automatica (output multi-riga di `describe_npc`).
    pub description: &'a str,
    pub place: Option<PoiKind>,
    /// Nome della texture dello sprite (es. `182_3-0`): nome di png e txt.
    pub sprite_stem: String,
    /// Nome della texture del ritratto (es. `tfac00i0.rci_23-0`).
    pub portrait_stem: Option<String>,
    /// Sprite intero e flag "righe capovolte" (come `ModTextures::needs_v_flip`).
    pub sprite: Option<(Handle<Image>, bool)>,
    /// Ritratto mostrato nel dialogo e flag "righe capovolte" (`is_hd`).
    pub portrait: Option<(Handle<Image>, bool)>,
}

/// Valori letti dal file; `None` = usa quello automatico.
#[derive(Debug, Default, PartialEq)]
pub struct ClassicOverride {
    pub name: Option<String>,
    pub description: Option<String>,
    pub voice: Option<String>,
    /// Solo se presenti nel txt.
    pub walk_fps: Option<f32>,
    pub idle_fps: Option<f32>,
}

fn parse_fps(content: &str, key: &str) -> Option<f32> {
    for line in content.lines() {
        let Some(rest) = line.trim().strip_prefix(key) else { continue };
        let Some(value) = rest.trim_start().strip_prefix('=') else { continue };
        if let Ok(n) = value.trim().parse::<f32>() {
            return Some(n.max(0.1));
        }
    }
    None
}

/// Identita' scritta nel txt: `207,213 flat:182:3`.
fn npc_id(npc: &ClassicNpc) -> String {
    format!("{},{} {}", npc.city.0, npc.city.1, npc.key)
}

fn file_id(content: &str) -> Option<&str> {
    content.lines().find_map(|l| {
        let rest = l.trim().strip_prefix("id")?.trim_start().strip_prefix('=')?;
        Some(rest.trim())
    })
}

/// `<stem>.txt` -> 0, `<stem>_N.txt` -> N (confronto senza maiuscole).
fn txt_index(file_name: &str, stem: &str) -> Option<usize> {
    let lower = file_name.to_ascii_lowercase();
    let rest = lower.strip_suffix(".txt")?.strip_prefix(&stem.to_ascii_lowercase())?;
    if rest.is_empty() {
        return Some(0);
    }
    let n = rest.strip_prefix('_')?;
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    n.parse().ok()
}

fn txt_name(stem: &str, n: usize) -> String {
    if n == 0 { format!("{stem}.txt") } else { format!("{stem}_{n}.txt") }
}

enum Lookup {
    Found(String),
    Free(PathBuf),
}

/// Cerca il txt con la nostra identita' tra quelli che condividono la texture;
/// se non c'e', indica il primo nome libero.
fn lookup(dir: &Path, npc: &ClassicNpc) -> Lookup {
    let id = npc_id(npc);
    let mut used: HashSet<usize> = HashSet::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(n) = txt_index(name, &npc.sprite_stem) else { continue };
            used.insert(n);
            if let Ok(content) = std::fs::read_to_string(entry.path()) {
                if file_id(&content) == Some(id.as_str()) {
                    return Lookup::Found(content);
                }
            }
        }
    }
    let n = (0..).find(|n| !used.contains(n)).unwrap_or(0);
    Lookup::Free(dir.join(txt_name(&npc.sprite_stem, n)))
}

fn position_word(place: Option<PoiKind>) -> &'static str {
    match place {
        Some(PoiKind::Tavern) => "Tavern",
        Some(PoiKind::Shop) => "Shop",
        Some(PoiKind::Guild) => "Guild",
        Some(PoiKind::Palace) => "Palace",
        _ => "Any",
    }
}

/// `describe_npc` produce piu' righe ("- Campo: valore"); nel file serve una riga sola.
fn one_line(description: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for line in description.lines() {
        let line = line.trim();
        let line = line.strip_prefix("- ").unwrap_or(line).trim();
        if line.is_empty() {
            continue;
        }
        let mut s = line.replace('"', "'");
        if !s.ends_with(['.', '!', '?']) {
            s.push('.');
        }
        parts.push(s);
    }
    parts.join(" ")
}

fn base_text(npc: &ClassicNpc) -> String {
    let name = npc.label.lines().next().unwrap_or("").trim();
    let name = if name.is_empty() { "NPC" } else { name };
    let portrait = npc.portrait_stem.as_deref().unwrap_or("-");
    format!(
        "{name}\n\
# NPC classico: file generato automaticamente, modificalo a piacere (non verra' sovrascritto).\n\
# Citta': {city} ({x}, {y})  |  chiave: {key}\n\
# id: identifica questo NPC (citta' + tipo), non modificarlo.\n\
# La prima riga e' il nome mostrato nel dialogo.\n\
# description: sostituisce la descrizione automatica nel prompt dell'LLM (una riga sola).\n\
# voice: es. af_kore:0.9,af_sky:0.1 (vuoto = voce automatica).\n\
# position: solo informativa (dove l'NPC e' stato incontrato), non sposta l'NPC.\n\
# Immagini condivise: {sprite}.png (sprite), {portrait}.png (ritratto).\n\
# Animazioni facoltative (condivise): {sprite}_walk_0.png, {sprite}_walk_1.png, ...\n\
#   e {sprite}_idle_0.png, {sprite}_idle_1.png, ... (mood = frame idle).\n\
# Velocita' (facoltative): togli il # davanti a walk_fps / idle_fps.\n\
id = {id}\n\
description = \"{desc}\"\n\
voice =\n\
position = {pos}\n\
# walk_fps = 8.0\n\
# idle_fps = 4.0\n",
city = if npc.city_name.is_empty() { "?" } else { npc.city_name },
            x = npc.city.0,
            y = npc.city.1,
            key = npc.key,
            sprite = npc.sprite_stem,
            id = npc_id(npc),
            desc = one_line(npc.description),
            pos = position_word(npc.place),
    )
}

fn save_one(path: PathBuf, img: &Option<(Handle<Image>, bool)>, images: &Assets<Image>, mods: &ModTextures) {
    if path.exists() {
        return;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
    let Some((handle, flip_y)) = img else {
        warn!("NPC classico: {name} non salvata: nessuna immagine disponibile per questo NPC");
        return;
    };
    // 1) texture di mod: ri-decodifica dal container (RGBA8 top-down, niente flip),
    //    perche' in Assets<Image> la copia CPU puo' mancare o essere BCn.
    if let Some((w, h, rgba)) = mods.export_rgba(handle) {
        match std::fs::write(&path, encode_png_rgba(w, h, &rgba)) {
            Ok(()) => info!("NPC classico: salvata {}", path.display()),
            Err(e) => warn!("NPC classico: impossibile scrivere {}: {e}", path.display()),
        }
        return;
    }
    // 2) texture vanilla/procedurali: pixel RGBA8 in Assets<Image>.
    let Some(image) = images.get(handle) else {
        warn!("NPC classico: {name} non salvata: l'immagine non e' in Assets<Image> (probabilmente caricata solo per la GPU)");
        return;
    };
    match rgba_png(image, *flip_y) {
        Ok(png) => match std::fs::write(&path, png) {
            Ok(()) => info!("NPC classico: salvata {}", path.display()),
            Err(e) => warn!("NPC classico: impossibile scrivere {}: {e}", path.display()),
        },
        Err(why) => warn!("NPC classico: {name} non salvata: {why}"),
    }
}

/// Salva `<texture>.png` (sprite) e `<texture-ritratto>.png` solo se mancano:
/// i nomi sono quelli delle texture, quindi non si duplicano mai e se li modifichi
/// non vengono sovrascritti.
fn save_images(dir: &Path, npc: &ClassicNpc, images: &Assets<Image>, mods: &ModTextures) {
    save_one(dir.join(format!("{}.png", npc.sprite_stem)), &npc.sprite, images, mods);
    if let Some(stem) = &npc.portrait_stem {
        save_one(dir.join(format!("{stem}.png")), &npc.portrait, images, mods);
    }
}

/// RGBA8 (o BGRA8) -> PNG; `Err(motivo)` se il formato non e' supportato o i pixel non sono in CPU.
fn rgba_png(image: &Image, flip_y: bool) -> Result<Vec<u8>, String> {
    use bevy::render::render_resource::TextureFormat as F;
    let (w, h) = (image.width() as usize, image.height() as usize);
    let format = image.texture_descriptor.format;
    let swap = match format {
        F::Rgba8UnormSrgb | F::Rgba8Unorm => false,
        F::Bgra8UnormSrgb | F::Bgra8Unorm => true,
        other => return Err(format!("formato {other:?} non supportato")),
    };
    let data = &image.data;
    if w == 0 || h == 0 || data.len() != w * h * 4 {
        return Err(format!("pixel non disponibili in CPU ({w}x{h}, {} byte, formato {format:?})", data.len()));
    }
    let mut rgba = Vec::with_capacity(data.len());
    for y in 0..h {
        let sy = if flip_y { h - 1 - y } else { y };
        let row = &data[sy * w * 4..(sy + 1) * w * 4];
        if swap {
            for px in row.chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        } else {
            rgba.extend_from_slice(row);
        }
    }
    Ok(encode_png_rgba(w as u32, h as u32, &rgba))
}

fn crc32(chunk_type: &[u8], data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in chunk_type.iter().chain(data) {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(kind, data).to_be_bytes());
}

/// PNG RGBA8 senza dipendenze (deflate "stored", nessuna compressione).
fn encode_png_rgba(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let stride = w as usize * 4;
    let mut raw = Vec::with_capacity((stride + 1) * h as usize);
    for row in rgba.chunks_exact(stride) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65535).peekable();
    if blocks.peek().is_none() {
        z.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(b) = blocks.next() {
        z.push(u8::from(blocks.peek().is_none()));
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    let (mut a, mut s) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65521;
        s = (s + a) % 65521;
    }
    z.extend_from_slice(&((s << 16) | a).to_be_bytes());

    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    push_chunk(&mut out, b"IHDR", &ihdr);
    push_chunk(&mut out, b"IDAT", &z);
    push_chunk(&mut out, b"IEND", &[]);
    out
}

fn parse(content: &str) -> ClassicOverride {
    let (name, description, voice, _position) = parse_custom_txt(content);
    ClassicOverride {
        name: (!name.is_empty()).then_some(name),
        description,
        voice,
        walk_fps: parse_fps(content, "walk_fps"),
        idle_fps: parse_fps(content, "idle_fps"),
    }
}

/// Legge il file dell'NPC se esiste, altrimenti lo crea con i valori automatici
/// (e salva le immagini mancanti). Non fallisce mai: se la cartella manca o non e'
/// scrivibile ripiega sul comportamento automatico.
pub fn resolve(dir: &Path, npc: &ClassicNpc, images: &Assets<Image>, mods: &ModTextures) -> ClassicOverride {
    if !dir.is_dir() {
        return ClassicOverride::default();
    }
    save_images(dir, npc, images, mods);
    match lookup(dir, npc) {
        Lookup::Found(content) => parse(&content),
        Lookup::Free(path) => {
            let written = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut f| f.write_all(base_text(npc).as_bytes()));
            match written {
                Ok(()) => info!("NPC classico: creato {}", path.display()),
                Err(e) => warn!("NPC classico: impossibile creare {}: {e}", path.display()),
            }
            ClassicOverride::default()
        }
    }
}

/// Legge il file dell'NPC **se esiste già**, senza crearlo né esportare immagini.
///
/// A differenza di `resolve`, non ha effetti collaterali: se il txt manca
/// restituisce `ClassicOverride::default()` senza scrivere nulla. Usata dai
/// sistemi di prefetch (es. le voci degli NPC in background, in `interior.rs`)
/// che non devono creare file solo per leggere una descrizione.
///
/// Costruisce un `ClassicNpc` minimo: `lookup` usa solo `city`, `key` e
/// `sprite_stem`; gli altri campi (`label`, `description`, `place`, ...) non
/// servono per individuare il file e restano vuoti. `parse` legge solo i
/// campi del txt e ignora il resto, quindi il `ClassicNpc` "finto" non
/// inquina il risultato.
pub fn resolve_existing(
    dir: &Path,
    city: (i32, i32),
                        key: &str,
                        sprite_stem: &str,
) -> ClassicOverride {
    if !dir.is_dir() {
        return ClassicOverride::default();
    }
    let npc = ClassicNpc {
        city,
        city_name: "",
        key,
        label: "",
        description: "",
        place: None,
        sprite_stem: sprite_stem.to_string(),
        portrait_stem: None,
        sprite: None,
        portrait: None,
    };
    match lookup(dir, &npc) {
        Lookup::Found(content) => parse(&content),
        // Nessun txt esistente: nessun override, niente scritture.
        Lookup::Free(_) => ClassicOverride::default(),
    }
}

// =============================================================================
// Animazioni condivise (walk / idle) degli NPC classici
// =============================================================================

/// Velocita' di default se il txt non ha `walk_fps` / `idle_fps` (come per i custom).
pub const DEFAULT_WALK_FPS: f32 = 8.0;
pub const DEFAULT_IDLE_FPS: f32 = 4.0;

/// Frame di animazione di un NPC classico, pronti da assegnare al follower.
/// Le immagini hanno il nome della texture, quindi sono condivise da tutti gli
/// NPC che usano lo stesso sprite (`182_11-0_walk_0.png`, `_walk_1.png`, ...).
pub struct ClassicAnim {
    pub walk: Vec<Handle<StandardMaterial>>,
    pub walk_fps: f32,
    pub idle: Vec<Handle<StandardMaterial>>,
    pub idle_fps: f32,
}

/// Nomi (senza estensione) dei frame `<sprite>_<kind>_0.png`, `_1`, ... presenti
/// in `dir`, finche' sono consecutivi (si ferma al primo buco). `kind` e' `walk`
/// o `idle`. Il confronto ignora le maiuscole, ma si ritorna il nome REALE del
/// file, cosi' il caricamento funziona anche su Linux (filesystem case-sensitive).
pub fn frame_stems(dir: &Path, sprite_stem: &str, kind: &str) -> Vec<String> {
    let prefix = format!("{sprite_stem}_{kind}_").to_ascii_lowercase();
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for entry in rd.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let lower = name.to_ascii_lowercase();
        if !lower.starts_with(&prefix) {
            continue;
        }
        let Some(stem) = lower.strip_suffix(".png") else { continue };
        // ".png" e' ASCII: la stessa lunghezza vale per il nome originale.
        found.insert(stem.to_string(), name[..name.len() - 4].to_string());
    }
    let mut out = Vec::new();
    for i in 0.. {
        match found.get(&format!("{prefix}{i}")) {
            Some(real) => out.push(real.clone()),
            None => break,
        }
    }
    out
}

/// Un materiale per ogni frame (stessa costruzione di `load_custom_walk_frames`).
/// `None` se manca anche un solo frame: un'animazione a meta' sarebbe peggio di nessuna.
fn frame_materials(
    stems: &[String],
    what: &str,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<Vec<Handle<StandardMaterial>>> {
    let mut frames = Vec::with_capacity(stems.len());
    for stem in stems {
        let Some(handle) = mods.image_by_name(stem, images) else {
            warn!("NPC classico: impossibile caricare il frame {what} '{stem}'");
            return None;
        };
        let uv = if mods.needs_v_flip(&handle) {
            Affine2::from_scale_angle_translation(Vec2::new(1.0, -1.0), 0.0, Vec2::new(0.0, 1.0))
        } else {
            Affine2::IDENTITY
        };
        frames.push(materials.add(StandardMaterial {
            base_color_texture: Some(handle),
            uv_transform: uv,
            alpha_mode: AlphaMode::Mask(0.5),
            cull_mode: None,
            double_sided: true,
            perceptual_roughness: 0.9,
            reflectance: 0.1,
            ..default()
        }));
    }
    Some(frames)
}

/// Carica le animazioni walk/idle di un NPC classico dalla cartella delle texture.
///
/// - `sprite_stem`: nome della texture dello sprite (es. `182_11-0`), lo stesso
///   usato per `ClassicNpc::sprite_stem`;
/// - `ov`: override letto dal txt (`resolve_existing`) per `walk_fps` / `idle_fps`;
///   con `&ClassicOverride::default()` si usano i valori di default.
///
/// Ritorna `None` se non esiste nessun frame (l'NPC resta statico come prima).
pub fn load_anim(
    dir: &Path,
    sprite_stem: &str,
    ov: &ClassicOverride,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
) -> Option<ClassicAnim> {
    let walk_stems = frame_stems(dir, sprite_stem, "walk");
    let idle_stems = frame_stems(dir, sprite_stem, "idle");
    if walk_stems.is_empty() && idle_stems.is_empty() {
        return None;
    }
    let walk = if walk_stems.is_empty() {
        Vec::new()
    } else {
        frame_materials(&walk_stems, "walk", mods, images, materials).unwrap_or_default()
    };
    let idle = if idle_stems.is_empty() {
        Vec::new()
    } else {
        frame_materials(&idle_stems, "idle", mods, images, materials).unwrap_or_default()
    };
    if walk.is_empty() && idle.is_empty() {
        return None;
    }
    info!(
        "NPC classico '{sprite_stem}': {} frame walk, {} frame idle",
        walk.len(),
        idle.len()
    );
    Some(ClassicAnim {
        walk,
        walk_fps: ov.walk_fps.unwrap_or(DEFAULT_WALK_FPS),
        idle,
        idle_fps: ov.idle_fps.unwrap_or(DEFAULT_IDLE_FPS),
    })
}

// =============================================================================
// Profilo LLM (nome + background + voce)
// =============================================================================

/// Cerca, tra i `.txt` che condividono la `sprite_stem`, quello che contiene
/// l'`id` di (city, key). Ritorna il percorso se lo trova.
///
/// È la controparte "path-aware" di `lookup`: `lookup` restituisce il contenuto
/// (usato da `resolve`/`resolve_existing`), questa funzione il percorso (usato
/// da `apply_llm_background` per riscrivere il file).
fn find_existing_path(
    dir: &Path,
    city: (i32, i32),
                      key: &str,
                      sprite_stem: &str,
) -> Option<PathBuf> {
    let id = format!("{},{} {}", city.0, city.1, key);
    let Ok(rd) = std::fs::read_dir(dir) else { return None };
    for entry in rd.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(_) = txt_index(name, sprite_stem) else { continue };
        if let Ok(content) = std::fs::read_to_string(entry.path()) {
            if file_id(&content) == Some(id.as_str()) {
                return Some(entry.path());
            }
        }
    }
    None
}

/// Primo nome libero `<stem>.txt` / `<stem>_N.txt`, senza aprire i file.
fn next_free_path(dir: &Path, sprite_stem: &str) -> PathBuf {
    let mut used: HashSet<usize> = HashSet::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(n) = txt_index(name, sprite_stem) {
                used.insert(n);
            }
        }
    }
    let n = (0..).find(|n| !used.contains(n)).unwrap_or(0);
    dir.join(txt_name(sprite_stem, n))
}

/// Scrive nel `.txt` dell'NPC il nome proprio e il background generati
/// dall'LLM (`NPC_VOICE` → `{name, description, text}`), ma **solo se il
/// file è ancora "auto"**. Non sovrascrive mai un file personalizzato.
///
/// Regole:
/// - `.txt` assente → viene creato con `name` come prima riga e
///   `description = "<background>"`. Il resto del template (immagini,
///   posizione, ...) è quello di `base_text`.
/// - `.txt` esistente con prima riga == `original_label` (il descrittore
///   visivo auto) **e** `description` che inizia con `"Appearance (visual
///   descriptor"` (il prefisso che `describe_npc_in` produce sempre) →
///   viene riscritto: prima riga = `name`, description = `background`. Il
///   resto del file è preservato (commenti, `voice`, `position`, `walk_fps`,
///   `idle_fps` restano com'erano).
/// - In ogni altro caso (prima riga diversa da `original_label`, oppure
///   description che non inizia col prefisso auto) → il file è considerato
///   personalizzato e non viene toccato.
///
/// `name` e `background` possono essere `None`: se lo sono entrambi, la
/// funzione non fa nulla. Se `name` è `None`, la prima riga resta
/// `original_label`; se `background` è `None`, la description resta vuota
/// (o invariata).
///
/// La funzione è **idempotente**: dopo aver scritto `name`, la prima riga
/// non coincide più con `original_label`, quindi una seconda chiamata non
/// tocca il file.
pub fn apply_llm_background(
    dir: &Path,
    city: (i32, i32),
                            key: &str,
                            sprite_stem: &str,
                            original_label: &str,
                            name: Option<&str>,
                            background: Option<&str>,
) {
    if !dir.is_dir() {
        return;
    }
    if name.is_none() && background.is_none() {
        return;
    }
    match find_existing_path(dir, city, key, sprite_stem) {
        // Nessun file esistente: lo crea con nome+background.
        None => {
            let new_name = name.unwrap_or(original_label);
            let new_desc = background.unwrap_or("");
            let npc = ClassicNpc {
                city,
                city_name: "",
                key,
                label: new_name,
                description: new_desc,
                place: None,
                sprite_stem: sprite_stem.to_string(),
                portrait_stem: None,
                sprite: None,
                portrait: None,
            };
            let body = base_text(&npc);
            let path = next_free_path(dir, sprite_stem);
            if let Err(e) = std::fs::write(&path, body) {
                warn!("NPC classico: impossibile scrivere {}: {e}", path.display());
            } else {
                info!(
                    "NPC classico: creato {} con nome+background LLM",
                    path.display()
                );
            }
        }
        // File esistente: capiamo se è ancora "auto".
        Some(path) => {
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => return,
            };

            // Nome: la prima riga "utile" (dopo i commenti, prima di `id =`).
            // Il template auto non quota il nome, quindi la riga è testo nudo.
            let current_name = content
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && !l.starts_with('#') && !l.contains('='))
            .unwrap_or("");
            let name_is_auto = current_name == original_label;

            // Description: consideriamo "auto" se assente, vuota, o se inizia
            // con il prefisso che `describe_npc_in` produce sempre
            // ("Appearance (visual descriptor from the game data, ...)").
            let ov = parse(&content);
            let desc_is_auto = match ov.description.as_deref() {
                None | Some("") => true,
                Some(d) => d.starts_with("Appearance (visual descriptor"),
            };

            if !name_is_auto || !desc_is_auto {
                debug!(
                    "NPC classico: {} è personalizzato (nome auto: {}, desc auto: {}), non applico il background LLM",
                       sprite_stem, name_is_auto, desc_is_auto
                );
                return;
            }

            let new_name = name.unwrap_or(original_label);
            let new_desc = background.unwrap_or("");

            // Riscrive solo le righe che ci interessano, preservando commenti
            // e altre righe (voice, position, walk_fps, idle_fps, ...).
            let mut out = String::new();
            let mut replaced_name = false;
            let mut replaced_desc = false;
            for line in content.lines() {
                let trimmed = line.trim();
                if !replaced_name
                    && !trimmed.is_empty()
                    && !trimmed.starts_with('#')
                    && !trimmed.contains('=')
                    {
                        // Prima riga "utile" dopo i commenti: è il nome.
                        out.push_str(new_name);
                        out.push('\n');
                        replaced_name = true;
                        continue;
                    }
                    if trimmed.starts_with("description") {
                        out.push_str(&format!(
                            "description = \"{}\"\n",
                            new_desc.replace('"', "'")
                        ));
                        replaced_desc = true;
                        continue;
                    }
                    out.push_str(line);
                    out.push('\n');
            }
            if !replaced_desc {
                // Non c'era: aggiungila in fondo.
                out.push_str(&format!(
                    "description = \"{}\"\n",
                    new_desc.replace('"', "'")
                ));
            }
            if let Err(e) = std::fs::write(&path, out) {
                warn!("NPC classico: impossibile riscrivere {}: {e}", path.display());
            } else {
                info!(
                    "NPC classico: applicato nome+background LLM a {}",
                    path.display()
                );
            }
        }
    }
}

/// Scrive nel `.txt` dell'NPC la voce TTS scelta dall'LLM (`NPC_VOICE` -> `voice`), ma solo se il
/// campo `voice` e' ancora vuoto: una voce impostata dall'utente non viene mai toccata.
///
/// Va chiamata subito dopo `apply_llm_background` (il file esiste gia'). `voice` deve essere gia'
/// validata (`quests::sanitize_voice`). Se il file non si trova non fa nulla: la voce resta
/// quella automatica.
pub fn apply_llm_voice(dir: &Path, city: (i32, i32), key: &str, sprite_stem: &str, voice: &str) {
    let voice = voice.trim();
    if voice.is_empty() || !dir.is_dir() {
        return;
    }
    let Some(path) = find_existing_path(dir, city, key, sprite_stem) else { return };
    let Ok(content) = std::fs::read_to_string(&path) else { return };
    let mut out = String::with_capacity(content.len() + voice.len() + 16);
    let mut done = false;
    for line in content.lines() {
        if !done {
            if let Some(rest) = line.trim().strip_prefix("voice") {
                if let Some(value) = rest.trim_start().strip_prefix('=') {
                    if !value.trim().trim_matches('"').trim().is_empty() {
                        return; // voce scelta dall'utente
                    }
                    out.push_str(&format!("voice = {voice}\n"));
                    done = true;
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if !done {
        out.push_str(&format!("voice = {voice}\n"));
    }
    match std::fs::write(&path, out) {
        Ok(()) => info!("NPC classico: voce '{voice}' applicata a {}", path.display()),
        Err(e) => warn!("NPC classico: impossibile riscrivere {}: {e}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESC: &str = "- Appearance (visual descriptor from the game data, not a name or title): Blonde woman\n\
- Occupation: You work here as a barmaid.\n- Location: inside a tavern\n- Temperament (not visible, shapes how they talk): gruff";

    fn npc<'a>(city: (i32, i32), name: &'a str) -> ClassicNpc<'a> {
        ClassicNpc {
            city,
            city_name: name,
            key: "flat:182:3",
            label: "Blonde woman",
            description: DESC,
            place: Some(PoiKind::Tavern),
            sprite_stem: "182_3-0".into(),
            portrait_stem: Some("tfac00i0.rci_23-0".into()),
            sprite: None,
            portrait: None,
        }
    }

    #[test]
    fn txt_index_matches_only_numbered_siblings() {
        assert_eq!(txt_index("182_3-0.txt", "182_3-0"), Some(0));
        assert_eq!(txt_index("182_3-0_2.txt", "182_3-0"), Some(2));
        assert_eq!(txt_index("182_3-0_walk_0.txt", "182_3-0"), None);
        assert_eq!(txt_index("182_3-0_.txt", "182_3-0"), None);
        assert_eq!(txt_index("182_3-0.png", "182_3-0"), None);
        assert_eq!(txt_index("182_30-0.txt", "182_3-0"), None);
    }

    #[test]
    fn png_encoder_writes_valid_signature() {
        let rgba = vec![255u8, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 9, 9, 9, 9];
        let png = encode_png_rgba(2, 2, &rgba);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        std::fs::write(std::env::temp_dir().join("classic_npc_test.png"), png).unwrap();
    }

    #[test]
    fn shared_texture_gets_numbered_txt_and_each_npc_finds_its_own() {
        let dir = std::env::temp_dir().join(format!("classic_npc_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let imgs = Assets::<Image>::default();
        let mods = ModTextures::default();
        let a = npc((207, 213), "Daggerfall");
        let b = npc((322, 164), "Wayrest");
        let c = npc((10, 20), "Sentinel");

        // 1a volta: crea, nessun override.
        assert_eq!(resolve(&dir, &a, &imgs, &mods), ClassicOverride::default());
        assert_eq!(resolve(&dir, &b, &imgs, &mods), ClassicOverride::default());
        assert_eq!(resolve(&dir, &c, &imgs, &mods), ClassicOverride::default());
        for f in ["182_3-0.txt", "182_3-0_1.txt", "182_3-0_2.txt"] {
            assert!(dir.join(f).is_file(), "manca {f}");
        }
        println!("{}", std::fs::read_to_string(dir.join("182_3-0_1.txt")).unwrap());

        // 2a volta: nessun file nuovo, base identica all'automatico.
        let ov = resolve(&dir, &b, &imgs, &mods);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 3);
        assert_eq!(ov.name.as_deref(), Some("Blonde woman"));
        assert_eq!(ov.description.as_deref(), Some(one_line(DESC).as_str()));
        assert_eq!((ov.voice, ov.walk_fps, ov.idle_fps), (None, None, None));

        // Personalizzo il secondo (Wayrest): lo ritrova per id, gli altri restano base.
        let path_b = dir.join("182_3-0_1.txt");
        let text = std::fs::read_to_string(&path_b).unwrap();
        let id_line = text.lines().find(|l| l.starts_with("id")).unwrap().to_string();
        std::fs::write(
            &path_b,
            format!("Ellie the warrior\n{id_line}\ndescription =\"a strong woman that admires you\"\nvoice = af_kore:0.9, af_sky:0.1\nposition = Tavern\nwalk_fps = 12\n# idle_fps = 3\n"),
        )
        .unwrap();
        let ov = resolve(&dir, &b, &imgs, &mods);
        assert_eq!(ov.name.as_deref(), Some("Ellie the warrior"));
        assert_eq!(ov.description.as_deref(), Some("a strong woman that admires you"));
        assert_eq!(ov.voice.as_deref(), Some("af_kore:0.9,af_sky:0.1"));
        assert_eq!((ov.walk_fps, ov.idle_fps), (Some(12.0), None));
        assert_eq!(resolve(&dir, &a, &imgs, &mods).name.as_deref(), Some("Blonde woman"));

        // Cartella inesistente: nessun effetto collaterale.
        let missing = dir.join("nope");
        assert_eq!(resolve(&missing, &a, &imgs, &mods), ClassicOverride::default());
        assert!(!missing.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---------------------------------------------------------- resolve_existing

    #[test]
    fn resolve_existing_does_not_create_anything() {
        let dir = std::env::temp_dir().join(format!("classic_npc_existing_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Nessun txt: nessun override, nessun file creato.
        let ov = resolve_existing(&dir, (207, 213), "flat:182:3", "182_3-0");
        assert_eq!(ov, ClassicOverride::default());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0,
                   "resolve_existing non deve creare file");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_existing_reads_an_existing_file() {
        let dir = std::env::temp_dir().join(format!("classic_npc_existing2_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Crea il txt "come se" resolve fosse già stato chiamato.
        let npc = npc((207, 213), "Daggerfall");
        let imgs = Assets::<Image>::default();
        let mods = ModTextures::default();
        let _ = resolve(&dir, &npc, &imgs, &mods);
        let path = dir.join("182_3-0.txt");
        assert!(path.is_file());

        // Personalizzo il file.
        std::fs::write(
            &path,
            format!(
                "Ellie\nid = 207,213 flat:182:3\ndescription = \"custom desc\"\nvoice = af_sarah\nposition = Tavern\nwalk_fps = 6\n",
            ),
        )
        .unwrap();

        // `resolve_existing` trova e legge l'override.
        let ov = resolve_existing(&dir, (207, 213), "flat:182:3", "182_3-0");
        assert_eq!(ov.name.as_deref(), Some("Ellie"));
        assert_eq!(ov.description.as_deref(), Some("custom desc"));
        assert_eq!(ov.voice.as_deref(), Some("af_sarah"));
        assert_eq!(ov.walk_fps, Some(6.0));

        // Città diversa: il file c'è ma con id diverso, quindi non corrisponde.
        let ov = resolve_existing(&dir, (322, 164), "flat:182:3", "182_3-0");
        assert_eq!(ov, ClassicOverride::default());

        // Cartella inesistente: default, niente panico.
        let missing = dir.join("nope");
        let ov = resolve_existing(&missing, (207, 213), "flat:182:3", "182_3-0");
        assert_eq!(ov, ClassicOverride::default());
        assert!(!missing.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ------------------------------------------------------ apply_llm_background

    #[test]
    fn apply_llm_background_creates_file_when_missing() {
        let dir = std::env::temp_dir().join(format!("classic_npc_bg_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Nessun file: `apply_llm_background` crea.
        apply_llm_background(
            &dir,
            (207, 213),
                             "flat:181:3",
                             "181_3-0",
                             "blue-cloaked boy",
                             Some("Matthew"),
                             Some("You are Matthew, a member of the Temple."),
        );
        let path = dir.join("181_3-0.txt");
        assert!(path.is_file());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("Matthew\n"), "{content}");
        assert!(
            content.contains("description = \"You are Matthew, a member of the Temple.\""),
                "{content}"
        );
        assert!(content.contains("id = 207,213 flat:181:3"), "{content}");

        // `resolve_existing` lo trova.
        let ov = resolve_existing(&dir, (207, 213), "flat:181:3", "181_3-0");
        assert_eq!(ov.name.as_deref(), Some("Matthew"));
        assert_eq!(
            ov.description.as_deref(),
                   Some("You are Matthew, a member of the Temple.")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_llm_background_rewrites_auto_file() {
        let dir = std::env::temp_dir().join(format!("classic_npc_bg_auto_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Simula il file auto creato da `resolve`.
        let base = npc((207, 213), "Daggerfall");
        let imgs = Assets::<Image>::default();
        let mods = ModTextures::default();
        let _ = resolve(&dir, &base, &imgs, &mods);
        let path = dir.join("182_3-0.txt");
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(before.starts_with("Blonde woman\n"));
        assert!(before.contains("description = \"Appearance (visual descriptor"));

        // LLM produce nome + background: `apply_llm_background` riscrive.
        apply_llm_background(
            &dir,
            (207, 213),
                             "flat:182:3",
                             "182_3-0",
                             "Blonde woman",
                             Some("Eleanor"),
                             Some("You are Eleanor, a barmaid of the tavern."),
        );
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.starts_with("Eleanor\n"), "{after}");
        assert!(
            after.contains("description = \"You are Eleanor, a barmaid of the tavern.\""),
                "{after}"
        );
        // Il resto è preservato.
        assert!(after.contains("id = 207,213 flat:182:3"), "{after}");
        assert!(after.contains("voice ="), "{after}");
        assert!(after.contains("# walk_fps = 8.0"), "{after}");

        // Idempotente: una seconda chiamata non tocca il file (nome non più auto).
        apply_llm_background(
            &dir,
            (207, 213),
                             "flat:182:3",
                             "182_3-0",
                             "Blonde woman",
                             Some("Different"),
                             Some("Different description."),
        );
        let again = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after, again);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_llm_background_respects_customized_name() {
        let dir = std::env::temp_dir().join(format!("classic_npc_bg_name_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let base = npc((207, 213), "Daggerfall");
        let imgs = Assets::<Image>::default();
        let mods = ModTextures::default();
        let _ = resolve(&dir, &base, &imgs, &mods);
        let path = dir.join("182_3-0.txt");

        // L'utente cambia solo il nome.
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replacen("Blonde woman", "Ellie", 1)).unwrap();

        apply_llm_background(
            &dir,
            (207, 213),
                             "flat:182:3",
                             "182_3-0",
                             "Blonde woman",
                             Some("Matthew"),
                             Some("You are Matthew, a member of the Temple."),
        );
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("Ellie"));
        assert!(!after.contains("Matthew"));
        assert!(!after.contains("You are Matthew"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_llm_background_respects_customized_description() {
        let dir = std::env::temp_dir().join(format!("classic_npc_bg_desc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let base = npc((207, 213), "Daggerfall");
        let imgs = Assets::<Image>::default();
        let mods = ModTextures::default();
        let _ = resolve(&dir, &base, &imgs, &mods);
        let path = dir.join("182_3-0.txt");

        // L'utente cambia solo la description.
        let text = std::fs::read_to_string(&path).unwrap();
        let modified: String = text
        .lines()
        .map(|l| {
            if l.trim().starts_with("description") {
                "description = \"My custom description.\""
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
        std::fs::write(&path, modified).unwrap();

        apply_llm_background(
            &dir,
            (207, 213),
                             "flat:182:3",
                             "182_3-0",
                             "Blonde woman",
                             Some("Matthew"),
                             Some("You are Matthew, a member of the Temple."),
        );
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("My custom description."));
        assert!(!after.contains("You are Matthew"));
        // Il nome NON viene aggiornato (file considerato personalizzato).
        assert!(!after.contains("Matthew"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_llm_background_noop_when_both_none() {
        let dir = std::env::temp_dir().join(format!("classic_npc_bg_noop_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        apply_llm_background(
            &dir,
            (207, 213),
                             "flat:182:3",
                             "182_3-0",
                             "Blonde woman",
                             None,
                             None,
        );
        // Nessun file creato.
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_llm_voice_fills_only_an_empty_voice_field() {
        let dir = std::env::temp_dir().join(format!("classic_npc_voice_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        apply_llm_background(&dir, (207, 213), "flat:181:3", "181_3-0", "blue-cloaked boy", Some("Matthew"), Some("You are Matthew."));
        let path = dir.join("181_3-0.txt");
        assert!(std::fs::read_to_string(&path).unwrap().contains("\nvoice =\n"));

        // Voce vuota: viene riempita, il resto del file resta com'era.
        apply_llm_voice(&dir, (207, 213), "flat:181:3", "181_3-0", "am_liam");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\nvoice = am_liam\n"), "{content}");
        assert!(content.contains("id = 207,213 flat:181:3") && content.contains("# voice: es."), "{content}");
        assert_eq!(resolve_existing(&dir, (207, 213), "flat:181:3", "181_3-0").voice.as_deref(), Some("am_liam"));

        // Voce gia' impostata (anche dall'utente): non si tocca.
        apply_llm_voice(&dir, (207, 213), "flat:181:3", "181_3-0", "bm_george");
        assert!(std::fs::read_to_string(&path).unwrap().contains("\nvoice = am_liam\n"));

        // Nessun file per quell'NPC: nessun effetto.
        apply_llm_voice(&dir, (1, 2), "flat:181:3", "181_3-0", "bm_george");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn frame_stems_are_consecutive_and_case_insensitive() {
        let dir = std::env::temp_dir().join(format!("classic_frames_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in [
            "182_11-0_walk_0.png",
            "182_11-0_Walk_1.PNG",
            "182_11-0_walk_2.png",
            "182_11-0_walk_4.png", // dopo un buco: ignorato
            "182_11-0_idle_0.png",
            "182_11-0.png",
            "182_11-0_walk_0.txt",
            "182_12-0_walk_0.png", // altra texture
        ] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        assert_eq!(
            frame_stems(&dir, "182_11-0", "walk"),
            vec!["182_11-0_walk_0", "182_11-0_Walk_1", "182_11-0_walk_2"]
        );
        assert_eq!(frame_stems(&dir, "182_11-0", "idle"), vec!["182_11-0_idle_0"]);
        assert!(frame_stems(&dir, "184_10-0", "walk").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
