//! NPC "custom" (le `people` dei record RMB) con casa fissa.
//!
//! Difetto precedente: gli NPC venivano generati a ogni ingresso in UN QUALSIASI
//! interno, quindi spawnavano dappertutto. Qui invece:
//! - il giocatore ha UNA sola casa NPC: il primo edificio popolato in cui entra;
//! - la casa viene memorizzata nel salvataggio (`save.json`) e gli NPC ricompaiono
//!   solo quando si rientra in QUELL'edificio;
//! - se un NPC viene reclutato (tasto R vicino a lui) resta fuori dai piedi per
//!   sempre; quando TUTTI gli NPC della casa sono reclutati, la casa si libera e
//!   puo' essere rivendicata dal prossimo edificio popolato visitato.

use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::interior_enter::{BuildingKey, CurrentInterior};
use crate::interior_render::ActiveScene;
use crate::player::Player;

/// Identificativo stabile di un NPC dentro un interno: indice nella lista
/// `people` del BlockData interno (sotto-record dell'edificio).
pub type NpcIndex = usize;

/// Chiave completa di un NPC: edificio + indice.
pub type NpcKey = (String, u8, usize);

/// La casa degli NPC custom: l'unico edificio in cui possono essere generati.
/// `None` finche' il giocatore non entra per la prima volta in un edificio
/// popolato (o dopo che tutti gli NPC sono stati reclutati).
#[derive(Resource, Default)]
pub struct NpcHome {
    pub building: Option<BuildingKey>,
}

/// NPC custom presenti in scena (figli della radice dell'interno corrente).
#[derive(Component)]
pub struct CustomNpc {
    pub index: NpcIndex,
}

/// Payload del salvataggio: serializzabile, scritto in `save.json`.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveData {
    /// casa degli NPC: [block_name, subrecord], assente se libera
    pub npc_home: Option<(String, u8)>,
    /// NPC gia' reclutati (non vanno piu' rigenerati)
    pub recruited: Vec<NpcKey>,
}

const SAVE_FILE: &str = "save.json";

fn save_path() -> std::path::PathBuf {
    std::env::var("DFU_SAVE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(SAVE_FILE))
}

/// Insieme degli NPC reclutati, indicizzato per edificio.
#[derive(Resource, Default)]
pub struct RecruitedNpcs {
    pub set: HashMap<NpcKey, ()>,
}

impl RecruitedNpcs {
    pub fn is_recruited(&self, building: &BuildingKey, index: NpcIndex) -> bool {
        self.set.contains_key(&(building.block_name.clone(), building.subrecord, index))
    }

    pub fn recruit(&mut self, building: &BuildingKey, index: NpcIndex) {
        self.set.insert((building.block_name.clone(), building.subrecord, index), ());
    }

    /// quanti NPC risultano reclutati per un dato edificio
    pub fn count_for(&self, building: &BuildingKey) -> usize {
        let name = building.block_name.as_str();
        let sub = building.subrecord;
        self.set.keys().filter(|(n, s, _)| n == name && *s == sub).count()
    }
}

/// Sistema (Startup): carica `save.json` e ripristina casa + reclutati.
pub fn load_save(mut home: ResMut<NpcHome>, mut recruited: ResMut<RecruitedNpcs>) {
    let Ok(raw) = std::fs::read_to_string(save_path()) else { return };
    let Ok(data) = serde_json::from_str::<SaveData>(&raw) else {
        warn!("{} corrotto: ignorato", save_path().display());
        return;
    };
    home.building = data.npc_home.map(|(block_name, subrecord)| BuildingKey { block_name, subrecord });
    recruited.set = data.recruited.iter().cloned().map(|k| (k, ())).collect();
    info!(
        "Salvataggio caricato: casa NPC {}, {} reclutati",
        home.building.as_ref().map(|k| k.block_name.as_str()).unwrap_or("(nessuna)"),
        recruited.set.len(),
    );
}

/// Scrive il salvataggio su `save.json` (casa + elenco reclutati).
pub fn write_save(home: &NpcHome, recruited: &RecruitedNpcs) {
    let mut recruited_keys: Vec<NpcKey> = recruited.set.keys().cloned().collect();
    recruited_keys.sort();
    let data = SaveData {
        npc_home: home.building.as_ref().map(|k| (k.block_name.clone(), k.subrecord)),
        recruited: recruited_keys,
    };
    match serde_json::to_string_pretty(&data) {
        Ok(json) => {
            if let Err(e) = std::fs::write(save_path(), json) {
                warn!("impossibile scrivere {}: {e}", save_path().display());
            }
        }
        Err(e) => warn!("impossibile serializzare il salvataggio: {e}"),
    }
}

/// Regola d'oro: gli NPC custom si generano SOLO nella loro casa.
pub fn can_spawn_in(home: &NpcHome, building: &BuildingKey) -> bool {
    home.building.as_ref() == Some(building)
}

/// Da chiamare quando il giocatore entra in un edificio POPOLATO:
/// - se la casa e' libera, questo edificio diventa la casa degli NPC (viene
///   restituito `true` cosi' il chiamante puo' generarli e salvare);
/// - se la casa esiste gia', gli NPC si rigenerano solo rientrando in quell'edificio.
pub fn claim_home_on_enter(home: &mut NpcHome, building: &BuildingKey, has_people: bool) -> bool {
    if !has_people {
        return false;
    }
    match &home.building {
        Some(k) => k == building,
        None => {
            home.building = Some(building.clone());
            true
        }
    }
}

/// Recluta l'NPC custom piu' vicino al giocatore (raggio `RECRUIT_RADIUS`).
/// Se era l'ultimo NPC non reclutato della casa, la casa si libera affinche'
/// il prossimo edificio popolato visitato possa ospitare i nuovi NPC.
const RECRUIT_RADIUS: f32 = 2.5;

#[allow(clippy::too_many_arguments)]
pub fn recruit_npc_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    scene: Res<ActiveScene>,
    current: Res<CurrentInterior>,
    player: Query<&Transform, With<Player>>,
    npcs: Query<(&CustomNpc, &GlobalTransform)>,
    mut commands: Commands,
    mut home: ResMut<NpcHome>,
    mut recruited: ResMut<RecruitedNpcs>,
    people_counts: Res<PeopleCounts>,
) {
    // R vale solo dentro un interno che sia la casa degli NPC
    if *scene != ActiveScene::Interior || !keyboard.just_pressed(KeyCode::KeyR) {
        return;
    }
    let Some(building) = current.building.clone() else { return };
    if !can_spawn_in(&home, &building) {
        return;
    }
    let Ok(ptf) = player.get_single() else { return };
    let mut best: Option<(f32, Entity, NpcIndex)> = None;
    for (npc, gtf) in &npcs {
        let dist = gtf.translation().distance(ptf.translation);
        if dist < RECRUIT_RADIUS && best.map_or(true, |(bd, _, _)| dist < bd) {
            best = Some((dist, npc.entity(), npc.index));
        }
    }
    let Some((_, entity, index)) = best else { return };

    recruited.recruit(&building, index);
    commands.entity(entity).despawn_recursive();
    info!("NPC #{index} reclutato in {}", building.block_name);

    // casa libera quando non resta nessun NPC da generare
    let total = people_counts.get(&(building.block_name.clone(), building.subrecord)).copied();
    if let Some(total) = total {
        if recruited.count_for(&building) >= total {
            home.building = None;
            info!("Tutti gli NPC reclutati: la casa e' libera, il prossimo edificio popolato la occupera'");
        }
    }
    write_save(&home, &recruited);
}

/// Conteggio degli NPC di ogni interno visitato (registrato da `interior_enter`
/// quando la mesh viene costruita): serve a sapere quando la casa e' tutta vuota.
#[derive(Resource, Default)]
pub struct PeopleCounts {
    pub map: HashMap<(String, u8), usize>,
}

/// Registra il numero di NPC di un interno (chiamato da `interior_enter` dopo
/// aver costruito la mesh) per sapere quando la casa e' completamente svuotata.
pub fn register_npc_count(counts: &mut PeopleCounts, building: &BuildingKey, total: usize) {
    counts.map.insert((building.block_name.clone(), building.subrecord), total);
}

pub struct NpcHomePlugin;

impl Plugin for NpcHomePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NpcHome>()
            .init_resource::<RecruitedNpcs>()
            .init_resource::<PeopleCounts>()
            .add_systems(Startup, load_save)
            .add_systems(Update, recruit_npc_system);
    }
}
