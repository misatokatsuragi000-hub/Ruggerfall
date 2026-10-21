//! Dati condivisi: archivi di gioco, origine mobile e altezze del suolo.
//!
//! Sistema di coordinate: in Bevy X = est, Y = su, Z = -nord. Le posizioni sono
//! relative a un'origine mobile (in map pixel) così restano piccole anche a
//! centinaia di chilometri dal punto di partenza (precisione degli f32).

use bevy::prelude::*;
use daggerfall_data::terrain_mesh::{terrain_height, CELL_SIZE, MAP_PIXEL_SIZE};
use daggerfall_data::{HeightSamples, PakFile, Palette, WoodsFile};
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
    pub winter: bool,
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
