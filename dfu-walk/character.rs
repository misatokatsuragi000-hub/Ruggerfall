//! Classi, attributi e abilità alla Daggerfall.
//!
//! ## Modello
//!
//! * **8 attributi** (Forza, Intelligenza, Volontà, Agilità, Costituzione,
//!   Personalità, Velocità, Fortuna), 50 di base + punti bonus da spendere.
//! * **35 abilità**, divise per classe in 1 primaria, 3 maggiori, 6 minori e
//!   25 "varie" (come in Daggerfall). Le abilità salgono **usandole**: ogni uso
//!   (`SkillUsed`) aumenta un tally; raggiunta la soglia l'abilità sale di 1.
//! * **Livelli**: ogni aumento di abilità dà punti di avanzamento (più per le
//!   abilità di classe); raggiunta la soglia si sale di livello e si ricevono
//!   punti attributo da assegnare.
//! * **Statistiche derivate** (`DerivedStats`, risorsa aggiornata ogni frame):
//!   PV massimi, magicka, fatica, bonus danno / colpire, carico massimo. Gli
//!   altri moduli (vitals, combat, equipment) devono leggere da qui.
//!
//! ## Dati delle classi
//!
//! Le 18 classi sono nel testo `DEFAULT_CLASSES_TXT`, **con valori
//! indicativi** (non estratti dai file originali CLASS*.CFG). Per correggerle
//! senza ricompilare crea `assets/classes.txt` con lo stesso formato: se il
//! file esiste sostituisce interamente il catalogo di default.
//!
//! ## Integrazione
//!
//! 1. In `main.rs`: `mod character;` e, **dopo** `tuning_menu::TuningMenuPlugin`,
//!    `.add_plugins(character::CharacterPlugin)`. Il plugin aggiunge
//!    `EguiPlugin` solo se non c'è già.
//! 2. Per far salire un'abilità da un altro modulo:
//!    `ev.send(character::SkillUsed { skill: character::Skill::LongBlade, amount: 1 })`
//!    con `EventWriter<character::SkillUsed>`.
//! 3. Per far salire di livello dopo il riposo (se `LEVEL_REQUIRES_REST`):
//!    `EventWriter<character::CharacterRested>`.
//! 4. Alla prima partenza (nessun `character.sav`) si apre la **creazione del
//!    personaggio** (UI egui, gestita da questo modulo). Dopo la creazione:
//!    - **C** = equipaggiamento (`equipment.rs`, UI nativa Bevy);
//!    - **K** = scheda del personaggio (`character_sheet.rs`, UI nativa Bevy).
//!
//! Il personaggio è salvato in `character.sav` (testo semplice) ogni 5 s se
//! modificato; non è ancora legato al modulo `savegame`.

use std::fmt::Write as _;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin};

use crate::dialog::DialogState;
use crate::player::Player;

// ---------------------------------------------------------------------------
// Parametri di bilanciamento (tutti modificabili)
// ---------------------------------------------------------------------------

/// Valore base di ogni attributo (poi si spende il pool bonus).
pub const BASE_ATTR: i32 = 50;
pub const MAX_ATTR: i32 = 100;
/// Il pool bonus iniziale si tira fra questi due valori (inclusi).
const ROLL_MIN: i32 = 12;
const ROLL_MAX: i32 = 32;
/// Valori iniziali delle abilità per ruolo.
const START_PRIMARY: i32 = 45;
const START_MAJOR: i32 = 35;
const START_MINOR: i32 = 25;
const START_MISC: i32 = 8;
pub const SKILL_MAX: i32 = 100;
/// Punti di avanzamento ottenuti per ogni aumento di abilità, per ruolo.
const POINTS_PRIMARY: f32 = 3.0;
const POINTS_MAJOR: f32 = 2.0;
const POINTS_MINOR: f32 = 1.0;
const POINTS_MISC: f32 = 0.5;
/// Punti attributo da assegnare a ogni livello.
const LEVELUP_ATTR_POINTS: i32 = 6;
/// Se `true` il livello sale solo con `CharacterRested` (come dormire in DF).
const LEVEL_REQUIRES_REST: bool = false;
/// I PV massimi sono `(PV/livello + modificatore COS) * (livello + questo)`.
const HEALTH_BASE_LEVELS: i32 = 3;
/// Se `true` mentre una finestra è aperta si setta `DialogState` (cursore
/// libero + input di gioco bloccato). Mettilo a `false` se interferisce.
const USE_DIALOG_STATE: bool = true;
const SAVE_FILE: &str = "character.sav";
const CLASSES_FILE: &str = "assets/classes.txt";

// ---------------------------------------------------------------------------
// Attributi
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Attribute {
    Strength,
    Intelligence,
    Willpower,
    Agility,
    Endurance,
    Personality,
    Speed,
    Luck,
}

impl Attribute {
    pub const ALL: [Attribute; 8] = [
        Attribute::Strength,
        Attribute::Intelligence,
        Attribute::Willpower,
        Attribute::Agility,
        Attribute::Endurance,
        Attribute::Personality,
        Attribute::Speed,
        Attribute::Luck,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Attribute::Strength => "Forza",
            Attribute::Intelligence => "Intelligenza",
            Attribute::Willpower => "Volontà",
            Attribute::Agility => "Agilità",
            Attribute::Endurance => "Costituzione",
            Attribute::Personality => "Personalità",
            Attribute::Speed => "Velocità",
            Attribute::Luck => "Fortuna",
        }
    }
}

// ---------------------------------------------------------------------------
// Abilità
// ---------------------------------------------------------------------------

/// Le 35 abilità di Daggerfall (stesso ordine del gioco).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Skill {
    Medical,
    Etiquette,
    Streetwise,
    Jumping,
    Orcish,
    Harpy,
    Giantish,
    Dragonish,
    Nymph,
    Daedric,
    Spriggan,
    Centaurian,
    Impish,
    Lockpicking,
    Mercantile,
    Pickpocket,
    Stealth,
    Swimming,
    Climbing,
    Backstabbing,
    Dodging,
    Running,
    Destruction,
    Restoration,
    Illusion,
    Alteration,
    Thaumaturgy,
    Mysticism,
    ShortBlade,
    LongBlade,
    HandToHand,
    Axe,
    BluntWeapon,
    Archery,
    CriticalStrike,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SkillGroup {
    Social,
    Physical,
    Stealth,
    Combat,
    Magic,
    Language,
}

impl SkillGroup {
    pub const ALL: [SkillGroup; 6] = [
        SkillGroup::Combat,
        SkillGroup::Magic,
        SkillGroup::Stealth,
        SkillGroup::Physical,
        SkillGroup::Social,
        SkillGroup::Language,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SkillGroup::Social => "Sociali e utili",
            SkillGroup::Physical => "Fisiche",
            SkillGroup::Stealth => "Furtività e ladrocinio",
            SkillGroup::Combat => "Combattimento",
            SkillGroup::Magic => "Magia",
            SkillGroup::Language => "Linguaggi",
        }
    }
}

/// (abilità, chiave inglese per i file, nome italiano, gruppo)
const SKILL_TABLE: [(Skill, &str, &str, SkillGroup); 35] = [
    (Skill::Medical, "Medical", "Medicina", SkillGroup::Social),
    (Skill::Etiquette, "Etiquette", "Etichetta", SkillGroup::Social),
    (Skill::Streetwise, "Streetwise", "Sapere di strada", SkillGroup::Social),
    (Skill::Jumping, "Jumping", "Salto", SkillGroup::Physical),
    (Skill::Orcish, "Orcish", "Orchesco", SkillGroup::Language),
    (Skill::Harpy, "Harpy", "Arpia", SkillGroup::Language),
    (Skill::Giantish, "Giantish", "Gigantesco", SkillGroup::Language),
    (Skill::Dragonish, "Dragonish", "Draconico", SkillGroup::Language),
    (Skill::Nymph, "Nymph", "Ninfa", SkillGroup::Language),
    (Skill::Daedric, "Daedric", "Daedrico", SkillGroup::Language),
    (Skill::Spriggan, "Spriggan", "Spriggan", SkillGroup::Language),
    (Skill::Centaurian, "Centaurian", "Centaurico", SkillGroup::Language),
    (Skill::Impish, "Impish", "Folletto", SkillGroup::Language),
    (Skill::Lockpicking, "Lockpicking", "Scasso", SkillGroup::Stealth),
    (Skill::Mercantile, "Mercantile", "Commercio", SkillGroup::Social),
    (Skill::Pickpocket, "Pickpocket", "Borseggio", SkillGroup::Stealth),
    (Skill::Stealth, "Stealth", "Furtività", SkillGroup::Stealth),
    (Skill::Swimming, "Swimming", "Nuoto", SkillGroup::Physical),
    (Skill::Climbing, "Climbing", "Scalata", SkillGroup::Physical),
    (Skill::Backstabbing, "Backstabbing", "Pugnalata alle spalle", SkillGroup::Stealth),
    (Skill::Dodging, "Dodging", "Schivata", SkillGroup::Physical),
    (Skill::Running, "Running", "Corsa", SkillGroup::Physical),
    (Skill::Destruction, "Destruction", "Distruzione", SkillGroup::Magic),
    (Skill::Restoration, "Restoration", "Ripristino", SkillGroup::Magic),
    (Skill::Illusion, "Illusion", "Illusione", SkillGroup::Magic),
    (Skill::Alteration, "Alteration", "Alterazione", SkillGroup::Magic),
    (Skill::Thaumaturgy, "Thaumaturgy", "Taumaturgia", SkillGroup::Magic),
    (Skill::Mysticism, "Mysticism", "Misticismo", SkillGroup::Magic),
    (Skill::ShortBlade, "ShortBlade", "Lama corta", SkillGroup::Combat),
    (Skill::LongBlade, "LongBlade", "Lama lunga", SkillGroup::Combat),
    (Skill::HandToHand, "HandToHand", "Mani nude", SkillGroup::Combat),
    (Skill::Axe, "Axe", "Ascia", SkillGroup::Combat),
    (Skill::BluntWeapon, "BluntWeapon", "Armi contundenti", SkillGroup::Combat),
    (Skill::Archery, "Archery", "Tiro con l'arco", SkillGroup::Combat),
    (Skill::CriticalStrike, "CriticalStrike", "Colpo critico", SkillGroup::Combat),
];

impl Skill {
    pub const COUNT: usize = 35;

    pub fn all() -> impl Iterator<Item = Skill> {
        SKILL_TABLE.iter().map(|e| e.0)
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn key(self) -> &'static str {
        SKILL_TABLE[self.index()].1
    }

    pub fn name(self) -> &'static str {
        SKILL_TABLE[self.index()].2
    }

    pub fn group(self) -> SkillGroup {
        SKILL_TABLE[self.index()].3
    }

    /// Difficoltà di avanzamento propria dell'abilità (1.0 = normale).
    pub fn advancement_multiplier(self) -> f32 {
        1.0
    }

    /// Cerca per chiave inglese (senza distinguere maiuscole/spazi).
    pub fn from_key(s: &str) -> Option<Skill> {
        let k: String = s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_lowercase();
        SKILL_TABLE
        .iter()
        .find(|e| e.1.to_lowercase() == k)
        .map(|e| e.0)
    }
}

// ---------------------------------------------------------------------------
// Classi
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SkillRole {
    Primary,
    Major,
    Minor,
    Misc,
}

impl SkillRole {
    fn start_value(self) -> i32 {
        match self {
            SkillRole::Primary => START_PRIMARY,
            SkillRole::Major => START_MAJOR,
            SkillRole::Minor => START_MINOR,
            SkillRole::Misc => START_MISC,
        }
    }

    fn level_points(self) -> f32 {
        match self {
            SkillRole::Primary => POINTS_PRIMARY,
            SkillRole::Major => POINTS_MAJOR,
            SkillRole::Minor => POINTS_MINOR,
            SkillRole::Misc => POINTS_MISC,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            SkillRole::Primary => "Primaria",
            SkillRole::Major => "Maggiore",
            SkillRole::Minor => "Minore",
            SkillRole::Misc => "",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClassDef {
    pub name: String,
    /// Punti ferita per livello.
    pub hp_per_level: i32,
    /// Moltiplicatore dei punti magia (magicka = Intelligenza × questo).
    pub spell_mult: f32,
    /// Difficoltà di avanzamento della classe (più alto = più lento).
    pub advancement: f32,
    pub primary: Skill,
    pub major: [Skill; 3],
    pub minor: [Skill; 6],
}

impl ClassDef {
    pub fn role(&self, skill: Skill) -> SkillRole {
        if self.primary == skill {
            SkillRole::Primary
        } else if self.major.contains(&skill) {
            SkillRole::Major
        } else if self.minor.contains(&skill) {
            SkillRole::Minor
        } else {
            SkillRole::Misc
        }
    }
}

/// Catalogo delle classi disponibili.
#[derive(Resource, Clone)]
pub struct ClassCatalog {
    pub classes: Vec<ClassDef>,
}

impl ClassCatalog {
    pub fn class(&self, idx: usize) -> &ClassDef {
        &self.classes[idx.min(self.classes.len() - 1)]
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.classes
        .iter()
        .position(|c| c.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Carica `assets/classes.txt` se esiste, altrimenti i valori di default.
    /// Con file non valido o vuoto ripiega sul default (e lo scrive nel log).
    fn load() -> Self {
        if let Ok(text) = std::fs::read_to_string(CLASSES_FILE) {
            let classes = parse_classes(&text);
            if !classes.is_empty() {
                info!("Classi: {} da {CLASSES_FILE}", classes.len());
                return Self { classes };
            }
            warn!("{CLASSES_FILE}: nessuna classe valida, uso il catalogo di default");
        }
        let classes = parse_classes(DEFAULT_CLASSES_TXT);
        Self { classes }
    }
}

/// Formato: `[Nome]` poi `hp`, `spell`, `adv`, `primary`, `major` (3 abilità),
/// `minor` (6 abilità). Le abilità sono le chiavi inglesi (vedi `Skill::key`).
fn parse_classes(text: &str) -> Vec<ClassDef> {
    #[derive(Default)]
    struct Raw {
        name: String,
        hp: Option<i32>,
        spell: Option<f32>,
        adv: Option<f32>,
        primary: Option<Skill>,
        major: Vec<Skill>,
        minor: Vec<Skill>,
        bad: bool,
    }

    fn finish(raw: Raw, out: &mut Vec<ClassDef>) {
        if raw.name.is_empty() {
            return;
        }
        let (Some(hp), Some(primary)) = (raw.hp, raw.primary) else {
            warn!("Classe '{}': mancano hp o primary, ignorata", raw.name);
            return;
        };
        if raw.bad || raw.major.len() != 3 || raw.minor.len() != 6 {
            warn!(
                "Classe '{}': servono 1 primary, 3 major e 6 minor validi, ignorata",
                raw.name
            );
            return;
        }
        let mut all = vec![primary];
        all.extend(&raw.major);
        all.extend(&raw.minor);
        let mut sorted = all.clone();
        sorted.sort_by_key(|s| s.index());
        sorted.dedup();
        if sorted.len() != all.len() {
            warn!("Classe '{}': abilità ripetute, ignorata", raw.name);
            return;
        }
        out.push(ClassDef {
            name: raw.name,
            hp_per_level: hp.max(1),
                 spell_mult: raw.spell.unwrap_or(0.5).max(0.0),
                 advancement: raw.adv.unwrap_or(1.0).max(0.1),
                 primary,
                 major: [raw.major[0], raw.major[1], raw.major[2]],
                 minor: [
                     raw.minor[0],
                     raw.minor[1],
                     raw.minor[2],
                     raw.minor[3],
                     raw.minor[4],
                     raw.minor[5],
                 ],
        });
    }

    let mut out = Vec::new();
    let mut cur = Raw::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            finish(std::mem::take(&mut cur), &mut out);
            cur.name = line[1..line.len() - 1].trim().to_string();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim().to_lowercase(), v.trim());
        let skills = |v: &str, bad: &mut bool| -> Vec<Skill> {
            v.split(',')
            .filter_map(|s| {
                let sk = Skill::from_key(s);
                if sk.is_none() {
                    warn!("Abilità sconosciuta '{}'", s.trim());
                    *bad = true;
                }
                sk
            })
            .collect()
        };
        match k.as_str() {
            "hp" => cur.hp = v.parse().ok(),
            "spell" => cur.spell = v.parse().ok(),
            "adv" => cur.adv = v.parse().ok(),
            "primary" => {
                cur.primary = Skill::from_key(v);
                if cur.primary.is_none() {
                    cur.bad = true;
                }
            }
            "major" => cur.major = skills(v, &mut cur.bad),
            "minor" => cur.minor = skills(v, &mut cur.bad),
            _ => {}
        }
    }
    finish(cur, &mut out);
    out
}

/// Le 18 classi di Daggerfall. VALORI INDICATIVI: abilità, PV e moltiplicatori
/// sono approssimazioni, non i dati originali. Sostituiscili con
/// `assets/classes.txt`.
const DEFAULT_CLASSES_TXT: &str = "
[Mage]
hp = 6
spell = 2.0
primary = Destruction
major = Restoration, Illusion, Alteration
minor = Thaumaturgy, Mysticism, Medical, Daedric, Etiquette, Streetwise

[Spellsword]
hp = 8
spell = 1.5
primary = LongBlade
major = Destruction, Restoration, Alteration
minor = Dodging, Medical, Running, Swimming, Jumping, Etiquette

[Battlemage]
hp = 8
spell = 1.5
primary = Destruction
major = LongBlade, Alteration, Illusion
minor = Axe, Dodging, Medical, Mysticism, Running, Climbing

[Sorcerer]
hp = 6
spell = 3.0
primary = Mysticism
major = Destruction, Illusion, Thaumaturgy
minor = Daedric, Restoration, Alteration, Medical, Etiquette, Impish

[Healer]
hp = 6
spell = 2.0
primary = Restoration
major = Medical, Mysticism, Alteration
minor = Etiquette, Streetwise, BluntWeapon, Thaumaturgy, Mercantile, Dodging

[Nightblade]
hp = 8
spell = 1.5
primary = Stealth
major = ShortBlade, Illusion, Alteration
minor = Backstabbing, Dodging, Climbing, Running, Lockpicking, Mysticism

[Bard]
hp = 8
spell = 1.0
primary = Etiquette
major = Streetwise, Mercantile, ShortBlade
minor = Dodging, Illusion, Archery, Medical, Running, Jumping

[Burglar]
hp = 8
spell = 0.5
primary = Lockpicking
major = Stealth, Pickpocket, Climbing
minor = ShortBlade, Dodging, Mercantile, Streetwise, Running, Jumping

[Rogue]
hp = 8
spell = 0.5
primary = ShortBlade
major = Streetwise, Dodging, CriticalStrike
minor = Pickpocket, Lockpicking, Mercantile, Running, Jumping, Backstabbing

[Acrobat]
hp = 8
spell = 0.5
primary = Jumping
major = Climbing, Dodging, Running
minor = Swimming, Stealth, HandToHand, ShortBlade, Streetwise, Medical

[Thief]
hp = 8
spell = 0.5
primary = Pickpocket
major = Stealth, Lockpicking, Streetwise
minor = Backstabbing, Climbing, Dodging, Mercantile, ShortBlade, Running

[Assassin]
hp = 8
spell = 0.5
primary = Backstabbing
major = ShortBlade, Stealth, CriticalStrike
minor = Climbing, Dodging, Lockpicking, Running, Jumping, Medical

[Monk]
hp = 10
spell = 0.5
primary = HandToHand
major = Dodging, Medical, CriticalStrike
minor = Running, Jumping, Climbing, Swimming, BluntWeapon, Etiquette

[Archer]
hp = 10
spell = 0.5
primary = Archery
major = ShortBlade, Stealth, Running
minor = Climbing, Dodging, Jumping, Medical, Swimming, LongBlade

[Ranger]
hp = 10
spell = 0.5
primary = LongBlade
major = Archery, Climbing, Swimming
minor = Running, Dodging, Stealth, Medical, Jumping, Axe

[Barbarian]
hp = 12
spell = 0.5
primary = Axe
major = BluntWeapon, Running, Swimming
minor = Climbing, Dodging, Jumping, HandToHand, CriticalStrike, Medical

[Warrior]
hp = 12
spell = 0.5
primary = LongBlade
major = Axe, BluntWeapon, Dodging
minor = Running, Swimming, Climbing, Jumping, HandToHand, Medical

[Knight]
hp = 10
spell = 0.5
primary = LongBlade
major = Etiquette, BluntWeapon, Dodging
minor = Archery, Medical, Mercantile, Running, Swimming, Restoration
";

// ---------------------------------------------------------------------------
// Personaggio
// ---------------------------------------------------------------------------

#[derive(Resource, Clone)]
pub struct Character {
    /// `false` finché la creazione non è conclusa.
    pub created: bool,
    pub name: String,
    /// Indice nel `ClassCatalog`.
    pub class: usize,
    pub level: u32,
    pub attrs: [i32; 8],
    pub skills: [i32; Skill::COUNT],
    /// Tally di usi verso il prossimo aumento di ciascuna abilità.
    pub uses: [u32; Skill::COUNT],
    /// Punti di avanzamento verso il prossimo livello.
    pub level_points: f32,
    /// Punti attributo ancora da assegnare (livelli guadagnati).
    pub pending_attr_points: i32,
    /// Da salvare su disco.
    pub dirty: bool,
}

impl Default for Character {
    fn default() -> Self {
        Self {
            created: false,
            name: String::new(),
            class: 0,
            level: 1,
            attrs: [BASE_ATTR; 8],
            skills: [START_MISC; Skill::COUNT],
            uses: [0; Skill::COUNT],
            level_points: 0.0,
            pending_attr_points: 0,
            dirty: false,
        }
    }
}

/// Statistiche calcolate da attributi, classe e livello.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Default)]
pub struct DerivedStats {
    pub max_health: i32,
    pub max_magicka: i32,
    pub max_fatigue: i32,
    /// Bonus ai danni in mischia (da Forza), può essere negativo.
    pub damage_mod: i32,
    /// Bonus a colpire (da Agilità), può essere negativo.
    pub to_hit_mod: i32,
    /// Carico massimo trasportabile in kg.
    pub max_encumbrance: f32,
}

pub struct TallyResult {
    pub raised: bool,
    pub levels_gained: u32,
}

/// Usi necessari per aumentare di 1 un'abilità (forma usata da Daggerfall:
/// cresce col valore dell'abilità e del livello).
fn required_uses(value: i32, level: u32, class_adv: f32, skill_adv: f32) -> u32 {
    let level_mod = 1.04f64.powi(level as i32);
    let v = (value.max(1) as f64 * skill_adv as f64 * class_adv as f64 * level_mod) / 2.0 + 0.5;
    (v.floor() as u32).max(1)
}

/// Punti di avanzamento necessari per passare da `level` a `level + 1`.
fn level_points_needed(level: u32) -> f32 {
    12.0 + 4.0 * level as f32
}

fn stat_mod(value: i32) -> i32 {
    ((value - 50) as f32 / 10.0).floor() as i32
}

impl Character {
    pub fn create(name: &str, class: usize, attrs: [i32; 8], cat: &ClassCatalog) -> Self {
        let def = cat.class(class);
        let mut skills = [START_MISC; Skill::COUNT];
        for s in Skill::all() {
            skills[s.index()] = def.role(s).start_value();
        }
        Self {
            created: true,
            name: name.trim().to_string(),
            class,
            level: 1,
            attrs,
            skills,
            uses: [0; Skill::COUNT],
            level_points: 0.0,
            pending_attr_points: 0,
            dirty: true,
        }
    }

    pub fn attr(&self, a: Attribute) -> i32 {
        self.attrs[a.index()]
    }

    pub fn skill(&self, s: Skill) -> i32 {
        self.skills[s.index()]
    }

    pub fn derived(&self, cat: &ClassCatalog) -> DerivedStats {
        let def = cat.class(self.class);
        let str_ = self.attr(Attribute::Strength);
        let int = self.attr(Attribute::Intelligence);
        let agi = self.attr(Attribute::Agility);
        let end = self.attr(Attribute::Endurance);
        let hp_level = (def.hp_per_level + stat_mod(end)).max(1);
        DerivedStats {
            max_health: hp_level * (self.level as i32 + HEALTH_BASE_LEVELS),
            max_magicka: (int as f32 * def.spell_mult).floor() as i32,
            max_fatigue: str_ + end,
            damage_mod: stat_mod(str_),
            to_hit_mod: stat_mod(agi),
            max_encumbrance: (str_ as f32 * 1.5).floor(),
        }
    }

    pub fn level_progress(&self) -> f32 {
        (self.level_points / level_points_needed(self.level)).clamp(0.0, 1.0)
    }

    /// Registra l'uso di un'abilità; alza abilità e livello se serve.
    pub fn tally_skill(&mut self, skill: Skill, amount: u32, cat: &ClassCatalog) -> TallyResult {
        let mut result = TallyResult {
            raised: false,
            levels_gained: 0,
        };
        let i = skill.index();
        if self.skills[i] >= SKILL_MAX || amount == 0 {
            return result;
        }
        let def = cat.class(self.class);
        let role = def.role(skill);
        self.uses[i] += amount;
        loop {
            if self.skills[i] >= SKILL_MAX {
                self.uses[i] = 0;
                break;
            }
            let need = required_uses(
                self.skills[i],
                self.level,
                def.advancement,
                skill.advancement_multiplier(),
            );
            if self.uses[i] < need {
                break;
            }
            self.uses[i] -= need;
            self.skills[i] += 1;
            self.level_points += role.level_points();
            result.raised = true;
        }
        if result.raised {
            self.dirty = true;
            if !LEVEL_REQUIRES_REST {
                result.levels_gained = self.apply_level_ups();
            }
        }
        result
    }

    /// Sale di livello finché i punti bastano. Restituisce i livelli guadagnati.
    pub fn apply_level_ups(&mut self) -> u32 {
        let mut gained = 0;
        while self.level_points >= level_points_needed(self.level) {
            self.level_points -= level_points_needed(self.level);
            self.level += 1;
            self.pending_attr_points += LEVELUP_ATTR_POINTS;
            gained += 1;
            self.dirty = true;
        }
        gained
    }

    // --- Salvataggio su file di testo ---

    pub fn to_text(&self, cat: &ClassCatalog) -> String {
        fn join<T: ToString>(v: &[T]) -> String {
            v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
        }
        let mut s = String::new();
        let _ = writeln!(s, "name={}", self.name.replace(['\n', '\r'], " "));
        let _ = writeln!(s, "class={}", cat.class(self.class).name);
        let _ = writeln!(s, "level={}", self.level);
        let _ = writeln!(s, "points={}", self.level_points);
        let _ = writeln!(s, "pending={}", self.pending_attr_points);
        let _ = writeln!(s, "attrs={}", join(&self.attrs));
        let _ = writeln!(s, "skills={}", join(&self.skills));
        let _ = writeln!(s, "uses={}", join(&self.uses));
        s
    }

    pub fn from_text(text: &str, cat: &ClassCatalog) -> Option<Self> {
        fn list<T: std::str::FromStr + Copy + Default, const N: usize>(v: &str) -> Option<[T; N]> {
            let items: Vec<T> = v
            .split(',')
            .map(|x| x.trim().parse::<T>().ok())
            .collect::<Option<Vec<_>>>()?;
            if items.len() != N {
                return None;
            }
            let mut out = [T::default(); N];
            out.copy_from_slice(&items);
            Some(out)
        }
        let mut ch = Character::default();
        let mut have_class = false;
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let v = v.trim();
            match k.trim() {
                "name" => ch.name = v.to_string(),
                "class" => {
                    ch.class = cat.index_of(v)?;
                    have_class = true;
                }
                "level" => ch.level = v.parse().ok()?,
                "points" => ch.level_points = v.parse().ok()?,
                "pending" => ch.pending_attr_points = v.parse().ok()?,
                "attrs" => ch.attrs = list::<i32, 8>(v)?,
                "skills" => ch.skills = list::<i32, { Skill::COUNT }>(v)?,
                "uses" => ch.uses = list::<u32, { Skill::COUNT }>(v)?,
                _ => {}
            }
        }
        if !have_class || ch.name.trim().is_empty() || ch.level == 0 {
            return None;
        }
        ch.created = true;
        Some(ch)
    }
}

fn load_saved_character(cat: &ClassCatalog) -> Option<Character> {
    let text = std::fs::read_to_string(SAVE_FILE).ok()?;
    let ch = Character::from_text(&text, cat);
    if ch.is_none() {
        warn!("{SAVE_FILE} non leggibile (o classe non più presente): nuova creazione");
    }
    ch
}

// ---------------------------------------------------------------------------
// Eventi
// ---------------------------------------------------------------------------

/// Un'abilità è stata usata (da inviare dagli altri moduli).
#[derive(Event, Clone, Copy, Debug)]
pub struct SkillUsed {
    pub skill: Skill,
    pub amount: u32,
}

/// Il personaggio è salito di livello.
#[derive(Event, Clone, Copy, Debug)]
pub struct LevelUp {
    pub level: u32,
}

/// Il personaggio ha riposato (serve se `LEVEL_REQUIRES_REST`).
#[derive(Event, Clone, Copy, Debug)]
pub struct CharacterRested;

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

struct Draft {
    name: String,
    class: usize,
    attrs: [i32; 8],
    pool: i32,
}

#[derive(Resource)]
struct CharacterUi {
    draft: Option<Draft>,
    owns_dialog: bool,
    rng: u64,
}

fn next_u64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn roll_pool(rng: &mut u64) -> i32 {
    ROLL_MIN + (next_u64(rng) % (ROLL_MAX - ROLL_MIN + 1) as u64) as i32
}

pub struct CharacterPlugin;

impl Plugin for CharacterPlugin {
    fn build(&self, app: &mut App) {
        let catalog = ClassCatalog::load();
        let mut rng = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
        | 1;

        let (character, draft) = match load_saved_character(&catalog) {
            Some(c) => (c, None),
            None => (
                Character::default(),
                     Some(Draft {
                         name: String::new(),
                          class: 0,
                          attrs: [BASE_ATTR; 8],
                          pool: roll_pool(&mut rng),
                     }),
            ),
        };
        let derived = character.derived(&catalog);

        if !app.is_plugin_added::<EguiPlugin>() {
            app.add_plugins(EguiPlugin);
        }
        app.insert_resource(catalog)
        .insert_resource(character)
        .insert_resource(derived)
        .insert_resource(CharacterUi {
            draft,
            owns_dialog: false,
            rng,
        })
        .add_event::<SkillUsed>()
        .add_event::<LevelUp>()
        .add_event::<CharacterRested>()
        .add_systems(
            Update,
            (
                apply_skill_events,
             apply_rest_events,
             auto_tally_player_skills,
             update_derived_stats,
             autosave_character,
             character_ui,
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// Sistemi
// ---------------------------------------------------------------------------

fn apply_skill_events(
    mut events: EventReader<SkillUsed>,
    mut ch: ResMut<Character>,
    cat: Res<ClassCatalog>,
    mut level_events: EventWriter<LevelUp>,
) {
    if !ch.created {
        events.clear();
        return;
    }
    for e in events.read() {
        let r = ch.tally_skill(e.skill, e.amount, &cat);
        if r.raised {
            info!("{} sale a {}", e.skill.name(), ch.skill(e.skill));
        }
        for _ in 0..r.levels_gained {
            info!("Livello {} raggiunto!", ch.level);
            level_events.send(LevelUp { level: ch.level });
        }
    }
}

fn apply_rest_events(
    mut events: EventReader<CharacterRested>,
    mut ch: ResMut<Character>,
    mut level_events: EventWriter<LevelUp>,
) {
    if events.is_empty() {
        return;
    }
    events.clear();
    if !LEVEL_REQUIRES_REST || !ch.created {
        return;
    }
    let gained = ch.apply_level_ups();
    for _ in 0..gained {
        info!("Livello {} raggiunto!", ch.level);
        level_events.send(LevelUp { level: ch.level });
    }
}

/// Alza da solo le abilità che si deducono dallo stato del player: Salto
/// (a ogni salto) e Scalata (ogni 2 s di arrampicata).
fn auto_tally_player_skills(
    time: Res<Time>,
    ch: Res<Character>,
    player: Query<&Player>,
    mut events: EventWriter<SkillUsed>,
    mut state: Local<(bool, f32)>,
) {
    if !ch.created {
        return;
    }
    let Ok(p) = player.get_single() else {
        return;
    };
    let (prev_ground, climb_t) = &mut *state;
    if *prev_ground && !p.on_ground && p.vel_y > 1.0 {
        events.send(SkillUsed {
            skill: Skill::Jumping,
            amount: 1,
        });
    }
    *prev_ground = p.on_ground;
    if p.climbing {
        *climb_t += time.delta_secs();
        if *climb_t >= 2.0 {
            *climb_t = 0.0;
            events.send(SkillUsed {
                skill: Skill::Climbing,
                amount: 1,
            });
        }
    } else {
        *climb_t = 0.0;
    }
}

fn update_derived_stats(
    ch: Res<Character>,
    cat: Res<ClassCatalog>,
    mut derived: ResMut<DerivedStats>,
) {
    let new = ch.derived(&cat);
    if *derived != new {
        *derived = new;
    }
}

fn autosave_character(
    time: Res<Time>,
    mut timer: Local<f32>,
    mut ch: ResMut<Character>,
    cat: Res<ClassCatalog>,
) {
    *timer += time.delta_secs();
    if *timer < 5.0 || !ch.dirty || !ch.created {
        return;
    }
    *timer = 0.0;
    ch.dirty = false;
    if let Err(e) = std::fs::write(SAVE_FILE, ch.to_text(&cat)) {
        warn!("Impossibile salvare {SAVE_FILE}: {e}");
    }
}

// ---------------------------------------------------------------------------
// Interfaccia (egui)
// ---------------------------------------------------------------------------

fn class_summary(ui: &mut egui::Ui, def: &ClassDef) {
    ui.label(format!(
        "Punti ferita per livello: {}   Moltiplicatore magia: ×{:.2}   Avanzamento: ×{:.2}",
        def.hp_per_level, def.spell_mult, def.advancement
    ));
    ui.label(format!("Primaria: {}", def.primary.name()));
    ui.label(format!(
        "Maggiori: {}",
        def.major.iter().map(|s| s.name()).collect::<Vec<_>>().join(", ")
    ));
    ui.label(format!(
        "Minori: {}",
        def.minor.iter().map(|s| s.name()).collect::<Vec<_>>().join(", ")
    ));
}

/// UI egui del modulo.
///
/// **Solo creazione del personaggio.** La scheda del personaggio (attributi,
/// abilità, livello, statistiche derivate) è offerta da `character_sheet.rs`
/// col tasto **K**, che usa la UI nativa Bevy come il resto del gioco. Il
/// tasto **C** è libero per `equipment::toggle_equipment`.
///
/// In precedenza questa funzione intercettava anche **C** e **Esc** per
/// aprire/chiudere una seconda scheda egui: sovrascriveva il binding di C
/// (impedendo l'apertura dell'equipaggiamento) e andava in conflitto con la
/// UI nativa di K. Il blocco C/Esc e la scheda egui ridondante sono stati
/// rimossi; `sheet_open` è stato eliminato.
#[allow(clippy::too_many_arguments)]
fn character_ui(
    mut contexts: EguiContexts,
    mut ui_state: ResMut<CharacterUi>,
    mut ch: ResMut<Character>,
    cat: Res<ClassCatalog>,
    mut dialog: ResMut<DialogState>,
) {
    let ui_state = &mut *ui_state;
    let Some(ctx) = contexts.try_ctx_mut() else {
        return;
    };

    // Cursore libero e input di gioco fermo solo durante la creazione.
    // Dopo la creazione `draft` è `None`: la UI di questo modulo non fa
    // nulla e C resta libero per l'equipaggiamento (K apre la scheda
    // nativa via `character_sheet.rs`).
    let want_open = ui_state.draft.is_some();
    if want_open != ui_state.owns_dialog {
        ui_state.owns_dialog = want_open;
        if USE_DIALOG_STATE {
            dialog.open = want_open;
            dialog.forced_by_menu = want_open;
        }
    }

    // --- Creazione del personaggio ---
    let mut created: Option<Character> = None;
    if let Some(draft) = ui_state.draft.as_mut() {
        let mut reroll = false;
        egui::Window::new("Creazione del personaggio")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Nome:");
                ui.text_edit_singleline(&mut draft.name);
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Classe:");
                egui::ComboBox::from_id_salt("class_select")
                .selected_text(cat.class(draft.class).name.clone())
                .show_ui(ui, |ui| {
                    for (i, c) in cat.classes.iter().enumerate() {
                        ui.selectable_value(&mut draft.class, i, c.name.as_str());
                    }
                });
            });
            class_summary(ui, cat.class(draft.class));
            ui.separator();
            ui.horizontal(|ui| {
                ui.strong("Attributi");
                ui.label(format!("   Punti da spendere: {}", draft.pool));
                if ui.button("Ritira i dadi").clicked() {
                    reroll = true;
                }
            });
            egui::Grid::new("attr_grid")
            .num_columns(4)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                for a in Attribute::ALL {
                    let i = a.index();
                    ui.label(a.name());
                    if ui
                        .add_enabled(draft.attrs[i] > BASE_ATTR, egui::Button::new("-"))
                        .clicked()
                        {
                            draft.attrs[i] -= 1;
                            draft.pool += 1;
                        }
                        ui.label(draft.attrs[i].to_string());
                    if ui
                        .add_enabled(
                            draft.pool > 0 && draft.attrs[i] < MAX_ATTR,
                            egui::Button::new("+"),
                        )
                        .clicked()
                        {
                            draft.attrs[i] += 1;
                            draft.pool -= 1;
                        }
                        ui.end_row();
                }
            });
            ui.separator();
            let ready = !draft.name.trim().is_empty() && draft.pool == 0;
            if ui
                .add_enabled(ready, egui::Button::new("Inizia l'avventura"))
                .clicked()
                {
                    created = Some(Character::create(&draft.name, draft.class, draft.attrs, &cat));
                }
                if !ready {
                    ui.small("Serve un nome e vanno spesi tutti i punti.");
                }
        });
        if reroll {
            draft.attrs = [BASE_ATTR; 8];
            draft.pool = roll_pool(&mut ui_state.rng);
        }
    }
    if let Some(new_char) = created {
        info!("Personaggio creato: {} ({})", new_char.name, cat.class(new_char.class).name);
        *ch = new_char;
        ui_state.draft = None;
    }
}
