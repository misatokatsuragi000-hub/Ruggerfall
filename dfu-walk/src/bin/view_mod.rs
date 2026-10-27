//! Browser ed esportatore di texture per file `.dfmod`.
//!
//! ## Modalità GUI (default)
//! ```
//! cargo run --release --bin view_mod -- <file.dfmod> [filtro]
//! ```
//! Elenca le texture sulla sinistra, anteprima a destra. Il tasto **E** esporta
//! la texture selezionata come PNG in `./exported/<nome>.png`.
//!
//! ## Modalità CLI (headless)
//! ```
//! # elenco nomi (senza aprire la GUI)
//! view_mod <file.dfmod> --list
//! view_mod <file.dfmod> --list --filter 381
//!
//! # esporta una texture singola come PNG (scrive ./381_0-0.png)
//! view_mod <file.dfmod> --export 381_0-0
//! view_mod <file.dfmod> --export 381_0-0 --out ./my.png
//!
//! # esporta tutte le texture che contengono "381" in ./exported/
//! view_mod <file.dfmod> --export-all --filter 381
//! view_mod <file.dfmod> --export-all --filter 381 --out ./exported/
//! ```
//!
//! I PNG sono esportati **top-down** (convenzione standard): puoi aprirli in GIMP o
//! Photoshop, modificarli, salvarli come PNG e metterli nella cartella
//! `StreamingAssets/Textures` del gioco. `ModTextures` li userà come fallback dopo
//! la mod `.dfmod`.

use std::path::{Path, PathBuf};

use bevy::input::mouse::MouseWheel;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat as BevyFormat};
use bevy::window::PrimaryWindow;
use dfmod::{MapKind, ModArchive, Texture2DArray, TextureFormat as DfFormat, TextureKey};

const VISIBLE_ROWS: usize = 22;
/// Cartella dove finiscono i PNG esportati dalla GUI e da `--export-all`.
const EXPORT_DIR: &str = "exported";

// =============================================================================
// AssetItem
// =============================================================================

#[derive(Clone)]
enum AssetItem {
    Tex2D {
        name: String,
        width: u32,
        height: u32,
        format: DfFormat,
            mip_count: u32,
            data: Vec<u8>,
            is_decoded_rgba8: bool,
    },
}

impl AssetItem {
    fn name(&self) -> &str {
        match self {
            Self::Tex2D { name, .. } => name,
        }
    }
    fn width(&self) -> u32 {
        match self {
            Self::Tex2D { width, .. } => *width,
        }
    }
    fn height(&self) -> u32 {
        match self {
            Self::Tex2D { height, .. } => *height,
        }
    }
    fn mips(&self) -> u32 {
        match self {
            Self::Tex2D { mip_count, .. } => *mip_count,
        }
    }
    fn format_info(&self) -> DfFormat {
        match self {
            Self::Tex2D { format, .. } => *format,
        }
    }
}

// =============================================================================
// BrowserState
// =============================================================================

#[derive(Resource)]
struct BrowserState {
    items: Vec<AssetItem>,
    selected_idx: usize,
    scroll_top: usize,
    flip_v: bool,
    needs_update: bool,
    /// Ultimo messaggio di export (successo o errore), mostrato nell'header.
    last_export: Option<String>,
    /// Cartella di output per il tasto E della GUI.
    export_dir: PathBuf,
}

#[derive(Component)]
struct ListRowSlot(usize);
#[derive(Component)]
struct RowIndexText(usize);
#[derive(Component)]
struct RowNameText(usize);
#[derive(Component)]
struct PreviewImageNode;
#[derive(Component)]
struct InfoHeaderText;
#[derive(Component)]
struct PageIndicatorText;

// =============================================================================
// Decodifica / esportazione
// =============================================================================

/// Gira in verticale una matrice RGBA8 in-place (swap delle righe).
fn flip_v(rgba: &mut [u8], w: usize, h: usize) {
    let row = w * 4;
    for y in 0..h / 2 {
        let top = y * row;
        let bot = (h - 1 - y) * row;
        for i in 0..row {
            rgba.swap(top + i, bot + i);
        }
    }
}

/// Decodifica una texture in RGBA8 top-down.
///
/// - Se `is_decoded_rgba8` è true, `data` è già RGBA8 top-down (prodotta da
///   `Texture2D::to_rgba8_top_down`) e viene restituita così com'è.
/// - Altrimenti `data` è nei formati grezzi di Unity (bottom-up): BC7, DXT1, DXT5,
///   BC4, BC5, BC6H, RGBA32. Usa `texture2ddecoder` (che scrive BGRA32 in un
///   buffer `u32`) per i formati compressi, poi converte in RGBA8 e fa flip.
fn decode_to_rgba_top_down(
    data: &[u8],
    format: DfFormat,
        width: u32,
        height: u32,
        is_decoded_rgba8: bool,
) -> Result<Vec<u8>, String> {
    let w = width as usize;
    let h = height as usize;
    if w == 0 || h == 0 {
        return Err(format!("dimensioni non valide: {}x{}", w, h));
    }

    if is_decoded_rgba8 {
        if data.len() < w * h * 4 {
            return Err(format!(
                "dati RGBA8 troppo corti: {} < {}",
                data.len(),
                               w * h * 4
            ));
        }
        return Ok(data[..w * h * 4].to_vec());
    }

    let mip0_size = format
    .mip0_size(w, h)
    .ok_or_else(|| format!("formato {:?} senza dimensione mip0 nota", format))?;
    let mip0 = data
    .get(..mip0_size)
    .ok_or_else(|| format!("dati troppo corti: {} < {}", data.len(), mip0_size))?;

    // texture2ddecoder 0.1.x riempie un buffer di u32 in formato BGRA32.
    let mut bgra32 = vec![0u32; w * h];
    let res: Result<(), &'static str> = match format.0 {
        // BC7 (25) e BC7 lineare Unity (108)
        25 | 108 => texture2ddecoder::decode_bc7(mip0, w, h, &mut bgra32),
        // DXT1 (10) = BC1
        10 => texture2ddecoder::decode_bc1(mip0, w, h, &mut bgra32),
        // DXT5 (12) = BC3
        12 => texture2ddecoder::decode_bc3(mip0, w, h, &mut bgra32),
        // BC4 (26) = R8
        26 => texture2ddecoder::decode_bc4(mip0, w, h, &mut bgra32),
        // BC5 (27) = RG8
        27 => texture2ddecoder::decode_bc5(mip0, w, h, &mut bgra32),
        // BC6H (24) = HDR unsigned
        24 => texture2ddecoder::decode_bc6_unsigned(mip0, w, h, &mut bgra32),
        // RGBA32 (4): già RGBA8, ma ancora in ordine Unity (bottom-up).
        // Lo passiamo per lo stesso percorso BGRA32 così il flip finale
        // porta tutto in top-down come gli altri.
        4 => {
            if mip0.len() < w * h * 4 {
                return Err("dati RGBA32 troppo corti".to_string());
            }
            for i in 0..w * h {
                let r = mip0[i * 4] as u32;
                let g = mip0[i * 4 + 1] as u32;
                let b = mip0[i * 4 + 2] as u32;
                let a = mip0[i * 4 + 3] as u32;
                bgra32[i] = (a << 24) | (r << 16) | (g << 8) | b;
            }
            Ok(())
        }
        other => {
            return Err(format!(
                "formato {} ({}) non supportato per export",
                               other,
                               format.name()
            ))
        }
    };
    res.map_err(|e| format!("decodifica {}: {e}", format.name()))?;

    // BGRA32 → RGBA8
    let mut rgba = vec![0u8; w * h * 4];
    for (i, &p) in bgra32.iter().enumerate() {
        rgba[i * 4] = ((p >> 16) & 0xFF) as u8; // R
        rgba[i * 4 + 1] = ((p >> 8) & 0xFF) as u8; // G
        rgba[i * 4 + 2] = (p & 0xFF) as u8; // B
        rgba[i * 4 + 3] = ((p >> 24) & 0xFF) as u8; // A
    }

    // I formati di Unity sono bottom-up: flip per avere un PNG standard top-down.
    flip_v(&mut rgba, w, h);
    Ok(rgba)
}

/// Salva un buffer RGBA8 come PNG.
fn save_png(path: &Path, rgba: &[u8], w: u32, h: u32) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let img = image::RgbaImage::from_raw(w, h, rgba.to_vec())
    .ok_or_else(|| "dimensioni non valide per l'immagine".to_string())?;
    img.save(path).map_err(|e| e.to_string())
}

/// Esporta un `AssetItem` come PNG. Usato dalla GUI (tasto E).
fn export_asset_item(item: &AssetItem, out_path: &Path) -> Result<(), String> {
    match item {
        AssetItem::Tex2D {
            width,
            height,
            format,
                data,
                is_decoded_rgba8,
                ..
        } => {
            let rgba = decode_to_rgba_top_down(data, *format, *width, *height, *is_decoded_rgba8)?;
            save_png(out_path, &rgba, *width, *height)
        }
    }
}

/// Esporta una `Texture2D` della mod come PNG. Usato dal CLI.
fn export_texture2d(tex: &dfmod::Texture2D, out_path: &Path) -> Result<(), String> {
    let format = tex.format;
    let (w, h) = (tex.width, tex.height);

    let rgba_top_down = match format.0 {
        // Formati che la crate `dfmod` sa già decodificare in RGBA8 top-down.
        1 | 3 | 4 | 5 | 7 | 10 | 12 | 13 | 14 => tex
        .to_rgba8_top_down()
        .map_err(|e| format!("decode {}: {e}", format.name()))?,
        // Formati compressi che decodifichiamo con texture2ddecoder.
        24 | 25 | 26 | 27 | 108 => decode_to_rgba_top_down(&tex.data, format, w, h, false)?,
        other => {
            return Err(format!(
                "formato {} ({}) non supportato",
                               other,
                               format.name()
            ))
        }
    };
    save_png(out_path, &rgba_top_down, w, h)
}

/// Esporta una `Texture2DArray`: una PNG per ogni slice, nome `<base>[N].png`.
fn export_texture_array(
    arr: &Texture2DArray,
    out_dir: &Path,
    base_name: &str,
) -> Result<usize, String> {
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let (w, h) = (arr.width, arr.height);
    let mip0_size = arr
    .format
    .mip0_size(w as usize, h as usize)
    .ok_or_else(|| format!("formato array {:?} senza mip0", arr.format))?;
    if mip0_size == 0 {
        return Err("mip0 vuoto".to_string());
    }
    let n_slices = arr.depth as usize;
    if arr.data.len() < mip0_size * n_slices {
        return Err(format!(
            "dati array troppo corti: {} < {}",
            arr.data.len(),
                           mip0_size * n_slices
        ));
    }

    let mut written = 0usize;
    for slice in 0..n_slices {
        let start = slice * mip0_size;
        let end = start + mip0_size;
        let slice_data = &arr.data[start..end];
        let rgba = decode_to_rgba_top_down(slice_data, arr.format, w, h, false)?;
        let out = out_dir.join(format!("{base_name}[{slice}].png"));
        save_png(&out, &rgba, w, h)?;
        written += 1;
    }
    Ok(written)
}

/// Nome sicuro per il filesystem.
fn sanitize_name(name: &str) -> String {
    name.chars()
    .map(|c| match c {
        '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
         c => c,
    })
    .collect()
}

// =============================================================================
// CLI: list / export-one / export-all
// =============================================================================

fn run_list(archive_path: &Path, filter: Option<&str>) {
    let archive = match ModArchive::open(archive_path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Errore apertura mod {}: {e}", archive_path.display());
            std::process::exit(1);
        }
    };
    let f = filter.map(|s| s.to_ascii_lowercase());
    let matches = |n: &str| f.as_deref().map_or(true, |f| n.contains(f));

    println!("--- TEXTURE 2D ---");
    for name in archive.texture_names() {
        if matches(name) {
            println!("{name}");
        }
    }
    println!("\n--- TEXTURE ARRAY ---");
    for name in archive.texture_array_names() {
        if matches(name) {
            if let Ok(Some(arr)) = archive.texture_array(name) {
                for slice in 0..arr.depth as usize {
                    println!(
                        "{name}[{slice}] ({}x{}, {:?})",
                             arr.width, arr.height, arr.format
                    );
                }
            }
        }
    }
}

fn run_export_one(archive_path: &Path, name: &str, out_path: Option<&Path>) {
    let archive = match ModArchive::open(archive_path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Errore apertura mod {}: {e}", archive_path.display());
            std::process::exit(1);
        }
    };

    // 1. Provo come texture 2D
    if let Ok(Some(tex)) = archive.texture(name) {
        let out = out_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(format!("{}.png", sanitize_name(name))));
        match export_texture2d(&tex, &out) {
            Ok(()) => {
                println!(
                    "Esportato {} -> {} ({}x{}, {:?})",
                         name,
                         out.display(),
                         tex.width,
                         tex.height,
                         tex.format
                );
                return;
            }
            Err(e) => {
                eprintln!("Errore export {name}: {e}");
                std::process::exit(1);
            }
        }
    }

    // 2. Provo come array
    if let Ok(Some(arr)) = archive.texture_array(name) {
        let out_dir = out_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from(EXPORT_DIR));
        match export_texture_array(&arr, &out_dir, &sanitize_name(name)) {
            Ok(n) => {
                println!(
                    "Esportate {} slice di {} in {} ({}x{}, {:?})",
                         n,
                         name,
                         out_dir.display(),
                         arr.width,
                         arr.height,
                         arr.format
                );
                return;
            }
            Err(e) => {
                eprintln!("Errore export array {name}: {e}");
                std::process::exit(1);
            }
        }
    }

    // 3. Cerco un materiale con quel nome
    if let Ok(Some(mat)) = archive.material(name) {
        match archive.material_texture(&mat, "_MainTex") {
            Ok(Some(tex)) => {
                let out = out_path
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from(format!("{}.png", sanitize_name(name))));
                match export_texture2d(&tex, &out) {
                    Ok(()) => {
                        println!(
                            "Esportato (da materiale) {} -> {} ({}x{}, {:?})",
                                 name,
                                 out.display(),
                                 tex.width,
                                 tex.height,
                                 tex.format
                        );
                        return;
                    }
                    Err(e) => {
                        eprintln!("Errore export materiale {name}: {e}");
                        std::process::exit(1);
                    }
                }
            }
            Ok(None) => {
                eprintln!("Materiale '{name}' senza _MainTex");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("Errore lettura materiale '{name}': {e}");
                std::process::exit(1);
            }
        }
    }

    eprintln!("'{name}' non trovato (né texture 2D, né array, né materiale)");
    std::process::exit(1);
}

fn run_export_all(archive_path: &Path, filter: Option<&str>, out_dir: Option<&Path>) {
    let archive = match ModArchive::open(archive_path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Errore apertura mod {}: {e}", archive_path.display());
            std::process::exit(1);
        }
    };
    let out_root = out_dir
    .map(|p| p.to_path_buf())
    .unwrap_or_else(|| PathBuf::from(EXPORT_DIR));
    if let Err(e) = std::fs::create_dir_all(&out_root) {
        eprintln!("Impossibile creare {}: {e}", out_root.display());
        std::process::exit(1);
    }
    let f = filter.map(|s| s.to_ascii_lowercase());
    let matches = |n: &str| f.as_deref().map_or(true, |f| n.contains(f));

    let mut exported = 0usize;
    let mut failed = 0usize;

    // Texture 2D
    let names: Vec<String> = archive
    .texture_names()
    .filter(|n| matches(n))
    .map(str::to_string)
    .collect();
    for name in &names {
        let Some(tex) = archive.texture(name).ok().flatten() else {
            failed += 1;
            continue;
        };
        let out = out_root.join(format!("{}.png", sanitize_name(name)));
        match export_texture2d(&tex, &out) {
            Ok(()) => {
                println!("  OK  {name} -> {}", out.display());
                exported += 1;
            }
            Err(e) => {
                eprintln!("  ERR {name}: {e}");
                failed += 1;
            }
        }
    }

    // Texture array
    let arr_names: Vec<String> = archive
    .texture_array_names()
    .filter(|n| matches(n))
    .map(str::to_string)
    .collect();
    for name in &arr_names {
        let Some(arr) = archive.texture_array(name).ok().flatten() else {
            failed += 1;
            continue;
        };
        let sub_dir = out_root.join(sanitize_name(name));
        match export_texture_array(&arr, &sub_dir, &sanitize_name(name)) {
            Ok(n) => {
                println!("  OK  {name}: {n} slice in {}", sub_dir.display());
                exported += n;
            }
            Err(e) => {
                eprintln!("  ERR {name}: {e}");
                failed += 1;
            }
        }
    }

    println!("\nTotale esportati: {exported}, falliti: {failed}");
    println!("Cartella: {}", out_root.display());
}

// =============================================================================
// GUI
// =============================================================================

/// Raccoglie tutte le texture in `AssetItem` come fa la vecchia GUI.
fn collect_items(archive: &ModArchive, filter: Option<&str>) -> (Vec<AssetItem>, usize) {
    let mut items = Vec::new();
    let mut skipped = 0usize;
    let f = filter.map(|s| s.to_ascii_lowercase());
    let matches = |n: &str| f.as_deref().map_or(true, |f| n.contains(f));

    // Texture2D singole
    for name in archive.texture_names() {
        if !matches(name) {
            continue;
        }
        let tex = match archive.texture(name) {
            Ok(Some(t)) => t,
            Ok(None) => continue,
            Err(e) => {
                eprintln!("Skipping '{}': {e}", name);
                skipped += 1;
                continue;
            }
        };
        let can_gpu_load = bevy_format_direct(tex.format).is_some();
        let (format, data, is_decoded) = if can_gpu_load {
            (tex.format, tex.data, false)
        } else {
            match tex.to_rgba8_top_down() {
                Ok(rgba) => (DfFormat::RGBA32, rgba, true),
                Err(_) => {
                    eprintln!(
                        "Skipping '{}': formato {:?} non supportato e decode CPU fallito",
                        name, tex.format
                    );
                    skipped += 1;
                    continue;
                }
            }
        };
        items.push(AssetItem::Tex2D {
            name: name.to_owned(),
                   width: tex.width,
                   height: tex.height,
                   format,
                       mip_count: tex.mip_count,
                       data,
                       is_decoded_rgba8: is_decoded,
        });
    }

    // Texture2DArray: espanse in slice
    for name in archive.texture_array_names() {
        if !matches(name) {
            continue;
        }
        let arr = match archive.texture_array(name) {
            Ok(Some(a)) => a,
            Ok(None) => continue,
            Err(e) => {
                eprintln!("Skipping array '{}': {e}", name);
                skipped += 1;
                continue;
            }
        };
        if bevy_format_direct(arr.format).is_none() {
            eprintln!(
                "Skipping array '{}': formato {:?} non supportato",
                name, arr.format
            );
            skipped += 1;
            continue;
        }
        let slice_size_bytes =
        total_mip_size_compressed(arr.width, arr.height, arr.mip_count, arr.format).unwrap_or(0);
        if slice_size_bytes == 0 || arr.data.len() < slice_size_bytes * arr.depth as usize {
            eprintln!("Skipping array '{}': data size mismatch", name);
            skipped += 1;
            continue;
        }
        for s_idx in 0..arr.depth as usize {
            let start = s_idx * slice_size_bytes;
            let end = start + slice_size_bytes;
            let slice_data = arr.data[start..end].to_vec();
            items.push(AssetItem::Tex2D {
                name: format!("{}[{}]", name, s_idx),
                       width: arr.width,
                       height: arr.height,
                       format: arr.format,
                           mip_count: arr.mip_count,
                           data: slice_data,
                           is_decoded_rgba8: false,
            });
        }
    }

    items.sort_by(|a, b| a.name().cmp(b.name()));
    (items, skipped)
}

fn bevy_format_direct(format: DfFormat) -> Option<BevyFormat> {
    if format == DfFormat(108) {
        return Some(BevyFormat::Bc7RgbaUnorm);
    }
    match format {
        DfFormat::BC7 => Some(BevyFormat::Bc7RgbaUnormSrgb),
        DfFormat::DXT1 => Some(BevyFormat::Bc1RgbaUnormSrgb),
        DfFormat::DXT5 => Some(BevyFormat::Bc3RgbaUnormSrgb),
        DfFormat::BC4 => Some(BevyFormat::Bc4RUnorm),
        DfFormat::BC5 => Some(BevyFormat::Bc5RgUnorm),
        DfFormat::BC6H => Some(BevyFormat::Bc6hRgbUfloat),
        DfFormat::RGBA32 => Some(BevyFormat::Rgba8UnormSrgb),
        _ => None,
    }
}

fn mip_size_compressed(w: u32, h: u32, format: DfFormat) -> Option<usize> {
    let block_size: usize = match format {
        DfFormat::BC7 | DfFormat(108) => 16,
        DfFormat::DXT1 | DfFormat::BC4 => 8,
        DfFormat::DXT5 | DfFormat::BC5 | DfFormat::BC6H => 16,
        _ => return None,
    };
    let bw = ((w + 3) / 4) as usize;
    let bh = ((h + 3) / 4) as usize;
    Some(bw * bh * block_size)
}

fn mip_size_rgba8(w: u32, h: u32) -> usize {
    (w as usize) * (h as usize) * 4
}

fn total_mip_size_compressed(w: u32, h: u32, mips: u32, format: DfFormat) -> Option<usize> {
    let mut total = 0usize;
    let (mut mw, mut mh) = (w, h);
    for _ in 0..mips.max(1) {
        total += mip_size_compressed(mw, mh, format)?;
        mw = (mw / 2).max(1);
        mh = (mh / 2).max(1);
    }
    Some(total)
}

fn safe_image_raw(
    w: u32,
    h: u32,
    format: DfFormat,
        mips: u32,
        mut data: Vec<u8>,
        is_rgba8: bool,
) -> Option<Image> {
    let bevy_fmt = if is_rgba8 {
        BevyFormat::Rgba8UnormSrgb
    } else {
        bevy_format_direct(format)?
    };
    let expected_mip0 = if is_rgba8 {
        mip_size_rgba8(w, h)
    } else {
        mip_size_compressed(w, h, format)?
    };
    if expected_mip0 == 0 || data.is_empty() {
        return None;
    }
    if data.len() < expected_mip0 {
        data.resize(expected_mip0, 0);
    }
    let mut final_mips = 1u32;
    if !is_rgba8 && mips > 1 {
        let mut acc = 0usize;
        let (mut mw, mut mh) = (w, h);
        for i in 0..mips {
            let ms = mip_size_compressed(mw, mh, format)?;
            if acc + ms > data.len() {
                break;
            }
            acc += ms;
            final_mips = i + 1;
            mw = (mw / 2).max(1);
            mh = (mh / 2).max(1);
        }
        data.truncate(acc);
    } else if is_rgba8 {
        data.truncate(expected_mip0);
    }
    let mut img = Image::new(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        bevy_fmt,
        RenderAssetUsages::default(),
    );
    img.texture_descriptor.mip_level_count = final_mips;
    Some(img)
}

fn run_gui(archive_path: &Path, filter: Option<&str>) {
    let archive = match ModArchive::open(archive_path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Errore apertura mod {}: {e}", archive_path.display());
            std::process::exit(1);
        }
    };

    let (items, skipped) = collect_items(&archive, filter);
    if items.is_empty() {
        eprintln!("Nessuna texture trovata ({} saltate).", skipped);
        std::process::exit(0);
    }
    println!(
        "Trovati {} asset ({} saltati). Avvio interfaccia... (E = esporta PNG)",
             items.len(),
             skipped
    );

    App::new()
    .add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "DFMod Texture Browser (E = esporta)".into(),
                             resolution: (1360.0_f32, 820.0_f32).into(),
                             ..default()
        }),
        ..default()
    }))
    .insert_resource(ClearColor(Color::srgb(0.09, 0.09, 0.11)))
    .insert_resource(BrowserState {
        items,
        selected_idx: 0,
        scroll_top: 0,
        flip_v: true,
        needs_update: true,
        last_export: None,
        export_dir: PathBuf::from(EXPORT_DIR),
    })
    .add_systems(Startup, setup_gui)
    .add_systems(Update, (handle_input, handle_clicks, update_ui_system))
    .run();
}

fn setup_gui(mut commands: Commands, assets: Res<AssetServer>) {
    let font: Handle<Font> = assets.load("fonts/Sherwood.ttf");
    commands.spawn(Camera2d::default());
    commands
    .spawn(Node {
        width: Val::Percent(100.0),
           height: Val::Percent(100.0),
           flex_direction: FlexDirection::Row,
           ..default()
    })
    .with_children(|root| {
        root.spawn((
            Node {
                width: Val::Px(350.0),
                    height: Val::Percent(100.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(10.0)),
                    row_gap: Val::Px(4.0),
                    border: UiRect::right(Val::Px(2.0)),
                    ..default()
            },
            BackgroundColor(Color::srgb(0.12, 0.12, 0.15)),
                    BorderColor(Color::srgb(0.2, 0.2, 0.25)),
        ))
        .with_children(|sidebar| {
            sidebar.spawn((
                Text::new("TEXTURES EXPLORER"),
                           TextFont {
                               font_size: 16.0,
                               ..default()
                           },
                           TextColor(Color::srgb(0.9, 0.75, 0.3)),
            ));
            sidebar.spawn((
                Text::new(""),
                           TextFont {
                               font_size: 12.0,
                               ..default()
                           },
                           TextColor(Color::srgb(0.6, 0.6, 0.65)),
                           PageIndicatorText,
            ));
            for i in 0..VISIBLE_ROWS {
                sidebar
                .spawn((
                    Button,
                    ListRowSlot(i),
                        Node {
                            width: Val::Percent(100.0),
                        height: Val::Px(28.0),
                        padding: UiRect::horizontal(Val::Px(8.0)),
                        align_items: AlignItems::Center,
                        justify_content: JustifyContent::SpaceBetween,
                        border: UiRect::all(Val::Px(1.0)),
                        ..default()
                        },
                        BackgroundColor(Color::srgb(0.15, 0.15, 0.19)),
                        BorderColor(Color::NONE),
                ))
                .with_children(|row| {
                    row.spawn((
                        Text::new(""),
                               TextFont {
                                   font_size: 11.0,
                                   ..default()
                               },
                               TextColor(Color::srgb(0.5, 0.5, 0.55)),
                               RowIndexText(i),
                    ));
                    row.spawn((
                        Text::new(""),
                               TextFont {
                                   font_size: 13.0,
                                   ..default()
                               },
                               TextColor(Color::WHITE),
                               RowNameText(i),
                    ));
                });
            }
        });

        root.spawn((
            Node {
                flex_grow: 1.0,
                height: Val::Percent(100.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(16.0)),
                    justify_content: JustifyContent::SpaceBetween,
                    ..default()
            },
            BackgroundColor(Color::srgb(0.08, 0.08, 0.10)),
        ))
        .with_children(|preview_panel| {
            preview_panel
            .spawn((
                Node {
                    width: Val::Percent(100.0),
                    padding: UiRect::all(Val::Px(12.0)),
                    flex_direction: FlexDirection::Column,
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(Color::srgb(0.13, 0.13, 0.17)),
                    BorderColor(Color::srgb(0.22, 0.22, 0.28)),
            ))
            .with_children(|info_box| {
                info_box.spawn((
                    Text::new("Caricamento..."),
                                TextFont {
                                    font_size: 14.0,
                                    ..default()
                                },
                                TextColor(Color::srgb(0.95, 0.95, 0.95)),
                                InfoHeaderText,
                ));
            });
            preview_panel
            .spawn(Node {
                width: Val::Percent(100.0),
                   height: Val::Percent(78.0),
                   justify_content: JustifyContent::Center,
                   align_items: AlignItems::Center,
                   ..default()
            })
            .with_children(|img_container| {
                img_container.spawn((
                    ImageNode::default(),
                                     Node {
                                         max_width: Val::Percent(92.0),
                                     max_height: Val::Percent(92.0),
                                     align_self: AlignSelf::Center,
                                     ..default()
                                     },
                                     PreviewImageNode,
                ));
            });
            preview_panel.spawn((
                Text::new(
                    "Controlli: [Click/Rotella] Sfoglia | [Su/Giù] Seleziona | \
[V] Capovolgi | [E] Esporta PNG | [Esc] Esci",
                ),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.55, 0.55, 0.6)),
            ));
        });
    });
}

fn handle_input(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut mouse_wheel: EventReader<MouseWheel>,
    mut state: ResMut<BrowserState>,
) {
    let len = state.items.len();
    let mut changed = false;

    for ev in mouse_wheel.read() {
        if ev.y > 0.0 {
            if state.scroll_top > 0 {
                state.scroll_top = state.scroll_top.saturating_sub(3);
                changed = true;
            }
        } else if ev.y < 0.0 {
            if state.scroll_top + VISIBLE_ROWS < len {
                state.scroll_top = (state.scroll_top + 3).min(len.saturating_sub(VISIBLE_ROWS));
                changed = true;
            }
        }
    }

    if keyboard.just_pressed(KeyCode::ArrowDown) || keyboard.just_pressed(KeyCode::KeyS) {
        if state.selected_idx + 1 < len {
            state.selected_idx += 1;
            if state.selected_idx >= state.scroll_top + VISIBLE_ROWS {
                state.scroll_top = state.selected_idx - VISIBLE_ROWS + 1;
            }
            changed = true;
        }
    }
    if keyboard.just_pressed(KeyCode::ArrowUp) || keyboard.just_pressed(KeyCode::KeyW) {
        if state.selected_idx > 0 {
            state.selected_idx -= 1;
            if state.selected_idx < state.scroll_top {
                state.scroll_top = state.selected_idx;
            }
            changed = true;
        }
    }

    if keyboard.just_pressed(KeyCode::KeyV) {
        state.flip_v = !state.flip_v;
        changed = true;
    }

    if keyboard.just_pressed(KeyCode::KeyE) {
        if let Some(item) = state.items.get(state.selected_idx) {
            let safe_name = sanitize_name(item.name());
            let path = state.export_dir.join(format!("{}.png", safe_name));
            match export_asset_item(item, &path) {
                Ok(()) => {
                    let msg = format!("Esportato: {}", path.display());
                    info!("{msg}");
                    state.last_export = Some(msg);
                }
                Err(e) => {
                    let msg = format!("Errore export: {e}");
                    warn!("{msg}");
                    state.last_export = Some(msg);
                }
            }
            changed = true;
        }
    }

    if keyboard.just_pressed(KeyCode::Escape) {
        std::process::exit(0);
    }

    if changed {
        state.needs_update = true;
    }
}

fn handle_clicks(
    mut interaction_query: Query<(&Interaction, &ListRowSlot), (Changed<Interaction>, With<Button>)>,
                 mut state: ResMut<BrowserState>,
) {
    for (interaction, slot) in &mut interaction_query {
        if *interaction == Interaction::Pressed {
            let actual_idx = state.scroll_top + slot.0;
            if actual_idx < state.items.len() && actual_idx != state.selected_idx {
                state.selected_idx = actual_idx;
                state.needs_update = true;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn update_ui_system(
    mut state: ResMut<BrowserState>,
    mut images: ResMut<Assets<Image>>,
    mut image_node_query: Query<(&mut ImageNode, &mut Node), With<PreviewImageNode>>,
                    mut info_query: Query<
                    &mut Text,
                    (
                        With<InfoHeaderText>,
                     Without<RowIndexText>,
                     Without<RowNameText>,
                     Without<PageIndicatorText>,
                    ),
                    >,
                    mut page_query: Query<
                    &mut Text,
                    (
                        With<PageIndicatorText>,
                     Without<InfoHeaderText>,
                     Without<RowIndexText>,
                     Without<RowNameText>,
                    ),
                    >,
                    mut row_buttons: Query<(&ListRowSlot, &mut BackgroundColor, &mut Visibility), With<Button>>,
                    mut idx_texts: Query<
                    (&RowIndexText, &mut Text),
                    (Without<InfoHeaderText>, Without<RowNameText>, Without<PageIndicatorText>),
                    >,
                    mut name_texts: Query<
                    (&RowNameText, &mut Text),
                    (Without<InfoHeaderText>, Without<RowIndexText>, Without<PageIndicatorText>),
                    >,
                    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if !state.needs_update {
        return;
    }
    state.needs_update = false;

    let len = state.items.len();
    let sel = state.selected_idx;
    let top = state.scroll_top;

    if let Ok(mut txt) = page_query.get_single_mut() {
        **txt = format!(
            "Totale: {} | Da {} a {}",
            len,
            top + 1,
            (top + VISIBLE_ROWS).min(len)
        );
    }

    for (slot, mut bg, mut vis) in &mut row_buttons {
        let item_idx = top + slot.0;
        if item_idx < len {
            *vis = Visibility::Inherited;
            let is_sel = item_idx == sel;
            *bg = if is_sel {
                BackgroundColor(Color::srgb(0.18, 0.38, 0.75))
            } else if slot.0 % 2 == 0 {
                BackgroundColor(Color::srgb(0.15, 0.15, 0.19))
            } else {
                BackgroundColor(Color::srgb(0.13, 0.13, 0.16))
            };
        } else {
            *vis = Visibility::Hidden;
        }
    }

    for (slot_idx, mut txt) in &mut idx_texts {
        let item_idx = top + slot_idx.0;
        if item_idx < len {
            **txt = format!("#{:04}", item_idx + 1);
        }
    }

    for (slot_name, mut txt) in &mut name_texts {
        let item_idx = top + slot_name.0;
        if item_idx < len {
            let item = &state.items[item_idx];
            **txt = item.name().to_string();
        }
    }

    let item = &state.items[sel];

    let image_handle = match item {
        AssetItem::Tex2D {
            width,
            height,
            format,
                mip_count,
                data,
                is_decoded_rgba8,
                ..
        } => safe_image_raw(*width, *height, *format, *mip_count, data.clone(), *is_decoded_rgba8)
        .map(|img| images.add(img)),
    };

    if let Some(handle) = image_handle {
        if let Ok((mut img_node, mut node)) = image_node_query.get_single_mut() {
            img_node.image = handle;
            img_node.flip_y = state.flip_v;
            node.aspect_ratio = if item.height() > 0 {
                Some(item.width() as f32 / item.height() as f32)
            } else {
                None
            };
        }
    } else {
        let placeholder = Image::new(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            vec![255, 0, 0, 255],
            BevyFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        if let Ok((mut img_node, _)) = image_node_query.get_single_mut() {
            img_node.image = images.add(placeholder);
        }
    }

    if let Ok(mut txt) = info_query.get_single_mut() {
        let fmt_name = format!("{:?}", item.format_info());
        let export_line = state
        .last_export
        .as_deref()
        .map(|m| format!("\n{}", m))
        .unwrap_or_default();
        **txt = format!(
            "Nome: {}  |  Tipo: Texture2D\n\
Risoluzione: {} x {}  |  Formato: {}  |  Mipmaps: {}{}",
item.name(),
                        item.width(),
                        item.height(),
                        fmt_name,
                        item.mips(),
                        export_line,
        );
    }

    if let Ok(mut win) = windows.get_single_mut() {
        win.title = format!("DFMod Browser - [{}/{}] {}", sel + 1, len, item.name());
    }
}

// =============================================================================
// main
// =============================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Gui,
    List,
    ExportOne,
    ExportAll,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let mut archive_path: Option<PathBuf> = None;
    let mut mode = Mode::Gui;
    let mut export_name: Option<String> = None;
    let mut filter: Option<String> = None;
    let mut out_path: Option<PathBuf> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--list" => mode = Mode::List,
            "--export" => {
                if i + 1 < args.len() {
                    export_name = Some(args[i + 1].clone());
                    mode = Mode::ExportOne;
                    i += 1;
                }
            }
            "--export-all" => {
                mode = Mode::ExportAll;
            }
            "--filter" => {
                if i + 1 < args.len() {
                    filter = Some(args[i + 1].clone());
                    i += 1;
                }
            }
            "--out" => {
                if i + 1 < args.len() {
                    out_path = Some(PathBuf::from(&args[i + 1]));
                    i += 1;
                }
            }
            s if !s.starts_with("--") && archive_path.is_none() => {
                archive_path = Some(PathBuf::from(s));
            }
            _ => {}
        }
        i += 1;
    }

    let Some(archive_path) = archive_path else {
        eprintln!(
            "Uso: view_mod <file.dfmod> [--list | --export <nome> | --export-all] \
[--filter <str>] [--out <path>]"
        );
        std::process::exit(1);
    };

    match mode {
        Mode::List => run_list(&archive_path, filter.as_deref()),
        Mode::ExportOne => {
            let name = export_name.as_deref().unwrap_or_else(|| {
                eprintln!("--export vuole un nome");
                std::process::exit(1);
            });
            run_export_one(&archive_path, name, out_path.as_deref());
        }
        Mode::ExportAll => run_export_all(&archive_path, filter.as_deref(), out_path.as_deref()),
        Mode::Gui => run_gui(&archive_path, filter.as_deref()),
    }
}

// =============================================================================
// Test
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flip_v_swaps_top_and_bottom_rows() {
        // 3x3, pixel: riga 0 = [1,2,3], riga 1 = [4,5,6], riga 2 = [7,8,9]
        let mut px = vec![
            1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255,
            4, 0, 0, 255, 5, 0, 0, 255, 6, 0, 0, 255,
            7, 0, 0, 255, 8, 0, 0, 255, 9, 0, 0, 255,
        ];
        flip_v(&mut px, 3, 3);
        assert_eq!(px[0], 7);
        assert_eq!(px[4], 8);
        assert_eq!(px[8], 9);
        assert_eq!(px[12], 4);
        assert_eq!(px[16], 5);
        assert_eq!(px[20], 6);
        assert_eq!(px[24], 1);
        assert_eq!(px[28], 2);
        assert_eq!(px[32], 3);
    }

    #[test]
    fn sanitize_name_removes_unsafe_chars() {
        assert_eq!(sanitize_name("381_0-0"), "381_0-0");
        assert_eq!(sanitize_name("a/b:c*?"), "a_b_c__");
    }

    #[test]
    fn decode_rgba8_passthrough_returns_top_down() {
        // 1x2: pixel alto = 10, pixel basso = 20 (in una PNG top-down)
        let data = vec![10, 0, 0, 255, 20, 0, 0, 255];
        let out = decode_to_rgba_top_down(&data, DfFormat::RGBA32, 1, 2, true).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn decode_rgba32_compressed_bottom_up_gets_flipped() {
        // Simuliamo una Texture2D RGBA32 in ordine Unity (bottom-up):
        // primo pixel nei dati = riga in basso.
        let data = vec![
            20, 0, 0, 255, // riga in basso (in ordine Unity)
            10, 0, 0, 255, // riga in alto
        ];
        let out = decode_to_rgba_top_down(&data, DfFormat::RGBA32, 1, 2, false).unwrap();
        // Dopo il flip: riga 0 (top-down) = 10, riga 1 = 20
        assert_eq!(out[0], 10);
        assert_eq!(out[4], 20);
    }
}
