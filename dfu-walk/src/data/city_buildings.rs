//! Assembla gli edifici e gli arredi urbani di Daggerfall.
//! - Palazzo Reale, Mura e Gilde: monumentali procedurali (Regola 1).
//! - Edifici residenziali e commerciali veri e propri: procedurali completi con texture (Regola 2).
//! - Tutto il resto (staccionate, banchi market, pozzi, carretti, porte/portici singoli): mesh originali ARCH3D (Regola 3).

use std::collections::HashMap;

use super::arch3d::{Arch3dModel, BuildingTextures};
use super::blocks::RmbBlock;
use super::bsa::BsaArchive;
use super::building_gen::{
    choose_front_side, generate_building, seed_for, CityBuilder, Footprint, BLOCK_SIZE_M, DF_UNIT,
};
use super::maps::DFLocation;
use super::palace_gen::{generate_guild_hall, generate_royal_palace};
use super::wall_gen::{classify_wall, generate_wall, wall_footprint, WallKind};

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

    // =====================================================================
    // SOLO la capitale Daggerfall riceve il Palazzo Reale procedurale.
    // Tutti gli altri palazzi (BuildingType 16) usano la geometria ARCH3D.
    // =====================================================================
    let is_daggerfall_city = location.name.to_ascii_uppercase() == "DAGGERFALL";

    let mut model_cache: HashMap<u32, Option<(Arch3dModel, Footprint, BuildingTextures)>> = HashMap::new();
    let mut palace_spawned = false;

    let palace_target_bx = 5.min(bw.saturating_sub(2));
    let palace_target_by = 6.min(bh.saturating_sub(2));

    let road_x = (start_bx + palace_target_bx) as f32 * BLOCK_SIZE_M + BLOCK_SIZE_M * 0.5;
    let road_z_south = -((start_by + 1) as f32 * BLOCK_SIZE_M);
    let road_z_north = -((start_by + palace_target_by) as f32 * BLOCK_SIZE_M + BLOCK_SIZE_M * 0.5) + 32.0;

    let is_on_avenue_strict = |x: f32, z: f32| -> bool {
        if z > (road_z_south + 4.0) || z < (road_z_north - 40.0) {
            return false;
        }
        let dist_x = (x - road_x).abs();
        dist_x < 22.0 || (z <= road_z_north && dist_x < 42.0)
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

            // --- REGOLA 1: Palazzo Reale (SOLO Daggerfall) ---
            let palace_by_position = is_daggerfall_city
            && !palace_spawned
            && (
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

            // --- REGOLA 1: Palazzo Reale per BuildingType (SOLO Daggerfall) ---
            if is_daggerfall_city
                && !palace_spawned
                && rmb.buildings.iter().any(|b| b.building_type == 16)
                {
                    let center_x = bx0 + BLOCK_SIZE_M * 0.5;
                    let center_z = -(bz0 + BLOCK_SIZE_M * 0.5);
                    let gy = ground_height(center_x, center_z);
                    let palace_mesh = generate_royal_palace(None, None);
                    builder.add(&palace_mesh, [0.0, 0.0, 0.0], 0.0, [center_x, gy, center_z]);
                    stats.built += 1;
                    palace_spawned = true;
                    continue;
                }

                // Identificazione Gilda (Regola 1)
                let is_guild_block = !is_wall_block
                && (bx != palace_target_bx || by != palace_target_by)
                && (upper_name.starts_with("MAGE")
                || upper_name.starts_with("FIGH")
                || upper_name.starts_with("THIE")
                || upper_name.starts_with("DARK")
                || upper_name.starts_with("GUIL")
                || rmb.buildings.iter().any(|b| b.building_type == 11));

                let guild_pm_index = if is_guild_block {
                    let mut best_idx = None;
                    let mut max_area = 0.0f32;
                    for (idx, pm) in rmb.placed_models().iter().enumerate() {
                        let maybe_fp = match model_cache.entry(pm.model_id).or_insert_with(|| {
                            let raw = arch3d_bsa.get_by_id(pm.model_id)?;
                            let m = Arch3dModel::parse(pm.model_id, raw).ok()?;
                            let fp = Footprint::from_model(&m)?;
                            let btex = m.building_textures();
                            Some((m, fp, btex))
                        }) {
                            Some(triple) => Some(triple.1),
                            None => None,
                        };
                        if let Some(fp) = maybe_fp {
                            let area = fp.width * fp.depth;
                            if area > max_area {
                                max_area = area;
                                best_idx = Some(idx);
                            }
                        }
                    }
                    best_idx
                } else {
                    None
                };

                let is_south_gate_block = is_wall_block && bx == palace_target_bx && by < palace_target_by;
                let mut south_gate_spawned = false;

                for (pm_idx, pm) in rmb.placed_models().iter().enumerate() {
                    stats.objects += 1;

                    let (model, fp, mut btex) = match model_cache.entry(pm.model_id).or_insert_with(|| {
                        let raw = arch3d_bsa.get_by_id(pm.model_id)?;
                        let m = Arch3dModel::parse(pm.model_id, raw).ok()?;
                        let fp = Footprint::from_model(&m)?;
                        let btex = m.building_textures();
                        Some((m, fp, btex))
                    }) {
                        Some(triple) => (triple.0.clone(), triple.1, triple.2),
                        None => {
                            stats.missing_models += 1;
                            continue;
                        }
                    };

                    let px = bx0 + pm.x * DF_UNIT;
                    let pz = -(bz0 + pm.n * DF_UNIT);
                    let rot = pm.rotation_deg;
                    let (s, c) = rot.to_radians().sin_cos();
                    let wx = px + fp.cx * c + fp.cz * s;
                    let wz = pz - fp.cx * s + fp.cz * c;

                    // --- REGOLA 1: Mura cittadine e porte monumentali ---
                    if is_wall_block {
                        let wfp = wall_footprint(&fp);
                        let is_wall_model = pm.model_id == 446 || classify_wall(&wfp).is_some();

                        if is_south_gate_block && is_wall_model {
                            if !south_gate_spawned {
                                let mut gate_fp = wfp;
                                gate_fp.width = BLOCK_SIZE_M;
                                gate_fp.depth = gate_fp.depth.max(3.0);
                                gate_fp.height = gate_fp.height.max(11.0);
                                let seed = seed_for(
                                    pm.model_id,
                                    gx as i32 * 4096 + 2048,
                                    gz as i32 * 4096 + pm.n as i32,
                                );
                                let wall_tex = btex.wall.or(Some((317, 0)));
                                let mesh = generate_wall(&gate_fp, WallKind::Gate, seed, wall_tex);
                                let gy = ground_height(road_x, pz);
                                builder.add(&mesh, [0.0, 0.0, 0.0], rot, [road_x, gy, pz]);
                                stats.built += 1;
                                south_gate_spawned = true;
                            }
                            continue;
                        }

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
                            let lift = (-(pm.y as f32) * DF_UNIT).clamp(0.0, 3.0);
                            let gy = ground_height(px, pz) + lift;
                            builder.add(&mesh, [0.0, 0.0, 0.0], rot, [px, gy, pz]);
                            stats.built += 1;
                            continue;
                        }
                    }

                    let is_guild_building = is_guild_block && Some(pm_idx) == guild_pm_index;

                    // Calcolo esatto quota base per modelli ARCH3D
                    let min_model_y = model.vertices.iter().map(|v| v[1]).fold(f32::MAX, f32::min);

                    // =====================================================================
                    // Un palazzo (BuildingType 16) in una città NON-Daggerfall NON deve
                    // diventare procedurale: deve usare la mesh ARCH3D originale (Regola 3).
                    // =====================================================================
                    let is_palace_sub = !is_daggerfall_city
                    && pm.subrecord.map_or(false, |i| {
                        rmb.buildings.get(i as usize).map_or(false, |b| b.building_type == 16)
                    });

                    // =========================================================================
                    // CONDIZIONE INFALLIBILE:
                    // Un vero edificio procedurale DEVE soddisfare TUTTE queste condizioni:
                    // 1. Non è la Gilda (che ha il suo generatore monumentale a cupola)
                    // 2. Non è un palazzo non-Daggerfall (geometria originale ARCH3D)
                    // 3. È un edificio registrato nel sotto-record (pm.is_building)
                    // 4. Il blocco non è un mercato (MARK) o muro (WALL)
                    // 5. Non è un prop (serie 41xxx) né una porta singola (ID 74)
                    // 6. Ha un'impronta reale da casa piena (lato >= 4.2m, altezza >= 3.2m, area >= 25m²)
                    // =========================================================================
                    let is_procedural_house = !is_guild_building
                    && !is_palace_sub
                    && pm.is_building
                    && !upper_name.starts_with("MARK")
                    && !upper_name.starts_with("WALL")
                    && (pm.model_id < 41000 || pm.model_id >= 42000)
                    && pm.model_id != 74
                    && fp.classify().is_some();

                    // --- REGOLA 2: Casa / Edificio procedurale vero e proprio ---
                    if is_procedural_house {
                        if is_on_avenue_strict(wx, wz) {
                            stats.skipped += 1;
                            continue;
                        }

                        let kind = fp.classify().unwrap();
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

                        if btex.wall.is_none() {
                            btex.wall = Some((317, 0));
                        }
                        if btex.roof.is_none() {
                            btex.roof = Some((69, 0));
                        }

                        let mesh = generate_building(&fp, kind, front, seed, btex);
                        let lift = (-(pm.y as f32) * DF_UNIT).clamp(0.0, 3.0);
                        let gy = ground_height(px, pz) + lift;
                        builder.add(&mesh, [fp.cx, 0.0, fp.cz], rot, [px, gy, pz]);
                        stats.built += 1;
                        continue;
                    }

                    // --- REGOLA 1: Gilda Monumentale a cupola ---
                    if is_guild_building {
                        if is_on_avenue_strict(wx, wz) {
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
                        let mut guild_tex = btex;
                        guild_tex.wall = Some((335, 0));
                        guild_tex.roof = Some((339, 0));

                        let mesh = generate_guild_hall(&fp, front, seed, guild_tex);
                        let lift = (-(pm.y as f32) * DF_UNIT).clamp(0.0, 3.0);
                        let gy = ground_height(px, pz) + lift;
                        builder.add(&mesh, [fp.cx, 0.0, fp.cz], rot, [px, gy, pz]);
                        stats.built += 1;
                        continue;
                    }

                    // =========================================================================
                    // REGOLA 3: TUTTO IL RESTO
                    // Include anche i palazzi delle città non-Daggerfall (is_palace_sub).
                    // =========================================================================
                    let gy = ground_height(px, pz);
                    let final_y = if min_model_y < -0.05 {
                        gy - min_model_y
                    } else {
                        gy
                    };
                    builder.add_arch3d_instance(&model, rot, [px, final_y, pz]);
                    stats.built += 1;
                }
        }
    }

    (builder, stats)
}
