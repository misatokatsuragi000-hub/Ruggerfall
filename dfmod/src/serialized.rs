//! Unity SerializedFile (the content of a `CAB-*` node), formats 17..=21 (Unity 2017.x - 2021.x).
//! Daggerfall Unity mods are built with Unity 2019.4, i.e. format 21.
//!
//! Header (big-endian): `u32 metadata_size | u32 file_size | u32 version | u32 data_offset |
//! u8 endianness (0 = little) | 3 reserved`. Everything after the header uses the declared
//! endianness. Object *data* is at `data_offset + object.byte_start`.
//!
//! Object contents are decoded generically from the embedded **type tree** into [`Value`], so no
//! per-Unity-version struct layouts are hard-coded. (Bundles built with "Do not write type tree"
//! have no tree; those would need hand-written readers and are reported as `NoTypeTree`.)

use std::collections::HashMap;

use crate::bundle::{Bundle, Node};
use crate::common_strings::{lookup, COMMON_STRINGS};
use crate::error::{Error, Result};
use crate::reader::Reader;

/// Well-known Unity class ids.
pub mod class_id {
    pub const GAME_OBJECT: i32 = 1;
    pub const TRANSFORM: i32 = 4;
    pub const TEXTURE_2D: i32 = 28;
    pub const TEXTURE_2D_ARRAY: i32 = 187;
    pub const TEXT_ASSET: i32 = 49;
    pub const MONO_BEHAVIOUR: i32 = 114;
    pub const ASSET_BUNDLE: i32 = 142;
}

const META_FLAG_ALIGN: i32 = 0x4000;

#[derive(Debug, Clone)]
pub struct TypeNode {
    pub level: u8,
    pub type_name: String,
    pub name: String,
    pub meta_flags: i32,
}

#[derive(Debug, Clone)]
pub struct TypeTree {
    nodes: Vec<TypeNode>,
    /// `ends[i]` = index one past the last descendant of node `i`.
    ends: Vec<usize>,
}

impl TypeTree {
    fn new(nodes: Vec<TypeNode>) -> Self {
        let mut ends = vec![nodes.len(); nodes.len()];
        let mut stack: Vec<usize> = Vec::new();
        for (i, n) in nodes.iter().enumerate() {
            while let Some(&top) = stack.last() {
                if nodes[top].level >= n.level {
                    ends[top] = i;
                    stack.pop();
                } else {
                    break;
                }
            }
            stack.push(i);
        }
        Self { nodes, ends }
    }

    pub fn nodes(&self) -> &[TypeNode] {
        &self.nodes
    }

    fn children(&self, i: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut j = i + 1;
        while j < self.ends[i] {
            out.push(j);
            j = self.ends[j];
        }
        out
    }
}

#[derive(Debug, Clone)]
pub struct SerializedType {
    pub class_id: i32,
    pub tree: Option<TypeTree>,
}

#[derive(Debug, Clone)]
pub struct ObjectInfo {
    pub path_id: i64,
    /// Offset of the object's data inside the node (already includes `data_offset`).
    pub byte_start: u64,
    pub byte_size: u32,
    pub type_index: usize,
    pub class_id: i32,
}

/// A dynamically-typed value decoded from a type tree.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    /// A Unity `string` that is valid UTF-8.
    Str(String),
    /// `TypelessData`, `vector<UInt8>`, or a Unity `string` holding non-UTF-8 bytes.
    Bytes(Vec<u8>),
    Array(Vec<Value>),
    /// A struct: ordered `(field name, value)` pairs.
    Struct(Vec<(String, Value)>),
}

impl Value {
    pub fn get(&self, field: &str) -> Option<&Value> {
        match self {
            Value::Struct(f) => f.iter().find(|(n, _)| n == field).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Value::Int(v) => Some(v),
            Value::UInt(v) => i64::try_from(v).ok(),
            _ => None,
        }
    }
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Value::UInt(v) => Some(v),
            Value::Int(v) => u64::try_from(v).ok(),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Value::Float(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(v) => Some(v),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    /// Raw bytes of a `Bytes` value, or the UTF-8 bytes of a `Str`.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            Value::Str(s) => Some(s.as_bytes()),
            _ => None,
        }
    }
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct SerializedFile {
    pub version: u32,
    pub unity_version: String,
    pub little_endian: bool,
    pub types: Vec<SerializedType>,
    pub objects: Vec<ObjectInfo>,
    by_path_id: HashMap<i64, usize>,
}

impl SerializedFile {
    /// Parses the metadata of the serialized file stored in `node` (reads only the header region).
    pub fn parse(bundle: &Bundle, node: &Node) -> Result<Self> {
        let head = bundle.read_node(node, 0, 20.min(node.size as usize))?;
        let mut r = Reader::new(&head, false);
        let _metadata_size = r.u32()?;
        let _file_size = r.u32()?;
        let version = r.u32()?;
        let data_offset = r.u32()? as u64;
        if !(17..=21).contains(&version) {
            return Err(Error::UnsupportedSerializedVersion(version));
        }
        // Metadata (types + object table + ...) lives entirely before `data_offset`.
        let meta = bundle.read_node(node, 0, data_offset.min(node.size) as usize)?;
        Self::parse_metadata(&meta)
    }

    fn parse_metadata(meta: &[u8]) -> Result<Self> {
        let mut r = Reader::new(meta, false);
        let _metadata_size = r.u32()?;
        let _file_size = r.u32()?;
        let version = r.u32()?;
        let data_offset = r.u32()? as u64;
        if !(17..=21).contains(&version) {
            return Err(Error::UnsupportedSerializedVersion(version));
        }
        let endian = r.u8()?;
        r.skip(3)?;
        let little_endian = endian == 0;
        r.set_little_endian(little_endian);

        let unity_version = r.cstr()?;
        let _target_platform = r.i32()?;
        let has_type_tree = r.u8()? != 0;

        let ntypes = r.i32()?;
        if ntypes < 0 {
            return Err(Error::Invalid("negative type count".into()));
        }
        let mut types = Vec::with_capacity(ntypes as usize);
        for _ in 0..ntypes {
            let class_id = r.i32()?;
            let _is_stripped = r.u8()?;
            let _script_type_index = r.i16()?;
            if class_id == class_id::MONO_BEHAVIOUR {
                r.skip(16)?; // script id
            }
            r.skip(16)?; // old type hash
            let tree = if has_type_tree {
                let tree = Self::read_type_tree(&mut r, version)?;
                if version >= 21 {
                    let ndeps = r.i32()?;
                    if ndeps < 0 {
                        return Err(Error::Invalid("negative type dependency count".into()));
                    }
                    r.skip(ndeps as usize * 4)?;
                }
                Some(tree)
            } else {
                None
            };
            types.push(SerializedType { class_id, tree });
        }

        let nobjects = r.i32()?;
        if nobjects < 0 {
            return Err(Error::Invalid("negative object count".into()));
        }
        let mut objects = Vec::with_capacity(nobjects as usize);
        let mut by_path_id = HashMap::with_capacity(nobjects as usize);
        for i in 0..nobjects as usize {
            r.align(4)?;
            let path_id = r.i64()?;
            let byte_start = r.u32()? as u64 + data_offset;
            let byte_size = r.u32()?;
            let type_index = r.i32()?;
            let ty = types
                .get(usize::try_from(type_index).map_err(|_| Error::Invalid("negative type index".into()))?)
                .ok_or_else(|| Error::Invalid(format!("object type index {type_index} out of range")))?;
            objects.push(ObjectInfo {
                path_id,
                byte_start,
                byte_size,
                type_index: type_index as usize,
                class_id: ty.class_id,
            });
            by_path_id.insert(path_id, i);
        }
        // Script types, externals and ref types follow; none are needed for asset extraction.

        Ok(Self { version, unity_version, little_endian, types, objects, by_path_id })
    }

    fn read_type_tree(r: &mut Reader, version: u32) -> Result<TypeTree> {
        let n = r.i32()?;
        let string_size = r.i32()?;
        if n < 0 || string_size < 0 {
            return Err(Error::Invalid("negative type tree size".into()));
        }
        struct Raw {
            level: u8,
            type_off: u32,
            name_off: u32,
            meta_flags: i32,
        }
        let mut raw = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let _node_version = r.u16()?;
            let level = r.u8()?;
            let _type_flags = r.u8()?;
            let type_off = r.u32()?;
            let name_off = r.u32()?;
            let _byte_size = r.i32()?;
            let _index = r.i32()?;
            let meta_flags = r.i32()?;
            if version >= 19 {
                r.skip(8)?; // ref type hash
            }
            raw.push(Raw { level, type_off, name_off, meta_flags });
        }
        let local = r.take(string_size as usize)?;
        let resolve = |off: u32| -> Result<String> {
            let s = if off & 0x8000_0000 != 0 {
                lookup(COMMON_STRINGS, (off & 0x7fff_ffff) as usize)
            } else {
                lookup(local, off as usize)
            };
            s.map(str::to_owned)
                .ok_or_else(|| Error::Invalid(format!("bad type tree string offset {off:#x}")))
        };
        let nodes = raw
            .iter()
            .map(|n| -> Result<TypeNode> {
                Ok(TypeNode {
                    level: n.level,
                    type_name: resolve(n.type_off)?,
                    name: resolve(n.name_off)?,
                    meta_flags: n.meta_flags,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if nodes.is_empty() {
            return Err(Error::Invalid("empty type tree".into()));
        }
        Ok(TypeTree::new(nodes))
    }

    pub fn object(&self, path_id: i64) -> Option<&ObjectInfo> {
        self.by_path_id.get(&path_id).map(|&i| &self.objects[i])
    }

    pub fn objects_of_class(&self, class_id: i32) -> impl Iterator<Item = &ObjectInfo> {
        self.objects.iter().filter(move |o| o.class_id == class_id)
    }

    /// Decodes an object into a [`Value`] using its type tree.
    pub fn read_object(&self, bundle: &Bundle, node: &Node, obj: &ObjectInfo) -> Result<Value> {
        let tree = self.types[obj.type_index]
            .tree
            .as_ref()
            .ok_or(Error::NoTypeTree(obj.class_id))?;
        let bytes = bundle.read_node(node, obj.byte_start, obj.byte_size as usize)?;
        let mut r = Reader::new(&bytes, self.little_endian);
        read_value(tree, 0, &mut r)
    }
}

fn is_byte_type(t: &str) -> bool {
    matches!(t, "UInt8" | "char")
}

fn read_value(tree: &TypeTree, i: usize, r: &mut Reader) -> Result<Value> {
    let node = &tree.nodes[i];
    let mut align = node.meta_flags & META_FLAG_ALIGN != 0;
    let v = match node.type_name.as_str() {
        "bool" => Value::Bool(r.u8()? != 0),
        "SInt8" => Value::Int(r.u8()? as i8 as i64),
        "UInt8" | "char" => Value::UInt(r.u8()? as u64),
        "short" | "SInt16" => Value::Int(r.i16()? as i64),
        "unsigned short" | "UInt16" => Value::UInt(r.u16()? as u64),
        "int" | "SInt32" => Value::Int(r.i32()? as i64),
        "unsigned int" | "UInt32" | "Type*" => Value::UInt(r.u32()? as u64),
        "long long" | "SInt64" => Value::Int(r.i64()?),
        "unsigned long long" | "UInt64" | "FileSize" => Value::UInt(r.u64()?),
        "float" => Value::Float(r.f32()? as f64),
        "double" => Value::Float(r.f64()?),
        "string" => {
            let len = r.u32()? as usize;
            let bytes = r.take(len)?;
            align = true;
            match String::from_utf8(bytes.to_vec()) {
                Ok(s) => Value::Str(s),
                Err(e) => Value::Bytes(e.into_bytes()),
            }
        }
        "TypelessData" => {
            let len = r.u32()? as usize;
            let bytes = r.take(len)?;
            align = true;
            Value::Bytes(bytes.to_vec())
        }
        "Array" => read_array(tree, i, r)?,
        _ => {
            let kids = tree.children(i);
            if kids.len() == 1 && tree.nodes[kids[0]].type_name == "Array" {
                // vector / map / set / list: the real payload is the single `Array` child.
                read_array(tree, kids[0], r)?
            } else {
                let mut fields = Vec::with_capacity(kids.len());
                for k in kids {
                    fields.push((tree.nodes[k].name.clone(), read_value(tree, k, r)?));
                }
                Value::Struct(fields)
            }
        }
    };
    if align {
        r.align(4)?;
    }
    Ok(v)
}

/// `Array` nodes have exactly two children: `size` (int) and `data` (the element type).
fn read_array(tree: &TypeTree, arr: usize, r: &mut Reader) -> Result<Value> {
    let kids = tree.children(arr);
    if kids.len() != 2 {
        return Err(Error::Invalid("malformed Array node in type tree".into()));
    }
    let n = r.i32()?;
    if n < 0 {
        return Err(Error::Invalid("negative array length".into()));
    }
    let n = n as usize;
    let elem = kids[1];
    let v = if is_byte_type(&tree.nodes[elem].type_name) {
        Value::Bytes(r.take(n)?.to_vec())
    } else {
        // Every element occupies at least one byte, so a count above the remaining length means
        // corrupt data; reject it instead of allocating a huge Vec. (Arrays of zero-sized
        // structs would be rejected too; Unity does not emit those in practice.)
        if n > r.remaining() {
            return Err(Error::Invalid(format!("array length {n} exceeds remaining data")));
        }
        let mut out = Vec::with_capacity(n.min(r.remaining()));
        for _ in 0..n {
            out.push(read_value(tree, elem, r)?);
        }
        Value::Array(out)
    };
    if tree.nodes[arr].meta_flags & META_FLAG_ALIGN != 0 {
        r.align(4)?;
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(level: u8, ty: &str, name: &str, flags: i32) -> TypeNode {
        TypeNode { level, type_name: ty.into(), name: name.into(), meta_flags: flags }
    }

    #[test]
    fn subtree_ends() {
        let t = TypeTree::new(vec![
            node(0, "Root", "Base", 0),
            node(1, "int", "a", 0),
            node(1, "Pair", "b", 0),
            node(2, "int", "x", 0),
            node(1, "int", "c", 0),
        ]);
        assert_eq!(t.ends, vec![5, 2, 4, 4, 5]);
        assert_eq!(t.children(0), vec![1, 2, 4]);
        assert_eq!(t.children(2), vec![3]);
    }

    #[test]
    fn reads_struct_with_string_and_array() {
        // struct { int id; string name; vector<int> xs }
        let tree = TypeTree::new(vec![
            node(0, "Thing", "Base", 0),
            node(1, "int", "id", 0),
            node(1, "string", "name", META_FLAG_ALIGN),
            node(2, "Array", "Array", META_FLAG_ALIGN),
            node(1, "vector", "xs", 0),
            node(2, "Array", "Array", 0),
            node(3, "int", "size", 0),
            node(3, "int", "data", 0),
        ]);
        // Node 3 is the string's internal char array; `string` is read as a primitive, so the
        // reader must never descend into it.
        let mut b = Vec::new();
        b.extend(7i32.to_le_bytes());
        b.extend(3u32.to_le_bytes());
        b.extend(b"abc");
        b.push(0); // pad to 4
        b.extend(2i32.to_le_bytes());
        b.extend(10i32.to_le_bytes());
        b.extend(20i32.to_le_bytes());
        let mut r = Reader::new(&b, true);
        let id = read_value(&tree, 1, &mut r).unwrap();
        let name = read_value(&tree, 2, &mut r).unwrap();
        let xs = read_value(&tree, 4, &mut r).unwrap();
        assert_eq!(id, Value::Int(7));
        assert_eq!(name, Value::Str("abc".into()));
        assert_eq!(xs, Value::Array(vec![Value::Int(10), Value::Int(20)]));
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn corrupt_array_length_is_rejected() {
        let tree = TypeTree::new(vec![
            node(0, "vector", "v", 0),
            node(1, "Array", "Array", 0),
            node(2, "int", "size", 0),
            node(2, "int", "data", 0),
        ]);
        let b = i32::MAX.to_le_bytes();
        let mut r = Reader::new(&b, true);
        assert!(read_value(&tree, 0, &mut r).is_err());
    }
}
