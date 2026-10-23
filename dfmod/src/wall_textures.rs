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
    /// flat (archivio, record) -> (materiale, larghezza px, altezza px)
    flats: HashMap<(u16, u8), Option<(Handle<StandardMaterial>, f32, f32)>>,
    /// (archivio originale, record) -> dimensioni in pixel della texture originale
    sizes: HashMap<(u16, u8), (f32, f32)>,
}

/// Le UV delle mura superano 1: serve la ripetizione.
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
    // versione HD delle mod, se c'e'
    let key = TextureKey { archive, record: record as u32, frame: 0, kind: MapKind::Albedo };
    if let Some(handle) = mods.image_by_key(&key, images) {
        if let Some(img) = images.get_mut(&handle) {
            img.sampler = repeat_sampler();
        }
        return Some(handle);
    }

    // texture originale da TEXTURE.nnn
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
    /// Materiale per la texture `(archive, record)` del modello originale, nel clima e
    /// nella stagione del map pixel.
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

        // se la variante per clima/stagione non esiste, riprova con l'archivio del modello
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
}

impl WallTextures {
    /// Dimensioni in pixel del record originale di TEXTURE.nnn. Servono a normalizzare le UV
    /// dei modelli ARCH3D (1/16 di pixel: dividere per 16 e poi per larghezza/altezza).
    /// Si usa sempre la texture originale, anche se una mod fornisce la versione HD:
    /// l'immagine HD copre comunque l'intervallo 0..1.
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
}

impl WallTextures {
    /// Materiale di un flat (sprite): l'indice di palette 0 e' trasparente. Restituisce anche
    /// larghezza e altezza in pixel. `None` se l'archivio o il record non esistono.
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
        let result: Option<(Handle<StandardMaterial>, f32, f32)> = (|| {
            let path = find_file_ci(&data.dir, &archive_file_name(archive as u32))?;
            let file = TextureFile::open(&path).ok()?;
            let frame = file.frame(record as usize, 0).ok()?;
            if frame.width == 0 || frame.height == 0 {
                return None;
            }
            let rgba = frame.to_rgba(&data.palette, Some(0));
            let (w, h) = (frame.width as f32, frame.height as f32);
            let mut image = Image::new(
                Extent3d { width: frame.width as u32, height: frame.height as u32, depth_or_array_layers: 1 },
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
            Some((material, w, h))
        })();
        self.flats.insert((archive, record), result.clone());
        result
    }
}
