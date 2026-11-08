//! Vegetazione e alberi 3D.
//!
//! - Sezione 1: alberi **wild** sparsi fuori dalle mura (disposizione originale di Daggerfall).
//! - Nessun filare procedurale: gli alberi in città arrivano dai **flat RMB** (vedi `tiles.rs`),
//!   che vengono renderizzati come cross-billboard 3D usando `NaturePool::tree_for_flat`.
//!
//! ## Stagione: mappatura ufficiale DFU
//!
//! Fonte: Interkarma (autore di Daggerfall Unity), forum DFWorkshop.
//!
//! ```text
//! Clima                    Summer  Winter
//! Rainforest               500  →  500   (nessuna neve)
//! Sub-tropical             501  →  501   (nessuna neve)
//! Swamp                    502  →  502   (nessuna neve)
//! Desert                   503  →  503   (nessuna neve)
//! Temperate Woodland       504  →  505
//! Woodland Hills           506  →  507
//! Haunted Woodlands        508  →  509
//! Mountains                510  →  511
//! ```
//!
//! Regola: **la variante invernale è `base + 1` solo per 504, 506, 508, 510**
//! (archivi pari ≥ 504). Gli archivi 500-503 non hanno variante invernale nel
//! gioco originale, quindi restano identici in ogni stagione.
//!
//! La mappatura `clima → archivio base` **non è duplicata qui**: viene da
//! `crate::data::climate::world_climate_settings` (che è già allineata a DFU).
//! Il vecchio codice locale usava `231 → 506` (sbagliato: 506 è Woodland Hills,
//! il clima di Daggerfall è 231 Temperate Woodland → 504) e `base + 8` per
//! l'inverno (sbagliato: la regola è `+1`). Risultato: `231 + Winter → 514`,
//! archivio inesistente → fallback a 506 → **alberi sempre identici**,
//! estate e inverno, dentro e fuori città.
//!
//! `NaturePool::cache` è keyed su `archive_id` (es. 504, 505), quindi cambiando
//! stagione la chiave cambia e i `TreeType` vengono ricaricati automaticamente:
//! non serve invalidare `NaturePool`.
//!
//! Se l'archivio invernale (`505`, `507`, `509`, `511`) non esiste (mod senza
//! varianti invernali HD, o ARENA2 incompleto), `get_or_load` fa fallback
//! all'archivio estivo (`archive_id - 1`) con un warning: meglio alberi estivi
//! che nessun albero.
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
//! **Non toccare senza testare**: se gli alberi appaiono capovolti, il primo
//! posto da guardare è questo `if`.

use crate::data::climate::Season;
use crate::data::world_climate_settings;
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
// Scelta archivio in base a clima e stagione
// ---------------------------------------------------------------------------

/// Archivio di natura per `(climate, season)`, come in Daggerfall Unity.
///
/// **Regola DFU** (fonte: Interkarma su forum.dfworkshop.net):
///
/// - il clima decide l'archivio **base** (500..=510), via
///   `crate::data::climate::world_climate_settings(...).nature_archive`;
/// - la variante invernale è `base + 1` **solo** per `504`, `506`, `508`, `510`;
/// - `500`, `501`, `502`, `503` non hanno variante invernale: restano identici;
/// - `Rain` condivide l'archivio estivo (la neve è solo invernale).
///
/// Non duplicare la mappatura `climate → base`: quella di
/// `world_climate_settings` è già allineata a DFU ed è l'unica fonte di verità.
pub fn nature_archive_for_climate(climate: i32, season: Season) -> u32 {
    let base = world_climate_settings(climate).nature_archive as u32;
    match season {
        Season::Summer | Season::Rain => base,
        Season::Winter => {
            // Varianti invernali: 504→505, 506→507, 508→509, 510→511.
            // Cioè solo archivi pari ≥ 504. 500-503 non hanno neve (foresta
            // pluviale, sub-tropicale, palude, deserto).
            if base >= 504 && base % 2 == 0 {
                base + 1
            } else {
                base
            }
        }
    }
}

fn tree_count_for_climate(climate: i32) -> usize {
    match climate {
        223 => 0,
        224 | 225 => 60,   // Deserto
        226 => 110,        // Montagna
        227 => 450,        // Foresta pluviale
        228 => 360,        // Palude
        229 => 280,        // Sub-tropicale
        230 => 320,        // Boschi di montagna
        231 | 232 => 400,  // Boschi (Daggerfall)
        _ => 240,
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
pub(crate) fn create_cross_billboard_mesh(w: f32, h: f32, is_unity_bottom_up: bool) -> Mesh {
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
            let mut by_rec =
            Self::build_archive(archive_id, data, mods, images, meshes, materials);
            // Fallback: se l'archivio invernale (505, 507, 509, 511) non
            // esiste, usa il corrispondente estivo (archive_id - 1). Meglio
            // alberi estivi che nessun albero.
            if by_rec.is_empty()
                && matches!(archive_id, 505 | 507 | 509 | 511)
                {
                    let base = archive_id - 1;
                    warn!(
                        "Archivio vegetazione invernale {} mancante, uso il corrispondente estivo {}",
                        archive_id, base
                    );
                    by_rec = Self::build_archive(base, data, mods, images, meshes, materials);
                }
            // Lookup per VERO numero di record (serve ai flat/scenery RMB delle
            // città). `species` invece e' ordinata per dimensione e serve solo
            // alla scelta casuale degli alberi selvatici.
            for (r, tt) in &by_rec {
                self.by_record.insert((archive_id, *r), tt.clone());
            }
            let mut species: Vec<TreeType> = by_rec.into_iter().map(|(_, tt)| tt).collect();
            species.sort_by(|a, b| b.size.partial_cmp(&a.size).unwrap_or(std::cmp::Ordering::Equal));
            self.cache.insert(archive_id, species);
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
    ) -> Vec<(u32, TreeType)> {
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
            species.push((r as u32, TreeType {
                mesh: mesh_handle,
                material: mat_handle,
                size: h_meters * w_meters,
                w_meters,
                h_meters,
            }));
        }
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
///
/// `season` (Summer, Winter, Rain) pilota `nature_archive_for_climate`.
pub fn spawn_trees_for_chunk(
    commands: &mut Commands,
    chunk_entity: Entity,
    mx: i32,
    my: i32,
    climate: i32,
    season: Season,
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
    let archive = nature_archive_for_climate(climate, season);
    let tree_species = pool.get_or_load(archive, data, mods, images, meshes, materials);
    if tree_species.is_empty() {
        return;
    }
    let seed = ((mx as u64) << 32) ^ (my as u64) ^ 0x9e3779b97f4a7c15;
    let mut rng = SimpleRng::new(seed);

    commands.entity(chunk_entity).with_children(|parent| {
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
            let scale_var = rng.gen_f32(0.5, 0.9);
            parent.spawn((
                Mesh3d(tree.mesh.clone()),
                          MeshMaterial3d(tree.material.clone()),
                          Transform::from_xyz(local_x, y, local_z).with_scale(Vec3::splat(scale_var)),
            ));
        }
    });
}
