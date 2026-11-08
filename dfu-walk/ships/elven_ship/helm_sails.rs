//! Timone radiale + vele + bandiere del veliero elfico.
//!
//! ## Cosa contiene
//!
//! - **Ruota del timone** (`build_helm_wheel`): 8 razze, mozzo dorato con
//!   gemma topazio, piedistallo + colonna (avorio) + doppio filetto dorato.
//!   Costruita **statica** in `MeshBuilder` (è solo decorazione sulla
//!   terrazza P4, non si anima).
//! - **3 vele triangolari** (`build_sails`): mesh costruite una volta sola
//!   con la sagitta base **ridotta a 1/10**. Tutta la deformazione (gonfiore
//!   nella direzione del vento, svolazzio, caduta) è delegata al vertex
//!   shader `sail_animator.wgsl` tramite `SailMaterial`.
//! - **3 bandiere a coda di rondine** (`build_flags`): mesh piatte; sventolano
//!   nella direzione del vento, sempre via vertex shader.
//! - **`ElvenAnimatedMaterials`**: due `Handle<SailMaterial>` (vele e
//!   bandiere) i cui `params` (`time`, `wind_strength`, `wind_angle`) vengono
//!   aggiornati ogni frame da `ElvenShipPlugin::update_sail_time` leggendo
//!   `OceanSea.storm` e `OceanSea.wind_angle`: **le vele seguono il vento
//!   reale del mare** (magnitudine + direzione).
//! - **`spawn_elven_animated_parts`**: crea le entità figlie.
//!
//! ## Texture delle vele
//!
//! Le vele usano `assets/textures/vele.png` (percorso Bevy `textures/vele.png`).
//! Se il file manca, si ricade su una texture procedurale "magica" (blu notte
//! con bagliori diagonali) generata a runtime: il gioco non si rompe e il log
//! mostra un warning. La texture procedurale resta disponibile come fallback.
//!
//! ## Perché GPU
//!
//! La mesh è statica e viene deformata interamente dal vertex shader. Costo
//! CPU per frame: 3 `f32` per materiale (`time`, `wind_strength`, `wind_angle`).
//!
//! ## Layout uniform
//!
//! `SailParams` (in `sail_material.rs`) ha **8 campi, 32 byte** (7 scalari + 1
//! pad di allineamento a 16). Deve combaciare **byte-per-byte** con
//! `SailParams` in `sail_animator.wgsl`. Se aggiungi un campo, mettilo in
//! fondo a entrambi.

use bevy::math::Vec3;
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use super::super::{AnimatedPart, ElvenMaterial};
use super::primitives::{cylinder, quad, sphere, torus_horizontal, MeshBuilder, Part};
use super::sail_material::{SailExtension, SailMaterial, SailParams};

// =============================================================================
// 1. RUOTA RADIALE DEL TIMONE (statica)
// =============================================================================

const WHEEL_TOTAL_H: f32 = 0.5;
const WHEEL_OUTER_R: f32 = 0.15;
const WHEEL_RIM_R: f32 = 0.12;
const WHEEL_KNOB_R: f32 = 0.038;
const WHEEL_KNOB_CR: f32 = WHEEL_RIM_R + 0.012;
const WHEEL_SPOKES: usize = 8;
const WHEEL_CENTER_Y: f32 = WHEEL_TOTAL_H - WHEEL_OUTER_R;

/// Costruisce la ruota del timone sulla terrazza P4 dell'aft castle
/// (`aftCastleGroup.position = (-26.5, 4.9, 0)`, terrazza a y 10.58).
/// Ruota di π/2 su Y per allineare l'asse della ruota a X (prua/poppa).
pub fn build_helm_wheel(b: &mut MeshBuilder) {
    let anchor = Vec3::new(0.42, 10.58, 0.0);
    let rot = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
    let to_world = |p: Vec3| anchor + rot * p;

    // === Piedistallo ===
    cylinder(
        b.part(ElvenMaterial::Ivory),
             to_world(Vec3::new(0.0, 0.04, 0.0)),
             0.26, 0.22, 0.08, 16, true,
    );
    torus_horizontal(
        b.part(ElvenMaterial::Gold),
                     to_world(Vec3::new(0.0, 0.082, 0.0)),
                     0.02, 0.235, 24, 8,
    );

    // === Colonna ===
    let post_h = WHEEL_CENTER_Y - 0.10;
    cylinder(
        b.part(ElvenMaterial::Ivory),
             to_world(Vec3::new(0.0, 0.08 + post_h * 0.5, 0.0)),
             0.075, 0.050, post_h, 12, true,
    );
    cylinder(
        b.part(ElvenMaterial::Gold),
             to_world(Vec3::new(0.0, WHEEL_CENTER_Y - 0.045, 0.0)),
             0.062, 0.088, 0.09, 12, true,
    );

    // === Ruota (piano XY locale) ===
    let wheel_center_local = Vec3::new(0.0, WHEEL_CENTER_Y, -0.15);
    let wheel_center_w = to_world(wheel_center_local);
    wheel_torus_xy(
        b.part(ElvenMaterial::Ivory),
                   wheel_center_w,
                   rot,
                   0.028, WHEEL_RIM_R, 40, 10,
    );
    for z_off in [0.026_f32, -0.026] {
        let c = wheel_center_w + rot * Vec3::new(0.0, 0.0, z_off);
        wheel_torus_xy(
            b.part(ElvenMaterial::Gold),
                       c,
                       rot,
                       0.009, WHEEL_RIM_R, 40, 6,
        );
    }

    // === 8 razze + manici + pomelli d'oro ===
    for s in 0..WHEEL_SPOKES {
        let ang = (s as f32 / WHEEL_SPOKES as f32) * std::f32::consts::TAU;
        let dir = Vec3::new(ang.cos(), ang.sin(), 0.0);

        let spoke_center_local = dir * (WHEEL_RIM_R + 0.02) * 0.5;
        let spoke_center_w = to_world(wheel_center_local + spoke_center_local);
        let spoke_q = rot * Quat::from_rotation_z(ang - std::f32::consts::FRAC_PI_2);
        cylinder_oriented(
            b.part(ElvenMaterial::Ivory),
                          spoke_center_w,
                          0.021, 0.014, WHEEL_RIM_R + 0.02, 6, spoke_q,
        );

        let handle_center_local = dir * (WHEEL_RIM_R + 0.045);
        let handle_center_w = to_world(wheel_center_local + handle_center_local);
        cylinder_oriented(
            b.part(ElvenMaterial::Ivory),
                          handle_center_w,
                          0.015, 0.015, 0.10, 6, spoke_q,
        );

        let collar_local = dir * (WHEEL_RIM_R + 0.018);
        let collar_center_w = to_world(wheel_center_local + collar_local);
        torus_axis_xy(
            b.part(ElvenMaterial::Gold),
                      collar_center_w,
                      rot,
                      0.008, 0.026, 12, 6,
        );

        let knob_local = dir * WHEEL_KNOB_CR;
        let knob_center_w = to_world(wheel_center_local + knob_local);
        sphere(
            b.part(ElvenMaterial::Gold),
               knob_center_w,
               WHEEL_KNOB_R, 10, 10,
        );
    }

    // === Mozzo (cilindro dorato + doppia cornice + gemma topazio) ===
    cylinder_oriented(
        b.part(ElvenMaterial::Gold),
                      wheel_center_w,
                      0.088, 0.088, 0.11, 16,
                      rot * Quat::from_rotation_x(std::f32::consts::FRAC_PI_2),
    );
    for z_off in [0.05f32, -0.05] {
        let c = wheel_center_w + rot * Vec3::new(0.0, 0.0, z_off);
        torus_axis_xy(b.part(ElvenMaterial::Gold), c, rot, 0.024, 0.095, 28, 8);
        for b_i in 0..4 {
            let a = (b_i as f32 / 4.0) * std::f32::consts::TAU + std::f32::consts::FRAC_PI_4;
            let stud_local = wheel_center_local
            + Vec3::new(a.cos() * 0.095, a.sin() * 0.095, z_off * 1.35);
            sphere(b.part(ElvenMaterial::Gold), to_world(stud_local), 0.014, 8, 8);
        }
    }
    let gem_base_local = wheel_center_local + Vec3::new(0.0, 0.0, 0.058);
    let gem_center_w = to_world(gem_base_local);
    gem_pyramid_oriented(
        b.part(ElvenMaterial::Topaz),
                         gem_center_w,
                         0.056, 0.072, rot,
    );
    let gem_base_local_b = wheel_center_local + Vec3::new(0.0, 0.0, -0.058);
    let gem_center_w_b = to_world(gem_base_local_b);
    gem_pyramid_oriented(
        b.part(ElvenMaterial::Topaz),
                         gem_center_w_b,
                         0.056, 0.072,
                         rot * Quat::from_rotation_y(std::f32::consts::PI),
    );
}

fn wheel_torus_xy(part: &mut Part, center: Vec3, q: Quat, r_tube: f32, r_ring: f32, segs: usize, tube_segs: usize) {
    for i in 0..segs {
        let u0 = (i as f32 / segs as f32) * std::f32::consts::TAU;
        let u1 = ((i + 1) as f32 / segs as f32) * std::f32::consts::TAU;
        for j in 0..tube_segs {
            let v0 = (j as f32 / tube_segs as f32) * std::f32::consts::TAU;
            let v1 = ((j + 1) as f32 / tube_segs as f32) * std::f32::consts::TAU;
            let p = |u: f32, v: f32| -> Vec3 {
                let rr = r_ring + r_tube * v.cos();
                center + q * Vec3::new(rr * u.cos(), rr * u.sin(), r_tube * v.sin())
            };
            quad(part, p(u0, v0), p(u1, v0), p(u1, v1), p(u0, v1));
        }
    }
}

fn torus_axis_xy(part: &mut Part, center: Vec3, q: Quat, r_tube: f32, r_ring: f32, segs: usize, tube_segs: usize) {
    wheel_torus_xy(part, center, q, r_tube, r_ring, segs, tube_segs);
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

fn gem_pyramid_oriented(part: &mut Part, center: Vec3, r: f32, h: f32, q: Quat) {
    let tip_local = Vec3::new(0.0, 0.0, h);
    let ring_local = [
        Vec3::new(-r, 0.0, 0.0),
        Vec3::new(0.0, r, 0.0),
        Vec3::new(r, 0.0, 0.0),
        Vec3::new(0.0, -r, 0.0),
    ];
    for k in 0..4 {
        let a = center + q * ring_local[k];
        let b = center + q * ring_local[(k + 1) % 4];
        let t = center + q * tip_local;
        use super::primitives::tri;
        tri(part, a, b, t, [1.0; 4], [0.0, 0.0], [1.0, 0.0], [0.5, 1.0]);
    }
}

// =============================================================================
// 2. MATERIALI (due handle: uno per le vele, uno per le bandiere)
// =============================================================================

/// Contiene i due `Handle<SailMaterial>` condivisi da tutte le vele e da
/// tutte le bandiere. I loro `params.time`, `params.wind_strength` e
/// `params.wind_angle` vengono aggiornati ogni frame da
/// `ElvenShipPlugin::update_sail_time`, leggendo `OceanSea.storm` e
/// `OceanSea.wind_angle`.
pub struct ElvenAnimatedMaterials {
    pub sail: Handle<SailMaterial>,
    pub flag: Handle<SailMaterial>,
}

impl ElvenAnimatedMaterials {
    /// Crea i due `SailMaterial` (vele e bandiere).
    ///
    /// Le **vele** usano `assets/textures/vele.png` se presente (percorso Bevy
    /// `textures/vele.png`), altrimenti una texture procedurale di riserva
    /// (blu notte con bagliori). Le **bandiere** restano procedurali.
    ///
    /// `asset_server` è necessario per caricare il PNG tramite Bevy AssetServer.
    pub fn load(
        sail_mats: &mut Assets<SailMaterial>,
        images: &mut Assets<Image>,
        asset_server: &AssetServer,
    ) -> Self {
        // Percorso Bevy (relativo a `assets/`). Controllo anche il percorso
        // su disco per loggare un messaggio chiaro se il file manca.
        const VELE_ASSET_PATH: &str = "textures/vele.png";
        const VELE_DISK_PATH: &str = "assets/textures/vele.png";

        let sail_tex = if std::path::Path::new(VELE_DISK_PATH).exists() {
            info!("Veliero elfico: vele caricate da {VELE_DISK_PATH}");
            asset_server.load(VELE_ASSET_PATH)
        } else {
            warn!(
                "Veliero elfico: {VELE_DISK_PATH} non trovato, uso la texture \
procedurale di riserva per le vele"
            );
            images.add(generate_magic_sail_texture(512))
        };
        let flag_tex = images.add(generate_magic_flag_texture(256));

        let sail = sail_mats.add(SailMaterial {
            base: StandardMaterial {
                base_color_texture: Some(sail_tex),
                                 base_color: Color::WHITE,
                                 perceptual_roughness: 0.85,
                                 metallic: 0.0,
                                 cull_mode: None,
                                 double_sided: true,
                                 alpha_mode: AlphaMode::Blend,
                                 ..default()
            },
            extension: SailExtension {
                params: SailParams {
                    time: 0.0,
                    sail_amp: 1.5,
                    flag_amp: 0.0,
                    phase: 0.0,
                    kind: 0,
                    wind_strength: 0.4,
                    wind_angle: 0.0,
                    _pad0: 0.0,
                },
            },
        });
        let flag = sail_mats.add(SailMaterial {
            base: StandardMaterial {
                base_color_texture: Some(flag_tex),
                                 base_color: Color::WHITE,
                                 perceptual_roughness: 0.85,
                                 metallic: 0.0,
                                 cull_mode: None,
                                 double_sided: true,
                                 alpha_mode: AlphaMode::Blend,
                                 ..default()
            },
            extension: SailExtension {
                params: SailParams {
                    time: 0.0,
                    sail_amp: 0.0,
                    flag_amp: 0.8,
                    phase: 0.9,
                    kind: 1,
                    wind_strength: 0.4,
                    wind_angle: 0.0,
                    _pad0: 0.0,
                },
            },
        });
        Self { sail, flag }
    }
}

/// Trama "magica" per le vele (fallback se `textures/vele.png` manca): blu
/// notte con bagliori diagonali e chiazze organiche (rumore di Perlin).
///
/// Non viene più usata quando `textures/vele.png` esiste: la funzione resta
/// disponibile come fallback e come sorgente per eventuali esperimenti.
#[allow(dead_code)]
fn generate_magic_sail_texture(size: u32) -> Image {
    use crate::data::perlin::perlin_noise;
    let s = size as usize;
    let mut data = vec![0u8; s * s * 4];
    for y in 0..s {
        for x in 0..s {
            let fx = x as f32 / s as f32;
            let fy = y as f32 / s as f32;

            let n1 = perlin_noise(fx * 4.0, fy * 4.0);
            let n2 = perlin_noise(fx * 12.0, fy * 12.0) * 0.5;
            let noise = (n1 + n2) / 1.5;

            let diag = (fx + fy * 0.3) * 5.0;
            let streak = (diag.fract() * std::f32::consts::TAU).sin().max(0.0).powf(8.0);

            let base_blue = 0.12 + noise * 0.20;
            let highlight = streak * 0.65 + noise * 0.15;

            let r = (highlight * 0.7 + 0.05).min(1.0);
            let g = (highlight * 0.85 + base_blue * 0.3).min(1.0);
            let b = (base_blue + highlight * 0.4).min(1.0);

            let idx = (y * s + x) * 4;
            data[idx] = (r * 255.0) as u8;
            data[idx + 1] = (g * 255.0) as u8;
            data[idx + 2] = (b * 255.0) as u8;
            data[idx + 3] = 255;
        }
    }
    let mut image = Image::new(
        Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = bevy::image::ImageSampler::linear();
    image
}

/// Trama delle bandiere: oro con variazione organica (Perlin), asta scura e
/// punta chiara.
fn generate_magic_flag_texture(size: u32) -> Image {
    use crate::data::perlin::perlin_noise;
    let s = size as usize;
    let mut data = vec![0u8; s * s * 4];
    for y in 0..s {
        for x in 0..s {
            let fx = x as f32 / s as f32;
            let fy = y as f32 / s as f32;

            let n = perlin_noise(fx * 6.0, fy * 6.0);
            let gold = (fx * 1.2 - 0.1).clamp(0.0, 1.0);
            let r = (0.9 * gold + 0.05 + n * 0.15).min(1.0);
            let g = (0.7 * gold + 0.05 + n * 0.15).min(1.0);
            let b = (0.15 * gold + 0.05 + n * 0.10).min(1.0);

            let idx = (y * s + x) * 4;
            data[idx] = (r * 255.0) as u8;
            data[idx + 1] = (g * 255.0) as u8;
            data[idx + 2] = (b * 255.0) as u8;
            data[idx + 3] = 255;
        }
    }
    let mut image = Image::new(
        Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.sampler = bevy::image::ImageSampler::linear();
    image
}

// =============================================================================
// 3. VELE TRIANGOLARI (mesh quasi piatta, deformata dallo shader)
// =============================================================================

/// Marker. Il materiale condiviso (`ElvenAnimatedMaterials.sail`) fa il
/// resto: la posizione dei vertici è cotta nella mesh (quasi piatta,
/// sagitta 1/10), tutta la deformazione è nel vertex shader.
#[derive(Component)]
pub struct ElvenSail;

/// Le tre vele del Three.js: tack/head/clew + sagitta iniziale.
pub fn build_sails() -> Vec<AnimatedPart> {
    vec![
        AnimatedPart::TriangularSail {
            tack: Vec3::new(44.0, 5.0, 0.0),
            head: Vec3::new(19.0, 29.0, 0.0),
            clew: Vec3::new(20.0, 5.0, 7.0),
            initial_sag: 2.6,
        },
        AnimatedPart::TriangularSail {
            tack: Vec3::new(27.0, 5.0, 2.0),
            head: Vec3::new(0.0, 38.0, 0.0),
            clew: Vec3::new(0.0, 4.5, 8.0),
            initial_sag: 4.4,
        },
        AnimatedPart::TriangularSail {
            tack: Vec3::new(10.0, 5.0, 5.0),
            head: Vec3::new(-18.0, 30.0, 0.0),
            clew: Vec3::new(-18.0, 5.0, 7.0),
            initial_sag: 3.4,
        },
    ]
}

const SAIL_RAD_SEGS: usize = 6;
const SAIL_ARC_SEGS: usize = 10;
const SAIL_SUB: usize = 3;

/// Costruisce la mesh statica della vela con una sagitta base **ridotta a
/// 1/10** di quella originale. La vela parte quasi piatta: tutta la
/// deformazione principale avviene nel vertex shader in base a
/// `wind_strength` e `wind_angle`.
///
/// Le normali sono **ricalcolate geometricamente** (perpendicolari al piano
/// della vela), invece di essere tutte `[0,1,0]`: senza questo, il bulge
/// calcolato dallo shader lungo `n` spinge i vertici verticalmente invece
/// che lateralmente, e l'effetto "gonfio" non si vedrebbe.
fn build_sail_mesh(tack: Vec3, head: Vec3, clew: Vec3, sag: f32) -> (Mesh, Vec<[f32; 3]>) {
    let total_verts = SAIL_SUB * (SAIL_RAD_SEGS + 1) * (SAIL_ARC_SEGS + 1);
    let mut positions = vec![[0.0_f32; 3]; total_verts];
    let mut normals = vec![[0.0_f32, 0.0, 1.0]; total_verts];
    let mut uvs = vec![[0.0_f32; 2]; total_verts];
    let mut indices: Vec<u32> = Vec::with_capacity(SAIL_SUB * SAIL_RAD_SEGS * SAIL_ARC_SEGS * 6);

    // UV statiche.
    for k in 0..SAIL_SUB {
        for i in 0..=SAIL_RAD_SEGS {
            let v = i as f32 / SAIL_RAD_SEGS as f32;
            for j in 0..=SAIL_ARC_SEGS {
                let u = (k as f32 + j as f32 / SAIL_ARC_SEGS as f32) / SAIL_SUB as f32;
                let idx = (k * (SAIL_RAD_SEGS + 1) + i) * (SAIL_ARC_SEGS + 1) + j;
                uvs[idx] = [u, v];
            }
        }
    }

    // Connettività della griglia.
    for k in 0..SAIL_SUB {
        let base = k * (SAIL_RAD_SEGS + 1) * (SAIL_ARC_SEGS + 1);
        for i in 0..SAIL_RAD_SEGS {
            for j in 0..SAIL_ARC_SEGS {
                let a = base + i * (SAIL_ARC_SEGS + 1) + j;
                let b = base + (i + 1) * (SAIL_ARC_SEGS + 1) + j;
                let c = base + (i + 1) * (SAIL_ARC_SEGS + 1) + (j + 1);
                let d = base + i * (SAIL_ARC_SEGS + 1) + (j + 1);
                indices.extend_from_slice(&[
                    a as u32, b as u32, d as u32,
                    b as u32, c as u32, d as u32,
                ]);
            }
        }
    }

    // Sagitta base ridotta a 1/10: la vela parte quasi piatta.
    update_sail_positions(&mut positions, tack, head, clew, sag * 0.1);

    // Ricalcola le normali geometricamente dalla griglia.
    for k in 0..SAIL_SUB {
        let base = k * (SAIL_RAD_SEGS + 1) * (SAIL_ARC_SEGS + 1);
        for i in 0..=SAIL_RAD_SEGS {
            for j in 0..=SAIL_ARC_SEGS {
                let idx = base + i * (SAIL_ARC_SEGS + 1) + j;
                let i_next = if i < SAIL_RAD_SEGS { i + 1 } else { i.saturating_sub(1) };
                let i_prev = if i > 0 { i - 1 } else { (i + 1).min(SAIL_RAD_SEGS) };
                let j_next = if j < SAIL_ARC_SEGS { j + 1 } else { j.saturating_sub(1) };
                let j_prev = if j > 0 { j - 1 } else { (j + 1).min(SAIL_ARC_SEGS) };

                let p_i_next = Vec3::from(positions[base + i_next * (SAIL_ARC_SEGS + 1) + j]);
                let p_i_prev = Vec3::from(positions[base + i_prev * (SAIL_ARC_SEGS + 1) + j]);
                let p_j_next = Vec3::from(positions[base + i * (SAIL_ARC_SEGS + 1) + j_next]);
                let p_j_prev = Vec3::from(positions[base + i * (SAIL_ARC_SEGS + 1) + j_prev]);

                let tangent_i = p_i_next - p_i_prev;
                let tangent_j = p_j_next - p_j_prev;
                let n = tangent_i.cross(tangent_j).normalize_or_zero();
                if n != Vec3::ZERO {
                    normals[idx] = n.to_array();
                }
            }
        }
    }

    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions.clone());
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    (mesh, positions)
}

/// Costruisce le posizioni dei vertici della vela a sagitta `sag`.
///
/// Chiamata **una sola volta** durante lo spawn, con `sag = initial_sag * 0.1`:
/// la geometria è quasi piatta, il vertex shader aggiunge il gonfiore in
/// base al vento. Porting 1:1 di `updateSail()` del Three.js.
fn update_sail_positions(positions: &mut [[f32; 3]], tack: Vec3, head: Vec3, clew: Vec3, sag: f32) {
    let sag_mag = sag.abs().max(0.01);
    let sag_sign = if sag >= 0.0 { -1.0 } else { 1.0 };

    let chord_main = clew - head;
    let l_main = chord_main.length();
    let u_chord_main = chord_main.normalize_or_zero();
    let mid_main = (head + clew) * 0.5;
    let v_tack_to_clew = clew - tack;
    let norm_plane = chord_main.cross(v_tack_to_clew).normalize_or_zero();
    let wind_normal = norm_plane * sag_sign;

    let h_main = sag_mag * 0.6;
    let r_main = (l_main * l_main + 4.0 * h_main * h_main) / (8.0 * h_main);
    let theta0_main = (l_main / (2.0 * r_main)).min(1.0).asin();

    let arc_point = |t: f32| -> Vec3 {
        let theta = -theta0_main + 2.0 * theta0_main * t;
        let along_chord = r_main * theta.sin();
        let along_wind = r_main * theta.cos() - r_main * theta0_main.cos();
        mid_main + u_chord_main * along_chord + wind_normal * along_wind
    };

    let nodes = [arc_point(0.0), arc_point(1.0 / 3.0), arc_point(2.0 / 3.0), arc_point(1.0)];

    for k in 0..SAIL_SUB {
        let p_top = nodes[k];
        let p_base = nodes[k + 1];

        let sub_chord = p_base - p_top;
        let l_sub = sub_chord.length();
        let u_sub_chord = sub_chord.normalize_or_zero();
        let sub_mid = (p_top + p_base) * 0.5;
        let in_radial = (tack - sub_mid).normalize_or_zero();

        let h_incavo = l_sub * 0.22;
        let r_sub = (l_sub * l_sub + 4.0 * h_incavo * h_incavo) / (8.0 * h_incavo);
        let theta0_sub = (l_sub / (2.0 * r_sub)).min(1.0).asin();

        let mut sub_arc = [[0.0_f32; 3]; SAIL_ARC_SEGS + 1];
        for j in 0..=SAIL_ARC_SEGS {
            let tj = j as f32 / SAIL_ARC_SEGS as f32;
            let theta = -theta0_sub + 2.0 * theta0_sub * tj;
            let along_chord = r_sub * theta.sin();
            let inward_offset = r_sub * theta.cos() - r_sub * theta0_sub.cos();
            let wind_bulge_sub = inward_offset * (sag_mag / 3.2);
            let pt = sub_mid
            + u_sub_chord * along_chord
            + in_radial * inward_offset
            + wind_normal * wind_bulge_sub;
            sub_arc[j] = pt.to_array();
        }

        let base = k * (SAIL_RAD_SEGS + 1) * (SAIL_ARC_SEGS + 1);
        for i in 0..=SAIL_RAD_SEGS {
            let s = i as f32 / SAIL_RAD_SEGS as f32;
            let belly_span = (s * std::f32::consts::PI).sin() * sag_mag * 0.1;

            for j in 0..=SAIL_ARC_SEGS {
                let tj = j as f32 / SAIL_ARC_SEGS as f32;
                let sub_belly = (tj * std::f32::consts::PI).sin() * 0.35 + 0.65;
                let yard_lock = if k == 0 {
                    ((j as f32 / SAIL_ARC_SEGS as f32) * std::f32::consts::FRAC_PI_2).sin()
                } else {
                    1.0
                };
                let total_belly = belly_span * sub_belly * yard_lock;

                let px = tack.x * (1.0 - s) + sub_arc[j][0] * s + wind_normal.x * total_belly;
                let py = tack.y * (1.0 - s) + sub_arc[j][1] * s + wind_normal.y * total_belly;
                let pz = tack.z * (1.0 - s) + sub_arc[j][2] * s + wind_normal.z * total_belly;

                let idx = base + i * (SAIL_ARC_SEGS + 1) + j;
                positions[idx] = [px, py, pz];
            }
        }
    }
}

// =============================================================================
// 4. BANDIERE A CODA DI RONDINE (mesh statica)
// =============================================================================

const FLAG_COLS: usize = 8;
const FLAG_ROWS: usize = 4;

/// Marker. Il materiale condiviso (`ElvenAnimatedMaterials.flag`) fa il
/// resto: la posizione dei vertici è cotta nella mesh piatta, l'ondeggio è
/// nel vertex shader (direzione del vento inclusa).
#[derive(Component)]
pub struct ElvenFlag;

/// Le tre bandiere sui tre alberi (in punta ai pennoni).
pub fn build_flags() -> Vec<AnimatedPart> {
    vec![
        AnimatedPart::SwallowtailFlag { pos: Vec3::new(19.0, 33.0, 0.0), width: 16.0, height: 1.2 },
        AnimatedPart::SwallowtailFlag { pos: Vec3::new( 0.0, 46.1, 0.0), width: 24.0, height: 1.6 },
        AnimatedPart::SwallowtailFlag { pos: Vec3::new(-18.0, 37.3, 0.0), width: 16.0, height: 1.2 },
    ]
}

fn build_flag_mesh(width: f32, height: f32) -> Mesh {
    let cols = FLAG_COLS;
    let rows = FLAG_ROWS;
    let verts = (cols + 1) * (rows + 1);
    let mut positions = Vec::with_capacity(verts);
    let mut normals = Vec::with_capacity(verts);
    let mut uvs = Vec::with_capacity(verts);
    let mut indices = Vec::with_capacity(cols * rows * 6);

    // Forma a coda di rondine.
    for j in 0..=rows {
        let fy = j as f32 / rows as f32;
        for i in 0..=cols {
            let fx = i as f32 / cols as f32;
            let notch = if fx > 0.75 {
                let t = (fx - 0.75) / 0.25;
                t * 0.5
            } else {
                0.0
            };
            let y_center_offset = (fy - 0.5) * (1.0 - notch);
            let y = y_center_offset * height;
            let x = fx * width;
            positions.push([x, y, 0.0]);
            normals.push([0.0, 0.0, 1.0]);
            uvs.push([fx, 1.0 - fy]);
        }
    }
    for j in 0..rows {
        for i in 0..cols {
            let a = j * (cols + 1) + i;
            let b = a + 1;
            let c = (j + 1) * (cols + 1) + i + 1;
            let d = (j + 1) * (cols + 1) + i;
            indices.extend_from_slice(&[
                a as u32, b as u32, d as u32,
                b as u32, c as u32, d as u32,
            ]);
        }
    }
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

// =============================================================================
// 5. SPAWN
// =============================================================================

/// Crea le entità figlie delle vele e delle bandiere.
///
/// La mesh è costruita una sola volta (con la sagitta base 1/10 per le vele,
/// piatta per le bandiere) e non viene più toccata. Il materiale
/// `SailMaterial` (condiviso) riceve `params` aggiornati ogni frame da
/// `ElvenShipPlugin::update_sail_time`. Il vertex shader
/// `sail_animator.wgsl` applica la deformazione in base al vento reale.
pub fn spawn_elven_animated_parts(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mats: &ElvenAnimatedMaterials,
    parent: Entity,
    animated: &[AnimatedPart],
) {
    for part in animated {
        match part {
            AnimatedPart::TriangularSail { tack, head, clew, initial_sag } => {
                let (mesh, _) = build_sail_mesh(*tack, *head, *clew, *initial_sag);
                commands
                .spawn((
                    Mesh3d(meshes.add(mesh)),
                        MeshMaterial3d(mats.sail.clone()),
                        Transform::IDENTITY,
                        ElvenSail,
                ))
                .set_parent(parent);
            }
            AnimatedPart::SwallowtailFlag { pos, width, height } => {
                let mesh = build_flag_mesh(*width, *height);
                commands
                .spawn((
                    Mesh3d(meshes.add(mesh)),
                        MeshMaterial3d(mats.flag.clone()),
                        Transform::from_translation(*pos),
                        ElvenFlag,
                ))
                .set_parent(parent);
            }
            AnimatedPart::RadialHelm { .. } => {
                // La ruota è statica, già costruita in `build_helm_wheel`.
            }
        }
    }
}
