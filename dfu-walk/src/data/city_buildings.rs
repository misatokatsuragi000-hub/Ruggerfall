//! Assembla tutti gli edifici procedurali e mura allineando l'asse Nord-Sud alle strade.

use std::collections::HashMap;

use crate::arch3d::{Arch3dModel, BuildingTextures};
use crate::blocks::RmbBlock;
use crate::bsa::BsaArchive;
use crate::building_gen::{
    choose_front_side, generate_building, seed_for, CityBuilder, Footprint, BLOCK_SIZE_M, DF_UNIT,
};
use crate::maps::DFLocation;
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

    for by in 0..bh {
        for bx in 0..bw {
            let Some(name) = location.block_names.get(by * location.width + bx) else { continue };
            let Some(raw) = blocks_bsa.get_by_name(name) else { continue };
            let Ok(rmb) = RmbBlock::parse(raw) else { continue };

            // In Daggerfall la riga by=0 è il Nord della città (blocco WALLAA02/03),
            // la riga bh-1 è il Sud (WALLAA00/01). L'inversione r = bh - 1 - by
            // allinea edifici e strade alle rispettive quote Z del terreno.
            let gx = start_bx + bx;
            let gz = start_by + by;
            let bx0 = gx as f32 * BLOCK_SIZE_M;
            let bz0 = gz as f32 * BLOCK_SIZE_M;
            let bounds = [bx0, -(bz0 + BLOCK_SIZE_M), bx0 + BLOCK_SIZE_M, -bz0];
            let is_wall_block = name.to_ascii_uppercase().starts_with("WALL");

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

                // 1. Mura, archi di porta e torri
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

                // 2. Case, botteghe, gilde, taverne
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
