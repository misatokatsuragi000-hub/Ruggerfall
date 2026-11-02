//! NPC delle citta': entita' fisiche ESTERNE generate dalle liste "person" degli
//! interni (vedi `daggerfall_data::interior_people`). Un NPC senza sprite per
//! camminare viene rappresentato come un semplice omino 3D (corpo + testa); la
//! sua posizione nel mondo esterno e' dedotta dalla stanza in cui Daggerfall lo
//! piazza, quindi di fatto "esce dall'interno".
//!
//! Reclutamento: avvicinandosi a un NPC libero l'omino si illumina; premendo
//! R diventa compagno (`Recruited`) e segue il giocatore. Gli NPC reclutati
//! sopravvivono allo scaricamento dei tile (sono radicati nella scena, non nei
//! chunk) e restano accanto al giocatore finche' non li si licenzia tenendo
//! premuto R in mezzo ai compagni.

use crate::player::Player;
use crate::world::{HeightField, Origin};
use bevy::prelude::*;

/// Raggio (metri) entro cui un NPC libero puo' essere reclutato.
pub const RECRUIT_RADIUS: f32 = 4.0;
/// Distanza (metri) a cui il compagno si piazza dietro il giocatore.
pub const FOLLOW_DISTANCE: f32 = 2.2;
/// Oltre questa distanza dal giocatore l'NPC viene teletrasportato (per non
/// restare indietro durante il rebase dell'origine o nei voli).
pub const TELEPORT_DISTANCE: f32 = 60.0;
/// Massimo numero di compagni contemporanei.
pub const MAX_RECRUITS: usize = 6;

/// NPC libero, generato da un interno: puo' essere reclutato con R.
#[derive(Component)]
pub struct RecruitableNpc {
    /// nome del blocco RMB di provenienza (diagnostica)
    pub block_name: String,
    /// orientamento iniziale in radianti
    pub facing: f32,
}

/// Marker degli NPC gia' reclutati che seguono il giocatore.
#[derive(Component)]
pub struct Recruited;

/// Contatore dei compagni in scena (per il titolo della finestra).
#[derive(Resource, Default)]
pub struct RecruitCount(pub usize);

/// Colori di base variati per texture_record, cosi' gli NPC di interni diversi
/// si distinguono visivamente (l'archivio/record person di Daggerfall indica
/// volto/vestiti, che qui diventano solo una tinta).
fn npc_color(texture_record: u8) -> Color {
    let palette = [
        Color::srgb(0.75, 0.62, 0.45), // tunica chiara
        Color::srgb(0.45, 0.30, 0.22), // cuoio
        Color::srgb(0.30, 0.35, 0.55), // blu nobile
        Color::srgb(0.55, 0.20, 0.20), // rosso
        Color::srgb(0.25, 0.45, 0.28), // verde
        Color::srgb(0.50, 0.50, 0.52), // grigio armatura
        Color::srgb(0.20, 0.20, 0.24), // nero
        Color::srgb(0.80, 0.75, 0.60), // bianco avorio
    ];
    palette[texture_record as usize % palette.len()]
}

/// Spawna l'omino (corpo cilindrico + testa sferica) alla posizione data.
/// L'entita' radice non ha mesh: corpo e testa sono figli, cosi' ruotano con
/// l'NPC; la tinta del corpo cambia quando viene reclutato.
pub fn spawn_npc(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    pos: Vec3,
    facing: f32,
    texture_record: u8,
    block_name: String,
) {
    let body = meshes.add(Cylinder::new(0.35, 1.1));
    let head = meshes.add(Sphere::new(0.22));
    let color = npc_color(texture_record);
    let body_mat = materials.add(StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.9,
        ..default()
    });
    let head_mat = materials.add(StandardMaterial {
        base_color: Color::srgb(0.82, 0.66, 0.52),
        perceptual_roughness: 0.8,
        ..default()
    });

    commands.spawn((
        Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(-facing)),
        RecruitableNpc { block_name, facing },
        NpcBody(body_mat),
    ));
    // corpo e testa come figli: ruotano con l'NPC
    if let Some(root) = commands.entities().last().copied() {
        commands.entity(root).with_children(|c| {
            c.spawn((
                Mesh3d(body),
                MeshMaterial3d(body_mat),
                Transform::from_xyz(0.0, 0.55, 0.0),
            ));
            c.spawn((
                Mesh3d(head),
                MeshMaterial3d(head_mat),
                Transform::from_xyz(0.0, 1.35, 0.0),
            ));
        });
    }
}

/// Materiale del corpo, ricolorato all'atto del reclutamento/licenziamento.
#[derive(Component)]
struct NpcBody(Handle<StandardMaterial>);

/// Sistema: gestisce il reclutamento col tasto R e aggiorna le posizioni dei
/// compagni che seguono il giocatore.
pub fn recruit_and_follow(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    player: Query<&Transform, With<Player>>,
    origin: Res<Origin>,
    field: Res<HeightField>,
    mut npcs: Query<(Entity, &mut Transform, &mut NpcBody, Option<&Recruited>)>,
    mut count: ResMut<RecruitCount>,
    mut mats: ResMut<Assets<StandardMaterial>>,
) {
    let Ok(ptf) = player.get_single() else { return };

    // conta i compagni attuali
    let recruits = npcs.iter().filter(|(_, _, _, r)| r.is_some()).count();
    count.0 = recruits;

    // NPC libero piu' vicino entro RECRUIT_RADIUS (player e NPC stanno nella
    // stessa scena relativa all'origine, niente conversioni necessarie)
    let mut nearest: Option<(Entity, f32)> = None;
    for (e, tf, _, recruited) in npcs.iter() {
        if recruited.is_some() {
            continue;
        }
        let d = tf.translation.distance(ptf.translation);
        if d <= RECRUIT_RADIUS && nearest.map_or(true, |(_, bd)| d < bd) {
            nearest = Some((e, d));
        }
    }

    if keys.just_pressed(KeyCode::KeyR) {
        if let Some((e, _)) = nearest {
            if count.0 < MAX_RECRUITS {
                commands.entity(e).insert(Recruited);
                count.0 += 1;
            }
        } else if count.0 > 0 {
            // nessun NPC libero vicino: licenzia il compagno piu' lontano
            let mut farthest: Option<(f32, Entity)> = None;
            for (e, tf, _, recruited) in npcs.iter() {
                if recruited.is_none() {
                    continue;
                }
                let d = tf.translation.distance(ptf.translation);
                if farthest.map_or(true, |(fd, _)| d > fd) {
                    farthest = Some((d, e));
                }
            }
            if let Some((_, e)) = farthest {
                commands.entity(e).remove::<Recruited>();
                count.0 = count.0.saturating_sub(1);
            }
        }
    }

    // movimento dei compagni: mantieni FOLLOW_DISTANCE dietro il giocatore
    let forward = ptf.rotation * Vec3::new(0.0, 0.0, -1.0);
    for (e, mut tf, body, recruited) in npcs.iter_mut() {
        let _ = e;
        let is_recruited = recruited.is_some();
        // evidenza visiva: oro se compagno, leggero glow ambra se reclutabile
        let near = nearest.map_or(false, |(ne, _)| ne == e);
        let target_color = if is_recruited {
            Color::srgb(0.85, 0.70, 0.25)
        } else if near {
            Color::srgb(0.95, 0.85, 0.40)
        } else {
            Color::WHITE
        };
        if let Some(mat) = mats.get_mut(&body.0) {
            if mat.base_color != target_color {
                mat.base_color = target_color;
            }
        }
        if !is_recruited {
            // gli NPC liberi restano dove li ha piazzati l'interno: niente moto
            // (nessuno sprite per camminare)
            continue;
        }
        let to_player = ptf.translation - tf.translation;
        let flat = Vec2::new(to_player.x, to_player.z);
        let dist = flat.length();
        if to_player.length() > TELEPORT_DISTANCE {
            // ricongiungimento istantaneo (rebase origine, voli, caricamenti)
            let back = forward.normalize_or_zero() * FOLLOW_DISTANCE;
            tf.translation = ptf.translation - Vec3::new(back.x, 0.0, back.z);
        } else if dist > FOLLOW_DISTANCE {
            let dir = Vec3::new(flat.x, 0.0, flat.y) / dist.max(1e-5);
            let step = (dist - FOLLOW_DISTANCE).min(9.0);
            tf.translation += dir * step;
        }
        // incolla i piedi al suolo
        if let Some(g) = field.ground(&origin, tf.translation.x, tf.translation.z) {
            tf.translation.y = g;
        }
        // rivolto verso il giocatore
        let yaw = (-to_player.x).atan2(-to_player.z);
        tf.rotation = Quat::from_rotation_y(yaw);
    }
}
