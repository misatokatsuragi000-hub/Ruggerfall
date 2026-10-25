//! Texture originali di Daggerfall per le mura procedurali: un materiale per ogni
//! (archivio, record), con clima e stagione applicati come per il resto del mondo.
//!
//! ## Normal map
//!
//! DREAM fornisce normal map in formato BC7 con canale verde **DirectX** (Y giù).
//! Bevy le interpreta in **OpenGL** (Y su), quindi la luce risulterebbe invertita
//! (i dettagli sembrano incavati invece che in rilievo). Per correggere:
//!
//! - le normal map vengono caricate con `ModTextures::image_normal_by_key`, che
//!   decodifica BC7 → Rgba8, gira il canale verde e allinea i pixel alla
//!   convenzione V dell'albedo (bottom-up);
//! - se la mod non ha la normal map, si deriva dall'albedo con un Sobel tenue
//!   (blur 3x3 + strength 1.0), che è meglio di niente;
//! - se nessuno dei due funziona, il materiale resta senza normal map.
//!
//! ## Flip V delle texture dei muri
//!
//! Le texture BC7 di DREAM (come i dati raw di Unity/Daggerfall) sono in ordine
//! "bottom-up" (riga 0 = fondo). Bevy assume V=0 in alto: senza correzione i muri
//! appaiono capovolti in Y. `material()` applica quindi un `uv_transform` che
//! gira la V **quando** l'albedo è BC7 (`ModTextures::needs_v_flip`), lo stesso
//! meccanismo usato dai flat. La normal map caricata da `image_normal_by_key`
//! è già in fondo-up (allineata all'albedo BC7), quindi condivide lo stesso
//! `uv_transform`.
//!
//! I flat (decorazioni, NPC) **non** ricevono normal map: sono billboard, il
//! normal mapping non aggiunge nulla.

use crate::mod_textures::ModTextures;
use crate::world::GameData;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::math::Affine2;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use crate::data::climate::{apply_climate, world_climate_settings, Season};
use crate::data::{archive_file_name, find_file_ci, TextureFile};
use dfmod::{MapKind, TextureKey};
use std::collections::{HashMap, HashSet};

#[derive(Resource, Default)]
pub struct WallTextures {
    cache: HashMap<(u32, u8), Handle<StandardMaterial>>,
    flats: HashMap<(u16, u8), Option<(Handle<StandardMaterial>, f32, f32)>>,
    flat_frames: HashMap<(u16, u8, u16), Option<(Handle<StandardMaterial>, f32, f32)>>,
    sizes: HashMap<(u16, u8), (f32, f32)>,
    normal_cache: HashMap<(u32, u8), Option<Handle<Image>>>,
    normal_materials: HashSet<AssetId<StandardMaterial>>,
}

fn repeat_sampler() -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Nearest,
        min_filter: ImageFilterMode::Linear,
        ..default()
    })
}

/// Gira in verticale una matrice RGBA8 in-place (swap delle righe).
/// Duplicato di `mod_textures::flip_v_in_place`: piccola funzione, non vale
/// la pena renderla `pub(crate)` solo per questo.
fn flip_v_rgba8(rgba: &mut [u8], w: usize, h: usize) {
    let row = w * 4;
    for y in 0..h / 2 {
        let top = y * row;
        let bot = (h - 1 - y) * row;
        for i in 0..row {
            rgba.swap(top + i, bot + i);
        }
    }
}

/// Albedo da mod o dal file originale.
///
/// Restituisce anche `needs_flip`: true se la texture è in ordine "bottom-up"
/// e quindi va mostrata con la V capovolta tramite `uv_transform`. È true per
/// i BC7 di DREAM (vedi `ModTextures::needs_v_flip`).
fn load_albedo(
    archive: u32,
    record: u8,
    data: &GameData,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
) -> Option<(Handle<Image>, bool)> {
    let key = TextureKey {
        archive,
        record: record as u32,
        frame: 0,
        kind: MapKind::Albedo,
    };
    if let Some(handle) = mods.image_by_key(&key, images) {
        if let Some(img) = images.get_mut(&handle) {
            img.sampler = repeat_sampler();
        }
        let needs_flip = mods.needs_v_flip(&handle);
        return Some((handle, needs_flip));
    }
    let path = find_file_ci(&data.dir, &archive_file_name(archive))?;
    let file = TextureFile::open(&path).ok()?;
    let frame = file.frame(record as usize, 0).ok()?;
    if frame.width == 0 || frame.height == 0 {
        return None;
    }
    let rgba = frame.to_rgba(&data.palette, None);
    let mut image = Image::new(
        Extent3d {
            width: frame.width as u32,
            height: frame.height as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = repeat_sampler();
    // Vanilla ipotizzata top-down (Bevy-friendly): niente flip. Se anche le
    // texture originali risultassero capovolte, cambiare `false` in `true`.
    Some((images.add(image), false))
}

/// Deriva una normal map dall'albedo (blur 3x3 + Sobel, strength bassa).
/// Solo fallback se la mod non ha la normal map vera.
fn derive_normal_map(rgba: &[u8], w: usize, h: usize) -> Image {
    debug_assert_eq!(rgba.len(), w * h * 4);

    let lum_at = |x: i32, y: i32| -> f32 {
        let cx = x.clamp(0, w as i32 - 1) as usize;
        let cy = y.clamp(0, h as i32 - 1) as usize;
        let p = (cy * w + cx) * 4;
        let r = rgba[p] as f32 / 255.0;
        let g = rgba[p + 1] as f32 / 255.0;
        let b = rgba[p + 2] as f32 / 255.0;
        0.2126 * r + 0.7152 * g + 0.0722 * b
    };

    let mut blurred = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.0;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    sum += lum_at(x as i32 + dx, y as i32 + dy);
                }
            }
            blurred[y * w + x] = sum / 9.0;
        }
    }
    let lum = |x: i32, y: i32| -> f32 {
        let cx = x.clamp(0, w as i32 - 1) as usize;
        let cy = y.clamp(0, h as i32 - 1) as usize;
        blurred[cy * w + cx]
    };

    const STRENGTH: f32 = 1.0;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let dx = lum(x as i32 + 1, y as i32) - lum(x as i32 - 1, y as i32);
            let dy = lum(x as i32, y as i32 + 1) - lum(x as i32, y as i32 - 1);
            let nx = -dx * STRENGTH;
            let ny = -dy * STRENGTH;
            let nz = 1.0;
            let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-6);
            let p = (y * w + x) * 4;
            out[p] = (((nx / len) * 0.5 + 0.5) * 255.0) as u8;
            out[p + 1] = (((ny / len) * 0.5 + 0.5) * 255.0) as u8;
            out[p + 2] = (((nz / len) * 0.5 + 0.5) * 255.0) as u8;
            out[p + 3] = 255;
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: w as u32,
            height: h as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        out,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::default(),
    );
    image.sampler = repeat_sampler();
    image
}

fn flat_albedo_from_mod(
    archive: u16,
    record: u8,
    frame: u16,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
) -> Option<(Handle<Image>, bool)> {
    let key = TextureKey {
        archive: archive as u32,
        record: record as u32,
        frame: frame as u32,
        kind: MapKind::Albedo,
    };
    let handle = mods.image_by_key(&key, images)?;
    let needs_flip = mods.needs_v_flip(&handle);
    Some((handle, needs_flip))
}

impl WallTextures {
    /// Normal map per un muro: prima dalla mod (decodificata + verde girato),
    /// poi derivata dall'albedo, poi niente.
    ///
    /// `albedo_needs_flip`: se true, l'albedo del muro è in ordine bottom-up
    /// (BC7 mod) e il materiale verrà mostrato con `uv_transform` che gira la V.
    /// In tal caso, se la normal map viene **derivata** dal file originale,
    /// gio l'albedo di input al Sobel per farla finire nella stessa convenzione
    /// bottom-up dell'albedo BC7 (altrimenti la luce risulterebbe invertita).
    fn normal_map(
        &mut self,
        archive: u32,
        record: u8,
        albedo_needs_flip: bool,
        data: &GameData,
        mods: &mut ModTextures,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        if let Some(cached) = self.normal_cache.get(&(archive, record)) {
            return cached.clone();
        }

        // 1. Normal map dalla mod, corretta (DirectX → OpenGL, V allineato).
        let key = TextureKey {
            archive,
            record: record as u32,
            frame: 0,
            kind: MapKind::Normal,
        };
        if let Some(handle) = mods.image_normal_by_key(&key, images) {
            // Le mura vanno tiled: forziamo il sampler Repeat. Nota che
            // `image_normal_by_key` crea l'immagine con `RENDER_WORLD`, quindi
            // dopo il primo upload sulla GPU `get_mut` può restituire `None`.
            if let Some(img) = images.get_mut(&handle) {
                img.sampler = repeat_sampler();
            }
            info!("Normal map {archive}:{record} dalla mod (verde girato, Rgba8Unorm)");
            self.normal_cache.insert((archive, record), Some(handle.clone()));
            return Some(handle);
        }

        // 2. Deriva dall'albedo originale.
        info!("Normal map {archive}:{record} non in mod, derivo dall'albedo");
        let path = find_file_ci(&data.dir, &archive_file_name(archive))?;
        let file = TextureFile::open(&path).ok()?;
        let frame = file.frame(record as usize, 0).ok()?;
        if frame.width == 0 || frame.height == 0 {
            self.normal_cache.insert((archive, record), None);
            return None;
        }
        let mut rgba = frame.to_rgba(&data.palette, None);
        // Allinea l'input alla convenzione dell'albedo mod (BC7 = bottom-up),
        // così la normal derivata può condividere lo stesso `uv_transform`.
        if albedo_needs_flip {
            flip_v_rgba8(&mut rgba, frame.width, frame.height);
        }
        let normal = derive_normal_map(&rgba, frame.width, frame.height);
        let handle = images.add(normal);
        self.normal_cache.insert((archive, record), Some(handle.clone()));
        Some(handle)
    }

    pub fn material(
        &mut self,
        archive: u16,
        record: u8,
        world_climate: i32,
        data: &GameData,
        mods: &mut ModTextures,
        images: &mut Assets<Image>,
        materials: &mut Assets<StandardMaterial>,
    ) -> Handle<StandardMaterial> {
        let season = if data.winter { Season::Winter } else { Season::Summer };
        let climate = world_climate_settings(world_climate).climate_type;
        let final_archive = apply_climate(archive as i32, record as i32, climate, season).max(0) as u32;
        let key = (final_archive, record);
        if let Some(handle) = self.cache.get(&key) {
            return handle.clone();
        }

        let albedo_result = load_albedo(final_archive, record, data, mods, images)
        .or_else(|| load_albedo(archive as u32, record, data, mods, images));
        let (albedo, needs_flip) = match albedo_result {
            Some((handle, flip)) => (Some(handle), flip),
            None => (None, false),
        };

        let normal = self.normal_map(final_archive, record, needs_flip, data, mods, images);

        // Flip V per i muri BC7 (bottom-up, convenzione Unity/Daggerfall).
        // Stesso meccanismo dei flat (vedi `flat_material_frame`).
        let uv_transform = if needs_flip {
            Affine2::from_scale_angle_translation(
                Vec2::new(1.0, -1.0),
                                                  0.0,
                                                  Vec2::new(0.0, 1.0),
            )
        } else {
            Affine2::IDENTITY
        };

        let has_normal = normal.is_some();
        let material = match albedo {
            Some(texture) => StandardMaterial {
                base_color_texture: Some(texture),
                normal_map_texture: normal,
                uv_transform,
                perceptual_roughness: 0.9,
                reflectance: 0.1,
                ..default()
            },
            None => {
                warn!("Texture mura {final_archive}/{record} non trovata: uso un grigio pietra");
                StandardMaterial {
                    base_color: Color::srgb(0.5, 0.49, 0.47),
                    perceptual_roughness: 0.9,
                    ..default()
                }
            }
        };
        let handle = materials.add(material);
        if has_normal {
            self.normal_materials.insert(handle.id());
        }
        self.cache.insert(key, handle.clone());
        handle
    }

    pub fn has_normal_map(&self, mat: &Handle<StandardMaterial>) -> bool {
        self.normal_materials.contains(&mat.id())
    }

    pub fn original_size(&mut self, archive: u16, record: u8, data: &GameData) -> (f32, f32) {
        if let Some(size) = self.sizes.get(&(archive, record)) {
            return *size;
        }
        let size = (|| {
            let path = find_file_ci(&data.dir, &archive_file_name(archive as u32))?;
            let file = TextureFile::open(&path).ok()?;
            let frame = file.frame(record as usize, 0).ok()?;
            (frame.width > 0 && frame.height > 0)
            .then(|| (frame.width as f32, frame.height as f32))
        })()
        .unwrap_or((64.0, 64.0));
        self.sizes.insert((archive, record), size);
        size
    }

    pub fn flat_material_frame(
        &mut self,
        archive: u16,
        record: u8,
        frame: u16,
        data: &GameData,
        mods: &mut ModTextures,
        images: &mut Assets<Image>,
        materials: &mut Assets<StandardMaterial>,
    ) -> Option<(Handle<StandardMaterial>, f32, f32)> {
        let key = (archive, record, frame);
        if let Some(cached) = self.flat_frames.get(&key) {
            return cached.clone();
        }
        let result: Option<(Handle<StandardMaterial>, f32, f32)> = (|| {
            let path = find_file_ci(&data.dir, &archive_file_name(archive as u32))?;
            let file = TextureFile::open(&path).ok()?;
            let rec = file.record(record as usize)?;
            let img = file.frame(record as usize, frame as usize).ok()?;
            if img.width == 0 || img.height == 0 {
                return None;
            }

            let scale_x = rec.scale_x as f32 / 256.0;
            let scale_y = rec.scale_y as f32 / 256.0;
            let final_w = img.width as f32 * (1.0 + scale_x);
            let final_h = img.height as f32 * (1.0 + scale_y);

            let (image_handle, uv_transform) =
            match flat_albedo_from_mod(archive, record, frame, mods, images) {
                Some((handle, needs_flip)) => {
                    let tf = if needs_flip {
                        Affine2::from_scale_angle_translation(
                            Vec2::new(1.0, -1.0),
                                                              0.0,
                                                              Vec2::new(0.0, 1.0),
                        )
                    } else {
                        Affine2::IDENTITY
                    };
                    (handle, tf)
                }
                None => {
                    let rgba = img.to_rgba(&data.palette, Some(0));
                    let mut image = Image::new(
                        Extent3d {
                            width: img.width as u32,
                            height: img.height as u32,
                            depth_or_array_layers: 1,
                        },
                        TextureDimension::D2,
                        rgba,
                        TextureFormat::Rgba8UnormSrgb,
                        RenderAssetUsages::default(),
                    );
                    image.sampler = ImageSampler::nearest();
                    (images.add(image), Affine2::IDENTITY)
                }
            };

            let material = materials.add(StandardMaterial {
                base_color_texture: Some(image_handle),
                                         alpha_mode: AlphaMode::Mask(0.5),
                                         cull_mode: None,
                                         double_sided: true,
                                         perceptual_roughness: 0.9,
                                         reflectance: 0.1,
                                         uv_transform,
                                         ..default()
            });
            Some((material, final_w, final_h))
        })();
        self.flat_frames.insert(key, result.clone());
        result
    }

    pub fn flat_material(
        &mut self,
        archive: u16,
        record: u8,
        data: &GameData,
        mods: &mut ModTextures,
        images: &mut Assets<Image>,
        materials: &mut Assets<StandardMaterial>,
    ) -> Option<(Handle<StandardMaterial>, f32, f32)> {
        if let Some(cached) = self.flats.get(&(archive, record)) {
            return cached.clone();
        }
        let result = self.flat_material_frame(archive, record, 0, data, mods, images, materials);
        self.flats.insert((archive, record), result.clone());
        result
    }
}
