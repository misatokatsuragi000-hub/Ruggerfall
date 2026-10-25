use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    prelude::*,
    render::render_asset::RenderAssetUsages as BevyRenderAssetUsages,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat as BevyFormat},
};
use dfmod::{MapKind, ModArchive, Texture2D, TextureFormat as DfFormat, TextureKey};

type Entry = (usize, String);

/// Gira in verticale una matrice di pixel RGBA8 in-place (swap delle righe).
fn flip_v_in_place(rgba: &mut [u8], w: usize, h: usize) {
    let row = w * 4;
    for y in 0..h / 2 {
        let top = y * row;
        let bot = (h - 1 - y) * row;
        for i in 0..row {
            rgba.swap(top + i, bot + i);
        }
    }
}

#[derive(Resource, Default)]
pub struct ModTextures {
    archives: Vec<ModArchive>,
    by_key: HashMap<TextureKey, Entry>,
    by_name: HashMap<String, Entry>,
    terrain_by_archive: HashMap<u32, Entry>,
    arrays_by_name: HashMap<String, Entry>,
    handles: HashMap<(Entry, bool), Option<Handle<Image>>>,
    terrain_handles: HashMap<u32, Option<Handle<Image>>>,
    /// Immagini BC7: i dati sono nell'ordine di Unity (riga 0 = in basso), quindi vanno
    /// mostrate con la V capovolta. Le altre (PNG, DXT, ...) sono gia' raddrizzate in `decode_2d`.
    flipped: HashSet<AssetId<Image>>,
    /// Dimensioni in pixel delle immagini 2D. Le immagini `RENDER_WORLD` spariscono da
    /// `Assets<Image>` dopo il primo upload sulla GPU, quindi `images.get()` non basta.
    sizes: HashMap<AssetId<Image>, UVec2>,
    /// Quante texture 2D (di qualsiasi mappa) le mod forniscono per ogni archivio di Daggerfall.
    coverage: HashMap<u32, usize>,
    /// Materiali Unity delle mod (DREAM ecc.), indicizzati per (archivio, record, frame):
    /// (indice mod, nome del materiale). Le loro texture non compaiono fra i nomi delle texture 2D.
    materials_by_key: HashMap<(u32, u32, u32), Entry>,
    /// Normal, emissione e metallico gia' convertiti: ((mod, nome#proprieta'), ordine grezzo) -> immagine.
    map_handles: HashMap<(Entry, bool), Option<Handle<Image>>>,
    pub nearest_filter: bool,
}

/// Le normal map BC7 non si possono decodificare su CPU: Unity le memorizza con X e Y nei canali
/// R e G e Z da ricostruire, quindi usarle come RGB piatto illumina in modo sbagliato. Mettere `true`
/// solo se le tue normal map BC7 hanno gia' Z nel canale blu.
const ASSUME_BC7_NORMALS_ARE_RGB: bool = true;

/// Mappe aggiuntive di un materiale della mod, gia' convertite per `StandardMaterial`.
#[derive(Debug, Clone, Default)]
pub struct HdMaps {
    pub normal: Option<Handle<Image>>,
    /// Metallico nel canale B e rugosita' nel G (convenzione glTF di Bevy).
    pub metallic_roughness: Option<Handle<Image>>,
    pub emission: Option<Handle<Image>>,
}

/// Normale Unity -> RGB di Bevy (X in R, Y in G, Z in B, Y verso l'alto come in OpenGL).
/// Se la mappa e' in DXT5nm (R quasi sempre 255, X nell'alpha) ricostruisce Z. Restituisce true in quel caso.
pub(crate) fn convert_unity_normal(px: &mut [u8]) -> bool {
    let count = (px.len() / 4).max(1) as u64;
    let (mut sum_r, mut sum_a) = (0u64, 0u64);
    for p in px.chunks_exact(4) {
        sum_r += p[0] as u64;
        sum_a += p[3] as u64;
    }
    let dxt5nm = sum_r / count >= 230 && sum_a / count < 250;
    for p in px.chunks_exact_mut(4) {
        if dxt5nm {
            let x = p[3] as f32 / 255.0 * 2.0 - 1.0;
            let y = p[1] as f32 / 255.0 * 2.0 - 1.0;
            let z = (1.0 - x * x - y * y).max(0.0).sqrt();
            p[0] = p[3];
            p[2] = ((z * 0.5 + 0.5) * 255.0).round() as u8;
        }
        p[3] = 255;
    }
    dxt5nm
}

/// `_MetallicGlossMap` di Unity (R = metallico, A = liscio) -> convenzione glTF/Bevy
/// (B = metallico, G = rugosita' = 1 - liscio).
pub(crate) fn convert_unity_metallic(px: &mut [u8]) {
    for p in px.chunks_exact_mut(4) {
        let (metallic, smooth) = (p[0], p[3]);
        p[0] = 255;
        p[1] = 255 - smooth;
        p[2] = metallic;
        p[3] = 255;
    }
}

/// Parametri di un materiale della mod che si traducono in `StandardMaterial`.
#[derive(Debug, Clone, Copy)]
pub struct MaterialParams {
    /// `_Color` (sRGB), normalmente bianco.
    pub color: [f32; 4],
    pub metallic: f32,
    /// `_Glossiness` di Unity (1 = liscio); in Bevy `perceptual_roughness = 1 - glossiness`.
    pub glossiness: f32,
    /// `_EmissionColor` (lineare, puo' superare 1 se HDR), se il materiale lo definisce.
    pub emission: Option<[f32; 3]>,
}

impl ModTextures {
    pub fn add(&mut self, path: &Path) -> dfmod::Result<()> {
        let m = ModArchive::open(path)?;
        let idx = self.archives.len();

        let mut tex_count = 0;
        let mut mod_coverage: HashMap<u32, usize> = HashMap::new();
        let mut unparsed: Vec<String> = Vec::new();
        let mut unparsed_count = 0usize;
        for name in m.texture_names() {
            let entry = (idx, name.to_owned());
            if let Some(k) = TextureKey::parse(name) {
                *mod_coverage.entry(k.archive).or_default() += 1;
                *self.coverage.entry(k.archive).or_default() += 1;
                self.by_key.insert(k, entry.clone());
            } else {
                unparsed_count += 1;
                if unparsed.len() < 6 {
                    unparsed.push(name.to_owned());
                }
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

        let mut material_count = 0usize;
        let mut materials_unparsed: Vec<String> = Vec::new();
        for name in m.material_names() {
            match TextureKey::parse(name) {
                Some(k) if k.kind == MapKind::Albedo => {
                    *mod_coverage.entry(k.archive).or_default() += 1;
                    *self.coverage.entry(k.archive).or_default() += 1;
                    self.materials_by_key.insert((k.archive, k.record, k.frame), (idx, name.to_owned()));
                    material_count += 1;
                }
                _ => {
                    if materials_unparsed.len() < 6 {
                        materials_unparsed.push(name.to_owned());
                    }
                }
            }
        }
        if material_count > 0 || !materials_unparsed.is_empty() {
            info!("  {material_count} materiali con nome archivio_record-frame");
            if !materials_unparsed.is_empty() {
                info!("  altri materiali con nomi diversi, ad es. {materials_unparsed:?}");
            }
        }

        if tex_count > 0 || material_count > 0 {
            let mut top: Vec<(u32, usize)> = mod_coverage.into_iter().collect();
            top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            top.truncate(10);
            let top: Vec<String> = top.iter().map(|(a, n)| format!("{a}({n})")).collect();
            info!("  archivi coperti: {}", top.join(" "));
            if unparsed_count > 0 {
                info!("  {unparsed_count} nomi non nel formato archivio_record-frame, ad es. {unparsed:?}");
            }
        }

        self.archives.push(m);
        Ok(())
    }

    /// Quante texture 2D delle mod riguardano questo archivio di Daggerfall.
    pub fn coverage(&self, archive: u32) -> usize {
        self.coverage.get(&archive).copied().unwrap_or(0)
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
        if let Some(entry) = self.by_key.get(key).cloned() {
            let srgb = !matches!(key.kind, MapKind::Normal | MapKind::MetallicGloss);
            return self.load_2d(entry, srgb, images);
        }
        // Nessuna texture 2D con questo nome: la mod puo' fornirla come materiale Unity.
        self.material_image(key, images)
    }

    /// Texture assegnata dalla mod al materiale (archivio, record, frame): `_MainTex` per l'albedo,
    /// `_BumpMap`, `_EmissionMap`, `_MetallicGlossMap` per le altre mappe.
    fn material_image(&mut self, key: &TextureKey, images: &mut Assets<Image>) -> Option<Handle<Image>> {
        let (idx, mat_name) = self.materials_by_key.get(&(key.archive, key.record, key.frame))?.clone();
        let property = match key.kind {
            MapKind::Albedo => "_MainTex",
            MapKind::Normal => "_BumpMap",
            MapKind::Emission => "_EmissionMap",
            MapKind::MetallicGloss => "_MetallicGlossMap",
        };
        let srgb = matches!(key.kind, MapKind::Albedo | MapKind::Emission);
        let id = ((idx, format!("{mat_name}#{property}")), srgb);
        if let Some(cached) = self.handles.get(&id) {
            return cached.clone();
        }
        let image = (|| {
            let archive = &self.archives[idx];
            let material = match archive.material(&mat_name) {
                Ok(Some(m)) => m,
                     Ok(None) => return None,
                     Err(e) => {
                         warn!("Errore lettura materiale `{mat_name}`: {e}");
                         return None;
                     }
            };
            let texture = match archive.material_texture(&material, property) {
                Ok(Some(t)) => t,
                     Ok(None) => return None,
                     Err(e) => {
                         warn!("Errore texture `{property}` del materiale `{mat_name}`: {e}");
                         return None;
                     }
            };
            self.image_from_texture(&mat_name, texture, srgb)
        })();
        let handle = image.map(|img| self.add_image(img, images));
        self.handles.insert(id, handle.clone());
        handle
    }

    /// Normal, emissione e metallico HD di (archivio, record, frame), convertiti per Bevy.
    /// `raw_order`: l'albedo e' un BC7 nell'ordine di Unity (riga 0 in basso, vedi `needs_v_flip`):
    /// le mappe devono avere lo stesso ordine per condividere la stessa trasformazione UV.
    pub fn material_maps(
        &mut self,
        archive: u32,
        record: u32,
        frame: u32,
        raw_order: bool,
        images: &mut Assets<Image>,
    ) -> HdMaps {
        HdMaps {
            normal: self.material_map(archive, record, frame, MapKind::Normal, raw_order, images),
            metallic_roughness: self.material_map(archive, record, frame, MapKind::MetallicGloss, raw_order, images),
            emission: self.material_map(archive, record, frame, MapKind::Emission, raw_order, images),
        }
    }

    /// Una mappa HD (normal, emissione o metallico) da una texture 2D con suffisso di mappa
    /// oppure dalla proprieta' corrispondente del materiale della mod.
    pub fn material_map(
        &mut self,
        archive: u32,
        record: u32,
        frame: u32,
        kind: MapKind,
        raw_order: bool,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let key = TextureKey { archive, record, frame, kind };
        let property = match kind {
            MapKind::Albedo => "_MainTex",
            MapKind::Normal => "_BumpMap",
            MapKind::Emission => "_EmissionMap",
            MapKind::MetallicGloss => "_MetallicGlossMap",
        };
        let (idx, name, from_material) = if let Some((idx, name)) = self.by_key.get(&key).cloned() {
            (idx, name, false)
        } else {
            let (idx, name) = self.materials_by_key.get(&(archive, record, frame))?.clone();
            (idx, name, true)
        };
        let cache_key = ((idx, format!("{name}#{property}#{from_material}")), raw_order);
        if let Some(cached) = self.map_handles.get(&cache_key) {
            return cached.clone();
        }
        let image = (|| {
            let archive_ref = &self.archives[idx];
            let texture = if from_material {
                let material = archive_ref.material(&name).ok()??;
                archive_ref.material_texture(&material, property).ok()??
            } else {
                archive_ref.texture(&name).ok()??
            };
            self.decode_map(&name, texture, kind, raw_order)
        })();
        let handle = image.map(|img| self.add_image(img, images));
        self.map_handles.insert(cache_key, handle.clone());
        handle
    }

    /// Converte una mappa nel formato di Bevy. I BC7 non si decodificano su CPU: restano utilizzabili
    /// solo come emissione (e come normale se `ASSUME_BC7_NORMALS_ARE_RGB`), e solo nell'ordine grezzo.
    fn decode_map(&self, name: &str, tex: Texture2D, kind: MapKind, raw_order: bool) -> Option<Image> {
        let srgb = matches!(kind, MapKind::Albedo | MapKind::Emission);
        if tex.format == DfFormat::BC7 {
            let usable = raw_order
            && match kind {
                MapKind::Albedo | MapKind::Emission => true,
                MapKind::Normal => ASSUME_BC7_NORMALS_ARE_RGB,
                MapKind::MetallicGloss => false,
            };
            if !usable {
                debug!("mappa {kind:?} di `{name}` in BC7 non usabile (ordine grezzo: {raw_order})");
                return None;
            }
            return self.image_from_texture(name, tex, srgb);
        }
        let (width, height) = (tex.width, tex.height);
        let decoded = if raw_order { tex.to_rgba8() } else { tex.to_rgba8_top_down() };
        let mut px = match decoded {
            Ok(px) => px,
            Err(e) => {
                warn!("Impossibile decodificare la mappa {kind:?} di `{name}` ({}): {e}", tex.format.name());
                return None;
            }
        };
        match kind {
            MapKind::Normal => {
                convert_unity_normal(&mut px);
            }
            MapKind::MetallicGloss => convert_unity_metallic(&mut px),
            _ => {}
        }
        let mut image = Image::new(
            Extent3d { width, height, depth_or_array_layers: 1 },
            TextureDimension::D2,
            px,
            if srgb { BevyFormat::Rgba8UnormSrgb } else { BevyFormat::Rgba8Unorm },
                RenderAssetUsages::RENDER_WORLD,
        );
        image.sampler = if self.nearest_filter { ImageSampler::nearest() } else { ImageSampler::linear() };
        Some(image)
    }

    /// Parametri (colore, metallico, lucido) del materiale della mod per (archivio, record, frame).
    pub fn material_params(&self, archive: u32, record: u32, frame: u32) -> Option<MaterialParams> {
        let (idx, name) = self.materials_by_key.get(&(archive, record, frame))?;
        let material = self.archives[*idx].material(name).ok()??;
        let float = |k: &str| material.floats.get(k).copied();
        Some(MaterialParams {
            color: material.colors.get("_Color").copied().unwrap_or([1.0; 4]),
             metallic: float("_Metallic").unwrap_or(0.0),
             glossiness: float("_Glossiness").or_else(|| float("_Smoothness")).unwrap_or(0.1),
             emission: material.colors.get("_EmissionColor").map(|c| [c[0], c[1], c[2]]),
        })
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

    /// Questa immagine va mostrata con la V capovolta (BC7)? Gli sprite e le icone HD delle mod
    /// DREAM sono BC7; quelli decodificati su CPU (PNG, DXT) sono gia' nel verso giusto.
    pub fn needs_v_flip(&self, handle: &Handle<Image>) -> bool {
        self.flipped.contains(&handle.id())
    }

    /// Dimensioni in pixel di un'immagine caricata da una mod.
    pub fn image_size(&self, handle: &Handle<Image>) -> Option<Vec2> {
        self.sizes.get(&handle.id()).map(|s| s.as_vec2())
    }

    /// Immagine HD di un'icona o di uno sprite di Daggerfall: prima la variante per materiale
    /// (`{archivio}_{record}-{frame}_{materiale}`, come nelle mod delle armi), poi quella generica.
    pub fn item_image(
        &mut self,
        archive: u32,
        record: u32,
        frame: u32,
        material: Option<&str>,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        if let Some(material) = material {
            // Daggerfall Unity numera gli archivi con tre cifre ("083_5-0"): si provano entrambe.
            for name in [
                format!("{archive:03}_{record}-{frame}_{material}"),
                    format!("{archive}_{record}-{frame}_{material}"),
            ] {
                if let Some(handle) = self.image_by_name(&name, images) {
                    return Some(handle);
                }
            }
        }
        self.albedo(archive, record, frame, images)
    }

    /// Mappa albedo HD di (archivio, record, frame): prima la chiave interpretata, poi i nomi
    /// con e senza zeri iniziali nell'archivio.
    pub fn albedo(
        &mut self,
        archive: u32,
        record: u32,
        frame: u32,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let key = TextureKey { archive, record, frame, kind: MapKind::Albedo };
        self.image_by_key(&key, images)
        .or_else(|| self.image_by_name(&format!("{archive:03}_{record}-{frame}"), images))
        .or_else(|| self.image_by_name(&format!("{archive}_{record}-{frame}"), images))
    }

    /// Prima immagine il cui nome (minuscolo) inizia per `prefix`. Fra piu' candidati si
    /// preferisce quello che finisce con uno dei suffissi di `prefer` (nell'ordine dato).
    /// Serve per i file che le mod nominano con estensione o suffissi di materiale diversi.
    pub fn image_by_prefix(
        &mut self,
        prefix: &str,
        prefer: &[String],
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let prefix = prefix.to_ascii_lowercase();
        let chosen = {
            let mut matches: Vec<&String> = self.by_name.keys().filter(|k| k.starts_with(&prefix)).collect();
            if matches.is_empty() {
                return None;
            }
            matches.sort();
            prefer
            .iter()
            .find_map(|p| matches.iter().find(|k| k.ends_with(p.as_str())))
            // senza materiale preferito: la versione generica (nome == prefisso), poi la prima
            .or_else(|| matches.iter().find(|k| ***k == prefix))
            .or_else(|| matches.first())
            .map(|k| k.to_string())?
        };
        self.image_by_name(&chosen, images)
    }

    /// Nomi (minuscoli) che contengono `needle`: per i messaggi diagnostici.
    pub fn names_containing(&self, needle: &str, limit: usize) -> Vec<String> {
        let needle = needle.to_ascii_lowercase();
        let mut found: Vec<String> = self.by_name.keys().filter(|k| k.contains(&needle)).cloned().collect();
        found.sort();
        found.truncate(limit);
        found
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.by_name.keys().map(String::as_str)
    }

    fn load_2d(&mut self, entry: Entry, srgb: bool, images: &mut Assets<Image>) -> Option<Handle<Image>> {
        let id = (entry, srgb);
        if let Some(cached) = self.handles.get(&id) {
            return cached.clone();
        }
        let handle = self.decode_2d(&id.0, srgb).map(|img| self.add_image(img, images));
        self.handles.insert(id, handle.clone());
        handle
    }

    /// Aggiunge un'immagine decodificata ricordando se e' BC7 (da capovolgere) e le sue dimensioni.
    fn add_image(&mut self, img: Image, images: &mut Assets<Image>) -> Handle<Image> {
        let bc7 = matches!(
            img.texture_descriptor.format,
            BevyFormat::Bc7RgbaUnorm | BevyFormat::Bc7RgbaUnormSrgb
        );
        let size = img.texture_descriptor.size;
        let handle = images.add(img);
        if bc7 {
            self.flipped.insert(handle.id());
        }
        self.sizes.insert(handle.id(), UVec2::new(size.width, size.height));
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
        self.image_from_texture(name, tex, srgb)
    }

    /// Normal map caricata come `Rgba8Unorm` (lineare, non sRGB), con il canale
    /// verde girato da DirectX a OpenGL e i pixel riportati alla convenzione
    /// "bottom-up" per coerenza con l'albedo BC7.
    ///
    /// Questo metodo decodifica sempre su CPU, anche se la texture è BC7 nativa.
    /// Il costo è 4 byte per pixel in più di VRAM per ogni normal map, ma permette
    /// di correggere la convenzione del canale verde di DREAM.
    pub fn image_normal_by_key(
        &mut self,
        key: &TextureKey,
        images: &mut Assets<Image>,
    ) -> Option<Handle<Image>> {
        let entry = self.by_key.get(key)?.clone();
        // Chiave separata dalla cache normale (`image_by_key` usa sRGB=true per
        // l'albedo; qui sRGB=false), così non c'è collisione.
        let id = (entry.clone(), false);
        if let Some(cached) = self.handles.get(&id) {
            return cached.clone();
        }

        let result = (|| {
            let tex = self.archives[entry.0].texture(&entry.1).ok()??;
            let mut rgba = tex.to_rgba8_top_down().ok()?;

            // `to_rgba8_top_down` produce pixel con row 0 = top; l'albedo BC7 di DREAM
            // ha row 0 = bottom. Riportiamo la normal map alla stessa convenzione per
            // poter usare lo stesso uv_transform dell'albedo.
            flip_v_in_place(&mut rgba, tex.width as usize, tex.height as usize);

            // DirectX (Y giù) → OpenGL (Y su): inverte il canale verde.
            for chunk in rgba.chunks_exact_mut(4) {
                chunk[1] = 255 - chunk[1];
            }

            let mut image = Image::new(
                Extent3d {
                    width: tex.width,
                    height: tex.height,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                rgba,
                BevyFormat::Rgba8Unorm, // lineare, le normal map non sono sRGB
                BevyRenderAssetUsages::RENDER_WORLD,
            );
            image.sampler = if self.nearest_filter {
                ImageSampler::nearest()
            } else {
                ImageSampler::linear()
            };
            Some(images.add(image))
        })();

        self.handles.insert(id, result.clone());
        result
    }

    /// Converte una `Texture2D` della mod in `Image` di Bevy (BC7 diretto, il resto decodificato su CPU).
    fn image_from_texture(&self, name: &str, tex: Texture2D, srgb: bool) -> Option<Image> {

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dxt5nm_normal_gets_its_z_back() {
        // pixel DXT5nm piatto: x = y = 0 -> R=255, G=128, A=128; Z deve diventare ~255
        let mut px = vec![255, 128, 0, 128, 255, 128, 0, 128];
        assert!(convert_unity_normal(&mut px));
        assert_eq!(&px[0..4], &[128, 128, 255, 255]);
    }

    #[test]
    fn plain_rgb_normal_is_left_alone() {
        let mut px = vec![128, 128, 255, 255, 120, 130, 250, 255];
        assert!(!convert_unity_normal(&mut px));
        assert_eq!(px, [128, 128, 255, 255, 120, 130, 250, 255]);
    }

    #[test]
    fn unity_metallic_map_is_converted_to_gltf_layout() {
        // R = metallico 200, A = liscio 100  ->  B = 200, G = 155
        let mut px = vec![200, 9, 9, 100];
        convert_unity_metallic(&mut px);
        assert_eq!(px, [255, 155, 200, 255]);
    }
}
