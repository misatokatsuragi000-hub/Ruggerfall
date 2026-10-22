//! Assembla gli edifici con corridoio della Grande Strada Reale allargato a 52 metri.

use std::collections::HashMap;

use crate::arch3d::{Arch3dModel, BuildingTextures};
use crate::blocks::RmbBlock;
use crate::bsa::BsaArchive;
use crate::building_gen::{
    choose_front_side, generate_building, seed_for, CityBuilder, Footprint, BLOCK_SIZE_M, DF_UNIT,
};
use crate::maps::DFLocation;
use crate::palace_gen::generate_royal_palace;
use crate::wall_gen::{classify_wall, generate_wall, wall_footprint, WallKind};

#[derive(Debug, Default, Clone, Copy)]
pub struct CityStats {
    pub objects: usize,
    pub built: usize,
    pub skipped: usize,
    pub missing_models: usize,
}

pub fn build_city_buildings(
    location: &DFLocation,
    blocks_bsa: &BsaArchive,
    arch3d_bsa: &BsaArchive,
    ground_height: &dyn Fn(f32, f32) -> f32,
) -> (CityBuilder, CityStats) {
    let mut builder = CityBuilder::new();
    let mut stats = CityStats::default();

    let bw = location.width.min(8);
    let bh = location.height.min(8);
    let start_bx = (8 - bw) / 2;
    let start_by = (8 - bh) / 2;

    let mut model_cache: HashMap<u32, Option<(Footprint, BuildingTextures)>> = HashMap::new();
    let mut palace_spawned = false;

    let palace_target_bx = 5.min(bw.saturating_sub(2));
    let palace_target_by = 6.min(bh.saturating_sub(2));

    let road_x = (start_bx + palace_target_bx) as f32 * BLOCK_SIZE_M + BLOCK_SIZE_M * 0.5;
    let road_z_south = -((start_by + 1) as f32 * BLOCK_SIZE_M);
    let road_z_north = -((start_by + palace_target_by) as f32 * BLOCK_SIZE_M + BLOCK_SIZE_M * 0.5) + 32.0;

    // Fascia di rispetto allargata: 26 metri per lato (52m totali) e 46m davanti al palazzo
    let is_on_avenue = |x: f32, z: f32| -> bool {
        if z > road_z_south || z < (road_z_north - 40.0) {
            return false;
        }
        let dist_x = (x - road_x).abs();
        dist_x < 26.0 || (z <= road_z_north && dist_x < 46.0)
    };

    for by in 0..bh {
        for bx in 0..bw {
            let Some(name) = location.block_names.get(by * location.width + bx) else { continue };
            let upper_name = name.to_ascii_uppercase();

            let gx = start_bx + bx;
            let gz = start_by + by;
            let bx0 = gx as f32 * BLOCK_SIZE_M;
            let bz0 = gz as f32 * BLOCK_SIZE_M;
            let bounds = [bx0, -(bz0 + BLOCK_SIZE_M), bx0 + BLOCK_SIZE_M, -bz0];

            // --- Palazzo, prima fase: coordinate o nome, senza lookup in BLOCKS.BSA ---
            let palace_by_position = !palace_spawned && (
                (bx == palace_target_bx && by == palace_target_by)
                || upper_name.starts_with("PALA")
                || upper_name == "CASTAA01.RMB"
            );
            if palace_by_position {
                let center_x = bx0 + BLOCK_SIZE_M * 0.5;
                let center_z = -(bz0 + BLOCK_SIZE_M * 0.5);
                let gy = ground_height(center_x, center_z);
                let palace_mesh = generate_royal_palace(None, None);
                builder.add(&palace_mesh, [0.0, 0.0, 0.0], 0.0, [center_x, gy, center_z]);
                stats.built += 1;
                palace_spawned = true;
                continue;
            }

            let Some(raw) = blocks_bsa.get_by_name(name) else { continue };
            let Ok(rmb) = RmbBlock::parse(raw) else { continue };
            let is_wall_block = upper_name.starts_with("WALL");

            // --- Palazzo, seconda fase: rilevato dal building_type ---
            if !palace_spawned && rmb.buildings.iter().any(|b| b.building_type == 16) {
                let center_x = bx0 + BLOCK_SIZE_M * 0.5;
                let center_z = -(bz0 + BLOCK_SIZE_M * 0.5);
                let gy = ground_height(center_x, center_z);
                let palace_mesh = generate_royal_palace(None, None);
                builder.add(&palace_mesh, [0.0, 0.0, 0.0], 0.0, [center_x, gy, center_z]);
                stats.built += 1;
                palace_spawned = true;
                continue;
            }

            for pm in rmb.placed_models() {
                stats.objects += 1;

                let (fp, btex) = match model_cache.entry(pm.model_id).or_insert_with(|| {
                    let raw = arch3d_bsa.get_by_id(pm.model_id)?;
                    let m = Arch3dModel::parse(pm.model_id, raw).ok()?;
                    let fp = Footprint::from_model(&m)?;
                    let btex = m.building_textures();
                    Some((fp, btex))
                }) {
                    Some(pair) => *pair,
                    None => {
                        stats.missing_models += 1;
                        continue;
                    }
                };

                let px = bx0 + pm.x * DF_UNIT;
                let pz = -(bz0 + pm.n * DF_UNIT);
                let rot = pm.rotation_deg;

                if is_wall_block {
                    let wfp = wall_footprint(&fp);
                    let maybe_wkind = if pm.model_id == 446 {
                        Some(WallKind::Gate)
                    } else {
                        classify_wall(&wfp)
                    };

                    if let Some(wkind) = maybe_wkind {
                        let seed = seed_for(
                            pm.model_id,
                            pm.x as i32 + gx as i32 * 4096,
                            pm.n as i32 + gz as i32 * 4096,
                        );
                        let wall_tex = btex.wall.or(Some((317, 0)));
                        let mesh = generate_wall(&wfp, wkind, seed, wall_tex);
                        let lift = if is_wall_block { 0.0 } else { (-(pm.y as f32) * DF_UNIT).clamp(0.0, 3.0) };
                        let gy = ground_height(px, pz) + lift;

                        builder.add(&mesh, [0.0, 0.0, 0.0], rot, [px, gy, pz]);
                        stats.built += 1;
                        continue;
                    }
                }

                let Some(kind) = fp.classify() else {
                    let wfp = wall_footprint(&fp);
                    let maybe_wkind = if pm.model_id == 446 {
                        Some(WallKind::Gate)
                    } else {
                        classify_wall(&wfp)
                    };

                    if let Some(wkind) = maybe_wkind {
                        let seed = seed_for(
                            pm.model_id,
                            pm.x as i32 + gx as i32 * 4096,
                            pm.n as i32 + gz as i32 * 4096,
                        );
                        let wall_tex = btex.wall.or(Some((317, 0)));
                        let mesh = generate_wall(&wfp, wkind, seed, wall_tex);
                        let lift = if is_wall_block { 0.0 } else { (-(pm.y as f32) * DF_UNIT).clamp(0.0, 3.0) };
                        let gy = ground_height(px, pz) + lift;
                        builder.add(&mesh, [0.0, 0.0, 0.0], rot, [px, gy, pz]);
                        stats.built += 1;
                        continue;
                    }

                    stats.skipped += 1;
                    continue;
                };

                let (s, c) = rot.to_radians().sin_cos();
                let wx = px + fp.cx * c + fp.cz * s;
                let wz = pz - fp.cx * s + fp.cz * c;

                if is_on_avenue(wx, wz) {
                    stats.skipped += 1;
                    continue;
                }

                let front = choose_front_side(
                    [wx, wz],
                    [fp.width * 0.5, fp.depth * 0.5],
                    rot,
                    bounds,
                );
                let seed = seed_for(
                    pm.model_id,
                    pm.x as i32 + gx as i32 * 4096,
                    pm.n as i32 + gz as i32 * 4096,
                );

                let mesh = generate_building(&fp, kind, front, seed, btex);

                let lift = if is_wall_block { 0.0 } else { (-(pm.y as f32) * DF_UNIT).clamp(0.0, 3.0) };
                let gy = ground_height(px, pz) + lift;
                builder.add(&mesh, [fp.cx, 0.0, fp.cz], rot, [px, gy, pz]);
                stats.built += 1;
            }
        }
    }

    (builder, stats)
}
