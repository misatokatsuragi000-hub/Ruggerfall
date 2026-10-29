//! Terreno e città di Daggerfall in Bevy con shader WGSL,
//! mod HD di D.R.E.A.M., dungeon procedurali (RDB), mappa interattiva (tasto M),
//! mappa locale (tasto L), vegetazione 3D e colonna sonora originale.
//!
//! Comandi:
//!   clic = cattura il mouse
//!   Esc = rilascia, chiude i pannelli
//!   WASD = cammina, Maiusc = corri, Spazio = salta
//!   F = volo (Spazio/Ctrl su e giù, Maiusc = velocissimo)
//!   T = ferma/riprendi il tempo, "," e "." = un'ora indietro/avanti
//!   M = mappa dell'Iliac Bay
//!   L = mappa locale della città (taverne verdi, negozi rossi, gilde blu)
//!   G = attiva/disattiva l'erba
//!   E / U = entra nel locale/dungeon / esci vicino alla porta
//!   Backspace = esci sempre da dungeon/locali
//!   C = equipaggiamento
//!   Z = sguaina/rinfodera arma
//!   X = intensità torcia
mod torch;
mod data;
mod equipment;
mod grass;
mod fps_weapon;
mod dungeon_enemies;
mod interior;
mod local_map;
mod dialog;
mod map;
mod mod_textures;
mod music;
mod tuning_menu;
mod music_zones;
mod player;
mod savegame;
mod quests;
mod quest_llm;
mod dfquest; // ← sostituisce quest_script
mod sky;
mod terrain_material;
mod tiles;
mod trees;
mod tts;
mod wall_textures;
mod world;
mod compass;
mod vitals;
mod dungeon;
mod relief_post;
mod custom_cursor;
use crate::data::blocks::RmbBlock;
use crate::data::bsa::{BsaArchive, BsaDirectory};
use crate::data::maps::{DFLocation, MapsReader};
use crate::data::terrain_mesh::{terrain_height, CELL_SIZE, MAP_PIXEL_SIZE};
use crate::data::{find_file_ci, generate_default_samples, PakFile, Palette, WoodsFile};
use bevy::pbr::{DistanceFog, FogFalloff, MaterialPlugin};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use mod_textures::ModTexturesPlugin;
use player::{Player, EYE_HEIGHT};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use terrain_material::TerrainMaterial;
use tiles::*;
use trees::NaturePool;
use world::*;
// =============================================================================
// Bridge Bevy ↔ dfquest
// =============================================================================
/// Wrapper `Resource` attorno a `dfquest::Library`.
#[derive(Resource, Default)]
pub struct QuestLibRes(pub dfquest::Library);
/// Wrapper `Resource` attorno a `dfquest::QuestStates`.
#[derive(Resource, Default)]
pub struct QuestStatesRes(pub dfquest::QuestStates);
const USAGE: &str = "uso: dfu-walk <cartella ARENA2> [--mods <cartella>] [--textures <cartella>]
[--pixel X Y] [--radius N] [--winter] [--hour H] [--day-minutes M] [--outside]";
const SKY: Color = Color::srgb(0.55, 0.72, 0.92);
// =============================================================================
// Risorse di avvio
// =============================================================================
#[derive(Resource, Clone, Copy)]
pub struct StartDungeonConfig {
    pub start_in_dungeon: bool,
}
struct Args {
    dir: PathBuf,
    mods_dir: PathBuf,
    textures_dir: PathBuf,
    pixel: Option<(i32, i32)>,
    radius: i32,
    winter: bool,
    hour: f32,
    day_minutes: f32,
    start_dungeon: bool,
}
#[derive(Resource)]
struct StartInfo {
    mx: i32,
    my: i32,
}
#[derive(Resource)]
struct RadiusSetting(i32);
// =============================================================================
// Utility di parsing
// =============================================================================
fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("errore: {msg}");
    std::process::exit(1)
}
fn next_value<T: std::str::FromStr>(it: &mut impl Iterator<Item = String>, what: &str) -> T {
    it.next()
    .and_then(|v| v.parse().ok())
    .unwrap_or_else(|| fail(format!("{what} vuole un numero\n{USAGE}")))
}
fn parse_args() -> Args {
    let mut it = std::env::args().skip(1);
    let dir = it.next().unwrap_or_else(|| fail(USAGE));
    let default_dream_mods = PathBuf::from(
        "/mnt/1CE16DBE48849731/Daggerfall/DaggerfallUnity_Data/StreamingAssets/Mods",
    );
    let default_mods = if default_dream_mods.exists() {
        default_dream_mods
    } else {
        PathBuf::from("mods")
    };
    let default_dream_textures = PathBuf::from(
        "/mnt/1CE16DBE48849731/Daggerfall/DaggerfallUnity_Data/StreamingAssets/Textures",
    );
    let default_textures = if default_dream_textures.exists() {
        default_dream_textures
    } else {
        PathBuf::from("textures")
    };
    let mut args = Args {
        dir: PathBuf::from(dir),
        mods_dir: default_mods,
        textures_dir: default_textures,
        pixel: None,
        radius: 2,
        winter: false,
        hour: 10.5,
        day_minutes: 24.0,
        start_dungeon: true,
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--outside" => args.start_dungeon = false,
            "--mods" => {
                let m = it
                .next()
                .unwrap_or_else(|| fail(format!("--mods vuole un percorso\n{USAGE}")));
                args.mods_dir = PathBuf::from(m);
            }
            "--textures" => {
                let m = it
                .next()
                .unwrap_or_else(|| fail(format!("--textures vuole un percorso\n{USAGE}")));
                args.textures_dir = PathBuf::from(m);
            }
            "--pixel" => {
                let x: i32 = next_value(&mut it, "--pixel");
                let y: i32 = next_value(&mut it, "--pixel");
                args.pixel = Some((x, y));
            }
            "--radius" => args.radius = next_value::<i32>(&mut it, "--radius").clamp(1, 8),
            "--winter" => args.winter = true,
            "--hour" => args.hour = next_value(&mut it, "--hour"),
            "--day-minutes" => {
                args.day_minutes = next_value::<f32>(&mut it, "--day-minutes").max(0.0)
            }
            other => fail(format!("opzione sconosciuta: {other}\n{USAGE}")),
        }
    }
    args
}
// =============================================================================
// Caricamento dati
// =============================================================================
fn load_data(args: &Args) -> GameData {
    let open = |name: &str| {
        find_file_ci(&args.dir, name)
        .unwrap_or_else(|| fail(format!("{name} non trovato in {}", args.dir.display())))
    };
    let read_file = |name: &str| {
        let p = open(name);
        std::fs::read(&p)
        .unwrap_or_else(|e| fail(format!("impossibile leggere {}: {e}", p.display())))
    };
    let maps_buf = Arc::new(read_file("MAPS.BSA"));
    let blocks_buf = Arc::new(read_file("BLOCKS.BSA"));
    let arch3d_buf = Arc::new(read_file("ARCH3D.BSA"));
    let maps_bsa = BsaArchive::parse(&maps_buf).unwrap_or_else(|e| fail(e));
    let maps_reader = MapsReader::new(&maps_bsa);
    let mut locations = HashMap::new();
    for region in 0..62 {
        let name_file = format!("MAPNAMES.{:03}", region);
        if let Some(names_raw) = maps_bsa.get_by_name(&name_file) {
            if names_raw.len() < 4 {
                continue;
            }
            let count =
            u32::from_le_bytes(names_raw[0..4].try_into().unwrap_or_default()) as usize;
            for i in 0..count {
                if let Some(loc) = maps_reader.get_location(region, i) {
                    let mx = (loc.world_x / 32768) as i32;
                    let my = (MAP_HEIGHT_PIXELS - 1) - ((loc.world_y / 32768) as i32);
                    locations.insert((mx, my), loc);
                }
            }
        }
    }
    println!(
        "Caricate {} località (città, villaggi, dungeon) dalla mappa.",
             locations.len()
    );
    if let Ok(blocks_bsa_for_fix) = BsaArchive::parse(&blocks_buf) {
        fix_missing_block_names(&mut locations, &blocks_bsa_for_fix);
    }
    GameData {
        dir: args.dir.clone(),
        woods: Arc::new(WoodsFile::open(open("WOODS.WLD")).unwrap_or_else(|e| fail(e))),
        climate: Arc::new(PakFile::open(open("CLIMATE.PAK")).unwrap_or_else(|e| fail(e))),
        palette: Arc::new(Palette::open(open("ART_PAL.COL")).unwrap_or_else(|e| fail(e))),
        winter: args.winter,
        maps_buf,
        blocks_buf,
        arch3d_buf,
        locations: Arc::new(locations),
    }
}
/// Corregge i nomi di blocco che non esistono in BLOCKS.BSA.
fn fix_missing_block_names(
    locations: &mut HashMap<(i32, i32), DFLocation>,
                           blocks_bsa: &BsaArchive,
) {
    let all_names: HashSet<&str> = match &blocks_bsa.directory {
        BsaDirectory::Named(map) => map.keys().map(String::as_str).collect(),
        _ => return,
    };
    let mut sorted: Vec<&str> = all_names.iter().copied().collect();
    sorted.sort();
    let expected_type = |prefix: &str| -> Option<u8> {
        match prefix {
            "MAGE" | "FIGH" | "THIE" | "DARK" | "GUIL" => Some(11),
            "TEMP" | "SHRI" => Some(14),
            "PALA" | "CAST" => Some(16),
            _ => None,
        }
    };
    let mut fixed = 0usize;
    let mut unfixed = 0usize;
    for loc in locations.values_mut() {
        for name in loc.block_names.iter_mut() {
            if all_names.contains(name.as_str()) {
                continue;
            }
            if name.len() < 6 {
                unfixed += 1;
                continue;
            }
            let prefix = &name[..4];
            let suffix6 = &name[name.len() - 6..];
            let stem = &name[..name.len() - 4];
            let number = &stem[stem.len() - 2..];
            // --- Tentativo 1: DFU esatto (prefix + suffix6) ---
            let mut found = String::new();
            for real in &sorted {
                if real.starts_with(prefix) && real.ends_with(suffix6) {
                    found = real.to_string();
                }
            }
            if !found.is_empty() {
                eprintln!("fixup blocco [{}]: {} -> {} (esatto)", loc.name, name, found);
                *name = found;
                fixed += 1;
                continue;
            }
            // --- Tentativo 2: prefix + ?? + numero + ".RMB" ---
            let number_suffix = format!("{number}.RMB");
            let candidates: Vec<&str> = sorted
            .iter()
            .filter(|n| {
                n.len() == prefix.len() + 8
                && n.starts_with(prefix)
                && n.ends_with(&number_suffix)
                && n.chars()
                .skip(prefix.len())
                .take(2)
                .all(|c| c.is_ascii_alphabetic())
            })
            .copied()
            .collect();
            let with_big_model = candidates.iter().find(|&&n| {
                blocks_bsa
                .get_by_name(n)
                .and_then(|raw| RmbBlock::parse(raw).ok())
                .map_or(false, |rmb| rmb.objects_3d.iter().any(|o| o.model_id >= 40000))
            });
            let preferred_type = expected_type(prefix);
            let with_expected_type = preferred_type.and_then(|t| {
                candidates.iter().find(|&&n| {
                    blocks_bsa
                    .get_by_name(n)
                    .and_then(|raw| RmbBlock::parse(raw).ok())
                    .map_or(false, |rmb| rmb.buildings.iter().any(|b| b.building_type == t))
                })
            });
            let best = with_big_model
            .copied()
            .or_else(|| with_expected_type.copied())
            .or_else(|| candidates.first().copied());
            if let Some(real) = best {
                eprintln!(
                    "fixup blocco [{}]: {} -> {} (numero, big_model={}, type={:?})",
                          loc.name,
                          name,
                          real,
                          with_big_model.is_some(),
                          preferred_type
                );
                *name = real.to_string();
                fixed += 1;
                continue;
            }
            eprintln!(
                "fixup blocco [{}]: {} NON correggibile (prefisso {}, numero {})",
                      loc.name, name, prefix, number
            );
            unfixed += 1;
        }
    }
    eprintln!("fixup blocchi: {fixed} corretti, {unfixed} non correggibili");
}
// =============================================================================
// Scelta del punto di partenza
// =============================================================================
fn find_start(data: &GameData) -> (i32, i32) {
    let (cx, cy) = (MAP_WIDTH_PIXELS / 2, MAP_HEIGHT_PIXELS / 2);
    for r in 0..120i32 {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs().max(dy.abs()) != r {
                    continue;
                }
                let (x, y) = (cx + dx, cy + dy);
                if in_map(x, y)
                    && data.woods.height_value(x, y) >= 30
                    && climate_of(data, x, y) != 223
                    {
                        return (x, y);
                    }
            }
        }
    }
    (cx, cy)
}
// =============================================================================
// main
// =============================================================================
fn main() {
    let args = parse_args();
    let data = load_data(&args);

    // Inizializza la cache degli NPC custom dalla cartella delle texture
    let custom_npcs = interior::CustomNpcCache::load(&args.textures_dir);

    let privateer_loc = data
    .locations
    .values()
    .find(|l| l.name.to_ascii_uppercase().contains("PRIVATEER"))
    .cloned();
    let (mx, my) = args.pixel.unwrap_or_else(|| {
        if let Some(ref ph) = privateer_loc {
            let ph_mx = (ph.world_x / 32768) as i32;
            let ph_my = (MAP_HEIGHT_PIXELS - 1) - ((ph.world_y / 32768) as i32);
            (ph_mx, ph_my)
        } else if data.locations.contains_key(&(207, 213)) {
            (207, 213)
        } else {
            find_start(&data)
        }
    });
    if !in_map(mx, my) {
        fail(format!(
            "il map pixel ({mx}, {my}) è fuori dalla mappa (0..999, 0..499)"
        ));
    }
    println!("Partenza dal map pixel ({mx}, {my}), raggio {}.", args.radius);
    println!("Cartella mod: {}", args.mods_dir.display());
    println!("Cartella texture sciolte: {}", args.textures_dir.display());
    println!("Comandi: clic = cattura il mouse, Esc = rilascia, WASD = cammina, Maiusc = corri, ");
    println!("         Spazio = salta, F = volo (Spazio/Ctrl su e giù, Maiusc in volo = velocissimo). ");
    println!("         T = ferma/riprendi il tempo, , e . = un'ora indietro/avanti. ");
    println!("         M = apri/chiudi mappa dell'Iliac Bay. ");
    println!("         L = mappa locale della città (taverne verdi, negozi rossi, gilde blu). ");
    println!("         G = attiva/disattiva l'erba. ");
    println!("         E / U = entra nel locale o dungeon / esci vicino alla porta (Backspace = esci sempre). ");
    println!("         C = equipaggiamento, Z = arma, X = torcia. ");
    // ── dfquest: libreria + stato delle quest ─────────────────────────────
    let (quest_lib, quest_warnings) = dfquest::Library::load_dir(
        std::path::Path::new(dfquest::DEFAULT_QUESTS_DIR),
    );
    for w in &quest_warnings {
        eprintln!("dfquest: {w}");
    }
    println!(
        "dfquest: {} quest script caricati ({} avvisi)",
             quest_lib.len(),
             quest_warnings.len()
    );
    let mut quest_states = std::fs::read_to_string("quest_states.txt")
    .map(|t| dfquest::QuestStates::from_text(&t))
    .unwrap_or_default();
    for script in quest_lib.scripts() {
        if quest_states.phase(&script.id) == dfquest::QuestPhase::Active {
            for t in script.startup_topics() {
                quest_states.enable_topic(&script.id, &t.symbol);
            }
        }
    }
    println!(
        "dfquest: S0000001 phase = {:?}, N0B00Y16 caricata: {}",
        quest_states.phase("S0000001"),
             quest_lib.get("N0B00Y16").is_some(),
    );
    // ────────────────────────────────────────────────────────────────────────
    App::new()
    .add_plugins(
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Daggerfall - terreno e città".into(),
                                 resolution: (1280.0_f32, 720.0_f32).into(),
                                 ..default()
            }),
            ..default()
        }),
    )
    .add_plugins(dialog::DialogPlugin)
    .add_plugins(quest_llm::QuestPlugin)
    .add_plugins(tts::TtsPlugin)
    .add_plugins(fps_weapon::FpsWeaponPlugin)
    .add_plugins(compass::CompassPlugin)
    .add_plugins(vitals::VitalsPlugin)
    .add_plugins(tuning_menu::TuningMenuPlugin)
    .add_plugins(dungeon_enemies::DungeonEnemyPlugin)
    .add_plugins(torch::TorchPlugin)
    .add_plugins(savegame::SaveGamePlugin)
    .add_plugins(equipment::EquipmentPlugin)
    .add_plugins(relief_post::FakeReliefPlugin)
    .add_plugins(dungeon::DungeonPlugin)
    .add_plugins(MaterialPlugin::<TerrainMaterial>::default())
    .add_plugins(ModTexturesPlugin {
        dir: args.mods_dir.clone(),
                 textures_dir: Some(args.textures_dir.clone()),
                 nearest_filter: false,
    })
    .add_plugins(custom_cursor::CustomCursorPlugin {
        cursor_path: Some(args.textures_dir.join("cursorA.png")),
    })
    .insert_resource(ClearColor(SKY))
    .insert_resource(AmbientLight {
        color: Color::srgb(0.85, 0.9, 1.0),
                     brightness: 450.0,
    })
    .insert_resource(data)
    // ── dfquest: risorse lette da `dialog.rs` (wrapper Resource) ──────
    .insert_resource(QuestLibRes(quest_lib))
    .insert_resource(QuestStatesRes(quest_states))
    // ── NPC Custom ────────────────────────────────────────────────────
    .insert_resource(custom_npcs)
    // ────────────────────────────────────────────────────────────────
    .insert_resource(HeightField::default())
    .insert_resource(StartInfo { mx, my })
    .insert_resource(StartDungeonConfig {
        start_in_dungeon: args.start_dungeon,
    })
    .insert_resource(RadiusSetting(args.radius))
    .insert_resource(NaturePool::default())
    .insert_resource(grass::GrassSettings::default())
    .insert_resource(wall_textures::WallTextures::default())
    .insert_resource(interior::InteriorState::default())
    .insert_resource(sky::SkyConfig {
        hour: args.hour,
        day_minutes: args.day_minutes,
    })
    .add_event::<music::MusicEvent>()
    .insert_resource(music::MusicController::default())
    .init_resource::<music::MusicDucking>()
    .add_systems(
        Startup,
        (
            setup,
         sky::setup_sky,
         map::setup_map_ui,
         local_map::setup_local_map,
         music::start_music,
         grass::setup_grass,
        )
        .chain(),
    )
    // ── Gruppo A: simulazione (13 sistemi, sotto il limite di 16) ─────
    .add_systems(
        Update,
        (
            player::grab_cursor,
         player::player_look,
         interior::interior_enter_exit,
         interior::interior_move,
         interior::face_flats,
         tiles::update_npcs,
         tiles::update_followers, // ← aggiunto
         tiles::update_enemies,
         player::player_move,
         rebase_origin,
         stream_tiles,
         collect_tiles,
         sky::update_sky.run_if(interior::outside),
        )
        .chain(),
    )
    .add_systems(
        Update,
        (
            map::toggle_map,
         map::update_map_marker,
         local_map::toggle_local_map,
         local_map::update_local_map,
         music_zones::update_music_zone,
         music::play_music,
         music::duck_music,
         update_title,
        )
        .chain()
        .after(collect_tiles),
    )
    .run();
}
// =============================================================================
// Setup iniziale
// =============================================================================
fn setup(
    mut commands: Commands,
    data: Res<GameData>,
    start: Res<StartInfo>,
    radius: Res<RadiusSetting>,
) {
    let streaming = Streaming::new(radius.0, (1024, 1024));
    commands.insert_resource(streaming);
    let origin = Origin {
        east: start.mx,
        north: MAP_HEIGHT_PIXELS - start.my,
    };
    let samples = generate_default_samples(&data.woods, start.mx, start.my)
    .unwrap_or_else(|e| fail(e));
    let ground = terrain_height(&samples, 64.0, 64.0);
    commands.insert_resource(origin);
    let reach = (radius.0 as f32 + 0.3) * MAP_PIXEL_SIZE;
    commands.spawn((
        Camera3d::default(),
                    Projection::Perspective(PerspectiveProjection {
                        fov: 70.0_f32.to_radians(),
                                            near: 0.1,
                                            far: 60_000.0,
                                            ..default()
                    }),
                    DistanceFog {
                        color: SKY,
                        falloff: FogFalloff::Linear {
                            start: reach * 0.5,
                            end: reach,
                        },
                        ..default()
                    },
                    Transform::from_xyz(
                        MAP_PIXEL_SIZE / 2.0,
                        ground + EYE_HEIGHT,
                        -MAP_PIXEL_SIZE / 2.0,
                    ),
                    Player::new(ground),
    ));
    let _ = CELL_SIZE;
}
// =============================================================================
// Titolo della finestra
// =============================================================================
#[allow(clippy::too_many_arguments)]
fn update_title(
    time: Res<Time>,
    origin: Res<Origin>,
    streaming: Res<Streaming>,
    data: Res<GameData>,
    clock: Res<sky::SkyClock>,
    music: Res<music::MusicController>,
    player: Query<(&Player, &Transform)>,
                mut windows: Query<&mut Window, With<PrimaryWindow>>,
                mut state: Local<(f32, f32)>,
) {
    state.0 += time.delta_secs();
    state.1 = state.1 * 0.9 + time.delta_secs() * 0.1;
    if state.0 < 0.5 {
        return;
    }
    state.0 = 0.0;
    let (Ok((p, tf)), Ok(mut window)) = (player.get_single(), windows.get_single_mut()) else {
        return;
    };
    let (mx, my) = map_pixel_of(&origin, tf.translation);
    let fps = if state.1 > 0.0 { 1.0 / state.1 } else { 0.0 };
    let track = music
    .current_track
    .as_deref()
    .map(|t| t.rsplit('/').next().unwrap_or(t))
    .unwrap_or("nessuna");
    window.title = format!(
        "Daggerfall - map pixel ({mx}, {my}) clima {} - quota {:.0} m - ore {:02}:{:02} - {}{} tile - {fps:.0} fps - ♪ {track} [M: mappa, L: locale]",
                           climate_of(&data, mx, my),
                           p.feet_y,
                           clock.hours.floor() as i32,
                           ((clock.hours.fract()) * 60.0) as i32,
                           if p.fly { "volo - " } else { "" },
                               streaming.loaded_count(),
    );
}
