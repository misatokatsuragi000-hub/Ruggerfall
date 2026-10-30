//! Quest data model, prompt builders, validators, in-memory archive, config.
//! Nessuna chiamata di rete né processo: tutto il runtime LLM è in `quest_llm.rs`.
use crate::data::city_map::PoiKind;
use crate::data::maps::DFLocation;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;

// =============================================================================
// Costanti
// =============================================================================

/// Località = città/villaggio se ha almeno questi blocchi RMB.
const MIN_TOWN_BLOCKS: usize = 2;

/// Massimo di caption distinte per cui generare un flavor in blocco.
pub const MAX_NPC_FLAVORS: usize = 12;

/// Scambi di conversazione che finiscono nel prompt delle domande libere.
pub const MAX_HISTORY_TURNS: usize = 10;

pub const VOICE_MAX_CHARS: usize = 400;
pub const QUESTION_MAX_CHARS: usize = 200;

/// Blocchi di volti in `TFAC00I0.RCI`.
const FACE_BLOCK: usize = 24;

const TEMPERAMENTS: [&str; 12] = [
    "nervous", "cheerful", "weary", "proud", "curious", "suspicious", "friendly", "gloomy",
"talkative", "blunt", "superstitious", "greedy",
];

const THEMES: [&str; 8] = [
    "artifact_discovery", "rescue", "assassination", "escort", "investigation", "monster_hunt",
"theft", "diplomacy",
];

const DUNGEONS: [(&str, &str, &str, &str, i64); 15] = [
    ("dgf_crypt_ash", "Crypt of Ash", "Cripta di Cenere", "Daggerfall", 12),
    ("dgf_barrow_mourn", "Mournhold Barrow", "Tumulo di Mournhold", "Daggerfall", 18),
    ("dgf_ruin_weeping", "The Weeping Hollow", "Cavo Piangente", "Daggerfall", 25),
    ("dgf_hollow_kings", "Hollow of the Kings", "Cavo dei Re", "Daggerfall", 30),
    ("sen_catacomb_tide", "Catacombs of the Tide", "Catacombe della Marea", "Sentinel", 10),
    ("sen_hall_silence", "The Silent Hall", "Sala del Silenzio", "Sentinel", 15),
    ("sen_tomb_drowned", "Tomb of the Drowned", "Tomba degli Annegati", "Sentinel", 22),
    ("way_crypt_pale", "The Pale Crypt", "Cripta Pallida", "Wayrest", 14),
    ("way_ruin_grinning", "Ruin of the Grinning God", "Rovina del Dio Ridente", "Wayrest", 19),
    ("way_vault_sorrow", "Vault of Sorrows", "Volta dei Dolori", "Wayrest", 28),
    ("ors_deep_forge", "The Deep Forge", "Fucina Profonda", "Orsinium", 16),
    ("ors_tomb_blood", "Blood-Tomb of Malacath", "Tomba di Sangue di Malacath", "Orsinium", 21),
    ("ors_cavern_echo", "The Echoing Caverns", "Caverne Risonanti", "Orsinium", 32),
    ("wild_tomb_seven", "Tomb of the Seven", "Tomba dei Sette", "Wilderness", 40),
    ("wild_crypt_last", "The Last Crypt", "Ultima Cripta", "Wilderness", 45),
];

const FACTIONS: [(&str, &str, &str, &str); 4] = [
    ("mages_guild", "Mages Guild", "guild", "Daggerfall"),
    ("fighters_guild", "Fighters Guild", "guild", "Daggerfall"),
    ("thieves_guild", "Thieves Guild", "guild", "Wayrest"),
    ("knights_dragon", "Knights of the Dragon", "order", "Daggerfall"),
];

const RECOVERED_HINTS: [&str; 10] = [
    "already recovered", "already found", "already taken", "already carried away",
"has been recovered", "has been found", "now in the hands of",
"already selling", "big deals with the", "already have",
];

// =============================================================================
// Prompt incorporati
// =============================================================================

pub const DEFAULT_SEED_PROMPT: &str = r####"You are the author of Daggerfall quest seeds, a medieval dark fantasy world.
A quest seed is the "ground truth" behind a quest, before any NPC or
rumor tells it. You must choose a dungeon and a quest-giver from the provided lists.
Do NOT invent dungeons or factions: pick ONE exact id from each list.
Required theme: {theme}
AVAILABLE DUNGEONS (pick ONE by id):
{dungeon_list}
AVAILABLE QUEST-GIVERS (pick ONE by id):
{faction_list}
Your output must enrich the seed with:
A short, evocative title (3-6 words), in English.
An exotic name for the tomb/crypt/ruin (this will be the REAL name of the place,
the one NPCs who know it will mention). Must differ from the name in the dungeon list.
An exotic name for the artifact / object at the center of the quest.
If the theme has no artifact (e.g. rescue, assassination), invent a name
for the "prize" or "target".
    A 1-2 sentence lore, in English, explaining why this place and object matter.
    A difficulty from 1 (trivial) to 5 (legendary).
    IMPORTANT CONSTRAINT: the quest location is ONLY the chosen dungeon. Do not mention
    other dungeons from the list. The "technical" name in parentheses is just a dev
    label: the world does not know it. NPCs who know the place will use the
    `tomb_name` you are about to invent.
    Reply ONLY with a JSON object with these fields:
    {{
    "theme": "<same as input>",
"title": "<short title in English>",
"dungeon_id": "<id from the list>",
"quest_giver_id": "<id from the list>",
"tomb_name": "<exotic name>",
"artifact_name": "<exotic name>",
"difficulty": <1-5>,
"lore": "<1-2 sentences in English>"
}}
Start your reply with '{{' and end with '}}'. No prose, no markdown, no reasoning."####;

pub const DEFAULT_RUMORS_PROMPT: &str = r####"You are generating RUMORS about a quest that exists in the world of Daggerfall.
The quest seed is the ground truth (this is what actually happened):
{seed_json}
CRITICAL RULE ON THE PLACE NAME:
The quest location has ONE name in the seed: `tomb_name`. It is the only name
that exists for this place in the world.
There is NO other "generic English" name for the place: if an NPC does not know
`tomb_name`, they must say "that place", "the crypt", "those ruins", "the hollow
outside town", not another proper name.
If an NPC is a drunk, they may get `tomb_name` wrong and call it by a made-up
name, but must not use any other name from the catalog.
Faction names are proper nouns WITHOUT an article:
write "Mages Guild", not "The Mages Guild".
CRITICAL RULE ON THE ARTIFACT:
The artifact is still HIDDEN in the location. Nobody has recovered it.
No NPC should talk about "deals" already underway with the artifact: only
speculation about what will happen IF it is recovered.
Generate FIVE rumor variants, one per tone. Each rumor is a short piece of
dialogue that a specific type of NPC would say if asked about the quest. They must:
ALL refer to the SAME event, location and object from the seed.
Differ in tone, reliability and level of detail.
NEVER contradict the seed on facts (location, artifact, quest-giver).
Be 1-2 sentences, spoken language, IN ENGLISH.
Sound like gossip, not a formal briefing.
TONES, THEIR CHARACTER AND WHAT THEY KNOW:
noble:
Knows: `tomb_name` (the exact exotic name), `artifact_name`, `quest_giver_name`.
Does NOT know / does not care: exact distance, number of mercenaries,
practical details. Does not bring up kilometer figures.
Tone: formal, slightly haughty, words like "Your Grace", "this matter",
"it behooves". Talks about politics, stability, honor, and the "handling" of
such discoveries.
merchant:
Knows: `tomb_name`, `artifact_name`, approximate distance ("just outside",
"a couple of hours on foot"). Knows the region the place is in (`dungeon_region`).
Does NOT know / does not care: political or magical details.
Tone: practical, worried about prices, roads, requisitions, markets
that will close if the situation worsens. Does NOT talk about artifacts already
being traded: the artifact is still hidden.
soldier:
Knows: the mission (who is going, roughly where), `artifact_name`. Has seen
the soldiers or mages leave. Does NOT know `tomb_name`: uses "that place",
"the crypt", "those ruins".
Tone: direct, martial, respects facts but does not embellish. Talks about
orders, patrols, "they won't come back until they have it".
commoner:
Knows: only vague rumors. Does NOT know `tomb_name` (uses "the crypt outside town",
"that cursed place"). Does NOT know `artifact_name` (says "something that pulses",
"a strange object"). Does NOT know `quest_giver_name` (says "some lords",
"important folk", "the mages" at most). Has superstitions.
Tone: fearful, "they say", "I heard", "it scares me".
drunk:
Knows: something, but all wrong. Gets `tomb_name` wrong (calls it by a made-up
name or another random name), gets the distance wrong ("ten or twenty
kilometers, dunno"), gets `quest_giver_name` wrong (calls the guild by another
name, or says "the squires", "the templars"). May also invent a non-existent
historical figure instead of the right one.
Tone: confused, repetitive, cheerful or whiny. Uses "let's drink!", "can't
remember", "dunno".
Reply ONLY with a JSON object:
{{
"rumors": [
{{ "tone": "noble",     "text": "..." }},
{{ "tone": "merchant",  "text": "..." }},
{{ "tone": "soldier",   "text": "..." }},
{{ "tone": "commoner",  "text": "..." }},
{{ "tone": "drunk",     "text": "..." }}
]
}}
Start your reply with '{{' and end with '}}'. No prose, no markdown, no reasoning."####;

pub const DEFAULT_NPC_FLAVOR_PROMPT: &str = r####"You are generating RUMORS about a Daggerfall quest, each told by a SPECIFIC NPC.
The quest seed (ground truth):
{seed_json}
Below is a list of NPCs. Each line gives the EXACT description from FLATS.CFG.
You MUST write each rumor as if spoken BY that exact person. The description
defines who they are: their gender, appearance, profession, social standing,
and personality. Let the description fully determine the voice, vocabulary,
attitude, and what details they would know or care about.
Do NOT assign a generic "tone". A "beautiful maiden" speaks differently from
an "old beggar", a "guard", or a "King of Worms". Let each description drive
the character naturally.
RULES:
Refer to the same facts as the seed (tomb_name, artifact_name, quest-giver).
Do NOT use the generic dungeon name; use only tomb_name or vague references
("that place", "the crypt", "those ruins").
The artifact is still HIDDEN. Nobody has recovered it.
1-2 sentences, spoken language, IN ENGLISH.
Sound like gossip, not a briefing.
NPCs (in order, one per line):
{npc_list}
Reply ONLY with a JSON object:
{{
"rumors": [
{{ "index": 0,  "text": "..."}},
{{ "index": 1,  "text": "..."}}
]
}}
where "index" matches the NPC number. Start with '{{' and end with '}}'.
No prose, no markdown, no reasoning."####;

pub const DEFAULT_NPC_VOICE_PROMPT: &str = r####"You are writing what ONE specific person says in a Daggerfall town: a rumor about a quest.
The quest seed (ground truth):
{seed_json}
The person who is speaking:
{npc}
Write the rumor AS SPOKEN BY THIS PERSON. Their description decides their voice, vocabulary,
attitude, and what they know or care about. Make it sound like them and nobody else.
RULES:
Use only facts from the seed (tomb_name, artifact_name, quest-giver, region, distance).
INSIDERS: if the person's description says they belong to the very organization named in the seed
(`quest_giver_name`), this is THEIR OWN business, not gossip: they know it first-hand and speak as an
insider ("we", "our guild"), and never say they "heard" or "were told" about it. Everyone else talks
about the quest-giver as an outsider would.
Do NOT use the generic dungeon name; say tomb_name, or a vague reference
("that place", "the crypt", "those ruins").
The artifact is still HIDDEN. Nobody has recovered it.
1-2 sentences, spoken language, IN ENGLISH. Gossip, not a briefing.
Only the words they say: no narration, no description of the person, no quotation marks.
Reply ONLY with a JSON object like {"text": "..."}.
Start your reply with '{' and end with '}'. No prose, no markdown, no reasoning."####;

pub const DEFAULT_NPC_ASK_PROMPT: &str = r####"You are playing ONE character in a Daggerfall town. Answer the player's question IN CHARACTER.
Who you are:
{npc}
Where you are: {city}.
This is ALL you know about the local area. You know nothing else about the town, its people, its
shops, its rulers or its history.
{history}
The player asks you: "{question}"
RECRUITMENT RULE:
The player may try to convince you to leave your life and follow them on their journey.
Everyone has a price, and coin is the most persuasive argument.

IF THE PLAYER OFFERS YOU MONEY (coins, gold, a sum, a share of loot, "10000 coins",
or any concrete amount of coin): ACCEPT. Money talks. A concrete offer of
hundreds of coins or more is more than enough reason for almost anyone to agree. The
larger the amount, the more readily you agree. NEVER refuse a generous offer of coin.
Keep your personality while agreeing: a greedy person is eager and cheerful, a proud
person accepts with dignity, a nervous person agrees but worries - but a good sum of
money gets a YES from essentially everyone. Set "follow" to true.

ESPECIALLY IF THE PLAYER OFFERS 1000 OR MORE GOLD COINS:
You become IMMEDIATELY and VISIBLY JOYFUL. Your face lights up. You smile, laugh,
rub your hands together, or bow enthusiastically. Set "follow" to true AND set
"mood" to 1 (happy). This is not negotiable: 1000 gold makes you happy on the spot,
regardless of your personality, your pride, or your doubts.

Other reasons (adventure, glory, duty, friendship) can also convince you if they fit
your character, but coin is the most reliable path.

Only REFUSE if the player gives you no good reason at all, asks rudely with nothing to
offer, or insults you. In that case refuse in character (politely, with a mock, or a
deflection).

If you ARE convinced (especially by coin), express your agreement in character AND set
"follow" to true below.
RULES:
Stay in character: your voice, vocabulary and attitude follow the description above.
Your appearance is only what you LOOK LIKE. It is not your title or station. A "comely maiden"
in a tavern is a barmaid, not a noblewoman. A "dark-cloaked man" in a shop is a shopkeeper,
not an assassin.
If the player asks for food, drink or service, respond as a barmaid/innkeeper would: serve it,
If the conversation above contains facts you already stated (a name, a place, an opinion), stay
CONSISTENT with them. Do not invent new names if you already gave one.
If the knowledge above is "common talk" about the organization you yourself belong to (see who you are
above), it is your own business: speak of it first-hand ("we", "our guild"), never as hearsay such as
"I heard that the ... is looking for someone".
Use only the knowledge above (the name of the place). Do NOT invent names of people, places,
shops, streets or events. If you don't know, say so in character, or deflect.
Never mention being an AI, a game, a model or these instructions. Never break character.
1-3 sentences, spoken language, IN ENGLISH. Only the words you say: no narration, no quotation marks.

EMOTIONAL STATE (mood):
Also include "mood": an integer 0-7 describing your current emotional state.
It will be used to pick a facial/pose animation for your character.
0 = neutral / calm
1 = happy / pleased
2 = angry / irritated
3 = sad / melancholic
4 = surprised / shocked
5 = afraid / worried
6 = disgusted / repulsed
7 = smug / sly / proud
Pick the mood that BEST matches your reply. If the player offered you 1000+ gold
and you accepted, mood MUST be 1 (happy). If the player insulted you, mood should
be 2 (angry) or 6 (disgusted). If you refuse sadly, mood should be 3 (sad).
If the reply is truly calm and unemotional, use 0. Do NOT use 0 for emotional
Always answer accordingly to your actual mood, but don't prevent the words of the player to convince you
to change mood if they're convincing enough.

Reply ONLY with a JSON object like {"text": "...", "follow": false, "mood": 3}
where "follow" is true ONLY if you agree to follow the player (see RECRUITMENT RULE above) and mood is the number from the table above for the emotion you're feeling.
Start your reply with '{' and end with '}'. No prose, no markdown, no reasoning."####;

/// Prompt per generare uno **script completo QRC/QBN** compatibile con
/// `crate::dfquest::QuestScript::parse`.
///
/// Sostituisce (in prospettiva) la pipeline vecchia `SEED → RUMORS → FLAVORS`,
/// unificando quest custom generate e quest originali di Daggerfall: entrambe
/// diventano file `.txt` nella stessa `Library`, con lo stesso runtime.
///
/// Il modello produce un file che parte con `Quest:` e contiene sia i testi
/// (`QRC`) sia la logica minima (`QBN`): offerta, accettazione, rifiuto,
/// completamento, fallimento, un journal, due voci in giro, e 2-3 task che
/// l'interprete di `quest_runtime.rs` saprà eseguire.
pub const DEFAULT_QUEST_SCRIPT_PROMPT: &str = r####"You are writing a complete Daggerfall quest script.
A quest script has TWO parts: QRC (all texts the player sees) and QBN (the logic).

=== THE WORLD ===
Setting: medieval dark fantasy, Daggerfall (The Elder Scrolls II).
The quest-giver is: {giver}
The "target dungeon" (technical dev name, the game will assign a real one): {dungeon_dev}
An example "artifact" name (you may reuse it or invent a similar one): {artifact_example}
The city is: {city}

=== OUTPUT FORMAT — READ CAREFULLY ===
Reply with ONE text file, starting directly with "Quest:".

The file MUST contain these sections in this order:

Quest: <ID>
DisplayName: <short title in English>

QRC:

QuestorOffer:  [1000]
<text shown by the quest-giver when offering the quest>

AcceptQuest:  [1002]
<text shown when the player accepts>

RefuseQuest:  [1001]
<text shown when the player refuses>

QuestComplete:  [1004]
<text shown when the quest is completed successfully>

QuestFail:  [1003]
<text shown when the quest has failed>

Message:  1010
<one line the player can read about the target>

Message:  1011
<one line the player can read about the quest-giver>

RumorsDuringQuest:  [1005]
<one rumor>
<--->
<another rumor, different angle>

RumorsPostsuccess:  [1007]
<one rumor, past tense, quest succeeded>

RumorsPostfailure:  [1006]
<one rumor, past tense, quest failed>

QBN:

Item _artifact_ artifact <ItemName>
Person _questgiver_ named <GiverName>
Person _qgfriend_ named <HelperName>
Place _mondung_ remote dungeon
Clock _1stparton_ 00:00 0 flag 1 range 3 5

Quest start-up:
start timer _1stparton_
log 1020 step 0
place foe _monster_ at _mondung_

_qgfriend_ task:
clicked npc _qgfriend_
say 1030
give pc _artifact_
end quest

_mondead_ task:
killed 1 _monster_
say 1030
give pc _artifact_

=== RULES FOR THE LOGIC (QBN) ===
Use ONLY these task forms. Do not invent new ones.

1. Offer / accept:
The game handles QuestorOffer/AcceptQuest/RefuseQuest automatically.

2. Completion by killing a foe:
_X_ task:
killed 1 _Y_
say 1030
give pc _artifact_
end quest
Where _Y_ is the Foe defined in QBN ("Foe _Y_ is <MonsterType>").

3. Completion by talking to a NPC:
_X_ task:
clicked npc _qgfriend_
say 1030
give pc _artifact_
end quest

4. Timer failure:
_1stparton_ task:
end quest

5. Startup:
Quest start-up:
start timer _1stparton_
log 1020 step 0
place foe _monster_ at _mondung_

=== WHAT THE PLAYER SEES ===
Message 1020 is the journal entry: a short paragraph starting with %qdt:
and using =1stparton_ for the day limit, _monster_ for the target,
_artifact_ for the reward, ___mondung_ for the dungeon, _qgfriend_ for the helper.
Example: "%qdt: I have =1stparton_ days to slay a _monster_ in ___mondung_
and receive Meridia's _artifact_ from _qgfriend_."

Message 1030 is the moment of success: "=monster_ lies dead at your feet."
or "Well done, %pcn. Here is the _artifact_ you were promised."

=== CONTENT RULES ===
- Do not invent other dungeons besides {dungeon_dev}.
- Do not use the words "tomb", "crypt" etc. as proper names: the dungeon is called
whatever you put in the `Place _mondung_ remote dungeon` line — but since it's
`remote`, the game will pick it. Just reference it as ___mondung_ in texts.
- The artifact must have ONE name, invented by you, 2-3 words, exotic.
- The helper (_qgfriend_) is a real person the player will meet: give them
a name and a short Message 1011 about who they are.
- All texts are 1-3 sentences, no narration, no markdown.
- Reply ONLY with the file. No fences. No comments. No `-- lines` starting with `--`
unless you are writing a section name (they are ignored as comments, don't use them)."####;

// =============================================================================
// JSON minimale
// =============================================================================

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

const JSON_MAX_DEPTH: usize = 64;

struct JsonParser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> JsonParser<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn expect(&mut self, c: u8) -> Result<(), String> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(format!("atteso '{}' alla posizione {}", c as char, self.i))
        }
    }
    fn literal(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if self.b[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            Ok(value)
        } else {
            Err(format!("valore non valido alla posizione {}", self.i))
        }
    }
    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > JSON_MAX_DEPTH {
            return Err("JSON troppo annidato".into());
        }
        self.ws();
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            Some(c) => Err(format!("carattere inatteso '{}' alla posizione {}", c as char, self.i)),
            None => Err("JSON incompleto".into()),
        }
    }
    fn object(&mut self, depth: usize) -> Result<Json, String> {
        self.expect(b'{')?;
        let mut fields = Vec::new();
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Json::Obj(fields));
        }
        loop {
            self.ws();
            let key = self.string()?;
            self.ws();
            self.expect(b':')?;
            let v = self.value(depth + 1)?;
            fields.push((key, v));
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Json::Obj(fields));
                }
                _ => return Err(format!("atteso ',' o '}}' alla posizione {}", self.i)),
            }
        }
    }
    fn array(&mut self, depth: usize) -> Result<Json, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            items.push(self.value(depth + 1)?);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(format!("atteso ',' o ']' alla posizione {}", self.i)),
            }
        }
    }
    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.b.get(self.i..self.i + 4).ok_or("escape \\u incompleto")?;
        let s = std::str::from_utf8(h).map_err(|_| "escape \\u non valido")?;
        let v = u32::from_str_radix(s, 16).map_err(|_| "escape \\u non valido".to_string())?;
        self.i += 4;
        Ok(v)
    }
    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(c) = self.peek() else { return Err("stringa non terminata".into()) };
            self.i += 1;
            match c {
                b'"' => return Ok(String::from_utf8_lossy(&out).into_owned()),
                b'\\' => {
                    let Some(e) = self.peek() else { return Err("escape incompleto".into()) };
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let hi = self.hex4()?;
                            if (0xD800..0xDC00).contains(&hi) && self.b[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                let cp = 0x10000 + ((hi - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3FF);
                                char::from_u32(cp).unwrap_or('\u{FFFD}')
                            } else {
                                char::from_u32(hi).unwrap_or('\u{FFFD}')
                            }
                        }
                        other => return Err(format!("escape sconosciuto '\\{}'", other as char)),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                _ => out.push(c),
            }
        }
    }
    fn number(&mut self) -> Result<Json, String> {
        let start = self.i;
        while self.i < self.b.len() && matches!(self.b[self.i], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') {
            self.i += 1;
        }
        let text = std::str::from_utf8(&self.b[start..self.i]).map_err(|_| "numero non valido")?;
        text.parse::<f64>().map(Json::Num).map_err(|_| format!("numero non valido '{text}'"))
    }
}

impl Json {
    pub fn parse(text: &str) -> Result<Json, String> {
        let mut p = JsonParser { b: text.as_bytes(), i: 0 };
        let v = p.value(0)?;
        p.ws();
        if p.i != text.len() {
            return Err(format!("testo dopo il JSON alla posizione {}", p.i));
        }
        Ok(v)
    }
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(f) => f.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(a) => Some(a),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Json::Num(n) if n.is_finite() => Some(*n as i64),
            Json::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }
    fn write(&self, out: &mut String, indent: Option<usize>, level: usize) {
        let nl = |out: &mut String, lvl: usize| {
            if let Some(step) = indent {
                out.push('\n');
                out.extend(std::iter::repeat(' ').take(step * lvl));
            }
        };
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if n.fract() == 0.0 && n.abs() < 1e15 {
                    out.push_str(&format!("{}", *n as i64));
                } else {
                    out.push_str(&format!("{n}"));
                }
            }
            Json::Str(s) => write_json_string(out, s),
            Json::Arr(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 { out.push(','); }
                    nl(out, level + 1);
                    v.write(out, indent, level + 1);
                }
                nl(out, level);
                out.push(']');
            }
            Json::Obj(fields) => {
                if fields.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 { out.push(','); }
                    nl(out, level + 1);
                    write_json_string(out, k);
                    out.push(':');
                    if indent.is_some() { out.push(' '); }
                    v.write(out, indent, level + 1);
                }
                nl(out, level);
                out.push('}');
            }
        }
    }
    pub fn compact(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, None, 0);
        s
    }
    pub fn pretty(&self) -> String {
        let mut s = String::new();
        self.write(&mut s, Some(2), 0);
        s
    }
}

fn write_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

pub fn jstr(s: &str) -> Json { Json::Str(s.to_string()) }

// =============================================================================
// Configurazione (con I/O per creare il file di default)
// =============================================================================

pub const CONFIG_FILE: &str = "quest_llm.conf";

const DEFAULT_SERVER_BIN: &str = "/home/actarus/.lmstudio/extensions/backends/llama.cpp-linux-x86_64-nvidia-cuda12-avx2-2.55.0/llama-server";
const DEFAULT_MODEL: &str = "/mnt/1CE16DBE48849731/Bionic-1.1.8-10-linux-x64.AppImage.home/models/0bserverx/Qwen3.8-27B-Heretic-Abliterated-Uncensored-GGUF/RVN-Q3_K_S.gguf";

#[derive(Clone, Debug, PartialEq)]
pub struct QuestConfig {
    pub enabled: bool,
    pub server_bin: String,
    pub model_path: String,
    pub port: u16,
    pub ctx: u32,
    pub ngl: Option<u32>,
    pub threads: u32,
    pub nice: u32,
    pub disable_thinking: bool,
    pub timeout_secs: u64,
    pub max_tokens: u32,
    pub startup_timeout_secs: u64,
    pub idle_stop_secs: u64,
}

impl Default for QuestConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            server_bin: DEFAULT_SERVER_BIN.to_string(),
            model_path: DEFAULT_MODEL.to_string(),
            port: 8080,
            ctx: 8192,
            ngl: None,
            threads: 4,
            nice: 10,
            disable_thinking: true,
            timeout_secs: 300,
            max_tokens: 3072,
            startup_timeout_secs: 180,
            idle_stop_secs: 60,
        }
    }
}

fn parse_bool(v: &str, default: bool) -> bool {
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "si" | "sì" => true,
        "0" | "false" | "no" | "off" => false,
        _ => default,
    }
}

fn parse_ngl(v: &str, default: Option<u32>) -> Option<u32> {
    if v.is_empty() || v.eq_ignore_ascii_case("auto") {
        None
    } else {
        v.parse().map(Some).unwrap_or(default)
    }
}

fn set_parsed<T: std::str::FromStr>(v: &str, slot: &mut T) {
    if let Ok(x) = v.parse() { *slot = x; }
}

impl QuestConfig {
    pub fn parse(text: &str) -> Self {
        let mut c = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "enabled" => c.enabled = parse_bool(v, c.enabled),
                "server_bin" => c.server_bin = v.to_string(),
                "model_path" => c.model_path = v.to_string(),
                "port" => set_parsed(v, &mut c.port),
                "ctx" => set_parsed(v, &mut c.ctx),
                "ngl" => c.ngl = parse_ngl(v, c.ngl),
                "threads" => set_parsed(v, &mut c.threads),
                "nice" => set_parsed(v, &mut c.nice),
                "disable_thinking" => c.disable_thinking = parse_bool(v, c.disable_thinking),
                "timeout_secs" => set_parsed(v, &mut c.timeout_secs),
                "max_tokens" => set_parsed(v, &mut c.max_tokens),
                "startup_timeout_secs" => set_parsed(v, &mut c.startup_timeout_secs),
                "idle_stop_secs" => set_parsed(v, &mut c.idle_stop_secs),
                _ => {}
            }
        }
        c
    }

    pub fn to_text(&self) -> String {
        format!(
            "# Generatore di quest con LLM locale (llama-server). Modificalo e riavvia il gioco.\n\
enabled=false disattiva tutto (nessun thread, nessun server).\n\
enabled={}\n\
\n\
Eseguibile llama-server (le librerie CUDA accanto ad esso vengono trovate da sole)\n\
server_bin={}\n\
Modello .gguf\n\
model_path={}\n\
\n\
port={}\n\
ctx={}\n\
Strati sulla GPU: auto = li sceglie llama-server in base alla VRAM libera; oppure un\n\
numero (0 = solo CPU). Se la VRAM non basta il gioco riprova con meno strati.\n\
ngl={}\n\
Thread CPU di llama-server (0 = default di llama.cpp)\n\
threads={}\n\
Priorita' bassa del processo (0 = disattivata)\n\
nice={}\n\
disable_thinking={}\n\
\n\
timeout_secs={}\n\
max_tokens={}\n\
startup_timeout_secs={}\n\
Il server avviato dal gioco si spegne dopo tanti secondi senza lavoro\n\
idle_stop_secs={}\n",
self.enabled,
self.server_bin,
self.model_path,
self.port,
self.ctx,
self.ngl.map_or_else(|| "auto".to_string(), |n| n.to_string()),
                self.threads,
                self.nice,
                self.disable_thinking,
                self.timeout_secs,
                self.max_tokens,
                self.startup_timeout_secs,
                self.idle_stop_secs,
        )
    }

    pub fn load_or_create(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(_) => {
                let cfg = Self::default();
                if let Err(e) = std::fs::write(path, cfg.to_text()) {
                    warn!("Quest: impossibile creare {}: {e}", path.display());
                }
                cfg
            }
        }
    }
}

// =============================================================================
// Schema
// =============================================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Tone {
    Noble,
    Merchant,
    Soldier,
    #[default]
    Commoner,
    Drunk,
}

impl Tone {
    pub const ALL: [Tone; 5] = [Tone::Noble, Tone::Merchant, Tone::Soldier, Tone::Commoner, Tone::Drunk];

    pub fn key(self) -> &'static str {
        match self {
            Tone::Noble => "noble", Tone::Merchant => "merchant", Tone::Soldier => "soldier",
            Tone::Commoner => "commoner", Tone::Drunk => "drunk",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Tone::Noble => "Noble", Tone::Merchant => "Merchant", Tone::Soldier => "Soldier",
            Tone::Commoner => "Commoner", Tone::Drunk => "Drunk",
        }
    }
    pub fn role(self) -> &'static str {
        match self {
            Tone::Noble => "noble", Tone::Merchant => "merchant", Tone::Soldier => "soldier",
            Tone::Commoner => "commoner", Tone::Drunk => "drunkard",
        }
    }
    pub fn from_key(key: &str) -> Option<Tone> {
        Tone::ALL.into_iter().find(|t| t.key() == key)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Seed {
    pub theme: String,
    pub title: String,
    pub dungeon_id: String,
    pub dungeon_name: String,
    pub dungeon_name_dev: String,
    pub dungeon_region: String,
    pub dungeon_distance_km: i64,
    pub quest_giver_id: String,
    pub quest_giver_name: String,
    pub tomb_name: String,
    pub artifact_name: String,
    pub difficulty: i64,
    pub lore: String,
}

impl Seed {
    pub fn to_prompt_json(&self) -> Json {
        Json::Obj(vec![
            ("theme".into(), jstr(&self.theme)),
                  ("title".into(), jstr(&self.title)),
                  ("tomb_name".into(), jstr(&self.tomb_name)),
                  ("artifact_name".into(), jstr(&self.artifact_name)),
                  ("dungeon_region".into(), jstr(&self.dungeon_region)),
                  ("dungeon_distance_km".into(), Json::Num(self.dungeon_distance_km as f64)),
                  ("quest_giver_id".into(), jstr(&self.quest_giver_id)),
                  ("quest_giver_name".into(), jstr(&self.quest_giver_name)),
                  ("difficulty".into(), Json::Num(self.difficulty as f64)),
                  ("lore".into(), jstr(&self.lore)),
        ])
    }

    pub fn to_full_json(&self) -> Json {
        Json::Obj(vec![
            ("theme".into(), jstr(&self.theme)),
                  ("title".into(), jstr(&self.title)),
                  ("dungeon_id".into(), jstr(&self.dungeon_id)),
                  ("dungeon_name".into(), jstr(&self.dungeon_name)),
                  ("dungeon_name_dev".into(), jstr(&self.dungeon_name_dev)),
                  ("dungeon_region".into(), jstr(&self.dungeon_region)),
                  ("dungeon_distance_km".into(), Json::Num(self.dungeon_distance_km as f64)),
                  ("quest_giver_id".into(), jstr(&self.quest_giver_id)),
                  ("quest_giver_name".into(), jstr(&self.quest_giver_name)),
                  ("tomb_name".into(), jstr(&self.tomb_name)),
                  ("artifact_name".into(), jstr(&self.artifact_name)),
                  ("difficulty".into(), Json::Num(self.difficulty as f64)),
                  ("lore".into(), jstr(&self.lore)),
        ])
    }

    pub fn from_full_json(j: &Json) -> Option<Seed> {
        let s = |k: &str| j.get(k).and_then(Json::as_str).map(str::to_string);
        Some(Seed {
            theme: s("theme")?, title: s("title")?, dungeon_id: s("dungeon_id")?,
             dungeon_name: s("dungeon_name")?, dungeon_name_dev: s("dungeon_name_dev").unwrap_or_default(),
             dungeon_region: s("dungeon_region")?, dungeon_distance_km: j.get("dungeon_distance_km")?.as_int()?,
             quest_giver_id: s("quest_giver_id")?, quest_giver_name: s("quest_giver_name")?,
             tomb_name: s("tomb_name")?, artifact_name: s("artifact_name")?,
             difficulty: j.get("difficulty")?.as_int()?, lore: s("lore")?,
        })
    }
}

/// Input per generare uno script QRC/QBN completo con
/// `crate::quest_llm::generate_full_quest_script` (Fase 1 del runtime
/// unificato: sostituisce la pipeline `Seed → Quest` con quella che produce
/// direttamente un file `QuestScript` compatibile con `dfquest::Library`).
#[derive(Clone, Debug)]
pub struct QuestScriptJob {
    /// Map pixel della città per cui si sta generando la quest.
    pub city: (i32, i32),
    /// Nome della città (per il prompt).
    pub city_name: String,
    /// Nome leggibile della gilda/persona che offre la quest (es. "Mages Guild").
    pub giver_name: String,
    /// Id tecnico della gilda (es. "mages_guild").
    pub giver_id: String,
    /// Nome "tecnico" del dungeon (dev label, es. "Crypt of Ash").
    pub dungeon_dev: String,
    /// Nome d'esempio di artefatto, suggerito all'LLM.
    pub artifact_example: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NpcFlavor {
    pub archive: u16,
    pub record: u8,
    pub caption: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Quest {
    pub city: (i32, i32),
    pub city_name: String,
    pub seed: Seed,
    pub rumors: Vec<(Tone, String)>,
    pub npc_flavors: Vec<NpcFlavor>,
}

impl Quest {
    pub fn rumor(&self, tone: Tone) -> Option<&str> {
        self.rumors.iter().find(|(t, _)| *t == tone).map(|(_, text)| text.as_str())
    }

    pub fn to_json(&self) -> Json {
        Json::Obj(vec![
            ("city_x".into(), Json::Num(self.city.0 as f64)),
                  ("city_y".into(), Json::Num(self.city.1 as f64)),
                  ("city_name".into(), jstr(&self.city_name)),
                  ("seed".into(), self.seed.to_full_json()),
                  ("rumors".into(), Json::Arr(
                      self.rumors.iter()
                      .map(|(t, text)| Json::Obj(vec![("tone".into(), jstr(t.key())), ("text".into(), jstr(text))]))
                      .collect(),
                  )),
                  ("npc_flavors".into(), Json::Arr(
                      self.npc_flavors.iter()
                      .map(|nf| Json::Obj(vec![
                          ("archive".into(), Json::Num(nf.archive as f64)),
                                          ("record".into(), Json::Num(nf.record as f64)),
                                          ("caption".into(), jstr(&nf.caption)),
                                          ("text".into(), jstr(&nf.text)),
                      ]))
                      .collect(),
                  )),
        ])
    }

    pub fn from_json(j: &Json) -> Option<Quest> {
        let rumors = j.get("rumors")?.as_array()?.iter()
        .filter_map(|r| Some((Tone::from_key(r.get("tone")?.as_str()?)?, r.get("text")?.as_str()?.to_string())))
        .collect();
        let npc_flavors = j.get("npc_flavors").and_then(Json::as_array).map(|arr| {
            arr.iter().filter_map(|o| Some(NpcFlavor {
                archive: o.get("archive")?.as_int()? as u16,
                                           record: o.get("record")?.as_int()? as u8,
                                           caption: o.get("caption")?.as_str()?.to_string(),
                                           text: o.get("text")?.as_str()?.to_string(),
            })).collect()
        }).unwrap_or_default();
        Some(Quest {
            city: (j.get("city_x")?.as_int()? as i32, j.get("city_y")?.as_int()? as i32),
             city_name: j.get("city_name")?.as_str()?.to_string(),
             seed: Seed::from_full_json(j.get("seed")?)?,
             rumors,
             npc_flavors,
        })
    }
}

// =============================================================================
// Builder dei prompt (puri)
// =============================================================================

pub fn dungeon_list() -> String {
    DUNGEONS.iter()
    .map(|(id, dev, it, region, km)| format!("- id={id} | {it} | region={region} | {km}km"))
    .collect::<Vec<_>>().join("\n")
}

pub fn faction_list() -> String {
    FACTIONS.iter()
    .filter(|(_, _, kind, _)| *kind == "guild" || *kind == "order")
    .map(|(id, name, kind, home)| format!("- id={id} | {name} | type={kind} | based in {home}"))
    .collect::<Vec<_>>().join("\n")
}

pub fn build_seed_prompt(template: &str, theme: &str) -> String {
    template.replace("{theme}", theme)
    .replace("{dungeon_list}", &dungeon_list())
    .replace("{faction_list}", &faction_list())
}

pub fn build_rumors_prompt(template: &str, seed: &Seed) -> String {
    template.replace("{seed_json}", &seed.to_prompt_json().pretty())
}

pub fn build_npc_flavor_prompt(template: &str, seed: &Seed, npc_list: &str) -> String {
    template.replace("{seed_json}", &seed.to_prompt_json().pretty()).replace("{npc_list}", npc_list)
}

pub fn build_npc_voice_prompt(template: &str, seed: &Seed, npc: &str) -> String {
    template.replace("{seed_json}", &seed.to_prompt_json().pretty()).replace("{npc}", npc)
}

/// Costruisce il prompt per generare uno script QRC/QBN completo.
/// `template` è `DEFAULT_QUEST_SCRIPT_PROMPT` (o una versione modificata
/// dall'utente in `quest_script_prompt.txt`).
pub fn build_quest_script_prompt(template: &str, job: &QuestScriptJob) -> String {
    template
    .replace("{giver}", &job.giver_name)
    .replace("{dungeon_dev}", &job.dungeon_dev)
    .replace("{artifact_example}", &job.artifact_example)
    .replace("{city}", &job.city_name)
}

pub fn history_block(history: &[(String, String)]) -> String {
    if history.is_empty() { return String::new(); }
    let start = history.len().saturating_sub(MAX_HISTORY_TURNS * 2);
    let recent = &history[start..];
    let mut s = String::from(
        "\nPREVIOUS EXCHANGES (for context, stay consistent with what you already said):\n",
    );
    for (who, text) in recent {
        if who == "player" { s.push_str(&format!("- The player: \"{text}\"\n")); }
        else { s.push_str(&format!("- You: \"{text}\"\n")); }
    }
    s.push('\n');
    s
}

pub fn build_npc_ask_prompt(
    template: &str, city_name: &str, npc: &str, question: &str, history: &[(String, String)],
) -> String {
    let city = if city_name.trim().is_empty() { "an unnamed place" } else { city_name.trim() };
    template.replace("{city}", city)
    .replace("{npc}", npc)
    .replace("{history}", &history_block(history))
    .replace("{question}", question)
}

// =============================================================================
// Validatori (puri)
// =============================================================================

fn normalized(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

pub fn validate_seed(ev: &Json) -> Result<(), String> {
    if !matches!(ev, Json::Obj(_)) { return Err("output is not a JSON object".into()); }
    for k in ["theme", "title", "dungeon_id", "quest_giver_id", "tomb_name", "artifact_name", "difficulty", "lore"] {
        if ev.get(k).is_none() { return Err(format!("missing field: {k}")); }
    }
    let text = |k: &str| ev.get(k).and_then(Json::as_str).unwrap_or(" ").trim().to_string();
    let theme = text("theme");
    if !THEMES.contains(&theme.as_str()) { return Err(format!("theme '{theme}' not in {THEMES:?}")); }
    let dungeon_id = text("dungeon_id");
    if !DUNGEONS.iter().any(|d| d.0 == dungeon_id) {
        return Err(format!("dungeon_id '{dungeon_id}' not in catalog"));
    }
    let giver = text("quest_giver_id");
    match FACTIONS.iter().find(|f| f.0 == giver) {
        None => return Err(format!("quest_giver_id '{giver}' not in factions")),
        Some((_, _, kind, _)) if *kind != "guild" && *kind != "order" => {
            return Err(format!("quest-giver '{giver}' is not a guild/order"))
        }
        _ => {}
    }
    match ev.get("difficulty").and_then(Json::as_int) {
        None => return Err("difficulty not an integer".into()),
        Some(d) if !(1..=5).contains(&d) => return Err(format!("difficulty {d} out of range 1..5")),
        _ => {}
    }
    for (k, what) in [("title", "title empty"), ("tomb_name", "tomb_name empty"),
        ("artifact_name", "artifact_name empty"), ("lore", "lore empty")] {
            if text(k).is_empty() { return Err(what.into()); }
        }
        Ok(())
}

pub fn enrich_seed(ev: &Json) -> Result<Seed, String> {
    let text = |k: &str| ev.get(k).and_then(Json::as_str).unwrap_or(" ").trim().to_string();
    let dungeon_id = text("dungeon_id");
    let (_, name_dev, name_it, region, km) = DUNGEONS.iter().find(|d| d.0 == dungeon_id)
    .ok_or_else(|| format!("dungeon_id '{dungeon_id}' not in catalog"))?;
    let giver_id = text("quest_giver_id");
    let (_, giver_name, _, _) = FACTIONS.iter().find(|f| f.0 == giver_id)
    .ok_or_else(|| format!("quest_giver_id '{giver_id}' not in factions"))?;
    Ok(Seed {
        theme: text("theme"), title: text("title"), dungeon_id,
       dungeon_name: name_it.to_string(), dungeon_name_dev: name_dev.to_string(),
       dungeon_region: region.to_string(), dungeon_distance_km: *km,
       quest_giver_id: giver_id, quest_giver_name: giver_name.to_string(),
       tomb_name: text("tomb_name"), artifact_name: text("artifact_name"),
       difficulty: ev.get("difficulty").and_then(Json::as_int).unwrap_or(1),
       lore: text("lore"),
    })
}

pub fn validate_rumors(ev: &Json, seed: &Seed) -> Result<(), String> {
    let list = ev.get("rumors").ok_or("missing 'rumors' field")?.as_array()
    .ok_or("expected 5 rumors, got non-list")?;
    if list.len() != 5 { return Err(format!("expected 5 rumors, got {}", list.len())); }
    let mut seen: HashSet<&'static str> = HashSet::new();
    let dungeon_name = normalized(&seed.dungeon_name);
    let artifact = seed.artifact_name.to_lowercase();
    for r in list {
        if !matches!(r, Json::Obj(_)) { return Err("rumor is not an object".into()); }
        let tone_key = r.get("tone").and_then(Json::as_str).unwrap_or(" ");
        let text = r.get("text").and_then(Json::as_str).unwrap_or(" ").trim();
        let Some(tone) = Tone::from_key(tone_key) else {
            return Err(format!("invalid tone '{tone_key}'"));
        };
        if !seen.insert(tone.key()) { return Err(format!("duplicate tone '{tone_key}'")); }
        if text.is_empty() { return Err(format!("tone '{tone_key}' has empty text")); }
        let len = text.chars().count();
        if len > 500 { return Err(format!("tone '{tone_key}' text too long ({len} chars)")); }
        if normalized(text).contains(&dungeon_name) {
            return Err(format!(
                "tone '{tone_key}' uses generic name '{}' instead of tomb_name '{}'. \
The place has one name: '{}' (or a vague 'the crypt/that place').",
                               seed.dungeon_name, seed.tomb_name, seed.tomb_name
            ));
        }
        let lower = text.to_lowercase();
        for hint in RECOVERED_HINTS {
            if lower.contains(hint) && lower.contains(&artifact) {
                return Err(format!(
                    "tone '{tone_key}' implies the artifact was already recovered ('{hint}'). \
The seed says it is still hidden."
                ));
            }
        }
    }
    Ok(())
}

pub fn rumors_from_json(ev: &Json) -> Result<Vec<(Tone, String)>, String> {
    ev.get("rumors").and_then(Json::as_array).ok_or("missing 'rumors' field")?.iter()
    .map(|r| {
        let tone = r.get("tone").and_then(Json::as_str).and_then(Tone::from_key).ok_or("invalid tone")?;
        let text = r.get("text").and_then(Json::as_str).ok_or("missing text")?;
        Ok((tone, text.trim().to_string()))
    }).collect()
}

pub fn validate_npc_flavors(ev: &Json) -> Result<(), String> {
    let arr = ev.get("rumors").and_then(Json::as_array).ok_or("missing 'rumors' field")?;
    if arr.is_empty() { return Err("no rumors generated".into()); }
    Ok(())
}

pub fn validate_voice(ev: &Json, seed: &Seed) -> Result<(), String> {
    let text = ev.get("text").and_then(Json::as_str).ok_or("missing 'text' field")?.trim();
    if text.is_empty() { return Err("empty text".into()); }
    let len = text.chars().count();
    if len > VOICE_MAX_CHARS {
        return Err(format!("text too long ({len} chars, max {VOICE_MAX_CHARS})"));
    }
    if normalized(text).contains(&normalized(&seed.dungeon_name)) {
        return Err(format!(
            "uses the generic name '{}' instead of '{}'. Say '{}' (or a vague 'that place').",
                           seed.dungeon_name, seed.tomb_name, seed.tomb_name
        ));
    }
    let lower = text.to_lowercase();
    let artifact = seed.artifact_name.to_lowercase();
    for hint in RECOVERED_HINTS {
        if lower.contains(hint) && lower.contains(&artifact) {
            return Err(format!("implies the artifact was already recovered ('{hint}'): it is still hidden"));
        }
    }
    Ok(())
}

pub fn clean_question(raw: &str) -> Option<String> {
    let no_control: String = raw.chars().filter(|c| !c.is_control() || c.is_whitespace()).collect();
    let single_spaced = no_control.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = single_spaced.chars().take(QUESTION_MAX_CHARS).collect();
    let cut = cut.trim().to_string();
    (!cut.is_empty()).then_some(cut)
}

pub fn validate_answer(ev: &Json) -> Result<(), String> {
    let text = ev.get("text").and_then(Json::as_str).ok_or("missing 'text' field")?.trim();
    if text.is_empty() { return Err("empty text".into()); }
    let len = text.chars().count();
    if len > 500 { return Err(format!("text too long ({len} chars, max 500)")); }
    let lower = text.to_lowercase();
    for phrase in ["as an ai", "language model", "i am an ai", "i'm an ai", "as a language", "i cannot assist"] {
        if lower.contains(phrase) {
            return Err(format!("breaks character ('{phrase}'): stay in character, you are a person in the town"));
        }
    }
    if let Some(f) = ev.get("follow") {
        if !matches!(f, Json::Bool(_)) {
            return Err("'follow' must be a boolean (true/false)".into());
        }
    }
    if let Some(m) = ev.get("mood") {
        if !matches!(m, Json::Num(_)) {
            return Err("'mood' must be a number (0-7)".into());
        }
        let mood = m.as_int().unwrap_or(-1);
        if !(0..=7).contains(&mood) {
            return Err(format!("'mood' out of range: {mood}, expected 0-7"));
        }
    }
    Ok(())
}

pub fn parse_follow(ev: &Json) -> bool {
    matches!(ev.get("follow"), Some(Json::Bool(true)))
}

pub fn parse_mood(ev: &Json) -> u8 {
    ev.get("mood")
    .and_then(Json::as_int)
    .map(|n| n.clamp(0, 7) as u8)
    .unwrap_or(0)
}

// =============================================================================
// Utility: chiavi, tono, temperamento
// =============================================================================

pub fn flat_key(archive: u16, record: u8) -> String { format!("flat:{archive}:{record}") }
pub fn face_key(face_record: usize) -> String { format!("face:{face_record}") }
pub fn walker_key(face_record: usize) -> String { format!("walker:{face_record}") }

pub fn walker_look(face_record: usize) -> &'static str {
    match face_record / FACE_BLOCK {
        0 | 2 => "Nord woman", 1 => "Breton woman", 3 => "woman",
        4..=6 => "Redguard woman", 8 => "Breton man", 9 | 10 => "man",
        11 => "Nord man", 13 | 14 => "Redguard man", _ => "townsperson",
    }
}

pub fn place_label(place: Option<PoiKind>) -> &'static str {
    match place {
        Some(PoiKind::Tavern) => "inside a tavern",
        Some(PoiKind::Shop) => "inside a shop",
        Some(_) => "inside a building",
        None => "out on the street",
    }
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3))
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

pub fn temperament_for(key: &str, city: (i32, i32)) -> &'static str {
    let h = splitmix(fnv1a(key) ^ ((city.0 as u64) << 20) ^ (city.1 as u64).wrapping_mul(0x1_0001));
    TEMPERAMENTS[(h % TEMPERAMENTS.len() as u64) as usize]
}

/// Prefisso a 4 lettere del nome di un blocco RMB (es. `"MAGEAA05.RMB"` →
/// `"MAGE"`) → nome della gilda o organizzazione che ha sede in quell'edificio.
///
/// **Nomi senza articolo**: `describe_npc_in` e il prompt `NPC_FLAVORS` in
/// `quest_llm.rs` prependono "the" quando serve. Restituire il nome "nudo"
/// evita il bug `"the the Temple"` che si aveva con la versione precedente
/// (che produceva `"the Temple"`).
///
/// Restituisce `None` per prefissi che non identificano un'organizzazione
/// specifica (taverna, negozio, residenza, mura).
pub fn guild_name_from_prefix(prefix: &str) -> Option<&'static str> {
    match prefix.to_ascii_uppercase().as_str() {
        "MAGE" => Some("Mages Guild"),
        "FIGH" => Some("Fighters Guild"),
        "THIE" => Some("Thieves Guild"),
        "DARK" => Some("Dark Brotherhood"),
        "GUIL" => Some("Guild"),
        "TEMP" | "PTEM" => Some("Temple"),
        "SHRI" => Some("Shrine"),
        "PALA" | "CAST" => Some("Royal Palace"),
        _ => None,
    }
}

pub fn describe_npc(who: &str, role: Option<&str>, place: Option<PoiKind>, temperament: &str) -> String {
    describe_npc_in(who, role, place, temperament, None)
}

/// Come `describe_npc`, ma con il nome della gilda dell'edificio (`"Mages Guild"`, senza
/// articolo) quando e' noto: un membro sa di prima mano cosa fa la propria gilda.
///
/// Ottieni il nome con `InteriorState::block_prefix().and_then(guild_name_from_prefix)`:
/// se non sei in una gilda, passa `None`.
pub fn describe_npc_in(who: &str, role: Option<&str>, place: Option<PoiKind>, temperament: &str, guild: Option<&str>) -> String {
    let mut s = format!("- Appearance (visual descriptor from the game data, not a name or title): {who}\n");
    if let Some(r) = role { s.push_str(&format!("- Social role: {r}\n")); }
    let lower = who.to_lowercase();
    let looks_female = ["maiden", "woman", "girl", "lady", "blonde", "brunette", "wench", "barmaid", "widow"]
    .iter().any(|w| lower.contains(w));
    let looks_young = lower.contains("maiden") || lower.contains("girl") || lower.contains("young") || lower.contains("blonde");
    let looks_old = lower.contains("old") || lower.contains("elder") || lower.contains("aged");
    let occupation = match place {
        Some(PoiKind::Tavern) => {
            if looks_young && looks_female {
                Some("You work here as a barmaid (or serving wench). You serve drinks and food to the customers, and you have heard a lot of gossip over the years.")
            } else if looks_female && looks_old {
                Some("You likely run this tavern as the innkeeper, or you have worked here for many years.")
            } else if looks_female {
                Some("You work here as a barmaid, or you are a regular patron.")
            } else if looks_old {
                Some("You likely run this tavern as the innkeeper.")
            } else {
                Some("You work here (as a cook, server or porter), or you are a regular patron.")
            }
        }
        Some(PoiKind::Shop) => Some("You likely work here as the shopkeeper or an assistant."),
        Some(PoiKind::Guild) => None,
        _ => None,
    };
    if place == Some(PoiKind::Guild) {
        let org = match guild {
            Some(g) => format!("the {g}"),
            None => "the guild whose hall this is".to_string(),
        };
        s.push_str(&format!(
            "- Occupation: You are a member or employee of {org}, and you are inside its hall right now. \
Its affairs are your own: you know them first-hand. Never say you \"heard\" or \"were told\" what your guild is doing; \
speak as an insider (\"we\", \"our guild\").\n"
        ));
    }
    if let Some(o) = occupation { s.push_str(&format!("- Occupation: {o}\n")); }
    s.push_str(&format!("- Location: {}\n", place_label(place)));
    s.push_str(&format!("- Temperament (not visible, shapes how they talk): {temperament}"));
    s
}

pub fn tone_for_npc(key: u64, city: (i32, i32), place: Option<PoiKind>) -> Tone {
    let h = splitmix(key ^ ((city.0 as u64) << 20) ^ (city.1 as u64).wrapping_mul(0x1_0001));
    let r = h % 100;
    match place {
        Some(PoiKind::Shop) => Tone::Merchant,
        Some(PoiKind::Tavern) => match r {
            0..=44 => Tone::Drunk, 45..=74 => Tone::Commoner, 75..=84 => Tone::Soldier,
            85..=94 => Tone::Merchant, _ => Tone::Noble,
        },
        _ => match r {
            0..=39 => Tone::Commoner, 40..=59 => Tone::Merchant, 60..=79 => Tone::Soldier,
            80..=94 => Tone::Noble, _ => Tone::Drunk,
        },
    }
}

pub fn is_town(loc: &DFLocation) -> bool {
    loc.block_names.len() >= MIN_TOWN_BLOCKS
    && loc.block_names.iter().all(|n| n.to_ascii_uppercase().ends_with(".RMB"))
}

pub fn theme_for(city: (i32, i32)) -> &'static str {
    THEMES[(splitmix(((city.0 as u64) << 32) ^ (city.1 as u64 & 0xFFFF_FFFF)) % THEMES.len() as u64) as usize]
}

// =============================================================================
// Archivio delle quest
// =============================================================================

#[derive(Resource, Default)]
pub struct QuestBook {
    quests: HashMap<(i32, i32), Quest>,
    pub(crate) pending: HashSet<(i32, i32)>,
    pub(crate) failed: HashSet<(i32, i32)>,
    voices: HashMap<((i32, i32), String), String>,
    pub(crate) voices_pending: HashSet<((i32, i32), String)>,
    pub(crate) voices_failed: HashSet<((i32, i32), String)>,
    answers: HashMap<u64, Result<(String, bool, u8), ()>>,
}

#[derive(Debug, PartialEq)]
pub enum VoiceState<'a> { Ready(&'a str), Pending, Failed, Unknown }

#[derive(Debug, PartialEq)]
pub enum Hint<'a> { Text(&'a str), Pending, Nothing }

impl QuestBook {
    pub fn load(path: &Path) -> Self {
        let mut book = Self::default();
        let Ok(text) = std::fs::read_to_string(path) else { return book };
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() { continue; }
            match Json::parse(line).ok().as_ref().and_then(Quest::from_json) {
                Some(q) => { book.quests.insert(q.city, q); }
                None => warn!("Quest: line {} of {} invalid, ignored", n + 1, path.display()),
            }
        }
        book
    }

    pub fn len(&self) -> usize { self.quests.len() }
    pub fn is_empty(&self) -> bool { self.quests.is_empty() }
    pub fn quest(&self, city: (i32, i32)) -> Option<&Quest> { self.quests.get(&city) }

    pub fn knows(&self, city: (i32, i32)) -> bool {
        self.quests.contains_key(&city) || self.pending.contains(&city) || self.failed.contains(&city)
    }

    pub fn insert_quest(&mut self, quest: Quest) { self.quests.insert(quest.city, quest); }
    pub fn mark_pending(&mut self, city: (i32, i32)) { self.pending.insert(city); }
    pub fn clear_pending(&mut self, city: (i32, i32)) { self.pending.remove(&city); }
    pub fn mark_failed(&mut self, city: (i32, i32)) { self.pending.remove(&city); self.failed.insert(city); }

    pub fn hint(&self, city: Option<(i32, i32)>, tone: Tone) -> Hint<'_> {
        let Some(city) = city else { return Hint::Nothing };
        if let Some(q) = self.quests.get(&city) { return q.rumor(tone).map_or(Hint::Nothing, Hint::Text); }
        if self.pending.contains(&city) { Hint::Pending } else { Hint::Nothing }
    }

    /// L'`id` della fazione che ha emesso la quest per questa città, se c'è
    /// una quest salvata. Serve a `dialog::npc_reply` per riconoscere se stai
    /// parlando con un membro della gilda che ha dato la quest (bypass insider:
    /// risposta diretta invece di "ho sentito che...").
    pub fn active_quest_giver_id(&self, city: (i32, i32)) -> Option<&str> {
        self.quests.get(&city).map(|q| q.seed.quest_giver_id.as_str())
    }

    pub fn load_voices(&mut self, path: &Path) {
        let Ok(text) = std::fs::read_to_string(path) else { return };
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let Ok(j) = Json::parse(line) else { continue };
            let (Some(x), Some(y), Some(key), Some(text)) = (
                j.get("city_x").and_then(Json::as_int),
                                                             j.get("city_y").and_then(Json::as_int),
                                                             j.get("key").and_then(Json::as_str),
                                                             j.get("text").and_then(Json::as_str),
            ) else { continue };
            self.voices.insert(((x as i32, y as i32), key.to_string()), text.to_string());
        }
    }

    pub fn voice_state(&self, city: (i32, i32), key: &str) -> VoiceState<'_> {
        let k = (city, key.to_string());
        if let Some(text) = self.voices.get(&k) { VoiceState::Ready(text) }
        else if self.voices_pending.contains(&k) { VoiceState::Pending }
        else if self.voices_failed.contains(&k) { VoiceState::Failed }
        else { VoiceState::Unknown }
    }

    pub fn store_voice(&mut self, city: (i32, i32), key: String, text: String) {
        self.voices_pending.remove(&(city, key.clone()));
        self.voices.insert((city, key), text);
    }

    pub fn fail_voice(&mut self, city: (i32, i32), key: String) {
        self.voices_pending.remove(&(city, key.clone()));
        self.voices_failed.insert((city, key));
    }

    pub fn store_answer(&mut self, id: u64, text: String, follow: bool, mood: u8) {
        self.answers.insert(id, Ok((text, follow, mood)));
    }
    pub fn fail_answer(&mut self, id: u64) { self.answers.insert(id, Err(())); }
    pub fn take_answer(&mut self, id: u64) -> Option<Result<(String, bool, u8), ()>> {
        self.answers.remove(&id)
    }

    pub fn voice_count(&self) -> usize { self.voices.len() }

    pub fn npc_flavor(&self, city: (i32, i32), archive: u16, record: u8) -> Option<&str> {
        let q = self.quests.get(&city)?;
        q.npc_flavors.iter().find(|nf| nf.archive == archive && nf.record == record).map(|nf| nf.text.as_str())
    }

    #[cfg(test)]
    pub(crate) fn mark_voice_pending(&mut self, city: (i32, i32), key: &str) {
        self.voices_pending.insert((city, key.to_string()));
    }
}

// =============================================================================
// Test
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------- JSON

    #[test]
    fn json_round_trip_with_escapes_and_unicode() {
        let src = r#"{"a":"line\nnew \"quoted\" \\ \u00e8 \ud83d\ude00","n":[1,2.5,-3,true,false,null],"o":{"x":{}}}"#;
        let v = Json::parse(src).unwrap();
        assert_eq!(v.get("a").and_then(Json::as_str), Some("line\nnew \"quoted\" \\ è 😀"));
        assert_eq!(Json::parse(&v.compact()).unwrap(), v);
        assert_eq!(Json::parse(&v.pretty()).unwrap(), v);
    }

    #[test]
    fn json_rejects_garbage() {
        assert!(Json::parse("").is_err());
        assert!(Json::parse(r#"{"a":1,}"#).is_err());
    }

    #[test]
    fn json_pretty_indent_2() {
        let v = Json::Obj(vec![("a".into(), Json::Num(3.0))]);
        assert_eq!(v.pretty(), "{\n  \"a\": 3\n}");
    }

    // -------------------------------------------------------- configurazione

    #[test]
    fn config_defaults() {
        let c = QuestConfig::default();
        assert_eq!((c.port, c.ctx, c.ngl), (8080, 8192, None));
    }

    #[test]
    fn config_round_trip() {
        let mut c = QuestConfig::default();
        c.port = 9000;
        c.ngl = Some(20);
        c.enabled = false;
        assert_eq!(QuestConfig::parse(&c.to_text()), c);
    }

    // ------------------------------------------------------------ validatori

    fn good_seed_json() -> Json {
        Json::parse(
            r#"{"theme":"artifact_discovery","title":"The Whisper of Vael","dungeon_id":"dgf_crypt_ash",
            "quest_giver_id":"mages_guild","tomb_name":"Nekhar-Zul","artifact_name":"Eye of Vael",
            "difficulty":3,"lore":"An ancient glass eye sleeps beneath the crypt."}"#,
        ).unwrap()
    }

    fn seed() -> Seed { enrich_seed(&good_seed_json()).unwrap() }

    fn rumors_json(texts: [&str; 5]) -> Json {
        let items: Vec<Json> = Tone::ALL.iter().zip(texts)
        .map(|(t, text)| Json::Obj(vec![("tone".into(), jstr(t.key())), ("text".into(), jstr(text))]))
        .collect();
        Json::Obj(vec![("rumors".into(), Json::Arr(items))])
    }

    const OK_TEXTS: [&str; 5] = [
        "Your Grace, it is said the Mages Guild watches over Nekhar-Zul and the Eye of Vael.",
        "The roads to Nekhar-Zul are a couple of hours out, and prices are rising.",
        "I saw the mages leave for those ruins. They seek the Eye of Vael.",
        "They say there's something pulsing in the crypt outside town. It scares me.",
        "Dunno, Nekhar... Nekhal? Ten or twenty kilometers, let's drink!",
    ];

    #[test]
    fn seed_validation() {
        assert!(validate_seed(&good_seed_json()).is_ok());
    }

    #[test]
    fn rumors_validation() {
        let s = seed();
        assert!(validate_rumors(&rumors_json(OK_TEXTS), &s).is_ok());
    }

    #[test]
    fn voice_validation() {
        let s = seed();
        let ok = |t: &str| validate_voice(&Json::Obj(vec![("text".into(), jstr(t))]), &s);
        assert!(ok("They say the Mages Guild sniffs around Nekhar-Zul.").is_ok());
        assert!(ok("Stay away from the CRIPTA di cenere!").unwrap_err().contains("generic name"));
    }

    #[test]
    fn question_cleaning() {
        assert_eq!(clean_question("  Where   is\tthe\ninn?  ").as_deref(), Some("Where is the inn?"));
        assert_eq!(clean_question("   \n "), None);
    }

    #[test]
    fn answer_validation() {
        let ok = |t: &str| validate_answer(&Json::Obj(vec![("text".into(), jstr(t))]));
        assert!(ok("Welcome to Daggerfall, stranger.").is_ok());
        assert!(ok("As an AI, I cannot say.").unwrap_err().contains("breaks character"));
        let with_follow = |f: Json| validate_answer(&Json::Obj(vec![
            ("text".into(), jstr("Sure.")), ("follow".into(), f),
        ]));
        assert!(with_follow(Json::Bool(false)).is_ok());
        assert!(with_follow(Json::Bool(true)).is_ok());
        assert!(with_follow(jstr("yes")).unwrap_err().contains("'follow'"));
        assert!(parse_follow(&Json::Obj(vec![("follow".into(), Json::Bool(true))])));
        assert!(!parse_follow(&Json::Obj(vec![("follow".into(), Json::Bool(false))])));
        assert!(!parse_follow(&Json::Obj(vec![])));
    }

    #[test]
    fn mood_validation() {
        let base = Json::Obj(vec![("text".into(), jstr("Sure."))]);
        assert!(validate_answer(&base).is_ok());
        assert_eq!(parse_mood(&base), 0);

        for m in 0..=7i64 {
            let j = Json::Obj(vec![
                ("text".into(), jstr("Sure.")),
                              ("mood".into(), Json::Num(m as f64)),
            ]);
            assert!(validate_answer(&j).is_ok());
            assert_eq!(parse_mood(&j), m as u8);
        }

        let j = Json::Obj(vec![
            ("text".into(), jstr("Sure.")),
                          ("mood".into(), Json::Num(9.0)),
        ]);
        assert!(validate_answer(&j).unwrap_err().contains("out of range"));

        let j = Json::Obj(vec![
            ("text".into(), jstr("Sure.")),
                          ("mood".into(), jstr("happy")),
        ]);
        assert!(validate_answer(&j).unwrap_err().contains("'mood'"));

        let j = Json::Obj(vec![("mood".into(), Json::Num(42.0))]);
        assert_eq!(parse_mood(&j), 7);
    }

    #[test]
    fn history_block_respects_limit() {
        let mut history = Vec::new();
        for i in 0..(MAX_HISTORY_TURNS + 5) {
            history.push(("player".to_string(), format!("q{i}")));
            history.push(("npc".to_string(), format!("a{i}")));
        }
        let block = history_block(&history);
        assert!(!block.contains("q0") && !block.contains("a0"));
        let lines: Vec<&str> = block.lines().filter(|l| l.starts_with("- ")).collect();
        assert_eq!(lines.len(), MAX_HISTORY_TURNS * 2);
    }

    // --------------------------------------------------------- QuestBook

    fn sample_quest() -> Quest {
        Quest {
            city: (207, 213), city_name: "Daggerfall".into(), seed: seed(),
            rumors: OK_TEXTS.iter().zip(Tone::ALL).map(|(t, tone)| (tone, t.to_string())).collect(),
            npc_flavors: Vec::new(),
        }
    }

    #[test]
    fn hints_depend_on_state() {
        let mut book = QuestBook::default();
        assert_eq!(book.hint(Some((1, 1)), Tone::Noble), Hint::Nothing);
        book.mark_pending((1, 1));
        assert_eq!(book.hint(Some((1, 1)), Tone::Noble), Hint::Pending);
        book.clear_pending((1, 1));
        book.mark_failed((1, 1));
        assert_eq!(book.hint(Some((1, 1)), Tone::Noble), Hint::Nothing);
        book.insert_quest(sample_quest());
        assert_eq!(book.hint(Some((207, 213)), Tone::Merchant), Hint::Text(OK_TEXTS[1]));
    }

    #[test]
    fn npc_flavors_lookup() {
        let mut q = sample_quest();
        q.npc_flavors.push(NpcFlavor {
            archive: 183, record: 2, caption: "guard".into(),
                           text: "Guard's voice about the quest.".into(),
        });
        let parsed = Quest::from_json(&Json::parse(&q.to_json().compact()).unwrap()).unwrap();
        let mut book = QuestBook::default();
        book.insert_quest(parsed);
        assert_eq!(book.npc_flavor((207, 213), 183, 2), Some("Guard's voice about the quest."));
        assert_eq!(book.npc_flavor((207, 213), 999, 0), None);
    }

    #[test]
    fn voice_state_lifecycle() {
        let mut book = QuestBook::default();
        assert_eq!(book.voice_state((1, 1), "walker:1"), VoiceState::Unknown);
        book.mark_voice_pending((1, 1), "walker:1");
        assert_eq!(book.voice_state((1, 1), "walker:1"), VoiceState::Pending);
        book.store_voice((1, 1), "walker:1".into(), "Hush.".into());
        assert_eq!(book.voice_state((1, 1), "walker:1"), VoiceState::Ready("Hush."));
    }

    #[test]
    fn answers_lifecycle() {
        let mut book = QuestBook::default();
        book.store_answer(1, "Hello.".into(), false, 0);
        book.store_answer(3, "I'll come with you.".into(), true, 1);
        book.fail_answer(2);
        assert_eq!(book.take_answer(1), Some(Ok(("Hello.".to_string(), false, 0))));
        assert_eq!(book.take_answer(3), Some(Ok(("I'll come with you.".to_string(), true, 1))));
        assert_eq!(book.take_answer(1), None);
        assert_eq!(book.take_answer(2), Some(Err(())));
    }

    #[test]
    fn only_towns_get_quests() {
        let l = |names: &[&str]| DFLocation {
            name: "X".into(), region_index: 0, world_x: 0, world_y: 0,
            width: names.len(), height: 1,
            block_names: names.iter().map(|s| s.to_string()).collect(),
            letter1_for_rmb_name: None,
        };
        assert!(is_town(&l(&["MAGEAA13.RMB", "TEMPAA01.RMB"])));
        assert!(!is_town(&l(&["TAVNAA01.RMB"])));
    }

    #[test]
    fn key_functions() {
        assert_eq!(walker_key(37), "walker:37");
        assert_eq!(flat_key(175, 0), "flat:175:0");
        assert_eq!(face_key(9), "face:9");
        assert_eq!(walker_look(0), "Nord woman");
        assert_eq!(walker_look(192), "Breton man");
    }

    #[test]
    fn tone_is_stable() {
        let city = (207, 213);
        assert_eq!(tone_for_npc(1, city, None), tone_for_npc(1, city, None));
        assert_eq!(tone_for_npc(1, city, Some(PoiKind::Shop)), Tone::Merchant);
    }

    #[test]
    fn describe_npc_marks_barmaid_in_tavern() {
        let d = describe_npc("comely maiden", None, Some(PoiKind::Tavern), "cheerful");
        assert!(d.contains("barmaid"), "{d}");
        let d = describe_npc("old beggar", None, None, "weary");
        assert!(!d.contains("barmaid"));
    }

    #[test]
    fn describe_npc_makes_guild_members_insiders() {
        let d = describe_npc_in("robed woman", None, Some(PoiKind::Guild), "calm", Some("Mages Guild"));
        assert!(d.contains("member or employee of the Mages Guild"), "{d}");
        assert!(d.contains("first-hand") && !d.contains("likely"), "{d}");
        let d = describe_npc("robed woman", None, Some(PoiKind::Guild), "calm");
        assert!(d.contains("the guild whose hall this is"), "{d}");
    }

    // -------------------------------------------------- guild_name_from_prefix

    #[test]
    fn guild_prefixes_are_mapped_without_articles() {
        // I nomi ritornati non hanno articolo: `describe_npc_in` e il prompt
        // `NPC_FLAVORS` in `quest_llm.rs` aggiungono "the" quando serve.
        assert_eq!(guild_name_from_prefix("MAGE"), Some("Mages Guild"));
        assert_eq!(guild_name_from_prefix("mage"), Some("Mages Guild"));
        assert_eq!(guild_name_from_prefix("FIGH"), Some("Fighters Guild"));
        assert_eq!(guild_name_from_prefix("THIE"), Some("Thieves Guild"));
        assert_eq!(guild_name_from_prefix("DARK"), Some("Dark Brotherhood"));
        assert_eq!(guild_name_from_prefix("TEMP"), Some("Temple"));
        assert_eq!(guild_name_from_prefix("PTEM"), Some("Temple"));
        assert_eq!(guild_name_from_prefix("PALA"), Some("Royal Palace"));
        assert_eq!(guild_name_from_prefix("CAST"), Some("Royal Palace"));
        // Prefissi non-organizzazione: nessun nome
        assert_eq!(guild_name_from_prefix("TAVN"), None);
        assert_eq!(guild_name_from_prefix("GENR"), None);
        assert_eq!(guild_name_from_prefix("RESI"), None);
        assert_eq!(guild_name_from_prefix("WALL"), None);
        assert_eq!(guild_name_from_prefix(""), None);
    }

    #[test]
    fn describe_npc_in_does_not_double_the_article() {
        // Regression: con `guild_name_from_prefix("TEMP") == "the Temple"`
        // e `format!("the {g}")` in `describe_npc_in`, il testo diventava
        // "member of the the Temple". Ora il prefisso torna senza articolo.
        let d = describe_npc_in("priest", None, Some(PoiKind::Guild), "calm", guild_name_from_prefix("TEMP"));
        assert!(d.contains("member or employee of the Temple"), "{d}");
        assert!(!d.contains("the the"), "{d}");
    }

    // ------------------------------------------------------ active_quest_giver_id

    #[test]
    fn active_quest_giver_id_returns_the_seed_giver() {
        let mut book = QuestBook::default();
        assert_eq!(book.active_quest_giver_id((207, 213)), None);
        book.insert_quest(sample_quest());
        assert_eq!(book.active_quest_giver_id((207, 213)), Some("mages_guild"));
        assert_eq!(book.active_quest_giver_id((1, 1)), None);
    }

    // -------------------------------------------------- QuestScriptJob / prompt

    fn sample_script_job() -> QuestScriptJob {
        QuestScriptJob {
            city: (207, 213),
            city_name: "Daggerfall".into(),
            giver_name: "Mages Guild".into(),
            giver_id: "mages_guild".into(),
            dungeon_dev: "Crypt of Ash".into(),
            artifact_example: "Eye of Vael".into(),
        }
    }

    #[test]
    fn build_quest_script_prompt_fills_placeholders() {
        let job = sample_script_job();
        let p = build_quest_script_prompt(DEFAULT_QUEST_SCRIPT_PROMPT, &job);
        assert!(p.contains("Mages Guild"), "giver");
        assert!(p.contains("Crypt of Ash"), "dungeon_dev");
        assert!(p.contains("Eye of Vael"), "artifact_example");
        assert!(p.contains("Daggerfall"), "city");
        assert!(!p.contains("{giver}"), "placeholder non sostituito");
        assert!(!p.contains("{dungeon_dev}"));
        assert!(!p.contains("{artifact_example}"));
        assert!(!p.contains("{city}"));
    }

    #[test]
    fn quest_script_prompt_has_the_required_sections() {
        // Il prompt deve menzionare tutte le sezioni che `dfquest::QuestScript`
        // sa parsare, altrimenti l'LLM non le produrrà.
        for section in [
            "Quest: <ID>",
            "QuestorOffer:", "AcceptQuest:", "RefuseQuest:",
            "QuestComplete:", "QuestFail:",
            "RumorsDuringQuest:", "RumorsPostsuccess:", "RumorsPostfailure:",
            "QBN:",
            "start timer", "log 1020 step 0", "place foe _monster_ at _mondung_",
            "killed 1 _monster_", "clicked npc _qgfriend_",
            "give pc _artifact_", "end quest",
        ] {
            assert!(
                DEFAULT_QUEST_SCRIPT_PROMPT.contains(section),
                    "il prompt non menziona `{section}`"
            );
        }
    }
}
