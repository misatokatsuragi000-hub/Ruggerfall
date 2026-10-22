//! Mappa locale di una città: edifici dall'alto, con taverne (verde), negozi (rosso)
//! e gilde (blu) evidenziati. Tutto in memoria, senza dipendenze: produce un'immagine
//! RGBA (mappa + legenda) che il viewer carica come texture.
//!
//! Coordinate: quelle locali del map pixel (x a est, z = -nord, origine nell'angolo
//! sud-ovest), le stesse di `city_buildings`. Nell'immagine il nord è in alto.

use std::collections::{HashMap, HashSet};

use crate::arch3d::Arch3dModel;
use crate::blocks::RmbBlock;
use crate::bsa::BsaArchive;
use crate::building_gen::{Footprint, BLOCK_SIZE_M, DF_UNIT};
use crate::maps::DFLocation;
use crate::wall_gen::{classify_wall, wall_footprint};

pub const MAP_PX: usize = 640;
pub const STRIP_PX: usize = 56;

/// Dimensioni (larghezza, altezza) dell'immagine: mappa quadrata + striscia con la legenda.
pub fn image_size() -> (usize, usize) {
    (MAP_PX, MAP_PX + STRIP_PX)
}

// ---------------------------------------------------------------- categorie

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoiKind {
    Tavern,
    Shop,
    Guild,
}

impl PoiKind {
    pub const ALL: [PoiKind; 3] = [PoiKind::Tavern, PoiKind::Shop, PoiKind::Guild];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn color(self) -> [u8; 3] {
        match self {
            PoiKind::Tavern => [70, 210, 90],
            PoiKind::Shop => [230, 65, 60],
            PoiKind::Guild => [75, 125, 240],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PoiKind::Tavern => "TAVERNE",
            PoiKind::Shop => "NEGOZI",
            PoiKind::Guild => "GILDE",
        }
    }
}

/// Categoria dal `BuildingType` (valori dell'enum di Daggerfall Unity).
/// Taverne: 15. Gilde: sale delle gilde (11) e templi (14). Negozi: alchimista, armaiolo,
/// libraio, sarto, mobili, gioielli, emporio, banco dei pegni, armi. Banche e biblioteche
/// non sono incluse: aggiungi 3 o 10 qui se le vuoi come negozi.
pub fn poi_kind(building_type: u8) -> Option<PoiKind> {
    match building_type {
        15 => Some(PoiKind::Tavern),
        11 | 14 => Some(PoiKind::Guild),
        0 | 2 | 5 | 6 | 7 | 8 | 9 | 12 | 13 => Some(PoiKind::Shop),
        _ => None,
    }
}

pub fn building_type_name(t: u8) -> &'static str {
    match t {
        0 => "Alchemist",
        1 => "HouseForSale",
        2 => "Armorer",
        3 => "Bank",
        4 => "Town4",
        5 => "Bookseller",
        6 => "ClothingStore",
        7 => "FurnitureStore",
        8 => "GemStore",
        9 => "GeneralStore",
        10 => "Library",
        11 => "GuildHall",
        12 => "PawnShop",
        13 => "WeaponSmith",
        14 => "Temple",
        15 => "Tavern",
        16 => "Palace",
        17..=22 => "House",
        _ => "?",
    }
}

// ---------------------------------------------------------------- dati

#[derive(Clone, Copy, Debug)]
pub struct MapBuilding {
    pub cx: f32,
    pub cz: f32,
    pub half_w: f32,
    pub half_d: f32,
    /// stessa convenzione di `CityBuilder::add`
    pub rot_deg: f32,
    pub kind: Option<PoiKind>,
    pub building_type: u8,
}

/// Quadrato di mondo mostrato dall'immagine: angolo nord-ovest (x_min, z_min) e lato.
#[derive(Clone, Copy, Debug)]
pub struct MapView {
    pub x_min: f32,
    pub z_min: f32,
    pub side_m: f32,
}

#[derive(Clone, Debug)]
pub struct CityMap {
    pub name: String,
    /// rettangolo della località: [x_min, z_min, x_max, z_max]
    pub rect: [f32; 4],
    pub view: MapView,
    pub buildings: Vec<MapBuilding>,
    /// numero di edifici per categoria (indice = `PoiKind::index`)
    pub counts: [usize; 3],
}

impl CityMap {
    pub fn view(&self) -> MapView {
        self.view
    }
}

/// Raccoglie gli edifici della località con lo stesso piazzamento di `build_city_buildings`.
pub fn collect_city_map(location: &DFLocation, blocks_bsa: &BsaArchive, arch3d_bsa: &BsaArchive) -> CityMap {
    let bw = location.width.min(8);
    let bh = location.height.min(8);
    let start_bx = (8 - bw) / 2;
    let start_by = (8 - bh) / 2;

    let mut footprints: HashMap<u32, Option<Footprint>> = HashMap::new();
    let mut buildings = Vec::new();
    let mut seen: [HashSet<(usize, usize, u8)>; 3] = Default::default();

    for by in 0..bh {
        for bx in 0..bw {
            let Some(name) = location.block_names.get(by * location.width + bx) else { continue };
            let Some(raw) = blocks_bsa.get_by_name(name) else { continue };
            let Ok(rmb) = RmbBlock::parse(raw) else { continue };
            let is_wall_block = name.to_ascii_uppercase().starts_with("WALL");

            let gx = start_bx + bx;
            let gz = start_by + by;
            let bx0 = gx as f32 * BLOCK_SIZE_M;
            let bz0 = gz as f32 * BLOCK_SIZE_M;

            for obj in rmb.placed_models().iter().filter(|o| o.is_building) {
                let fp = footprints.entry(obj.model_id).or_insert_with(|| {
                    arch3d_bsa
                        .get_by_id(obj.model_id)
                        .and_then(|raw| Arch3dModel::parse(obj.model_id, raw).ok())
                        .and_then(|m| Footprint::from_model(&m))
                });
                let Some(fp) = *fp else { continue };
                let passes = if is_wall_block { classify_wall(&fp).is_some() } else { fp.classify().is_some() };
                if !passes {
                    continue;
                }

                let px = bx0 + obj.x * DF_UNIT;
                let pz = -(bz0 + obj.n * DF_UNIT);
                let rot = obj.rotation_deg;
                let (s, c) = rot.to_radians().sin_cos();
                let wx = px + fp.cx * c + fp.cz * s;
                let wz = pz - fp.cx * s + fp.cz * c;

                // le mura non sono negozi: i loro BuildingData non contano
                let data = if is_wall_block {
                    None
                } else {
                    obj.subrecord.and_then(|i| rmb.buildings.get(i as usize).map(|b| (i, *b)))
                };
                let (kind, building_type) = match data {
                    Some((i, b)) => {
                        let k = poi_kind(b.building_type);
                        if let Some(k) = k {
                            seen[k.index()].insert((gx, gz, i));
                        }
                        (k, b.building_type)
                    }
                    None => (None, 0xFF),
                };

                // le mura sono ruotate di 90 gradi come nel generatore
                let shape = if is_wall_block { wall_footprint(&fp) } else { fp };
                buildings.push(MapBuilding {
                    cx: wx,
                    cz: wz,
                    half_w: shape.width * 0.5,
                    half_d: shape.depth * 0.5,
                    rot_deg: rot,
                    kind,
                    building_type,
                });
            }
        }
    }

    let rect = [
        start_bx as f32 * BLOCK_SIZE_M,
        -((start_by + bh) as f32) * BLOCK_SIZE_M,
        (start_bx + bw) as f32 * BLOCK_SIZE_M,
        -(start_by as f32) * BLOCK_SIZE_M,
    ];
    let side = (rect[2] - rect[0]).max(rect[3] - rect[1]);
    let view = MapView {
        x_min: (rect[0] + rect[2]) * 0.5 - side * 0.5,
        z_min: (rect[1] + rect[3]) * 0.5 - side * 0.5,
        side_m: side,
    };

    CityMap {
        name: location.name.clone(),
        rect,
        view,
        buildings,
        counts: [seen[0].len(), seen[1].len(), seen[2].len()],
    }
}

// ---------------------------------------------------------------- disegno

const GROUND: [u8; 3] = [44, 52, 44];
const OUTSIDE: [u8; 3] = [22, 24, 24];
const GRID: [u8; 3] = [62, 72, 62];
const PLAIN_FILL: [u8; 3] = [120, 118, 112];
const PLAIN_EDGE: [u8; 3] = [70, 68, 64];
const STRIP_BG: [u8; 3] = [18, 18, 22];
const WHITE: [u8; 3] = [235, 235, 235];

fn put(buf: &mut [u8], w: usize, h: usize, x: i32, y: i32, c: [u8; 3]) {
    if x < 0 || y < 0 || x as usize >= w || y as usize >= h {
        return;
    }
    let i = (y as usize * w + x as usize) * 4;
    buf[i] = c[0];
    buf[i + 1] = c[1];
    buf[i + 2] = c[2];
    buf[i + 3] = 255;
}

fn fill_circle(buf: &mut [u8], w: usize, h: usize, cx: f32, cy: f32, r: f32, c: [u8; 3]) {
    let (x0, x1) = ((cx - r).floor() as i32, (cx + r).ceil() as i32);
    let (y0, y1) = ((cy - r).floor() as i32, (cy + r).ceil() as i32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            if dx * dx + dy * dy <= r * r {
                put(buf, w, h, x, y, c);
            }
        }
    }
}

fn darker(c: [u8; 3]) -> [u8; 3] {
    [(c[0] as f32 * 0.55) as u8, (c[1] as f32 * 0.55) as u8, (c[2] as f32 * 0.55) as u8]
}

fn draw_building(buf: &mut [u8], view: &MapView, scale: f32, b: &MapBuilding, fill: [u8; 3], edge: [u8; 3]) {
    let (s, c) = b.rot_deg.to_radians().sin_cos();
    let ex = b.half_w * c.abs() + b.half_d * s.abs();
    let ez = b.half_w * s.abs() + b.half_d * c.abs();
    let px0 = ((b.cx - ex - view.x_min) * scale).floor() as i32;
    let px1 = ((b.cx + ex - view.x_min) * scale).ceil() as i32;
    let py0 = ((b.cz - ez - view.z_min) * scale).floor() as i32;
    let py1 = ((b.cz + ez - view.z_min) * scale).ceil() as i32;
    let e = 1.0 / scale;
    for py in py0..=py1 {
        for px in px0..=px1 {
            let wx = view.x_min + (px as f32 + 0.5) / scale;
            let wz = view.z_min + (py as f32 + 0.5) / scale;
            let (dx, dz) = (wx - b.cx, wz - b.cz);
            // rotazione inversa di quella di CityBuilder::add
            let lx = dx * c - dz * s;
            let lz = dx * s + dz * c;
            if lx.abs() <= b.half_w && lz.abs() <= b.half_d {
                let on_edge = lx.abs() > b.half_w - e || lz.abs() > b.half_d - e;
                put(buf, MAP_PX, MAP_PX + STRIP_PX, px, py, if on_edge { edge } else { fill });
            }
        }
    }
}

/// Immagine RGBA `image_size()`: mappa in alto, legenda in basso.
pub fn render_city_map(map: &CityMap) -> Vec<u8> {
    let (w, h) = image_size();
    let mut buf = vec![0u8; w * h * 4];
    let view = map.view;
    let scale = MAP_PX as f32 / view.side_m;

    // sfondo: dentro la località più chiaro
    for py in 0..MAP_PX {
        for px in 0..MAP_PX {
            let wx = view.x_min + (px as f32 + 0.5) / scale;
            let wz = view.z_min + (py as f32 + 0.5) / scale;
            let inside = wx >= map.rect[0] && wx <= map.rect[2] && wz >= map.rect[1] && wz <= map.rect[3];
            put(&mut buf, w, h, px as i32, py as i32, if inside { GROUND } else { OUTSIDE });
        }
    }

    // griglia dei blocchi
    for k in 0..=8 {
        let x = k as f32 * BLOCK_SIZE_M;
        if x >= map.rect[0] - 0.01 && x <= map.rect[2] + 0.01 {
            let px = ((x - view.x_min) * scale).round() as i32;
            for py in 0..MAP_PX as i32 {
                put(&mut buf, w, h, px, py, GRID);
            }
        }
        let z = -(k as f32) * BLOCK_SIZE_M;
        if z >= map.rect[1] - 0.01 && z <= map.rect[3] + 0.01 {
            let py = ((z - view.z_min) * scale).round() as i32;
            for px in 0..MAP_PX as i32 {
                put(&mut buf, w, h, px, py, GRID);
            }
        }
    }

    // edifici comuni, poi quelli evidenziati, poi i pallini
    for b in map.buildings.iter().filter(|b| b.kind.is_none()) {
        draw_building(&mut buf, &view, scale, b, PLAIN_FILL, PLAIN_EDGE);
    }
    for b in map.buildings.iter().filter(|b| b.kind.is_some()) {
        let col = b.kind.unwrap().color();
        draw_building(&mut buf, &view, scale, b, col, darker(col));
    }
    for b in map.buildings.iter().filter(|b| b.kind.is_some()) {
        let col = b.kind.unwrap().color();
        let (cx, cy) = ((b.cx - view.x_min) * scale, (b.cz - view.z_min) * scale);
        fill_circle(&mut buf, w, h, cx, cy, 6.5, [12, 12, 12]);
        fill_circle(&mut buf, w, h, cx, cy, 4.5, col);
    }

    // cornice
    for i in 0..MAP_PX as i32 {
        for t in 0..2 {
            put(&mut buf, w, h, i, t, WHITE);
            put(&mut buf, w, h, i, MAP_PX as i32 - 1 - t, WHITE);
            put(&mut buf, w, h, t, i, WHITE);
            put(&mut buf, w, h, MAP_PX as i32 - 1 - t, i, WHITE);
        }
    }

    // striscia: nome e legenda
    for py in MAP_PX..h {
        for px in 0..w {
            put(&mut buf, w, h, px as i32, py as i32, STRIP_BG);
        }
    }
    draw_text(&mut buf, w, h, 10, MAP_PX as i32 + 6, &map.name, 3, WHITE);
    let mut x = 10;
    let y = MAP_PX as i32 + 34;
    for k in PoiKind::ALL {
        let col = k.color();
        for dy in 0..12 {
            for dx in 0..12 {
                put(&mut buf, w, h, x + dx, y + 1 + dy, col);
            }
        }
        let text = format!("{} {}", k.label(), map.counts[k.index()]);
        let adv = draw_text(&mut buf, w, h, x + 18, y, &text, 2, WHITE);
        x += 18 + adv + 26;
    }
    buf
}

/// Immagine con un solo messaggio al centro (nessuna città, errore...).
pub fn render_notice(text: &str) -> Vec<u8> {
    let (w, h) = image_size();
    let mut buf = vec![0u8; w * h * 4];
    for py in 0..h {
        for px in 0..w {
            put(&mut buf, w, h, px as i32, py as i32, STRIP_BG);
        }
    }
    let width = text.chars().count() as i32 * 6 * 3;
    draw_text(&mut buf, w, h, (w as i32 - width) / 2, h as i32 / 2 - 10, text, 3, WHITE);
    buf
}

// ---------------------------------------------------------------- font 5x7

fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        '2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
        '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        '6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        '9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        '-' => [0, 0, 0, 0b11111, 0, 0, 0],
        '\'' => [0b00100, 0b00100, 0b01000, 0, 0, 0, 0],
        '.' => [0, 0, 0, 0, 0, 0b01100, 0b01100],
        _ => [0; 7],
    }
}

/// Disegna `text` in maiuscolo; ritorna la larghezza in pixel.
fn draw_text(buf: &mut [u8], w: usize, h: usize, x: i32, y: i32, text: &str, scale: i32, color: [u8; 3]) -> i32 {
    let mut cx = x;
    for ch in text.chars() {
        let g = glyph(ch);
        for (row, &bits) in g.iter().enumerate() {
            for col in 0..5i32 {
                if (bits >> (4 - col)) & 1 == 1 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            put(buf, w, h, cx + col * scale + sx, y + row as i32 * scale + sy, color);
                        }
                    }
                }
            }
        }
        cx += 6 * scale;
    }
    cx - x
}

// ---------------------------------------------------------------- test

#[cfg(test)]
mod tests {
    use super::*;

    fn test_map(buildings: Vec<MapBuilding>) -> CityMap {
        CityMap {
            name: "TEST".into(),
            rect: [0.0, -102.4, 102.4, 0.0],
            view: MapView { x_min: 0.0, z_min: -102.4, side_m: 102.4 },
            buildings,
            counts: [1, 0, 0],
        }
    }

    fn pixel(buf: &[u8], map: &CityMap, wx: f32, wz: f32) -> [u8; 3] {
        let scale = MAP_PX as f32 / map.view.side_m;
        let px = ((wx - map.view.x_min) * scale) as usize;
        let py = ((wz - map.view.z_min) * scale) as usize;
        let i = (py * MAP_PX + px) * 4;
        [buf[i], buf[i + 1], buf[i + 2]]
    }

    fn building(rot: f32, kind: Option<PoiKind>) -> MapBuilding {
        MapBuilding { cx: 50.0, cz: -50.0, half_w: 10.0, half_d: 4.0, rot_deg: rot, kind, building_type: 0 }
    }

    #[test]
    fn categories() {
        assert_eq!(poi_kind(15), Some(PoiKind::Tavern));
        assert_eq!(poi_kind(11), Some(PoiKind::Guild));
        assert_eq!(poi_kind(14), Some(PoiKind::Guild));
        for t in [0, 2, 5, 6, 7, 8, 9, 12, 13] {
            assert_eq!(poi_kind(t), Some(PoiKind::Shop), "tipo {t}");
        }
        assert_eq!(poi_kind(1), None); // casa in vendita
        assert_eq!(poi_kind(17), None); // casa
        assert_eq!(PoiKind::Tavern.color()[1] > PoiKind::Tavern.color()[0], true); // verde
        assert_eq!(PoiKind::Shop.color()[0] > PoiKind::Shop.color()[2], true); // rosso
        assert_eq!(PoiKind::Guild.color()[2] > PoiKind::Guild.color()[0], true); // blu
    }

    #[test]
    fn image_has_expected_size_and_marker_color() {
        let map = test_map(vec![building(0.0, Some(PoiKind::Tavern))]);
        let buf = render_city_map(&map);
        let (w, h) = image_size();
        assert_eq!(buf.len(), w * h * 4);
        assert_eq!(pixel(&buf, &map, 50.0, -50.0), PoiKind::Tavern.color());
        // un po' fuori dal pallino ma dentro l'edificio: colore di riempimento
        assert_eq!(pixel(&buf, &map, 57.0, -50.0), PoiKind::Tavern.color());
    }

    #[test]
    fn rotation_is_applied_like_city_builder() {
        // 90 gradi: l'asse lungo (x locale) diventa l'asse z del mondo
        let map = test_map(vec![building(90.0, None)]);
        let buf = render_city_map(&map);
        assert_eq!(pixel(&buf, &map, 50.0, -42.0), PLAIN_FILL); // lungo z: dentro
        assert_eq!(pixel(&buf, &map, 58.0, -50.0), GROUND); // lungo x: fuori
        // senza rotazione e' il contrario
        let map = test_map(vec![building(0.0, None)]);
        let buf = render_city_map(&map);
        assert_eq!(pixel(&buf, &map, 58.0, -50.0), PLAIN_FILL);
        assert_eq!(pixel(&buf, &map, 50.0, -42.0), GROUND);
    }

    #[test]
    fn text_draws_pixels() {
        let buf = render_notice("NESSUNA CITTA");
        let bg = [STRIP_BG[0], STRIP_BG[1], STRIP_BG[2], 255];
        assert!(buf.chunks_exact(4).any(|p| p != bg));
        let (w, h) = image_size();
        assert_eq!(buf.len(), w * h * 4);
    }
}
