//! Animazione e comportamento di camminata degli NPC per le strade delle città.
//!
//! Estratto da `tiles.rs` per alleggerirlo. Contiene **solo** il sistema
//! `update_npcs` (movimento, cambio target, cambio frame, orientamento verso
//! la camera) e nient'altro.
//!
//! Restano in `tiles.rs`:
//! - la struct `NpcWalker` (la usano anche `collect_tiles` per lo spawn e
//!   `Follower::from_walker` per convertire un NPC in follower);
//! - `NpcSpriteSet` e `build_npc_sprites` (usati in `collect_tiles`);
//! - gli helper `pick_target`, `npc_blocked`, `orientation_index` (condivisi
//!   con `update_followers`/`update_enemies` o con lo spawn).

use bevy::prelude::*;

use crate::data::terrain_mesh::{terrain_height, CELL_SIZE};
use crate::player::Player;
use crate::tiles::{
    npc_blocked, orientation_index, pick_target, NpcWalker, NPC_FRAME_SECONDS,
    NPC_IDLE_DISTANCE, NPC_SPEED, ORIENTATIONS,
};

pub fn update_npcs(
    time: Res<Time>,
    player: Query<&Transform, (With<Player>, Without<NpcWalker>)>,
    mut npcs: Query<(
        &mut NpcWalker,
        &mut Transform,
        &GlobalTransform,
        &mut MeshMaterial3d<StandardMaterial>,
    ), Without<crate::tiles::Follower>>,
    mut last_player_pos: Local<Option<Vec3>>,
) {
    let real_dt = time.delta_secs();
    let dt = real_dt.min(0.1);
    let Ok(player_tf) = player.get_single() else { return };
    let player_pos = player_tf.translation;
    let player_still =
        last_player_pos.is_some_and(|p| p.distance(player_pos) < 0.1 * real_dt.max(1e-4));
    *last_player_pos = Some(player_pos);
    let cam_forward = player_tf.forward();
    let cam_forward_xz = Vec2::new(cam_forward.x, cam_forward.z).normalize_or_zero();
    let yaw = (cam_forward_xz != Vec2::ZERO).then(|| (-cam_forward_xz.x).atan2(-cam_forward_xz.y));
    for (mut npc, mut tf, gtf, mut material) in &mut npcs {
        let npc = &mut *npc;
        let world = gtf.translation();
        let to_player = Vec2::new(player_pos.x - world.x, player_pos.z - world.z);
        let idle = player_still && to_player.length() < NPC_IDLE_DISTANCE;
        if !idle {
            let pos = tf.translation.xz();
            let to_target = npc.target - pos;
            let step = NPC_SPEED * dt;
            let mut new_pos = if to_target.length() > step {
                pos + to_target.normalize() * step
            } else {
                npc.target
            };
            let mut hit = false;
            if let Some(c) = npc.colliders.as_deref() {
                let feet =
                    terrain_height(&npc.samples, new_pos.x / CELL_SIZE, -new_pos.y / CELL_SIZE);
                let fixed = c.resolve(new_pos, feet, crate::tiles::NPC_RADIUS);
                hit = fixed.distance(new_pos) > 0.01;
                new_pos = fixed;
            }
            tf.translation.x = new_pos.x;
            tf.translation.z = new_pos.y;
            if hit || (npc.target - new_pos).length() < 0.1 {
                let blocked = |p: Vec2| npc_blocked(npc.colliders.as_deref(), &npc.samples, p);
                npc.target = pick_target(
                    new_pos,
                    &mut npc.heading,
                    npc.bounds,
                    &mut npc.rng,
                    hit,
                    &blocked,
                );
            }
            npc.frame_timer += dt;
            if npc.frame_timer >= NPC_FRAME_SECONDS {
                npc.frame_timer -= NPC_FRAME_SECONDS;
                npc.frame = (npc.frame + 1) % npc.sprites.walk[0].len();
            }
        }
        let (record, flip, frame, handle, size) = if idle {
            (5, false, 0, &npc.sprites.idle[0], npc.sprites.idle_size)
        } else {
            let (record, flip) = ORIENTATIONS[orientation_index(npc.heading, to_player)];
            let frames = &npc.sprites.walk[record];
            let frame = npc.frame % frames.len();
            (record, flip, frame, &frames[frame], npc.sprites.walk_size[record])
        };
        if npc.shown != Some((record, flip, frame)) {
            *material = MeshMaterial3d(handle.clone());
            npc.shown = Some((record, flip, frame));
        }
        tf.scale = Vec3::new(if flip { -size.x } else { size.x }, size.y, 1.0);
        let gx = (tf.translation.x / CELL_SIZE).clamp(0.0, 128.0);
        let gy = (-tf.translation.z / CELL_SIZE).clamp(0.0, 128.0);
        tf.translation.y = terrain_height(&npc.samples, gx, gy) + size.y * 0.5;
        if let Some(yaw) = yaw {
            tf.rotation = Quat::from_rotation_y(yaw);
        }
    }
}