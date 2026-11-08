//! Navi disponibili. Ogni tipo produce una mesh statica + (facoltativo) un
//! collider Rapier + componenti animati (vele, bandiere, timone).
//!
//! Il gioco carica UNA sola nave alla volta (`ShipKind` sul componente
//! `crate::ship::Ship`). La nave attuale (`Medieval`, `ship.glb`) resta
//! intatta: `ShipKind::Medieval` usa il GLB, `ShipKind::Elven` usa
//! `elven_ship::build()`.
//!
//! ## Aggiungere una nave nuova
//!
//! 1. crea `src/ships/<nome>_ship.rs` con una `pub fn build(...) -> BuiltShip`
//! 2. aggiungi una variante `ShipKind::<Nome>` qui sotto
//! 3. in `crate::ship::spawn_ship`, matcha sul kind e chiama il builder giusto
//! 4. aggiungi al `Plugin::build` di questo modulo eventuali sistemi di
//!    animazione specifici (vele, bandiere, timone)

pub mod elven_ship;

use bevy::prelude::*;
use std::sync::Arc;
use bevy_rapier3d::prelude::Collider;

use crate::data::mesh_builder::Submesh;

/// Quale modello di nave spawnare.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ShipKind {
    /// Navi medievali importate da `ship.glb` (comportamento attuale).
    #[default]
    Medieval,
    /// Veliero elfico procedurale (questo porting).
    Elven,
}

/// Mesh statica di una nave: submesh per ogni materiale, più eventuali
/// componenti animati da agganciare come figli della radice.
pub struct BuiltShip {
    /// Submesh raggruppati per indice di materiale (`ELVEN_MATERIALS`).
    pub submeshes: Vec<(ElvenMaterial, Submesh)>,
    /// Mesh totale (vertici/normali/uv condivisi fra i submesh).
    pub mesh: crate::data::mesh_builder::WorldMesh,
    /// Collider trimesh da attaccare alla radice con `Collider::trimesh`.
    pub collider: Option<Collider>,
    /// Altezza del ponte (per farci camminare il player).
    pub deck_height: f32,
    /// Offset locale del timone (dove si mette in piedi il player quando E).
    pub helm_local: Vec3,
    /// Componenti animati (vele, bandiere, timone) da spawnare come figli
    /// della radice. Contiene la descrizione di cosa spawnare, non le entità:
    /// `spawn_ship` le crea nel mondo.
    pub animated: Vec<AnimatedPart>,
}

/// Parte animata figlia della nave. Il chiamante spawna il figlio con la
/// `Transform` iniziale e i componenti giusti; l'animazione è gestita da
/// sistemi in `elven_ship.rs`.
pub enum AnimatedPart {
    /// Vela triangolare: 3 punti (tack, head, clew) e sagitta iniziale.
    TriangularSail {
        tack: Vec3,
        head: Vec3,
        clew: Vec3,
        initial_sag: f32,
    },
    /// Bandiera a coda di rondine: posizione, dimensione, materiale.
    SwallowtailFlag {
        pos: Vec3,
        width: f32,
        height: f32,
    },
    /// Ruota del timone: posizione, orientamento, raggio.
    RadialHelm {
        pos: Vec3,
        yaw: f32,
        radius: f32,
    },
}

/// Materiali del veliero elfico. Indice usato in `submeshes` per legare
/// ogni triangolo al materiale giusto in `elven_ship::materials()`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ElvenMaterial {
    Ivory,
    DeckWood,
    Gold,
    DarkWindow,
    DoorWood,
    BlueDoor,
    BlueRoof,
    Ruby,
    Topaz,
}