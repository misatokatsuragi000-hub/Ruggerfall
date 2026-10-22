//! Texture originali di Daggerfall per le mura procedurali: un materiale per ogni
//! (archivio, record), con clima e stagione applicati come per il resto del mondo.
//! Se una mod fornisce la versione HD della stessa texture, usa quella.

use crate::mod_textures::ModTextures;
use crate::world::GameData;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use daggerfall_data::climate::{apply_climate, world_climate_settings, Season};
use daggerfall_data::{archive_file_name, find_file_ci, TextureFile};
use dfmod::{MapKind, TextureKey};
use std::collections::HashMap;

#[derive(Resource, Default)]
pub struct WallTextures {
    /// (archivio dopo il clima, record) -> materiale
    cache: HashMap<(u32, u8), Handle<StandardMaterial>>,
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
