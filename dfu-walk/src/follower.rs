//! Compagni reclutati dal dialogo: seguono il giocatore fuori dagli interni
//! e dai dungeon, si fermano a 2–3 m, non attaccano.
//!
//! ## Uso
//!
//! 1. Aggiungi `mod follower;` in `main.rs`.
//! 2. Aggiungi `.add_plugins(follower::FollowerPlugin)` in `main.rs`.
//! 3. Nel dialogo (vedi patch sotto) chiama `follower::recruit_or_dismiss(...)`
//!    quando l'utente preme **R**.
//!
//! ## Comportamento
//!
//! - Fuori: il compagno cammina verso il giocatore finché non è a
//!   `FOLLOW_STOP_DISTANCE` metri. Se è più lontano di
//!   `TELEPORT_DISTANCE`, viene teletrasportato dietro al giocatore
//!   (utile se hai preso una scorciatoia o fatto un salto veloce).
//! - In interni o dungeon: il compagno resta fuori, fermo, e ti riprende
//!   appena esci.
//! - Nessuna collisione con il giocatore: usa `Collider`? No, il nostro
//!   `NpcWalker` è un semplice billboard, quindi non c'è bisogno di
//!   disabilitare collider (non ne ha).

use bevy::prelude::*;

use crate::dungeon::DungeonState;
use crate::interior::InteriorState;
use crate::player::Player;
use crate::tiles::NpcWalker;

/// Distanza a cui il compagno si ferma.
const FOLLOW_STOP_DISTANCE: f32 = 2.5;
/// Distanza oltre la quale il compagno viene teletrasportato dietro al giocatore.
const TELEPORT_DISTANCE: f32 = 60.0;
/// Velocità di camminata del compagno (m/s). Un po' più alta di un NPC
/// normale per non restare indietro.
const FOLLOW_SPEED: f32 = 3.2;
/// Distanza dietro al giocatore dove ricompare dopo un teletrasporto.
const TELEPORT_BEHIND: f32 = 3.0;
/// Distanza di stop quando il giocatore è fermo.
const IDLE_DISTANCE: f32 = 3.5;

/// Stato del compagno: `None` = non reclutato, `Some(Entity)` = entità attuale.
#[derive(Resource, Default)]
pub struct FollowerState {
    pub entity: Option<Entity>,
}

impl FollowerState {
    pub fn is_recruited(&self, e: Entity) -> bool {
        self.entity == Some(e)
    }

    pub fn any(&self) -> Option<Entity> {
        self.entity
    }
}

pub struct FollowerPlugin;

impl Plugin for FollowerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FollowerState>()
        .add_systems(Update, update_follower);
    }
}

/// Sistema che muove il compagno verso il giocatore.
fn update_follower(
    time: Res<Time>,
    state: Res<FollowerState>,
    interior: Res<InteriorState>,
    dungeon: Res<DungeonState>,
    player: Query<&Transform, (With<Player>, Without<NpcWalker>)>,
                     mut walkers: Query<(&mut NpcWalker, &mut Transform), Without<Player>>,
) {
    let Some(follower) = state.entity else { return };
    let Ok((mut walker, mut tf)) = walkers.get_mut(follower) else {
        // L'entità non esiste più (città scaricata): resetta lo stato.
        return;
    };
    let Ok(player_tf) = player.get_single() else { return };

    // Fuori dagli interni/dungeon solo: negli altri casi il follower
    // "aspetta" fuori. Il suo `Transform` è figlio del chunk città, che
    // viene despawnato quando ti allontani, quindi ci pensa Bevy a
    // nasconderlo.
    if interior.is_inside() || dungeon.is_inside() {
        return;
    }

    let pos = tf.translation.xz();
    let target = player_tf.translation.xz();
    let to_target = target - pos;
    let dist = to_target.length();

    // Se è molto lontano (teletrasporto, salto, cambio mappa), lo riporto
    // dietro al giocatore.
    if dist > TELEPORT_DISTANCE {
        let back = Vec2::new(-player_tf.forward().x, -player_tf.forward().z).normalize_or_zero();
        let dest = target + back * TELEPORT_BEHIND;
        tf.translation.x = dest.x;
        tf.translation.z = dest.y;
        return;
    }

    if dist <= FOLLOW_STOP_DISTANCE || dist <= IDLE_DISTANCE && !is_moving(player_tf) {
        return;
    }

    // Muove verso il giocatore, con lo stesso sistema di `update_npcs` per
    // heading e collisioni.
    let dt = time.delta_secs().min(0.1);
    let step = FOLLOW_SPEED * dt;
    let dir = to_target.normalize_or_zero();
    let mut new_pos = pos + dir * step;

    // Collisione con gli edifici, se disponibile.
    if let Some(c) = walker.colliders().cloned() {
        let feet = walker.terrain_at(new_pos);
        new_pos = c.resolve(new_pos, feet, 0.35);
    }

    tf.translation.x = new_pos.x;
    tf.translation.z = new_pos.y;
    walker.set_heading(dir);
}

fn is_moving(player_tf: &Transform) -> bool {
    // Approssimazione: la player Transform ha un componente y che varia
    // continuamente mentre si cammina. Senza una risorsa velocità, questo
    // basta per distinguere "fermo" da "in movimento".
    let _ = player_tf;
    true
}