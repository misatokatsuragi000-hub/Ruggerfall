//! Vegetazione e alberi 3D.
//!
//! - Sezione 1: alberi **wild** sparsi fuori dalle mura (disposizione originale di Daggerfall).
//! - Nessun filare procedurale: gli alberi in città arrivano dai **flat RMB** (vedi `tiles.rs`),
//!   che vengono renderizzati come cross-billboard 3D usando `NaturePool::tree_for_flat`.
//!
//! ## Orientamento delle texture
//!
//! Le UV del billboard sono configurate in modo che la BASE del mesh (y=0, dove ci sono
//! le radici) mostri la BASE della texture, e la CIMA (y=h, dove c'è la chioma) mostri
//! la CIMA. `is_unity_bottom_up` compensa l'ordine di memoria delle righe:
//!
//!   - `false` (vanilla, PNG, DXT): riga 0 = top dell'immagine → `v_base=0.0`
//!   - `true`  (BC7 Unity):          riga 0 = bottom dell'immagine → `v_base=1.0`
//!
//! **Nota storica**: in passato questa formula era invertita per errore; la versione
//! attuale è quella verificata "a schermo". Se vedi alberi capovolti, il primo posto
//! da guardare è questo `if`.

use crate::mod_textures::ModTextures;
use crate::world::GameData;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use crate::data::terrain_mesh::{terrain_height, CELL_SIZE, TERRAIN_HEIGHT_SCALE};
use crate::data::{archive_file_name, find_file_ci, HeightSamples, TextureFile};
use dfmod::{MapKind, TextureKey};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Scelta archivio / densità in base a clima e stagione
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// RNG deterministico (xorshift)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Cross-billboard mesh (due piani perpendicolari)
// ---------------------------------------------------------------------------

/// Due piani perpendicolari (X-Y e Z-Y) alti `h` e larghi `w`, con la base
/// centrata sull'origine e appoggiata a `y = 0`.
///
/// `is_unity_bottom_up`:
///   * `false` → texture top-down (riga 0 = alto): la base del billboard
///               deve mostrare la base della texture (radici).
///   * `true`  → texture bottom-up (riga 0 = basso, convenzione Unity/BC7):
///               la base del billboard deve mostrare la base della texture
///               (radici), che in memoria è alla riga 0.
///
/// In entrambi i casi, la "base" è la parte del mesh a `y=0`.
pub(crate) fn create_cross_billboard_mesh(w: f32, h: f32, is_unity_bottom_up: bool) -> Mesh {
    let hw = w / 2.0;
    let positions = vec![
        // Piano A (X-Y)
        [-hw, 0.0, 0.0],
        [hw, 0.0, 0.0],
        [hw, h, 0.0],
        [-hw, h, 0.0],
        // Piano B (Z-Y)
        [0.0, 0.0, -hw],
        [0.0, 0.0, hw],
        [0.0, h, hw],
        [0.0, h, -hw],
    ];
    let normals = vec![[0.0, 1.0, 0.0]; 8];

    // V=0 deve cadere sulla riga 0 della texture.
    //   top-down: riga 0 = alto   → v_top = 0.0, v_base = 1.0
    //   bottom-up: riga 0 = basso → v_top = 1.0, v_base = 0.0
    //
    // **Non toccare senza testare**: se le texture della tua mod hanno
    // convenzione opposta, inverti i due rami.
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

// ---------------------------------------------------------------------------
// TreeType + NaturePool
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct TreeType {
    pub mesh: Handle<Mesh>,
    pub material: Handle<StandardMaterial>,
    pub size: f32,
    /// Dimensioni della mesh in metri (per scalare uniformemente quando
    /// piazziamo l'albero in città a dimensioni volute).
    pub w_meters: f32,
    pub h_meters: f32,
}

#[derive(Resource, Default)]
pub struct NaturePool {
    cache: HashMap<u32, Vec<TreeType>>,
    /// Lookup diretto `(archive, record) -> TreeType` per i flat di natura
    /// piazzati dai blocchi RMB delle città.
    by_record: HashMap<(u32, u32), TreeType>,
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
        if !self.cache.contains_key(&archive_id) {
            let species = Self::build_archive(archive_id, data, mods, images, meshes, materials);
            self.cache.insert(archive_id, species);
        }
        if !self.by_record.keys().any(|(a, _)| *a == archive_id) {
            let species = self.cache.get(&archive_id).cloned().unwrap_or_default();
            for (r, tt) in species.into_iter().enumerate() {
                self.by_record.insert((archive_id, r as u32), tt);
            }
        }
        self.cache.get(&archive_id).map(Vec::as_slice).unwrap_or(&[])
    }

    fn build_archive(
        archive_id: u32,
        data: &GameData,
        mods: &mut ModTextures,
        images: &mut Assets<Image>,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
    ) -> Vec<TreeType> {
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

            // Per gli archivi di natura (>= 500) NON si applica la scala del record:
            // gli alberi usano la dimensione nativa dei pixel (scale 256 è un residuo
            // del formato e raddoppierebbe erroneamente tutti gli alberi).
            let (final_w, final_h) = if archive_id >= 500 {
                (w_px as f32, h_px as f32)
            } else {
                let rec = tex_file.record(r).unwrap();
                let scale_x = rec.scale_x as f32 / 256.0;
                let scale_y = rec.scale_y as f32 / 256.0;
                (w_px as f32 * (1.0 + scale_x), h_px as f32 * (1.0 + scale_y))
            };

            // Alberi grandi: 0.15 (era 0.12) per essere più imponenti.
            // Clamp: 6-35 m di altezza, 4-25 m di larghezza.
            let h_meters = (final_h * 0.15).clamp(6.0, 35.0);
            let w_meters = (final_w * 0.15).clamp(4.0, 25.0);

            let key = TextureKey {
                archive: archive_id,
                record: r as u32,
                frame: 0,
                kind: MapKind::Albedo,
            };

            // Il vero discriminante dell'ordine delle righe è `needs_v_flip`
            // (vero per BC7, falso per PNG/vanilla), NON `is_hd`.
            let (image_handle, bottom_up) = if let Some(hd_handle) = mods.image_by_key(&key, images) {
                let flip = mods.needs_v_flip(&hd_handle);
                (hd_handle, flip)
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
                (img, false) // vanilla = top-down
            };

            let mesh_handle = meshes.add(create_cross_billboard_mesh(w_meters, h_meters, bottom_up));
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
                size: h_meters * w_meters,
                w_meters,
                h_meters,
            });
        }
        species.sort_by(|a, b| b.size.partial_cmp(&a.size).unwrap_or(std::cmp::Ordering::Equal));
        species
    }

    pub fn tree_for_flat(&self, archive_id: u32, record: u32) -> Option<TreeType> {
        self.by_record.get(&(archive_id, record)).cloned()
    }
}

// ---------------------------------------------------------------------------
// Spawn alberi per un chunk di terreno
// ---------------------------------------------------------------------------

/// Popola un chunk con alberi **wild** (fuori dal rettangolo urbano).
///
/// La vegetazione **in città** non passa di qui: arriva dai flat RMB e viene
/// renderizzata in `tiles.rs::collect_tiles` tramite `NaturePool::tree_for_flat`.
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
    city_rect: Option<crate::data::terrain_city::CityTileRect>,
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

    commands.entity(chunk_entity).with_children(|parent| {
        // Alberi selvaggi esterni alle mura (disposizione originale di Daggerfall).
        // NIENTE viale procedurale: gli alberi in città arrivano dai flat del RMB.
        let count = tree_count_for_climate(climate);
        for _ in 0..count {
            let gx = rng.gen_f32(2.0, 126.0);
            let gy = rng.gen_f32(2.0, 126.0);
            if let Some(r) = city_rect {
                let m = 1.5;
                if gx >= r.min_x as f32 - m
                    && gx <= r.max_x as f32 + m
                    && gy >= r.min_y as f32 - m
                    && gy <= r.max_y as f32 + m
                    {
                        continue;
                    }
            }
            let y = terrain_height(samples, gx, gy);
            if y <= (27.0 * TERRAIN_HEIGHT_SCALE) {
                continue;
            }
            let slope_x =
            (terrain_height(samples, gx + 0.8, gy) - terrain_height(samples, gx - 0.8, gy)).abs();
            let slope_y =
            (terrain_height(samples, gx, gy + 0.8) - terrain_height(samples, gx - 0.8, gy)).abs();
            if slope_x.max(slope_y) > 7.0 {
                continue;
            }
            let species_idx = rng.gen_range(tree_species.len());
            let tree = &tree_species[species_idx];
            let local_x = gx * CELL_SIZE;
            let local_z = -(gy * CELL_SIZE);
            // Wild scatter: variazione di scala più ampia per non avere
            // un bosco tutto uguale.
            let scale_var = rng.gen_f32(1.0, 1.7);
            parent.spawn((
                Mesh3d(tree.mesh.clone()),
                          MeshMaterial3d(tree.material.clone()),
                          Transform::from_xyz(local_x, y, local_z).with_scale(Vec3::splat(scale_var)),
            ));
        }
    });
}
