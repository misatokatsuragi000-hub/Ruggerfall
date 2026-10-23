//! Texture originali di Daggerfall per le mura procedurali: un materiale per ogni
//! (archivio, record), con clima e stagione applicati come per il resto del mondo.
//! Se una mod fornisce la versione HD della stessa texture, usa quella.
use crate::mod_textures::ModTextures;
use crate::world::GameData;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use crate::data::climate::{apply_climate, world_climate_settings, Season};
use crate::data::{archive_file_name, find_file_ci, TextureFile};
use dfmod::{MapKind, TextureKey};
use std::collections::HashMap;

#[derive(Resource, Default)]
pub struct WallTextures {
    /// (archivio dopo il clima, record) -> materiale
    cache: HashMap<(u32, u8), Handle<StandardMaterial>>,
    /// flat frame 0: (archivio, record) -> (materiale, larghezza scalata, altezza scalata)
    flats: HashMap<(u16, u8), Option<(Handle<StandardMaterial>, f32, f32)>>,
    /// frame animati: (archivio, record, frame) -> (materiale, larghezza scalata, altezza scalata)
    flat_frames: HashMap<(u16, u8, u16), Option<(Handle<StandardMaterial>, f32, f32)>>,
    /// (archivio originale, record) -> dimensioni in pixel della texture originale
    sizes: HashMap<(u16, u8), (f32, f32)>,
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

fn load_image(
    archive: u32,
    record: u8,
    data: &GameData,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
) -> Option<Handle<Image>> {
    let key = TextureKey { archive, record: record as u32, frame: 0, kind: MapKind::Albedo };
    if let Some(handle) = mods.image_by_key(&key, images) {
        if let Some(img) = images.get_mut(&handle) {
            img.sampler = repeat_sampler();
        }
        return Some(handle);
    }
    let path = find_file_ci(&data.dir, &archive_file_name(archive))?;
    let file = TextureFile::open(&path).ok()?;
    let frame = file.frame(record as usize, 0).ok()?;
    if frame.width == 0 || frame.height == 0 {
        return None;
    }
    let rgba = frame.to_rgba(&data.palette, None);
    let mut image = Image::new(
        Extent3d { width: frame.width as u32, height: frame.height as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = repeat_sampler();
    Some(images.add(image))
}

impl WallTextures {
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
        let image = load_image(final_archive, record, data, mods, images)
        .or_else(|| load_image(archive as u32, record, data, mods, images));
        let material = match image {
            Some(texture) => StandardMaterial {
                base_color_texture: Some(texture),
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
        self.cache.insert(key, handle.clone());
        handle
    }

    pub fn original_size(&mut self, archive: u16, record: u8, data: &GameData) -> (f32, f32) {
        if let Some(size) = self.sizes.get(&(archive, record)) {
            return *size;
        }
        let size = (|| {
            let path = find_file_ci(&data.dir, &archive_file_name(archive as u32))?;
            let file = TextureFile::open(&path).ok()?;
            let frame = file.frame(record as usize, 0).ok()?;
            (frame.width > 0 && frame.height > 0).then(|| (frame.width as f32, frame.height as f32))
        })()
        .unwrap_or((64.0, 64.0));
        self.sizes.insert((archive, record), size);
        size
    }

    /// Materiale di un flat con frame specifico (per animazioni NPC).
    /// Applica il fattore di scala ScaleX/ScaleY dal record (come in Daggerfall Unity).
    pub fn flat_material_frame(
        &mut self,
        archive: u16,
        record: u8,
        frame: u16,
        data: &GameData,
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
            let rgba = img.to_rgba(&data.palette, Some(0));

            // Applica il fattore di scala dal record (diviso 256 come in DFU).
            let scale_x = rec.scale_x as f32 / 256.0;
            let scale_y = rec.scale_y as f32 / 256.0;
            let final_w = img.width as f32 * (1.0 + scale_x);
            let final_h = img.height as f32 * (1.0 + scale_y);

            let mut image = Image::new(
                Extent3d { width: img.width as u32, height: img.height as u32, depth_or_array_layers: 1 },
                TextureDimension::D2,
                rgba,
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::default(),
            );
            image.sampler = ImageSampler::nearest();
            let material = materials.add(StandardMaterial {
                base_color_texture: Some(images.add(image)),
                                         alpha_mode: AlphaMode::Mask(0.5),
                                         cull_mode: None,
                                         double_sided: true,
                                         perceptual_roughness: 0.9,
                                         reflectance: 0.1,
                                         ..default()
            });
            Some((material, final_w, final_h))
        })();
        self.flat_frames.insert(key, result.clone());
        result
    }

    /// Versione semplificata: frame 0 (per decorazioni e flat statici).
    pub fn flat_material(
        &mut self,
        archive: u16,
        record: u8,
        data: &GameData,
        images: &mut Assets<Image>,
        materials: &mut Assets<StandardMaterial>,
    ) -> Option<(Handle<StandardMaterial>, f32, f32)> {
        if let Some(cached) = self.flats.get(&(archive, record)) {
            return cached.clone();
        }
        let result = self.flat_material_frame(archive, record, 0, data, images, materials);
        self.flats.insert((archive, record), result.clone());
        result
    }
}
