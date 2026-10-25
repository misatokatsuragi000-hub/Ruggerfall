//! `Texture2D` (class id 28) extraction and pixel decoding.

use crate::bundle::Bundle;
use crate::dxt;
use crate::error::{Error, Result};
use crate::serialized::Value;

/// `UnityEngine.TextureFormat` numeric value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureFormat(pub u32);

impl TextureFormat {
    pub const ALPHA8: Self = Self(1);
    pub const ARGB4444: Self = Self(2);
    pub const RGB24: Self = Self(3);
    pub const RGBA32: Self = Self(4);
    pub const ARGB32: Self = Self(5);
    pub const RGB565: Self = Self(7);
    pub const DXT1: Self = Self(10);
    pub const DXT5: Self = Self(12);
    pub const RGBA4444: Self = Self(13);
    pub const BGRA32: Self = Self(14);
    pub const BC6H: Self = Self(24);
    pub const BC7: Self = Self(25);
    pub const BC4: Self = Self(26);
    pub const BC5: Self = Self(27);
    pub const DXT1_CRUNCHED: Self = Self(28);
    pub const DXT5_CRUNCHED: Self = Self(29);

    pub fn name(self) -> &'static str {
        match self.0 {
            1 => "Alpha8",
            2 => "ARGB4444",
            3 => "RGB24",
            4 => "RGBA32",
            5 => "ARGB32",
            7 => "RGB565",
            9 => "R16",
            10 => "DXT1",
            12 => "DXT5",
            13 => "RGBA4444",
            14 => "BGRA32",
            24 => "BC6H",
            25 => "BC7",
            26 => "BC4",
            27 => "BC5",
            28 => "DXT1Crunched",
            29 => "DXT5Crunched",
            30..=33 => "PVRTC",
            34 | 45..=47 => "ETC/ETC2",
            48..=60 => "ASTC",
            64 | 65 => "ETC_Crunched",
            _ => "unknown",
        }
    }

    /// Byte size of mip level 0 for an uncompressed or block-compressed format, if known.
    pub fn mip0_size(self, w: usize, h: usize) -> Option<usize> {
        let blocks = ((w + 3) / 4) * ((h + 3) / 4);
        Some(match self.0 {
            1 => w * h,
            2 | 7 | 13 => w * h * 2,
            3 => w * h * 3,
            4 | 5 | 14 => w * h * 4,
            10 | 26 => blocks * 8,
            12 | 25 | 27 | 24 => blocks * 16,
            _ => return None,
        })
    }

    /// `true` for formats a GPU can sample directly from the raw `data` (BCn); useful to skip
    /// the CPU decode and upload blocks as-is.
    pub fn is_bc(self) -> bool {
        matches!(self.0, 10 | 12 | 24 | 25 | 26 | 27)
    }
}

#[derive(Debug, Clone)]
pub struct Texture2D {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub format: TextureFormat,
    pub mip_count: u32,
    /// All mip levels, level 0 first, exactly as stored by Unity (bottom row first).
    pub data: Vec<u8>,
}

impl Texture2D {
    /// Builds a texture from the decoded object. Streamed data (`m_StreamData`) is fetched from
    /// the matching `.resS` node of `bundle`.
    pub fn from_value(v: &Value, bundle: &Bundle) -> Result<Self> {
        let name = v.get("m_Name").and_then(Value::as_str).unwrap_or_default().to_owned();
        let width = field_u32(v, "m_Width")?;
        let height = field_u32(v, "m_Height")?;
        let format = TextureFormat(field_u32(v, "m_TextureFormat")?);
        let mip_count = v.get("m_MipCount").and_then(Value::as_u64).unwrap_or(1) as u32;

        let mut data = v
            .get("image data")
            .and_then(Value::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default();

        if data.is_empty() {
            if let Some(sd) = v.get("m_StreamData") {
                let size = sd.get("size").and_then(Value::as_u64).unwrap_or(0) as usize;
                let offset = sd.get("offset").and_then(Value::as_u64).unwrap_or(0);
                let path = sd.get("path").and_then(Value::as_str).unwrap_or("");
                if size > 0 && !path.is_empty() {
                    // e.g. "archive:/CAB-0123.../CAB-0123....resS" -> node "CAB-0123....resS"
                    let base = path.rsplit('/').next().unwrap_or(path);
                    let node = bundle
                        .node_by_name(base)
                        .ok_or_else(|| Error::Invalid(format!("stream node `{base}` not found in bundle")))?;
                    data = bundle.read_node(node, offset, size)?;
                }
            }
        }
        if data.is_empty() {
            return Err(Error::Invalid(format!("texture `{name}` has no pixel data")));
        }
        Ok(Self { name, width, height, format, mip_count, data })
    }

    /// Decodes mip level 0 to RGBA8 in **storage order** (row 0 = bottom row of the image).
    pub fn to_rgba8(&self) -> Result<Vec<u8>> {
        let (w, h) = (self.width as usize, self.height as usize);
        let need = self
            .format
            .mip0_size(w, h)
            .ok_or(Error::UnsupportedTextureFormat(self.format.0, self.format.name()))?;
        let d = self
            .data
            .get(..need)
            .ok_or_else(|| Error::Invalid(format!("texture `{}` data too short", self.name)))?;
        let n = w * h;
        let mut out = Vec::with_capacity(n * 4);
        match self.format {
            TextureFormat::ALPHA8 => d.iter().for_each(|&a| out.extend_from_slice(&[255, 255, 255, a])),
            TextureFormat::RGB24 => d.chunks_exact(3).for_each(|p| out.extend_from_slice(&[p[0], p[1], p[2], 255])),
            TextureFormat::RGBA32 => out.extend_from_slice(d),
            TextureFormat::ARGB32 => d.chunks_exact(4).for_each(|p| out.extend_from_slice(&[p[1], p[2], p[3], p[0]])),
            TextureFormat::BGRA32 => d.chunks_exact(4).for_each(|p| out.extend_from_slice(&[p[2], p[1], p[0], p[3]])),
            TextureFormat::RGB565 => d.chunks_exact(2).for_each(|p| {
                let v = u16::from_le_bytes([p[0], p[1]]);
                let (r, g, b) = (((v >> 11) & 31) as u8, ((v >> 5) & 63) as u8, (v & 31) as u8);
                out.extend_from_slice(&[(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2), 255]);
            }),
            TextureFormat::ARGB4444 => d.chunks_exact(2).for_each(|p| {
                let v = u16::from_le_bytes([p[0], p[1]]);
                let n = |s: u32| (((v >> s) & 15) as u8) * 17;
                out.extend_from_slice(&[n(8), n(4), n(0), n(12)]);
            }),
            TextureFormat::RGBA4444 => d.chunks_exact(2).for_each(|p| {
                let v = u16::from_le_bytes([p[0], p[1]]);
                let n = |s: u32| (((v >> s) & 15) as u8) * 17;
                out.extend_from_slice(&[n(12), n(8), n(4), n(0)]);
            }),
            TextureFormat::DXT1 => return dxt::decode_bc1(d, w, h),
            TextureFormat::DXT5 => return dxt::decode_bc3(d, w, h),
            f => return Err(Error::UnsupportedTextureFormat(f.0, f.name())),
        }
        Ok(out)
    }

    /// Like [`to_rgba8`](Self::to_rgba8) but flipped so row 0 is the top row, which is what PNG
    /// encoders and most GPU upload paths expect.
    pub fn to_rgba8_top_down(&self) -> Result<Vec<u8>> {
        let mut px = self.to_rgba8()?;
        let stride = self.width as usize * 4;
        let h = self.height as usize;
        for y in 0..h / 2 {
            let (top, bottom) = px.split_at_mut((h - 1 - y) * stride);
            top[y * stride..(y + 1) * stride].swap_with_slice(&mut bottom[..stride]);
        }
        Ok(px)
    }
}

fn field_u32(v: &Value, name: &'static str) -> Result<u32> {
    v.get(name)
        .and_then(Value::as_u64)
        .and_then(|x| u32::try_from(x).ok())
        .ok_or(Error::MissingField(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tex(format: TextureFormat, w: u32, h: u32, data: Vec<u8>) -> Texture2D {
        Texture2D { name: "t".into(), width: w, height: h, format, mip_count: 1, data }
    }

    #[test]
    fn channel_orders() {
        assert_eq!(tex(TextureFormat::ARGB32, 1, 1, vec![9, 1, 2, 3]).to_rgba8().unwrap(), [1, 2, 3, 9]);
        assert_eq!(tex(TextureFormat::BGRA32, 1, 1, vec![3, 2, 1, 9]).to_rgba8().unwrap(), [1, 2, 3, 9]);
        assert_eq!(tex(TextureFormat::RGB24, 1, 1, vec![1, 2, 3]).to_rgba8().unwrap(), [1, 2, 3, 255]);
        assert_eq!(tex(TextureFormat::ALPHA8, 1, 1, vec![7]).to_rgba8().unwrap(), [255, 255, 255, 7]);
    }

    #[test]
    fn rgb565_expands_to_full_range() {
        let px = tex(TextureFormat::RGB565, 1, 1, 0xFFFFu16.to_le_bytes().to_vec()).to_rgba8().unwrap();
        assert_eq!(px, [255, 255, 255, 255]);
    }

    #[test]
    fn flip_swaps_rows() {
        let t = tex(TextureFormat::RGBA32, 1, 3, vec![1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3]);
        assert_eq!(t.to_rgba8_top_down().unwrap(), [3, 3, 3, 3, 2, 2, 2, 2, 1, 1, 1, 1]);
    }

    #[test]
    fn crunched_is_reported_not_garbled() {
        let err = tex(TextureFormat::DXT5_CRUNCHED, 4, 4, vec![0; 16]).to_rgba8().unwrap_err();
        assert!(matches!(err, Error::UnsupportedTextureFormat(29, "DXT1Crunched" | "DXT5Crunched")));
    }

    #[test]
    fn short_data_is_an_error() {
        assert!(tex(TextureFormat::RGBA32, 2, 2, vec![0; 15]).to_rgba8().is_err());
    }
}

#[derive(Debug, Clone)]
pub struct Texture2DArray {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub format: TextureFormat,
    pub mip_count: u32,
    /// Dati binari grezzi (per BC7 pronti per l'upload diretto in GPU/Vulkan).
    pub data: Vec<u8>,
}

impl Texture2DArray {
    pub fn from_value(v: &Value, bundle: &Bundle) -> Result<Self> {
        let name = v.get("m_Name").and_then(Value::as_str).unwrap_or_default().to_owned();
        let width = field_u32(v, "m_Width")?;
        let height = field_u32(v, "m_Height")?;
        let depth = field_u32(v, "m_Depth")?;
        
        let format_num = v
            .get("m_TextureFormat")
            .or_else(|| v.get("m_Format"))
            .and_then(Value::as_u64)
            .ok_or(Error::MissingField("m_TextureFormat"))? as u32;
        let format = TextureFormat(format_num);
        let mip_count = v.get("m_MipCount").and_then(Value::as_u64).unwrap_or(1) as u32;

        let mut data = v
            .get("image data")
            .or_else(|| v.get("_typelessdata"))
            .and_then(Value::as_bytes)
            .map(<[u8]>::to_vec)
            .unwrap_or_default();

        if data.is_empty() {
            if let Some(sd) = v.get("m_StreamData") {
                let size = sd.get("size").and_then(Value::as_u64).unwrap_or(0) as usize;
                let offset = sd.get("offset").and_then(Value::as_u64).unwrap_or(0);
                let path = sd.get("path").and_then(Value::as_str).unwrap_or("");
                if size > 0 && !path.is_empty() {
                    let base = path.rsplit('/').next().unwrap_or(path);
                    let node = bundle
                        .node_by_name(base)
                        .ok_or_else(|| Error::Invalid(format!("stream node `{base}` not found in bundle")))?;
                    data = bundle.read_node(node, offset, size)?;
                }
            }
        }
        if data.is_empty() {
            return Err(Error::Invalid(format!("texture array `{name}` has no pixel data")));
        }
        Ok(Self { name, width, height, depth, format, mip_count, data })
    }
}
