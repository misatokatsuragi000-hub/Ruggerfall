//! Dati condivisi: archivi di gioco, origine mobile e altezze del suolo.
//!
//! Sistema di coordinate: in Bevy X = est, Y = su, Z = -nord. Le posizioni sono
//! relative a un'origine mobile (in map pixel) così restano piccole anche a
//! centinaia di chilometri dal punto di partenza (precisione degli f32).
//!
//! ## Stagione
//!
//! `GameData::season` sostituisce il vecchio `winter: bool`. È `Season` (Summer,
//! Winter, Rain) di `crate::data::climate`, così anche la pioggia ha il suo
//! archivio di texture (offset +2) e non collassa su "non inverno". Il cambio a
//! runtime (es. tasto F4) funziona solo se qualcuno invalida `Streaming` per far
//! ricostruire i tile: vedi `tiles::Streaming::invalidate_all` e il sistema
//! `cycle_season` in `main.rs`.

use bevy::prelude::*;
use crate::data::climate::Season;
use crate::data::maps::DFLocation;
use crate::data::terrain_mesh::{terrain_height, CELL_SIZE, MAP_PIXEL_SIZE};
use crate::data::{HeightSamples, PakFile, Palette, WoodsFile};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub const MAP_WIDTH_PIXELS: i32 = 1000;
pub const MAP_HEIGHT_PIXELS: i32 = 500;
const S: f64 = MAP_PIXEL_SIZE as f64;
const CELL: f64 = CELL_SIZE as f64;

/// File di Daggerfall caricati in memoria.
#[derive(Resource, Clone)]
pub struct GameData {
    pub dir: PathBuf,
    pub woods: Arc<WoodsFile>,
    pub climate: Arc<PakFile>,
    pub palette: Arc<Palette>,
    /// Stagione corrente. Sostituisce il vecchio `winter: bool`:
    /// - `Summer` → archivi base;
    /// - `Winter` → archivi +1 (neve);
    /// - `Rain`   → archivi +2 (pioggia).
    ///
    /// Il cambio a runtime (es. tasto F4) funziona solo se qualcuno invalida
    /// `Streaming` (vedi `tiles::Streaming::invalidate_all`).
    pub season: Season,
    // Buffer per città, blocchi RMB e modelli 3D
    pub maps_buf: Arc<Vec<u8>>,
    pub blocks_buf: Arc<Vec<u8>>,
    pub arch3d_buf: Arc<Vec<u8>>,
    pub locations: Arc<HashMap<(i32, i32), DFLocation>>,
}

/// Origine mobile, in map pixel: `east` = colonna del map pixel, `north` = 500 - riga.
#[derive(Resource, Clone, Copy, Debug)]
pub struct Origin {
    pub east: i32,
    pub north: i32,
}

/// Altezze dei map pixel caricati, per far camminare il giocatore sul suolo.
#[derive(Resource, Default)]
pub struct HeightField {
    pub tiles: HashMap<(i32, i32), Arc<HeightSamples>>,
}

pub fn in_map(mx: i32, my: i32) -> bool {
    (0..MAP_WIDTH_PIXELS).contains(&mx) && (0..MAP_HEIGHT_PIXELS).contains(&my)
}

/// Coordinate assolute in metri (est, nord) di una posizione di Bevy.
pub fn absolute(origin: &Origin, pos: Vec3) -> (f64, f64) {
    (
        pos.x as f64 + origin.east as f64 * S,
     -(pos.z as f64) + origin.north as f64 * S,
    )
}

/// Map pixel (colonna, riga) che contiene una posizione di Bevy.
pub fn map_pixel_of(origin: &Origin, pos: Vec3) -> (i32, i32) {
    let (east, north) = absolute(origin, pos);
    ((east / S).floor() as i32, MAP_HEIGHT_PIXELS - (north / S).floor() as i32)
}

/// Posizione dell'angolo sud-ovest di un map pixel rispetto all'origine.
pub fn tile_translation(origin: &Origin, mx: i32, my: i32) -> Vec3 {
    Vec3::new(
        (mx - origin.east) as f32 * MAP_PIXEL_SIZE,
              0.0,
              -(((MAP_HEIGHT_PIXELS - my) - origin.north) as f32) * MAP_PIXEL_SIZE,
    )
}

impl HeightField {
    /// Altezza del suolo in metri nel punto (x, z) di Bevy, se il map pixel è caricato.
    pub fn ground(&self, origin: &Origin, x: f32, z: f32) -> Option<f32> {
        let (east, north) = absolute(origin, Vec3::new(x, 0.0, z));
        let mx = (east / S).floor() as i32;
        let my = MAP_HEIGHT_PIXELS - (north / S).floor() as i32;
        let samples = self.tiles.get(&(mx, my))?;
        let gx = ((east - mx as f64 * S) / CELL) as f32;
        let gy = ((north - (MAP_HEIGHT_PIXELS - my) as f64 * S) / CELL) as f32;
        Some(terrain_height(samples, gx, gy))
    }
}
