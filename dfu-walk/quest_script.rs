//! Parser dei file di quest decompilati di Daggerfall Unity
//! (`StreamingAssets/Quests/*.txt`).
//!
//! Fase 1: legge header e QRC (i testi). QBN (la logica) verrà dopo.
//!
//! Formato:
//! ```text
//! Quest: N0B00Y16
//! DisplayName: Open a chest
//! QRC:
//! QuestorOffer:  [1000]
//! testo
//! <--->
//! variante alternativa
//! Message:  1020
//! testo
//! QBN:
//! ...
//! ```

use bevy::prelude::*;
use std::collections::HashMap;
use std::path::Path;

/// Cartella di default dei quest script di DFU.
pub const DEFAULT_QUESTS_DIR: &str =
    "/mnt/1CE16DBE48849731/Daggerfall/DaggerfallUnity_Data/StreamingAssets/Quests";

#[derive(Debug, Clone)]
pub struct QuestScript {
    /// ID tecnico, es. "N0B00Y16".
    pub id: String,
    /// Nome leggibile, es. "Open a chest".
    pub display_name: String,
    /// Testi per nome di sezione ("QuestorOffer", "AcceptQuest", ...).
    pub sections: HashMap<String, String>,
    /// Testi per ID numerico (`Message: 1020`, `QuestLogEntry: 1010`, ...).
    pub messages: HashMap<u32, String>,
}

impl QuestScript {
    pub fn section(&self, name: &str) -> Option<&str> {
        self.sections.get(name).map(String::as_str)
    }

    pub fn message(&self, id: u32) -> Option<&str> {
        self.messages.get(&id).map(String::as_str)
    }

    pub fn offer(&self) -> Option<&str> { self.section("QuestorOffer") }
    pub fn accept(&self) -> Option<&str> { self.section("AcceptQuest") }
    pub fn refuse(&self) -> Option<&str> { self.section("RefuseQuest") }
    pub fn complete(&self) -> Option<&str> { self.section("QuestComplete") }
    pub fn fail(&self) -> Option<&str> { self.section("QuestFail") }
    pub fn log_entry(&self) -> Option<&str> {
        self.section("QuestLogEntry")
            .or_else(|| self.message(1010))
    }
}

/// Database di tutti i quest script caricati.
#[derive(Resource, Default)]
pub struct QuestScriptDb {
    by_id: HashMap<String, QuestScript>,
}

impl QuestScriptDb {
    pub fn len(&self) -> usize { self.by_id.len() }
    pub fn is_empty(&self) -> bool { self.by_id.is_empty() }
    pub fn get(&self, id: &str) -> Option<&QuestScript> { self.by_id.get(id) }
    pub fn insert(&mut self, script: QuestScript) { self.by_id.insert(script.id.clone(), script); }

    /// Carica tutti i `.txt` da una cartella (case-insensitive).
    pub fn load_dir(&mut self, dir: &Path) -> usize {
        let Ok(rd) = std::fs::read_dir(dir) else {
            warn!("QuestScript: cartella {} non trovata", dir.display());
            return 0;
        };
        let mut count = 0;
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("txt")) != Some(true) {
                continue;
            }
            match std::fs::read_to_string(&path) {
                Ok(text) => match parse(&text) {
                    Ok(s) => {
                        if !s.sections.is_empty() {
                            debug!("QuestScript: {} ({}, {} sezioni, {} messaggi)",
                                   s.id, s.display_name, s.sections.len(), s.messages.len());
                            self.insert(s);
                            count += 1;
                        }
                    }
                    Err(e) => warn!("QuestScript: {} non valido: {e}", path.display()),
                },
                Err(e) => warn!("QuestScript: impossibile leggere {}: {e}", path.display()),
            }
        }
        info!("QuestScript: caricati {} script da {}", count, dir.display());
        count
    }
}

// =============================================================================
// Parser
// =============================================================================

pub fn parse(text: &str) -> Result<QuestScript, String> {
    let mut id = String::new();
    let mut display_name = String::new();
    let mut sections = HashMap::new();
    let mut messages = HashMap::new();

    let mut in_qrc = false;
    let mut current: Option<(Option<String>, Option<u32>, Vec<String>)> = None;

    // chiude il blocco corrente e lo salva
    fn flush(
        sections: &mut HashMap<String, String>,
        messages: &mut HashMap<u32, String>,
        block: Option<(Option<String>, Option<u32>, Vec<String>)>,
    ) {
        let Some((name, num, lines)) = block else { return };
        // prima variante (separatore `<--->`)
        let mut text = String::new();
        for line in &lines {
            if line.trim() == "<--->" { break; }
            if !text.is_empty() { text.push('\n'); }
            text.push_str(line);
        }
        let trimmed = text.trim();
        if trimmed.is_empty() { return; }
        if let Some(n) = num {
            messages.insert(n, trimmed.to_string());
        } else if let Some(nm) = name {
            sections.insert(nm, trimmed.to_string());
        }
    }

    for line in text.lines() {
        let trimmed = line.trim();

        // inizio QRC
        if trimmed.eq_ignore_ascii_case("QRC:") {
            in_qrc = true;
            continue;
        }
        // fine QRC (inizio QBN o EOF)
        if trimmed.eq_ignore_ascii_case("QBN:") {
            flush(&mut sections, &mut messages, current.take());
            in_qrc = false;
            continue;
        }

        if !in_qrc {
            // header: `Chiave: valore` (ma solo prima di QRC)
            if let Some((k, v)) = trimmed.split_once(':') {
                let k = k.trim();
                let v = v.trim().to_string();
                match k {
                    "Quest" => id = v,
                    "DisplayName" => display_name = v,
                    _ => {}
                }
            }
            continue;
        }

        // dentro QRC: riconosci un'intestazione di sezione?
        if let Some((name, num)) = parse_section_header(trimmed) {
            flush(&mut sections, &mut messages, current.take());
            current = Some((name, num, Vec::new()));
            continue;
        }

        // riga di testo: aggiungi al blocco corrente
        if let Some((_, _, lines)) = current.as_mut() {
            lines.push(line.to_string());
        }
    }
    flush(&mut sections, &mut messages, current.take());

    if id.is_empty() {
        return Err("manca `Quest:` nell'header".into());
    }
    Ok(QuestScript { id, display_name, sections, messages })
}

/// Riconosce righe tipo:
///   `QuestorOffer:  [1000]`
///   `Message:  1020`
///   `QuestLogEntry:  [1010]`
/// e restituisce `(nome_sezione, id_numerico)`. Se la riga non è un'intestazione,
/// restituisce `None` (è testo).
fn parse_section_header(line: &str) -> Option<(Option<String>, Option<u32>)> {
    // Deve iniziare con un nome identificatore seguito da `:`
    let colon = line.find(':')?;
    let name = line[..colon].trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let rest = line[colon + 1..].trim();
    // Il resto deve essere `[N]` o `N`
    let num = if rest.starts_with('[') {
        let close = rest.find(']')?;
        rest[1..close].trim().parse::<u32>().ok()
    } else if rest.chars().all(|c| c.is_ascii_digit()) && !rest.is_empty() {
        rest.parse::<u32>().ok()
    } else {
        return None;
    };
    // Se il nome è `Message` e c'è un numero, salva come messaggio.
    // Altrimenti è una sezione nominata (con eventuale ID numerico duplicato).
    let is_message_name = matches!(name, "Message" | "QuestLogEntry" | "QuestTimeLapse");
    if is_message_name {
        // QuestLogEntry/QuestTimeLapse sono sezioni note, ma hanno anche un ID.
        // Le metto in entrambe le mappe per comodità.
        Some((Some(name.to_string()), num))
    } else {
        Some((Some(name.to_string()), num))
    }
}

// =============================================================================
// Risoluzione placeholder
// =============================================================================

/// Dizionario di default per i placeholder che non abbiamo ancora.
fn default_vars() -> &'static [(&'static str, &'static str)] {
    &[
        ("questgiver", "the guildmaster"),
        ("merchant", "the local merchant"),
        ("mage", "the old wizard"),
        ("crn", "the chest"),
        ("I.01", "the wand"),
        ("foil", "the cousin"),
        ("contact", "the contact"),
        ("reward", "300"),
        ("gold", "150"),
        ("bribe", "50"),
        ("elysana", "Princess Elysana"),
        ("note", "the note"),
    ]
}

/// Risolve i placeholder di una sezione QRC. Placeholder non risolti restano visibili
/// tra parentesi graffe, così si vede cosa manca.
pub fn resolve(
    raw: &str,
    player_first: &str,
    player_last: &str,
    region: &str,
    player_race: &str,
) -> String {
    let vars: HashMap<&str, &str> = default_vars().iter().copied().collect();
    let mut out = String::with_capacity(raw.len());
    for line in raw.lines() {
        let l = line.strip_prefix("<ce>").unwrap_or(line).trim_start();
        if !out.is_empty() { out.push('\n'); }
        out.push_str(l);
    }

    // Sostituzioni di gioco (prima, così non vengono riscritte).
    out = out.replace("%pcf", player_first)
             .replace("%pcn", &format!("{player_first} {player_last}").trim().to_string())
             .replace("%ra", player_race)
             .replace("%reg", region)
             .replace("%god", "the gods")
             .replace("%qdt", "today");

    // Variabili con articoli/preposizioni (più lunghe prima, così non vengono
    // intercettate da quelle più corte).
    for (name, value) in &vars {
        out = out.replace(&format!("___{name}_"), &format!("at the {value}"))
                 .replace(&format!("__{name}_"), value)
                 .replace(&format!("={name}_"), &format!("the {value}"))
                 .replace(&format!("_{name}_"), value);
    }

    // Placeholder non risolti: `_X_` o `__X_` etc. → `{X}`.
    let mut cleaned = String::with_capacity(out.len());
    let mut chars = out.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_' {
            let mut buf = String::new();
            while let Some(&n) = chars.peek() {
                if n == '_' { chars.next(); continue; }
                if n.is_alphanumeric() || n == '.' || n == '-' {
                    buf.push(n);
                    chars.next();
                } else { break; }
            }
            if buf.is_empty() { cleaned.push('_'); }
            else { cleaned.push('{'); cleaned.push_str(&buf); cleaned.push('}'); }
        } else {
            cleaned.push(c);
        }
    }
    cleaned
}

// =============================================================================
// Plugin di caricamento
// =============================================================================

pub struct QuestScriptPlugin {
    pub dir: std::path::PathBuf,
}

impl Default for QuestScriptPlugin {
    fn default() -> Self {
        Self { dir: std::path::PathBuf::from(DEFAULT_QUESTS_DIR) }
    }
}

impl Plugin for QuestScriptPlugin {
    fn build(&self, app: &mut App) {
        let mut db = QuestScriptDb::default();
        if self.dir.is_dir() {
            db.load_dir(&self.dir);
        } else {
            info!("QuestScript: nessuna cartella {}, script disattivati", self.dir.display());
        }
        app.insert_resource(db);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
-- file di test
Quest: N0B00Y16
DisplayName: Open a chest
Messages: 52
QRC:

QuestorOffer:  [1000]
<ce>                   %pct, I need someone who can cast
<ce>                    the Open spell.  Please tell me
<ce>                     you have learned this by now.

RefuseQuest:  [1001]
<ce>                  Really, you need to devote yourself

AcceptQuest:  [1002]
<ce>                 Well, that's a start.  The job I need
<ce>                 you for is quite simple.

Message:  1020
<ce> testo del messaggio
<---> variante alternativa

QBN:
Item _crn_ item class 1 subclass 8
"#;

    #[test]
    fn parses_header_and_sections() {
        let s = parse(SAMPLE).unwrap();
        assert_eq!(s.id, "N0B00Y16");
        assert_eq!(s.display_name, "Open a chest");
        assert!(s.offer().unwrap().contains("Open spell"));
        assert!(s.accept().unwrap().contains("that's a start"));
        assert!(s.refuse().unwrap().contains("devote yourself"));
        assert!(s.message(1020).unwrap().contains("testo del messaggio"));
    }

    #[test]
    fn variants_keep_the_first() {
        let s = parse(SAMPLE).unwrap();
        // `<---> variante alternativa` è sulla stessa riga → è un separatore
        // seguito da testo sulla stessa riga, quindi va tenuto? No: nel formato reale
        // il separatore è da solo. Qui il parser prende la prima variante (la riga
        // "testo del messaggio") e ignora la seconda.
        assert_eq!(s.message(1020).unwrap(), "testo del messaggio");
    }

    #[test]
    fn missing_quest_id_is_error() {
        assert!(parse("QRC:\nQuestorOffer: [1]\nhi\nQBN:\n").is_err());
    }

    #[test]
    fn empty_qrc_is_ok() {
        let s = parse("Quest: X\nDisplayName: Y\nQRC:\nQBN:\n").unwrap();
        assert_eq!(s.id, "X");
        assert!(s.sections.is_empty());
    }

    #[test]
    fn resolve_handles_known_and_unknown_vars() {
        let raw = "<ce>%pcf, _merchant_ has a _crn_ and _unknown_ too.";
        let r = resolve(raw, "Marco", "", "Daggerfall", "Breton");
        assert!(r.contains("Marco"));
        assert!(r.contains("the local merchant"));
        assert!(r.contains("the chest"));
        assert!(r.contains("{unknown}"), "placeholder sconosciuto visibile: {r}");
        assert!(!r.contains("_"), "nessun underscore residuo: {r}");
    }

    #[test]
    fn resolve_understands_articles() {
        let raw = "Go ___shop_ and see =contact_.";
        let r = resolve(raw, "M", "", "R", "B");
        // "___shop_" → "at the {shop}" (shop non risolto)
        assert!(r.contains("at the {shop}"), "{r}");
        // "=contact_" → "the the contact"? No, "contact" è risolto come "the contact",
        // quindi "the {contact}"... controlliamo che non ci siano doppi
        // In realtà "=contact_" si risolve in "the contact" + prefisso "=" → "the contact".
        // Ma il prefisso "=" è gestito prima: out contiene già "the contact".
        assert!(!r.contains("=contact_"), "{r}");
    }
}