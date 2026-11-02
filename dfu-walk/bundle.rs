//! UnityFS container (the outer layer of a `.dfmod`).
//!
//! Layout (all big-endian):
//! ```text
//! "UnityFS\0" | u32 format version | cstr "5.x.x" | cstr unity revision
//! u64 total size | u32 compressed blockinfo size | u32 uncompressed blockinfo size | u32 flags
//! [align 16 if version >= 7]
//! blockinfo (compressed)   -- or at the end of the file if flags & 0x80
//! data blocks...
//! ```
//! Blockinfo, once decompressed: `16B hash | i32 nblocks | (u32 usize, u32 csize, u16 flags)* |
//! i32 nnodes | (i64 offset, i64 size, u32 flags, cstr path)*`.
//!
//! Blocks are decoded lazily and cached, so reading one texture out of a multi-hundred-MB bundle
//! only decompresses the blocks that texture touches (LZ4/LZ4HC bundles; an LZMA bundle is one
//! solid block and must be decoded whole).

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::ops::Deref;
use std::path::Path;
use std::sync::{Arc, Mutex};

use memmap2::Mmap;

use crate::error::{Error, Result};
use crate::reader::Reader;

const COMPRESSION_MASK: u32 = 0x3f;
const FLAG_BLOCKINFO_AT_END: u32 = 0x80;

/// A file stored inside the bundle (a serialized file, a `.resS` stream, ...).
#[derive(Debug, Clone)]
pub struct Node {
    /// Offset in the *uncompressed* data stream.
    pub offset: u64,
    pub size: u64,
    /// Bit 2 (value 4) marks a SerializedFile.
    pub flags: u32,
    pub path: String,
}

impl Node {
    pub fn is_serialized_file(&self) -> bool {
        self.flags & 4 != 0
    }
}

#[derive(Debug, Clone)]
struct Block {
    uncompressed_start: u64,
    uncompressed_size: u32,
    compressed_start: usize,
    compressed_size: u32,
    flags: u16,
}

enum Backing {
    Mmap(Mmap),
    Owned(Vec<u8>),
}

impl Deref for Backing {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Backing::Mmap(m) => m,
            Backing::Owned(v) => v,
        }
    }
}

struct BlockCache {
    map: HashMap<usize, Arc<Vec<u8>>>,
    order: VecDeque<usize>,
    capacity: usize,
}

impl BlockCache {
    fn get(&self, i: usize) -> Option<Arc<Vec<u8>>> {
        self.map.get(&i).cloned()
    }
    fn insert(&mut self, i: usize, v: Arc<Vec<u8>>) {
        if self.map.insert(i, v).is_none() {
            self.order.push_back(i);
            while self.order.len() > self.capacity {
                if let Some(old) = self.order.pop_front() {
                    self.map.remove(&old);
                }
            }
        }
    }
}

pub struct Bundle {
    backing: Backing,
    blocks: Vec<Block>,
    nodes: Vec<Node>,
    cache: Mutex<BlockCache>,
    pub format_version: u32,
    pub unity_revision: String,
}

impl Bundle {
    /// Memory-maps `path`. The file must not be modified while the bundle is alive.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path)?;
        // SAFETY: standard mmap caveat: undefined behaviour if another process truncates the file.
        let map = unsafe { Mmap::map(&file)? };
        Self::from_backing(Backing::Mmap(map))
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        Self::from_backing(Backing::Owned(bytes))
    }

    fn from_backing(backing: Backing) -> Result<Self> {
        let data: &[u8] = &backing;
        let mut r = Reader::new(data, false);

        let sig = r.cstr()?;
        if sig != "UnityFS" {
            return Err(Error::BadSignature(sig));
        }
        let format_version = r.u32()?;
        if !(6..=8).contains(&format_version) {
            return Err(Error::UnsupportedBundleVersion(format_version));
        }
        let _player_version = r.cstr()?;
        let unity_revision = r.cstr()?;
        let _total_size = r.u64()?;
        let comp_info_size = r.u32()? as usize;
        let uncomp_info_size = r.u32()? as usize;
        let flags = r.u32()?;

        if format_version >= 7 {
            r.align(16)?;
        }

        let (info_start, data_start) = if flags & FLAG_BLOCKINFO_AT_END != 0 {
            let s = data
                .len()
                .checked_sub(comp_info_size)
                .ok_or_else(|| Error::Invalid("blockinfo larger than file".into()))?;
            (s, r.pos())
        } else {
            (r.pos(), r.pos() + comp_info_size)
        };
        let comp_info = data
            .get(info_start..info_start + comp_info_size)
            .ok_or_else(|| Error::Invalid("blockinfo out of range".into()))?;
        let info = decompress(comp_info, uncomp_info_size, flags & COMPRESSION_MASK)?;

        let mut ir = Reader::new(&info, false);
        ir.skip(16)?; // hash
        let nblocks = ir.i32()?;
        if nblocks < 0 {
            return Err(Error::Invalid("negative block count".into()));
        }
        let mut blocks = Vec::with_capacity(nblocks as usize);
        let (mut ustart, mut cstart) = (0u64, data_start);
        for _ in 0..nblocks {
            let usize_ = ir.u32()?;
            let csize = ir.u32()?;
            let bflags = ir.u16()?;
            blocks.push(Block {
                uncompressed_start: ustart,
                uncompressed_size: usize_,
                compressed_start: cstart,
                compressed_size: csize,
                flags: bflags,
            });
            ustart += usize_ as u64;
            cstart += csize as usize;
        }
        if cstart > data.len() {
            return Err(Error::Invalid("data blocks extend past end of file".into()));
        }

        let nnodes = ir.i32()?;
        if nnodes < 0 {
            return Err(Error::Invalid("negative node count".into()));
        }
        let mut nodes = Vec::with_capacity(nnodes as usize);
        for _ in 0..nnodes {
            let offset = ir.i64()?;
            let size = ir.i64()?;
            let nflags = ir.u32()?;
            let path = ir.cstr()?;
            if offset < 0 || size < 0 {
                return Err(Error::Invalid("negative node offset/size".into()));
            }
            nodes.push(Node { offset: offset as u64, size: size as u64, flags: nflags, path });
        }

        Ok(Self {
            backing,
            blocks,
            nodes,
            cache: Mutex::new(BlockCache { map: HashMap::new(), order: VecDeque::new(), capacity: 16 }),
            format_version,
            unity_revision,
        })
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn node_by_name(&self, name: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.path == name || n.path.rsplit('/').next() == Some(name))
    }

    /// Reads `len` bytes at `offset` *within* `node`.
    pub fn read_node(&self, node: &Node, offset: u64, len: usize) -> Result<Vec<u8>> {
        let end = offset
            .checked_add(len as u64)
            .ok_or_else(|| Error::Invalid("range overflow".into()))?;
        if end > node.size {
            return Err(Error::Invalid(format!(
                "read of {len} bytes at {offset} exceeds node `{}` ({} bytes)",
                node.path, node.size
            )));
        }
        self.read_range(node.offset + offset, len)
    }

    /// Reads a range of the uncompressed stream, decoding only the blocks it overlaps.
    pub fn read_range(&self, start: u64, len: usize) -> Result<Vec<u8>> {
        let end = start + len as u64;
        let mut out = Vec::with_capacity(len);
        let mut idx = self
            .blocks
            .partition_point(|b| b.uncompressed_start + b.uncompressed_size as u64 <= start);
        let mut cur = start;
        while cur < end {
            let b = self
                .blocks
                .get(idx)
                .ok_or_else(|| Error::Invalid("read past end of bundle data".into()))?;
            let data = self.block(idx)?;
            let in_off = (cur - b.uncompressed_start) as usize;
            let take = ((end - cur) as usize).min(data.len() - in_off);
            out.extend_from_slice(&data[in_off..in_off + take]);
            cur += take as u64;
            idx += 1;
        }
        Ok(out)
    }

    fn block(&self, idx: usize) -> Result<Arc<Vec<u8>>> {
        if let Some(hit) = self.cache.lock().unwrap().get(idx) {
            return Ok(hit);
        }
        let b = &self.blocks[idx];
        let src = &self.backing[b.compressed_start..b.compressed_start + b.compressed_size as usize];
        let decoded = decompress(src, b.uncompressed_size as usize, b.flags as u32 & COMPRESSION_MASK)?;
        let arc = Arc::new(decoded);
        self.cache.lock().unwrap().insert(idx, arc.clone());
        Ok(arc)
    }
}

/// Compression types used by UnityFS: 0 none, 1 LZMA, 2 LZ4, 3 LZ4HC (same bitstream as LZ4).
fn decompress(src: &[u8], uncompressed_size: usize, kind: u32) -> Result<Vec<u8>> {
    let out = match kind {
        0 => src.to_vec(),
        1 => {
            // Unity stores 5 property bytes then a raw LZMA stream with no size field;
            // rebuild the standard 13-byte `.lzma` header from the size we already know.
            if src.len() < 5 {
                return Err(Error::Decompress("LZMA stream too short".into()));
            }
            let mut framed = Vec::with_capacity(src.len() + 8);
            framed.extend_from_slice(&src[..5]);
            framed.extend_from_slice(&(uncompressed_size as u64).to_le_bytes());
            framed.extend_from_slice(&src[5..]);
            let mut out = Vec::with_capacity(uncompressed_size);
            lzma_rs::lzma_decompress(&mut framed.as_slice(), &mut out)
                .map_err(|e| Error::Decompress(format!("{e:?}")))?;
            out
        }
        2 | 3 => lz4_flex::block::decompress(src, uncompressed_size)
            .map_err(|e| Error::Decompress(e.to_string()))?,
        other => return Err(Error::UnsupportedCompression(other)),
    };
    if out.len() != uncompressed_size {
        return Err(Error::Decompress(format!(
            "expected {uncompressed_size} bytes, got {}",
            out.len()
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal valid UnityFS bundle with the given blocks (each compressed with LZ4)
    /// and one node spanning all of them.
    fn build(blocks: &[&[u8]], node_name: &str) -> Vec<u8> {
        let comp: Vec<Vec<u8>> = blocks.iter().map(|b| lz4_flex::block::compress(b)).collect();

        let mut info = vec![0u8; 16];
        info.extend((blocks.len() as i32).to_be_bytes());
        for (b, c) in blocks.iter().zip(&comp) {
            info.extend((b.len() as u32).to_be_bytes());
            info.extend((c.len() as u32).to_be_bytes());
            info.extend(3u16.to_be_bytes()); // LZ4HC
        }
        let total: usize = blocks.iter().map(|b| b.len()).sum();
        info.extend(1i32.to_be_bytes());
        info.extend(0i64.to_be_bytes());
        info.extend((total as i64).to_be_bytes());
        info.extend(4u32.to_be_bytes());
        info.extend(node_name.as_bytes());
        info.push(0);
        let comp_info = lz4_flex::block::compress(&info);

        let mut out = b"UnityFS\0".to_vec();
        out.extend(7u32.to_be_bytes());
        out.extend(b"5.x.x\0");
        out.extend(b"2019.4.40f1\0");
        out.extend(0u64.to_be_bytes());
        out.extend((comp_info.len() as u32).to_be_bytes());
        out.extend((info.len() as u32).to_be_bytes());
        out.extend(3u32.to_be_bytes()); // LZ4HC blockinfo, not at end
        while out.len() % 16 != 0 {
            out.push(0);
        }
        out.extend(comp_info);
        for c in comp {
            out.extend(c);
        }
        out
    }

    #[test]
    fn roundtrip_and_cross_block_reads() {
        let a: Vec<u8> = (0..200u8).collect();
        let b: Vec<u8> = (200..=255u8).chain(0..44).collect();
        let bundle = Bundle::from_bytes(build(&[&a, &b], "CAB-test")).unwrap();
        assert_eq!(bundle.nodes().len(), 1);
        let node = bundle.node_by_name("CAB-test").unwrap().clone();
        assert!(node.is_serialized_file());

        let all: Vec<u8> = a.iter().chain(&b).copied().collect();
        assert_eq!(bundle.read_node(&node, 0, all.len()).unwrap(), all);
        // range straddling the block boundary at 200
        assert_eq!(bundle.read_node(&node, 190, 20).unwrap(), all[190..210].to_vec());
        // out-of-range reads are errors, not panics
        assert!(bundle.read_node(&node, 0, all.len() + 1).is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(Bundle::from_bytes(b"PK\x03\x04\0".to_vec()), Err(Error::BadSignature(_))));
    }
}
