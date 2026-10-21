//! Geometria del terreno di un map pixel: 128x128 celle, ognuna con 4 vertici
//! propri per poter avere UV diverse (tile, rotazione, specchiatura).
//!
//! Sistema di coordinate locale della mesh (pensato per Bevy, Y verso l'alto):
//! x cresce a est, z = -nord, origine nell'angolo sud-ovest del map pixel.
//! Scale come in DFU: un map pixel è 32768 * 0,025 m, le altezze sono moltiplicate
//! per `TERRAIN_HEIGHT_SCALE`.

use crate::atlas::{ATLAS_COLS, ATLAS_ROWS};
use crate::heightmap::HeightSamples;
use crate::tilemap::TILEMAP_DIM;

/// Lato di un map pixel in metri (WorldMapTerrainDim * GlobalScale).
pub const MAP_PIXEL_SIZE: f32 = 32768.0 * 0.025;
/// Lato di una cella in metri.
pub const CELL_SIZE: f32 = MAP_PIXEL_SIZE / TILEMAP_DIM as f32;
/// Moltiplicatore verticale (TerrainScale predefinito di DFU).
pub const TERRAIN_HEIGHT_SCALE: f32 = 2.5;
/// Margine in texel dai bordi della tile, per non pescare i vicini.
const UV_INSET_TEXELS: f32 = 0.25;

#[derive(Default, Clone, Debug)]
pub struct TerrainMeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Altezza in metri di un punto della griglia.
fn grid_height(samples: &HeightSamples, x: usize, y: usize) -> f32 {
    samples.height(x, y) * TERRAIN_HEIGHT_SCALE
}

/// Altezza in metri nel punto (gx, gy) delle coordinate di griglia (0..=128, y a
/// nord), interpolata sugli stessi due triangoli con cui viene disegnata la mesh.
pub fn terrain_height(samples: &HeightSamples, gx: f32, gy: f32) -> f32 {
    let cells = (samples.dim - 1) as f32;
    let gx = gx.clamp(0.0, cells);
    let gy = gy.clamp(0.0, cells);
    let ix = (gx.floor() as usize).min(samples.dim - 2);
    let iy = (gy.floor() as usize).min(samples.dim - 2);
    let (fx, fy) = (gx - ix as f32, gy - iy as f32);
    let h00 = grid_height(samples, ix, iy);
    let h10 = grid_height(samples, ix + 1, iy);
    let h01 = grid_height(samples, ix, iy + 1);
    let h11 = grid_height(samples, ix + 1, iy + 1);
    if fx >= fy {
        // triangolo (p00, p10, p11)
        h00 + fx * (h10 - h00) + fy * (h11 - h10)
    } else {
        // triangolo (p00, p11, p01)
        h00 + fx * (h11 - h01) + fy * (h01 - h00)
    }
}

/// Trasformazione delle coordinate nella cella come in
/// DaggerfallTilemapTextureArray.shader: `t` = tileData & 3 (1 = rotazione,
/// 2 = specchiatura, cioè 180 gradi, 3 = entrambe). (vx, vy) con vy verso nord.
pub fn tile_uv_transform(t: u8, vx: f32, vy: f32) -> (f32, f32) {
    match t & 3 {
        0 => (vx, vy),
        1 => (vy, 1.0 - vx),
        2 => (1.0 - vx, 1.0 - vy),
        _ => (1.0 - vy, vx),
    }
}

/// `tilemap` è la tilemap già nel formato per lo shader (`encode_for_shader`):
/// `(tile << 2) | trasformazione`, indice x + y * 128.
/// `tile_px` è la dimensione in pixel di una tile dell'atlante.
pub fn build_terrain_mesh(samples: &HeightSamples, tilemap: &[u8], tile_px: (usize, usize)) -> TerrainMeshData {
    assert_eq!(samples.dim, TILEMAP_DIM + 1, "servono 129x129 campioni");
    assert_eq!(tilemap.len(), TILEMAP_DIM * TILEMAP_DIM);
    let (tw, th) = (tile_px.0 as f32, tile_px.1 as f32);
    let (atlas_w, atlas_h) = (tw * ATLAS_COLS as f32, th * ATLAS_ROWS as f32);

    // Normali per punto della griglia, da differenze centrali.
    let dim = samples.dim;
    let normal_at = |x: usize, y: usize| -> [f32; 3] {
        let (x0, x1) = (x.saturating_sub(1), (x + 1).min(dim - 1));
        let (y0, y1) = (y.saturating_sub(1), (y + 1).min(dim - 1));
        let dhdx = (grid_height(samples, x1, y) - grid_height(samples, x0, y)) / ((x1 - x0) as f32 * CELL_SIZE);
        let dhdy = (grid_height(samples, x, y1) - grid_height(samples, x, y0)) / ((y1 - y0) as f32 * CELL_SIZE);
        // z locale = -nord, quindi dh/dz = -dh/dy
        let (nx, ny, nz) = (-dhdx, 1.0, dhdy);
        let len = (nx * nx + ny * ny + nz * nz).sqrt();
        [nx / len, ny / len, nz / len]
    };

    let cells = TILEMAP_DIM * TILEMAP_DIM;
    let mut mesh = TerrainMeshData {
        positions: Vec::with_capacity(cells * 4),
        normals: Vec::with_capacity(cells * 4),
        uvs: Vec::with_capacity(cells * 4),
        indices: Vec::with_capacity(cells * 6),
    };

    // Angoli di una cella in senso antiorario visti da sopra (nord in alto).
    const CORNERS: [(usize, usize); 4] = [(0, 0), (1, 0), (1, 1), (0, 1)];
    for cy in 0..TILEMAP_DIM {
        for cx in 0..TILEMAP_DIM {
            let value = tilemap[cx + cy * TILEMAP_DIM];
            let tile = ((value >> 2) as usize).min(ATLAS_COLS * ATLAS_ROWS - 1);
            let (col, row) = ((tile % ATLAS_COLS) as f32, (tile / ATLAS_COLS) as f32);
            let base = mesh.positions.len() as u32;
            for (dx, dy) in CORNERS {
                let (gx, gy) = (cx + dx, cy + dy);
                mesh.positions.push([
                    gx as f32 * CELL_SIZE,
                    grid_height(samples, gx, gy),
                    -(gy as f32 * CELL_SIZE),
                ]);
                mesh.normals.push(normal_at(gx, gy));

                let (u, v) = tile_uv_transform(value, dx as f32, dy as f32);
                // v verso nord; nell'immagine la riga 0 è in alto, quindi si capovolge.
                let px = UV_INSET_TEXELS + u * (tw - 2.0 * UV_INSET_TEXELS);
                let py = UV_INSET_TEXELS + (1.0 - v) * (th - 2.0 * UV_INSET_TEXELS);
                mesh.uvs.push([(col * tw + px) / atlas_w, (row * th + py) / atlas_h]);
            }
            mesh.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(f: impl Fn(usize, usize) -> f32) -> HeightSamples {
        // f(x, y) in metri "grezzi" (prima di TERRAIN_HEIGHT_SCALE)
        let dim = 129;
        let mut data = vec![0.0; dim * dim];
        for x in 0..dim {
            for y in 0..dim {
                data[y + x * dim] = f(x, y) / crate::heightmap::MAX_TERRAIN_HEIGHT;
            }
        }
        HeightSamples { dim, data }
    }

    fn plain_tilemap(value: u8) -> Vec<u8> {
        vec![value; 128 * 128]
    }

    #[test]
    fn constants_match_dfu() {
        assert!((MAP_PIXEL_SIZE - 819.2).abs() < 1e-3);
        assert!((CELL_SIZE - 6.4).abs() < 1e-4);
    }

    #[test]
    fn uv_transform_follows_the_shader() {
        assert_eq!(tile_uv_transform(0, 0.25, 0.75), (0.25, 0.75));
        assert_eq!(tile_uv_transform(1, 0.25, 0.75), (0.75, 0.75)); // (vy, 1 - vx)
        assert_eq!(tile_uv_transform(2, 0.25, 0.75), (0.75, 0.25)); // (1 - vx, 1 - vy)
        assert_eq!(tile_uv_transform(3, 0.25, 0.75), (0.25, 0.25)); // (1 - vy, vx)
        // il valore completo dello shader porta con sé anche l'indice di tile
        assert_eq!(tile_uv_transform((10 << 2) | 2, 0.0, 0.0), (1.0, 1.0));
    }

    #[test]
    fn mesh_sizes_and_upward_winding() {
        let m = build_terrain_mesh(&samples(|_, _| 100.0), &plain_tilemap(0), (64, 64));
        assert_eq!(m.positions.len(), 128 * 128 * 4);
        assert_eq!(m.normals.len(), m.positions.len());
        assert_eq!(m.uvs.len(), m.positions.len());
        assert_eq!(m.indices.len(), 128 * 128 * 6);
        assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
        for tri in m.indices.chunks(3).take(50) {
            let p = |i: u32| m.positions[i as usize];
            let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
            let (u, v) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
            let ny = u[2] * v[0] - u[0] * v[2];
            assert!(ny > 0.0, "triangolo rivolto verso il basso");
        }
        // vertice (0,0) in origine, ultimo angolo a est e a nord (z negativo)
        assert_eq!(m.positions[0], [0.0, 150.0, 0.0]);
        let last = m.positions[128 * 128 * 4 - 2]; // angolo (1,1) dell'ultima cella
        assert!((last[0] - 819.2).abs() < 1e-2 && (last[2] + 819.2).abs() < 1e-2);
    }

    #[test]
    fn flat_ground_has_upward_normals_and_slopes_tilt_them() {
        let flat = build_terrain_mesh(&samples(|_, _| 50.0), &plain_tilemap(0), (64, 64));
        assert!(flat.normals.iter().all(|n| (n[1] - 1.0).abs() < 1e-5));
        // sale verso est: la normale punta verso ovest (x negativa)
        let east = build_terrain_mesh(&samples(|x, _| x as f32 * 2.0), &plain_tilemap(0), (64, 64));
        assert!(east.normals[10][0] < -0.1 && east.normals[10][1] > 0.0);
        // sale verso nord: la normale punta a sud, cioè verso +z locale
        let north = build_terrain_mesh(&samples(|_, y| y as f32 * 2.0), &plain_tilemap(0), (64, 64));
        assert!(north.normals[10][2] > 0.1);
    }

    #[test]
    fn uvs_land_inside_the_right_atlas_tile() {
        let tile = 10usize; // colonna 2, riga 1
        let m = build_terrain_mesh(&samples(|_, _| 100.0), &plain_tilemap((tile << 2) as u8), (64, 64));
        let (u0, u1) = (2.0 * 64.0 / 512.0, 3.0 * 64.0 / 512.0);
        let (v0, v1) = (1.0 * 64.0 / 448.0, 2.0 * 64.0 / 448.0);
        for uv in &m.uvs {
            assert!(uv[0] > u0 && uv[0] < u1 && uv[1] > v0 && uv[1] < v1, "{uv:?}");
        }
        // senza trasformazioni: angolo sud-ovest della cella = in basso a sinistra della tile
        assert!(m.uvs[0][0] < m.uvs[1][0]); // verso est la u cresce
        assert!(m.uvs[0][1] > m.uvs[3][1]); // verso nord la v (immagine) diminuisce
    }

    #[test]
    fn terrain_height_matches_vertices_and_the_drawn_triangles() {
        let s = samples(|x, y| (x * 3 + y * 5) as f32);
        // sui punti della griglia coincide con le altezze dei vertici
        for (gx, gy) in [(0, 0), (10, 20), (128, 128), (127, 3)] {
            let expected = s.height(gx, gy) * TERRAIN_HEIGHT_SCALE;
            assert!((terrain_height(&s, gx as f32, gy as f32) - expected).abs() < 1e-3);
        }
        // un piano viene riprodotto esattamente anche dentro la cella
        let (gx, gy) = (40.3, 77.8);
        let expected = (gx * 3.0 + gy * 5.0) * TERRAIN_HEIGHT_SCALE;
        assert!((terrain_height(&s, gx, gy) - expected).abs() < 0.05, "{}", terrain_height(&s, gx, gy));
        // fuori dai bordi viene bloccato
        assert_eq!(terrain_height(&s, -5.0, -5.0), terrain_height(&s, 0.0, 0.0));
    }

    #[test]
    fn height_on_the_diagonal_uses_the_mesh_diagonal() {
        // una cella con un solo angolo alzato (p11): la diagonale p00-p11 resta una linea retta
        let s = samples(|x, y| if (x, y) == (11, 11) { 10.0 } else { 0.0 });
        let h = terrain_height(&s, 10.5, 10.5);
        assert!((h - 5.0 * TERRAIN_HEIGHT_SCALE).abs() < 1e-3, "{h}");
        // triangolo (p00, p10, p11): qui conta solo fy * (h11 - h10)
        assert!((terrain_height(&s, 10.9, 10.1) - (0.1 * 10.0) * TERRAIN_HEIGHT_SCALE).abs() < 1e-3);
    }
}
