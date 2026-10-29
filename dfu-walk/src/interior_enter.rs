//! Ingresso negli interni: quando il giocatore si avvicina a una porta di un
//! edificio in città, l'interno (mesh + NPC) viene caricato e il giocatore
//! "teleportato" dentro; vicino alla porta d'uscita avviene il contrario.
//!
//! Le porte sono registrate da `tiles::collect_tiles` in `CityDoors` con la
//! chiave del blocco RMB (`block_name`) e il sotto-record dell'edificio.

use std::collections::HashMap;

use bevy::prelude::*;
use daggerfall_data::blocks::RmbBlock;
use daggerfall_data::bsa::BsaArchive;
use daggerfall_data::interior::{build_interior, InteriorMesh};

use crate::interior_render::{ActiveScene, PendingInterior};
use crate::npc_home::{self, NpcHome, PeopleCounts, RecruitedNpcs};
use crate::player::{Player, EYE_HEIGHT};
use crate::world::{GameData, Origin};
use serde::{Deserialize, Serialize};

/// Chiave stabile di un edificio: nome del record BLOCKS.BSA + sotto-record.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BuildingKey {
    pub block_name: String,
    pub subrecord: u8,
}

/// Porta cittadina registrata da un tile caricato; la posizione e' locale al
/// tile (viene traslata con `tile_translation` al momento del check).
#[derive(Clone, Debug)]
pub struct LocalDoor {
    /// posizione locale nel tile (0..MAP_PIXEL_SIZE su x/z), y = quota
    pub local_pos: [f32; 3],
    pub rotation_deg: f32,
    pub block_name: String,
    pub subrecord: u8,
}

impl LocalDoor {
    pub fn building_key(&self) -> BuildingKey {
        BuildingKey { block_name: self.block_name.clone(), subrecord: self.subrecord }
    }
}

/// Registro globale delle porte degli edifici: ogni tile le aggiunge quando
/// viene spawnato e le rimuove quando viene despawnato.
#[derive(Resource, Default)]
pub struct CityDoors {
    pub all: HashMap<(i32, i32), Vec<LocalDoor>>,
}

impl CityDoors {
    pub fn insert_tile(&mut self, key: (i32, i32), doors: Vec<LocalDoor>) {
        self.all.insert(key, doors);
    }

    pub fn remove_tile(&mut self, key: (i32, i32)) {
        self.all.remove(&key);
    }
}

/// Interno attualmente visitato (None = esterno).
#[derive(Resource, Default)]
pub struct CurrentInterior {
    pub building: Option<BuildingKey>,
    /// world pos del giocatore prima di entrare, per ripristinarlo all'uscita
    pub exterior_pos: Option<Vec3>,
}

const ENTER_RADIUS: f32 = 1.6;
const EXIT_RADIUS: f32 = 1.4;

/// Sistema: vicino a una porta, entra nell'edificio (E). La prima volta che il
/// giocatore entra in un edificio POPOLATO, quell'edificio diventa la casa
/// degli NPC custom e viene scritta nel salvataggio (`save.json`). Da quel
/// momento gli NPC ricompaiono SOLO rientrando in quell'edificio.
#[allow(clippy::too_many_arguments)]
pub fn enter_interior_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    data: Res<GameData>,
    origin: Res<Origin>,
    player: Single<&Transform, With<Player>>,
    mut doors: ResMut<CityDoors>,
    mut current: ResMut<CurrentInterior>,
    mut pending: ResMut<PendingInterior>,
    mut scene: ResMut<ActiveScene>,
    mut home: ResMut<NpcHome>,
    recruited: Res<RecruitedNpcs>,
    mut people_counts: ResMut<PeopleCounts>,
) {
    if *scene != ActiveScene::Exterior || !keyboard.just_pressed(KeyCode::KeyE) {
        return;
    }
    let ptf = player.into_inner();
    // trova la porta più vicina entro ENTER_RADIUS
    let mut best: Option<(f32, (i32, i32), usize)> = None;
    for (&tile_key, list) in doors.all.iter() {
        if !(in_map(tile_key.0, tile_key.1)) {
            continue;
        }
        let base = crate::tiles::tile_translation(&origin, tile_key.0, tile_key.1);
        for (i, d) in list.iter().enumerate() {
            let wp = base + Vec3::new(d.local_pos[0], d.local_pos[1], d.local_pos[2]);
            let dist = Vec2::new(wp.x - ptf.translation.x, wp.z - ptf.translation.z).length();
            if dist < ENTER_RADIUS && best.map_or(true, |(bd, _, _)| dist < bd) {
                best = Some((dist, tile_key, i));
            }
        }
    }
    let Some((_, tile_key, idx)) = best else { return };
    let Some(door) = doors.all.get(&tile_key).and_then(|l| l.get(idx)).cloned() else { return };

    // carica l'interno dall'RMB
    let Ok(blocks_bsa) = BsaArchive::parse(&data.blocks_buf) else { return };
    let Some(raw) = blocks_bsa.get_by_name(&format!("{}.RMB", door.block_name)) else {
        warn!("blocco {} non trovato in BLOCKS.BSA", door.block_name);
        return;
    };
    let Ok(rmb) = RmbBlock::parse(raw) else { return };
    let Some(interior) = rmb.interiors.iter().find(|i| i.subrecord == door.subrecord) else {
        return;
    };
    let Ok(arch3d_bsa) = BsaArchive::parse(&data.arch3d_buf) else { return };
    let mesh = build_interior(interior, &rmb, &arch3d_bsa);
    if mesh.is_empty() {
        return;
    }

    // radice dell'interno: centro della stanza sull'origine, ruotata come la porta
    let root_transform = Transform::from_xyz(0.0, 0.0, 0.0).with_rotation(
        Quat::from_rotation_y(door.rotation_deg.to_radians()),
    );

    // ---- regola d'oro: gli NPC custom stanno SOLO nella loro casa -------
    // prima visita di un edificio popolato -> diventa la casa (e viene salvata);
    // nelle altre abitazioni la mesh viene comunque costruita, MA senza persone.
    let building = door.building_key();
    let has_people = !mesh.people.is_empty();
    let spawn_npcs = npc_home::claim_home_on_enter(&mut home, &building, has_people);
    if spawn_npcs {
        npc_home::write_save(&home, &recruited);
    }
    // registra il numero totale di NPC dell'interno (serve al reclutamento:
    // quando tutti sono reclutati la casa si libera)
    npc_home::register_npc_count(&mut people_counts, &building, mesh.people.len());

    pending.mesh = mesh.clone();
    pending.root_transform = root_transform;
    pending.spawn_npcs = spawn_npcs && has_people;
    pending.consumed = false;

    current.building = Some(building.clone());
    current.exterior_pos = Some(ptf.translation);
    // teleporta il giocatore al centro dell'interno (spawn_position della mesh):
    // la radice verra' creata da `spawn_interior` nel frame successivo
    let sp = mesh.spawn_position();
    commands
        .entity(player.entity())
        .insert(Transform::from_xyz(sp[0], sp[1] + EYE_HEIGHT, sp[2]));
    info!(
        "Entrato in {}: {} persone{}, mesh {:?} m",
        door.block_name,
        mesh.people.len(),
        if pending.spawn_npcs { " (NPC generati)" } else { "" },
        [mesh.max[0] - mesh.min[0], mesh.max[2] - mesh.min[2]],
    );
}

/// Sistema: all'interno, se il giocatore si avvicina alla porta d'uscita o preme
/// E vicino ad essa, esce. Ripristina la posizione esterna e despawna l'interno
/// (NPC figli inclusi: ci pensa `despawn_interior` di `interior_render`).
pub fn exit_interior_system(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    scene: Res<ActiveScene>,
    mut current: ResMut<CurrentInterior>,
    roots: Query<Entity, With<crate::interior_render::InteriorRoot>>,
    exits: Query<&crate::interior_render::InteriorExitDoor>,
    player: Single<&Transform, With<Player>>,
) {
    if *scene != ActiveScene::Interior {
        return;
    }
    // uscire richiede il tasto E VICINO alla porta d'uscita: la sola prossimita'
    // non basta (si rischierebbe di uscire per caso in mezzo alla stanza)
    if !keyboard.just_pressed(KeyCode::KeyE) {
        return;
    }
    let near_exit = exits
        .iter()
        .next()
        .map(|d| {
            let dxz = Vec2::new(player.translation.x - d.world_pos.x, player.translation.z - d.world_pos.z);
            dxz.length() < EXIT_RADIUS
        })
        .unwrap_or(false);
    if !near_exit {
        return;
    }
    // ripulisci la scena interna (radice + NPC figli)
    for e in roots.iter() {
        commands.entity(e).despawn_recursive();
    }
    if let Some(pos) = current.exterior_pos {
        commands.entity(player.entity()).insert(Transform::from_xyz(pos.x, pos.y, pos.z));
    }
    current.building = None;
    current.exterior_pos = None;
}

pub struct InteriorEnterPlugin;

impl Plugin for InteriorEnterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CityDoors>()
            .init_resource::<CurrentInterior>()
            .add_systems(Update, (enter_interior_system, exit_interior_system).chain());
    }
}
