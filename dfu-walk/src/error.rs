use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a UnityFS bundle (signature {0:?})")]
    BadSignature(String),
    #[error("unsupported UnityFS format version {0}")]
    UnsupportedBundleVersion(u32),
    #[error("unsupported compression type {0}")]
    UnsupportedCompression(u32),
    #[error("decompression failed: {0}")]
    Decompress(String),
    #[error("unexpected end of data at offset {offset} (wanted {wanted} bytes)")]
    Eof { offset: usize, wanted: usize },
    #[error("invalid data: {0}")]
    Invalid(String),
    #[error("unsupported serialized file version {0} (supported: 17..=21, i.e. Unity 2017.x-2021.x)")]
    UnsupportedSerializedVersion(u32),
    #[error("object with path id {0} not found")]
    ObjectNotFound(i64),
    #[error("serialized file has no type tree for class id {0}")]
    NoTypeTree(i32),
    #[error("unsupported texture format {0} ({1})")]
    UnsupportedTextureFormat(u32, &'static str),
    #[error("missing or mistyped field `{0}`")]
    MissingField(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;
