//! //! Caricamento e streaming dei map pixel con alberi 3D, case e shader WGSL.

use crate::mod_textures::ModTextures;
use crate::player::Player;
use crate::terrain_material::{TerrainExtension, TerrainMaterial};
use crate::trees::{spawn_trees_for_chunk, NaturePool};
use crate::wall_textures::WallTextures;
use crate::world::*;
use bevy::pbr::ExtendedMaterial;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{block_on, futures_lite::future, AsyncComputeTaskPool, Task};
use crate::data::bsa::BsaArchive;
use crate::data::building_gen::{MaterialKind, MeshPart, MATERIAL_COUNT};
use crate::data::city_buildings::build_city_buildings;
use crate::data::terrain_city::{assemble_city_tilemap, flatten_terrain_heights, CityTileRect};
use crate::data::terrain_mesh::{build_terrain_mesh, terrain_height, TerrainMeshData, CELL_SIZE};
use crate::data::{
    encode_for_shader, generate_default_samples, generate_default_tilemap,
    ground_archive, HeightSamples,
};
use std::collections::HashMap;
use std::sync::Arc;

const MAX_PENDING: usize = 4;
const MAX_SPAWNS_PER_FRAME: usize = 2;
const DEFAULT_CLIMATE: i32 = 231;

#[derive(Component)]
pub struct TerrainTile {
    pub mx: i32,
    pub my: i32,
}

pub struct TileResult {
    archive: i32,
    mesh: TerrainMeshData,
    encoded_tilemap: Vec<u8>,
    samples: Arc<HeightSamples>,
    city_parts: Option<[MeshPart; MATERIAL_COUNT]>,
    city_textured: Vec<((u16, u8), MeshPart)>,
    city_rect: Option<CityTileRect>,
}

#[derive(Resource)]
pub struct Streaming {
    pub radius: i32,
    pub tile_px: (usize, usize),
    loaded: HashMap<(i32, i32), Entity>,
    pending: HashMap<(i32, i32), Task<Option<TileResult>>>,
}

impl Streaming {
    pub fn new(radius: i32, tile_px: (usize, usize)) -> Self {
        Self {
            radius,
            tile_px,
            loaded: HashMap::new(),
            pending: HashMap::new(),
        }
    }

    pub fn loaded_count(&self) -> usize {
        self.loaded.len()
    }
}

pub fn climate_of(data: &GameData, mx: i32, my: i32) -> i32 {
    data.climate
    .map_pixel_value(mx, my)
    .map(i32::from)
    .unwrap_or(DEFAULT_CLIMATE)
}

fn to_bevy_part(part: MeshPart) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, part.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, part.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, part.uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, part.colors);
    mesh.insert_indices(Indices::U32(part.indices));
    mesh
}

fn city_material(kind: MaterialKind) -> StandardMaterial {
    // il colore vero arriva dai colori per vertice; qui solo le proprietà fisiche
    let (roughness, double_sided) = match kind {
        MaterialKind::Roof => (0.9, true), // il tetto non ha il lato inferiore
        MaterialKind::Opening => (0.4, false),
        _ => (0.9, false),
    };
    StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: roughness,
        reflectance: 0.1,
        double_sided,
        cull_mode: if double_sided { None } else { Some(bevy::render::render_resource::Face::Back) },
        ..default()
    }
}

fn build_tile(data: &GameData, tile_px: (usize, usize), mx: i32, my: i32) -> Option<TileResult> {
    let mut samples = generate_default_samples(&data.woods, mx, my).ok()?;

    let mut city_parts = None;
    let mut city_textured = Vec::new();
    let mut city_rect_opt = None;
    let mut tile_preset = None;

    if let Some(loc) = data.locations.get(&(mx, my)) {
        // i BSA si parsano solo per i pixel che contengono una località
        let blocks_bsa = BsaArchive::parse(&data.blocks_buf).ok()?;
        let arch3d_bsa = BsaArchive::parse(&data.arch3d_buf).ok()?;

        let (preset, city_rect) = assemble_city_tilemap(loc, &blocks_bsa);
        city_rect_opt = Some(city_rect);
        tile_preset = Some(preset);

        flatten_terrain_heights(&mut samples.data, samples.dim, &city_rect, 12.0);

        // quota del suolo in metri, come la mesh del terreno (x est, z = -nord)
        let ground = |x: f32, z: f32| terrain_height(&samples, x / CELL_SIZE, -z / CELL_SIZE);
        let (builder, stats) = build_city_buildings(loc, &blocks_bsa, &arch3d_bsa, &ground);
        info!(
            "{}: {} oggetti, {} edifici, {} scartati, {} modelli mancanti",
            loc.name, stats.objects, stats.built, stats.skipped, stats.missing_models
        );
        if !builder.is_empty() {
            let (parts, textured) = builder.into_all();
            city_parts = Some(parts);
            city_textured = textured;
        }
    }

    let tilemap = generate_default_tilemap(&samples, mx, my, tile_preset.as_deref()).ok()?;
    let encoded = encode_for_shader(&tilemap, true);
    let mesh = build_terrain_mesh(&samples, &encoded, tile_px);
    let archive = ground_archive(climate_of(data, mx, my), data.winter);

    Some(TileResult {
        archive,
         mesh,
         encoded_tilemap: encoded,
         samples: Arc::new(samples),
         city_parts,
         city_textured,
         city_rect: city_rect_opt,
    })
}

fn to_bevy_mesh(d: TerrainMeshData) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);

    let mut min_x = f32::MAX;
    let mut max_x = f32::MIN;
    let mut min_z = f32::MAX;
    let mut max_z = f32::MIN;
    for p in &d.positions {
        min_x = min_x.min(p[0]);
        max_x = max_x.max(p[0]);
        min_z = min_z.min(p[2]);
        max_z = max_z.max(p[2]);
    }
    let range_x = (max_x - min_x).max(1.0);
    let range_z = (max_z - min_z).max(1.0);

    let normalized_uvs: Vec<[f32; 2]> = d.positions
    .iter()
    .map(|p| [(p[0] - min_x) / range_x, (p[2] - min_z) / range_z])
    .collect();

    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, d.positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, d.normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, normalized_uvs);
    mesh.insert_indices(Indices::U32(d.indices));
    mesh
}

pub fn rebase_origin(
    mut origin: ResMut<Origin>,
    mut player: Query<&mut Transform, (With<Player>, Without<TerrainTile>)>,
                     mut tiles: Query<(&TerrainTile, &mut Transform), Without<Player>>,
) {
    let Ok(mut pt) = player.get_single_mut() else { return };
    let (east, north) = absolute(&origin, pt.translation);
    let side = crate::data::terrain_mesh::MAP_PIXEL_SIZE as f64;
    let (pe, pn) = ((east / side).floor() as i32, (north / side).floor() as i32);
    if (pe - origin.east).abs() < 2 && (pn - origin.north).abs() < 2 {
        return;
    }
    let (de, dn) = (pe - origin.east, pn - origin.north);
    origin.east = pe;
    origin.north = pn;
    pt.translation.x -= de as f32 * crate::data::terrain_mesh::MAP_PIXEL_SIZE;
    pt.translation.z += dn as f32 * crate::data::terrain_mesh::MAP_PIXEL_SIZE;
    for (tile, mut tf) in &mut tiles {
        tf.translation = tile_translation(&origin, tile.mx, tile.my);
    }
}

pub fn stream_tiles(
    mut commands: Commands,
    mut streaming: ResMut<Streaming>,
    mut field: ResMut<HeightField>,
    data: Res<GameData>,
    origin: Res<Origin>,
    player: Query<&Transform, With<Player>>,
) {
    let Ok(tf) = player.get_single() else { return };
    let (pmx, pmy) = map_pixel_of(&origin, tf.translation);
    let r = streaming.radius;
    let keep = r + 1;
    let near = |(mx, my): (i32, i32), d: i32| (mx - pmx).abs() <= d && (my - pmy).abs() <= d;

    let far: Vec<_> = streaming.loaded.keys().filter(|k| !near(**k, keep)).copied().collect();
    for key in far {
        if let Some(e) = streaming.loaded.remove(&key) {
            commands.entity(e).despawn_recursive();
        }
        field.tiles.remove(&key);
    }
    streaming.pending.retain(|k, _| near(*k, keep));

    let mut wanted = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            let key = (pmx + dx, pmy + dy);
            if in_map(key.0, key.1) && !streaming.loaded.contains_key(&key) && !streaming.pending.contains_key(&key) {
                wanted.push(key);
            }
        }
    }
    wanted.sort_by_key(|(mx, my)| (mx - pmx).pow(2) + (my - pmy).pow(2));

    let pool = AsyncComputeTaskPool::get();
    for key in wanted {
        if streaming.pending.len() >= MAX_PENDING {
            break;
        }
        let (data, tile_px) = (data.clone(), streaming.tile_px);
        let task = pool.spawn(async move { build_tile(&data, tile_px, key.0, key.1) });
        streaming.pending.insert(key, task);
    }
}

pub fn collect_tiles(
    mut commands: Commands,
    mut streaming: ResMut<Streaming>,
    mut field: ResMut<HeightField>,
    data: Res<GameData>,
    origin: Res<Origin>,
    mut mods: ResMut<ModTextures>,
    mut nature_pool: ResMut<NaturePool>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut terrain_materials: ResMut<Assets<TerrainMaterial>>,
    mut standard_materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut wall_textures: ResMut<WallTextures>,
) {
    let finished: Vec<_> = streaming
    .pending
    .iter_mut()
    .filter_map(|(key, task)| block_on(future::poll_once(task)).map(|result| (*key, result)))
    .take(MAX_SPAWNS_PER_FRAME)
    .collect();

    for (key, result) in finished {
        streaming.pending.remove(&key);
        let Some(tile) = result else { continue };

        let tile_array_handle = match mods.terrain_texture_array(tile.archive as u32, &mut images) {
            Some(h) => h,
            None => {
                warn!("Array terreno {} non trovato nelle mod", tile.archive);
                continue;
            }
        };

        let tilemap_image = Image::new(
            Extent3d {
                width: 128,
                height: 128,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            tile.encoded_tilemap,
            TextureFormat::R8Unorm,
            RenderAssetUsages::RENDER_WORLD,
        );
        let tilemap_handle = images.add(tilemap_image);

        let material = terrain_materials.add(ExtendedMaterial {
            base: StandardMaterial {
                perceptual_roughness: 1.0,
                reflectance: 0.1,
                ..default()
            },
            extension: TerrainExtension {
                tile_array: tile_array_handle,
                tilemap: tilemap_handle,
            },
        });

        let chunk_entity = commands
        .spawn((
            Mesh3d(meshes.add(to_bevy_mesh(tile.mesh))),
                MeshMaterial3d(material),
                Transform::from_translation(tile_translation(&origin, key.0, key.1)),
                TerrainTile { mx: key.0, my: key.1 },
        ))
        .id();

        // edifici: figli del chunk, così seguono rebase_origin e vengono scaricati con lui
        if let Some(parts) = tile.city_parts {
            commands.entity(chunk_entity).with_children(|parent| {
                for (kind, part) in MaterialKind::ALL.iter().zip(parts) {
                    if part.is_empty() {
                        continue;
                    }
                    let material = standard_materials.add(city_material(*kind));
                    parent.spawn((
                        Mesh3d(meshes.add(to_bevy_part(part))),
                        MeshMaterial3d(material),
                        Transform::IDENTITY,
                    ));
                }
            });
        }

        // parti con le texture originali di Daggerfall (mura): un materiale per texture
        let textured: Vec<_> = tile
            .city_textured
            .into_iter()
            .map(|((archive, record), part)| {
                let material = wall_textures.material(
                    archive,
                    record,
                    climate_of(&data, key.0, key.1),
                    &data,
                    &mut mods,
                    &mut images,
                    &mut standard_materials,
                );
                (material, part)
            })
            .collect();
        if !textured.is_empty() {
            commands.entity(chunk_entity).with_children(|parent| {
                for (material, part) in textured {
                    parent.spawn((
                        Mesh3d(meshes.add(to_bevy_part(part))),
                        MeshMaterial3d(material),
                        Transform::IDENTITY,
                    ));
                }
            });
        }

        spawn_trees_for_chunk(
            &mut commands,
            chunk_entity,
            key.0,
            key.1,
            climate_of(&data, key.0, key.1),
                              data.winter,
                              &tile.samples,
                              &data,
                              &mut mods,
                              &mut nature_pool,
                              &mut images,
                              &mut meshes,
                              &mut standard_materials,
                              tile.city_rect,
        );

        streaming.loaded.insert(key, chunk_entity);
        field.tiles.insert(key, tile.samples);
    }
}
