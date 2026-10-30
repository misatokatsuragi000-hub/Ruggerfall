//! "Casa" degli NPC custom: gli NPC interni (i flat "people" del BlockData
//! interno e gli NPC custom `npc_*.png`) vengono spawnati **solo** nell'unico
//! edificio che il giocatore ha reclamato come casa. Tutti gli altri edifici
//! restano vuoti (muri, arredi, luci sì; persone no).
//!
//! ## Regola
//!
//! - La **prima volta** che il giocatore entra in un edificio **popolato**
//!   (con almeno una persona nel BlockData interno), quell'edificio diventa
//!   la casa degli NPC.
//! - La casa è identificata da `InteriorState::building()` (`"mx,my:bx,by,sub"`)
//!   e conserva la lista dei flat `(archive, record)` che la abitano.
//! - Quando **tutti** questi flat risultano in `InteriorState::recruited` (sono
//!   stati reclutati via dialogo LLM in `dialog.rs`), la casa si libera e il
//!   prossimo edificio popolato che il giocatore visita la occupa.
//! - Il file `npc_home.txt` conserva lo stato tra sessioni. Il reclutamento
//!   continua a essere salvato in `savegame.txt` (da `savegame.rs`), non qui.
//!
//! ## Flusso pratico
//!
//! 1. Entri nella prima taverna con persone → casa reclamata, NPC spawnati.
//! 2. Entri in un'altra taverna → la casa è già occupata, gli NPC restano a
//!    casa. La taverna è vuota.
//! 3. Convinci un NPC a seguirti via LLM → `InteriorState::mark_recruited`.
//! 4. Rientri nella casa → l'NPC reclutato non ricompare (saltato da
//!    `interior.rs`); gli altri sì.
//! 5. Recluti tutti → `release_home_when_empty` libera la casa, il prossimo
//!    edificio popolato che visiti diventa la nuova casa.

use std::path::Path;

use bevy::prelude::*;

use crate::interior::InteriorState;

const SAVE_FILE: &str = "npc_home.txt";

/// Stato della casa NPC.
#[derive(Resource, Default)]
pub struct NpcHome {
    /// Identificativo dell'edificio (`InteriorState::building()`), None = libera.
    pub building: Option<String>,
    /// Flat `(archive, record)` delle persone che abitano la casa.
    pub npc_flats: Vec<(u16, u8)>,
}

impl NpcHome {
    /// True se `building_id` è la casa corrente.
    pub fn is_home(&self, building_id: &str) -> bool {
        self.building.as_deref() == Some(building_id)
    }

    /// True se c'è una casa reclamata.
    pub fn is_claimed(&self) -> bool {
        self.building.is_some()
    }
}

/// Tenta di reclamare `building_id` come casa.
///
/// - Se è **già** la casa: `true` (gli NPC vanno spawnati, quelli reclutati no).
/// - Se la casa è **libera** (`building` None) e ci sono abitanti: diventa la
///   casa, `true`.
/// - Altrimenti: `false` (non è la casa, niente NPC).
///
/// `flats` è la lista `(archive, record)` delle persone dell'interno. Se è
/// vuota, l'edificio non è "popolato" e non può diventare casa.
pub fn claim_home_on_enter(
    home: &mut NpcHome,
    building_id: &str,
    flats: &[(u16, u8)],
) -> bool {
    if flats.is_empty() {
        return false;
    }
    match &home.building {
        Some(b) => b == building_id,
        None => {
            home.building = Some(building_id.to_string());
            home.npc_flats = flats.to_vec();
            info!(
                "Casa NPC reclamata: {} ({} abitanti)",
                  building_id,
                  flats.len()
            );
            true
        }
    }
}

/// Se **tutti** i flat della casa sono in `InteriorState::recruited`, libera
/// la casa. Restituisce `true` se è stata liberata (il chiamante può salvare).
pub fn maybe_release_home(home: &mut NpcHome, interior: &InteriorState) -> bool {
    let Some(building_id) = home.building.clone() else {
        return false;
    };
    if home.npc_flats.is_empty() {
        return false;
    }
    let all_recruited = home
    .npc_flats
    .iter()
    .all(|(a, r)| interior.is_recruited(*a, *r));
    if all_recruited {
        info!(
            "Tutti gli NPC della casa {} sono stati reclutati: casa libera",
            building_id
        );
        home.building = None;
        home.npc_flats.clear();
        true
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Salvataggio su testo (`npc_home.txt`)
// ---------------------------------------------------------------------------
//
// Formato:
//   # commento
//   home <building_id>       (id con %20 al posto degli spazi, per sicurezza)
//   flat <archive> <record>  (una riga per abitante)
//
// Se il file non esiste, si parte con casa libera.

fn encode(s: &str) -> String {
    // Sostituisce solo i caratteri che spezzerebbero il parsing per spazi/newline.
    s.replace('%', "%25")
    .replace(' ', "%20")
    .replace('\n', "%0A")
    .replace('\r', "%0D")
}

fn decode(s: &str) -> String {
    s.replace("%20", " ")
    .replace("%0A", "\n")
    .replace("%0D", "\r")
    .replace("%25", "%")
}

pub fn load(path: &Path) -> NpcHome {
    let Ok(text) = std::fs::read_to_string(path) else {
        return NpcHome::default();
    };
    let mut home = NpcHome::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts.as_slice() {
            ["home", building] => {
                home.building = Some(decode(building));
            }
            ["flat", archive, record] => {
                if let (Ok(a), Ok(r)) = (archive.parse::<u16>(), record.parse::<u8>()) {
                    home.npc_flats.push((a, r));
                }
            }
            _ => {}
        }
    }
    if home.is_claimed() {
        info!(
            "npc_home: caricata casa {} ({} abitanti)",
              home.building.as_deref().unwrap_or("?"),
              home.npc_flats.len()
        );
    } else {
        info!("npc_home: nessuna casa reclamata, sarà il prossimo edificio popolato");
    }
    home
}

pub fn save(home: &NpcHome) {
    let path = Path::new(SAVE_FILE);
    let mut out = String::from("# npc_home: casa degli NPC custom\n");
    if let Some(b) = &home.building {
        out.push_str(&format!("home {}\n", encode(b)));
        for (a, r) in &home.npc_flats {
            out.push_str(&format!("flat {} {}\n", a, r));
        }
    }
    if let Err(e) = std::fs::write(path, out) {
        warn!("npc_home: impossibile scrivere {}: {e}", path.display());
    }
}

// ---------------------------------------------------------------------------
// Sistema: rilascio automatico quando tutti gli NPC sono reclutati
// ---------------------------------------------------------------------------

/// Se la casa è occupata ma tutti i suoi flat sono già stati reclutati
/// (via dialogo LLM in `dialog.rs` → `apply_recruit`), libera la casa e salva
/// il nuovo stato. Gira ogni frame: se non c'è niente da fare, costa solo un
/// paio di confronti.
pub fn release_home_when_empty(
    interior: Res<InteriorState>,
    mut home: ResMut<NpcHome>,
) {
    // Se la casa non è reclamata, non c'è niente da fare.
    if !home.is_claimed() {
        return;
    }
    if maybe_release_home(&mut home, &interior) {
        save(&home);
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct NpcHomePlugin;

impl Plugin for NpcHomePlugin {
    fn build(&self, app: &mut App) {
        let path = Path::new(SAVE_FILE);
        app.insert_resource(load(path))
        .add_systems(Update, release_home_when_empty);
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn home_with(building: Option<&str>, flats: &[(u16, u8)]) -> NpcHome {
        NpcHome {
            building: building.map(str::to_string),
            npc_flats: flats.to_vec(),
        }
    }

    #[test]
    fn empty_interior_cannot_be_home() {
        let mut h = NpcHome::default();
        assert!(!claim_home_on_enter(&mut h, "x", &[]));
        assert!(!h.is_claimed());
    }

    #[test]
    fn first_populated_building_becomes_home() {
        let mut h = NpcHome::default();
        assert!(claim_home_on_enter(&mut h, "A", &[(1, 0), (2, 0)]));
        assert_eq!(h.building.as_deref(), Some("A"));
        assert_eq!(h.npc_flats, vec![(1, 0), (2, 0)]);
    }

    #[test]
    fn second_building_does_not_replace_home() {
        let mut h = home_with(Some("A"), &[(1, 0)]);
        assert!(!claim_home_on_enter(&mut h, "B", &[(3, 0)]));
        assert_eq!(h.building.as_deref(), Some("A"));
    }

    #[test]
    fn reentering_home_returns_true() {
        let mut h = home_with(Some("A"), &[(1, 0)]);
        assert!(claim_home_on_enter(&mut h, "A", &[(1, 0)]));
        assert_eq!(h.building.as_deref(), Some("A"));
    }

    #[test]
    fn home_releases_when_all_recruited() {
        let mut h = home_with(Some("A"), &[(1, 0), (2, 0)]);
        let mut interior = InteriorState::default();
        interior.mark_recruited(1, 0);
        assert!(!maybe_release_home(&mut h, &interior));
        assert!(h.is_claimed());

        interior.mark_recruited(2, 0);
        assert!(maybe_release_home(&mut h, &interior));
        assert!(!h.is_claimed());
        assert!(h.npc_flats.is_empty());
    }

    #[test]
    fn releasing_an_empty_home_is_no_op() {
        let mut h = NpcHome::default();
        let interior = InteriorState::default();
        assert!(!maybe_release_home(&mut h, &interior));
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("npc_home_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("npc_home.txt");

        let h = home_with(Some("207,213:3,4,1"), &[(175, 0), (183, 2), (334, 19)]);
        // Serializza a mano come fa `save`, poi ricarica con `load`.
        let mut text = String::new();
        text.push_str(&format!("home {}\n", encode(h.building.as_deref().unwrap())));
        for (a, r) in &h.npc_flats {
            text.push_str(&format!("flat {} {}\n", a, r));
        }
        std::fs::write(&path, &text).unwrap();
        let back = load(&path);
        assert_eq!(back.building, h.building);
        assert_eq!(back.npc_flats, h.npc_flats);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn encode_decode_preserves_special_chars() {
        let s = "207,213:3,4,1 with spaces%and stuff";
        assert_eq!(decode(&encode(s)), s);
    }

    #[test]
    fn empty_file_gives_default_home() {
        let dir = std::env::temp_dir().join(format!("npc_home_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("npc_home.txt");
        let h = load(&path);
        assert!(!h.is_claimed());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
