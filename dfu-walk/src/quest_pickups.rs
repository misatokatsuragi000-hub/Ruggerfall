//! Pickup 3D degli oggetti di quest nei dungeon (es. il certificato di morte in `_ukcrypt_`).
//!
//! Quando entri in un dungeon, `spawn_dungeon_pickups` chiede a `QuestItems` cosa va messo in
//! quella localita' (`pending_pickups_for(.., nome)`) e lo materializza con lo stesso schema dei
//! `built.flats` di `dungeon.rs` (`wall_textures.flat_material`, figlio del `root`, `DungeonFlat`):
//! sparisce da solo con il root all'uscita e `face_dungeon_flats` lo gira verso il giocatore.
//!
//! Dove: tra i flat del dungeon RAGGIUNGIBILI A PIEDI dall'ingresso (`spell_guide::reachable_only`),
//! quelli piu' lontani (l'n-esimo oggetto sul n-esimo piu' lontano), appoggiato al pavimento
//! con `floor_at`. Ha una piccola luce calda
//! perche' i dungeon sono bui.
//!
//! Raccolta: [E] o click sinistro entro `PICKUP_REACH` metri. Consuma l'input (cosi' E non apre
//! anche una porta o esce) e invia `QuestItemPickup`: da li' lo gestisce `quest_items`
//! (zaino + toast "You pick up ...").
//!
//! Richiede la patch a `dungeon.rs` (`ActiveDungeon.name`, vedi `equipment_patch.md`).
//! In `main.rs`: `mod quest_pickups;` e `.add_plugins(quest_pickups::QuestPickupsPlugin)`.
//!
//! DA SCEGLIERE: `PICKUP_FLAT` (archivio, record) di uno sprite valido (carta, lettera, forziere).
//! Se non esiste compare un rettangolo dorato luminoso e il log lo dice.

use crate::data::building_gen::DF_UNIT;
use crate::dialog::DialogState;
use crate::dungeon::DungeonState;
use crate::fps_weapon::FpsWeaponState;
use crate::player::Player;
use bevy::ecs::system::SystemParam;


use crate::quest_items::{QuestItemPickup, QuestItems};
use crate::world::GameData;
use crate::{QuestLibRes, QuestStatesRes};
use bevy::prelude::*;
use std::cmp::Ordering;
use std::collections::HashSet;

/// TENTATIVO non verificato: sostituiscilo con uno sprite valido del tuo TEXTURE.NNN.
const PICKUP_FLAT: (u16, u8) = (205, 0);
const PICKUP_REACH: f32 = 2.5;

#[derive(SystemParam)]
pub struct FlatAssets<'w> {
    pub meshes: ResMut<'w, Assets<Mesh>>,
    pub images: ResMut<'w, Assets<Image>>,
    pub materials: ResMut<'w, Assets<StandardMaterial>>,
    pub wall_textures: ResMut<'w, crate::wall_textures::WallTextures>,
    pub mods: ResMut<'w, crate::mod_textures::ModTextures>,
}
/// Un oggetto di quest raccoglibile nel dungeon.
#[derive(Component, Clone, Debug)]
pub struct QuestPickup {
    pub symbol: String,
}

/// Dove mettere l'`n`-esimo oggetto: il `n`-esimo flat piu' lontano dall'ingresso (in orizzontale).
/// Senza flat, a fianco dell'ingresso.
pub fn pick_spot(flats: &[Vec3], spawn: Vec3, n: usize) -> Vec3 {
    let dist = |p: &Vec3| (p.x - spawn.x).hypot(p.z - spawn.z);
    let mut sorted: Vec<Vec3> = flats.to_vec();
    // stabile: a parita' di distanza resta l'ordine del dungeon, quindi il risultato e' deterministico
    sorted.sort_by(|a, b| dist(b).partial_cmp(&dist(a)).unwrap_or(Ordering::Equal));
    sorted.get(n).copied().unwrap_or(spawn + Vec3::new(1.5 * (n as f32 + 1.0), 0.0, 0.0))
}

pub struct QuestPickupsPlugin;

impl Plugin for QuestPickupsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                // prima delle porte e dell'uscita: se raccogli, E / click non fanno anche altro
                pickup_input
                    .before(crate::dungeon::dungeon_enter_exit)
                    .before(crate::dungeon::handle_dungeon_door_click),
                // dopo l'ingresso: il root del dungeon deve esistere
                spawn_dungeon_pickups.after(crate::dungeon::dungeon_enter_exit),
            ),
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_dungeon_pickups(
    mut commands: Commands,
    dungeon: Res<DungeonState>,
    items: Res<QuestItems>,
    lib: Res<QuestLibRes>,
    states: Res<QuestStatesRes>,
    data: Res<GameData>,
    mut assets: FlatAssets,
    mut last_root: Local<Option<Entity>>,
    existing: Query<&QuestPickup>,
) {
    let Some(active) = dungeon.active.as_ref() else {
        *last_root = None;
        return;
    };
    let fresh = *last_root != Some(active.root);
    if !fresh && !items.is_changed() {
        return;
    }
    *last_root = Some(active.root);

    let wanted = items.pending_pickups_for(&lib.0, &states.0, &active.name);
    if wanted.is_empty() {
        return;
    }
    let here: HashSet<&str> = existing.iter().map(|q| q.symbol.as_str()).collect();
    let built = &active.dungeon;
    let spawn = Vec3::new(built.spawn_pos[0], built.spawn_pos[1], built.spawn_pos[2]);
    let flats: Vec<Vec3> = built.flats.iter().map(|(p, _, _)| Vec3::new(p[0], p[1], p[2])).collect();
    // Solo punti raggiungibili a piedi dall'ingresso: niente oggetti in stanze isolate.
    // Se nessuno lo e', `pick_spot` ripiega su un punto accanto all'ingresso.
    let flats = if wanted.iter().any(|s| !here.contains(s.as_str())) {
        crate::spell_guide::reachable_only(built, spawn, flats)
    } else {
        flats
    };

    for (n, symbol) in wanted.iter().enumerate() {
        if here.contains(symbol.as_str()) {
            continue;
        }
        let spot = pick_spot(&flats, spawn, n);
        let floor = built.floor_at(spot.x, spot.z, spot.y, 2.0).unwrap_or(spot.y);

        // Stesso schema di dungeon.rs per i `built.flats`.
        let (archive, record) = PICKUP_FLAT;
        let (material, width, height) = match assets.wall_textures.flat_material(
            archive,
            record,
            &data,
            &mut assets.mods,
            &mut assets.images,
            &mut assets.materials,
        ) {
            Some((m, w_px, h_px)) => (m, w_px * DF_UNIT, h_px * DF_UNIT),
            None => {
                warn!("[quest_pickups] flat {archive}/{record} non trovato: scegli PICKUP_FLAT valido");
                let m = assets.materials.add(StandardMaterial {
                    base_color: Color::srgb(0.95, 0.80, 0.30),
                    emissive: LinearRgba::new(1.6, 1.2, 0.4, 1.0),
                    unlit: true,
                    double_sided: true,
                    cull_mode: None,
                    ..default()
                });
                (m, 0.35, 0.45)
            }
        };

        // Coordinate locali al root del dungeon, come i flat di `build_dungeon`.
        let pos = Vec3::new(spot.x, floor + height * 0.5 + 0.05, spot.z);
        let id = commands
            .spawn((
                Name::new(format!("QuestPickup {symbol}")),
                Mesh3d(assets.meshes.add(Rectangle::new(width, height))),
                MeshMaterial3d(material),
                Transform::from_translation(pos),
                Visibility::default(),
                crate::dungeon::DungeonFlat, // `face_dungeon_flats` lo gira verso il giocatore
                QuestPickup { symbol: symbol.clone() },
            ))
            .with_children(|c| {
                c.spawn((
                    PointLight {
                        color: Color::srgb(1.0, 0.85, 0.5),
                        intensity: 40_000.0,
                        range: 6.0,
                        shadows_enabled: false,
                        ..default()
                    },
                    Transform::default(),
                ));
            })
            .id();
        commands.entity(active.root).add_child(id);
        info!("[quest_pickups] `{symbol}` piazzato in {} a {pos}", active.name);
    }
}

/// [E] o click sinistro vicino a un pickup: lo raccoglie e consuma l'input.
#[allow(clippy::too_many_arguments)]
pub fn pickup_input(
    mut commands: Commands,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut mouse: ResMut<ButtonInput<MouseButton>>,
    dialog: Res<DialogState>,
    weapon: Option<Res<FpsWeaponState>>,
    dungeon: Res<DungeonState>,
    player: Query<(&Player, &Transform)>,
    pickups: Query<(Entity, &QuestPickup, &GlobalTransform)>,
    mut out: EventWriter<QuestItemPickup>,
) {
    if dialog.open || !dungeon.is_inside() {
        return;
    }
    let click = mouse.just_pressed(MouseButton::Left) && !weapon.as_ref().is_some_and(|w| w.drawn);
    let act = keys.just_pressed(KeyCode::KeyE);
    if !click && !act {
        return;
    }
    let Ok((p, tf)) = player.get_single() else { return };

    let best = pickups
        .iter()
        .filter_map(|(e, q, g)| {
            let t = g.translation();
            let d = (t.x - tf.translation.x).hypot(t.z - tf.translation.z);
            let dy = t.y - p.feet_y;
            (d <= PICKUP_REACH && dy > -0.5 && dy < 2.5).then(|| (d, e, q.symbol.clone()))
        })
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal));

    if let Some((_, entity, symbol)) = best {
        out.send(QuestItemPickup { symbol });
        commands.entity(entity).despawn_recursive();
        if act {
            keys.clear_just_pressed(KeyCode::KeyE);
        }
        if click {
            mouse.clear_just_pressed(MouseButton::Left);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_are_the_farthest_flats_first() {
        let spawn = Vec3::new(0.0, 0.0, 0.0);
        let flats = [
            Vec3::new(2.0, 1.0, 0.0),
            Vec3::new(30.0, 1.0, 0.0),
            Vec3::new(10.0, 1.0, 5.0),
        ];
        assert_eq!(pick_spot(&flats, spawn, 0), flats[1]);
        assert_eq!(pick_spot(&flats, spawn, 1), flats[2]);
        assert_eq!(pick_spot(&flats, spawn, 2), flats[0]);
    }

    #[test]
    fn spots_fall_back_next_to_the_entrance() {
        let spawn = Vec3::new(5.0, 0.0, 5.0);
        assert_eq!(pick_spot(&[], spawn, 0), Vec3::new(6.5, 0.0, 5.0));
        assert_eq!(pick_spot(&[Vec3::ZERO], spawn, 1), Vec3::new(8.0, 0.0, 5.0));
    }

    #[test]
    fn vertical_distance_is_ignored_when_ranking() {
        let spawn = Vec3::ZERO;
        let flats = [Vec3::new(5.0, 40.0, 0.0), Vec3::new(8.0, 0.0, 0.0)];
        assert_eq!(pick_spot(&flats, spawn, 0), flats[1]);
    }
}
