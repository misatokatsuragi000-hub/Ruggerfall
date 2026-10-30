//! Descrittori degli NPC "classici" (flat di DFU, volti generici, passanti).
//!
//! Funziona come per gli NPC custom (`npc_*.txt`), ma senza texture: la prima
//! volta che il giocatore parla con un NPC classico, in `textures_dir` viene
//! creato un file base con nome, descrizione, voce e posizione. Da quel momento
//! il file e' la fonte di verita': se lo modifichi, il dialogo usa i tuoi dati.
//!
//! ```text
//! classic_<citta>_<x>_<y>_<chiave>.txt      es. classic_wayrest_322_164_flat_182_11.txt
//!
//! Blonde woman                              <- nome (etichetta nel dialogo)
//! # ...commenti (ignorati)...
//! description = "..."                       <- sostituisce la descrizione automatica
//! voice = af_kore:0.9,af_sky:0.1            <- vuoto = voce automatica
//! position = Tavern                         <- solo informativa
//! ```
//!
//! Accanto al `.txt` vengono salvate le immagini, cosi' sai chi e' l'NPC:
//! `<stesso nome>.png` (sprite intero) e `<stesso nome>_por.png` (ritratto del dialogo).
//! Se vuoi trasformarlo in un NPC custom, rinomina png e txt in `npc_<nome>`.
//!
//! Identita': come per le voci in `QuestBook`, un NPC classico e' la coppia
//! (citta', chiave) dove la chiave e' `flat:A:R`, `face:N` o `walker:N`.
//! Il file viene riletto a ogni apertura del dialogo: le modifiche si vedono
//! subito, senza riavviare. Il file non viene mai sovrascritto.
//! Un campo vuoto o mancante ripiega sul valore automatico.

use crate::data::city_map::PoiKind;
use crate::interior::parse_custom_txt;
use crate::mod_textures::ModTextures;
use bevy::prelude::*;
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
    /// Sprite intero e flag "righe capovolte" (come `ModTextures::needs_v_flip`).
    pub sprite: Option<(Handle<Image>, bool)>,
    /// Ritratto mostrato nel dialogo e flag "righe capovolte" (`is_hd`).
    pub portrait: Option<(Handle<Image>, bool)>,
}

/// Valori letti dal file; `None` = usa quello automatico.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ClassicOverride {
    pub name: Option<String>,
    pub description: Option<String>,
    pub voice: Option<String>,
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    let mut last_underscore = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_underscore = false;
        } else if !last_underscore {
            out.push('_');
            last_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

pub fn file_path(dir: &Path, npc: &ClassicNpc) -> PathBuf {
    let city = slug(npc.city_name);
    let city = if city.is_empty() { "citta".to_string() } else { city };
    dir.join(format!(
        "classic_{}_{}_{}_{}.txt",
        city,
        npc.city.0.max(0),
        npc.city.1.max(0),
        slug(npc.key)
    ))
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

/// Salva `<nome>.png` (sprite) e `<nome>_por.png` (ritratto) accanto al txt,
/// solo se mancano (non sovrascrive mai).
fn save_images(txt_path: &Path, npc: &ClassicNpc, images: &Assets<Image>, mods: &ModTextures) {
    let stem = txt_path.with_extension("");
    let mut sprite = stem.clone().into_os_string();
    sprite.push(".png");
    let mut por = stem.into_os_string();
    por.push("_por.png");
    save_one(PathBuf::from(sprite), &npc.sprite, images, mods);
    save_one(PathBuf::from(por), &npc.portrait, images, mods);
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
    format!(
        "{name}\n\
         # NPC classico: file generato automaticamente, modificalo a piacere (non verra' sovrascritto).\n\
         # Citta': {city} ({x}, {y})  |  chiave: {key}\n\
         # La prima riga e' il nome mostrato nel dialogo.\n\
         # description: sostituisce la descrizione automatica nel prompt dell'LLM (una riga sola).\n\
         # voice: es. af_kore:0.9,af_sky:0.1 (vuoto = voce automatica).\n\
         # position: solo informativa (dove l'NPC e' stato incontrato), non sposta l'NPC.\n\
         # Immagini: stesso nome del file con .png (sprite) e _por.png (ritratto).\n\
         description = \"{desc}\"\n\
         voice =\n\
         position = {pos}\n",
        city = if npc.city_name.is_empty() { "?" } else { npc.city_name },
        x = npc.city.0,
        y = npc.city.1,
        key = npc.key,
        desc = one_line(npc.description),
        pos = position_word(npc.place),
    )
}

/// Legge il file dell'NPC se esiste, altrimenti lo crea con i valori automatici.
/// Non fallisce mai: se la cartella manca o non e' scrivibile ripiega sul
/// comportamento automatico.
pub fn resolve(dir: &Path, npc: &ClassicNpc, images: &Assets<Image>, mods: &ModTextures) -> ClassicOverride {
    if !dir.is_dir() {
        return ClassicOverride::default();
    }
    let path = file_path(dir, npc);
    save_images(&path, npc, images, mods);
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let (name, description, voice, _position) = parse_custom_txt(&content);
            ClassicOverride {
                name: (!name.is_empty()).then_some(name),
                description,
                voice,
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let written = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .and_then(|mut f| f.write_all(base_text(npc).as_bytes()));
            if let Err(e) = written {
                warn!("NPC classico: impossibile creare {}: {e}", path.display());
            }
            ClassicOverride::default()
        }
        Err(e) => {
            warn!("NPC classico: impossibile leggere {}: {e}", path.display());
            ClassicOverride::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESC: &str = "- Appearance (visual descriptor from the game data, not a name or title): Blonde woman\n\
- Occupation: You work here as a barmaid.\n- Location: inside a tavern\n- Temperament (not visible, shapes how they talk): gruff";

    fn npc<'a>() -> ClassicNpc<'a> {
        ClassicNpc {
            city: (322, 164),
            city_name: "Wayrest",
            key: "flat:182:11",
            label: "Blonde woman",
            description: DESC,
            place: Some(PoiKind::Tavern),
            sprite: None,
            portrait: None,
        }
    }

    #[test]
    fn file_name_is_stable_and_safe() {
        let p = file_path(Path::new("/t"), &npc());
        assert_eq!(p, Path::new("/t/classic_wayrest_322_164_flat_182_11.txt"));
    }

    #[test]
    fn png_encoder_writes_valid_signature() {
        let rgba = vec![255u8, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 9, 9, 9, 9];
        let png = encode_png_rgba(2, 2, &rgba);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        std::fs::write(std::env::temp_dir().join("classic_npc_test.png"), png).unwrap();
    }

    #[test]
    fn creates_then_reads_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("classic_npc_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let n = npc();
        let imgs = Assets::<Image>::default();
        let mods = ModTextures::default();

        // 1a volta: crea la base, nessun override.
        assert_eq!(resolve(&dir, &n, &imgs, &mods), ClassicOverride::default());
        let path = file_path(&dir, &n);
        let text = std::fs::read_to_string(&path).unwrap();
        println!("{text}");

        // 2a volta: legge la base, nome e descrizione identici all'automatico.
        let ov = resolve(&dir, &n, &imgs, &mods);
        assert_eq!(ov.name.as_deref(), Some("Blonde woman"));
        assert_eq!(ov.description.as_deref(), Some(one_line(DESC).as_str()));
        assert_eq!(ov.voice, None);

        // Personalizzazione: viene usata e non sovrascritta.
        std::fs::write(
            &path,
            "Ellie the warrior\ndescription =\"a strong woman that admires you\"\nvoice = af_kore:0.9, af_sky:0.1\nposition = Tavern\n",
        )
        .unwrap();
        let ov = resolve(&dir, &n, &imgs, &mods);
        assert_eq!(ov.name.as_deref(), Some("Ellie the warrior"));
        assert_eq!(ov.description.as_deref(), Some("a strong woman that admires you"));
        assert_eq!(ov.voice.as_deref(), Some("af_kore:0.9,af_sky:0.1"));

        // Cartella inesistente: nessun effetto collaterale.
        let missing = dir.join("nope");
        assert_eq!(resolve(&missing, &n, &imgs, &mods), ClassicOverride::default());
        assert!(!missing.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
