//! Castello di poppa: quattro livelli P1-P4 a torre, ognuno con merlatura
//! fitta, cornice dorata, porta d'accesso e anello di finestre. In cima a P4
//! c'è la terrazza merlata su cui vive l'Arcimago (vedi `mage.rs`).
//!
//! `aftCastleGroup` in Three.js stava in `(-26.5, 4.9, 0)`: tutti i `center`
//! sono traslati di quell'offset (in coordinate nave).

use bevy::math::Vec3;

use super::super::ElvenMaterial;
use super::decorations::{
    add_castle_windows, elven_door, parapet_with_dense_merlons,
};
use super::primitives::{cylinder, MeshBuilder};

const AFT_ORIGIN: Vec3 = Vec3::new(-26.5, 4.9, 0.0);

pub fn build(b: &mut MeshBuilder) {
    // ============================================================
    // P1 (base, r ~2.6, h 2.4) a y 1.2 locale
    // ============================================================
    let p1 = AFT_ORIGIN;
    cylinder(b.part(ElvenMaterial::Ivory), p1 + Vec3::new(0.0, 1.2, 0.0), 2.6, 2.3, 2.4, 8, true);
    cylinder(b.part(ElvenMaterial::Gold),  p1 + Vec3::new(0.0, 2.4, 0.0), 2.35, 2.65, 0.28, 8, true);
    parapet_with_dense_merlons(b, p1 + Vec3::new(0.0, 2.54, 0.0), 2.62, 24, 0.32, 0.20, 0.30);
    elven_door(b, p1 + Vec3::new(2.26, 0.05, 0.0), std::f32::consts::FRAC_PI_2, 0.9, 1.45, 0.45, true);

    // ============================================================
    // P2 (r ~1.9, h 2.2) a y 3.5
    // ============================================================
    let p2 = AFT_ORIGIN;
    cylinder(b.part(ElvenMaterial::Ivory), p2 + Vec3::new(0.0, 3.5, 0.0), 1.9, 1.7, 2.2, 8, true);
    cylinder(b.part(ElvenMaterial::Gold),  p2 + Vec3::new(0.0, 4.6, 0.0), 1.75, 2.05, 0.24, 8, true);
    parapet_with_dense_merlons(b, p2 + Vec3::new(0.0, 4.72, 0.0), 2.02, 18, 0.28, 0.18, 0.26);
    elven_door(b, p2 + Vec3::new(1.68, 2.45, 0.0), std::f32::consts::FRAC_PI_2, 0.75, 1.25, 0.20, false);

    // ============================================================
    // P3 (r ~1.35, h 4.0) a y 6.6
    // ============================================================
    let p3 = AFT_ORIGIN;
    cylinder(b.part(ElvenMaterial::Ivory), p3 + Vec3::new(0.0, 6.6, 0.0), 1.35, 1.20, 4.0, 6, true);
    cylinder(b.part(ElvenMaterial::Gold),  p3 + Vec3::new(0.0, 8.6, 0.0), 1.25, 1.45, 0.20, 6, true);
    parapet_with_dense_merlons(b, p3 + Vec3::new(0.0, 8.7, 0.0), 1.42, 14, 0.24, 0.16, 0.22);
    elven_door(b, p3 + Vec3::new(1.18, 4.65, 0.0), std::f32::consts::FRAC_PI_2, 0.6, 1.05, 0.16, false);

    // ============================================================
    // P4 (r ~0.9, h 1.8) a y 9.5
    // ============================================================
    let p4 = AFT_ORIGIN;
    cylinder(b.part(ElvenMaterial::Ivory), p4 + Vec3::new(0.0, 9.5, 0.0), 0.90, 0.75, 1.8, 6, true);
    // Balcony aperto (openEnded, senza caps): solo il bordo esterno.
    cylinder(b.part(ElvenMaterial::Gold), p4 + Vec3::new(0.0, 10.4, 0.0), 0.80, 0.95, 0.30, 12, false);
    elven_door(b, p4 + Vec3::new(0.72, 8.65, 0.0), std::f32::consts::FRAC_PI_2, 0.45, 0.8, 0.12, false);

    // Terrazza merlata sommitale (dove viene piazzato l'Arcimago).
    parapet_with_dense_merlons(b, p4 + Vec3::new(0.0, 10.5, 0.0), 0.82, 10, 0.35, 0.14, 0.09);

    // ============================================================
    // Finestre (4 anelli, uno per livello)
    // ============================================================
    add_castle_windows(
        b,
        p1 + Vec3::new(0.0, 1.2, 0.0),
        2.45, 8,
        &[67.5, 112.5, 157.5, 202.5, 247.5, 292.5],
        0.30, 1.30,
    );
    add_castle_windows(
        b,
        p2 + Vec3::new(0.0, 3.5, 0.0),
        1.80, 8,
        &[67.5, 112.5, 157.5, 202.5, 247.5, 292.5],
        0.24, 1.00,
    );
    add_castle_windows(
        b,
        p3 + Vec3::new(0.0, 6.6, 0.0),
        1.28, 6,
        &[60.0, 120.0, 180.0, 240.0, 300.0],
        0.20, 0.85,
    );
    add_castle_windows(
        b,
        p4 + Vec3::new(0.0, 9.5, 0.0),
        0.825, 6,
        &[60.0, 120.0, 180.0, 240.0, 300.0],
        0.16, 0.65,
    );
}