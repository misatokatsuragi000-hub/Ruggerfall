//! Texture delle mod `.dfmod` per Bevy (testato solo concettualmente, mirato a Bevy 0.16).
//!
//! - All'avvio indicizza i `.dfmod` in una cartella (solo il container, quindi è veloce).
//! - Decodifica una texture solo alla prima richiesta e ne tiene l'`Handle<Image>` in cache.
//! - Usa la risoluzione originale della mod: nessun ridimensionamento, nessun atlas.
//!
//! Cargo.toml:  dfmod = { path = "../dfmod" }

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use dfmod::{MapKind, ModArchive, TextureKey};

/// (indice della mod, nome della texture dentro la mod)
type Entry = (usize, String);

#[derive(Resource, Default)]
pub struct ModTextures {
    archives: Vec<ModArchive>,
    by_key: HashMap<TextureKey, Entry>,
    by_name: HashMap<String, Entry>,
    /// Cache per (texture, sRGB sì/no). `None` = decodifica fallita, non si riprova.
    handles: HashMap<(Entry, bool), Option<Handle<Image>>>,
    /// `true` per il look pixelato di Daggerfall, `false` per filtro bilineare.
    pub nearest_filter: bool,
}

impl ModTextures {
    /// Aggiunge una mod. Da chiamare in ordine di load order: le successive sovrascrivono.
    pub fn add(&mut self, path: &Path) -> dfmod::Result<()> {
        let m = ModArchive::open(path)?;
        let idx = self.archives.len();
        for name in m.texture_names() {
            let entry = (idx, name.to_owned());
            if let Some(k) = TextureKey::parse(name) {
                self.by_key.insert(k, entry.clone());
            }
            self.by_name.insert(name.to_owned(), entry);
        }
        info!("mod {}: {} texture", path.display(), m.texture_names().count());
        self.archives.push(m);
        Ok(())
    }

    /// Carica tutti i `.dfmod` di una cartella, in ordine alfabetico (deterministico).
    pub fn load_dir(&mut self, dir: &Path) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            info!("cartella mod `{}` non trovata, nessuna mod caricata", dir.display());
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
                warn!("mod {}: {e}", p.display());
            }
        }
    }

    /// Texture sostitutiva per una chiave Daggerfall (`archivio_record-frame[_Normal...]`).
    pub fn image_by_key(&mut self, key: &TextureKey, images: &mut Assets<Image>) -> Option<Handle<Image>> {
        let entry = self.by_key.get(key)?.clone();
        // Colore -> sRGB; normal/metallic sono dati lineari.
        let srgb = !matches!(key.kind, MapKind::Normal | MapKind::MetallicGloss);
        self.load(entry, srgb, images)
    }

    /// Texture per nome libero (es. `dragonbillboard_01`, `move00i0`), sempre trattata come colore.
    pub fn image_by_name(&mut self, name: &str, images: &mut Assets<Image>) -> Option<Handle<Image>> {
        let entry = self.by_name.get(&name.to_ascii_lowercase())?.clone();
        self.load(entry, true, images)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    fn load(&mut self, entry: Entry, srgb: bool, images: &mut Assets<Image>) -> Option<Handle<Image>> {
        let id = (entry, srgb);
        if let Some(cached) = self.handles.get(&id) {
            return cached.clone();
        }
        let handle = self.decode(&id.0, srgb).map(|img| images.add(img));
        self.handles.insert(id, handle.clone());
        handle
    }

    fn decode(&self, (idx, name): &Entry, srgb: bool) -> Option<Image> {
        let tex = match self.archives[*idx].texture(name) {
            Ok(Some(t)) => t,
            Ok(None) => return None,
            Err(e) => {
                warn!("texture mod `{name}`: {e}");
                return None;
            }
        };
        // Se il formato non è supportato (es. crunched) si torna None: il chiamante usa l'originale.
        let rgba = match tex.to_rgba8_top_down() {
            Ok(px) => px,
            Err(e) => {
                warn!("texture mod `{name}` ({}): {e}", tex.format.name());
                return None;
            }
        };
        let mut image = Image::new(
            Extent3d { width: tex.width, height: tex.height, depth_or_array_layers: 1 },
            TextureDimension::D2,
            rgba,
            if srgb { TextureFormat::Rgba8UnormSrgb } else { TextureFormat::Rgba8Unorm },
            RenderAssetUsages::RENDER_WORLD, // la copia CPU non serve: risparmia memoria
        );
        image.sampler = if self.nearest_filter { ImageSampler::nearest() } else { ImageSampler::linear() };
        Some(image)
    }
}

/// Indicizza `dir` all'avvio e inserisce la risorsa [`ModTextures`].
pub struct ModTexturesPlugin {
    pub dir: PathBuf,
    pub nearest_filter: bool,
}

impl Default for ModTexturesPlugin {
    fn default() -> Self {
        Self { dir: PathBuf::from("mods"), nearest_filter: true }
    }
}

impl Plugin for ModTexturesPlugin {
    fn build(&self, app: &mut App) {
        let mut mods = ModTextures { nearest_filter: self.nearest_filter, ..Default::default() };
        mods.load_dir(&self.dir);
        app.insert_resource(mods);
    }
}

// ---------------------------------------------------------------------------------------------
// Esempio d'uso
//
//   App::new()
//       .add_plugins((DefaultPlugins, ModTexturesPlugin::default()))
//       .add_systems(Startup, spawn_dragon)
//       .run();
//
//   fn spawn_dragon(
//       mut commands: Commands,
//       mut mods: ResMut<ModTextures>,
//       mut images: ResMut<Assets<Image>>,
//       mut meshes: ResMut<Assets<Mesh>>,
//       mut materials: ResMut<Assets<StandardMaterial>>,
//   ) {
//       // Mondo: la dimensione del quad viene dai dati ORIGINALI di Daggerfall, non dai pixel
//       // della mod. Cambia solo la densità dei pixel, non la scala in scena.
//       let (world_w, world_h) = (2.0, 1.25);
//       let base = mods
//           .image_by_name("dragonbillboard_01", &mut images)
//           .or_else(|| original_texture(&mut images)); // fallback alla texture di Daggerfall
//
//       commands.spawn((
//           Mesh3d(meshes.add(Rectangle::new(world_w, world_h))),
//           MeshMaterial3d(materials.add(StandardMaterial {
//               base_color_texture: base,
//               alpha_mode: AlphaMode::Blend,
//               unlit: true,
//               ..default()
//           })),
//       ));
//   }
//
//   // UI (Bevy 0.16): ImageNode::new(handle)
// ---------------------------------------------------------------------------------------------
