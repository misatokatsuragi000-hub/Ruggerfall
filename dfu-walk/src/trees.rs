//! Vegetazione e alberi 3D procedurali con supporto per texture HD delle mod.

use crate::mod_textures::ModTextures;
use crate::world::GameData;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use daggerfall_data::terrain_mesh::{terrain_height, CELL_SIZE, TERRAIN_HEIGHT_SCALE};
use daggerfall_data::{archive_file_name, find_file_ci, HeightSamples, TextureFile};
use dfmod::{MapKind, TextureKey};
use std::collections::HashMap;

pub fn nature_archive_for_climate(climate: i32, is_winter: bool) -> u32 {
    if is_winter {
        match climate {
            226 => 509,
            230 => 508,
            231 | 232 => 508,
            _ => 500,
        }
    } else {
        match climate {
            224 | 225 => 500,
            226 => 501,
            227 => 502,
            228 => 503,
            229 => 504,
            230 => 505,
            231 => 506,
            232 => 507,
            _ => 506,
        }
    }
}

fn tree_count_for_climate(climate: i32) -> usize {
    match climate {
        223 => 0,
        224 | 225 => 30,
        226 => 55,
        227 => 220,
        228 => 180,
        229 => 140,
        230 => 160,
        231 | 232 => 200,
        _ => 120,
    }
}

struct SimpleRng(u64);
impl SimpleRng {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 { 0x517cc1b727220a95 } else { seed })
    }
    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn gen_f32(&mut self, min: f32, max: f32) -> f32 {
        let t = (self.next_u64() & 0xFFFFFF) as f32 / 16777215.0;
        min + t * (max - min)
    }
    fn gen_range(&mut self, max: usize) -> usize {
        if max == 0 { 0 } else { (self.next_u64() % (max as u64)) as usize }
    }
}

/// Mesh a croce: orienta le coordinate V in base all'origine della texture.
fn create_cross_billboard_mesh(w: f32, h: f32, is_unity_bottom_up: bool) -> Mesh {
    let hw = w / 2.0;

    let positions = vec![
        [-hw, 0.0, 0.0],
        [hw, 0.0, 0.0],
        [hw, h, 0.0],
        [-hw, h, 0.0],
        [0.0, 0.0, -hw],
        [0.0, 0.0, hw],
        [0.0, h, hw],
        [0.0, h, -hw],
    ];

    let normals = vec![[0.0, 1.0, 0.0]; 8];

    // Se è una texture HD di Unity (DREAM) il tronco è a V=0; se è vanilla è a V=1
    let (v_base, v_top) = if is_unity_bottom_up { (0.0, 1.0) } else { (1.0, 0.0) };

    let uvs = vec![
        [0.0, v_base], [1.0, v_base], [1.0, v_top], [0.0, v_top],
        [0.0, v_base], [1.0, v_base], [1.0, v_top], [0.0, v_top],
    ];

    let indices = vec![
        0, 1, 2, 0, 2, 3,
        4, 5, 6, 4, 6, 7,
    ];

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

#[derive(Clone)]
pub struct TreeType {
    pub mesh: Handle<Mesh>,
    pub material: Handle<StandardMaterial>,
}

#[derive(Resource, Default)]
pub struct NaturePool {
    cache: HashMap<u32, Vec<TreeType>>,
}

impl NaturePool {
    pub fn get_or_load(
        &mut self,
        archive_id: u32,
        data: &GameData,
        mods: &mut ModTextures,
        images: &mut Assets<Image>,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
    ) -> &[TreeType] {
        self.cache.entry(archive_id).or_insert_with(|| {
            let filename = archive_file_name(archive_id);
            let Some(path) = find_file_ci(&data.dir, &filename) else {
                warn!("Archivio vegetazione `{filename}` non trovato in ARENA2");
                return Vec::new();
            };
            let Ok(tex_file) = TextureFile::open(&path) else {
                warn!("Impossibile aprire `{filename}`");
                return Vec::new();
            };

            let mut species = Vec::new();
            for r in 0..tex_file.record_count() {
                let Ok(frame) = tex_file.frame(r, 0) else { continue };
                let (w_px, h_px) = (frame.width, frame.height);
                if w_px == 0 || h_px == 0 {
                    continue;
                }

                // Dimensioni imponenti: alberi da 6m a 20m di altezza
                let h_meters = (h_px as f32 * 0.08).clamp(3.0, 22.0);
                let w_meters = (w_px as f32 * 0.08).clamp(2.0, 18.0);

                let key = TextureKey {
                    archive: archive_id,
                    record: r as u32,
                    frame: 0,
                    kind: MapKind::Albedo,
                };

                // Controlla se DREAM ha la versione HD BC7
                let (image_handle, is_hd) = if let Some(hd_handle) = mods.image_by_key(&key, images) {
                    (hd_handle, true)
                } else {
                    let rgba = frame.to_rgba(&data.palette, Some(0));
                    let img = images.add(Image::new(
                        Extent3d {
                            width: w_px as u32,
                            height: h_px as u32,
                            depth_or_array_layers: 1,
                        },
                        TextureDimension::D2,
                        rgba,
                        TextureFormat::Rgba8UnormSrgb,
                        RenderAssetUsages::default(),
                    ));
                    (img, false)
                };

                let mesh_handle = meshes.add(create_cross_billboard_mesh(w_meters, h_meters, is_hd));
                let mat_handle = materials.add(StandardMaterial {
                    base_color_texture: Some(image_handle),
                    alpha_mode: AlphaMode::Mask(0.5),
                    perceptual_roughness: 0.85,
                    reflectance: 0.1,
                    cull_mode: None,
                    ..default()
                });

                species.push(TreeType {
                    mesh: mesh_handle,
                    material: mat_handle,
                });
            }
            species
        })
    }
}

pub fn spawn_trees_for_chunk(
    commands: &mut Commands,
    chunk_entity: Entity,
    mx: i32,
    my: i32,
    climate: i32,
    winter: bool,
    samples: &HeightSamples,
    data: &GameData,
    mods: &mut ModTextures,
    pool: &mut NaturePool,
    images: &mut Assets<Image>,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    if climate == 223 {
        return;
    }

    let archive = nature_archive_for_climate(climate, winter);
    let tree_species = pool.get_or_load(archive, data, mods, images, meshes, materials);
    if tree_species.is_empty() {
        return;
    }

    let seed = ((mx as u64) << 32) ^ (my as u64) ^ 0x9e3779b97f4a7c15;
    let mut rng = SimpleRng::new(seed);
    let count = tree_count_for_climate(climate);

    commands.entity(chunk_entity).with_children(|parent| {
        for _ in 0..count {
            let gx = rng.gen_f32(2.0, 126.0);
            let gy = rng.gen_f32(2.0, 126.0);
            let y = terrain_height(samples, gx, gy);

            if y <= (27.0 * TERRAIN_HEIGHT_SCALE) {
                continue;
            }

            let slope_x = (terrain_height(samples, gx + 0.8, gy) - terrain_height(samples, gx - 0.8, gy)).abs();
            let slope_y = (terrain_height(samples, gx, gy + 0.8) - terrain_height(samples, gx, gy - 0.8)).abs();
            if slope_x.max(slope_y) > 7.0 {
                continue;
            }

            let species_idx = rng.gen_range(tree_species.len());
            let tree = &tree_species[species_idx];

            let local_x = gx * CELL_SIZE;
            let local_z = -(gy * CELL_SIZE);
            let scale_var = rng.gen_f32(0.9, 1.3);

            parent.spawn((
                Mesh3d(tree.mesh.clone()),
                MeshMaterial3d(tree.material.clone()),
                Transform::from_xyz(local_x, y, local_z).with_scale(Vec3::splat(scale_var)),
            ));
        }
    });
}
