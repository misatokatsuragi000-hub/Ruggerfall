//! Catalogo dei flat con le descrizioni di `FLATS.CFG`.
//!
//! ## Formato di FLATS.CFG (identico a FlatsFile.cs di Daggerfall Unity)
//!
//! Il file è composto da blocchi di **6 righe** separati da righe vuote:
//!
//! ```text
//! 175 0
//! beautiful maiden
//! ?2
//! -2
//!
//! 410
//! ```
//!
//! - riga 0: `arch record`
//! - riga 1: caption (descrizione)
//! - riga 2: gender ("1" = M, "2" = F, "?2"/"-1" = ambiguo)
//! - riga 3: unk2 / categoria (0 = personaggio unico, >0 = NPC civile, <0 = mostro/effetto)
//! - riga 4: unk3 (spesso vuoto)
//! - riga 5: face index
//!
//! ## Cosa conserva il catalogo
//!
//! Per ogni `(arch, record)` conserva **la caption migliore fra tutte le facce**,
//! con questa priorità:
//!
//! 1. **Categoria 0** (nome proprio di un personaggio unico) — vince sempre.
//! 2. **Categoria positiva** (NPC civile): `guard`, `merchant`, `cowled old man`.
//! 3. **Categoria negativa** (mostro o effetto): accettabile solo se non c'è di meglio.
use bevy::prelude::Resource;
use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct FlatInfo {
    pub archive: u16,
    pub record: u8,
    /// Face con cui è stata presa la caption.
    pub face: u16,
    /// Genere: 1 maschile, 2 femminile, 0 ambiguo.
    pub gender: u8,
    /// Categoria: 0 = personaggio unico, positivo = NPC civile, negativo = mostro/effetto.
    pub category: i32,
    /// Descrizione (`beautiful maiden`, `cowled old man`, `King of Worms`).
    pub caption: String,
}

impl FlatInfo {
    pub fn is_unique(&self) -> bool {
        self.category == 0 && !self.caption.is_empty()
    }

    fn priority(category: i32) -> u8 {
        match category {
            0 => 0,
            c if c > 0 => 1,
            _ => 2,
        }
    }

    fn priority_of(&self) -> u8 {
        Self::priority(self.category)
    }
}

#[derive(Resource, Default, Clone, Debug)]
pub struct FlatCatalog {
    by_flat: HashMap<(u16, u8), FlatInfo>,
}

impl FlatCatalog {
    pub fn load(arena2_dir: impl AsRef<Path>) -> Self {
        let dir = arena2_dir.as_ref();
        let Some(path) = super::find_file_ci(dir, "FLATS.CFG") else {
            return Self::default();
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return Self::default();
        };
        Self::parse(&bytes)
    }

    /// Parserizza il formato a blocchi di 6 righe di FLATS.CFG.
    pub fn parse(bytes: &[u8]) -> Self {
        let text = String::from_utf8_lossy(bytes);
        // Rimuovi BOM UTF-8 se presente
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let mut by_flat: HashMap<(u16, u8), FlatInfo> = HashMap::new();

        let mut block: Vec<&str> = Vec::new();
        for line in text.lines().map(str::trim).chain(std::iter::once("")) {
            if line.is_empty() {
                if block.len() >= 6 {
                    // riga 0: "arch record"
                    let mut parts = block[0].split_whitespace();
                    let arch_ok = parts.next().and_then(|s| s.parse::<u16>().ok());
                    let rec_ok = parts.next().and_then(|s| s.parse::<u8>().ok());
                    if let (Some(archive), Some(record)) = (arch_ok, rec_ok) {
                        let caption = block[1].to_string();
                        if !caption.is_empty() {
                            let gender = match block[2] {
                                "1" => 1,
                                "2" => 2,
                                _ => 0,
                            };
                            let category: i32 = block[3].parse().unwrap_or(0);
                            let face: u16 = block[5].parse().unwrap_or(0);

                            let candidate = FlatInfo {
                                archive,
                                record,
                                face,
                                gender,
                                category,
                                caption,
                            };

                            match by_flat.get(&(archive, record)) {
                                Some(existing) if existing.priority_of() <= candidate.priority_of() => {}
                                _ => {
                                    by_flat.insert((archive, record), candidate);
                                }
                            }
                        }
                    }
                }
                block.clear();
                continue;
            }
            block.push(line);
            if block.len() > 6 {
                block.clear();
            }
        }
        Self { by_flat }
    }

    pub fn get(&self, archive: u16, record: u8) -> Option<&FlatInfo> {
        self.by_flat.get(&(archive, record))
    }

    pub fn len(&self) -> usize {
        self.by_flat.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_flat.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Estratto nel formato REALE a blocchi di 6 righe.
    const SAMPLE: &[u8] = b"175 0\nbeautiful maiden\n?2\n-2\n\n410\n\n\
176 0\ndark-cloaked man\n1\n22\n\n360\n\n\
177 0\ncowled old man\n1\n11\n\n479\n\n\
178 4\nman with glowing eyes\n1\n24\n\n482\n\n\
178 4\nKing of Worms\n1\n0\n\n0\n\n\
180 0\ndark-haired woman in red\n2\n16\n\n371\n\n\
180 0\nLady Bridwell\n2\n0\n\n0\n\n\
182 21\nold beggar\n1\n-3\n\n440\n\n\
182 59\nblacksmith\n1\n2\n\n389\n\n\
183 2\nguard\n1\n16\n\n403\n\n\
183 19\nmajestic orc\n1\n-2\n\n410\n\n\
183 19\nGortwog\n1\n0\n\n0\n\n\
334 19\nGothryd\n1\n0\n\n0\n";

    #[test]
    fn captions_are_parsed_from_6_line_blocks() {
        let c = FlatCatalog::parse(SAMPLE);
        assert_eq!(c.get(175, 0).unwrap().caption, "beautiful maiden");
        assert_eq!(c.get(175, 0).unwrap().face, 410);
        assert_eq!(c.get(176, 0).unwrap().caption, "dark-cloaked man");
        assert_eq!(c.get(177, 0).unwrap().caption, "cowled old man");
    }

    #[test]
    fn unique_name_wins_over_generic_caption() {
        let c = FlatCatalog::parse(SAMPLE);
        let info = c.get(178, 4).unwrap();
        assert_eq!(info.caption, "King of Worms");
        assert_eq!(info.face, 0);
        assert!(info.is_unique());

        assert_eq!(c.get(180, 0).unwrap().caption, "Lady Bridwell");
        assert!(c.get(180, 0).unwrap().is_unique());

        assert_eq!(c.get(183, 19).unwrap().caption, "Gortwog");
        assert!(c.get(183, 19).unwrap().is_unique());
    }

    #[test]
    fn positive_category_entries() {
        let c = FlatCatalog::parse(SAMPLE);
        assert_eq!(c.get(183, 2).unwrap().caption, "guard");
        assert_eq!(c.get(183, 2).unwrap().category, 16);
        assert_eq!(c.get(182, 21).unwrap().caption, "old beggar");
        assert_eq!(c.get(182, 21).unwrap().category, -3);
    }

    #[test]
    fn gender_parsing() {
        let c = FlatCatalog::parse(SAMPLE);
        assert_eq!(c.get(176, 0).unwrap().gender, 1);
        assert_eq!(c.get(180, 0).unwrap().gender, 2);
        assert_eq!(c.get(175, 0).unwrap().gender, 0); // "?2"
        assert_eq!(c.get(334, 19).unwrap().gender, 1);
    }

    #[test]
    fn count_is_correct() {
        let c = FlatCatalog::parse(SAMPLE);
        // 175:0, 176:0, 177:0, 178:4, 180:0, 182:21, 182:59, 183:2, 183:19, 334:19
        assert_eq!(c.len(), 10);
    }

    #[test]
    fn missing_file_gives_empty_catalog() {
        let c = FlatCatalog::load("/questo/percorso/non/esiste");
        assert!(c.is_empty());
    }

    #[test]
    fn malformed_blocks_are_skipped() {
        let weird = b"foo bar\ncaption\n1\n0\n\n0\n\n175 1\ndark warrior\n2\n-2\n\n410\n\n999999 3\nout of range\n2\n-2\n\n410\n";
        let c = FlatCatalog::parse(weird);
        assert_eq!(c.len(), 1);
        assert!(c.get(175, 1).is_some());
    }

    #[test]
    fn first_caption_wins_at_equal_priority() {
        let two = b"182 0\nfirst caption\n1\n16\n\n360\n\n182 0\nsecond caption\n1\n17\n\n361\n";
        let c = FlatCatalog::parse(two);
        assert_eq!(c.get(182, 0).unwrap().caption, "first caption");
    }
}
