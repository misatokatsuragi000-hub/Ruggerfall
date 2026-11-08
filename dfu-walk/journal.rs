//! Giornale di quest (tasto J) e reputazione rudimentale.
//!
//! UI nativa Bevy 0.15 nello stile di `equipment.rs` (stessa palette, stessa gestione del
//! cursore, stesso modo di bloccare i tasti mentre e' aperta). Niente egui: `bevy_egui` per
//! Bevy 0.15 non arriva a egui 0.33 e il resto del progetto usa gia' la UI nativa.
//!
//! Contenuto del giornale:
//!   - Quest attive: titolo + annotazioni (`log N step S` del QBN) ordinate per step.
//!   - Quest concluse: completate / fallite.
//!   - Reputazione: legale (ricercato) e per persona, con un'etichetta (Neutrale, Amico, ...).
//!
//! Salvataggio: `quest_journal.txt` e `reputation.txt`, scritti quando cambiano.
//!
//! Controlli: [J] apri/chiudi - [Esc] chiudi - rotella / PagSu / PagGiu / Home / Fine per scorrere.
//!
//! In `main.rs`: `mod journal;` e `.add_plugins(journal::JournalPlugin)`.

use crate::dfquest::{pretty_name, Library, QuestPhase, QuestStates};
use crate::dialog::DialogState;
use crate::equipment::EquipmentState;
use crate::{QuestLibRes, QuestStatesRes};
use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::ui::ScrollPosition;
use bevy::window::{CursorGrabMode, PrimaryWindow};
use std::collections::BTreeMap;

const JOURNAL_FILE: &str = "quest_journal.txt";
const REPUTATION_FILE: &str = "reputation.txt";

// Palette di equipment.rs
const OVERLAY: Color = Color::srgba(0.02, 0.015, 0.01, 0.45);
const PANEL: Color = Color::srgba(0.10, 0.075, 0.055, 0.92);
const PANEL_BORDER: Color = Color::srgba(0.75, 0.58, 0.28, 0.50);
const CARD: Color = Color::srgba(0.12, 0.09, 0.065, 0.75);
const ACCENT: Color = Color::srgb(0.96, 0.78, 0.32);
const GOOD: Color = Color::srgb(0.40, 0.88, 0.45);
const BAD: Color = Color::srgb(0.92, 0.45, 0.40);
const TEXT: Color = Color::srgb(0.96, 0.93, 0.85);
const MUTED: Color = Color::srgb(0.74, 0.67, 0.54);

const PANEL_WIDTH: f32 = 780.0;
const SCROLL_MAX_VH: f32 = 62.0;
const WHEEL_STEP: f32 = 40.0;
const PAGE_STEP: f32 = 360.0;

// =============================================================================
// Dati: giornale
// =============================================================================

#[derive(Debug, Clone, PartialEq)]
pub struct JournalEntry {
    pub quest: String,
    pub step: u32,
    pub text: String,
}

/// Documento letto dal giocatore (lettera, certificato...): titolo e paragrafi.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub title: String,
    pub paragraphs: Vec<String>,
}

/// Annotazioni delle quest attive (una per quest+step, come `log N step S`) e documenti letti.
#[derive(Resource, Default, Debug)]
pub struct QuestJournal {
    pub entries: Vec<JournalEntry>,
    pub documents: Vec<Document>,
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl QuestJournal {
    pub fn set(&mut self, quest: &str, step: u32, text: &str) {
        self.entries.retain(|e| !(e.quest == quest && e.step == step));
        self.entries.push(JournalEntry { quest: quest.to_string(), step, text: one_line(text) });
        self.entries.sort_by(|a, b| (&a.quest, a.step).cmp(&(&b.quest, b.step)));
    }

    pub fn remove(&mut self, quest: &str, step: u32) {
        self.entries.retain(|e| !(e.quest == quest && e.step == step));
    }

    pub fn clear_quest(&mut self, quest: &str) {
        self.entries.retain(|e| e.quest != quest);
    }

    /// Aggiunge (o sostituisce, se c'e' gia' un documento con lo stesso titolo) un documento.
    /// `true` se e' nuovo.
    pub fn add_document(&mut self, title: &str, paragraphs: Vec<String>) -> bool {
        let doc = Document { title: title.to_string(), paragraphs };
        match self.documents.iter_mut().find(|d| d.title == title) {
            Some(d) => {
                *d = doc;
                false
            }
            None => {
                self.documents.push(doc);
                true
            }
        }
    }

    pub fn to_text(&self) -> String {
        let mut s = String::from(
            "# Giornale: entry<TAB>quest<TAB>step<TAB>testo / doc<TAB>titolo<TAB>paragrafi (\\n\\n)\n",
        );
        for e in &self.entries {
            s.push_str(&format!("entry\t{}\t{}\t{}\n", e.quest, e.step, esc(&e.text)));
        }
        for d in &self.documents {
            s.push_str(&format!("doc\t{}\t{}\n", esc(&d.title), esc(&d.paragraphs.join("\n\n"))));
        }
        s
    }

    pub fn from_text(text: &str) -> QuestJournal {
        let mut j = QuestJournal::default();
        for line in text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
            let p: Vec<&str> = line.splitn(4, '\t').collect();
            match p.as_slice() {
                ["entry", quest, step, body] => {
                    if let Ok(step) = step.parse::<u32>() {
                        j.set(quest, step, &unesc(body));
                    }
                }
                ["doc", title, body] => {
                    let paras = unesc(body).split("\n\n").map(str::to_string).collect();
                    j.add_document(&unesc(title), paras);
                }
                _ => {}
            }
        }
        j
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('\t', "\\t")
}

fn unesc(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

// =============================================================================
// Dati: reputazione
// =============================================================================

pub const REP_MIN: i32 = -100;
pub const REP_MAX: i32 = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct RepEntry {
    pub name: String,
    pub value: i32,
}

/// Reputazione rudimentale: un valore -100..=100 per persona e uno legale.
/// Le persone con `named` nello script sono identificate dal nome (valgono in tutte le quest);
/// le altre da `quest:simbolo`.
#[derive(Resource, Default, Debug, Clone)]
pub struct Reputation {
    pub people: BTreeMap<String, RepEntry>,
    pub legal: i32,
}

impl Reputation {
    pub fn get(&self, key: &str) -> i32 {
        self.people.get(key).map_or(0, |e| e.value)
    }

    /// Somma `delta` (con limiti) e restituisce il nuovo valore.
    pub fn add(&mut self, key: &str, name: &str, delta: i32) -> i32 {
        let e = self
            .people
            .entry(key.to_string())
            .or_insert_with(|| RepEntry { name: name.to_string(), value: 0 });
        e.value = (e.value + delta).clamp(REP_MIN, REP_MAX);
        e.value
    }

    pub fn add_legal(&mut self, delta: i32) -> i32 {
        self.legal = (self.legal + delta).clamp(REP_MIN, REP_MAX);
        self.legal
    }

    pub fn standing(v: i32) -> &'static str {
        match v {
            i32::MIN..=-60 => "Nemico giurato",
            -59..=-30 => "Ostile",
            -29..=-10 => "Diffidente",
            -9..=9 => "Neutrale",
            10..=29 => "Cordiale",
            30..=59 => "Amico",
            _ => "Alleato",
        }
    }

    pub fn legal_standing(v: i32) -> &'static str {
        match v {
            i32::MIN..=-60 => "Fuorilegge",
            -59..=-30 => "Ricercato",
            -29..=-10 => "Sospetto",
            -9..=9 => "Incensurato",
            _ => "Cittadino stimato",
        }
    }

    pub fn to_text(&self) -> String {
        let mut s = String::from("# Reputazione: rep<TAB>chiave<TAB>nome<TAB>valore / legal<TAB>valore\n");
        s.push_str(&format!("legal\t{}\n", self.legal));
        for (k, e) in &self.people {
            s.push_str(&format!("rep\t{k}\t{}\t{}\n", e.name, e.value));
        }
        s
    }

    pub fn from_text(text: &str) -> Reputation {
        let mut r = Reputation::default();
        for line in text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')) {
            let p: Vec<&str> = line.split('\t').collect();
            match p.as_slice() {
                ["legal", v] => r.legal = v.parse().unwrap_or(0),
                ["rep", k, name, v] => {
                    if let Ok(v) = v.parse::<i32>() {
                        r.people.insert(
                            k.to_string(),
                            RepEntry { name: name.to_string(), value: v.clamp(REP_MIN, REP_MAX) },
                        );
                    }
                }
                _ => {}
            }
        }
        r
    }
}

/// (chiave, nome da mostrare) per una persona di una quest.
pub fn rep_key(lib: &Library, states: &QuestStates, quest: &str, symbol: &str) -> (String, String) {
    let def = lib.get(quest).and_then(|s| s.persons.iter().find(|p| p.symbol == symbol));
    match def.and_then(|d| d.name.as_deref()) {
        Some(n) => (n.to_string(), pretty_name(n)),
        None => {
            let shown = lib.expand(quest, states, symbol);
            let shown = if shown.starts_with('_') { symbol.trim_matches('_').to_string() } else { shown };
            (format!("{quest}:{symbol}"), shown)
        }
    }
}

// =============================================================================
// UI
// =============================================================================

#[derive(Resource, Default)]
pub struct JournalState {
    pub open: bool,
    was_locked: bool,
    dirty: bool,
}

#[derive(Component)]
struct JournalRoot;
#[derive(Component)]
struct JournalScroll;
#[derive(Component)]
struct JournalContent;

pub struct JournalPlugin;

impl Plugin for JournalPlugin {
    fn build(&self, app: &mut App) {
        let journal = std::fs::read_to_string(JOURNAL_FILE)
            .map(|t| QuestJournal::from_text(&t))
            .unwrap_or_default();
        let rep = std::fs::read_to_string(REPUTATION_FILE)
            .map(|t| Reputation::from_text(&t))
            .unwrap_or_default();
        app.insert_resource(journal)
            .insert_resource(rep)
            .init_resource::<JournalState>()
            .add_systems(Startup, setup_journal_ui)
            .add_systems(
                Update,
                (journal_input, rebuild_journal)
                    .chain()
                    // stesso ordine di equipment.rs; prima di lui, cosi' se il giornale e' aperto
                    // gli nega i tasti (C, Esc) con `reset_all`.
                    .before(crate::equipment::toggle_equipment)
                    .before(crate::dialog::update_dialog)
                    .before(crate::player::grab_cursor),
            )
            .add_systems(Update, save_state);
    }
}

fn setup_journal_ui(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(OVERLAY),
            GlobalZIndex(91),
            Visibility::Hidden,
            JournalRoot,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    width: Val::Px(PANEL_WIDTH),
                    max_width: Val::Percent(94.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(20.0)),
                    row_gap: Val::Px(12.0),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(PANEL),
                BorderColor(PANEL_BORDER),
                BorderRadius::all(Val::Px(18.0)),
            ))
            .with_children(|panel| {
                panel
                    .spawn(Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(3.0), ..default() })
                    .with_children(|title| {
                        title.spawn((
                            Text::new("Giornale"),
                            TextFont { font_size: 24.0, ..default() },
                            TextColor(TEXT),
                        ));
                        title.spawn((
                            Node { width: Val::Px(40.0), height: Val::Px(2.5), ..default() },
                            BackgroundColor(ACCENT),
                            BorderRadius::all(Val::Px(2.0)),
                        ));
                    });
                panel.spawn((
                    Text::new("[J / Esc] Chiudi · [Rotella / PagSu / PagGiu / Home / Fine] Scorri"),
                    TextFont { font_size: 12.0, ..default() },
                    TextColor(MUTED),
                ));
                panel
                    .spawn((
                        Node {
                            flex_direction: FlexDirection::Column,
                            width: Val::Percent(100.0),
                            max_height: Val::Vh(SCROLL_MAX_VH),
                            overflow: Overflow::scroll_y(),
                            padding: UiRect::right(Val::Px(12.0)),
                            ..default()
                        },
                        ScrollPosition::default(),
                        JournalScroll,
                    ))
                    .with_children(|scroll| {
                        scroll.spawn((
                            Node {
                                flex_direction: FlexDirection::Column,
                                width: Val::Percent(100.0),
                                row_gap: Val::Px(10.0),
                                ..default()
                            },
                            JournalContent,
                        ));
                    });
            });
        });
}

#[allow(clippy::too_many_arguments)]
fn journal_input(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut state: ResMut<JournalState>,
    dialog: Res<DialogState>,
    equipment: Res<EquipmentState>,
    mut wheel: EventReader<MouseWheel>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut root: Query<&mut Visibility, With<JournalRoot>>,
    mut scroll: Query<&mut ScrollPosition, With<JournalScroll>>,
) {
    let toggle = if state.open {
        keys.just_pressed(KeyCode::KeyJ) || keys.just_pressed(KeyCode::Escape)
    } else {
        keys.just_pressed(KeyCode::KeyJ) && !dialog.open && !equipment.open
    };

    if toggle {
        let open = !state.open;
        state.open = open;
        state.dirty = true;
        if let Ok(mut window) = windows.get_single_mut() {
            if open {
                state.was_locked = window.cursor_options.grab_mode == CursorGrabMode::Locked;
                window.cursor_options.grab_mode = CursorGrabMode::None;
                window.cursor_options.visible = true;
            } else if state.was_locked {
                window.cursor_options.grab_mode = CursorGrabMode::Locked;
                window.cursor_options.visible = false;
            }
        }
        for mut v in &mut root {
            *v = if open { Visibility::Visible } else { Visibility::Hidden };
        }
        for mut sp in &mut scroll {
            sp.offset_y = 0.0;
        }
    }

    if state.open && !toggle {
        let mut delta = wheel.read().map(|e| -e.y * WHEEL_STEP).sum::<f32>();
        if keys.just_pressed(KeyCode::PageDown) {
            delta += PAGE_STEP;
        }
        if keys.just_pressed(KeyCode::PageUp) {
            delta -= PAGE_STEP;
        }
        let home = keys.just_pressed(KeyCode::Home);
        let end = keys.just_pressed(KeyCode::End);
        for mut sp in &mut scroll {
            if home {
                sp.offset_y = 0.0;
            } else if end {
                sp.offset_y = f32::MAX;
            } else {
                sp.offset_y = (sp.offset_y + delta).max(0.0);
            }
        }
    } else {
        wheel.clear();
    }

    if toggle || state.open {
        keys.reset_all();
    }
}

fn rebuild_journal(
    mut commands: Commands,
    mut state: ResMut<JournalState>,
    journal: Res<QuestJournal>,
    rep: Res<Reputation>,
    lib: Res<QuestLibRes>,
    states: Res<QuestStatesRes>,
    content: Query<Entity, With<JournalContent>>,
) {
    if !state.open {
        return;
    }
    if !(state.dirty || journal.is_changed() || rep.is_changed()) {
        return;
    }
    state.dirty = false;
    let Ok(content) = content.get_single() else { return };
    commands.entity(content).despawn_descendants();
    commands
        .entity(content)
        .with_children(|c| build_content(c, &journal, &rep, &lib.0, &states.0));
}

fn text_line(c: &mut ChildBuilder, s: impl Into<String>, size: f32, color: Color) {
    c.spawn((Text::new(s), TextFont { font_size: size, ..default() }, TextColor(color)));
}

fn heading(c: &mut ChildBuilder, s: &str) {
    text_line(c, s, 20.0, ACCENT);
}

fn card(c: &mut ChildBuilder, f: impl FnOnce(&mut ChildBuilder)) {
    c.spawn((
        Node {
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(Val::Px(12.0)),
            row_gap: Val::Px(6.0),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        BackgroundColor(CARD),
        BorderColor(PANEL_BORDER),
        BorderRadius::all(Val::Px(10.0)),
    ))
    .with_children(f);
}

fn standing_color(v: i32) -> Color {
    match v {
        i32::MIN..=-10 => BAD,
        10..=i32::MAX => GOOD,
        _ => TEXT,
    }
}

fn build_content(
    c: &mut ChildBuilder,
    journal: &QuestJournal,
    rep: &Reputation,
    lib: &Library,
    states: &QuestStates,
) {
    let title_of = |quest: &str| {
        lib.get(quest)
            .map(|s| s.display_name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| quest.to_string())
    };

    // ---- quest attive
    heading(c, "Quest attive");
    let mut by_quest: BTreeMap<&str, Vec<&JournalEntry>> = BTreeMap::new();
    for e in &journal.entries {
        by_quest.entry(e.quest.as_str()).or_default().push(e);
    }
    if by_quest.is_empty() {
        text_line(c, "Nessuna quest attiva.", 14.0, MUTED);
    }
    for (quest, mut entries) in by_quest {
        entries.sort_by_key(|e| e.step);
        let title = title_of(quest);
        card(c, |card| {
            text_line(card, title, 18.0, TEXT);
            for e in entries {
                text_line(card, e.text.clone(), 14.0, MUTED);
            }
        });
    }

    // ---- quest concluse
    let mut finished: Vec<(String, bool)> = lib
        .scripts()
        .filter_map(|s| match states.phase(&s.id) {
            QuestPhase::Completed => Some((title_of(&s.id), true)),
            QuestPhase::Failed => Some((title_of(&s.id), false)),
            _ => None,
        })
        .collect();
    finished.sort();
    if !finished.is_empty() {
        heading(c, "Quest concluse");
        for (title, ok) in finished {
            let (label, color) = if ok { ("completata", GOOD) } else { ("fallita", BAD) };
            text_line(c, format!("{title} — {label}"), 14.0, color);
        }
    }

    // ---- documenti letti
    if !journal.documents.is_empty() {
        heading(c, "Documenti");
        for d in &journal.documents {
            card(c, |card| {
                text_line(card, d.title.clone(), 18.0, TEXT);
                for para in &d.paragraphs {
                    text_line(card, para.clone(), 14.0, MUTED);
                }
            });
        }
    }

    // ---- reputazione
    heading(c, "Reputazione");
    text_line(
        c,
        format!("Legale: {} ({:+})", Reputation::legal_standing(rep.legal), rep.legal),
        14.0,
        standing_color(rep.legal),
    );
    if rep.people.is_empty() {
        text_line(c, "Nessuna reputazione particolare.", 14.0, MUTED);
    }
    let mut people: Vec<&RepEntry> = rep.people.values().collect();
    people.sort_by(|a, b| b.value.cmp(&a.value).then_with(|| a.name.cmp(&b.name)));
    for e in people {
        text_line(
            c,
            format!("{}: {} ({:+})", e.name, Reputation::standing(e.value), e.value),
            14.0,
            standing_color(e.value),
        );
    }
}

fn save_state(journal: Res<QuestJournal>, rep: Res<Reputation>) {
    if journal.is_changed() && !journal.is_added() {
        if let Err(e) = std::fs::write(JOURNAL_FILE, journal.to_text()) {
            warn!("Giornale: impossibile salvare {JOURNAL_FILE}: {e}");
        }
    }
    if rep.is_changed() && !rep.is_added() {
        if let Err(e) = std::fs::write(REPUTATION_FILE, rep.to_text()) {
            warn!("Reputazione: impossibile salvare {REPUTATION_FILE}: {e}");
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
    fn journal_set_replaces_and_orders() {
        let mut j = QuestJournal::default();
        j.set("S0000001", 1, "secondo");
        j.set("S0000001", 0, "primo\n  su due righe");
        j.set("S0000001", 1, "secondo, aggiornato");
        assert_eq!(j.entries.len(), 2);
        assert_eq!(j.entries[0].text, "primo su due righe");
        assert_eq!(j.entries[1].text, "secondo, aggiornato");
        j.remove("S0000001", 0);
        assert_eq!(j.entries.len(), 1);
        j.clear_quest("S0000001");
        assert!(j.entries.is_empty());
    }

    #[test]
    fn documents_roundtrip_with_paragraphs() {
        let mut j = QuestJournal::default();
        assert!(j.add_document("Letter", vec!["Primo paragrafo.".into(), "Secondo\tcon tab.".into()]));
        assert!(!j.add_document("Letter", vec!["Solo uno.".into()]));
        j.add_document("Death certificate", vec!["A".into(), "B".into(), "C".into()]);
        let back = QuestJournal::from_text(&j.to_text());
        assert_eq!(back.documents, j.documents);
        assert_eq!(back.documents[0].paragraphs, vec!["Solo uno.".to_string()]);
    }

    #[test]
    fn journal_roundtrip_keeps_special_chars() {
        let mut j = QuestJournal::default();
        j.set("Q1", 0, "tab\there \\ barra");
        let back = QuestJournal::from_text(&j.to_text());
        assert_eq!(back.entries, j.entries);
    }

    #[test]
    fn reputation_clamps_and_labels() {
        let mut r = Reputation::default();
        assert_eq!(r.add("Prince_Lhotun", "Prince Lhotun", 15), 15);
        assert_eq!(r.add("Prince_Lhotun", "Prince Lhotun", 500), REP_MAX);
        assert_eq!(r.add_legal(-20), -20);
        assert_eq!(r.add_legal(-500), REP_MIN);
        assert_eq!(Reputation::standing(0), "Neutrale");
        assert_eq!(Reputation::standing(15), "Cordiale");
        assert_eq!(Reputation::standing(-100), "Nemico giurato");
        assert_eq!(Reputation::legal_standing(-20), "Sospetto");
        assert_eq!(r.get("sconosciuto"), 0);
    }

    #[test]
    fn reputation_roundtrip() {
        let mut r = Reputation::default();
        r.add("Queen_Akorithi", "Queen Akorithi", -20);
        r.add("S0000001:_agentuk_", "Cassim", 25);
        r.add_legal(-20);
        let back = Reputation::from_text(&r.to_text());
        assert_eq!(back.legal, -20);
        assert_eq!(back.people, r.people);
    }
}
