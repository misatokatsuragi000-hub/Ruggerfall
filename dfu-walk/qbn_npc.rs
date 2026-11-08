//! Collegamento dei quest giver ai flat già presenti nel dungeon.
//!
//! L'approccio è:
//! 1. leggere FACTION.TXT;
//! 2. trovare i personaggi individuali (`type: 4`) con flat e faccia validi;
//! 3. confrontare il nome con i `persons` della main quest;
//! 4. agganciare il flat già spawnato dal dungeon inserendo:
//!    - InteriorNpc
//!    - CustomNpcName
//!    - QbnSymbol
//!
//! In questo modo non si spawna un PNG duplicato: si usa direttamente il
//! flat originale del RDB, che ha già la posizione corretta.
//!
//! ## PNG di quest QBN negli interni (`_agentuk_`, `_lhotun_`, ...)
//!
//! I personaggi della main quest piazzati dal QBN (`place npc _agentuk_ at
//! _agentplace_`) NON sono flat del RDB: sono entità dinamiche che il runtime
//! QBN (`qbn.rs`) registra in `Instance::placed`. Il sistema
//! `spawn_qbn_npcs_in_interior` li materializza quando il giocatore entra
//! nell'interno giusto.
//!
//! **Quando spawnare**: il runtime ha `QbnRuntime::placements()` che ritorna
//! `[(quest, person, place)]`. Il `place` è un simbolo QBN (`_agentplace_`)
//! che `Library::expand` risolve con `DefaultResolver::place`. Per
//! `_agentplace_` (target `["tavern"]`) il valore espanso è letteralmente
//! `"tavern"`: il check è "l'interno corrente è una taverna?".
//!
//! **Dove spawnare**: nel sistema locale della radice dell'interno, tra lo
//! spawn del giocatore e il centro geometrico della scena, sfalsato per
//! asse x in modo da non sovrapporre più PNG QBN piazzati nello stesso
//! interno.
//!
//! **Cosa spawnare**: preferisce un NPC custom con `qbn_symbol = <simbolo>`
//! (dà un volto dedicato da `npc_<nome>.png`); altrimenti usa un flat
//! originale dell'archivio `PERSON_ARCHIVES` scelto in modo deterministico
//! dall'hash del simbolo. In entrambi i casi il componente `QbnSymbol` fa
//! scattare il task QBN al click (`_S.14_` per `_agentuk_`).
//!
//! **Quest-giver in attesa (`NotStarted`)**: se una quest non è ancora
//! partita ma ha un `QuestorOffer` e un questor atteso (`offer_questor`),
//! il PNG viene spawnato in taverna anche senza un `place npc`. È il caso
//! di `_helseth_` (Blackmailing Prince Helseth): il QBN non piazza il
//! questor, quindi senza questo blocco non ci sarebbe nessuno a offrire la
//! quest. Il sistema `main_quest_offer` in `dialog.rs` riconosce il PNG
//! cliccato e mostra l'offerta.
//!
//! **Persistenza**: `SpawnedQbnNpcs` evita di ri-spawnare lo stesso PNG ogni
//! volta che il giocatore rientra nello stesso interno. Vive solo per la
//! sessione: dopo un load ricomincia da capo e il PNG appare nella prima
//! taverna che visiti.

use bevy::prelude::*;
use std::collections::{HashMap, HashSet};

use crate::data::building_gen::DF_UNIT;
use crate::data::city_map::PoiKind;
use crate::data::find_file_ci;
use crate::dfquest::{pretty_name, Library, PersonDef, QuestPhase, QuestStates};
use crate::dungeon::DungeonFlatSource;
use crate::interior::{
    CustomNpcCache, CustomNpcName, FlatBillboard, InteriorEntered, InteriorNpc, InteriorState,
};
use crate::main_quest::QbnRuntimeRes;
use crate::mod_textures::ModTextures;
use crate::wall_textures::WallTextures;
use crate::world::GameData;
use crate::{QuestLibRes, QuestStatesRes};

#[allow(dead_code)]
pub const HOME_TOWN: &str = "Sentinel";

/// Simbolo QBN del PNG (es. `_lhotun_`).
#[derive(Component, Clone, Debug)]
pub struct QbnSymbol(pub String);

/// Entity -> simbolo QBN. Lo legge `dialog.rs`.
#[derive(Resource, Default)]
pub struct QbnNpcIndex(pub HashMap<Entity, String>);

/// Mantenuta per compatibilità, ma non è più usata per lo spawn dinamico.
#[derive(Resource, Default)]
pub struct QbnNpcMemory {
    #[allow(dead_code)]
    spots: HashMap<(String, String), Vec3>,

    #[allow(dead_code)]
    pins: HashMap<String, String>,
}

/// PNG QBN già spawnati in questa sessione: (quest, symbol). Evita doppioni
/// quando il giocatore rientra nello stesso interno.
#[derive(Resource, Default)]
pub struct SpawnedQbnNpcs {
    pub spawned: HashSet<(String, String)>,
}

// =============================================================================
// FACTION.TXT
// =============================================================================

struct FactionPerson {
    name: String,
    face: usize,
    archive: u16,
    record: u8,
}

#[derive(Default)]
struct FactionEntry {
    ty: Option<i32>,
    name: Option<String>,
    face: Option<usize>,
    flat: Option<(u16, u8)>,
}

fn finalize_faction_entry(entry: FactionEntry, out: &mut Vec<FactionPerson>) {
    // `type: 4` = facINDIVIDUAL in FACTION.TXT.
    if entry.ty == Some(4) {
        if let (Some(name), Some(face), Some((archive, record))) =
            (entry.name, entry.face, entry.flat)
            {
                let name = name.trim().to_string();
                if !name.is_empty() {
                    out.push(FactionPerson {
                        name,
                        face,
                        archive,
                        record,
                    });
                }
            }
    }
}

fn load_faction_persons(data: &GameData) -> Vec<FactionPerson> {
    let Some(path) = find_file_ci(&data.dir, "FACTION.TXT") else {
        warn!(
            "[qbn_npc] FACTION.TXT non trovato in {}",
            data.dir.display()
        );
        return Vec::new();
    };

    let Ok(text) = std::fs::read_to_string(&path) else {
        warn!(
            "[qbn_npc] impossibile leggere {}",
            path.display()
        );
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut entry: Option<FactionEntry> = None;

    for raw in text.lines() {
        let line = raw.trim();

        if line.is_empty() || line.starts_with(';') {
            continue;
        }

        if line.starts_with('#') {
            if let Some(e) = entry.take() {
                finalize_faction_entry(e, &mut out);
            }
            entry = Some(FactionEntry::default());
            continue;
        }

        let Some(e) = entry.as_mut() else {
            continue;
        };

        let Some((key, value)) = line.split_once(':') else {
            continue;
        };

        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();

        match key.as_str() {
            "type" => {
                e.ty = value.parse::<i32>().ok();
            }
            "name" => {
                e.name = Some(value.to_string());
            }
            "face" => {
                if let Ok(f) = value.parse::<i32>() {
                    if f >= 0 {
                        e.face = Some(f as usize);
                    }
                }
            }
            "flat" => {
                let mut parts = value.split_whitespace();

                if let (Some(a), Some(r)) = (parts.next(), parts.next()) {
                    if let (Ok(archive), Ok(record)) =
                        (a.parse::<u16>(), r.parse::<u8>())
                        {
                            if e.flat.is_none() {
                                e.flat = Some((archive, record));
                            }
                        }
                }
            }
            _ => {}
        }
    }

    if let Some(e) = entry.take() {
        finalize_faction_entry(e, &mut out);
    }

    info!(
        "[qbn_npc] FACTION.TXT: {} personaggi individuali con flat valido",
        out.len()
    );

    out
}

// =============================================================================
// Match tra FACTION.TXT e i personaggi della main quest
// =============================================================================

fn quest_person_map(
    lib: &Library,
    states: &QuestStates,
) -> HashMap<String, (String, usize)> {
    let mut map = HashMap::new();

    if let Some(script) = lib.get(crate::main_quest::MAIN_QUEST_ID) {
        for p in &script.persons {
            let name = match p.name.as_deref() {
                Some(n) => pretty_name(n),
                None => lib.expand(crate::main_quest::MAIN_QUEST_ID, states, &p.symbol),
            };

            if name.starts_with('_') {
                continue;
            }

            let face = p.face.map(|f| f as usize).unwrap_or(0);

            map.insert(
                name.to_ascii_lowercase(),
                       (p.symbol.clone(), face),
            );
        }
    }

    map
}

fn explicit_quest_symbol(name: &str) -> Option<(String, usize)> {
    let n = name.to_ascii_lowercase();

    if n.contains("lhotun") {
        return Some(("_lhotun_".to_string(), 0));
    }

    if n.contains("akorithi") {
        return Some(("_akorithi_".to_string(), 0));
    }

    if n.contains("greklith") {
        return Some(("_greklith_".to_string(), 0));
    }

    if n.contains("vhosek") {
        return Some(("_vhosek_".to_string(), 0));
    }

    None
}

fn match_quest_person(
    faction_name: &str,
    quest_map: &HashMap<String, (String, usize)>,
) -> Option<(String, usize)> {
    let lower = faction_name.to_ascii_lowercase();

    // 1. Match esatto.
    if let Some(v) = quest_map.get(&lower) {
        return Some(v.clone());
    }

    // 2. Match sull'ultima parola:
    //    "Prince Lhotun" -> "lhotun"
    if let Some(last) = lower.split_whitespace().last() {
        if let Some(v) = quest_map.iter().find_map(|(k, v)| {
            if k.split_whitespace().last() == Some(last) {
                Some(v.clone())
            } else {
                None
            }
        }) {
            return Some(v);
        }
    }

    // 3. Match contenuto, solo per nomi abbastanza lunghi.
    if lower.len() >= 4 {
        if let Some(v) = quest_map.iter().find_map(|(k, v)| {
            if k.contains(lower.as_str()) || lower.contains(k.as_str()) {
                Some(v.clone())
            } else {
                None
            }
        }) {
            return Some(v);
        }
    }

    // 4. Fallback esplicito per i quest giver noti.
    explicit_quest_symbol(faction_name)
}

// =============================================================================
// Aggancio ai flat già spawnati dal dungeon
// =============================================================================

#[allow(clippy::too_many_arguments)]
pub fn attach_faction_quest_npcs(
    mut commands: Commands,
    data: Res<GameData>,
    lib: Res<QuestLibRes>,
    states: Res<QuestStatesRes>,
    flats: Query<(Entity, &DungeonFlatSource), Without<QbnSymbol>>,
                                 existing: Query<&QbnSymbol>,
                                 mut faction_cache: Local<Option<Vec<FactionPerson>>>,
) {
    if faction_cache.is_none() {
        *faction_cache = Some(load_faction_persons(&data));
    }

    let Some(persons) = faction_cache.as_ref() else {
        return;
    };

    if persons.is_empty() {
        return;
    }

    let debug = std::env::var_os("DF_QBN_FACTION_DEBUG").is_some();

    let quest_map = quest_person_map(&lib.0, &states.0);

    let mut by_flat: HashMap<(u16, u8), &FactionPerson> = HashMap::new();

    for p in persons.iter() {
        by_flat
        .entry((p.archive, p.record))
        .or_insert(p);
    }

    let mut assigned: HashSet<String> = existing
    .iter()
    .map(|s| s.0.clone())
    .collect();

    for (entity, src) in &flats {
        let Some(fp) = by_flat
        .get(&(src.archive, src.record))
        .copied()
        else {
            continue;
        };

        if debug {
            info!(
                "[qbn_npc] flat {}/{} corrisponde a '{}'",
                src.archive, src.record, fp.name
            );
        }

        let Some((symbol, _quest_face)) =
        match_quest_person(&fp.name, &quest_map)
        else {
            if debug {
                info!(
                    "[qbn_npc] '{}' non ha un simbolo QBN corrispondente",
                    fp.name
                );
            }
            continue;
        };

        if assigned.contains(&symbol) {
            continue;
        }

        commands.entity(entity).insert((
            InteriorNpc {
                face_record: fp.face,
                flat_archive: src.archive,
                flat_record: src.record,
                size: src.size,
                instance: 0,
            },
            CustomNpcName {
                name: fp.name.clone(),
            },
            QbnSymbol(symbol.clone()),
        ));

        assigned.insert(symbol.clone());

        info!(
            "[qbn_npc] agganciato quest giver '{}' al flat {}/{} (face {}, symbol {})",
              fp.name, src.archive, src.record, fp.face, symbol
        );
    }
}

// =============================================================================
// Spawn dinamico dei PNG QBN negli interni
// =============================================================================

/// Archivio per il flat di fallback quando non c'è un custom con `qbn_symbol`.
/// Scelta dell'archivio in base all'hash del simbolo: deterministica, così
/// due sessioni con lo stesso simbolo usano lo stesso sprite.
const PERSON_ARCHIVES: [u16; 24] = [
    381, 382, 383, 384,
385, 386, 391, 394,
387, 388, 389, 390,
392, 393, 451, 452,
395, 396, 397, 398,
453, 454, 455, 456,
];

/// Il valore espanso di `place` corrisponde al tipo di interno corrente?
fn place_matches_interior_kind(place: &str, kind: PoiKind) -> bool {
    let p = place.to_ascii_lowercase();
    match kind {
        PoiKind::Tavern => p.contains("tavern") || p.contains("inn"),
        PoiKind::Shop => p.contains("shop") || p.contains("store") || p.contains("market"),
        PoiKind::Guild => p.contains("guild") || p.contains("hall") || p.contains("temple"),
        PoiKind::Palace => p.contains("palace") || p.contains("castle"),
    }
}

fn default_flat_for_person(def: &PersonDef) -> (u16, u8) {
    let idx = (def.symbol
    .bytes()
    .fold(0u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64))
    % PERSON_ARCHIVES.len() as u64) as usize;
    (PERSON_ARCHIVES[idx], 0)
}

/// Materializza i PNG QBN piazzati dal runtime quando il giocatore entra
/// nell'interno giusto. Chiamato dopo `interior_enter_exit`, così la radice
/// esiste già e può fare da parent.
///
/// Oltre ai `placements()` (PNG piazzati da `place npc`), spawna i
/// **quest-giver in attesa** (`NotStarted` con `QuestorOffer`) in taverna:
/// vedi il doc del modulo.
#[allow(clippy::too_many_arguments)]
pub fn spawn_qbn_npcs_in_interior(
    mut events: EventReader<InteriorEntered>,
    interior: Res<InteriorState>,
    rt: Res<QbnRuntimeRes>,
    lib: Res<QuestLibRes>,
    states: Res<QuestStatesRes>,
    custom_npcs: Res<CustomNpcCache>,
    data: Res<GameData>,
    mut wall_textures: ResMut<WallTextures>,
    mut mods: ResMut<ModTextures>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut index: ResMut<QbnNpcIndex>,
    mut spawned: ResMut<SpawnedQbnNpcs>,
    existing: Query<&QbnSymbol>,
    mut commands: Commands,
) {
    for _ev in events.read() {
        let Some(kind) = interior.kind() else { continue };
        let Some(root) = interior.root() else { continue };
        let Some(spawn) = interior.scene_spawn() else { continue };
        let Some(center) = interior.scene_center() else { continue };
        let base_instance = interior.scene_people_count().unwrap_or(0) as u32;

        let mut slot = 0u32;
        for (quest, person, place) in rt.0.placements() {
            if states.0.phase(&quest) != QuestPhase::Active {
                continue;
            }
            if rt.0.is_muted(&quest, &person) {
                continue;
            }
            if spawned.spawned.contains(&(quest.clone(), person.clone())) {
                continue;
            }

            let resolved = lib.0.expand(&quest, &states.0, &place);
            if !place_matches_interior_kind(&resolved, kind) {
                continue;
            }

            let Some(script) = lib.0.get(&quest) else { continue };
            let Some(def) = script.person(&person) else { continue };

            let npc_name = def.name.as_deref().map(pretty_name).unwrap_or_else(|| {
                let e = lib.0.expand(&quest, &states.0, &person);
                if e.starts_with('_') {
                    person.trim_matches('_').to_string()
                } else {
                    e
                }
            });
            let face = def.face.unwrap_or(0) as usize;

            let (mat, w, h, arch, rec) = if let Some(cn) = custom_npcs
            .npcs
            .iter()
            .find(|n| n.qbn_symbol.as_deref() == Some(person.as_str()))
            {
                let Some(img) = mods.image_by_name(&cn.texture_stem, &mut images) else {
                    warn!(
                        "[qbn_npc] NPC custom '{}' per `{person}` non caricabile, salto",
                        cn.name
                    );
                    continue;
                };
                let sz = mods
                .image_size(&img)
                .or_else(|| images.get(&img).map(|i| i.size_f32()))
                .unwrap_or(Vec2::new(64.0, 128.0));
                let s = DF_UNIT * 0.074;
                let (w, h) = (sz.x * s, sz.y * s);
                let m = materials.add(StandardMaterial {
                    base_color_texture: Some(img),
                                      alpha_mode: AlphaMode::Mask(0.5),
                                      cull_mode: None,
                                      double_sided: true,
                                      perceptual_roughness: 0.9,
                                      reflectance: 0.1,
                                      ..default()
                });
                (m, w, h, 0u16, 0u8)
            } else {
                let (a, r) = default_flat_for_person(def);
                let Some((m, wp, hp)) = wall_textures.flat_material(
                    a,
                    r,
                    &data,
                    &mut mods,
                    &mut images,
                    &mut materials,
                ) else {
                    warn!(
                        "[qbn_npc] flat di fallback {a}/{r} per `{person}` non caricabile, salto"
                    );
                    continue;
                };
                (m, wp * DF_UNIT, hp * DF_UNIT, a, r)
            };

            // Tra spawn e centro geometrico (40%), sfalsato a sinistra per
            // ogni PNG QBN aggiuntivo: due agenti nello stesso interno non
            // finiscono sovrapposti.
            let pos = spawn
            + (center - spawn) * 0.4
            + Vec3::new((slot as f32) * 1.2 - 1.2, 0.0, 0.0);

            let e = commands
            .spawn((
                Mesh3d(meshes.add(Rectangle::new(w, h))),
                    MeshMaterial3d(mat),
                    Transform::from_xyz(pos.x, pos.y + h * 0.5, pos.z),
                    FlatBillboard,
                    InteriorNpc {
                        face_record: face,
                        flat_archive: arch,
                        flat_record: rec,
                        size: Vec2::new(w, h),
                    instance: base_instance + slot,
                    },
                    CustomNpcName { name: npc_name.clone() },
                    QbnSymbol(person.clone()),
            ))
            .id();
            commands.entity(root).add_child(e);
            index.0.insert(e, person.clone());
            slot += 1;
            // Log PRIMA dell'insert in `spawned.spawned`: `quest` e `person`
            // vengono mossi dall'insert e non sarebbero più leggibili qui.
            info!(
                "QBN NPC `{npc_name}` ({person}) piazzato in {kind:?} per la quest `{quest}` a {pos:?}"
            );
            spawned.spawned.insert((quest, person));
        }

        // ---------------------------------------------------------------------
        // Quest-giver in attesa (`NotStarted` con `QuestorOffer`).
        //
        // `_helseth_` non è in `placements()` finché la quest non parte: senza
        // questo blocco non c'è nessun PNG a offrire la quest. Appaiono in
        // taverna. Se il questor è già presente in scena (Lhotun da
        // FACTION.TXT), non lo spawniamo di nuovo.
        // ---------------------------------------------------------------------
        if matches!(kind, PoiKind::Tavern) {
            let mut pending: Vec<(String, String)> = Vec::new();
            for script in lib.0.scripts() {
                if states.0.phase(&script.id) != QuestPhase::NotStarted {
                    continue;
                }
                if script.message_named("QuestorOffer").is_none() {
                    continue;
                }
                let Some(questor) = crate::qbn::offer_questor(&lib.0, &script.id) else { continue };
                if spawned.spawned.contains(&(script.id.clone(), questor.clone())) {
                    continue;
                }
                if existing.iter().any(|s| s.0 == questor) {
                    continue;
                }
                pending.push((script.id.clone(), questor));
            }

            for (quest, person) in pending {
                let Some(script) = lib.0.get(&quest) else { continue };
                let Some(def) = script.person(&person) else { continue };
                let npc_name = def.name.as_deref().map(pretty_name).unwrap_or_else(|| {
                    let e = lib.0.expand(&quest, &states.0, &person);
                    if e.starts_with('_') {
                        person.trim_matches('_').to_string()
                    } else {
                        e
                    }
                });
                let face = def.face.unwrap_or(0) as usize;

                let (mat, w, h, arch, rec) = if let Some(cn) = custom_npcs
                .npcs
                .iter()
                .find(|n| n.qbn_symbol.as_deref() == Some(person.as_str()))
                {
                    let Some(img) = mods.image_by_name(&cn.texture_stem, &mut images) else {
                        warn!(
                            "[qbn_npc] NPC custom '{}' per `{person}` non caricabile, salto",
                            cn.name
                        );
                        continue;
                    };
                    let sz = mods
                    .image_size(&img)
                    .or_else(|| images.get(&img).map(|i| i.size_f32()))
                    .unwrap_or(Vec2::new(64.0, 128.0));
                    let s = DF_UNIT * 0.074;
                    let (w, h) = (sz.x * s, sz.y * s);
                    let m = materials.add(StandardMaterial {
                        base_color_texture: Some(img),
                                          alpha_mode: AlphaMode::Mask(0.5),
                                          cull_mode: None,
                                          double_sided: true,
                                          perceptual_roughness: 0.9,
                                          reflectance: 0.1,
                                          ..default()
                    });
                    (m, w, h, 0u16, 0u8)
                } else {
                    let (a, r) = default_flat_for_person(def);
                    let Some((m, wp, hp)) = wall_textures.flat_material(
                        a,
                        r,
                        &data,
                        &mut mods,
                        &mut images,
                        &mut materials,
                    ) else {
                        warn!(
                            "[qbn_npc] flat di fallback {a}/{r} per `{person}` non caricabile, salto"
                        );
                        continue;
                    };
                    (m, wp * DF_UNIT, hp * DF_UNIT, a, r)
                };

                let pos = spawn + (center - spawn) * 0.4 + Vec3::new(1.2, 0.0, 0.0);
                let e = commands
                .spawn((
                    Mesh3d(meshes.add(Rectangle::new(w, h))),
                        MeshMaterial3d(mat),
                        Transform::from_xyz(pos.x, pos.y + h * 0.5, pos.z),
                        FlatBillboard,
                        InteriorNpc {
                            face_record: face,
                            flat_archive: arch,
                            flat_record: rec,
                            size: Vec2::new(w, h),
                        instance: base_instance + slot,
                        },
                        CustomNpcName { name: npc_name.clone() },
                        QbnSymbol(person.clone()),
                ))
                .id();
                commands.entity(root).add_child(e);
                index.0.insert(e, person.clone());
                slot += 1;
                info!(
                    "QBN quest-giver `{npc_name}` ({person}) in attesa di offerta per `{quest}`"
                );
                spawned.spawned.insert((quest, person));
            }
        }
    }
}

// =============================================================================
// Plugin
// =============================================================================

pub struct QbnNpcPlugin;

impl Plugin for QbnNpcPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<QbnNpcIndex>()
        .init_resource::<QbnNpcMemory>()
        .init_resource::<SpawnedQbnNpcs>()
        .add_systems(
            Update,
            (
                attach_faction_quest_npcs,
             // `spawn_qbn_npcs_in_interior` consuma `InteriorEntered`,
             // che `interior_enter_exit` emette **dopo** aver creato la
             // radice dell'interno. Il `.after(...)` è indispensabile:
             // senza, il sistema potrebbe girare prima di `interior_enter_exit`
             // e trovare `interior.root() == None`.
             spawn_qbn_npcs_in_interior.after(crate::interior::interior_enter_exit),
            ),
        );
    }
}
