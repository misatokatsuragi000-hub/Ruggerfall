//! Loot pile alla morte dei nemici, sul modello di `DaggerfallLoot` di Daggerfall Unity.
//!
//! ## Flusso
//!
//! 1. `dungeon_enemies::apply_enemy_damage` despawna il nemico ed emette
//!    `dungeon_enemies::EnemyDiedEvent { pos, species, level, sprite_archive,
//!    sprite_record }`.
//! 2. `spawn_loot_on_enemy_death` consuma **quello stesso evento** (importato
//!    da `crate::dungeon_enemies::EnemyDiedEvent`) e spawna **un solo**
//!    billboard con il **cadavere originale di Daggerfall** (`CorpseTexture` di
//!    DFU, ricavato dall'archivio sprite del mostro con `corpse_flat_for_sprite`;
//!    se manca ripiega sullo sprite del mostro), appoggiato sul pavimento del
//!    dungeon, più `LootPile { items, gold, opened }` e `LootInfo` per la
//!    finestra.
//! 3. `interact_loot` (V vicino) apre la finestra egui del bottino (come la
//!    finestra Loot di DFU) e libera il cursore.
//! 4. `update_loot_window` disegna la finestra: oro e oggetti hanno ciascuno
//!    la propria icona (mod HD → TEXTURE.xxx vanilla → icona procedurale) e
//!    un popup al hover con nome e statistiche. Click sul singolo oggetto (o
//!    "Prendi tutto") lo sposta in `PlayerInventory`. Quando il pile è vuoto
//!    la finestra si chiude e il cadavere resta a terra.
//! 5. `despawn_expired_loot` rimuove i pile dopo `LOOT_LIFETIME_SECS`.
//!
//! ## Niente "scatter" a terra
//!
//! In DFU il cadavere è **un solo** flat con dentro un inventario. Non ci
//! sono piccoli flat di oro/arma/armatura sparsi attorno al corpo. La
//! versione precedente di questo modulo li aggiungeva leggendoli
//! dall'archivio `TEXTURE.476` (record 0/1/2/3), ma quell'archivio **non è**
//! un archivio di loot: nei file reali di Daggerfall contiene altre
//! immagini (a quanto pare figure umanoidi/maghi), e il risultato era che
//! per ogni ratto morto apparivano "un ratto e un mago". Rimuovendo gli
//! scatter il cadavere è solo quello del mostro, e il contenuto reale è
//! nella finestra di loot.
//!
//! ## Nota sull'evento `EnemyDiedEvent`
//!
//! `loot.rs` **non** dichiara il proprio evento: importa
//! `crate::dungeon_enemies::EnemyDiedEvent`. Se dichiarasse un tipo locale con
//! lo stesso nome, Bevy avrebbe due eventi distinti e il consumatore non
//! riceverebbe mai nulla.
//!
//! ## Icone: catalogo artefatti non usato
//!
//! `loot.rs` non droppa artefatti: `random_weapon`/`random_armor` creano
//! `InventoryItem` con `magic_item_param: None` e
//! `paperdoll_archive_override: None`, quindi la catena `icon_location`
//! (locale, senza template né catalogo) non li tocca. Se in futuro vorrai
//! droppare artefatti rari, aggiungi qui la logica: la catena completa è in
//! `equipment::icon_location_full`.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::window::{CursorGrabMode, PrimaryWindow};
use bevy_egui::{egui, EguiContexts};

use crate::data::{archive_file_name, find_file_ci, TextureFile};
use crate::dialog::DialogState;
// IMPORTANTE: stesso tipo emesso da `dungeon_enemies::apply_enemy_damage`.
// Se qui dichiarassimo un `EnemyDiedEvent` locale, sarebbe un tipo diverso
// e il consumatore non riceverebbe mai l'evento.
use crate::dungeon_enemies::{species_at, EnemyDiedEvent};
use crate::equipment::{
    EquipSlot, EquipmentState, InventoryItem, PlayerAppearance, PlayerInventory,
};
use crate::interior::FlatBillboard;
use crate::mod_textures::ModTextures;
use crate::player::Player;
use crate::wall_textures::WallTextures;
use crate::world::GameData;

/// Dimensione in metri del billboard di loot.
const LOOT_FLAT_SIZE: f32 = 1.6;
/// Metri per pixel dello sprite del cadavere (64 px = `LOOT_FLAT_SIZE`).
const LOOT_PIXEL_SCALE: f32 = LOOT_FLAT_SIZE / 64.0 * (2.0 / 3.0);
/// Lato massimo del billboard del cadavere, in metri.
const LOOT_MAX_SIZE: f32 = 2.5 * (2.0 / 3.0);
/// Distanza massima per aprire il contenitore con V.
const LOOT_INTERACT_RANGE: f32 = 2.5;
/// Quanto resta a terra un loot pile. `0` = per sempre.
const LOOT_LIFETIME_SECS: f32 = 600.0;

/// Archivi icone (come in equipment.rs).
const WEAPON_ARCHIVE: u32 = 234;
const WEAPON_ARCHIVE_FALLBACK: u32 = 233;

/// Componente del loot pile.
#[derive(Component)]
pub struct LootPile {
    pub items: Vec<InventoryItem>,
    pub gold: u32,
    pub opened: bool,
    pub age: f32,
}

/// Metadati per la finestra egui (titolo e ritratto del cadavere).
#[derive(Component)]
pub struct LootInfo {
    pub species: usize,
    pub sprite_archive: u16,
    pub sprite_record: u8,
}

/// Animazione verticale del billboard.
#[derive(Component)]
pub struct LootBobbing {
    pub base_y: f32,
    pub phase: f32,
}

/// Stato della finestra egui del bottino.
#[derive(Resource, Default)]
pub struct LootUiState {
    pub open: bool,
    pub pile: Option<Entity>,
    pub was_locked: bool,
}

/// Cache delle texture caricate dentro egui (icona = (archivio, record)).
#[derive(Resource, Default)]
struct EguiLootTextures {
    icons: HashMap<(u32, usize), egui::TextureId>,
    portraits: HashMap<(u16, u8), egui::TextureId>,
    fallbacks: HashMap<EquipSlot, egui::TextureId>,
    gold: Option<egui::TextureId>,
}

// ---------------------------------------------------------------------------
// Generazione loot (ispirata a LootChanceMatrix di DFU)
// ---------------------------------------------------------------------------

/// xorshift deterministico per non dipendere da `rand`.
fn next_rand(state: &mut u64) -> f32 {
    if *state == 0 {
        *state = 0x9E37_79B9_7F4A_7C15;
    }
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    ((*state >> 11) & 0xFF_FFFF) as f32 / 16_777_215.0
}

fn seed_for(pos: [f32; 3], species: usize) -> u64 {
    let px = pos[0].to_bits() as u64;
    let pz = pos[2].to_bits() as u64;
    px.wrapping_mul(0x9E37_79B9_7F4A_7C15)
    ^ pz.wrapping_mul(0x85EB_CA6B)
    ^ (species as u64).wrapping_mul(0xC2B2_AE35)
    ^ 0xA5A5_5A5A
}

fn generate_loot(species: usize, level: u8, pos: [f32; 3]) -> (u32, Vec<InventoryItem>) {
    let mut rng = seed_for(pos, species);
    // `species` è l'indice in `SPECIES`; gli umanoidi DFU hanno id >= 128.
    let is_humanoid = species_at(species).map_or(false, |s| s.id >= 128);

    let max_gold = if is_humanoid {
        (level as u32).max(1) * 8
    } else {
        (level as u32).max(1) * 12
    };
    let gold = 1 + (next_rand(&mut rng) * max_gold as f32) as u32;

    let mut items: Vec<InventoryItem> = Vec::new();

    let drop_weapon = if is_humanoid {
        next_rand(&mut rng) < 0.9
    } else {
        next_rand(&mut rng) < 0.10
    };
    let drop_armor = if is_humanoid {
        next_rand(&mut rng) < 0.9
    } else {
        next_rand(&mut rng) < 0.08
    };

    if drop_weapon {
        items.push(random_weapon(&mut rng, level));
    }
    if drop_armor {
        items.push(random_armor(&mut rng, level));
    }

    (gold, items)
}

fn random_weapon(rng: &mut u64, level: u8) -> InventoryItem {
    const WEAPONS: [(&str, EquipSlot, f32, f32); 4] = [
        ("Iron Longsword", EquipSlot::RightHand, 2.5, 6.0),
        ("Steel Battle Axe", EquipSlot::RightHand, 6.0, 10.0),
        ("Elven Saber", EquipSlot::RightHand, 2.0, 14.0),
        ("Dwarven Mace", EquipSlot::RightHand, 4.0, 16.0),
    ];
    let idx = ((next_rand(rng) * WEAPONS.len() as f32) as usize).min(WEAPONS.len() - 1);
    let (name, slot, weight, base_dmg) = WEAPONS[idx];
    InventoryItem {
        id: (next_rand(rng) * u32::MAX as f32) as u32,
        name: name.to_string(),
        slot,
        equipped: false,
        weight_kg: weight,
        armor_rating: (base_dmg * (1.0 + level as f32 * 0.05)) as i32,
        paperdoll_record: None,
        icon_source: None,
        template_id: None,
        magic_item_param: None,
        paperdoll_archive_override: None,
        description: "Looted from a fallen foe.".into(),
    }
}

fn random_armor(rng: &mut u64, level: u8) -> InventoryItem {
    const ARMORS: [(&str, EquipSlot, f32, f32); 4] = [
        ("Iron Cuirass", EquipSlot::Chest, 8.5, 10.0),
        ("Steel Greaves", EquipSlot::Legs, 6.0, 8.0),
        ("Leather Boots", EquipSlot::Feet, 1.5, 4.0),
        ("Chainmail Hauberk", EquipSlot::Chest, 5.5, 12.0),
    ];
    let idx = ((next_rand(rng) * ARMORS.len() as f32) as usize).min(ARMORS.len() - 1);
    let (name, slot, weight, base_ar) = ARMORS[idx];
    InventoryItem {
        id: (next_rand(rng) * u32::MAX as f32) as u32,
        name: name.to_string(),
        slot,
        equipped: false,
        weight_kg: weight,
        armor_rating: (base_ar * (1.0 + level as f32 * 0.04)) as i32,
        paperdoll_record: None,
        icon_source: None,
        template_id: None,
        magic_item_param: None,
        paperdoll_archive_override: None,
        description: "Looted from a fallen foe.".into(),
    }
}

/// Nome della specie. `species` è l'indice in `dungeon_enemies::SPECIES`, che NON
/// coincide con l'id del mostro (l'id 11 manca, quindi dopo il 10 gli indici
/// scalano di uno): si passa dall'id (ordine `MobileTypes` di DFU).
fn species_name(species: usize) -> &'static str {
    let Some(id) = species_at(species).map(|s| s.id) else {
        return "Creatura";
    };
    match id {
        0 => "Ratto",
        1 => "Imp",
        2 => "Spriggan",
        3 => "Pipistrello Gigante",
        4 => "Orso Grizzly",
        5 => "Tigre Dentisciabola",
        6 => "Ragno",
        7 => "Orco",
        8 => "Centauro",
        9 => "Mannaro",
        10 => "Ninfa",
        11 => "Pesce Macellaio",
        12 => "Sergente Orco",
        13 => "Arpia",
        14 => "Cinghiale Mannaro",
        15 => "Guerriero Scheletrico",
        16 => "Gigante",
        17 => "Zombie",
        18 => "Fantasma",
        19 => "Mummia",
        20 => "Scorpione Gigante",
        21 => "Sciamano Orco",
        22 => "Gargoyle",
        23 => "Wraith",
        24 => "Signore degli Orchi",
        25 => "Daedra del Gelo",
        26 => "Daedra del Fuoco",
        27 => "Daedroth",
        28 => "Vampiro",
        29 => "Daedra Seduttrice",
        30 => "Vampiro Antico",
        31 => "Signore Daedra",
        32 => "Lich",
        33 => "Lich Antico",
        34 | 40 => "Cucciolo di Drago",
        35 => "Atronach del Fuoco",
        36 => "Atronach di Ferro",
        37 => "Atronach di Carne",
        38 => "Atronach del Gelo",
        41 => "Dreugh",
        42 => "Lamia",
        128..=146 => "Umanoide",
        _ => "Creatura",
    }
}

// ---------------------------------------------------------------------------
// Plugin
// ---------------------------------------------------------------------------

pub struct LootPlugin;

impl Plugin for LootPlugin {
    fn build(&self, app: &mut App) {
        // `EnemyDiedEvent` è registrato da `DungeonEnemyPlugin`
        // (`.add_event::<EnemyDiedEvent>()` lì). `add_event` in Bevy è
        // idempotente: se lo registriamo anche qui non succede nulla, ma
        // tenerlo esplicito rende chiaro che questo plugin dipende da
        // quell'evento. Se `LootPlugin` viene aggiunto **prima** di
        // `DungeonEnemyPlugin`, questa riga garantisce che l'evento esista
        // comunque nel momento in cui `spawn_loot_on_enemy_death` gira.
        app.add_event::<EnemyDiedEvent>()
        .init_resource::<LootUiState>()
        .init_resource::<EguiLootTextures>()
        .add_systems(
            Update,
            (
                spawn_loot_on_enemy_death,
             animate_loot_bobbing,
             interact_loot,
             update_loot_window,
             despawn_expired_loot,
            )
            .chain(),
        );
    }
}

// ---------------------------------------------------------------------------
// Spawn: cadavere con l'immagine del mostro
// ---------------------------------------------------------------------------

/// Cadavere originale di Daggerfall (archivio flat, record) dato l'archivio sprite
/// del mostro (maschile o femminile). Dati di `EnemyBasics.cs` di DFU: campo
/// `CorpseTexture`, indicizzato per `MaleTexture`/`FemaleTexture`.
fn corpse_flat_for_sprite(sprite_archive: u16) -> Option<(u16, u8)> {
    Some(match sprite_archive {
        289 | 295 => (96, 0),
         277 => (96, 1),
         262 | 267 | 276 | 279 => (96, 2),
         283 | 285 => (96, 3),
         264 | 269 => (96, 5),
         296 => (305, 0),
         266 => (305, 1),
         297 => (305, 2),
         273 | 278 => (306, 0),
         270 => (306, 1),
         287 => (306, 2),
         288 => (306, 3),
         272 => (306, 4),
         274 => (306, 5),
         399 | 475 | 476 | 477 | 478 | 479 | 480 | 481 | 482 | 483 | 484 | 485 | 486 | 487 | 488 | 489 | 490 => (380, 1),
         282 => (400, 1),
         281 => (400, 2),
         280 => (400, 3),
         286 => (400, 4),
         284 => (400, 6),
         258 => (401, 0),
         255 => (401, 1),
         259 => (401, 2),
         260 => (401, 3),
         261 => (401, 4),
         275 => (401, 5),
         292 => (405, 0),
         291 => (405, 1),
         290 => (405, 2),
         293 => (405, 3),
         263 => (406, 0),
         271 => (406, 1),
         265 => (406, 2),
         257 => (406, 3),
         268 => (406, 4),
         256 => (406, 5),
         _ => return None,
    })
}

fn corpse_material(
    wall_textures: &mut WallTextures,
    data: &GameData,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    archive: u16,
    record: u8,
) -> Option<Handle<StandardMaterial>> {
    wall_textures
    .flat_material(archive, record, data, mods, images, materials)
    .map(|(m, _, _)| m)
}

/// Dimensioni in pixel dell'immagine **originale** di Daggerfall (TEXTURE.xxx).
/// Non si leggono dalla texture del materiale: con una mod HD sarebbe 4-8 volte
/// più grande e il cadavere uscirebbe gigante.
fn original_flat_size(data: &GameData, archive: u16, record: u8) -> Option<(f32, f32)> {
    let path = find_file_ci(&data.dir, &archive_file_name(archive as u32))?;
    let tex = TextureFile::open(path).ok()?;
    let img = tex.frame(record as usize, 0).ok()?;
    if img.width == 0 || img.height == 0 {
        return None;
    }
    Some((img.width as f32, img.height as f32))
}

#[allow(clippy::too_many_arguments)]
fn spawn_loot_on_enemy_death(
    mut commands: Commands,
    mut events: EventReader<EnemyDiedEvent>,
    mut mods: ResMut<ModTextures>,
    mut wall_textures: ResMut<WallTextures>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    data: Res<GameData>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut quad: Local<Option<Handle<Mesh>>>,
    mut first_event: Local<bool>,
) {
    let quad_handle = quad
    .get_or_insert_with(|| meshes.add(Rectangle::new(1.0, 1.0)))
    .clone();

    for ev in events.read() {
        // Log del primo evento ricevuto: se non compare nel log, il
        // problema è a monte (l'evento non arriva).
        if !*first_event {
            *first_event = true;
            info!(
                "Loot: primo EnemyDiedEvent ricevuto (specie {} a {:?})",
                  ev.species, ev.pos
            );
        }

        // `level` è già nell'evento (`dungeon_enemies::apply_enemy_damage`
        // lo calcola con `species_level`): non serve riaccedere a `SPECIES`.
        let level = ev.level;
        let (gold, items) = generate_loot(ev.species, level, ev.pos);

        // Il cadavere è il flat originale di Daggerfall (`CorpseTexture` di DFU,
        // dedotto dall'archivio sprite del mostro). Se la specie non è nella
        // tabella o il flat non si carica, ripiega sullo sprite del mostro.
        let enemy_archive = ev.sprite_archive;
        let enemy_record = ev.sprite_record;
        // (archivio, record) del flat realmente usato per il quad.
        let mut used = (enemy_archive, enemy_record);
        let mut material = None;
        if let Some((ca, cr)) = corpse_flat_for_sprite(enemy_archive) {
            material = corpse_material(
                &mut wall_textures, &data, &mut mods, &mut images, &mut materials, ca, cr,
            );
            if material.is_some() {
                used = (ca, cr);
            }
        }
        let material = material.unwrap_or_else(|| {
            corpse_material(
                &mut wall_textures, &data, &mut mods, &mut images, &mut materials,
                enemy_archive, enemy_record,
            )
            .unwrap_or_else(|| {
                // Fallback grigio: meglio un quad visibile che niente.
                materials.add(StandardMaterial {
                    base_color: Color::srgb(0.55, 0.45, 0.35),
                              alpha_mode: AlphaMode::Mask(0.5),
                              cull_mode: None,
                              double_sided: true,
                              ..default()
                })
            })
        });

        // Proporzioni e misura dalla texture ORIGINALE (non HD). I cadaveri sono
        // larghi e bassi. Limite di sicurezza per evitare quad enormi.
        let (w, h) = match original_flat_size(&data, used.0, used.1) {
            Some((pw, ph)) => (
                (pw * LOOT_PIXEL_SCALE).clamp(0.2, LOOT_MAX_SIZE),
                               (ph * LOOT_PIXEL_SCALE).clamp(0.2, LOOT_MAX_SIZE),
            ),
            None => (LOOT_FLAT_SIZE * (2.0 / 3.0), LOOT_FLAT_SIZE * (2.0 / 3.0) * 0.5),
        };

        // Quota del pavimento: `ev.ground_y` è la Y mondiale dei piedi del mostro
        // (`apply_enemy_damage` sottrae metà sprite e hover a `pos.y`). Il bordo
        // inferiore del cadavere poggia lì: prima si sommava altra altezza a
        // `ev.pos`, che è già il centro, e il cadavere finiva a mezz'aria.
        let floor_y = ev.ground_y;
        let center_y = floor_y + h * 0.5;

        commands.spawn((
            Name::new("Loot pile"),
                        Mesh3d(quad_handle.clone()),
                        MeshMaterial3d(material),
                        Transform::from_xyz(ev.pos[0], center_y, ev.pos[2])
                        .with_scale(Vec3::new(w, h, 1.0)),
                        FlatBillboard,
                        LootPile { items, gold, opened: false, age: 0.0 },
                        // Il ritratto nella finestra resta quello del mostro.
                        LootInfo {
                            species: ev.species,
                            sprite_archive: enemy_archive,
                            sprite_record: enemy_record,
                        },
        ));
    }
}

fn animate_loot_bobbing(
    time: Res<Time>,
    mut piles: Query<(&mut Transform, &LootBobbing), With<LootPile>>,
) {
    let t = time.elapsed_secs();
    for (mut tf, bob) in &mut piles {
        tf.translation.y = bob.base_y + (t * 1.7 + bob.phase).sin() * 0.05;
    }
}

// ---------------------------------------------------------------------------
// Interazione: V apre la finestra del bottino (come il Loot di DFU)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn interact_loot(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    player: Query<&Transform, With<Player>>,
    piles: Query<(Entity, &Transform, &LootPile), Without<Player>>,
                 mut ui_state: ResMut<LootUiState>,
                 equipment: Res<EquipmentState>,
                 dialog: Res<DialogState>,
                 mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    // Non aprire sopra altri menu.
    if ui_state.open || equipment.open || dialog.open {
        return;
    }
    if !keys.just_pressed(KeyCode::KeyV) {
        return;
    }
    let Ok(ptf) = player.get_single() else { return };
    let ppos = ptf.translation;

    let mut best: Option<(f32, Entity)> = None;
    for (e, tf, pile) in piles.iter() {
        if pile.items.is_empty() && pile.gold == 0 {
            continue;
        }
        let d = tf.translation.distance(ppos);
        if d <= LOOT_INTERACT_RANGE && best.map_or(true, |(bd, _)| d < bd) {
            best = Some((d, e));
        }
    }
    let Some((_, e)) = best else { return };

    ui_state.open = true;
    ui_state.pile = Some(e);
    if let Ok(mut window) = windows.get_single_mut() {
        ui_state.was_locked = window.cursor_options.grab_mode == CursorGrabMode::Locked;
        window.cursor_options.grab_mode = CursorGrabMode::None;
        window.cursor_options.visible = true;
    }
    // Consuma il tasto, altrimenti la finestra si chiuderebbe nello stesso frame.
    keys.reset(KeyCode::KeyV);
}

fn close_loot_window(
    ui_state: &mut LootUiState,
    windows: &mut Query<&mut Window, With<PrimaryWindow>>,
    restore_cursor: bool,
) {
    ui_state.open = false;
    ui_state.pile = None;
    if restore_cursor && ui_state.was_locked {
        if let Ok(mut window) = windows.get_single_mut() {
            window.cursor_options.grab_mode = CursorGrabMode::Locked;
            window.cursor_options.visible = false;
        }
    }
}

// ---------------------------------------------------------------------------
// Texture icone dentro egui (mod HD → vanilla → procedurale)
// ---------------------------------------------------------------------------

fn upload_rgba(
    ctx: &mut egui::Context,
    name: String,
    w: usize,
    h: usize,
    mut rgba: Vec<u8>,
    flip_y: bool,
) -> egui::TextureId {
    if flip_y && h > 1 {
        let stride = w * 4;
        for y in 0..h / 2 {
            for x in 0..stride {
                rgba.swap(y * stride + x, (h - 1 - y) * stride + x);
            }
        }
    }
    // `ctx.load_texture` in bevy_egui 0.33 (egui 0.29) restituisce un
    // `TextureHandle`, non un `TextureId`. Serve `.id()` per ottenerlo.
    ctx.load_texture(
        name,
        egui::ColorImage::from_rgba_unmultiplied([w.max(1), h.max(1)], &rgba),
                     egui::TextureOptions::NEAREST,
    )
    .id()
}

/// Stessa logica di `equipment::material_from_name` (lì è privata).
fn material_from_name(name: &str) -> Option<String> {
    const MATERIALS: [&str; 10] = [
        "iron", "steel", "silver", "elven", "dwarven",
        "mithril", "adamantium", "ebony", "orcish", "daedric",
    ];
    let first = name.split_whitespace().next()?.to_ascii_lowercase();
    MATERIALS.contains(&first.as_str()).then_some(first)
}

fn armor_archive(look: PlayerAppearance, slot: EquipSlot) -> u32 {
    if slot == EquipSlot::RightHand {
        return WEAPON_ARCHIVE;
    }
    let elf = matches!(look.race % 8, 3 | 4 | 5);
    match (look.female, elf) {
        (true, true) => 246,
        (true, false) => 247,
        (false, true) => 250,
        (false, false) => 251,
    }
}

/// Stessa logica di `equipment::icon_location` (lì è privata).
///
/// `loot.rs` non droppa artefatti (`magic_item_param` è sempre `None` sui
/// `InventoryItem` generati), quindi questa catena non deve gestire il
/// catalogo artefatti. Se in futuro aggiungerai artefatti rari ai drop,
/// importa `crate::equipment::icon_location_full` e passa il catalogo.
fn icon_location(look: PlayerAppearance, item: &InventoryItem) -> Option<(u32, usize)> {
    if let Some(source) = item.icon_source {
        return Some(source);
    }
    let record = item.paperdoll_record?;
    Some(match item.slot {
        EquipSlot::RightHand => (WEAPON_ARCHIVE, record.saturating_sub(1)),
         slot => (armor_archive(look, slot), record),
    })
}

/// RGBA di un record (archivio, record): prima mod HD, poi vanilla.
fn load_record_rgba(
    data: &GameData,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
    archive: u32,
    record: usize,
    name: &str,
) -> Option<(usize, usize, Vec<u8>, bool)> {
    let material = material_from_name(name);
    let mut archives = vec![archive];
    if archive == WEAPON_ARCHIVE {
        archives.push(WEAPON_ARCHIVE_FALLBACK);
    }

    // 1. Versione HD nelle mod.
    for &a in &archives {
        if let Some(handle) = mods.item_image(a, record as u32, 0, material.as_deref(), images) {
            let flip = mods.needs_v_flip(&handle);
            if let Some(img) = images.get(&handle) {
                // Le texture compresse non sono leggibili come RGBA: salta.
                if img.texture_descriptor.format == TextureFormat::Rgba8UnormSrgb
                    && !img.data.is_empty()
                    {
                        return Some((
                            img.width() as usize,
                                     img.height() as usize,
                                     img.data.clone(),
                                     flip,
                        ));
                    }
            }
        }
    }

    // 2. Originale di Daggerfall.
    for &a in &archives {
        let Some(path) = find_file_ci(&data.dir, &archive_file_name(a)) else { continue };
        let Ok(tex) = TextureFile::open(path) else { continue };
        let Ok(img) = tex.frame(record, 0) else { continue };
        if img.width == 0 || img.height == 0 {
            continue;
        }
        let rgba = img.to_rgba(&data.palette, Some(0));
        return Some((img.width, img.height, rgba, false));
    }
    None
}

fn item_texture_id(
    ctx: &mut egui::Context,
    cache: &mut EguiLootTextures,
    data: &GameData,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
    look: PlayerAppearance,
    item: &InventoryItem,
) -> egui::TextureId {
    if let Some((archive, record)) = icon_location(look, item) {
        if let Some(&id) = cache.icons.get(&(archive, record)) {
            return id;
        }
        if let Some((w, h, rgba, flip)) =
            load_record_rgba(data, mods, images, archive, record, &item.name)
            {
                let id = upload_rgba(ctx, format!("loot_icon_{archive}_{record}"), w, h, rgba, flip);
                cache.icons.insert((archive, record), id);
                return id;
            }
    }
    fallback_slot_texture(ctx, cache, item.slot)
}

fn portrait_texture(
    ctx: &mut egui::Context,
    cache: &mut EguiLootTextures,
    data: &GameData,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
    archive: u16,
    record: u8,
) -> Option<egui::TextureId> {
    if archive == 0 {
        return None;
    }
    if let Some(&id) = cache.portraits.get(&(archive, record)) {
        return Some(id);
    }
    let (w, h, rgba, flip) =
    load_record_rgba(data, mods, images, archive as u32, record as usize, "")?;
    let id = upload_rgba(
        ctx,
        format!("loot_portrait_{archive}_{record}"),
            w,
            h,
            rgba,
            flip,
    );
    cache.portraits.insert((archive, record), id);
    Some(id)
}

fn fallback_slot_texture(
    ctx: &mut egui::Context,
    cache: &mut EguiLootTextures,
    slot: EquipSlot,
) -> egui::TextureId {
    if let Some(&id) = cache.fallbacks.get(&slot) {
        return id;
    }
    let rgba = fallback_icon_rgba(slot);
    let id = upload_rgba(ctx, format!("loot_fallback_{slot:?}"), 32, 32, rgba, false);
    cache.fallbacks.insert(slot, id);
    id
}

fn gold_texture(ctx: &mut egui::Context, cache: &mut EguiLootTextures) -> egui::TextureId {
    if let Some(id) = cache.gold {
        return id;
    }
    const W: usize = 32;
    const H: usize = 32;
    let mut rgba = vec![0u8; W * H * 4];
    for y in 0..H {
        for x in 0..W {
            let dx = x as f32 - 15.5;
            let dy = y as f32 - 15.5;
            let d = (dx * dx + dy * dy).sqrt();
            if d < 10.0 {
                let c = if d < 6.5 {
                    [255, 226, 110, 255]
                } else {
                    [226, 178, 44, 255]
                };
                let o = (y * W + x) * 4;
                rgba[o..o + 4].copy_from_slice(&c);
            }
        }
    }
    let id = upload_rgba(ctx, "loot_gold".into(), W, H, rgba, false);
    cache.gold = Some(id);
    id
}

/// Icona procedurale 32×32 (versione compatta di `generate_fallback_icon`).
fn fallback_icon_rgba(slot: EquipSlot) -> Vec<u8> {
    const W: usize = 32;
    const H: usize = 32;
    let mut rgba = vec![0u8; W * H * 4];
    let mut set = |x: usize, y: usize, c: [u8; 4]| {
        if x < W && y < H {
            let o = (y * W + x) * 4;
            rgba[o..o + 4].copy_from_slice(&c);
        }
    };
    let metal = [190, 195, 205, 255];
    let dark = [120, 125, 135, 255];
    let gold = [230, 180, 50, 255];
    let wood = [140, 85, 40, 255];
    let red = [220, 45, 45, 240];

    match slot {
        EquipSlot::Chest => {
            for y in 6..26 {
                for x in 8..24 {
                    if (y < 10 && (x < 12 || x > 19)) || (y > 22 && (x < 11 || x > 20)) {
                        continue;
                    }
                    set(x, y, metal);
                }
            }
        }
        EquipSlot::Legs => {
            for y in 6..26 {
                for x in 9..14 { set(x, y, metal); }
                for x in 18..23 { set(x, y, metal); }
            }
        }
        EquipSlot::Feet => {
            for y in 10..24 {
                for x in 8..14 { set(x, y, metal); }
                for x in 18..24 { set(x, y, metal); }
            }
            for x in 6..14 { set(x, 24, dark); }
            for x in 18..26 { set(x, 24, dark); }
        }
        EquipSlot::Head => {
            for y in 6..24 {
                for x in 8..24 {
                    if (y < 9 && (x < 11 || x > 20))
                        || (y > 14 && y < 17 && x > 11 && x < 21)
                        {
                            continue;
                        }
                        set(x, y, metal);
                }
            }
        }
        EquipSlot::Hands => {
            for y in 10..24 {
                for x in 8..14 { set(x, y, dark); }
                for x in 18..24 { set(x, y, dark); }
            }
        }
        EquipSlot::Shoulders => {
            for y in 8..18 {
                for x in 4..13 { set(x, y, metal); }
                for x in 19..28 { set(x, y, metal); }
            }
        }
        EquipSlot::RightHand => {
            for i in 6..22 { set(i, i, metal); set(i + 1, i, metal); }
            for i in 20..26 { set(i + 3, i - 1, gold); set(i - 1, i + 3, gold); }
            for i in 22..27 { set(i, i, wood); }
        }
        EquipSlot::LeftHand => {
            for y in 6..26 {
                for x in 6..26 {
                    let d = ((x as f32 - 16.0).powi(2) + (y as f32 - 16.0).powi(2)).sqrt();
                    if d < 10.0 { set(x, y, metal); }
                    if (d - 7.0).abs() < 1.0 { set(x, y, wood); }
                    if d < 3.0 { set(x, y, gold); }
                }
            }
        }
        _ => {
            // Pozioni / amuleti / anelli / mantelli: fiala rossa.
            for y in 12..25 {
                for x in 10..22 {
                    let d = ((x as f32 - 16.0).powi(2) + (y as f32 - 18.0).powi(2)).sqrt();
                    if d < 6.0 { set(x, y, red); }
                }
            }
            for y in 8..13 {
                for x in 14..18 { set(x, y, [240, 240, 250, 200]); }
            }
            for x in 13..19 { set(x, 7, wood); }
        }
    }
    rgba
}

// ---------------------------------------------------------------------------
// Finestra egui del bottino (fedele alla finestra Loot di DFU)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn update_loot_window(
    mut contexts: EguiContexts,
    mut ui_state: ResMut<LootUiState>,
    mut inventory: ResMut<PlayerInventory>,
    equipment: Res<EquipmentState>,
    dialog: Res<DialogState>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut piles: Query<(Entity, &mut LootPile, &LootInfo)>,
                      data: Res<GameData>,
                      mut mods: ResMut<ModTextures>,
                      mut images: ResMut<Assets<Image>>,
                      mut cache: ResMut<EguiLootTextures>,
                      look: Res<PlayerAppearance>,
                      mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if !ui_state.open {
        return;
    }
    // Se un altro menu prende il controllo, chiudi senza toccare il cursore.
    if equipment.open || dialog.open {
        close_loot_window(&mut ui_state, &mut windows, false);
        return;
    }
    let Some(pile_entity) = ui_state.pile else {
        close_loot_window(&mut ui_state, &mut windows, true);
        return;
    };
    let Ok((_entity, mut pile, info)) = piles.get_mut(pile_entity) else {
        close_loot_window(&mut ui_state, &mut windows, true);
        return;
    };
    pile.opened = true;

    let mut close = keys.just_pressed(KeyCode::Escape) || keys.just_pressed(KeyCode::KeyV);
    let mut take_gold = false;
    let mut take_all = false;
    let mut take_indices: Vec<usize> = Vec::new();

    let species = info.species;
    let sprite_archive = info.sprite_archive;
    let sprite_record = info.sprite_record;

    let ctx = contexts.ctx_mut();

    // Precarica le texture PRIMA della finestra (dentro la closure di show
    // non si può accedere al Context).
    let portrait = portrait_texture(
        ctx, &mut cache, &data, &mut mods, &mut images,
        sprite_archive, sprite_record,
    );
    let gold_icon = if pile.gold > 0 {
        Some(gold_texture(ctx, &mut cache))
    } else {
        None
    };
    let mut icon_ids: Vec<egui::TextureId> = Vec::with_capacity(pile.items.len());
    for item in pile.items.iter() {
        icon_ids.push(item_texture_id(
            ctx, &mut cache, &data, &mut mods, &mut images, *look, item,
        ));
    }

    let pile_ref: &LootPile = &pile;
    let mut open = true;
    egui::Window::new(format!("Bottino — {}", species_name(species)))
    .id(egui::Id::new("dfu_loot_window"))
    .collapsible(false)
    .resizable(false)
    .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 70.0))
    // `Window::open` in bevy_egui 0.33 (egui 0.29) vuole `&mut bool`,
    // non `Option<&mut bool>`.
    .open(&mut open)
    .show(ctx, |ui| {
        // Testata: ritratto del cadavere + riepilogo.
        ui.horizontal(|ui| {
            if let Some(tex) = portrait {
                ui.add(egui::Image::new((tex, egui::Vec2::splat(52.0))));
            }
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(species_name(species)).strong());
                ui.label(
                    egui::RichText::new(format!(
                        "{} oggetti · {} GP a terra",
                        pile_ref.items.len(),
                                                pile_ref.gold
                    ))
                    .weak(),
                );
            });
        });
        ui.separator();

        egui::ScrollArea::vertical().max_height(250.0).show(ui, |ui| {
            // Riga dell'oro.
            if let Some(gtex) = gold_icon {
                let row = ui.horizontal(|ui| {
                    ui.add(egui::Image::new((gtex, egui::Vec2::splat(26.0))));
                    ui.label(format!("{} monete d'oro", pile_ref.gold));
                });
                let resp = ui.interact(
                    row.response.rect,
                    ui.make_persistent_id("loot_gold_row"),
                                       egui::Sense::click(),
                );
                // `hovered()` fa un borrow: si può chiamare prima di consumare.
                if resp.hovered() {
                    ui.painter().rect_filled(
                        row.response.rect.expand(2.0),
                                             4.0,
                                             egui::Color32::from_white_alpha(20),
                    );
                }
                // `on_hover_ui_at_pointer` CONSUMA `self` (builder pattern):
                // riassegniamo il risultato a `resp` così possiamo ancora
                // chiamare `.clicked()`.
                let resp = resp.on_hover_ui_at_pointer(|ui| {
                    ui.label(
                        egui::RichText::new("Monete d'oro")
                        .strong()
                        .color(egui::Color32::from_rgb(245, 200, 80)),
                    );
                    ui.label(egui::RichText::new("Clicca per raccoglierle").small().weak());
                });
                if resp.clicked() {
                    take_gold = true;
                }
            }

            // Singoli oggetti: icona + nome, click = raccogli.
            for (idx, item) in pile_ref.items.iter().enumerate() {
                let row = ui.horizontal(|ui| {
                    ui.add(egui::Image::new((icon_ids[idx], egui::Vec2::splat(28.0))));
                    ui.label(&item.name);
                });
                let resp = ui.interact(
                    row.response.rect,
                    ui.make_persistent_id(("loot_item_row", idx)),
                                       egui::Sense::click(),
                );
                if resp.hovered() {
                    ui.painter().rect_filled(
                        row.response.rect.expand(2.0),
                                             4.0,
                                             egui::Color32::from_white_alpha(20),
                    );
                }
                // Stesso pattern: `on_hover_ui_at_pointer` consuma `resp`.
                let resp = resp.on_hover_ui_at_pointer(|ui| {
                    ui.set_max_width(260.0);
                    ui.label(
                        egui::RichText::new(item.name.clone())
                        .strong()
                        .color(egui::Color32::from_rgb(245, 200, 80)),
                    );
                    ui.label(format!(
                        "Slot: {} · Armor +{} · {:.1} kg",
                        item.slot.label(),
                                     item.armor_rating,
                                     item.weight_kg
                    ));
                    ui.label(egui::RichText::new(item.description.clone()).italics().weak());
                    ui.label(egui::RichText::new("Clicca per raccogliere").small().weak());
                });
                if resp.clicked() {
                    take_indices.push(idx);
                }
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Prendi tutto").clicked() {
                take_all = true;
            }
            if ui.button("Lascia").clicked() {
                close = true;
            }
        });
        ui.label(
            egui::RichText::new("[V] o [Esc] chiudono · il resto rimane a terra")
            .small()
            .weak(),
        );
    });
    if !open {
        close = true;
    }

    // --- Trasferimento nello zaino (stessa logica del vecchio interact) ---
    if take_all {
        take_gold = true;
        take_indices = (0..pile.items.len()).collect();
    }
    take_indices.sort_unstable();
    take_indices.dedup();

    if take_gold && pile.gold > 0 {
        inventory.gold += pile.gold;
        pile.gold = 0;
        inventory.dirty = true;
    }
    for &idx in take_indices.iter().rev() {
        if idx < pile.items.len() {
            let mut item = pile.items.remove(idx);
            item.equipped = false;
            item.id = inventory.items.iter().map(|i| i.id).max().unwrap_or(0) + 1;
            inventory.items.push(item);
            inventory.dirty = true;
        }
    }

    // Pile svuotato: il cadavere resta a terra (come in DFU; sparisce solo per
    // scadenza, vedi `despawn_expired_loot`), si chiude solo la finestra.
    // `interact_loot` ignora i pile vuoti, quindi V non lo riapre.
    if pile.items.is_empty() && pile.gold == 0 {
        close = true;
    }
    if close {
        keys.reset(KeyCode::KeyV);
        close_loot_window(&mut ui_state, &mut windows, true);
    }
}

// ---------------------------------------------------------------------------
// Cleanup
// ---------------------------------------------------------------------------

fn despawn_expired_loot(
    time: Res<Time>,
    mut commands: Commands,
    mut piles: Query<(Entity, &mut LootPile)>,
) {
    if LOOT_LIFETIME_SECS <= 0.0 {
        return;
    }
    let dt = time.delta_secs();
    for (e, mut pile) in &mut piles {
        pile.age += dt;
        if pile.age >= LOOT_LIFETIME_SECS {
            commands.entity(e).despawn_recursive();
        }
    }
}
