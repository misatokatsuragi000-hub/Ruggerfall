//! Selezione delle texture per clima e stagione.
//! Porting di ClimateSwaps.cs e di MapsFile.GetWorldClimateSettings
//! (Daggerfall Unity, MIT). Le stranezze del codice originale sono mantenute
//! di proposito, così i numeri di archivio coincidono con quelli di DFU.

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClimateBase {
    Desert,
    Mountain,
    Temperate,
    Swamp,
}

impl ClimateBase {
    /// Valore aggiunto al numero del set (ClimateBaseType in DFU).
    pub fn offset(self) -> i32 {
        match self {
            ClimateBase::Desert => 0,
            ClimateBase::Mountain => 100,
            ClimateBase::Temperate => 300,
            ClimateBase::Swamp => 400,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Season {
    Summer,
    Winter,
    Rain,
}

const WINTER_OFFSET: i32 = 1;
const RAIN_OFFSET: i32 = 2;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TextureGroup {
    None,
    Nature,
    Terrain,
    Exterior,
    Interior,
}

/// Informazioni sul set di texture di un archivio. `set == None` significa
/// "non dipende dal clima".
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ClimateTextureInfo {
    pub set: Option<i32>,
    pub group: TextureGroup,
    pub supports_winter: bool,
    pub supports_rain: bool,
}

const SET_TERRAIN: i32 = 2;
const SET_CASTLE: i32 = 9;
const SET_MAGES_GUILD: i32 = 35;
const SET_MARBLE_FLOORS: i32 = 41;
const SET_DOORS: i32 = 74;
/// Set esterni con variante invernale.
const EXTERIOR_WINTER_SETS: [i32; 18] = [
    7, 9, 12, 14, 17, 26, 29, 35, 38, 42, 58, 61, 64, 69, 79, 80, 82, 83,
];
const INTERIOR_SETS: [i32; 20] = [
    11, 16, 19, 20, 22, 23, 24, 25, 28, 37, 40, 41, 44, 45, 47, 48, 60, 63, 66, 68,
];
const NATURE_NO_SNOW: [i32; 4] = [500, 501, 502, 503];
const NATURE_WITH_SNOW: [i32; 4] = [504, 506, 508, 510];

pub fn climate_texture_info(archive: i32) -> ClimateTextureInfo {
    let mut ci = ClimateTextureInfo {
        set: None,
        group: TextureGroup::None,
        supports_winter: false,
        supports_rain: false,
    };

    if archive > 499 {
        if NATURE_NO_SNOW.contains(&archive) {
            ci.set = Some(archive);
            ci.group = TextureGroup::Nature;
        } else if NATURE_WITH_SNOW.contains(&archive) {
            ci.set = Some(archive);
            ci.group = TextureGroup::Nature;
            ci.supports_winter = true;
        }
        return ci;
    }

    let set = archive - (archive / 100) * 100;
    if set == SET_TERRAIN {
        ci.set = Some(set);
        ci.group = TextureGroup::Terrain;
        ci.supports_winter = true;
        ci.supports_rain = true;
    } else if EXTERIOR_WINTER_SETS.contains(&set) {
        ci.set = Some(set);
        ci.group = TextureGroup::Exterior;
        ci.supports_winter = true;
    } else if set == SET_DOORS {
        // Riconosciuto come set, ma senza gruppo e senza inverno.
        ci.set = Some(set);
    } else if INTERIOR_SETS.contains(&set) {
        ci.set = Some(set);
        ci.group = TextureGroup::Interior;
    }
    ci
}

/// Converte un archivio di base nell'archivio per clima e stagione.
pub fn apply_climate(archive: i32, record: i32, climate: ClimateBase, season: Season) -> i32 {
    // Non scambiare le texture della porta 3, come nel gioco classico.
    if archive % 100 == 74 && record == 3 {
        return archive;
    }

    let mut ci = climate_texture_info(archive);
    let Some(set) = ci.set else {
        return archive;
    };

    // Mancano le texture dei pavimenti di marmo nella palude.
    if climate == ClimateBase::Swamp && set == SET_MARBLE_FLOORS {
        return archive;
    }

    // Nessun inverno nel deserto.
    if climate == ClimateBase::Desert {
        ci.supports_winter = false;
    }

    // Precedenza identica a DFU: (palude && gilda dei maghi) || (castello && record > 3).
    if (climate == ClimateBase::Swamp && set == SET_MAGES_GUILD)
        || (set == SET_CASTLE && record > 3)
    {
        ci.supports_winter = false;
    }

    // Archivi senza texture invernali.
    if (archive == 82 && record > 1) || archive == 77 {
        ci.supports_winter = false;
    }

    // Archivi con variante invernale ma specifici di un clima.
    let suppress_climate_index = matches!(archive, 75 | 76 | 77 | 79 | 80 | 82 | 83);

    if archive < 500 && !suppress_climate_index {
        let mut index = climate.offset() + set;
        if season == Season::Winter && ci.supports_winter {
            index += WINTER_OFFSET;
        } else if season == Season::Rain && ci.supports_rain {
            index += RAIN_OFFSET;
        }
        index
    } else {
        let mut index = archive;
        if season == Season::Winter && ci.supports_winter {
            index += WINTER_OFFSET;
        }
        index
    }
}

/// Valori di CLIMATE.PAK (223..=232).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ClimateSettings {
    pub world_climate: i32,
    pub climate_type: ClimateBase,
    pub ground_archive: i32,
    pub nature_archive: i32,
    pub sky_base: i32,
}

pub fn world_climate_settings(world_climate: i32) -> ClimateSettings {
    let (climate_type, ground_archive, nature_archive, sky_base) = match world_climate {
        223 => (ClimateBase::Swamp, 402, 504, 24),    // oceano
        224 | 225 => (ClimateBase::Desert, 2, 503, 8), // deserto
        226 => (ClimateBase::Mountain, 102, 510, 0),   // montagna
        227 => (ClimateBase::Swamp, 402, 500, 24),     // foresta pluviale
        228 => (ClimateBase::Swamp, 402, 502, 24),     // palude
        229 => (ClimateBase::Desert, 2, 501, 24),      // sub-tropicale
        230 => (ClimateBase::Temperate, 102, 506, 16), // boschi di montagna
        231 => (ClimateBase::Temperate, 302, 504, 16), // boschi
        232 => (ClimateBase::Temperate, 302, 508, 16), // boschi infestati
        _ => (ClimateBase::Temperate, 302, 504, 16),
    };
    ClimateSettings {
        world_climate,
        climate_type,
        ground_archive,
        nature_archive,
        sky_base,
    }
}

/// Archivio delle tile del terreno; d'inverno si passa alle texture con la neve
/// (+1), tranne nel deserto.
pub fn ground_archive(world_climate: i32, winter: bool) -> i32 {
    let s = world_climate_settings(world_climate);
    if winter && s.climate_type != ClimateBase::Desert {
        s.ground_archive + 1
    } else {
        s.ground_archive
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ClimateBase::*;
    use Season::*;

    #[test]
    fn plain_climate_offset_and_winter() {
        assert_eq!(apply_climate(12, 0, Temperate, Summer), 312);
        assert_eq!(apply_climate(12, 0, Temperate, Winter), 313);
        assert_eq!(apply_climate(12, 0, Mountain, Summer), 112);
        assert_eq!(apply_climate(12, 0, Swamp, Winter), 413);
        assert_eq!(apply_climate(12, 0, Desert, Summer), 12);
    }

    #[test]
    fn desert_never_gets_winter() {
        assert_eq!(apply_climate(12, 0, Desert, Winter), 12);
        assert_eq!(apply_climate(2, 0, Desert, Winter), 2);
    }

    #[test]
    fn terrain_set_supports_winter_and_rain() {
        assert_eq!(apply_climate(2, 0, Temperate, Summer), 302);
        assert_eq!(apply_climate(2, 0, Temperate, Winter), 303);
        assert_eq!(apply_climate(2, 0, Temperate, Rain), 304);
        // Le altre texture esterne non hanno variante pioggia.
        assert_eq!(apply_climate(12, 0, Temperate, Rain), 312);
    }

    #[test]
    fn doors_and_door_three_exception() {
        assert_eq!(apply_climate(74, 3, Temperate, Summer), 74);
        assert_eq!(apply_climate(74, 0, Temperate, Summer), 374);
        assert_eq!(apply_climate(74, 0, Temperate, Winter), 374);
    }

    #[test]
    fn non_climate_and_interior_sets() {
        assert_eq!(apply_climate(46, 0, Temperate, Summer), 46); // Misc: nessun set
        assert_eq!(apply_climate(22, 0, Temperate, Winter), 322); // interno, mai inverno
        assert_eq!(apply_climate(41, 0, Swamp, Summer), 41); // marmo nella palude
        assert_eq!(apply_climate(41, 0, Temperate, Summer), 341);
    }

    #[test]
    fn quirks_castle_mages_guild_and_suppressed_archives() {
        assert_eq!(apply_climate(9, 3, Temperate, Winter), 310);
        assert_eq!(apply_climate(9, 4, Temperate, Winter), 309); // record > 3: niente inverno
        assert_eq!(apply_climate(35, 0, Swamp, Winter), 435); // palude: niente inverno
        assert_eq!(apply_climate(35, 0, Temperate, Winter), 336);
        // Archivi specifici di clima: l'indice parte dall'archivio stesso.
        assert_eq!(apply_climate(79, 0, Temperate, Summer), 79);
        assert_eq!(apply_climate(79, 0, Temperate, Winter), 80);
        assert_eq!(apply_climate(82, 0, Temperate, Winter), 83);
        assert_eq!(apply_climate(82, 2, Temperate, Winter), 82);
        assert_eq!(apply_climate(77, 0, Temperate, Winter), 77);
    }

    #[test]
    fn nature_archives() {
        assert_eq!(apply_climate(504, 0, Temperate, Winter), 505);
        assert_eq!(apply_climate(504, 0, Temperate, Summer), 504);
        assert_eq!(apply_climate(500, 0, Temperate, Winter), 500);
        assert_eq!(apply_climate(505, 0, Temperate, Winter), 505);
        assert_eq!(apply_climate(999, 0, Temperate, Winter), 999);
    }

    #[test]
    fn texture_info_groups() {
        assert_eq!(climate_texture_info(302).group, TextureGroup::Terrain);
        assert_eq!(climate_texture_info(312).group, TextureGroup::Exterior);
        assert_eq!(climate_texture_info(322).group, TextureGroup::Interior);
        assert_eq!(climate_texture_info(510).group, TextureGroup::Nature);
        assert_eq!(climate_texture_info(46).set, None);
        assert!(!climate_texture_info(74).supports_winter);
    }

    #[test]
    fn world_climate_table() {
        let s = world_climate_settings(231);
        assert_eq!((s.climate_type, s.ground_archive, s.nature_archive), (Temperate, 302, 504));
        let s = world_climate_settings(224);
        assert_eq!((s.climate_type, s.ground_archive, s.nature_archive), (Desert, 2, 503));
        let s = world_climate_settings(230);
        assert_eq!((s.climate_type, s.ground_archive, s.nature_archive), (Temperate, 102, 506));
        let s = world_climate_settings(223);
        assert_eq!((s.climate_type, s.ground_archive, s.sky_base), (Swamp, 402, 24));
        assert_eq!(world_climate_settings(999).ground_archive, 302);
    }

    #[test]
    fn ground_archive_winter_rules() {
        assert_eq!(ground_archive(231, false), 302);
        assert_eq!(ground_archive(231, true), 303);
        assert_eq!(ground_archive(226, true), 103);
        assert_eq!(ground_archive(228, true), 403);
        // Deserto e sub-tropicale (tipo Desert): niente neve.
        assert_eq!(ground_archive(224, true), 2);
        assert_eq!(ground_archive(229, true), 2);
    }
}
