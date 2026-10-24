use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat as BevyFormat},
};
use dfmod::{MapKind, ModArchive, TextureFormat as DfFormat, TextureKey};

type Entry = (usize, String);

#[derive(Resource, Default)]
pub struct ModTextures {
    archives: Vec<ModArchive>,
    by_key: HashMap<TextureKey, Entry>,
    by_name: HashMap<String, Entry>,
    terrain_by_archive: HashMap<u32, Entry>,
    arrays_by_name: HashMap<String, Entry>,
    handles: HashMap<(Entry, bool), Option<Handle<Image>>>,
    terrain_handles: HashMap<u32, Option<Handle<Image>>>,
    pub nearest_filter: bool,
}

impl ModTextures {
    pub fn add(&mut self, path: &Path) -> dfmod::Result<()> {
        let m = ModArchive::open(path)?;
        let idx = self.archives.len();

        let mut tex_count = 0;
        for name in m.texture_names() {
            let entry = (idx, name.to_owned());
            if let Some(k) = TextureKey::parse(name) {
                self.by_key.insert(k, entry.clone());
            }
            // Indicizza con chiave minuscola per lookup case-insensitive
            self.by_name.insert(name.to_ascii_lowercase(), entry);
            tex_count += 1;
        }

        let mut array_count = 0;
        for name in m.texture_array_names() {
            let entry = (idx, name.to_owned());
            self.arrays_by_name.insert(name.to_ascii_lowercase(), entry.clone());

            let lower = name.to_ascii_lowercase();
            if let Some(num_str) = lower.strip_suffix("-texarray") {
                if let Ok(archive_id) = num_str.parse::<u32>() {
                    self.terrain_by_archive.insert(archive_id, entry);
                }
            }
            array_count += 1;
        }

        info!(
            "Mod caricata `{}`: {} texture 2D, {} array di terreno",
            path.display(),
              tex_count,
              array_count
        );

        self.archives.push(m);
        Ok(())
    }

    pub fn load_dir(&mut self, dir: &Path) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            info!("Cartella mod `{}` non trovata, nessuna mod caricata", dir.display());
            return;
        };
        let mut paths: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dfmod")))
        .collect();
        paths.sort();
        for p in paths {
            if let Err(e) = self.add(&p) {
                warn!("Errore nel caricamento della mod {}: {e}", p.display());
            }
        }
    }

    pub fn terrain_texture_array(
        &mut self,
        archive_id: u32,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        if let Some(cached) = self.terrain_handles.get(&archive_id) {
            return cached.clone();
        }

        let entry = self.terrain_by_archive.get(&archive_id)?.clone();
        let handle = self.load_array(&entry, images);
        self.terrain_handles.insert(archive_id, handle.clone());
        handle
    }

    pub fn texture_array_by_name(
        &mut self,
        name: &str,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let entry = self.arrays_by_name.get(&name.to_ascii_lowercase())?.clone();
        self.load_array(&entry, images)
    }

    fn load_array(&self, (idx, name): &Entry, images: &mut Assets<Image>) -> Option<Handle<Image>> {
        let arr = match self.archives[*idx].texture_array(name) {
            Ok(Some(a)) => a,
            Ok(None) => return None,
            Err(e) => {
                warn!("Errore lettura Texture2DArray `{name}`: {e}");
                return None;
            }
        };

        let mut image = Image::new(
            Extent3d {
                width: arr.width,
                height: arr.height,
                depth_or_array_layers: arr.depth,
            },
            TextureDimension::D2,
            arr.data,
            BevyFormat::Bc7RgbaUnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );

        image.texture_descriptor.mip_level_count = arr.mip_count;
        image.sampler = ImageSampler::linear();

        Some(images.add(image))
    }

    pub fn image_by_key(&mut self, key: &TextureKey, images: &mut Assets<Image>) -> Option<Handle<Image>> {
        let entry = self.by_key.get(key)?.clone();
        let srgb = !matches!(key.kind, MapKind::Normal | MapKind::MetallicGloss);
        self.load_2d(entry, srgb, images)
    }

    pub fn image_by_name(&mut self, name: &str, images: &mut Assets<Image>) -> Option<Handle<Image>> {
        let entry = self.by_name.get(&name.to_ascii_lowercase())?.clone();
        self.load_2d(entry, true, images)
    }

    /// Dimensioni in pixel (larghezza, altezza) di una texture 2D della mod, senza caricarla
    /// in `Assets<Image>`. Utile perche' le immagini create con `RENDER_WORLD` spariscono da
    /// `Assets<Image>` dopo il primo upload sulla GPU e `images.get()` non le trova piu'.
    pub fn size_by_name(&self, name: &str) -> Option<(u32, u32)> {
        let (idx, entry_name) = self.by_name.get(&name.to_ascii_lowercase())?;
        let tex = self.archives[*idx].texture(entry_name).ok()??;
        Some((tex.width, tex.height))
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    fn load_2d(&mut self, entry: Entry, srgb: bool, images: &mut Assets<Image>) -> Option<Handle<Image>> {
        let id = (entry, srgb);
        if let Some(cached) = self.handles.get(&id) {
            return cached.clone();
        }
        let handle = self.decode_2d(&id.0, srgb).map(|img| images.add(img));
        self.handles.insert(id, handle.clone());
        handle
    }

    fn decode_2d(&self, (idx, name): &Entry, srgb: bool) -> Option<Image> {
        let tex = match self.archives[*idx].texture(name) {
            Ok(Some(t)) => t,
            Ok(None) => return None,
            Err(e) => {
                warn!("Errore texture mod `{name}`: {e}");
                return None;
            }
        };

        // Supporto nativo hardware per BC7 (usato da DREAM - Sprites)
        if tex.format == DfFormat::BC7 {
            let mut image = Image::new(
                Extent3d {
                    width: tex.width,
                    height: tex.height,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                tex.data.clone(),
                                       if srgb { BevyFormat::Bc7RgbaUnormSrgb } else { BevyFormat::Bc7RgbaUnorm },
                                           RenderAssetUsages::RENDER_WORLD,
            );
            image.texture_descriptor.mip_level_count = tex.mip_count;
            image.sampler = if self.nearest_filter { ImageSampler::nearest() } else { ImageSampler::linear() };
            return Some(image);
        }

        // Formati non compressi o DXT decodificabili su CPU
        let rgba = match tex.to_rgba8_top_down() {
            Ok(px) => px,
            Err(e) => {
                warn!("Impossibile decodificare `{name}` ({}): {e}", tex.format.name());
                return None;
            }
        };

        let mut image = Image::new(
            Extent3d { width: tex.width, height: tex.height, depth_or_array_layers: 1 },
            TextureDimension::D2,
            rgba,
            if srgb { BevyFormat::Rgba8UnormSrgb } else { BevyFormat::Rgba8Unorm },
                RenderAssetUsages::RENDER_WORLD,
        );
        image.sampler = if self.nearest_filter { ImageSampler::nearest() } else { ImageSampler::linear() };
        Some(image)
    }
}

pub struct ModTexturesPlugin {
    pub dir: PathBuf,
    pub nearest_filter: bool,
}

impl Default for ModTexturesPlugin {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("mods"),
            nearest_filter: false,
        }
    }
}

impl Plugin for ModTexturesPlugin {
    fn build(&self, app: &mut App) {
        let mut mods = ModTextures {
            nearest_filter: self.nearest_filter,
            ..Default::default()
        };
        mods.load_dir(&self.dir);
        app.insert_resource(mods);
    }
}
