//! Oggetti di quest: certificato di morte, lettere, ricompensa magica.
//!
//! Stato (`QuestItems`, salvato in `quest_items.txt`):
//!   - `placed`: oggetti piazzati dal QBN in un luogo (`place item X at _ukcrypt_`), ancora da trovare;
//!   - `held`: oggetti che il giocatore ha (`give pc X`, oppure raccolti).
//!
//! `PlayerInventory` non cambia struttura: gli oggetti in mano compaiono nello zaino con un id
//! alto (`>= DOC_BASE`) e `sync_inventory` li tiene allineati con `held`.
//!   - documenti (lettere, certificato): id in `DOC_BASE..REWARD_BASE`. Non si equipaggiano:
//!     cliccarli li LEGGE (vedi `equipment_patch.md`), e il testo va nel giornale (J);
//!   - ricompensa (`magic_item`): id `>= REWARD_BASE`, un anello equipaggiabile come gli altri.
//!
//! Le condizioni del QBN (`toting deathCertificate and _lhotun_ clicked`) leggono i simboli in
//! mano tramite `QbnWorldInfo.items`, aggiornato da `main_quest`.
//!
//! RACCOLTA NEL DUNGEON: questo modulo non sa spawnare il pickup 3D (serve `dungeon.rs`).
//! Il punto di aggancio e' `QuestItems::pending_pickups_for(lib, states, nome_luogo)` (cosa
//! va messo in quel dungeon) piu' l'evento `QuestItemPickup { symbol }` (da inviare quando il
//! giocatore lo raccoglie).
//!
//! In `main.rs`: `mod quest_items;` e `.add_plugins(quest_items::QuestItemsPlugin)`.

use crate::dfquest::{Library, QuestStates};
use crate::equipment::{EquipSlot, InventoryItem, PlayerInventory};
use crate::main_quest::{QbnEffectEvent, QbnInput};
use crate::qbn::Effect;
use crate::{QuestLibRes, QuestStatesRes};
use bevy::prelude::*;
use std::collections::HashSet;

const ITEMS_FILE: &str = "quest_items.txt";

/// Primo id riservato agli oggetti di quest nell'inventario (gli altri sono piccoli).
pub const DOC_BASE: u32 = 1_000_000;
/// Da qui in su: ricompense equipaggiabili.
pub const REWARD_BASE: u32 = 1_500_000;

pub fn is_quest_item(id: u32) -> bool {
    id >= DOC_BASE
}

/// Documenti: si leggono, non si equipaggiano.
pub fn is_quest_document(id: u32) -> bool {
    (DOC_BASE..REWARD_BASE).contains(&id)
}

// =============================================================================
// Dati
// =============================================================================

#[derive(Clone, Debug, PartialEq)]
pub struct HeldItem {
    pub quest: String,
    pub symbol: String,
    pub kind: String,
    pub id: u32,
    /// `give pc` / ricompensa / `make X permanent`: resta a quest finita. Gli altri (`get item`,
    /// oggetti raccolti) si tolgono quando la quest finisce.
    pub permanent: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlacedItem {
    pub quest: String,
    pub symbol: String,
    /// Simbolo QBN del luogo (`_ukcrypt_`).
    pub place: String,
    pub kind: String,
}

#[derive(Resource, Default, Debug)]
pub struct QuestItems {
    pub held: Vec<HeldItem>,
    pub placed: Vec<PlacedItem>,
    next: u32,
}

/// Il giocatore ha raccolto un oggetto piazzato (da inviare dal codice del dungeon).
#[derive(Event, Debug, Clone)]
pub struct QuestItemPickup {
    pub symbol: String,
}

/// Il giocatore ha cliccato un oggetto di quest nello zaino (id dell'inventario).
#[derive(Event, Debug, Clone)]
pub struct QuestItemUse(pub u32);

/// `deathCertificate` -> "Death certificate", `letter38` -> "Letter", magic_item -> "Magic ring".
pub fn display_name(symbol: &str, kind: &str) -> String {
    if kind.eq_ignore_ascii_case("magic_item") {
        return "Magic ring".to_string();
    }
    let clean: String = symbol.trim_matches('_').chars().filter(|c| !c.is_ascii_digit()).collect();
    let mut out = String::new();
    let mut prev_lower = false;
    for c in clean.chars() {
        if c.is_uppercase() && prev_lower {
            out.push(' ');
        }
        out.extend(c.to_lowercase());
        prev_lower = c.is_lowercase();
    }
    let mut chars = out.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Quest item".to_string(),
    }
}

pub fn display_description(kind: &str) -> String {
    if kind.eq_ignore_ascii_case("magic_item") {
        "A token of gratitude for services rendered.".to_string()
    } else {
        "A document tied to a quest. Click to read it.".to_string()
    }
}

/// Parole che non distinguono un luogo da un altro ("Castle Faallem" ~ "Faallem").
const GENERIC_WORDS: &[&str] = &[
    "castle", "fortress", "temple", "crypt", "tower", "keep", "ruins", "ruin", "cave", "caves", "lair",
    "mine", "prison", "dungeon", "vault", "stronghold", "citadel", "of", "the",
];

fn distinctive_words(s: &str) -> Vec<String> {
    let mut v: Vec<String> = s
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .filter(|w| !GENERIC_WORDS.contains(&w.as_str()))
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Stesso luogo? Ignora maiuscole, spazi, underscore e l'ordine delle parole, e non conta le
/// parole generiche: "Fortress Fhojum" ~ "Fhojum" ~ "Fhojum Fortress". "Sentinel" NON e'
/// "Sentinel Pass" (le parole distintive devono coincidere).
pub fn same_place(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let (na, nb) = (norm(a), norm(b));
    if na.is_empty() || nb.is_empty() {
        return false;
    }
    if na == nb {
        return true;
    }
    let (ka, kb) = (distinctive_words(a), distinctive_words(b));
    if !ka.is_empty() && !kb.is_empty() {
        return ka == kb;
    }
    // una delle due ha solo parole generiche: contenimento, con un minimo di lunghezza
    na.len() >= 5 && nb.len() >= 5 && (na.contains(&nb) || nb.contains(&na))
}

impl QuestItems {
    fn alloc_id(&mut self, kind: &str) -> u32 {
        self.next += 1;
        if kind.eq_ignore_ascii_case("magic_item") {
            REWARD_BASE + self.next
        } else {
            DOC_BASE + self.next
        }
    }

    pub fn has(&self, symbol: &str) -> bool {
        self.held.iter().any(|h| h.symbol == symbol)
    }

    pub fn symbols(&self) -> HashSet<String> {
        self.held.iter().map(|h| h.symbol.clone()).collect()
    }

    pub fn find_by_id(&self, id: u32) -> Option<&HeldItem> {
        self.held.iter().find(|h| h.id == id)
    }

    /// Mette l'oggetto in mano, NON permanente. `false` se lo era gia'.
    pub fn give(&mut self, quest: &str, symbol: &str, kind: &str) -> bool {
        self.give_with(quest, symbol, kind, false)
    }

    pub fn give_with(&mut self, quest: &str, symbol: &str, kind: &str, permanent: bool) -> bool {
        if self.has(symbol) {
            // gia' in mano: al piu' lo si rende permanente
            if permanent {
                self.set_permanent(quest, symbol);
            }
            return false;
        }
        let id = self.alloc_id(kind);
        self.held.push(HeldItem {
            quest: quest.to_string(),
            symbol: symbol.to_string(),
            kind: kind.to_string(),
            id,
            permanent,
        });
        true
    }

    /// `make X permanent`.
    pub fn set_permanent(&mut self, quest: &str, symbol: &str) {
        for h in self.held.iter_mut().filter(|h| h.quest == quest && h.symbol == symbol) {
            h.permanent = true;
        }
    }

    /// La quest e' finita: via gli oggetti non permanenti (in mano o ancora da trovare).
    pub fn end_quest(&mut self, quest: &str) {
        self.held.retain(|h| h.quest != quest || h.permanent);
        self.placed.retain(|p| p.quest != quest);
    }

    /// Piazza un oggetto in un luogo, da trovare.
    pub fn place(&mut self, quest: &str, symbol: &str, place: &str, kind: &str) {
        if self.has(symbol) || self.placed.iter().any(|p| p.symbol == symbol) {
            return;
        }
        self.placed.push(PlacedItem {
            quest: quest.to_string(),
            symbol: symbol.to_string(),
            place: place.to_string(),
            kind: kind.to_string(),
        });
    }

    /// Raccolta: l'oggetto passa da `placed` a `held`. `None` se non era piazzato.
    pub fn pick_up(&mut self, symbol: &str) -> Option<HeldItem> {
        let i = self.placed.iter().position(|p| p.symbol == symbol)?;
        let p = self.placed.remove(i);
        self.give(&p.quest, &p.symbol, &p.kind);
        self.held.iter().find(|h| h.symbol == symbol).cloned()
    }

    /// Per ogni oggetto ancora da trovare: (nome dell'oggetto, nome del luogo scelto dalla quest).
    /// Serve ai messaggi e ai log: "il Death certificate si trova a Fortress Fhojum".
    pub fn pending_places(&self, lib: &Library, states: &QuestStates) -> Vec<(String, String)> {
        self.placed
            .iter()
            .map(|p| (display_name(&p.symbol, &p.kind), lib.expand(&p.quest, states, &p.place)))
            .collect()
    }

    /// Simboli degli oggetti che vanno messi nel luogo che si chiama `location`
    /// (es. "Castle Faallem"): quello scelto all'avvio per `_ukcrypt_`.
    pub fn pending_pickups_for(&self, lib: &Library, states: &QuestStates, location: &str) -> Vec<String> {
        self.placed
            .iter()
            .filter(|p| same_place(&lib.expand(&p.quest, states, &p.place), location))
            .map(|p| p.symbol.clone())
            .collect()
    }

    pub fn to_text(&self) -> String {
        let mut s = String::from("# Oggetti di quest: held/placed (campi separati da TAB)\n");
        s.push_str(&format!("next\t{}\n", self.next));
        for h in &self.held {
            s.push_str(&format!(
                "held\t{}\t{}\t{}\t{}\t{}\n",
                h.quest,
                h.symbol,
                h.id,
                h.kind,
                u8::from(h.permanent)
            ));
        }
        for p in &self.placed {
            s.push_str(&format!("placed\t{}\t{}\t{}\t{}\n", p.quest, p.symbol, p.place, p.kind));
        }
        s
    }

    pub fn from_text(text: &str) -> QuestItems {
        let mut q = QuestItems::default();
        for line in text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
            let f: Vec<&str> = line.split('\t').collect();
            match f.as_slice() {
                ["next", n] => q.next = n.parse().unwrap_or(0),
                ["held", quest, symbol, id, kind, rest @ ..] => {
                    if let Ok(id) = id.parse::<u32>() {
                        q.held.push(HeldItem {
                            quest: quest.to_string(),
                            symbol: symbol.to_string(),
                            kind: kind.to_string(),
                            id,
                            // salvataggi vecchi (senza il campo): permanente, non si tocca nulla
                            permanent: rest.first().map_or(true, |p| *p == "1"),
                        });
                        q.next = q.next.max(id % DOC_BASE % (REWARD_BASE - DOC_BASE));
                    }
                }
                ["placed", quest, symbol, place, kind] => q.placed.push(PlacedItem {
                    quest: quest.to_string(),
                    symbol: symbol.to_string(),
                    place: place.to_string(),
                    kind: kind.to_string(),
                }),
                _ => {}
            }
        }
        q
    }
}

// =============================================================================
// Plugin e sistemi
// =============================================================================

pub struct QuestItemsPlugin;

impl Plugin for QuestItemsPlugin {
    fn build(&self, app: &mut App) {
        let items = std::fs::read_to_string(ITEMS_FILE)
            .map(|t| QuestItems::from_text(&t))
            .unwrap_or_default();
        app.insert_resource(items)
            .add_event::<QuestItemPickup>()
            .add_event::<QuestItemUse>()
            .add_systems(
                Update,
                (track_item_effects, handle_pickups, handle_item_use, sync_inventory, save_items).chain(),
            );
    }
}

/// `give pc`, `get item`, `place item`, `make X permanent`, oro e fine quest diventano stato.
fn track_item_effects(
    mut events: EventReader<QbnEffectEvent>,
    mut items: ResMut<QuestItems>,
    mut inv: ResMut<PlayerInventory>,
    lib: Res<QuestLibRes>,
) {
    let kind_of = |quest: &str, symbol: &str| {
        lib.0
            .get(quest)
            .and_then(|s| s.items.iter().find(|d| d.symbol == symbol))
            .map(|d| d.kind.clone())
            .unwrap_or_default()
    };
    for QbnEffectEvent(e) in events.read() {
        match e {
            Effect::GiveItem { quest, item, permanent, .. } => {
                let kind = kind_of(quest, item);
                items.give_with(quest, item, &kind, *permanent);
            }
            Effect::GiveGold { amount, .. } => {
                inv.gold = inv.gold.saturating_add(*amount);
                inv.dirty = true;
            }
            Effect::MakePermanent { quest, item } => items.set_permanent(quest, item),
            Effect::PlaceItem { quest, item, place } => {
                let kind = kind_of(quest, item);
                items.place(quest, item, place, &kind);
            }
            Effect::QuestEnded { quest, .. } => items.end_quest(quest),
            _ => {}
        }
    }
}

/// Raccolta: l'oggetto passa in mano e il QBN riceve il clic (`clicked item ... say 1020`).
fn handle_pickups(
    mut pickups: EventReader<QuestItemPickup>,
    mut items: ResMut<QuestItems>,
    mut inputs: EventWriter<QbnInput>,
) {
    for ev in pickups.read() {
        if items.pick_up(&ev.symbol).is_some() {
            inputs.send(QbnInput::ItemClicked(ev.symbol.clone()));
        } else {
            warn!("[quest_items] raccolta di `{}`, ma non era piazzato", ev.symbol);
        }
    }
}

/// Clic nello zaino su un documento -> il QBN produce il testo da leggere.
fn handle_item_use(
    mut uses: EventReader<QuestItemUse>,
    items: Res<QuestItems>,
    mut inputs: EventWriter<QbnInput>,
) {
    for QuestItemUse(id) in uses.read() {
        if let Some(h) = items.find_by_id(*id) {
            inputs.send(QbnInput::ItemUsed { quest: h.quest.clone(), item: h.symbol.clone() });
        }
    }
}

fn make_inventory_item(h: &HeldItem) -> InventoryItem {
    let reward = h.kind.eq_ignore_ascii_case("magic_item");
    // DFU: anello per le reward, pergamena per i documenti.
    // I template DFU sono in ItemTemplates.json (135 = Ring, 279 = Parchment).
    let (slot, template_id) = if reward {
        (EquipSlot::Ring, Some(135))
    } else {
        (EquipSlot::Amulet, Some(279))
    };
    InventoryItem {
        id: h.id,
        name: display_name(&h.symbol, &h.kind),
        slot,
        equipped: false,
        weight_kg: if reward { 0.1 } else { 0.05 },
        armor_rating: 0,
        paperdoll_record: None,
        icon_source: None,
        template_id,
        magic_item_param: None,
        paperdoll_archive_override: None,
        description: display_description(&h.kind),
    }
}

/// Tiene lo zaino allineato con `QuestItems.held`. Parte alla prima esecuzione (la risorsa
/// risulta cambiata) e ad ogni modifica.
fn sync_inventory(items: Res<QuestItems>, mut inv: ResMut<PlayerInventory>) {
    if !items.is_changed() {
        return;
    }
    let wanted: HashSet<u32> = items.held.iter().map(|h| h.id).collect();
    let before = inv.items.len();
    inv.items.retain(|i| !is_quest_item(i.id) || wanted.contains(&i.id));
    let mut changed = inv.items.len() != before;
    for h in &items.held {
        if !inv.items.iter().any(|i| i.id == h.id) {
            inv.items.push(make_inventory_item(h));
            changed = true;
        }
    }
    if changed {
        inv.dirty = true;
    }
}

fn save_items(items: Res<QuestItems>) {
    if items.is_changed() && !items.is_added() {
        if let Err(e) = std::fs::write(ITEMS_FILE, items.to_text()) {
            warn!("Oggetti di quest: impossibile salvare {ITEMS_FILE}: {e}");
        }
    }
}

// =============================================================================
// Test
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_readable() {
        assert_eq!(display_name("deathCertificate", "letter"), "Death certificate");
        assert_eq!(display_name("letter38", "letter"), "Letter");
        assert_eq!(display_name("_magicitem_", "magic_item"), "Magic ring");
        assert_eq!(display_name("_", "letter"), "Quest item");
    }

    #[test]
    fn places_match_loosely() {
        assert!(same_place("Castle_Faallem", "Castle Faallem"));
        assert!(same_place("Castle Faallem", "Faallem"));
        assert!(same_place("  castle   FAALLEM ", "Castle_Faallem"));
        assert!(!same_place("Castle Faallem", "Fortress Fhojum"));
        // ordine delle parole e parole generiche non contano
        assert!(same_place("Fortress Fhojum", "Fhojum Fortress"));
        assert!(same_place("Fortress Fhojum", "FHOJUM"));
        // ma una parola distintiva in piu' e' un altro luogo
        assert!(!same_place("Sentinel", "Sentinel Pass"));
        assert!(!same_place("", "Faallem"));
        assert!(!same_place("Sentinel", "Sen")); // troppo corto per il match parziale
    }

    #[test]
    fn permanence_and_end_of_quest() {
        let mut q = QuestItems::default();
        q.give_with("S2", "letter43", "letter", false); // `get item`
        q.give_with("S2", "ring", "magic_item", true); // `give pc`
        q.give_with("S3", "other", "letter", false); // di un'altra quest
        q.place("S2", "lost", "_x_", "letter");
        q.set_permanent("S2", "letter43"); // `make letter43 permanent`
        q.end_quest("S2");
        assert!(q.has("letter43") && q.has("ring"), "i permanenti restano");
        assert!(q.has("other"), "le altre quest non si toccano");
        assert!(q.placed.is_empty(), "cio' che non e' stato trovato sparisce");
        q.give_with("S4", "tmp", "letter", false);
        q.end_quest("S4");
        assert!(!q.has("tmp"));
    }

    #[test]
    fn old_saves_without_the_permanent_field_keep_their_items() {
        let q = QuestItems::from_text("next\t1\nheld\tS1\tletter38\t1000001\tletter\n");
        assert_eq!(q.held.len(), 1);
        assert!(q.held[0].permanent);
    }

    #[test]
    fn id_ranges() {
        let mut q = QuestItems::default();
        q.give("S1", "deathCertificate", "letter");
        q.give("S1", "_magicitem_", "magic_item");
        let doc = q.held[0].id;
        let reward = q.held[1].id;
        assert!(is_quest_document(doc) && is_quest_item(doc));
        assert!(!is_quest_document(reward) && is_quest_item(reward));
        assert!(!is_quest_item(3)); // gli oggetti normali restano fuori
    }

    #[test]
    fn give_is_idempotent_and_pickup_moves_item() {
        let mut q = QuestItems::default();
        q.place("S1", "deathCertificate", "_ukcrypt_", "letter");
        assert!(!q.has("deathCertificate"));
        let h = q.pick_up("deathCertificate").expect("era piazzato");
        assert!(q.has("deathCertificate"));
        assert!(q.placed.is_empty());
        assert!(!q.give("S1", "deathCertificate", "letter"));
        assert!(q.pick_up("deathCertificate").is_none());
        assert_eq!(q.find_by_id(h.id).map(|x| x.symbol.as_str()), Some("deathCertificate"));
    }

    #[test]
    fn roundtrip_keeps_ids_and_does_not_reuse_them() {
        let mut q = QuestItems::default();
        q.give("S1", "letter38", "letter");
        q.place("S1", "deathCertificate", "_ukcrypt_", "letter");
        let mut back = QuestItems::from_text(&q.to_text());
        assert_eq!(back.held, q.held);
        assert_eq!(back.placed, q.placed);
        back.give("S1", "other", "letter");
        let ids: HashSet<u32> = back.held.iter().map(|h| h.id).collect();
        assert_eq!(ids.len(), back.held.len(), "id duplicati dopo il reload");
    }
}
