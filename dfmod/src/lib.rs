//! Reader for Daggerfall Unity `.dfmod` files.
//!
//! A `.dfmod` is a Unity 2019.4 **AssetBundle** (UnityFS container) holding a *SerializedFile*
//! whose objects are `Texture2D`, `TextAsset` (JSON, text databases, `.dll.bytes`), `GameObject`
//! prefabs, etc. This crate parses the container and the serialized file and exposes textures
//! and raw assets; it does **not** execute the managed code that some mods embed.
//!
//! ```no_run
//! let mod_ = dfmod::ModArchive::open("my_textures.dfmod")?;
//! for (key, name) in mod_.texture_keys() {
//!     if let Some(tex) = mod_.texture(name)? {
//!         let rgba = tex.to_rgba8_top_down()?; // 4 bytes per pixel, row 0 = top
//!         println!("{key:?}: {}x{}", tex.width, tex.height);
//!     }
//! }
//! # Ok::<(), dfmod::Error>(())
//! ```

pub mod archive;
pub mod bundle;
mod common_strings;
mod dxt;
pub mod error;
pub mod key;
mod reader;
pub mod serialized;
pub mod texture;

pub use archive::{AssetEntry, ModArchive};
pub use bundle::{Bundle, Node};
pub use error::{Error, Result};
pub use key::{MapKind, TextureKey};
pub use serialized::{SerializedFile, Value};
pub use texture::{Texture2D, Texture2DArray, TextureFormat};
