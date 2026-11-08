//! Stato della main quest dentro ai savegame.
//!
//! Ogni salvataggio `X` (`savegame.txt`, `saves/nome.txt`) ha un file gemello `X.quest` con
//! lo stato di TUTTA la quest in quel momento: fasi (`QuestStates`), runtime QBN, giornale,
//! reputazione e oggetti di quest. L'estensione e' `.quest`, quindi non compare nella lista
//! della finestra F2 (che mostra solo i `.txt`).
//!
//! Come si aggancia a `savegame.rs` senza toccare i sistemi di salvataggio (sono molti e a
//! 16 parametri): una fotografia dello stato (`LATEST`) e' aggiornata ogni 0.1 s da
//! `refresh_snapshot`; `write_savegame_to` la scrive accanto al salvataggio
//! (`write_sidecar`). In caricamento, `load_savegame_now` chiama `queue_restore`, che legge
//! il `.quest` e lo mette in coda; `apply_pending_restore` lo applica ai resource.
//!
//! Salvataggi vecchi, senza `.quest`: lo stato attuale resta com'e' (e il log lo dice).
//!
//! Dopo un restore, i file "globali" (`quest_states.txt`, `quest_runtime.txt`...) vengono
//! riscritti, cosi' sono coerenti con il salvataggio caricato.
//!
//! In `main.rs`: `mod quest_save;` (il plugin lo registra `SaveGamePlugin`).

use crate::dfquest::{QuestPhase, QuestStates};
use crate::journal::{QuestJournal, Reputation};
use crate::main_quest::QbnRuntimeRes;
use crate::qbn::QbnRuntime;
use crate::quest_items::QuestItems;
use crate::{QuestLibRes, QuestStatesRes};
use bevy::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const SECTION_PREFIX: &str = "#### quest-section: ";
const SECTION_SUFFIX: &str = " ####";

/// Ultima fotografia dello stato (testo gia' pronto da scrivere).
static LATEST: Mutex<String> = Mutex::new(String::new());
/// Sidecar letto da `load_savegame_now`, in attesa di essere applicato.
static PENDING: Mutex<Option<String>> = Mutex::new(None);

// =============================================================================
// Formato
// =============================================================================

#[derive(Debug, Clone, Default, PartialEq)]
pub struct QuestSnapshot {
    pub states: String,
    pub runtime: String,
    pub journal: String,
    pub reputation: String,
    pub items: String,
}

impl QuestSnapshot {
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (name, body) in [
            ("states", &self.states),
            ("runtime", &self.runtime),
            ("journal", &self.journal),
            ("reputation", &self.reputation),
            ("items", &self.items),
        ] {
            out.push_str(&format!("{SECTION_PREFIX}{name}{SECTION_SUFFIX}\n"));
            out.push_str(body);
            if !body.is_empty() && !body.ends_with('\n') {
                out.push('\n');
            }
        }
        out
    }

    /// `None` se il testo non contiene nessuna sezione riconosciuta.
    pub fn from_text(text: &str) -> Option<QuestSnapshot> {
        let mut snap = QuestSnapshot::default();
        let mut current: Option<String> = None;
        let mut any = false;
        let mut bufs: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for line in text.lines() {
            if let Some(name) = line
                .strip_prefix(SECTION_PREFIX)
                .and_then(|r| r.strip_suffix(SECTION_SUFFIX))
            {
                current = Some(name.trim().to_string());
                any = true;
                continue;
            }
            if let Some(name) = &current {
                let b = bufs.entry(name.clone()).or_default();
                b.push_str(line);
                b.push('\n');
            }
        }
        if !any {
            return None;
        }
        let take = |bufs: &mut std::collections::HashMap<String, String>, k: &str| bufs.remove(k).unwrap_or_default();
        snap.states = take(&mut bufs, "states");
        snap.runtime = take(&mut bufs, "runtime");
        snap.journal = take(&mut bufs, "journal");
        snap.reputation = take(&mut bufs, "reputation");
        snap.items = take(&mut bufs, "items");
        Some(snap)
    }
}

/// `savegame.txt` -> `savegame.txt.quest`.
pub fn sidecar_path(save: &Path) -> PathBuf {
    let mut s = save.as_os_str().to_owned();
    s.push(".quest");
    PathBuf::from(s)
}

// =============================================================================
// Ganci per savegame.rs
// =============================================================================

/// Scrive accanto a `save` l'ultima fotografia dello stato. Da chiamare dopo aver scritto il
/// salvataggio. Se non c'e' ancora nessuna fotografia non fa nulla.
pub fn write_sidecar(save: &Path) {
    let snap = LATEST.lock().map(|g| g.clone()).unwrap_or_default();
    if snap.is_empty() {
        return;
    }
    if let Err(e) = std::fs::write(sidecar_path(save), snap) {
        warn!("quest_save: impossibile scrivere {}: {e}", sidecar_path(save).display());
    }
}

/// Legge il sidecar di `save` e lo mette in coda per `apply_pending_restore`.
/// `false` se non esiste (salvataggio vecchio).
pub fn queue_restore(save: &Path) -> bool {
    match std::fs::read_to_string(sidecar_path(save)) {
        Ok(text) => {
            if let Ok(mut p) = PENDING.lock() {
                *p = Some(text);
                return true;
            }
            false
        }
        Err(_) => false,
    }
}

/// Cancella il sidecar quando si elimina un salvataggio.
pub fn remove_sidecar(save: &Path) {
    let p = sidecar_path(save);
    if p.exists() {
        if let Err(e) = std::fs::remove_file(&p) {
            warn!("quest_save: impossibile eliminare {}: {e}", p.display());
        }
    }
}

// =============================================================================
// Plugin
// =============================================================================

pub struct QuestSavePlugin;

impl Plugin for QuestSavePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                // prima di `handle_inputs`: se il salvataggio caricato ha gia' la quest in corso,
                // lo `StartQuest` automatico dell'avvio trova l'istanza e non fa nulla
                apply_pending_restore.before(crate::main_quest::handle_inputs),
                refresh_snapshot.after(apply_pending_restore),
            ),
        );
    }
}

fn refresh_snapshot(
    time: Res<Time>,
    mut acc: Local<f32>,
    states: Res<QuestStatesRes>,
    rt: Res<QbnRuntimeRes>,
    journal: Res<QuestJournal>,
    rep: Res<Reputation>,
    items: Res<QuestItems>,
) {
    *acc += time.delta_secs();
    if *acc < 0.1 {
        return;
    }
    *acc = 0.0;
    let snap = QuestSnapshot {
        states: states.0.to_text(),
        runtime: rt.0.to_text(),
        journal: journal.to_text(),
        reputation: rep.to_text(),
        items: items.to_text(),
    };
    if let Ok(mut latest) = LATEST.lock() {
        *latest = snap.to_text();
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_pending_restore(
    lib: Res<QuestLibRes>,
    mut states: ResMut<QuestStatesRes>,
    mut rt: ResMut<QbnRuntimeRes>,
    mut journal: ResMut<QuestJournal>,
    mut rep: ResMut<Reputation>,
    mut items: ResMut<QuestItems>,
) {
    let text = match PENDING.lock() {
        Ok(mut p) => p.take(),
        Err(_) => None,
    };
    let Some(text) = text else { return };
    let Some(snap) = QuestSnapshot::from_text(&text) else {
        warn!("quest_save: sidecar illeggibile, stato quest invariato");
        return;
    };

    states.0 = QuestStates::from_text(&snap.states);
    // come all'avvio (main.rs): gli argomenti di dialogo di avvio delle quest attive
    for script in lib.0.scripts() {
        if states.0.phase(&script.id) == QuestPhase::Active {
            for t in script.startup_topics() {
                states.0.enable_topic(&script.id, &t.symbol);
            }
        }
    }
    rt.0 = QbnRuntime::from_text(&snap.runtime);
    *journal = QuestJournal::from_text(&snap.journal);
    *rep = Reputation::from_text(&snap.reputation);
    *items = QuestItems::from_text(&snap.items);

    // journal/reputation/items si riscrivono da soli (cambiati); stati e runtime no.
    let _ = std::fs::write("quest_states.txt", states.0.to_text());
    let _ = std::fs::write("quest_runtime.txt", rt.0.to_text());
    info!("quest_save: stato della quest ripristinato dal salvataggio");
}

// =============================================================================
// Test
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> QuestSnapshot {
        QuestSnapshot {
            states: "phase S0000001 Active\n".into(),
            runtime: "now 12.5\ninst S0000001\n".into(),
            journal: "entry\tS0000001\t0\ttesto con # e ==== dentro\n".into(),
            reputation: "legal\t-20\n".into(),
            items: "next\t1\nheld\tS1\tdeathCertificate\t1000001\tletter\n".into(),
        }
    }

    #[test]
    fn snapshot_roundtrip() {
        let s = sample();
        assert_eq!(QuestSnapshot::from_text(&s.to_text()), Some(s));
    }

    #[test]
    fn empty_sections_survive() {
        let s = QuestSnapshot { states: "x\n".into(), ..Default::default() };
        let back = QuestSnapshot::from_text(&s.to_text()).unwrap();
        assert_eq!(back.states, "x\n");
        assert_eq!(back.items, "");
    }

    #[test]
    fn text_without_sections_is_rejected() {
        assert!(QuestSnapshot::from_text("niente di utile\n").is_none());
        assert!(QuestSnapshot::from_text("").is_none());
    }

    #[test]
    fn sidecar_sits_next_to_the_save() {
        assert_eq!(sidecar_path(Path::new("savegame.txt")), PathBuf::from("savegame.txt.quest"));
        assert_eq!(sidecar_path(Path::new("saves/Il mio.txt")), PathBuf::from("saves/Il mio.txt.quest"));
        // l'estensione e' `quest`: la lista F2 (solo `.txt`) non la mostra
        assert_eq!(sidecar_path(Path::new("saves/a.txt")).extension().and_then(|e| e.to_str()), Some("quest"));
    }
}
