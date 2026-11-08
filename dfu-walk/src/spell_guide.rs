//! Incantesimo "Pathfinder": mostra la strada verso l'obiettivo della quest nel dungeon.
//!
//! Il progetto non ha ancora un sistema di incantesimi: questo e' un incantesimo solo, a
//! tasto (`CAST_KEY`), senza mana. Vale solo dentro un dungeon e punta all'oggetto di quest
//! piazzato li' (`QuestPickup`, vedi `quest_pickups.rs`).
//!
//! Cosa fa, e perche' "dimostra che esiste anche se invisibile":
//!   - FARO: una colonna luminosa e una luce sull'obiettivo, anche se lo sprite non si vede;
//!   - SCIA: punti luminosi sul pavimento lungo il percorso a piedi (A* di `pathfind.rs` sulla
//!     geometria reale: muri e porte chiuse, scale incluse). I punti pulsano verso l'obiettivo
//!     e spariscono man mano che li superi; se esci dal percorso la scia si ricalcola;
//!   - AVVISI: a ogni lancio e ogni `REPORT_EVERY` secondi, distanza e direzione rispetto a
//!     dove guardi ("38 m, ahead and to the right, below you");
//!   - se non esiste un percorso calpestabile (livelli non collegati, porte bloccate...) resta
//!     il faro e l'avviso di direzione, e il testo lo dice.
//!
//! Dura `DURATION_SECS`; si rilancia per rinfrescarla. Finisce da sola quando raccogli
//! l'oggetto o esci dal dungeon.
//!
//! Le porte CHIUSE si considerano attraversabili (la scia passa per la porta: la apri tu);
//! per il resto vale `BuiltDungeon::blocked`.
//!
//! In `main.rs`: `mod pathfind; mod spell_guide;` e `.add_plugins(spell_guide::GuideSpellPlugin)`.
//! Se G e' gia' usato altrove, cambia `CAST_KEY`.

use crate::data::dungeon_builder::BuiltDungeon;
use crate::dialog::DialogState;
use crate::dungeon::DungeonState;
use crate::equipment::EquipmentState;
use crate::journal::JournalState;
use crate::pathfind::{flood, path_length, resample, Search, Status};
use crate::player::Player;
use crate::quest_items::display_name;
use crate::quest_pickups::QuestPickup;
use crate::toast::Toasts;
use bevy::prelude::*;
use std::cmp::Ordering;

const CAST_KEY: KeyCode = KeyCode::KeyG;
const CAST_KEY_LABEL: &str = "G";
/// Durata dell'incantesimo, in secondi reali.
const DURATION_SECS: f32 = 120.0;
/// Nodi A* espansi per frame.
const SEARCH_BUDGET: usize = 2_500;
const MAX_EXPANDED: usize = 300_000;
const CELL: f32 = 1.0;
const GOAL_RADIUS: f32 = 1.8;
const DOT_SPACING: f32 = 1.6;
const DOT_LIFT: f32 = 0.15;
/// Un punto si spegne quando il giocatore gli passa a meno di cosi' (orizzontale, m).
const DOT_CONSUMED_DIST: f32 = 1.3;
/// Oltre questa distanza dalla scia si ricalcola.
const STRAY_DIST: f32 = 9.0;
const REPORT_EVERY: f32 = 8.0;
/// Margine dai muri nel test di camminabilita' (il giocatore ha raggio 0.35).
const CHECK_RADIUS: f32 = 0.45;
const MAX_STEP_UP: f32 = 0.85;
const MAX_DROP: f32 = 1.3;
/// Una porta chiusa rende attraversabili le celle entro questo raggio dal suo centro.
const DOOR_PASS_RADIUS: f32 = 1.2;
/// Un punto e' "raggiungibile" se una cella calpestabile dell'ingresso sta entro questi
/// limiti (orizzontale, verticale in m). Piu' stretti di `GOAL_RADIUS` e del dy di `Search`:
/// quello che passa qui, la scia lo trova.
const REACH_RADIUS: f32 = 1.5;
const REACH_DY: f32 = 2.0;

// =============================================================================
// Direzione (puro, testato)
// =============================================================================

/// Dove sta `to` rispetto a dove guardi (`forward`), nel piano orizzontale `[x, z]`.
/// Bevy: avanti e' -Z, destra e' +X.
pub fn relative_direction(forward: [f32; 2], to: [f32; 2]) -> &'static str {
    let fl = forward[0].hypot(forward[1]);
    let tl = to[0].hypot(to[1]);
    if fl < 1e-4 || tl < 1e-4 {
        return "right here";
    }
    let (fx, fz) = (forward[0] / fl, forward[1] / fl);
    let (tx, tz) = (to[0] / tl, to[1] / tl);
    let ahead = fx * tx + fz * tz;
    let right = -fz * tx + fx * tz;
    let a = right.atan2(ahead).to_degrees();
    if a.abs() <= 22.5 {
        "straight ahead"
    } else if a > 22.5 && a <= 67.5 {
        "ahead and to the right"
    } else if a > 67.5 && a <= 112.5 {
        "to your right"
    } else if a > 112.5 && a <= 157.5 {
        "behind you to the right"
    } else if a < -22.5 && a >= -67.5 {
        "ahead and to the left"
    } else if a < -67.5 && a >= -112.5 {
        "to your left"
    } else if a < -112.5 && a >= -157.5 {
        "behind you to the left"
    } else {
        "behind you"
    }
}

/// "Death certificate — 38 m, ahead and to the right, below you". `to` e' il vettore
/// dal giocatore all'obiettivo.
pub fn describe_target(name: &str, to: Vec3, forward: Vec3) -> String {
    let dir = relative_direction([forward.x, forward.z], [to.x, to.z]);
    let vert = if to.y > 3.0 {
        ", above you"
    } else if to.y < -3.0 {
        ", below you"
    } else {
        ""
    };
    format!("{name} — {:.0} m, {dir}{vert}", to.length())
}

// =============================================================================
// Stato
// =============================================================================

/// Perche' le celle sono state scartate dalla ricerca (solo per la diagnostica).
#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct ProbeStats {
    pub(crate) ok: u32,
    pub(crate) no_floor: u32,
    pub(crate) too_low: u32,
    pub(crate) blocked: u32,
}

/// Si puo' stare in piedi in (x, z)? Coordinate locali al dungeon. Restituisce la quota del
/// pavimento. Condivisa tra la scia (`run_guide_search`) e la validazione dei punti di
/// piazzamento (`reachable_only`): le due devono dare per forza le stesse risposte.
///
/// `floor_at` con step = MAX_STEP_UP: come il giocatore, sceglie il pavimento piu' alto
/// raggiungibile con uno scalino. Le porte CHIUSE sono attraversabili (la apri tu).
pub(crate) fn dungeon_probe<'a>(
    built: &'a BuiltDungeon,
    mut stats: Option<&'a mut ProbeStats>,
) -> impl FnMut(f32, f32, f32) -> Option<f32> + 'a {
    let closed_doors: Vec<[f32; 3]> = built
        .doors
        .iter()
        .enumerate()
        .filter(|(i, _)| !built.door_open[*i])
        .map(|(_, d)| [d.center[0], d.center[1], d.center[2]])
        .collect();
    move |x: f32, z: f32, hint: f32| -> Option<f32> {
        let Some(fy) = built.floor_at(x, z, hint, MAX_STEP_UP) else {
            if let Some(s) = stats.as_deref_mut() {
                s.no_floor += 1;
            }
            return None;
        };
        if hint - fy > MAX_DROP {
            if let Some(s) = stats.as_deref_mut() {
                s.too_low += 1;
            }
            return None;
        }
        if built.blocked(x, z, fy, CHECK_RADIUS) {
            let at_closed_door = closed_doors
                .iter()
                .any(|c| (c[0] - x).hypot(c[2] - z) <= DOOR_PASS_RADIUS && (c[1] - fy).abs() < 2.5);
            if !at_closed_door {
                if let Some(s) = stats.as_deref_mut() {
                    s.blocked += 1;
                }
                return None;
            }
        }
        if let Some(s) = stats.as_deref_mut() {
            s.ok += 1;
        }
        Some(fy)
    }
}

/// Dei punti candidati (coordinate locali al dungeon) tiene solo quelli raggiungibili a
/// piedi dall'ingresso `spawn`, con le stesse regole della scia. Serve a non piazzare gli
/// oggetti di quest in stanze isolate. L'ordine dei punti resta quello di partenza.
pub fn reachable_only(built: &BuiltDungeon, spawn: Vec3, spots: Vec<Vec3>) -> Vec<Vec3> {
    let start_y = built.floor_at(spawn.x, spawn.z, spawn.y, 2.0).unwrap_or(spawn.y);
    let mut probe = dungeon_probe(built, None);
    let reach = flood([spawn.x, start_y, spawn.z], CELL, MAX_EXPANDED, &mut probe);
    let total = spots.len();
    let kept: Vec<Vec3> = spots
        .into_iter()
        .filter(|p| {
            let fy = built.floor_at(p.x, p.z, p.y, 2.0).unwrap_or(p.y);
            reach.contains_near([p.x, fy, p.z], REACH_RADIUS, REACH_DY)
        })
        .collect();
    info!(
        "[guide] raggiungibili dall'ingresso: {} celle{}; {} punti candidati su {} sono raggiungibili",
        reach.len(),
        if reach.truncated() { " (TETTO raggiunto: insieme incompleto)" } else { "" },
        kept.len(),
        total,
    );
    kept
}

#[derive(Component)]
struct GuideDot(usize);

#[derive(Component)]
struct GuideBeacon;

struct Target {
    symbol: String,
    name: String,
    /// Coordinate locali al dungeon.
    pos: Vec3,
}

#[derive(Resource, Default)]
pub struct GuideSpell {
    root: Option<Entity>,
    until: f32,
    target: Option<Target>,
    search: Option<Search>,
    dots: Vec<(Entity, Vec3)>,
    beacon: Option<Entity>,
    last_report: f32,
    last_search: f32,
    /// L'ultima ricerca non ha trovato un percorso: non si riprova da soli.
    failed: bool,
    stats: ProbeStats,
}

fn clear_trail(commands: &mut Commands, g: &mut GuideSpell, despawn: bool) {
    if despawn {
        for (e, _) in g.dots.drain(..) {
            commands.entity(e).despawn_recursive();
        }
        if let Some(b) = g.beacon.take() {
            commands.entity(b).despawn_recursive();
        }
    } else {
        g.dots.clear();
        g.beacon = None;
    }
    g.search = None;
}

fn spawn_beacon(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    root: Entity,
    at: Vec3,
) -> Entity {
    let mat = materials.add(StandardMaterial {
        base_color: Color::srgba(1.0, 0.85, 0.4, 0.30),
        emissive: LinearRgba::new(2.0, 1.5, 0.5, 1.0),
        unlit: true,
        alpha_mode: AlphaMode::Blend,
        double_sided: true,
        cull_mode: None,
        ..default()
    });
    let id = commands
        .spawn((
            Name::new("GuideBeacon"),
            Mesh3d(meshes.add(Cylinder::new(0.07, 6.0))),
            MeshMaterial3d(mat),
            Transform::from_translation(at + Vec3::Y * 2.5),
            Visibility::default(),
            GuideBeacon,
        ))
        .with_children(|c| {
            c.spawn((
                PointLight {
                    color: Color::srgb(1.0, 0.85, 0.5),
                    intensity: 120_000.0,
                    range: 14.0,
                    shadows_enabled: false,
                    ..default()
                },
                Transform::default(),
            ));
        })
        .id();
    commands.entity(root).add_child(id);
    id
}

fn spawn_dots(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    root: Entity,
    points: &[[f32; 3]],
) -> Vec<(Entity, Vec3)> {
    let mesh = meshes.add(Sphere::new(0.09));
    let mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.5, 0.9, 1.0),
        emissive: LinearRgba::new(0.8, 2.2, 3.0, 1.0),
        unlit: true,
        ..default()
    });
    let mut out = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        let pos = Vec3::new(p[0], p[1] + DOT_LIFT, p[2]);
        let id = commands
            .spawn((
                Name::new("GuideDot"),
                Mesh3d(mesh.clone()),
                MeshMaterial3d(mat.clone()),
                Transform::from_translation(pos),
                Visibility::default(),
                GuideDot(i),
            ))
            .id();
        commands.entity(root).add_child(id);
        out.push((id, pos));
    }
    out
}

// =============================================================================
// Plugin
// =============================================================================

pub struct GuideSpellPlugin;

impl Plugin for GuideSpellPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GuideSpell>().add_systems(
            Update,
            (announce_guide, cast_guide, run_guide_search, animate_guide)
                .chain()
                // dopo l'ingresso nel dungeon e dopo lo spawn dei pickup (devono esistere)
                .after(crate::dungeon::dungeon_enter_exit)
                .after(crate::quest_pickups::spawn_dungeon_pickups),
        );
    }
}

/// Un solo avviso quando compare un obiettivo: ricorda che l'incantesimo esiste.
fn announce_guide(pickups: Query<&QuestPickup, Added<QuestPickup>>, mut toasts: ResMut<Toasts>) {
    if pickups.iter().next().is_some() {
        toasts.push(format!(
            "You sense something here that does not belong. [{CAST_KEY_LABEL}] casts Pathfinder."
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn cast_guide(
    mut commands: Commands,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    time: Res<Time>,
    dialog: Res<DialogState>,
    equipment: Res<EquipmentState>,
    journal: Res<JournalState>,
    dungeon: Res<DungeonState>,
    player: Query<(&Player, &Transform)>,
    pickups: Query<(&QuestPickup, &Transform)>,
    mut guide: ResMut<GuideSpell>,
    mut toasts: ResMut<Toasts>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !keys.just_pressed(CAST_KEY) || dialog.open || equipment.open || journal.open {
        return;
    }
    keys.clear_just_pressed(CAST_KEY);

    let Some(active) = dungeon.active.as_ref() else {
        toasts.push("Pathfinder: there is nothing to find out here.");
        return;
    };
    let Ok((p, ptf)) = player.get_single() else { return };
    let o = active.origin;
    let me = Vec3::new(ptf.translation.x - o.x, p.feet_y - o.y, ptf.translation.z - o.z);

    // l'obiettivo piu' vicino (in linea d'aria)
    let best = pickups
        .iter()
        .map(|(q, t)| (q.symbol.clone(), t.translation))
        .min_by(|a, b| (a.1 - me).length().partial_cmp(&(b.1 - me).length()).unwrap_or(Ordering::Equal));
    let Some((symbol, pos)) = best else {
        toasts.push("Pathfinder: nothing in this place calls to you.");
        return;
    };

    clear_trail(&mut commands, &mut guide, true);
    let name = display_name(&symbol, "letter");
    let now = time.elapsed_secs();
    guide.root = Some(active.root);
    guide.until = now + DURATION_SECS;
    guide.last_report = now;
    guide.last_search = now;
    guide.failed = false;
    guide.stats = ProbeStats::default();
    guide.beacon = Some(spawn_beacon(&mut commands, &mut meshes, &mut materials, active.root, pos));
    guide.search = Some(Search::new([me.x, me.y, me.z], [pos.x, pos.y, pos.z], CELL, GOAL_RADIUS, MAX_EXPANDED));

    let forward = ptf.rotation * Vec3::NEG_Z;
    toasts.push(format!("Pathfinder: {}", describe_target(&name, pos - me, forward)));
    guide.target = Some(Target { symbol, name, pos });
}

#[allow(clippy::too_many_arguments)]
fn run_guide_search(
    mut commands: Commands,
    mut guide: ResMut<GuideSpell>,
    dungeon: Res<DungeonState>,
    player: Query<(&Player, &Transform)>,
    mut toasts: ResMut<Toasts>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if guide.search.is_none() {
        return;
    }
    let (Some(active), Some(root)) = (dungeon.active.as_ref(), guide.root) else { return };
    if active.root != root {
        return; // `animate_guide` ripulisce
    }
    let built = &active.dungeon;
    let g = &mut *guide;
    let mut probe = dungeon_probe(built, Some(&mut g.stats));

    let status = match g.search.as_mut() {
        Some(s) => s.step(SEARCH_BUDGET, &mut probe),
        None => return,
    };
    drop(probe); // rilascia il prestito di `g.stats` prima di toccare di nuovo `guide`

    match status {
        Status::Searching => {}
        Status::Failed => {
            // diagnostica: dove si e' fermata la ricerca e perche' le celle sono state scartate
            let info = guide.search.as_ref().map(|s| (s.expanded(), s.hit_limit(), s.closest()));
            let stats = guide.stats;
            guide.search = None;
            guide.failed = true;
            let (Ok((p, ptf)), Some(t)) = (player.get_single(), guide.target.as_ref()) else { return };
            let o = active.origin;
            let me = Vec3::new(ptf.translation.x - o.x, p.feet_y - o.y, ptf.translation.z - o.z);
            let forward = ptf.rotation * Vec3::NEG_Z;
            let mut text = format!(
                "Pathfinder: no walkable trail found, but it is there: {}",
                describe_target(&t.name, t.pos - me, forward)
            );
            if let Some((expanded, hit_limit, closest)) = info {
                info!(
                    "[guide] nessun percorso: {expanded} nodi espansi{}; celle ok {}, senza pavimento {}, troppo in basso {}, bloccate {}; obiettivo a {:?}; punto raggiunto piu' vicino {:?}",
                    if hit_limit { " (TETTO MAX_EXPANDED raggiunto)" } else { "" },
                    stats.ok, stats.no_floor, stats.too_low, stats.blocked,
                    t.pos,
                    closest,
                );
                if let Some((_, d)) = closest {
                    text.push_str(&format!(" (the closest I could get is {d:.0} m away)"));
                }
            }
            toasts.push(text);
        }
        Status::Found(path) => {
            guide.search = None;
            guide.failed = false;
            let points = resample(&path, DOT_SPACING);
            let dots = spawn_dots(&mut commands, &mut meshes, &mut materials, root, &points);
            guide.dots = dots;
            toasts.push(format!("Pathfinder: the trail is {:.0} m long. Follow the light.", path_length(&path)));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn animate_guide(
    mut commands: Commands,
    time: Res<Time>,
    dungeon: Res<DungeonState>,
    player: Query<(&Player, &Transform)>,
    pickups: Query<&QuestPickup>,
    mut guide: ResMut<GuideSpell>,
    mut toasts: ResMut<Toasts>,
    mut dots: Query<(&GuideDot, &mut Transform), Without<Player>>,
) {
    let Some(root) = guide.root else { return };
    let now = time.elapsed_secs();

    // Usciti dal dungeon: il root (e i figli) sono gia' spariti.
    let Some(active) = dungeon.active.as_ref().filter(|a| a.root == root) else {
        clear_trail(&mut commands, &mut guide, false);
        *guide = GuideSpell::default();
        return;
    };

    if now >= guide.until {
        clear_trail(&mut commands, &mut guide, true);
        *guide = GuideSpell::default();
        toasts.push("Pathfinder: the trail fades.");
        return;
    }
    let symbol = guide.target.as_ref().map(|t| t.symbol.clone()).unwrap_or_default();
    if !pickups.iter().any(|q| q.symbol == symbol) {
        clear_trail(&mut commands, &mut guide, true);
        *guide = GuideSpell::default();
        toasts.push("Pathfinder: the trail fades. You have found what you were looking for.");
        return;
    }

    let Ok((p, ptf)) = player.get_single() else { return };
    let o = active.origin;
    let me = Vec3::new(ptf.translation.x - o.x, p.feet_y - o.y, ptf.translation.z - o.z);
    let flat = |v: Vec3| (v.x - me.x).hypot(v.z - me.z);

    // spegni i punti che hai superato (tutti quelli fino al piu' vicino, se sei sopra di lui)
    let nearest = guide
        .dots
        .iter()
        .enumerate()
        .map(|(i, (_, pos))| (i, flat(*pos)))
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal));
    if let Some((idx, d)) = nearest {
        if d < DOT_CONSUMED_DIST {
            let gone: Vec<Entity> = guide.dots.drain(..=idx).map(|(e, _)| e).collect();
            for e in gone {
                commands.entity(e).despawn_recursive();
            }
        }
    }

    // fuori percorso: ricalcola (in silenzio, al massimo ogni 3 s e solo se prima funzionava)
    let target_pos = guide.target.as_ref().map_or(Vec3::ZERO, |t| t.pos);
    let lost = if guide.dots.is_empty() {
        flat(target_pos) > GOAL_RADIUS + 2.5
    } else {
        guide.dots.iter().map(|(_, pos)| flat(*pos)).fold(f32::MAX, f32::min) > STRAY_DIST
    };
    if lost && !guide.failed && guide.search.is_none() && now - guide.last_search > 3.0 {
        let old: Vec<Entity> = guide.dots.drain(..).map(|(e, _)| e).collect();
        for e in old {
            commands.entity(e).despawn_recursive();
        }
        guide.last_search = now;
        guide.stats = ProbeStats::default();
        guide.search = Some(Search::new(
            [me.x, me.y, me.z],
            [target_pos.x, target_pos.y, target_pos.z],
            CELL,
            GOAL_RADIUS,
            MAX_EXPANDED,
        ));
    }

    // avviso periodico
    if now - guide.last_report >= REPORT_EVERY {
        guide.last_report = now;
        if let Some(t) = guide.target.as_ref() {
            let forward = ptf.rotation * Vec3::NEG_Z;
            toasts.push(format!("Pathfinder: {}", describe_target(&t.name, t.pos - me, forward)));
        }
    }

    // i punti pulsano verso l'obiettivo
    let t = now * 5.0;
    for (dot, mut tf) in &mut dots {
        let s = 0.55 + 0.45 * (0.5 + 0.5 * (t - dot.0 as f32 * 0.55).sin());
        tf.scale = Vec3::splat(s);
    }
}

// =============================================================================
// Test
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    // Bevy: avanti = -Z, destra = +X
    const FWD: [f32; 2] = [0.0, -1.0];

    #[test]
    fn directions_around_the_player() {
        assert_eq!(relative_direction(FWD, [0.0, -5.0]), "straight ahead");
        assert_eq!(relative_direction(FWD, [5.0, 0.0]), "to your right");
        assert_eq!(relative_direction(FWD, [-5.0, 0.0]), "to your left");
        assert_eq!(relative_direction(FWD, [0.0, 5.0]), "behind you");
        assert_eq!(relative_direction(FWD, [5.0, -5.0]), "ahead and to the right");
        assert_eq!(relative_direction(FWD, [-5.0, -5.0]), "ahead and to the left");
        assert_eq!(relative_direction(FWD, [5.0, 5.0]), "behind you to the right");
        assert_eq!(relative_direction(FWD, [-5.0, 5.0]), "behind you to the left");
    }

    #[test]
    fn directions_follow_where_you_face() {
        // se guardi verso +X, "davanti" e' +X e "a destra" e' +Z
        assert_eq!(relative_direction([1.0, 0.0], [5.0, 0.0]), "straight ahead");
        assert_eq!(relative_direction([1.0, 0.0], [0.0, 5.0]), "to your right");
        assert_eq!(relative_direction([1.0, 0.0], [0.0, -5.0]), "to your left");
    }

    #[test]
    fn degenerate_vectors_do_not_panic() {
        assert_eq!(relative_direction([0.0, 0.0], [1.0, 0.0]), "right here");
        assert_eq!(relative_direction(FWD, [0.0, 0.0]), "right here");
    }

    #[test]
    fn description_mentions_distance_direction_and_height() {
        let fwd = Vec3::NEG_Z;
        let d = describe_target("Death certificate", Vec3::new(0.0, 0.0, -38.0), fwd);
        assert_eq!(d, "Death certificate — 38 m, straight ahead");
        let up = describe_target("X", Vec3::new(3.0, 6.0, 0.0), fwd);
        assert!(up.ends_with(", above you"), "{up}");
        let down = describe_target("X", Vec3::new(3.0, -6.0, 0.0), fwd);
        assert!(down.ends_with(", below you"), "{down}");
    }
}
