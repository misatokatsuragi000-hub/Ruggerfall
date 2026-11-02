//! Assembla gli edifici e gli arredi urbani di Daggerfall.

use std::collections::HashMap;

use super::arch3d::{Arch3dModel, BuildingTextures};
use super::blocks::RmbBlock;
use super::bsa::BsaArchive;
use super::building_gen::{
    choose_front_side, generate_building, seed_for, BuildingMesh, CityBuilder, Footprint,
    BLOCK_SIZE_M, DF_UNIT,
};
use super::custom_buildings::{generate_guild_hall, generate_pagan_temple, GuildStyle};
use super::maps::DFLocation;
use super::palace_gen::generate_royal_palace;
use super::wall_gen::{classify_wall, generate_wall, wall_footprint, WallKind};

// =============================================================================
// CONFIGURAZIONE
// =============================================================================

const PROCEDURAL_LANDMARKS: bool = true;
const PROCEDURAL_HOUSES: bool = false;
const PROCEDURAL_WALLS: bool = true;
const ALWAYS_ARCH3D: [&str; 4] = ["GRVE", "SHRI", "RUIN", "FARM"];

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

    let is_daggerfall_city = location.name.to_ascii_uppercase() == "DAGGERFALL";

    let mut model_cache: HashMap<u32, Option<(Arch3dModel, Option<Footprint>, BuildingTextures)>> =
    HashMap::new();
    let mut palace_spawned = false;

    let palace_target_bx = 5.min(bw.saturating_sub(2));
    let palace_target_by = 6.min(bh.saturating_sub(2));

    let road_x = (start_bx + palace_target_bx) as f32 * BLOCK_SIZE_M + BLOCK_SIZE_M * 0.5;
    let road_z_south = -((start_by + 1) as f32 * BLOCK_SIZE_M);
    let road_z_north =
    -((start_by + palace_target_by) as f32 * BLOCK_SIZE_M + BLOCK_SIZE_M * 0.5) + 32.0;

    let avenue_active = PROCEDURAL_LANDMARKS && is_daggerfall_city;
    let is_on_avenue_strict = |x: f32, z: f32| -> bool {
        if !avenue_active {
            return false;
        }
        if z > (road_z_south + 4.0) || z < (road_z_north - 40.0) {
            return false;
        }
        let dist_x = (x - road_x).abs();
        dist_x < 12.0 || (z <= road_z_north && dist_x < 22.0)
    };

    let model_debug = std::env::var_os("DF_MODEL_DEBUG").is_some();
    let block_debug = std::env::var("DF_MODEL_DEBUG_BLOCK").ok().map(|v| v.to_ascii_uppercase());
    let dump_ids: Vec<u32> = std::env::var("DF_MODEL_DUMP")
    .map(|v| v.split(',').filter_map(|t| t.trim().parse().ok()).collect())
    .unwrap_or_default();
    let mut dumped: std::collections::HashSet<u32> = std::collections::HashSet::new();

    for by in 0..bh {
        for bx in 0..bw {
            let Some(name) = location.block_names.get(by * location.width + bx) else { continue };
            let upper_name = name.to_ascii_uppercase();

            let gx = start_bx + bx;
            let gz = start_by + by;
            let bx0 = gx as f32 * BLOCK_SIZE_M;
            let bz0 = gz as f32 * BLOCK_SIZE_M;
            let bounds = [bx0, -(bz0 + BLOCK_SIZE_M), bx0 + BLOCK_SIZE_M, -bz0];

            let is_special_block = ALWAYS_ARCH3D.iter().any(|p| upper_name.starts_with(p));

            // --- Palazzo Reale procedurale (solo Daggerfall) ---
            let palace_by_position = PROCEDURAL_LANDMARKS
            && is_daggerfall_city
            && !palace_spawned
            && ((bx == palace_target_bx && by == palace_target_by)
            || upper_name.starts_with("PALA")
            || upper_name == "CASTAA01.RMB");

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

            // Palazzo Reale per building_type 16
            if PROCEDURAL_LANDMARKS
                && is_daggerfall_city
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

                let is_temple_block = PROCEDURAL_LANDMARKS
                && !is_wall_block
                && (upper_name.starts_with("TEMP")
                || upper_name.starts_with("PTEM")
                || rmb.buildings.iter().any(|b| b.building_type == 14));

                let guild_style = if PROCEDURAL_LANDMARKS && !is_special_block && !is_wall_block && !is_temple_block {
                    if upper_name.starts_with("MAGE") {
                        Some(GuildStyle::Mage)
                    } else if upper_name.starts_with("FIGH") {
                        Some(GuildStyle::Fighter)
                    } else if upper_name.starts_with("THIE") {
                        Some(GuildStyle::Thief)
                    } else if upper_name.starts_with("DARK") || upper_name.starts_with("GUIL") {
                        Some(GuildStyle::Hall)
                    } else if rmb.buildings.iter().any(|b| b.building_type == 11) {
                        Some(GuildStyle::Thief)
                    } else {
                        None
                    }
                } else {
                    None
                };

                let is_guild_block = !is_wall_block
                && (bx != palace_target_bx || by != palace_target_by)
                && guild_style.is_some();

                let is_south_gate_block =
                is_wall_block && bx == palace_target_bx && by < palace_target_by;
                let mut south_gate_spawned = false;

                let placed_models = rmb.placed_models();
                for pm in placed_models.iter() {
                    stats.objects += 1;

                    let (model, fp, btex) = match model_cache.entry(pm.model_id).or_insert_with(|| {
                        let Some(raw) = arch3d_bsa.get_by_id(pm.model_id) else {
                            if model_debug {
                                eprintln!("ARCH3D modello {}: assente da ARCH3D.BSA (blocco {name})", pm.model_id);
                            }
                            return None;
                        };
                        let m = match Arch3dModel::parse(pm.model_id, raw) {
                            Ok(m) => m,
                                                                                                Err(e) => {
                                                                                                    if model_debug {
                                                                                                        eprintln!(
                                                                                                            "ARCH3D modello {}: parse fallito ({e}), {} byte (blocco {name})",
                                                                                                                  pm.model_id,
                                                                                                                  raw.len()
                                                                                                        );
                                                                                                    }
                                                                                                    return None;
                                                                                                }
                        };
                        let fp = Footprint::from_model(&m);
                        let btex = m.building_textures();
                        Some((m, fp, btex))
                    }) {
                        Some(triple) => (triple.0.clone(), triple.1, triple.2),
                        None => {
                            stats.missing_models += 1;
                            continue;
                        }
                    };

                    if dump_ids.contains(&pm.model_id) && dumped.insert(pm.model_id) {
                        let path = format!("df_model_{}.txt", pm.model_id);
                        match std::fs::write(&path, model.dump()) {
                            Ok(()) => eprintln!("DUMP modello {} scritto in {path} (blocco {name})", pm.model_id),
                            Err(e) => eprintln!("DUMP modello {}: impossibile scrivere {path}: {e}", pm.model_id),
                        }
                    }

                    if let Some(filter) = block_debug.as_deref() {
                        if upper_name.contains(filter) {
                            let mut by_len = std::collections::BTreeMap::<usize, usize>::new();
                            let mut textures: Vec<(u16, u8)> = Vec::new();
                            for f in &model.faces {
                                *by_len.entry(f.vertex_indices.len()).or_default() += 1;
                                let t = (f.texture_archive, f.texture_record);
                                if !textures.contains(&t) && textures.len() < 8 {
                                    textures.push(t);
                                }
                            }
                            eprintln!(
                                "BLOCCO {name}: modello {} a ({:.1}, {:.1}) rot {:.0} | {} vertici, {} facce (punti per faccia: {by_len:?}) | texture {textures:?} | edificio: {}",
                                      pm.model_id,
                                      pm.x,
                                      pm.n,
                                      pm.rotation_deg,
                                      model.vertices.len(),
                                      model.faces.len(),
                                      pm.is_building,
                            );
                        }
                    }

                    let px = bx0 + pm.x * DF_UNIT;
                    let pz = -(bz0 + pm.n * DF_UNIT);
                    let rot = pm.rotation_deg;
                    let (s, c) = rot.to_radians().sin_cos();
                    let (wx, wz) = fp
                    .as_ref()
                    .map_or((px, pz), |f| {
                        (px + f.cx * c + f.cz * s, pz - f.cx * s + f.cz * c)
                    });

                    // --- FIX: RIMUOVI EDIFICI SULLA STRADA REALE ---
                    // Se è un edificio (non un muro o un flat) e si trova sull'avenue, saltalo.
                    if pm.is_building && is_on_avenue_strict(wx, wz) {
                        stats.skipped += 1;
                        continue;
                    }
                    // ----------------------------------------------

                    // Mura procedurali
                    if is_wall_block && PROCEDURAL_WALLS && !is_special_block {
                        if let Some(mut wfp) = fp.as_ref().map(wall_footprint) {
                            let is_wall_model = pm.model_id == 446 || classify_wall(&wfp).is_some();

                            // --- LOGICA PER I BASTIONI ANGOLARI E MURA ---
                            let is_corner = (bx == 0 || bx == bw.saturating_sub(1))
                            && (by == 0 || by == bh.saturating_sub(1));

                            if is_corner {
                                // Rendi il bastione più largo e profondo
                                wfp.width *= 1.8;
                                wfp.depth *= 1.8;
                                // Forza un'altezza imponente per i bastioni
                                wfp.height = wfp.height.max(35.0);
                            } else if is_daggerfall_city {
                                // Alza anche i muri normali per Daggerfall
                                wfp.height = wfp.height.max(18.0);
                            }
                            // ----------------------------------------------

                            if is_south_gate_block && is_wall_model {
                                if !south_gate_spawned {
                                    let mut gate_fp = wfp;
                                    gate_fp.width = BLOCK_SIZE_M;
                                    gate_fp.depth = gate_fp.depth.max(3.0);
                                    gate_fp.height = gate_fp.height.max(18.0); // Porta più alta
                                    let seed = seed_for(
                                        pm.model_id,
                                        gx as i32 * 4096 + 2048,
                                        gz as i32 * 4096 + pm.n as i32,
                                    );
                                    let wall_tex = btex.wall.or(Some((317, 0)));
                                    let mesh = generate_wall(&gate_fp, WallKind::Gate, seed, wall_tex);
                                    let gy = ground_height(road_x, pz);
                                    place_procedural(&mut builder, &mesh, [0.0, 0.0, 0.0], rot, road_x, pz, gy);
                                    stats.built += 1;
                                    south_gate_spawned = true;
                                }
                                continue;
                            }

                            let maybe_wkind = if pm.model_id == 446 {
                                Some(WallKind::Gate)
                            } else if is_corner {
                                Some(WallKind::Tower) // Forza la torre per gli angoli
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
                                let gy = ground_height(px, pz);
                                place_procedural(&mut builder, &mesh, [0.0, 0.0, 0.0], rot, px, pz, gy);
                                stats.built += 1;
                                continue;
                            }
                        }
                    }

                    let is_temple_sub = pm.subrecord.map_or(false, |i| {
                        rmb.buildings
                        .get(i as usize)
                        .map_or(false, |b| b.building_type == 14)
                    });
                    let is_temple_building = is_temple_block && is_temple_sub;

                    let is_guild_sub = pm.subrecord.map_or(false, |i| {
                        rmb.buildings
                        .get(i as usize)
                        .map_or(false, |b| b.building_type == 11)
                    });
                    let is_guild_building = !is_temple_building && is_guild_block && is_guild_sub;

                    let is_palace_sub = pm.subrecord.map_or(false, |i| {
                        rmb.buildings
                        .get(i as usize)
                        .map_or(false, |b| b.building_type == 16)
                    });

                    let is_procedural_house_candidate = !is_temple_building
                    && !is_guild_building
                    && !is_palace_sub
                    && !is_special_block
                    && pm.is_building
                    && !upper_name.starts_with("MARK")
                    && !upper_name.starts_with("WALL")
                    && (pm.model_id < 41000 || pm.model_id >= 42000)
                    && pm.model_id != 74
                    && fp.as_ref().map_or(false, |f| f.classify().is_some());

                    // Case procedurali
                    if PROCEDURAL_HOUSES && is_procedural_house_candidate {
                        let Some(fp_ref) = fp.as_ref() else { continue };
                        // (Il check avenue è già stato fatto sopra)

                        let kind = fp_ref.classify().unwrap();
                        let front = choose_front_side(
                            [wx, wz],
                            [fp_ref.width * 0.5, fp_ref.depth * 0.5],
                            rot,
                            bounds,
                        );
                        let seed = seed_for(
                            pm.model_id,
                            pm.x as i32 + gx as i32 * 4096,
                            pm.n as i32 + gz as i32 * 4096,
                        );

                        let mut house_tex = btex;
                        if house_tex.wall.is_none() { house_tex.wall = Some((317, 0)); }
                        if house_tex.roof.is_none() { house_tex.roof = Some((69, 0)); }

                        let mesh = generate_building(&fp_ref, kind, front, seed, house_tex);
                        let gy = ground_height(px, pz);
                        place_procedural(&mut builder, &mesh, [fp_ref.cx, 0.0, fp_ref.cz], rot, px, pz, gy);
                        stats.built += 1;
                        continue;
                    }

                    if is_temple_building {
                        let Some(fp_ref) = fp.as_ref() else {
                            let gy = ground_height(px, pz);
                            let min_model_y = model.vertices.iter().map(|v| v[1]).fold(f32::MAX, f32::min);
                            builder.add_arch3d_instance(&model, rot, [px, gy - min_model_y, pz]);
                            stats.built += 1;
                            continue;
                        };
                        let mut temple_tex = btex;
                        temple_tex.wall = btex.wall.or(Some((317, 0)));
                        temple_tex.roof = btex.roof.or(Some((69, 0)));

                        let mesh = generate_pagan_temple(&fp_ref, temple_tex);
                        let gy = ground_height(px, pz);
                        place_procedural(&mut builder, &mesh, [fp_ref.cx, 0.0, fp_ref.cz], rot, px, pz, gy);
                        stats.built += 1;
                        continue;
                    }

                    if is_guild_building {
                        let Some(fp_ref) = fp.as_ref() else {
                            let gy = ground_height(px, pz);
                            let min_model_y = model.vertices.iter().map(|v| v[1]).fold(f32::MAX, f32::min);
                            builder.add_arch3d_instance(&model, rot, [px, gy - min_model_y, pz]);
                            stats.built += 1;
                            continue;
                        };
                        let front = choose_front_side(
                            [wx, wz],
                            [fp_ref.width * 0.5, fp_ref.depth * 0.5],
                            rot,
                            bounds,
                        );
                        let seed = seed_for(
                            pm.model_id,
                            pm.x as i32 + gx as i32 * 4096,
                            pm.n as i32 + gz as i32 * 4096,
                        );

                        let mut guild_tex = btex;
                        let style = guild_style.unwrap_or(GuildStyle::Hall);
                        match style {
                            GuildStyle::Mage => {
                                guild_tex.wall = Some((335, 0));
                                guild_tex.roof = Some((339, 0));
                            }
                            GuildStyle::Fighter | GuildStyle::Thief => {
                                guild_tex.wall = btex.wall.or(Some((317, 0)));
                                guild_tex.roof = btex.roof.or(Some((69, 0)));
                            }
                            GuildStyle::Hall => {
                                guild_tex.wall = btex.wall.or(Some((335, 0)));
                                guild_tex.roof = btex.roof.or(Some((339, 0)));
                            }
                        }

                        let mesh = generate_guild_hall(&fp_ref, front, seed, guild_tex, style);
                        let gy = ground_height(px, pz);
                        place_procedural(&mut builder, &mesh, [fp_ref.cx, 0.0, fp_ref.cz], rot, px, pz, gy);
                        stats.built += 1;
                        continue;
                    }

                    let gy = ground_height(px, pz);
                    let min_model_y = model
                    .vertices
                    .iter()
                    .map(|v| v[1])
                    .fold(f32::MAX, f32::min);
                    let final_y = gy - min_model_y;
                    builder.add_arch3d_instance(&model, rot, [px, final_y, pz]);
                    stats.built += 1;
                }
        }
    }

    (builder, stats)
}

fn place_procedural(
    builder: &mut CityBuilder,
    mesh: &BuildingMesh,
    offset: [f32; 3],
    rot: f32,
    px: f32,
    pz: f32,
    ground_y: f32,
) {
    let min_y = mesh
    .parts
    .iter()
    .flat_map(|p| p.positions.iter().map(|v| v[1]))
    .chain(mesh.doors.positions.iter().map(|v| v[1]))
    .chain(mesh.windows.positions.iter().map(|v| v[1]))
    .fold(f32::MAX, f32::min);
    let final_y = if min_y < -0.05 {
        ground_y - min_y - offset[1]
    } else {
        ground_y
    };
    builder.add(mesh, offset, rot, [px, final_y, pz]);
}
