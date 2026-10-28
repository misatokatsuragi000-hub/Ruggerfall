//! BC1 (DXT1) and BC3 (DXT5) software decoders producing tightly packed RGBA8.
//! Output row 0 is the first row stored in the source (no vertical flip is applied here).

use crate::error::{Error, Result};

fn rgb565(c: u16) -> [u8; 3] {
    let (r, g, b) = (((c >> 11) & 31) as u8, ((c >> 5) & 63) as u8, (c & 31) as u8);
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}

/// Builds the 4-entry colour palette of a BC1 colour block (first 8 bytes of `block`).
/// `force_four`: BC2/BC3 colour blocks always use the 4-colour mode, regardless of c0 <= c1.
fn bc1_palette(block: &[u8], force_four: bool) -> [[u8; 4]; 4] {
    let c0 = u16::from_le_bytes([block[0], block[1]]);
    let c1 = u16::from_le_bytes([block[2], block[3]]);
    let (p0, p1) = (rgb565(c0), rgb565(c1));
    let mut pal = [[0u8; 4]; 4];
    pal[0] = [p0[0], p0[1], p0[2], 255];
    pal[1] = [p1[0], p1[1], p1[2], 255];
    if c0 > c1 || force_four {
        for k in 0..3 {
            pal[2][k] = ((2 * p0[k] as u32 + p1[k] as u32) / 3) as u8;
            pal[3][k] = ((p0[k] as u32 + 2 * p1[k] as u32) / 3) as u8;
        }
        pal[2][3] = 255;
        pal[3][3] = 255;
    } else {
        for k in 0..3 {
            pal[2][k] = ((p0[k] as u32 + p1[k] as u32) / 2) as u8;
        }
        pal[2][3] = 255;
        pal[3] = [0, 0, 0, 0]; // transparent black
    }
    pal
}

fn check(src: &[u8], w: usize, h: usize, block_bytes: usize) -> Result<(usize, usize)> {
    let (bw, bh) = ((w + 3) / 4, (h + 3) / 4);
    if src.len() < bw * bh * block_bytes {
        return Err(Error::Invalid(format!(
            "texture data too short: {} bytes, need {}",
            src.len(),
            bw * bh * block_bytes
        )));
    }
    Ok((bw, bh))
}

pub fn decode_bc1(src: &[u8], w: usize, h: usize) -> Result<Vec<u8>> {
    let (bw, bh) = check(src, w, h, 8)?;
    let mut out = vec![0u8; w * h * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let blk = &src[(by * bw + bx) * 8..][..8];
            let pal = bc1_palette(blk, false);
            let idx = u32::from_le_bytes([blk[4], blk[5], blk[6], blk[7]]);
            for py in 0..4 {
                for px in 0..4 {
                    let (x, y) = (bx * 4 + px, by * 4 + py);
                    if x < w && y < h {
                        let c = (idx >> (2 * (py * 4 + px))) & 3;
                        out[(y * w + x) * 4..][..4].copy_from_slice(&pal[c as usize]);
                    }
                }
            }
        }
    }
    Ok(out)
}

pub fn decode_bc3(src: &[u8], w: usize, h: usize) -> Result<Vec<u8>> {
    let (bw, bh) = check(src, w, h, 16)?;
    let mut out = vec![0u8; w * h * 4];
    for by in 0..bh {
        for bx in 0..bw {
            let blk = &src[(by * bw + bx) * 16..][..16];
            let (a0, a1) = (blk[0] as u32, blk[1] as u32);
            let mut bits = 0u64;
            for i in 0..6 {
                bits |= (blk[2 + i] as u64) << (8 * i);
            }
            let mut apal = [0u8; 8];
            apal[0] = a0 as u8;
            apal[1] = a1 as u8;
            if a0 > a1 {
                for i in 0..6u32 {
                    apal[2 + i as usize] = (((6 - i) * a0 + (1 + i) * a1) / 7) as u8;
                }
            } else {
                for i in 0..4u32 {
                    apal[2 + i as usize] = (((4 - i) * a0 + (1 + i) * a1) / 5) as u8;
                }
                apal[6] = 0;
                apal[7] = 255;
            }
            let pal = bc1_palette(&blk[8..16], true);
            let cidx = u32::from_le_bytes([blk[12], blk[13], blk[14], blk[15]]);
            for py in 0..4 {
                for px in 0..4 {
                    let (x, y) = (bx * 4 + px, by * 4 + py);
                    if x < w && y < h {
                        let n = py * 4 + px;
                        let c = pal[((cidx >> (2 * n)) & 3) as usize];
                        let a = apal[((bits >> (3 * n)) & 7) as usize];
                        out[(y * w + x) * 4..][..4].copy_from_slice(&[c[0], c[1], c[2], a]);
                    }
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bc1_solid_red() {
        // c0 = 0xF800 (pure red), c1 = 0, all indices 0 -> every pixel is c0.
        let blk = [0x00, 0xF8, 0x00, 0x00, 0, 0, 0, 0];
        let out = decode_bc1(&blk, 4, 4).unwrap();
        assert_eq!(out.len(), 64);
        assert!(out.chunks(4).all(|p| p == [255, 0, 0, 255]));
    }

    #[test]
    fn bc1_punchthrough_alpha() {
        // c0 <= c1 selects 3-colour mode; index 3 is transparent black.
        let blk = [0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        let out = decode_bc1(&blk, 4, 4).unwrap();
        assert!(out.chunks(4).all(|p| p == [0, 0, 0, 0]));
    }

    #[test]
    fn bc1_crops_non_multiple_of_four() {
        let blk = [0x00, 0xF8, 0x00, 0x00, 0, 0, 0, 0];
        let out = decode_bc1(&blk, 2, 3).unwrap();
        assert_eq!(out.len(), 2 * 3 * 4);
    }

    #[test]
    fn bc3_alpha_and_color() {
        // a0 = a1 = 128 (5-interpolant mode, index 0 -> a0), colour block solid red.
        let blk = [128, 128, 0, 0, 0, 0, 0, 0, 0x00, 0xF8, 0x00, 0x00, 0, 0, 0, 0];
        let out = decode_bc3(&blk, 4, 4).unwrap();
        assert!(out.chunks(4).all(|p| p == [255, 0, 0, 128]));
    }

    #[test]
    fn truncated_input_is_an_error() {
        assert!(decode_bc1(&[0; 7], 4, 4).is_err());
        assert!(decode_bc3(&[0; 15], 4, 4).is_err());
    }
}
