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
}

/// Componente marker della porta d'uscita dell'interno attualmente in scena:
/// posizione mondiale + normale verso l'esterno (per il check di prossimita').
#[derive(Component)]
pub struct InteriorExitDoor {
    pub world_pos: Vec3,
    pub outward_normal: Vec2,
}

/// Spawna la radice + i figli della mesh interna; ritorna l'entita' radice.
fn spawn_mesh(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    interior_textures: &InteriorTextures,
    mesh: &InteriorMesh,
    root: Transform,
) -> Entity {
    commands
        .spawn((root, InteriorRoot))
        .with_children(|parent| {
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
        })
        .id()
}

/// Sistema: converte `PendingInterior` (dati) in entita' Bevy e attiva la scena.
pub fn spawn_interior(
    mut pending: ResMut<PendingInterior>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    interior_textures: Res<InteriorTextures>,
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
    let root_entity = spawn_mesh(
        &mut commands,
        &mut meshes,
        &mut materials,
        &interior_textures,
        &mesh,
        root_tf,
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
            .init_resource::<PendingInterior>();
    }
}
