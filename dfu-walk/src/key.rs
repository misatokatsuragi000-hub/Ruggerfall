//! Parsing of Daggerfall texture names into (archive, record, frame, map kind).
//!
//! ASSUMPTION: the naming scheme `{archive}_{record}-{frame}[_Normal|_Emission|_MetallicGloss]`
//! is how I remember DFU's texture replacement convention; I could not check it against the DFU
//! source from here. Verify against `TextureReplacement.cs` / `ModManager` before relying on it.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MapKind {
    Albedo,
    Normal,
    Emission,
    MetallicGloss,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureKey {
    pub archive: u32,
    pub record: u32,
    pub frame: u32,
    pub kind: MapKind,
}

impl TextureKey {
    /// Parses a (case-insensitive) texture stem such as `301_0-0` or `10035_5-2_Normal`.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        let (archive, rest) = name.split_once('_')?;
        let (core, suffix) = match rest.split_once('_') {
            Some((c, s)) => (c, Some(s)),
            None => (rest, None),
        };
        let (record, frame) = match core.split_once('-') {
            Some((r, f)) => (r.parse().ok()?, f.parse().ok()?),
            None => (core.parse().ok()?, 0),
        };
        let kind = match suffix {
            None => MapKind::Albedo,
            Some("normal") => MapKind::Normal,
            Some("emission") => MapKind::Emission,
            Some("metallicgloss") => MapKind::MetallicGloss,
            Some(_) => return None,
        };
        Some(Self { archive: archive.parse().ok()?, record, frame, kind })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_shapes() {
        let k = TextureKey::parse("301_0-0").unwrap();
        assert_eq!((k.archive, k.record, k.frame, k.kind), (301, 0, 0, MapKind::Albedo));
        let k = TextureKey::parse("10035_5-2_Normal").unwrap();
        assert_eq!((k.archive, k.record, k.frame, k.kind), (10035, 5, 2, MapKind::Normal));
        assert_eq!(TextureKey::parse("12_3").unwrap().frame, 0);
    }

    #[test]
    fn rejects_non_texture_names() {
        assert!(TextureKey::parse("textdatabase").is_none());
        assert!(TextureKey::parse("301_x-0").is_none());
        assert!(TextureKey::parse("301_0-0_weird").is_none());
    }
}
