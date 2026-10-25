//! Inventory and Equipment menu (Key C): Native paperdoll (110x184 px),
//! graphical backpack grid, hover tooltip popups, and wearable equipment.
//!
//! Controls while open:
//!   [Click / 1..9] Equip or unequip item
//!   [Hover mouse] Show item details popup
//!   [R] Change race  ·  [Tab] Change gender  ·  [V] Change face  ·  [C / Esc] Close

use crate::data::{archive_file_name, find_file_ci, Palette, TextureFile};
use crate::dialog::DialogState;
use crate::mod_textures::ModTextures;
use crate::world::GameData;
use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::{CursorGrabMode, PrimaryWindow};
use std::collections::HashMap;

const DOLL_PALETTE: &str = "PAL.PAL";

// Native Daggerfall / DFU Paperdoll dimensions
pub const DOLL_WIDTH: usize = 110;
pub const DOLL_HEIGHT: usize = 184;

// --- UI Styling & Colors: Dark Fantasy Semitrasparente ---
const OVERLAY: Color = Color::srgba(0.02, 0.015, 0.01, 0.45);          // Velo scuro leggero sul mondo di gioco
const PANEL: Color = Color::srgba(0.10, 0.075, 0.055, 0.88);           // Noce e cuoio semi-trasparente (alpha 0.72)
const PANEL_BORDER: Color = Color::srgba(0.75, 0.58, 0.28, 0.50);      // Bordo in ottone lucido
const CARD: Color = Color::srgba(0.12, 0.09, 0.065, 0.75);             // Schede interne traslucide (alpha 0.55)
const SLOT_BG: Color = Color::srgba(0.0, 0.0, 0.0, 0.35);              // Incavi slot
const SLOT_BORDER: Color = Color::srgba(0.60, 0.45, 0.22, 0.40);       // Bordi slot
const ACCENT: Color = Color::srgb(0.96, 0.78, 0.32);                   // Oro splendente
const EQUIPPED_GREEN: Color = Color::srgb(0.40, 0.88, 0.45);           // Verde smeraldo per equipaggiato
const TEXT: Color = Color::srgb(0.96, 0.93, 0.85);                     // Pergamena chiara
const MUTED: Color = Color::srgb(0.74, 0.67, 0.54);                    // Pergamena antica / sabbia calda

const DOLL_HEIGHT_VH: f32 = 50.0;
const DEFAULT_DOLL_ASPECT: f32 = DOLL_WIDTH as f32 / DOLL_HEIGHT as f32;
const SLOT_SIZE: f32 = 50.0;

// ---------------------------------------------------------------------------
// Item & Inventory Data Model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EquipSlot {
    Head,
    Shoulders,
    Chest,
    Hands,
    Legs,
    Feet,
    RightHand,
    LeftHand,
    Amulet,
    Ring,
    Cloak,
}

impl EquipSlot {
    pub fn label(self) -> &'static str {
        match self {
            Self::Head => "Head",
            Self::Shoulders => "Shoulders",
            Self::Chest => "Chest",
            Self::Hands => "Hands",
            Self::Legs => "Legs",
            Self::Feet => "Feet",
            Self::RightHand => "Right Hand",
            Self::LeftHand => "Left Hand",
            Self::Amulet => "Amulet",
            Self::Ring => "Ring",
            Self::Cloak => "Cloak",
        }
    }
}

#[derive(Clone, Debug)]
pub struct InventoryItem {
    pub id: u32,
    pub name: String,
    pub slot: EquipSlot,
    pub equipped: bool,
    pub weight_kg: f32,
    pub armor_rating: i32,
    pub paperdoll_record: Option<usize>,
    pub icon_source: Option<(u32, usize)>,
    pub description: String,
}

#[derive(Resource)]
pub struct PlayerInventory {
    pub items: Vec<InventoryItem>,
    pub gold: u32,
    pub dirty: bool,
}

impl Default for PlayerInventory {
    fn default() -> Self {
        Self {
            gold: 320,
            dirty: true,
            items: vec![
                InventoryItem {
                    id: 1,
                    name: "Elven Plate Cuirass".into(),
                    slot: EquipSlot::Chest,
                    equipped: true,
                    weight_kg: 8.5,
                    armor_rating: 18,
                    paperdoll_record: Some(3),
                    icon_source: Some((247, 3)),
                    description: "Chest plate forged from lightweight, durable elven alloys.".into(),
                },
                InventoryItem {
                    id: 2,
                    name: "Steel Greaves".into(),
                    slot: EquipSlot::Legs,
                    equipped: true,
                    weight_kg: 6.0,
                    armor_rating: 12,
                    paperdoll_record: Some(4),
                    icon_source: Some((247, 4)),
                    description: "Articulated plate armor tailored to protect the lower legs.".into(),
                },
                InventoryItem {
                    id: 3,
                    name: "Iron Battle Boots".into(),
                    slot: EquipSlot::Feet,
                    equipped: true,
                    weight_kg: 4.5,
                    armor_rating: 6,
                    paperdoll_record: Some(2),
                    icon_source: Some((247, 2)),
                    description: "Heavy reinforced footwear with solid metal soles and caps.".into(),
                },
                InventoryItem {
                    id: 4,
                    name: "Steel Gauntlets".into(),
                    slot: EquipSlot::Hands,
                    equipped: true,
                    weight_kg: 2.0,
                    armor_rating: 5,
                    paperdoll_record: Some(6),
                    icon_source: Some((247, 6)),
                    description: "Combat gauntlets with layered, flexible finger plates.".into(),
                },
                InventoryItem {
                    id: 5,
                    name: "Steel Pauldrons".into(),
                    slot: EquipSlot::Shoulders,
                    equipped: true,
                    weight_kg: 3.5,
                    armor_rating: 6,
                    paperdoll_record: Some(7),
                    icon_source: Some((247, 7)),
                    description: "Curved shoulder guards crafted to deflect downward strikes.".into(),
                },
                InventoryItem {
                    id: 6,
                    name: "Steel Close Helm".into(),
                    slot: EquipSlot::Head,
                    equipped: false,
                    weight_kg: 3.0,
                    armor_rating: 8,
                    paperdoll_record: Some(5),
                    icon_source: Some((247, 5)),
                    description: "Knightly visor helmet providing solid cranial and throat protection.".into(),
                },
                InventoryItem {
                    id: 7,
                    name: "Elven Longsword".into(),
                    slot: EquipSlot::RightHand,
                    equipped: true,
                    weight_kg: 2.5,
                    armor_rating: 0,
                    paperdoll_record: Some(13),
                    icon_source: Some((234, 12)),
                    description: "Finely balanced single-edged blade etched with ancient elven runes.".into(),
                },
                InventoryItem {
                    id: 8,
                    name: "Iron Round Shield".into(),
                    slot: EquipSlot::LeftHand,
                    equipped: true,
                    weight_kg: 4.0,
                    armor_rating: 8,
                    paperdoll_record: Some(34),
                    icon_source: None,
                    description: "Sturdy infantry shield with central metal boss and reinforced rim.".into(),
                },
                InventoryItem {
                    id: 9,
                    name: "Chainmail Hauberk".into(),
                    slot: EquipSlot::Chest,
                    equipped: false,
                    weight_kg: 5.5,
                    armor_rating: 10,
                    paperdoll_record: Some(5),
                    icon_source: Some((247, 5)),
                    description: "Flexible mail shirt made of interlocking riveted iron rings.".into(),
                },
                InventoryItem {
                    id: 10,
                    name: "Greater Healing Potion".into(),
                    slot: EquipSlot::Amulet,
                    equipped: false,
                    weight_kg: 0.5,
                    armor_rating: 0,
                    paperdoll_record: None,
                    icon_source: Some((205, 0)),
                    description: "Wax-sealed glass flask containing an invigorating crimson brew.".into(),
                },
                InventoryItem {
                    id: 11,
                    name: "Steel Battle Axe".into(),
                    slot: EquipSlot::RightHand,
                    equipped: false,
                    weight_kg: 11.0,
                    armor_rating: 0,
                    paperdoll_record: Some(0),
                    icon_source: Some((234, 0)),
                    description: "Heavy double-headed axe. Slow to swing, but it bites deep.".into(),
                },
            ],
        }
    }
}

impl PlayerInventory {
    pub fn toggle_equip(&mut self, idx: usize) {
        if idx >= self.items.len() {
            return;
        }
        let will_equip = !self.items[idx].equipped;
        let slot = self.items[idx].slot;

        if will_equip {
            for (i, item) in self.items.iter_mut().enumerate() {
                if i != idx && item.slot == slot && item.equipped {
                    item.equipped = false;
                }
            }
            self.items[idx].equipped = true;
        } else {
            self.items[idx].equipped = false;
        }
        self.dirty = true;
    }

    pub fn total_weight(&self) -> f32 {
        self.items.iter().map(|i| i.weight_kg).sum()
    }

    pub fn total_armor(&self) -> i32 {
        self.items.iter().filter(|i| i.equipped).map(|i| i.armor_rating).sum()
    }

    pub fn equipped_in(&self, slot: EquipSlot) -> Option<&InventoryItem> {
        self.items.iter().find(|i| i.equipped && i.slot == slot)
    }

    pub fn equipped_index(&self, slot: EquipSlot) -> Option<usize> {
        self.items.iter().position(|i| i.equipped && i.slot == slot)
    }
}

// ---------------------------------------------------------------------------
// Appearance and State
// ---------------------------------------------------------------------------

#[derive(Resource, Default)]
pub struct EquipmentState {
    pub open: bool,
    was_locked: bool,
}

#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlayerAppearance {
    pub race: u8,
    pub female: bool,
    pub face: usize,
}

impl Default for PlayerAppearance {
    fn default() -> Self {
        Self { race: 4, female: true, face: 0 }
    }
}

pub fn appearance_label(look: PlayerAppearance) -> String {
    let race_name = match (look.race % 8, look.female) {
        (3, true) => "Dark Elf (Female)",
        (4, true) => "High Elf (Female)",
        (5, true) => "Wood Elf (Female)",
        (0, true) => "Breton (Female)",
        (1, true) => "Redguard (Female)",
        (2, true) => "Nord (Female)",
        (6, true) => "Khajiit (Female)",
        (7, true) => "Argonian (Female)",
        (3, false) => "Dark Elf (Male)",
        (4, false) => "High Elf (Male)",
        (5, false) => "Wood Elf (Male)",
        (0, false) => "Breton (Male)",
        (1, false) => "Redguard (Male)",
        (2, false) => "Nord (Male)",
        (6, false) => "Khajiit (Male)",
        (7, false) => "Argonian (Male)",
        _ => "Character",
    };
    format!("{} — Face {}", race_name, look.face + 1)
}

struct BuiltDoll {
    handle: Handle<Image>,
    size: Vec2,
}

#[derive(Resource, Default)]
struct DollCache {
    key: Option<(PlayerAppearance, Vec<(u32, bool)>)>,
    built: Option<BuiltDoll>,
    layered: Option<(Vec2, Vec<HdLayer>)>,
    layers_dirty: bool,
}

struct HdLayer {
    image: Handle<Image>,
    flip_y: bool,
    x: i32,
    y: i32,
    w: usize,
    h: usize,
}

#[derive(Resource, Default)]
struct ItemIconCache {
    icons: HashMap<u32, (Handle<Image>, bool)>,
}

// ---------------------------------------------------------------------------
// UI Components
// ---------------------------------------------------------------------------

#[derive(Component)]
struct EquipmentRoot;

#[derive(Component)]
struct DollImage;

#[derive(Component)]
struct DollMissing;

#[derive(Component)]
struct DollLayerNode;

#[derive(Component)]
struct DollDescText;

#[derive(Component)]
struct BackpackSummaryText;

#[derive(Component)]
struct BackpackSlotButton(usize);

#[derive(Component)]
struct BackpackSlotIcon(usize);

#[derive(Component)]
struct BackpackSlotBadge(usize);

#[derive(Component)]
struct EquipSlotButton(EquipSlot);

#[derive(Component)]
struct EquipSlotIcon(EquipSlot);

#[derive(Component)]
struct EquipSlotLabel(EquipSlot);

#[derive(Component)]
struct TooltipNode;

#[derive(Component)]
struct TooltipTitle;

#[derive(Component)]
struct TooltipSlot;

#[derive(Component)]
struct TooltipStats;

#[derive(Component)]
struct TooltipDesc;

#[derive(Component)]
struct TooltipStatus;

// ---------------------------------------------------------------------------
// Image File Reading (IMG / CIF / TEXTURE)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct DfImage {
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

const IMG_HEADER: usize = 12;
const IMG_RLE: usize = 2;
const MAX_SIDE: usize = 2048;

fn rle_decode(src: &[u8], out_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(out_len);
    let mut i = 0;
    while out.len() < out_len && i < src.len() {
        let c = src[i];
        i += 1;
        if c > 127 {
            let Some(&b) = src.get(i) else { break };
            i += 1;
            out.extend(std::iter::repeat(b).take((c - 127) as usize));
        } else {
            let end = (i + c as usize + 1).min(src.len());
            out.extend_from_slice(&src[i..end]);
            i = end;
        }
    }
    out.resize(out_len, 0);
    out
}

fn read_record(data: &[u8], pos: usize) -> Option<(DfImage, usize)> {
    let h = data.get(pos..pos.checked_add(IMG_HEADER)?)?;
    let i16_at = |o: usize| i16::from_le_bytes([h[o], h[o + 1]]);
    let u16_at = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]) as usize;
    let (width, height) = (u16_at(4), u16_at(6));
    let (compression, len) = (u16_at(8), u16_at(10));
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    let start = pos + IMG_HEADER;
    let body = data.get(start..start.checked_add(len)?)?;
    let n = width * height;
    let pixels = if compression == IMG_RLE {
        rle_decode(body, n)
    } else {
        let mut p = body[..body.len().min(n)].to_vec();
        p.resize(n, 0);
        p
    };
    let image = DfImage { x: i16_at(0) as i32, y: i16_at(2) as i32, width, height, pixels };
    Some((image, start + len))
}

fn read_img(data: &[u8]) -> Option<DfImage> {
    read_record(data, 0).map(|(image, _)| image)
}

fn read_cif(data: &[u8], index: usize) -> Option<DfImage> {
    let mut pos = 0;
    for _ in 0..index {
        pos = read_record(data, pos)?.1;
    }
    read_record(data, pos).map(|(image, _)| image)
}

fn doll_file_names(look: PlayerAppearance) -> (String, String) {
    let (g, r) = (look.female as u8, (look.race % 8));
    (format!("BODY{g}{r}I0.IMG"), format!("FACE{g}{r}I0.CIF"))
}

fn armor_archive(look: PlayerAppearance) -> u32 {
    let elf = matches!(look.race % 8, 3 | 4 | 5);
    match (look.female, elf) {
        (true, true) => 246,
        (true, false) => 247,
        (false, true) => 250,
        (false, false) => 251,
    }
}

const WEAPON_ARCHIVE: u32 = 234;
const WEAPON_ARCHIVE_FALLBACK: u32 = 233;

fn paperdoll_archive(look: PlayerAppearance, slot: EquipSlot) -> u32 {
    match slot {
        EquipSlot::RightHand => WEAPON_ARCHIVE,
        _ => armor_archive(look),
    }
}

fn icon_location(look: PlayerAppearance, item: &InventoryItem) -> Option<(u32, usize)> {
    if let Some(source) = item.icon_source {
        return Some(source);
    }
    let record = item.paperdoll_record?;
    Some(match item.slot {
        EquipSlot::RightHand => (WEAPON_ARCHIVE, record.saturating_sub(1)),
         slot => (paperdoll_archive(look, slot), record),
    })
}

fn load_paperdoll_item_sprite(
    data: &GameData,
    look: PlayerAppearance,
    item: &InventoryItem,
    record_idx: usize,
) -> Option<DfImage> {
    let archive_id = paperdoll_archive(look, item.slot);

    let path = find_file_ci(&data.dir, &archive_file_name(archive_id))
    .or_else(|| {
        if item.slot == EquipSlot::RightHand {
            find_file_ci(&data.dir, &archive_file_name(WEAPON_ARCHIVE_FALLBACK))
        } else {
            find_file_ci(&data.dir, &archive_file_name(247))
            .or_else(|| find_file_ci(&data.dir, &archive_file_name(251)))
        }
    })?;

    let tex = TextureFile::open(path).ok()?;
    let rec = tex.record(record_idx)?;
    let img = tex.frame(record_idx, 0).ok()?;

    let (ax, ay) = if rec.offset_x != 0 || rec.offset_y != 0 {
        (rec.offset_x as i32, rec.offset_y as i32)
    } else {
        match item.slot {
            EquipSlot::RightHand => (14, 76),
            EquipSlot::LeftHand => (66, 80),
            EquipSlot::Chest => (27, 40),
            EquipSlot::Legs => (31, 84),
            EquipSlot::Feet => (28, 138),
            EquipSlot::Hands => (15, 68),
            EquipSlot::Head => (38, 4),
            EquipSlot::Shoulders => (18, 32),
            _ => (25, 40),
        }
    };

    Some(DfImage {
        x: ax,
         y: ay,
         width: img.width,
         height: img.height,
         pixels: img.indices,
    })
}

// ---------------------------------------------------------------------------
// Fallback Procedural Icons
// ---------------------------------------------------------------------------

fn generate_fallback_icon(slot: EquipSlot) -> Image {
    const W: usize = 32;
    const H: usize = 32;
    let mut rgba = vec![0u8; W * H * 4];

    let mut set = |x: usize, y: usize, col: [u8; 4]| {
        if x < W && y < H {
            let o = (y * W + x) * 4;
            rgba[o..o + 4].copy_from_slice(&col);
        }
    };

    let metal = [190, 195, 205, 255];
    let dark_metal = [120, 125, 135, 255];
    let gold = [230, 180, 50, 255];
    let wood = [140, 85, 40, 255];
    let red_pot = [220, 45, 45, 240];

    match slot {
        EquipSlot::Chest => {
            for y in 6..26 {
                for x in 8..24 {
                    if (y < 10 && (x < 12 || x > 19)) || (y > 22 && (x < 11 || x > 20)) {
                        continue;
                    }
                    set(x, y, metal);
                }
            }
            for y in 10..22 { set(15, y, dark_metal); set(16, y, dark_metal); }
        }
        EquipSlot::Legs => {
            for y in 6..26 {
                for x in 9..14 { set(x, y, metal); }
                for x in 18..23 { set(x, y, metal); }
            }
        }
        EquipSlot::Feet => {
            for y in 10..24 {
                for x in 8..14 { set(x, y, metal); }
                for x in 18..24 { set(x, y, metal); }
            }
            for x in 6..14 { set(x, 24, dark_metal); }
            for x in 18..26 { set(x, 24, dark_metal); }
        }
        EquipSlot::Head => {
            for y in 6..24 {
                for x in 8..24 {
                    if (y < 9 && (x < 11 || x > 20)) || (y > 14 && y < 17 && x > 11 && x < 21) {
                        continue;
                    }
                    set(x, y, metal);
                }
            }
        }
        EquipSlot::Hands => {
            for y in 10..24 {
                for x in 8..14 { set(x, y, dark_metal); }
                for x in 18..24 { set(x, y, dark_metal); }
            }
        }
        EquipSlot::Shoulders => {
            for y in 8..18 {
                for x in 4..13 { set(x, y, metal); }
                for x in 19..28 { set(x, y, metal); }
            }
        }
        EquipSlot::RightHand => {
            for i in 6..22 { set(i, i, metal); set(i + 1, i, metal); }
            for i in 20..26 { set(i + 3, i - 1, gold); set(i - 1, i + 3, gold); }
            for i in 22..27 { set(i, i, wood); }
        }
        EquipSlot::LeftHand => {
            for y in 6..26 {
                for x in 6..26 {
                    let d = ((x as f32 - 16.0).powi(2) + (y as f32 - 16.0).powi(2)).sqrt();
                    if d < 10.0 { set(x, y, metal); }
                    if (d - 7.0).abs() < 1.0 { set(x, y, wood); }
                    if d < 3.0 { set(x, y, gold); }
                }
            }
        }
        _ => {
            for y in 12..25 {
                for x in 10..22 {
                    let d = ((x as f32 - 16.0).powi(2) + (y as f32 - 18.0).powi(2)).sqrt();
                    if d < 6.0 { set(x, y, red_pot); }
                }
            }
            for y in 8..13 { for x in 14..18 { set(x, y, [240, 240, 250, 200]); } }
            for x in 13..19 { set(x, 7, wood); }
        }
    }

    let mut img = Image::new(
        Extent3d { width: W as u32, height: H as u32, depth_or_array_layers: 1 },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    img.sampler = bevy::image::ImageSampler::nearest();
    img
}

fn material_from_name(name: &str) -> Option<String> {
    const MATERIALS: [&str; 10] = [
        "iron", "steel", "silver", "elven", "dwarven", "mithril", "adamantium", "ebony", "orcish", "daedric",
    ];
    let first = name.split_whitespace().next()?.to_ascii_lowercase();
    MATERIALS.contains(&first.as_str()).then_some(first)
}

fn load_item_icon(
    data: &GameData,
    look: PlayerAppearance,
    item: &InventoryItem,
    images: &mut Assets<Image>,
    mods: &mut ModTextures,
    cache: &mut ItemIconCache,
) -> (Handle<Image>, bool) {
    if let Some(entry) = cache.icons.get(&item.id) {
        return entry.clone();
    }

    let entry = (|| {
        let (archive, record) = icon_location(look, item)?;
        let mut archives = vec![archive];
        if archive == WEAPON_ARCHIVE {
            archives.push(WEAPON_ARCHIVE_FALLBACK);
        }

        // 1. Versione HD nelle mod.
        let material = material_from_name(&item.name);
        for &a in &archives {
            if let Some(handle) = mods.item_image(a, record as u32, 0, material.as_deref(), images) {
                let flip = mods.needs_v_flip(&handle);
                info!("icona di '{}': HD dalla mod, TEXTURE.{} record {}", item.name, a, record);
                return Some((handle, flip));
            }
        }

        // 2. Originale di Daggerfall.
        let tex = archives.iter().find_map(|&a| {
            find_file_ci(&data.dir, &archive_file_name(a)).and_then(|p| TextureFile::open(p).ok())
        })?;
        let img = tex.frame(record, 0).ok()?;
        if img.width == 0 || img.height == 0 {
            return None;
        }
        info!("icona di '{}': TEXTURE.{} record {}", item.name, archive, record);

        let rgba = img.to_rgba(&data.palette, Some(0));
        let mut image = Image::new(
            Extent3d { width: img.width as u32, height: img.height as u32, depth_or_array_layers: 1 },
            TextureDimension::D2,
            rgba,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        );
        image.sampler = bevy::image::ImageSampler::nearest();
        Some((images.add(image), false))
    })()
    .unwrap_or_else(|| {
        warn!("icona di '{}' non trovata: uso quella generata", item.name);
        (images.add(generate_fallback_icon(item.slot)), false)
    });

    cache.icons.insert(item.id, entry.clone());
    entry
}

// ---------------------------------------------------------------------------
// Layered Paperdoll Composition
// ---------------------------------------------------------------------------

struct Doll {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

fn compose_doll(
    body: &DfImage,
    head: &DfImage,
    armor_pieces: &[DfImage],
    palette: &Palette,
) -> Doll {
    let w = DOLL_WIDTH
    .max((body.x.max(0) as usize).saturating_add(body.width))
    .max((head.x.max(0) as usize).saturating_add(head.width));
    let h = DOLL_HEIGHT
    .max((body.y.max(0) as usize).saturating_add(body.height))
    .max((head.y.max(0) as usize).saturating_add(head.height));
    let mut rgba = vec![0u8; w * h * 4];

    let mut put = |x: i32, y: i32, index: u8| {
        if index == 0 || x < 0 || y < 0 || x as usize >= w || y as usize >= h {
            return;
        }
        let c = palette.color(index);
        let o = (y as usize * w + x as usize) * 4;
        rgba[o..o + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
    };

    // 1. Body
    for (i, &index) in body.pixels.iter().enumerate() {
        let bx = body.x + (i % body.width) as i32;
        let by = body.y + (i / body.width) as i32;
        put(bx, by, index);
    }

    // 2. Head
    for (i, &index) in head.pixels.iter().enumerate() {
        let hx = head.x + (i % head.width) as i32;
        let hy = head.y + (i / head.width) as i32;
        put(hx, hy, index);
    }

    // 3. Armor and weapons/shields
    for armor in armor_pieces {
        for (i, &index) in armor.pixels.iter().enumerate() {
            let ax = armor.x + (i % armor.width) as i32;
            let ay = armor.y + (i / armor.width) as i32;
            put(ax, ay, index);
        }
    }

    Doll { width: w, height: h, rgba }
}

fn build_doll(data: &GameData, look: PlayerAppearance, inventory: &PlayerInventory) -> Option<Doll> {
    let (body_name, head_name) = doll_file_names(look);
    let read = |name: &str| -> Option<Vec<u8>> {
        let path = find_file_ci(&data.dir, name);
        std::fs::read(path?).ok()
    };
    let body = read_img(&read(&body_name)?);
    let head = read_cif(&read(&head_name)?, look.face);
    let (Some(body), Some(head)) = (body, head) else {
        return None;
    };
    let palette = find_file_ci(&data.dir, DOLL_PALETTE)
    .and_then(|p| Palette::open(p).ok())
    .unwrap_or_else(|| (*data.palette).clone());

    let slot_order = [
        EquipSlot::Legs,
        EquipSlot::Feet,
        EquipSlot::Chest,
        EquipSlot::Hands,
        EquipSlot::Shoulders,
        EquipSlot::Head,
        EquipSlot::RightHand,
        EquipSlot::LeftHand,
    ];

    let mut equipped_layers = Vec::new();
    for slot in slot_order {
        if let Some(item) = inventory.equipped_in(slot) {
            if let Some(record_idx) = item.paperdoll_record {
                if let Some(layer_img) = load_paperdoll_item_sprite(data, look, item, record_idx) {
                    equipped_layers.push(layer_img);
                }
            }
        }
    }

    Some(compose_doll(&body, &head, &equipped_layers, &palette))
}

// ---------------------------------------------------------------------------
// Bevy UI Hierarchy & Slots
// ---------------------------------------------------------------------------

pub struct EquipmentPlugin;

impl Plugin for EquipmentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EquipmentState>()
        .init_resource::<PlayerAppearance>()
        .init_resource::<PlayerInventory>()
        .init_resource::<DollCache>()
        .init_resource::<ItemIconCache>()
        .add_systems(Startup, setup_equipment_ui)
        .add_systems(
            Update,
            (
                toggle_equipment,
             handle_backpack_clicks,
             handle_equip_slot_clicks,
             update_tooltip,
             refresh_doll,
             update_inventory_ui,
            )
            .chain()
            .before(crate::dialog::update_dialog)
            .before(crate::player::grab_cursor),
        );
    }
}

fn slot_column(parent: &mut ChildBuilder, slots: &[EquipSlot]) {
    parent
    .spawn(Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(8.0), ..default() })
    .with_children(|column| {
        for &slot in slots {
            column
            .spawn(Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(2.0),
                   ..default()
            })
            .with_children(|s| {
                s.spawn((
                    Button,
                    EquipSlotButton(slot),
                         Node {
                             width: Val::Px(SLOT_SIZE),
                         height: Val::Px(SLOT_SIZE),
                         border: UiRect::all(Val::Px(1.5)),
                         justify_content: JustifyContent::Center,
                         align_items: AlignItems::Center,
                         position_type: PositionType::Relative,
                         ..default()
                         },
                         BackgroundColor(SLOT_BG),
                         BorderColor(SLOT_BORDER),
                         BorderRadius::all(Val::Px(8.0)),
                ))
                .with_children(|box_node| {
                    box_node.spawn((
                        ImageNode::default(),
                                    Node {
                                        max_width: Val::Px(38.0),
                                    max_height: Val::Px(38.0),
                                    ..default()
                                    },
                                    Visibility::Hidden,
                                    EquipSlotIcon(slot),
                    ));
                    box_node.spawn((
                        Text::new(""),
                                    TextFont { font_size: 10.0, ..default() },
                                    TextColor(MUTED),
                                    EquipSlotLabel(slot),
                    ));
                });
                s.spawn((
                    Text::new(slot.label()),
                         TextFont { font_size: 11.0, ..default() },
                         TextColor(MUTED),
                ));
            });
        }
    });
}

fn setup_equipment_ui(mut commands: Commands, inventory: Res<PlayerInventory>) {
    commands
    .spawn((
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(OVERLAY),
            GlobalZIndex(90),
            Visibility::Hidden,
            EquipmentRoot,
    ))
    .with_children(|root| {
        root.spawn((
            Node {
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(20.0)),
                    row_gap: Val::Px(14.0),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
            },
            BackgroundColor(PANEL),
                    BorderColor(PANEL_BORDER),
                    BorderRadius::all(Val::Px(18.0)),
        ))
        .with_children(|panel| {
            // Header
            panel
            .spawn(Node {
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                column_gap: Val::Px(30.0),
                   ..default()
            })
            .with_children(|header| {
                header
                .spawn(Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(3.0), ..default() })
                .with_children(|title| {
                    title.spawn((
                        Text::new("Inventory & Character"),
                                 TextFont { font_size: 24.0, ..default() },
                                 TextColor(TEXT),
                    ));
                    title.spawn((
                        Node { width: Val::Px(40.0), height: Val::Px(2.5), ..default() },
                                 BackgroundColor(ACCENT),
                                 BorderRadius::all(Val::Px(2.0)),
                    ));
                });
                header
                .spawn(Node { align_items: AlignItems::Center, column_gap: Val::Px(8.0), ..default() })
                .with_children(|hint| {
                    hint.spawn((
                        Text::new("[Click / 1..9] Equip/Unequip · [Hover] Item details · [R] Race · [V] Face · [C/Esc] Close"),
                                TextFont { font_size: 12.0, ..default() },
                                TextColor(MUTED),
                    ));
                });
            });

            // Main Section: [Left Slots] - [Paperdoll] - [Right Slots] - [Backpack Grid]
            panel
            .spawn(Node { align_items: AlignItems::FlexStart, column_gap: Val::Px(16.0), ..default() })
            .with_children(|body| {
                // Left Slots
                slot_column(body, &[
                    EquipSlot::Head,
                    EquipSlot::Shoulders,
                    EquipSlot::Chest,
                    EquipSlot::Hands,
                    EquipSlot::Legs,
                    EquipSlot::Feet,
                ]);

                // Paperdoll Card
                body.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Center,
                        padding: UiRect::all(Val::Px(12.0)),
                            row_gap: Val::Px(8.0),
                            border: UiRect::all(Val::Px(1.0)),
                            ..default()
                    },
                    BackgroundColor(CARD),
                            BorderColor(PANEL_BORDER),
                            BorderRadius::all(Val::Px(14.0)),
                ))
                .with_children(|card| {
                    card.spawn((
                        Node {
                            position_type: PositionType::Relative,
                            left: Val::Px(-200.0), // <-- Offset a sinistra di 200 pixel
                                height: Val::Vh(DOLL_HEIGHT_VH),
                                aspect_ratio: Some(DEFAULT_DOLL_ASPECT),
                                ..default()
                        },
                        ImageNode::default(),
                                Visibility::Hidden,
                                DollImage,
                    ));
                    card.spawn((
                        Text::new("Character textures not found"),
                                TextFont { font_size: 13.0, ..default() },
                                TextColor(MUTED),
                                Visibility::Hidden,
                                DollMissing,
                    ));
                    card.spawn((
                        Text::new(appearance_label(PlayerAppearance::default())),
                                TextFont { font_size: 13.0, ..default() },
                                TextColor(ACCENT),
                                DollDescText,
                    ));
                });

                // Right Slots
                slot_column(body, &[
                    EquipSlot::RightHand,
                    EquipSlot::LeftHand,
                    EquipSlot::Amulet,
                    EquipSlot::Ring,
                    EquipSlot::Ring,
                    EquipSlot::Cloak,
                ]);

                // ==========================================
                // BACKPACK PANEL (ICON GRID)
                // ==========================================
                body.spawn((
                    Node {
                        width: Val::Px(350.0),
                            flex_direction: FlexDirection::Column,
                            padding: UiRect::all(Val::Px(14.0)),
                            row_gap: Val::Px(10.0),
                            border: UiRect::all(Val::Px(1.0)),
                            ..default()
                    },
                    BackgroundColor(CARD),
                            BorderColor(PANEL_BORDER),
                            BorderRadius::all(Val::Px(16.0)),
                ))
                .with_children(|backpack_panel| {
                    // Backpack Header
                    backpack_panel.spawn((
                        Node {
                            justify_content: JustifyContent::SpaceBetween,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                    )).with_children(|bh| {
                        bh.spawn((
                            Text::new("BACKPACK"),
                                  TextFont { font_size: 14.0, ..default() },
                                  TextColor(ACCENT),
                        ));
                        bh.spawn((
                            Text::new(""),
                                  TextFont { font_size: 11.5, ..default() },
                                  TextColor(MUTED),
                                  BackpackSummaryText,
                        ));
                    });

                    // Backpack Icons Grid
                    backpack_panel.spawn((
                        Node {
                            flex_direction: FlexDirection::Row,
                            flex_wrap: FlexWrap::Wrap,
                            column_gap: Val::Px(8.0),
                                          row_gap: Val::Px(8.0),
                                          justify_content: JustifyContent::FlexStart,
                                          ..default()
                        },
                    )).with_children(|grid| {
                        for idx in 0..inventory.items.len() {
                            grid.spawn((
                                Button,
                                BackpackSlotButton(idx),
                                        Node {
                                            width: Val::Px(72.0),
                                        height: Val::Px(72.0),
                                        border: UiRect::all(Val::Px(1.5)),
                                        justify_content: JustifyContent::Center,
                                        align_items: AlignItems::Center,
                                        position_type: PositionType::Relative,
                                        ..default()
                                        },
                                        BackgroundColor(SLOT_BG),
                                        BorderColor(SLOT_BORDER),
                                        BorderRadius::all(Val::Px(8.0)),
                            ))
                            .with_children(|slot_node| {
                                // Item Icon
                                slot_node.spawn((
                                    ImageNode::default(),
                                                 Node {
                                                     max_width: Val::Px(52.0),
                                                 max_height: Val::Px(52.0),
                                                 ..default()
                                                 },
                                                 BackpackSlotIcon(idx),
                                ));
                                // Quick Key (1..9)
                                slot_node.spawn((
                                    Node {
                                        position_type: PositionType::Absolute,
                                        top: Val::Px(2.0),
                                                 left: Val::Px(4.0),
                                                 ..default()
                                    },
                                    Text::new(if idx < 9 { format!("{}", idx + 1) } else { "".into() }),
                                                 TextFont { font_size: 10.0, ..default() },
                                                 TextColor(MUTED),
                                ));
                                // Equipped "E" Badge
                                slot_node.spawn((
                                    Node {
                                        position_type: PositionType::Absolute,
                                        top: Val::Px(2.0),
                                                 right: Val::Px(4.0),
                                                 ..default()
                                    },
                                    Text::new("E"),
                                                 TextFont { font_size: 11.0, ..default() },
                                                 TextColor(EQUIPPED_GREEN),
                                                 Visibility::Hidden,
                                                 BackpackSlotBadge(idx),
                                ));
                            });
                        }
                    });

                    backpack_panel.spawn((
                        Text::new("Click an icon to equip/unequip the item"),
                                          TextFont { font_size: 11.0, ..default() },
                                          TextColor(MUTED),
                    ));
                });
            });
        });
    });

    // ==========================================
    // FLOATING POPUP TOOLTIP
    // ==========================================
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Px(245.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(10.0)),
                    row_gap: Val::Px(4.0),
                    border: UiRect::all(Val::Px(1.5)),
                    ..default()
        },
        BackgroundColor(Color::srgba(0.08, 0.06, 0.04, 0.97)),
                    BorderColor(ACCENT),
                    BorderRadius::all(Val::Px(8.0)),
                    GlobalZIndex(200),
                    Visibility::Hidden,
                    TooltipNode,
    )).with_children(|tt| {
        tt.spawn((
            Text::new(""),
                  TextFont { font_size: 13.5, ..default() },
                  TextColor(ACCENT),
                  TooltipTitle,
        ));
        tt.spawn((
            Node {
                justify_content: JustifyContent::SpaceBetween,
                ..default()
            },
        )).with_children(|row| {
            row.spawn((
                Text::new(""),
                       TextFont { font_size: 11.0, ..default() },
                       TextColor(MUTED),
                       TooltipSlot,
            ));
            row.spawn((
                Text::new(""),
                       TextFont { font_size: 11.0, ..default() },
                       TextColor(Color::srgb(0.75, 0.88, 1.0)),
                       TooltipStats,
            ));
        });
        tt.spawn((
            Text::new(""),
                  TextFont { font_size: 10.5, ..default() },
                  TextColor(Color::srgb(0.85, 0.85, 0.88)),
                  TooltipDesc,
        ));
        tt.spawn((
            Text::new(""),
                  TextFont { font_size: 10.5, ..default() },
                  TextColor(EQUIPPED_GREEN),
                  TooltipStatus,
        ));
    });
}

// ---------------------------------------------------------------------------
// Logic & Systems
// ---------------------------------------------------------------------------

pub fn toggle_equipment(
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut state: ResMut<EquipmentState>,
    mut look: ResMut<PlayerAppearance>,
    mut inventory: ResMut<PlayerInventory>,
    dialog: Res<DialogState>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut root: Query<&mut Visibility, With<EquipmentRoot>>,
    mut desc: Query<&mut Text, (With<DollDescText>, Without<BackpackSummaryText>, Without<TooltipTitle>, Without<TooltipSlot>, Without<TooltipStats>, Without<TooltipDesc>, Without<TooltipStatus>, Without<EquipSlotLabel>)>,
) {
    let toggle = if state.open {
        keys.just_pressed(KeyCode::KeyC) || keys.just_pressed(KeyCode::Escape)
    } else {
        keys.just_pressed(KeyCode::KeyC) && !dialog.open
    };

    if toggle {
        let open = !state.open;
        state.open = open;
        if let Ok(mut window) = windows.get_single_mut() {
            if open {
                state.was_locked = window.cursor_options.grab_mode == CursorGrabMode::Locked;
                window.cursor_options.grab_mode = CursorGrabMode::None;
                window.cursor_options.visible = true;
                inventory.dirty = true;
            } else if state.was_locked {
                window.cursor_options.grab_mode = CursorGrabMode::Locked;
                window.cursor_options.visible = false;
            }
        }
        for mut visibility in &mut root {
            *visibility = if open { Visibility::Visible } else { Visibility::Hidden };
        }
    }

    if state.open && !toggle {
        let mut look_changed = false;
        if keys.just_pressed(KeyCode::KeyR) {
            look.race = (look.race + 1) % 8;
            look_changed = true;
        }
        if keys.just_pressed(KeyCode::Tab) {
            look.female = !look.female;
            look_changed = true;
        }
        if keys.just_pressed(KeyCode::KeyV) {
            look.face = (look.face + 1) % 10;
            look_changed = true;
        }
        if look_changed {
            inventory.dirty = true;
            for mut text in &mut desc {
                **text = appearance_label(*look);
            }
        }

        // Quick keys 1..9
        for (i, key) in [
            KeyCode::Digit1, KeyCode::Digit2, KeyCode::Digit3, KeyCode::Digit4,
            KeyCode::Digit5, KeyCode::Digit6, KeyCode::Digit7, KeyCode::Digit8,
            KeyCode::Digit9,
        ].iter().enumerate() {
            if keys.just_pressed(*key) {
                inventory.toggle_equip(i);
            }
        }
    }

    if toggle || state.open {
        keys.reset_all();
    }
}

fn handle_backpack_clicks(
    mut interaction_query: Query<
    (&Interaction, &BackpackSlotButton, &mut BackgroundColor),
                          (Changed<Interaction>, With<Button>),
                          >,
                          mut inventory: ResMut<PlayerInventory>,
) {
    for (interaction, slot, mut bg) in &mut interaction_query {
        match *interaction {
            Interaction::Pressed => {
                inventory.toggle_equip(slot.0);
                *bg = BackgroundColor(Color::srgb(0.25, 0.45, 0.25));
            }
            Interaction::Hovered => {
                *bg = BackgroundColor(Color::srgb(0.24, 0.18, 0.12));
            }
            Interaction::None => {
                *bg = BackgroundColor(SLOT_BG);
            }
        }
    }
}

fn handle_equip_slot_clicks(
    mut interaction_query: Query<
    (&Interaction, &EquipSlotButton, &mut BackgroundColor),
                            (Changed<Interaction>, With<Button>),
                            >,
                            mut inventory: ResMut<PlayerInventory>,
) {
    for (interaction, slot_btn, mut bg) in &mut interaction_query {
        match *interaction {
            Interaction::Pressed => {
                if let Some(idx) = inventory.equipped_index(slot_btn.0) {
                    inventory.toggle_equip(idx);
                }
                *bg = BackgroundColor(Color::srgb(0.35, 0.20, 0.20));
            }
            Interaction::Hovered => {
                *bg = BackgroundColor(Color::srgb(0.24, 0.18, 0.12));
            }
            Interaction::None => {
                *bg = BackgroundColor(SLOT_BG);
            }
        }
    }
}

fn update_tooltip(
    state: Res<EquipmentState>,
    inventory: Res<PlayerInventory>,
    windows: Query<&Window, With<PrimaryWindow>>,
    backpack_slots: Query<(&Interaction, &BackpackSlotButton)>,
                  equip_slots: Query<(&Interaction, &EquipSlotButton)>,
                  mut tooltip_node: Query<(&mut Node, &mut Visibility), With<TooltipNode>>,
                  mut title_q: Query<&mut Text, (With<TooltipTitle>, Without<TooltipSlot>, Without<TooltipStats>, Without<TooltipDesc>, Without<TooltipStatus>, Without<DollDescText>, Without<BackpackSummaryText>, Without<EquipSlotLabel>)>,
                  mut slot_q: Query<&mut Text, (With<TooltipSlot>, Without<TooltipTitle>, Without<TooltipStats>, Without<TooltipDesc>, Without<TooltipStatus>, Without<DollDescText>, Without<BackpackSummaryText>, Without<EquipSlotLabel>)>,
                  mut stats_q: Query<&mut Text, (With<TooltipStats>, Without<TooltipTitle>, Without<TooltipSlot>, Without<TooltipDesc>, Without<TooltipStatus>, Without<DollDescText>, Without<BackpackSummaryText>, Without<EquipSlotLabel>)>,
                  mut desc_q: Query<&mut Text, (With<TooltipDesc>, Without<TooltipTitle>, Without<TooltipSlot>, Without<TooltipStats>, Without<TooltipStatus>, Without<DollDescText>, Without<BackpackSummaryText>, Without<EquipSlotLabel>)>,
                  mut status_q: Query<(&mut Text, &mut TextColor), (With<TooltipStatus>, Without<TooltipTitle>, Without<TooltipSlot>, Without<TooltipStats>, Without<TooltipDesc>, Without<DollDescText>, Without<BackpackSummaryText>, Without<EquipSlotLabel>)>,
) {
    let Ok((mut tt_node, mut tt_vis)) = tooltip_node.get_single_mut() else { return };

    if !state.open {
        *tt_vis = Visibility::Hidden;
        return;
    }

    let mut hovered_item: Option<&InventoryItem> = None;

    for (interaction, slot) in &backpack_slots {
        if *interaction == Interaction::Hovered || *interaction == Interaction::Pressed {
            hovered_item = inventory.items.get(slot.0);
            break;
        }
    }

    if hovered_item.is_none() {
        for (interaction, eq) in &equip_slots {
            if *interaction == Interaction::Hovered || *interaction == Interaction::Pressed {
                hovered_item = inventory.equipped_in(eq.0);
                break;
            }
        }
    }

    let Some(item) = hovered_item else {
        *tt_vis = Visibility::Hidden;
        return;
    };

    let Ok(window) = windows.get_single() else { return };
    let Some(cursor_pos) = window.cursor_position() else {
        *tt_vis = Visibility::Hidden;
        return;
    };

    let tt_w = 245.0;
    let tt_h = 120.0;
    let mut left = cursor_pos.x + 14.0;
    let mut top = cursor_pos.y + 14.0;

    if left + tt_w > window.width() {
        left = (cursor_pos.x - tt_w - 8.0).max(8.0);
    }
    if top + tt_h > window.height() {
        top = (cursor_pos.y - tt_h - 8.0).max(8.0);
    }

    tt_node.left = Val::Px(left);
    tt_node.top = Val::Px(top);
    *tt_vis = Visibility::Visible;

    if let Ok(mut txt) = title_q.get_single_mut() {
        **txt = item.name.clone();
    }
    if let Ok(mut txt) = slot_q.get_single_mut() {
        **txt = format!("Slot: {}", item.slot.label());
    }
    if let Ok(mut txt) = stats_q.get_single_mut() {
        **txt = format!("Armor +{} | {:.1} kg", item.armor_rating, item.weight_kg);
    }
    if let Ok(mut txt) = desc_q.get_single_mut() {
        **txt = item.description.clone();
    }
    if let Ok((mut txt, mut col)) = status_q.get_single_mut() {
        if item.equipped {
            **txt = "● EQUIPPED (Click to unequip)".to_string();
            *col = TextColor(EQUIPPED_GREEN);
        } else {
            **txt = "○ IN BACKPACK (Click to equip)".to_string();
            *col = TextColor(ACCENT);
        }
    }
}

fn update_inventory_ui(
    mut inventory: ResMut<PlayerInventory>,
    state: Res<EquipmentState>,
    look: Res<PlayerAppearance>,
    data: Res<GameData>,
    mut icon_cache: ResMut<ItemIconCache>,
    mut images: ResMut<Assets<Image>>,
    mut mods: ResMut<ModTextures>,
    mut summary_q: Query<
    &mut Text,
    (
        With<BackpackSummaryText>,
     Without<EquipSlotLabel>,
     Without<DollDescText>,
     Without<TooltipTitle>,
     Without<TooltipSlot>,
     Without<TooltipStats>,
     Without<TooltipDesc>,
     Without<TooltipStatus>,
    ),
    >,
    mut bp_slots: Query<(&BackpackSlotButton, &mut BorderColor)>,
                       mut bp_icons: Query<(&BackpackSlotIcon, &mut ImageNode), Without<EquipSlotIcon>>,
                       mut bp_badges: Query<(&BackpackSlotBadge, &mut Visibility), (Without<EquipSlotIcon>, Without<EquipSlotLabel>)>,
                       mut eq_icons: Query<
                       (&EquipSlotIcon, &mut ImageNode, &mut Visibility),
                       (Without<BackpackSlotIcon>, Without<BackpackSlotBadge>, Without<EquipSlotLabel>),
                       >,
                       mut eq_labels: Query<
                       (&EquipSlotLabel, &mut Text, &mut Visibility),
                       (
                           Without<BackpackSlotBadge>,
                        Without<EquipSlotIcon>,
                        Without<BackpackSummaryText>,
                        Without<DollDescText>,
                        Without<TooltipTitle>,
                        Without<TooltipSlot>,
                        Without<TooltipStats>,
                        Without<TooltipDesc>,
                        Without<TooltipStatus>,
                       ),
                       >,
) {
    if !state.open || !inventory.dirty {
        return;
    }
    inventory.dirty = false;

    // 1. Backpack Summary
    if let Ok(mut summary_text) = summary_q.get_single_mut() {
        **summary_text = format!(
            "Weight: {:.1} kg  |  Armor: +{}  |  Gold: {} GP",
            inventory.total_weight(),
                                 inventory.total_armor(),
                                 inventory.gold
        );
    }

    // 2. Backpack slot icons, borders, and badges
    for (btn, mut border) in &mut bp_slots {
        if let Some(item) = inventory.items.get(btn.0) {
            *border = if item.equipped {
                BorderColor(EQUIPPED_GREEN)
            } else {
                BorderColor(SLOT_BORDER)
            };
        }
    }

    for (icon_comp, mut img_node) in &mut bp_icons {
        if let Some(item) = inventory.items.get(icon_comp.0) {
            let (handle, flip) = load_item_icon(&data, *look, item, &mut images, &mut mods, &mut icon_cache);
            img_node.image = handle;
            img_node.flip_y = flip;
        }
    }

    for (badge, mut vis) in &mut bp_badges {
        if let Some(item) = inventory.items.get(badge.0) {
            *vis = if item.equipped { Visibility::Inherited } else { Visibility::Hidden };
        }
    }

    // 3. Equipped paperdoll slots
    for (slot_comp, mut img_node, mut vis) in &mut eq_icons {
        if let Some(item) = inventory.equipped_in(slot_comp.0) {
            let (handle, flip) = load_item_icon(&data, *look, item, &mut images, &mut mods, &mut icon_cache);
            img_node.image = handle;
            img_node.flip_y = flip;
            *vis = Visibility::Inherited;
        } else {
            *vis = Visibility::Hidden;
        }
    }

    for (lbl_comp, mut txt, mut vis) in &mut eq_labels {
        if let Some(_item) = inventory.equipped_in(lbl_comp.0) {
            *vis = Visibility::Hidden;
        } else {
            **txt = "-".into();
            *vis = Visibility::Inherited;
        }
    }
}

fn layer_image(img: &DfImage, palette: &Palette) -> Image {
    let mut rgba = vec![0u8; img.width * img.height * 4];
    for (i, &index) in img.pixels.iter().take(img.width * img.height).enumerate() {
        if index == 0 {
            continue;
        }
        let c = palette.color(index);
        rgba[i * 4..i * 4 + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
    }
    let mut image = Image::new(
        Extent3d { width: img.width.max(1) as u32, height: img.height.max(1) as u32, depth_or_array_layers: 1 },
                               TextureDimension::D2,
                               rgba,
                               TextureFormat::Rgba8UnormSrgb,
                               RenderAssetUsages::default(),
    );
    image.sampler = bevy::image::ImageSampler::nearest();
    image
}

fn build_hd_layers(
    data: &GameData,
    look: PlayerAppearance,
    inventory: &PlayerInventory,
    mods: &mut ModTextures,
    images: &mut Assets<Image>,
) -> Option<(Vec2, Vec<HdLayer>)> {
    let (body_name, head_name) = doll_file_names(look);
    let read = |name: &str| -> Option<Vec<u8>> { std::fs::read(find_file_ci(&data.dir, name)?).ok() };
    let body = read_img(&read(&body_name)?)?;
    let head = read_cif(&read(&head_name)?, look.face)?;
    let palette = find_file_ci(&data.dir, DOLL_PALETTE)
    .and_then(|p| Palette::open(p).ok())
    .unwrap_or_else(|| (*data.palette).clone());

    let w = DOLL_WIDTH
    .max((body.x.max(0) as usize).saturating_add(body.width))
    .max((head.x.max(0) as usize).saturating_add(head.width));
    let h = DOLL_HEIGHT
    .max((body.y.max(0) as usize).saturating_add(body.height))
    .max((head.y.max(0) as usize).saturating_add(head.height));

    let (g, r) = (look.female as u8, look.race % 8);
    let mut sources: Vec<(DfImage, Vec<String>, Vec<String>)> = vec![
        (body, vec![format!("body{g}{r}i0")], Vec::new()),
        (head, vec![format!("face{g}{r}i0.cif_{}-0", look.face)], Vec::new()),
    ];
    let slot_order = [
        EquipSlot::Legs,
        EquipSlot::Feet,
        EquipSlot::Chest,
        EquipSlot::Hands,
        EquipSlot::Shoulders,
        EquipSlot::Head,
        EquipSlot::RightHand,
        EquipSlot::LeftHand,
    ];
    for slot in slot_order {
        let Some(item) = inventory.equipped_in(slot) else { continue };
        let Some(record) = item.paperdoll_record else { continue };
        let Some(native) = load_paperdoll_item_sprite(data, look, item, record) else { continue };
        let mut archives = vec![paperdoll_archive(look, slot)];
        if slot == EquipSlot::RightHand {
            archives.push(WEAPON_ARCHIVE_FALLBACK);
        }
        let prefixes: Vec<String> = archives
        .iter()
        .flat_map(|a| [format!("{a:03}_{record}-0"), format!("{a}_{record}-0")])
        .collect();
        let prefer = material_from_name(&item.name).map(|m| vec![format!("_{m}")]).unwrap_or_default();
        sources.push((native, prefixes, prefer));
    }

    let mut any_hd = false;
    let mut layers = Vec::with_capacity(sources.len());
    for (native, prefixes, prefer) in sources {
        let hd = prefixes.iter().find_map(|p| mods.image_by_prefix(p, &prefer, images));
        let (image, flip_y) = match hd {
            Some(handle) => {
                any_hd = true;
                let flip = mods.needs_v_flip(&handle);
                (handle, flip)
            }
            None => (images.add(layer_image(&native, &palette)), false),
        };
        layers.push(HdLayer { image, flip_y, x: native.x, y: native.y, w: native.width, h: native.height });
    }
    if !any_hd {
        warn!(
            "Nessuna texture HD per la bambola (cercato 'body{g}{r}i0'); nomi con 'body' nelle mod: {:?}",
              mods.names_containing("body", 8)
        );
    }
    any_hd.then(|| (Vec2::new(w as f32, h as f32), layers))
}

fn layer_rect_percent(layer: &HdLayer, doll: Vec2) -> (f32, f32, f32, f32) {
    (
        layer.x as f32 / doll.x * 100.0,
     layer.y as f32 / doll.y * 100.0,
     layer.w as f32 / doll.x * 100.0,
     layer.h as f32 / doll.y * 100.0,
    )
}

fn refresh_doll(
    mut commands: Commands,
    state: Res<EquipmentState>,
    look: Res<PlayerAppearance>,
    inventory: Res<PlayerInventory>,
    data: Res<GameData>,
    mut cache: ResMut<DollCache>,
    mut images: ResMut<Assets<Image>>,
    mut mods: ResMut<ModTextures>,
    mut doll: Query<(Entity, &mut ImageNode, &mut Node, &mut Visibility), (With<DollImage>, Without<DollMissing>)>,
                mut missing: Query<&mut Visibility, (With<DollMissing>, Without<DollImage>)>,
                layer_nodes: Query<Entity, With<DollLayerNode>>,
) {
    if !state.open {
        return;
    }

    let equipped_key: Vec<(u32, bool)> = inventory.items.iter().map(|i| (i.id, i.equipped)).collect();
    let current_key = Some((*look, equipped_key));

    if cache.key != current_key {
        cache.layered = build_hd_layers(&data, *look, &inventory, &mut mods, &mut images);
        cache.built = if cache.layered.is_some() {
            None
        } else {
            build_doll(&data, *look, &inventory).map(|d| {
                let mut image = Image::new(
                    Extent3d { width: d.width as u32, height: d.height as u32, depth_or_array_layers: 1 },
                    TextureDimension::D2,
                    d.rgba,
                    TextureFormat::Rgba8UnormSrgb,
                    RenderAssetUsages::default(),
                );
                image.sampler = bevy::image::ImageSampler::nearest();
                BuiltDoll { handle: images.add(image), size: Vec2::new(d.width as f32, d.height as f32) }
            })
        };
        cache.layers_dirty = true;
        cache.key = current_key;
    }

    if cache.layers_dirty {
        for entity in &layer_nodes {
            commands.entity(entity).despawn_recursive();
        }
        let container = doll.iter().next().map(|(entity, _, _, _)| entity);
        if let (Some((size, layers)), Some(container)) = (&cache.layered, container) {
            commands.entity(container).with_children(|parent| {
                for layer in layers {
                    let (left, top, width, height) = layer_rect_percent(layer, *size);
                    parent.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Percent(left),
                                  top: Val::Percent(top),
                                  width: Val::Percent(width),
                                  height: Val::Percent(height),
                                  ..default()
                        },
                        ImageNode { image: layer.image.clone(), flip_y: layer.flip_y, ..default() },
                                  DollLayerNode,
                    ));
                }
            });
        }
        cache.layers_dirty = false;
    }

    for (_, mut image, mut node, mut visibility) in &mut doll {
        if let Some((size, _)) = &cache.layered {
            image.color = Color::NONE;
            node.aspect_ratio = Some(size.x / size.y);
            *visibility = Visibility::Inherited;
        } else if let Some(built) = &cache.built {
            image.color = Color::WHITE;
            image.image = built.handle.clone();
            node.aspect_ratio = Some(built.size.x / built.size.y);
            *visibility = Visibility::Inherited;
        } else {
            *visibility = Visibility::Hidden;
        }
    }

    let has_doll = cache.layered.is_some() || cache.built.is_some();
    for mut visibility in &mut missing {
        *visibility = if has_doll { Visibility::Hidden } else { Visibility::Inherited };
    }
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn palette() -> Palette {
        let mut bytes = Vec::new();
        for i in 0..256usize {
            bytes.extend_from_slice(&[i as u8, (i + 1) as u8, (i + 2) as u8]);
        }
        Palette::from_bytes("PAL.PAL", &bytes).unwrap()
    }

    fn image(x: i32, y: i32, w: usize, h: usize, fill: u8) -> DfImage {
        DfImage { x, y, width: w, height: h, pixels: vec![fill; w * h] }
    }

    #[test]
    fn inventory_toggle_swaps_equipment() {
        let mut inv = PlayerInventory::default();
        assert!(inv.items[0].equipped);
        inv.toggle_equip(0);
        assert!(!inv.items[0].equipped);
        inv.toggle_equip(0);
        assert!(inv.items[0].equipped);
    }

    #[test]
    fn weapon_and_shield_icons_use_dfu_archives() {
        let inv = PlayerInventory::default();
        let sword = inv.items.iter().find(|i| i.slot == EquipSlot::RightHand).unwrap();
        let shield = inv.items.iter().find(|i| i.slot == EquipSlot::LeftHand).unwrap();
        let female_redguard = PlayerAppearance { race: 1, female: true, face: 0 };
        let male_elf = PlayerAppearance { race: 4, female: false, face: 0 };

        assert_eq!(icon_location(female_redguard, sword), Some((234, 12)));
        assert_eq!(sword.paperdoll_record, Some(13));
        assert_eq!(icon_location(female_redguard, shield), Some((247, 34)));
        assert_eq!(icon_location(male_elf, shield), Some((250, 34)));
        assert_eq!(paperdoll_archive(male_elf, EquipSlot::RightHand), 234);
        assert_eq!(paperdoll_archive(male_elf, EquipSlot::LeftHand), 250);

        let axe = inv.items.iter().find(|i| i.name.contains("Axe")).unwrap();
        assert_eq!(axe.slot, EquipSlot::RightHand);
        assert!(!axe.equipped);
        assert_eq!(icon_location(female_redguard, axe), Some((234, 0)));
        assert_eq!(axe.paperdoll_record, Some(0));

        let mut custom = sword.clone();
        custom.icon_source = None;
        assert_eq!(icon_location(female_redguard, &custom), Some((234, 12)));
    }

    #[test]
    fn layer_image_makes_index_zero_transparent_and_rect_is_in_percent() {
        let palette = {
            let mut bytes = Vec::new();
            for i in 0..256usize {
                bytes.extend_from_slice(&[i as u8, (i + 1) as u8, (i + 2) as u8]);
            }
            Palette::from_bytes("PAL.PAL", &bytes).unwrap()
        };
        let native = DfImage { x: 11, y: 46, width: 2, height: 1, pixels: vec![0, 5] };
        let image = layer_image(&native, &palette);
        assert_eq!(&image.data[0..4], &[0, 0, 0, 0]);
        assert_eq!(&image.data[4..8], &[5, 6, 7, 255]);

        let layer = HdLayer { image: Handle::default(), flip_y: false, x: 11, y: 46, w: 22, h: 92 };
        let (left, top, width, height) = layer_rect_percent(&layer, Vec2::new(110.0, 184.0));
        assert!((left - 10.0).abs() < 1e-4 && (top - 25.0).abs() < 1e-4);
        assert!((width - 20.0).abs() < 1e-4 && (height - 50.0).abs() < 1e-4);
    }

    #[test]
    fn compose_doll_draws_armor_layer_on_top() {
        let body = image(10, 20, 10, 20, 5);
        let head = image(12, 5, 4, 4, 9);
        let cuirass = image(10, 20, 10, 10, 22);
        let doll = compose_doll(&body, &head, &[cuirass], &palette());

        assert_eq!((doll.width, doll.height), (DOLL_WIDTH, DOLL_HEIGHT));
        let at = |x: usize, y: usize| doll.rgba[(y * DOLL_WIDTH + x) * 4..(y * DOLL_WIDTH + x) * 4 + 4].to_vec();
        assert_eq!(at(10, 20), vec![22, 23, 24, 255]);
    }
}
