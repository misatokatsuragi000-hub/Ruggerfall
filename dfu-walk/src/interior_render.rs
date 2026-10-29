//! Rendering degli interni: la mesh costruita da `daggerfall_data::interior` viene
//! spawnata come figli di una radice quando si entra in un edificio, e despawnata
//! quando si esce.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use daggerfall_data::building_gen::MeshPart;
use daggerfall_data::interior::{InteriorMaterial, InteriorMesh};

/// Dove si trova il giocatore adesso.
#[derive(Resource, PartialEq, Eq, Clone, Copy, Debug, Default)]
pub enum ActiveScene {
    #[default]
    Exterior,
    Interior,
}

/// Componente marker della radice dell'interno attualmente in scena.
#[derive(Component)]
pub struct InteriorRoot;

/// Texture gia' caricate per gli interni: chiave = (archivio Daggerfall, record).
/// Popolata dal caricatore delle texture vanilla al primo ingresso.
#[derive(Resource, Default)]
pub struct InteriorTextures {
    pub images: HashMap<(u16, u8), Handle<Image>>,
}

/// Materiale di base per le superfici interne quando non c'e' una texture dedicata:
/// legno scuro a pavimento, intonaco per le pareti, travi per il soffitto.
fn interior_base_material(kind: InteriorMaterial) -> StandardMaterial {
    let color = match kind {
        InteriorMaterial::Floor => Color::srgb(0.32, 0.22, 0.14),
        InteriorMaterial::Ceiling => Color::srgb(0.25, 0.2, 0.16),
        InteriorMaterial::Wall => Color::srgb(0.62, 0.57, 0.47),
        InteriorMaterial::Door => Color::srgb(0.25, 0.15, 0.08),
        InteriorMaterial::Furniture => Color::srgb(0.45, 0.35, 0.25),
    };
    StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.9,
        reflectance: 0.1,
        double_sided: true,
        cull_mode: None,
        ..default()
    }
}

fn to_bevy_part(part: MeshPart) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, part.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, part.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, part.uvs);
    if part.colors.iter().any(|c| *c != [1.0, 1.0, 1.0, 1.0]) {
        mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, part.colors);
    }
    mesh.insert_indices(Indices::U32(part.indices));
    mesh
}

/// Richiesta di mostrare un interno: compilata da `interior_enter`, consumata da
/// `spawn_interior` nel frame successivo.
#[derive(Resource, Default)]
pub struct PendingInterior {
    pub mesh: InteriorMesh,
    pub root_transform: Transform,
    /// true quando non c'e' nessuna richiesta in sospeso (stato iniziale/fresco)
    pub consumed: bool,
    /// se generare anche gli NPC custom della mesh: lo decide `interior_enter`
    /// in base alla casa degli NPC (`npc_home`). Mai `true` fuori dalla casa.
    pub spawn_npcs: bool,
}

/// Componente marker della porta d'uscita dell'interno attualmente in scena:
/// posizione mondiale + normale verso l'esterno (per il check di prossimita').
#[derive(Component)]
pub struct InteriorExitDoor {
    pub world_pos: Vec3,
    pub outward_normal: Vec2,
}

/// Spawna la radice + i figli della mesh interna; ritorna l'entita' radice.
/// Se `spawn_npcs`, genera anche gli NPC custom come ulteriori figli della
/// radice (marcati `CustomNpc` per il reclutamento con R).
#[allow(clippy::too_many_arguments)]
fn spawn_mesh(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    interior_textures: &InteriorTextures,
    npc_materials: &mut NpcMaterials,
    mesh: &InteriorMesh,
    root: Transform,
    spawn_npcs: bool,
    recruited: &crate::npc_home::RecruitedNpcs,
    building: &Option<crate::interior_enter::BuildingKey>,
) -> Entity {
    let root_entity = commands
        .spawn((root, InteriorRoot))
        .id();
    commands.entity(root_entity).with_children(|parent| {
        for kind in InteriorMaterial::ALL {
            let part = &mesh.parts[kind.index()];
            if part.is_empty() {
                continue;
            }
            let material = materials.add(interior_base_material(kind));
            parent.spawn((
                Mesh3d(meshes.add(to_bevy_part(part.clone()))),
                MeshMaterial3d(material),
                Transform::IDENTITY,
            ));
        }
        for ((archive, record), part) in &mesh.textured {
            if part.is_empty() {
                continue;
            }
            let handle = interior_textures.images.get(&(*archive, *record)).cloned();
            let material = match handle {
                Some(tex) => StandardMaterial {
                    base_color_texture: Some(tex),
                    base_color: Color::srgb(1.0, 1.0, 1.0),
                    perceptual_roughness: 0.9,
                    reflectance: 0.1,
                    double_sided: true,
                    cull_mode: None,
                    alpha_mode: AlphaMode::Blend,
                    ..default()
                },
                None => interior_base_material(InteriorMaterial::Furniture),
            };
            let material = materials.add(material);
            parent.spawn((
                Mesh3d(meshes.add(to_bevy_part(part.clone()))),
                MeshMaterial3d(material),
                Transform::IDENTITY,
            ));
        }
        // ---- NPC custom: SOLO se autorizzati (casa degli NPC) --------------
        if spawn_npcs {
            for (i, person) in mesh.people.iter().enumerate() {
                // non rigenerare chi e' gia' stato reclutato
                if let Some(b) = building {
                    if recruited.is_recruited(b, i) {
                        continue;
                    }
                }
                let material = npc_materials.material(person.texture_archive, person.texture_record, materials);
                // persona in piedi: capsula alta ~1.8 m, meta' sotto gli occhi
                let y = person.position[1] + 0.9 - crate::player::EYE_HEIGHT * 0.0;
                parent.spawn((
                    Mesh3d(meshes.add(npc_capsule())),
                    MeshMaterial3d(material),
                    CustomNpc { index: i },
                    Transform::from_xyz(person.position[0], y, person.position[2])
                        .with_rotation(Quat::from_rotation_y(-person.direction_deg.to_radians())),
                ));
            }
        }
    });
    root_entity
}

/// Componente marker degli NPC custom in scena (figli della radice).
pub use crate::npc_home::CustomNpc;

/// Colori deterministici per gli NPC: uno per ogni (archivio, record) dichiarato
/// nel RMB, cosi' lo stesso PNGD ha sempre lo stesso "vestito". La cache viene
/// riempita da `spawn_mesh` quando servono materiali nuovi.
#[derive(Resource, Default)]
pub struct NpcMaterials {
    cache: HashMap<(u16, u8), Handle<StandardMaterial>>,
}

impl NpcMaterials {
    /// Materiale per l'NPC con la texture `(archive, record)`; crea l'asset se
    /// non e' ancora in cache.
    fn material(
        &mut self,
        archive: u16,
        record: u8,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        if let Some(h) = self.cache.get(&(archive, record)) {
            return h.clone();
        }
        // tonalita' "veste", variate con un hash semplice dei due indici
        let hue = (archive as f32 * 7.0 + record as f32 * 23.0).rem_euclid(360.0);
        let color = Color::hsl(hue / 360.0, 0.45, 0.42);
        let handle = materials.add(StandardMaterial {
            base_color: color,
            perceptual_roughness: 0.8,
            ..default()
        });
        self.cache.insert((archive, record), handle.clone());
        handle
    }
}

/// Geometria dell'NPC: capsula alta ~1.8 m (0.55 semi-altezza cilindro + raggi).
fn npc_capsule() -> Mesh {
    use bevy::render::primitives::Capsule3d as RenderCapsule;
    Mesh::from(RenderCapsule::new(0.35, 0.55))
}

/// Sistema: converte `PendingInterior` (dati) in entita' Bevy e attiva la scena.
#[allow(clippy::too_many_arguments)]
pub fn spawn_interior(
    mut pending: ResMut<PendingInterior>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    interior_textures: Res<InteriorTextures>,
    mut npc_materials: ResMut<NpcMaterials>,
    recruited: Res<crate::npc_home::RecruitedNpcs>,
    current: Res<crate::interior_enter::CurrentInterior>,
    mut scene: ResMut<ActiveScene>,
    existing: Query<Entity, With<InteriorRoot>>,
) {
    if pending.consumed {
        return;
    }
    // se eravamo gia' dentro un altro interno, butta via tutto
    for e in existing.iter() {
        commands.entity(e).despawn_recursive();
    }
    let mesh = std::mem::take(&mut pending.mesh);
    pending.consumed = true;
    if mesh.is_empty() {
        return;
    }
    let root_tf = pending.root_transform;
    let spawn_npcs = pending.spawn_npcs;
    let root_entity = spawn_mesh(
        &mut commands,
        &mut meshes,
        &mut materials,
        &interior_textures,
        &mut npc_materials,
        &mesh,
        root_tf,
        spawn_npcs,
        &recruited,
        &current.building,
    );
    // marca la porta d'uscita (= ingresso dell'edificio): serve a
    // exit_interior_system per il check di prossimita'
    if let Some(d) = mesh.doors.first() {
        let pos3 = Vec3::new(d.position[0], d.position[1], d.position[2]);
        let world_pos = root_tf.transform_point3(pos3);
        let n = Vec2::new(d.normal[0], d.normal[1]).normalize_or_zero();
        let outward = root_tf.rotation * Vec3::new(n.x, 0.0, n.y);
        let outward = Vec2::new(outward.x, outward.z).normalize_or_zero();
        commands
            .entity(root_entity)
            .insert(InteriorExitDoor { world_pos, outward_normal: outward });
    }
    *scene = ActiveScene::Interior;
}

/// Sistema: all'uscita despawna la radice dell'interno.
pub fn despawn_interior(
    scene: Res<ActiveScene>,
    mut commands: Commands,
    roots: Query<Entity, With<InteriorRoot>>,
    mut pending: ResMut<PendingInterior>,
) {
    if *scene != ActiveScene::Exterior {
        return;
    }
    if !pending.consumed && pending.mesh.is_empty() {
        return;
    }
    let mut any = false;
    for e in roots.iter() {
        commands.entity(e).despawn_recursive();
        any = true;
    }
    if any {
        pending.mesh = InteriorMesh::default();
        pending.consumed = true;
    }
}

pub struct InteriorRenderPlugin;

impl Plugin for InteriorRenderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveScene>()
            .init_resource::<InteriorTextures>()
            .init_resource::<NpcMaterials>()
            .init_resource::<PendingInterior>();
    }
}
