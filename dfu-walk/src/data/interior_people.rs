//! Raccoglie gli NPC (record "person") dagli INTERNI degli edifici di una citta'.
//!
//! In Daggerfall gli RMB contengono due BlockData per sotto-record: l'esterno e
//! l'INTERNO. Le persone sono listate solo dentro il BlockData interno, quindi
//! per sapere chi popola una cella bisogna aprire l'interno dell'edificio.
//! Qui si estraggono le loro posizioni e le si riporta nel mondo esterno, vicino
//! alla porta d'ingresso corrispondente: diventano cosi' entita' fisiche che
//! possono uscire dall'edificio ed essere reclutate (`RecruitableNpc` in main.rs).

use crate::blocks::{BlockData, RmbBlock, Z_FROM_NORTH_EDGE};
use crate::bsa::BsaArchive;
use crate::building_gen::{DF_UNIT, BLOCK_SIZE_M};
use crate::city_buildings::ExteriorDoor;
use crate::maps::DFLocation;
use crate::terrain_mesh::MAP_PIXEL_SIZE;

const BLOCK_UNITS: f32 = 4096.0;

/// Numero massimo di NPC pubblicati per ogni blocco RMB (come le porte: un solo
/// edificio visitabile per cella, gli altri usano template generici).
const NPC_PER_BLOCK: usize = 6;
/// Un NPC non puo' nascere dentro il muro: la posizione viene schiacciata entro
/// questo margine dai bordi del blocco.
const EDGE_MARGIN_M: f32 = 2.0;

/// Un NPC di un interno, con la posizione ESTERNA proposta (assi Bevy: x est,
/// z = -nord, in metri, relativi all'angolo sud-ovest del map pixel come le
/// `ExteriorDoor`). `ground_y` e' la quota del suolo gia' campionata a `(x, z)`.
#[derive(Debug, Clone)]
pub struct ExteriorPerson {
    pub x: f32,
    pub ground_y: f32,
    pub z: f32,
    /// orientamento in gradi (convenzione `placed_models`: antiorario visto dall'alto)
    pub direction_deg: f32,
    pub texture_archive: u16,
    pub texture_record: u8,
    /// nome del blocco RMB da cui proviene (per debug/diagnostica)
    pub block_name: String,
}

/// Centro (x, n) in unita' Daggerfall dei modelli dell'interno, mediato sulle
/// posizioni locali dei suoi oggetti 3D. E' il punto verso cui "guarda" chi entra.
fn interior_center_local(data: &BlockData) -> (f32, f32) {
    let objs = &data.objects_3d;
    if objs.is_empty() {
        return (0.0, 0.0);
    }
    let (sx, sn): (f32, f32) = objs
        .iter()
        .map(|o| {
            let n = if Z_FROM_NORTH_EDGE { o.z as f32 } else { -(o.z as f32) };
            (o.x as f32, n)
        })
        .fold((0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
    (sx / objs.len() as f32, sn / objs.len() as f32)
}

/// Estrae gli NPC dagli interni degli edifici della localita', collocandoli nel
/// mondo esterno del tile. Per ogni blocco RMB della citta':
/// 1. prende i record person del BlockData INTERNO di ogni sotto-record;
/// 2. li trasforma nello spazio del blocco con la stessa matrice usata per i
///    modelli (`RmbBlock::placed_models`);
/// 3. se la posizione ricade fuori dal blocco (le stanze hanno coordinate locali
///    non sempre allineate alla griglia esterna) la sostituisce con un anello
///    attorno alla porta d'ingresso pubblicata da `build_city_buildings`, cosi'
///    l'NPC nasce appena fuori dalla facciata, pronto a gironzolare e reclutabile.
pub fn collect_interior_people(
    location: &DFLocation,
    blocks_buf: &[u8],
    doors: &[ExteriorDoor],
    ground_height: &dyn Fn(f32, f32) -> f32,
) -> Vec<ExteriorPerson> {
    let mut out = Vec::new();
    let Ok(blocks_bsa) = BsaArchive::parse(blocks_buf) else {
        return out;
    };

    let bw = location.width.min(8);
    let bh = location.height.min(8);
    let start_bx = (8 - bw) / 2;
    let start_by = (8 - bh) / 2;

    for by in 0..bh {
        for bx in 0..bw {
            let Some(name) = location.block_names.get(by * location.width + bx) else { continue };
            let upper_name = name.to_ascii_uppercase();
            if upper_name.starts_with("WALL") {
                continue;
            }
            // la porta pubblicata per questo blocco (se esiste)
            let door = doors.iter().find(|d| d.block_name == upper_name);

            let Some(raw) = blocks_bsa.get_by_name(name) else { continue };
            let Ok(rmb) = RmbBlock::parse(raw) else { continue };

            let gx = start_bx + bx;
            let gz = start_by + by;
            let bx0 = gx as f32 * BLOCK_SIZE_M;
            let bz0 = gz as f32 * BLOCK_SIZE_M;

            let mut spawned = 0usize;
            for interior in &rmb.interiors {
                let sub = interior.subrecord;
                // ancora e rotazione del sotto-record nel blocco
                let Some(sr) = rmb.subrecords.get(sub as usize) else { continue };
                let (ccx, ccn) = interior_center_local(&interior.data);
                let anchor_n = if Z_FROM_NORTH_EDGE { BLOCK_UNITS - sr.z as f32 } else { sr.z as f32 };
                let phi = sr.rotation_deg.to_radians();
                let (sn, cs) = phi.sin_cos();

                for p in &interior.data.people {
                    if spawned >= NPC_PER_BLOCK {
                        break;
                    }
                    // Y della person = altezza DF (positivo verso il basso): ignorata,
                    // la quota esterna arriva dal terreno
                    let lx = p.x as f32;
                    let ln = if Z_FROM_NORTH_EDGE { p.z as f32 } else { -(p.z as f32) };
                    // sistema del blocco: x est / n nord, unita' Daggerfall
                    let ex = sr.x as f32 + lx * cs - ln * sn;
                    let en = anchor_n + lx * sn + ln * cs;
                    let inside = (0.0..BLOCK_UNITS).contains(&ex) && (0.0..BLOCK_UNITS).contains(&en);

                    let (mx, mn) = if inside {
                        // dentro il blocco: clamp ai bordi e trascina verso il
                        // centro della stanza per evitare NPC incollati ai muri
                        let m = EDGE_MARGIN_M / DF_UNIT;
                        let mut ex = ex.clamp(m, BLOCK_UNITS - m);
                        let mut en = en.clamp(m, BLOCK_UNITS - m);
                        let pull = 0.15;
                        ex += (ccx - ex) * pull;
                        en += (ccn - en) * pull;
                        (ex, en)
                    } else {
                        // coordinata locale non allineata col blocco: piazza su un
                        // anello davanti alla porta (o al centro blocco se manca)
                        let (px, pz) = match door {
                            Some(d) => (d.world_pos[0], d.world_pos[2]),
                            None => (bx0 + BLOCK_SIZE_M * 0.5, -(bz0 + BLOCK_SIZE_M * 0.5)),
                        };
                        let ang = spawned as f32 * std::f32::consts::FRAC_PI_3;
                        let ax = px + 3.0 * ang.cos();
                        let az = pz + 3.0 * ang.sin();
                        ((ax - bx0) / DF_UNIT, (-az - bz0) / DF_UNIT)
                    };

                    // coordinate finali nel tile (locali al map pixel, come le porte)
                    let mut wx = bx0 + mx * DF_UNIT;
                    let mut wz = -(bz0 + mn * DF_UNIT);
                    wx = wx.clamp(EDGE_MARGIN_M, MAP_PIXEL_SIZE - EDGE_MARGIN_M);
                    wz = wz.clamp(-(MAP_PIXEL_SIZE - EDGE_MARGIN_M), -EDGE_MARGIN_M);

                    out.push(ExteriorPerson {
                        x: wx,
                        ground_y: ground_height(wx, wz),
                        z: wz,
                        direction_deg: sr.rotation_deg,
                        texture_archive: p.texture_archive,
                        texture_record: p.texture_record,
                        block_name: upper_name.clone(),
                    });
                    spawned += 1;
                }
                if spawned >= NPC_PER_BLOCK {
                    break;
                }
            }
        }
    }

    out
}
