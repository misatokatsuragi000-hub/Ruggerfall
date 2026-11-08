//! Baliste: 4 esemplari sul ponte centrale, due per lato. `createBallista`
//! del Three.js ridotto alle sole primitive che servono (cilindri, box,
//! tube, coni).

use bevy::math::Quat;
use bevy::prelude::Vec3;

use super::super::ElvenMaterial;
use super::primitives::{cone, cylinder, quad, torus_horizontal, tri, tube, MeshBuilder, Part};

pub fn build(b: &mut MeshBuilder) {
    let configs = [
        (Vec3::new(-4.0, 2.85, -3.8), -1.0_f32),
        (Vec3::new( 4.5, 2.85, -3.8), -1.0),
        (Vec3::new(-4.0, 2.85,  3.8),  1.0),
        (Vec3::new( 4.5, 2.85,  3.8),  1.0),
    ];
    for (pos, side) in configs {
        // Tre ruotate: base a Y=+Y (default), corpo ruotato di ±π/2 su Y.
        ballista(b, pos, if side > 0.0 { -std::f32::consts::FRAC_PI_2 } else { std::f32::consts::FRAC_PI_2 });
    }
}

fn ballista(b: &mut MeshBuilder, center: Vec3, yaw: f32) {
    let q = Quat::from_rotation_y(yaw);

    // Pedestal (cilindro avorio).
    cylinder(
        b.part(ElvenMaterial::Ivory),
        center + q * Vec3::new(0.0, 0.32, 0.0),
        0.38, 0.28, 0.65, 12, true,
    );
    // Anello dorato di base (toro orizzontale).
    torus_horizontal(
        b.part(ElvenMaterial::Gold),
        center + q * Vec3::new(0.0, 0.06, 0.0),
        0.06, 0.38, 16, 8,
    );
    // Fork (box dorato).
    box_part(
        b.part(ElvenMaterial::Gold),
        center + q * Vec3::new(0.0, 0.75, 0.0),
        Vec3::new(0.20, 0.45, 0.40),
        yaw,
    );
    // Stock (box avorio ruotato di -0.15 su Z, poi Y esterna).
    let stock_local_yaw = -0.15_f32;
    let stock_q = q * Quat::from_rotation_z(stock_local_yaw);
    push_box_oriented(
        b.part(ElvenMaterial::Ivory),
        center + q * Vec3::new(0.30, 0.95, 0.0),
        Vec3::new(2.4, 0.22, 0.32),
        stock_q,
    );
    // Rail dorata sopra lo stock.
    push_box_oriented(
        b.part(ElvenMaterial::Gold),
        center + q * Vec3::new(0.30, 1.08, 0.0),
        Vec3::new(2.4, 0.06, 0.10),
        stock_q,
    );
    // Arco (bow) come tube su 5 punti.
    {
        let pts = vec![
            Vec3::new(1.1, 1.08, -1.3),
            Vec3::new(1.3, 1.05, -0.6),
            Vec3::new(1.4, 1.02,  0.0),
            Vec3::new(1.3, 1.05,  0.6),
            Vec3::new(1.1, 1.08,  1.3),
        ];
        let part = b.part(ElvenMaterial::Gold);
        let base = part.positions.len();
        tube(part, &pts, 0.08, 16, 6);
        transform_from(part, base, center, yaw);
    }
    // Bolt (cilindro avorio ruotato -0.15 su Z).
    {
        let bolt_q = q * Quat::from_rotation_z(-0.15);
        cylinder_oriented(
            b.part(ElvenMaterial::Ivory),
            center + q * Vec3::new(0.5, 1.12, 0.0),
            0.04, 0.04, 2.0, 6, bolt_q,
        );
    }
    // Punta del bolt (cono a 4 lati, asse ruotato).
    {
        let head_q = q * Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2 - 0.15);
        cone_oriented(
            b.part(ElvenMaterial::Gold),
            center + q * Vec3::new(1.5, 0.96, 0.0),
            0.12, 0.45, 4, head_q,
        );
    }
    // Winch (cilindro dorato orizzontale).
    {
        let winch_q = q * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
        cylinder_oriented(
            b.part(ElvenMaterial::Gold),
            center + q * Vec3::new(-0.75, 1.15, 0.0),
            0.12, 0.12, 0.45, 8, winch_q,
        );
    }
}

// =============================================================================
// Helper locale: box e cilindro con orientamento arbitrario
// =============================================================================

fn transform_from(part: &mut Part, base: usize, center: Vec3, yaw: f32) {
    let q = Quat::from_rotation_y(yaw);
    for i in base..part.positions.len() {
        let p = Vec3::from(part.positions[i]);
        part.positions[i] = (center + q * p).to_array();
        let n = Vec3::from(part.normals[i]);
        part.normals[i] = (q * n).to_array();
    }
}

fn box_part(part: &mut Part, center: Vec3, half: Vec3, yaw: f32) {
    let q = Quat::from_rotation_y(yaw);
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
    let c = |i: usize| center + q * corners[i];
    quad(part, c(4), c(5), c(6), c(7));
    quad(part, c(1), c(0), c(3), c(2));
    quad(part, c(5), c(1), c(2), c(6));
    quad(part, c(0), c(4), c(7), c(3));
    quad(part, c(7), c(6), c(2), c(3));
    quad(part, c(0), c(1), c(5), c(4));
}

fn push_box_oriented(part: &mut Part, center: Vec3, half: Vec3, q: Quat) {
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
    let c = |i: usize| center + q * corners[i];
    quad(part, c(4), c(5), c(6), c(7));
    quad(part, c(1), c(0), c(3), c(2));
    quad(part, c(5), c(1), c(2), c(6));
    quad(part, c(0), c(4), c(7), c(3));
    quad(part, c(7), c(6), c(2), c(3));
    quad(part, c(0), c(1), c(5), c(4));
}

fn cylinder_oriented(part: &mut Part, center: Vec3, r_bot: f32, r_top: f32, h: f32, sides: usize, q: Quat) {
    let base = part.positions.len();
    cylinder(part, Vec3::ZERO, r_bot, r_top, h, sides, true);
    for i in base..part.positions.len() {
        let p = Vec3::from(part.positions[i]);
        part.positions[i] = (center + q * p).to_array();
        let n = Vec3::from(part.normals[i]);
        part.normals[i] = (q * n).to_array();
    }
}

fn cone_oriented(part: &mut Part, center: Vec3, r: f32, h: f32, sides: usize, q: Quat) {
    let base = part.positions.len();
    // `cone` costruisce con base a ZERO e punta verso +Y: la centro a h/2
    // per farla ruotare attorno al centro.
    cone(part, Vec3::new(0.0, -h * 0.5, 0.0), r, h, sides);
    for i in base..part.positions.len() {
        let p = Vec3::from(part.positions[i]);
        part.positions[i] = (center + q * p).to_array();
        let n = Vec3::from(part.normals[i]);
        part.normals[i] = (q * n).to_array();
    }
    let _ = tri; // warning suppression
}