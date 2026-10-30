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

use crate::data::city_map::PoiKind;
use crate::interior::parse_custom_txt;
use crate::mod_textures::ModTextures;
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
}
