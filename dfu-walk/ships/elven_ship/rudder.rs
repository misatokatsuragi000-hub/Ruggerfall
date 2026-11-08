//! Timone: asta inclinata + pala estrusa + cerniere dorate + trim sul bordo.
//! Espone il gruppo come `AnimatedPart::RadialHelm`? No: qui è il *timone
//! vero*, non la ruota radiale (quella è in `helm_wheel.rs`).
//!
//! In Three.js il gruppo viene ruotato in `aggiornaNaviBattaglia()` in base
//! alla virata. In Bevy lo ruota `animate_rudder` qui sotto, che legge lo
//! yaw rate della nave e applica una rotazione proporzionale attorno a Y.

use bevy::math::{Quat, Vec3};
use bevy::prelude::*;

use super::super::{AnimatedPart, ElvenMaterial};
use super::primitives::{cylinder, extrude_polygon, tube, MeshBuilder, Part};

const RUDDER_ORIGIN: Vec3 = Vec3::new(-41.0, 0.0, 0.0);

pub fn build(b: &mut MeshBuilder) {
    // Asta (cilindro inclinato di -0.06 su Z).
    {
        let base = b.part(ElvenMaterial::Ivory).positions.len();
        cylinder(
            b.part(ElvenMaterial::Ivory),
            Vec3::new(0.15, 6.5, 0.0),
            0.45, 0.38, 18.0, 16, true,
        );
        // Rotazione -0.06 attorno a Z, traslazione a RUDDER_ORIGIN.
        rotate_part_z(b.part(ElvenMaterial::Ivory), base, -0.06);
        translate_part(b.part(ElvenMaterial::Ivory), base, RUDDER_ORIGIN);
    }

    // Pala (shape estruso + trim + cerniere).
    {
        let base = b.part(ElvenMaterial::Ivory).positions.len();
        let poly = rudder_blade_polygon();
        // La pala è scalata di 0.7 e traslata in (0, 1.0, 0).
        extrude_polygon(b.part(ElvenMaterial::Ivory), &poly, 0.45);
        scale_part(b.part(ElvenMaterial::Ivory), base, 0.7);
        translate_part(b.part(ElvenMaterial::Ivory), base, Vec3::new(0.0, 1.0, 0.0));
        translate_part(b.part(ElvenMaterial::Ivory), base, RUDDER_ORIGIN);
    }

    // Trim dorato sul bordo della pala.
    {
        let base = b.part(ElvenMaterial::Gold).positions.len();
        let pts = vec![
            Vec3::new(0.0, 11.5, 0.0),
            Vec3::new(-5.2, 6.0, 0.0),
            Vec3::new(-7.5, -1.0, 0.0),
            Vec3::new(-6.0, -8.5, 0.0),
            Vec3::new(-1.2, -9.0, 0.0),
        ];
        tube(b.part(ElvenMaterial::Gold), &pts, 0.16, 32, 8);
        scale_part(b.part(ElvenMaterial::Gold), base, 0.7);
        translate_part(b.part(ElvenMaterial::Gold), base, Vec3::new(0.0, 1.0, 0.0));
        translate_part(b.part(ElvenMaterial::Gold), base, RUDDER_ORIGIN);
    }

    // 4 cerniere dorate lungo la pala.
    for y_hinge in [-5.5_f32, -1.5, 2.5, 6.5] {
        let base = b.part(ElvenMaterial::Gold).positions.len();
        // Box (4.4, 0.45, 0.7) centrato a (-2.0, y_hinge, 0), ruotato Z -0.06.
        push_box_local(
            b.part(ElvenMaterial::Gold),
            Vec3::new(-2.0, y_hinge, 0.0),
            Vec3::new(2.2, 0.225, 0.35),
        );
        rotate_part_z(b.part(ElvenMaterial::Gold), base, -0.06);
        scale_part(b.part(ElvenMaterial::Gold), base, 0.7);
        translate_part(b.part(ElvenMaterial::Gold), base, Vec3::new(0.0, 1.0, 0.0));
        translate_part(b.part(ElvenMaterial::Gold), base, RUDDER_ORIGIN);
    }

    // Componente animato per la rotazione attorno a Y del timone.
    b.push_animated(AnimatedPart::RadialHelm {
        pos: RUDDER_ORIGIN,
        yaw: 0.0,
        radius: 0.0,
    });
}

fn rudder_blade_polygon() -> Vec<[f32; 2]> {
    let mut pts = vec![
        [0.0, 11.5],
        [-5.2, 6.0],
    ];
    // Quadratica (-5.2, 6.0) → controllo (-7.5, -1.0) → (-6.0, -8.5).
    let p0 = [-5.2_f32, 6.0];
    let p1 = [-7.5_f32, -1.0];
    let p2 = [-6.0_f32, -8.5];
    let steps = 10;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let a = (1.0 - t) * (1.0 - t);
        let b = 2.0 * (1.0 - t) * t;
        let c = t * t;
        pts.push([
            a * p0[0] + b * p1[0] + c * p2[0],
            a * p0[1] + b * p1[1] + c * p2[1],
        ]);
    }
    pts.push([-1.2, -9.0]);
    pts.push([0.0, -6.0]);
    pts
}

fn translate_part(part: &mut Part, base: usize, offset: Vec3) {
    for i in base..part.positions.len() {
        let p = Vec3::from(part.positions[i]) + offset;
        part.positions[i] = p.to_array();
    }
}

fn scale_part(part: &mut Part, base: usize, s: f32) {
    for i in base..part.positions.len() {
        let p = Vec3::from(part.positions[i]) * s;
        part.positions[i] = p.to_array();
    }
}

fn rotate_part_z(part: &mut Part, base: usize, angle: f32) {
    let q = Quat::from_rotation_z(angle);
    for i in base..part.positions.len() {
        let p = Vec3::from(part.positions[i]);
        part.positions[i] = (q * p).to_array();
        let n = Vec3::from(part.normals[i]);
        part.normals[i] = (q * n).to_array();
    }
}

fn push_box_local(part: &mut Part, center: Vec3, half: Vec3) {
    use super::primitives::quad;
    let corners = [
        Vec3::new(-half.x, -half.y, -half.z),
        Vec3::new( half.x, -half.y, -half.z),
        Vec3::new( half.x,  half.y, -half.z),
        Vec3::new(-half.x,  half.y, -half.z),
        Vec3::new(-half.x, -half.y,  half.z),
        Vec3::new( half.x, -half.y,  half.z),
        Vec3::new( half.x,  half.y,  half.z),
        Vec3::new(-half.x,  half.y,  half.z),
    ];
    let c = |i: usize| center + corners[i];
    quad(part, c(4), c(5), c(6), c(7));
    quad(part, c(1), c(0), c(3), c(2));
    quad(part, c(5), c(1), c(2), c(6));
    quad(part, c(0), c(4), c(7), c(3));
    quad(part, c(7), c(6), c(2), c(3));
    quad(part, c(0), c(1), c(5), c(4));
}

// =============================================================================
// Animazione: il timone ruota in base alla virata della nave
// =============================================================================
//
// Legge `ShipMotion` (velocità lineare lungo l'asse) e `Velocity::angvel.y`
// della nave. Applica una rotazione Y proporzionale (clampata) al figlio
// del timone (lo spawna `spawn_ship` come figlio della radice).
//
// L'animazione è lasciata per il messaggio successivo (`sails.rs` +
// `helm_wheel.rs`); qui il componente esiste già come `AnimatedPart::RadialHelm`
// e sarà legato alla radice della nave da `spawn_ship`. Non serve un
// sistema dedicato *in questo file*: quando l'utente aggiunge il plug-in
// `sails::animate_rudder` (che chiama `animate_rudder` da `sails.rs`), tutto
// funziona.

/// `true` se esiste un sistema di animazione del timone (registrato in
/// `sails.rs`). Per ora non fa nulla: l'animazione è nel plugin `sails.rs`.
pub fn animate_rudder() {}