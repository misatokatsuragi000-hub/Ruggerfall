//! Terreno percorribile di Daggerfall con supporto per shader WGSL,
//! mod HD di D.R.E.A.M., mappa interattiva (tasto M), vegetazione 3D,
//! città/villaggi e colonna sonora originale.
mod data;
mod grass;
mod interior;
mod local_map;
mod dialog;
mod map;
mod mod_textures;
mod music;
mod music_zones;
mod player;
mod sky;
mod terrain_material;
mod tiles;
mod trees;
mod wall_textures;
mod world;

use crate::data::bsa::BsaArchive;
use crate::data::maps::MapsReader;
use crate::data::terrain_mesh::{terrain_height, CELL_SIZE, MAP_PIXEL_SIZE};
use crate::data::{find_file_ci, generate_default_samples, PakFile, Palette, WoodsFile};
use bevy::pbr::{DistanceFog, FogFalloff, MaterialPlugin};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use mod_textures::ModTexturesPlugin;
use player::{Player, EYE_HEIGHT};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use terrain_material::TerrainMaterial;
use tiles::*;
use trees::NaturePool;
use world::*;

const USAGE: &str = "uso: dfu-walk <cartella ARENA2> [--mods <cartella>] [--pixel X Y] [--radius N] [--winter] [--hour H] [--day-minutes M]";
const SKY: Color = Color::srgb(0.55, 0.72, 0.92);

struct Args {
    dir: PathBuf,
    mods_dir: PathBuf,
    pixel: Option<(i32, i32)>,
    radius: i32,
    winter: bool,
    hour: f32,
    day_minutes: f32,
}

#[derive(Resource)]
struct StartInfo {
    mx: i32,
    my: i32,
}

#[derive(Resource)]
struct RadiusSetting(i32);

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
    let mut args = Args {
        dir: PathBuf::from(dir),
        mods_dir: default_mods,
        pixel: None,
        radius: 2,
        winter: false,
        hour: 10.5,
        day_minutes: 24.0,
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--mods" => {
                let m = it
                .next()
                .unwrap_or_else(|| fail(format!("--mods vuole un percorso\n{USAGE}")));
                args.mods_dir = PathBuf::from(m);
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
        "Caricate {} località (città e villaggi) dalla mappa.",
             locations.len()
    );
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

fn main() {
    let args = parse_args();
    let data = load_data(&args);
    // Capitale Daggerfall: map_x = 207, map_y = 499 - 286 = 213
    let (mx, my) = args.pixel.unwrap_or_else(|| {
        if data.locations.contains_key(&(207, 213)) {
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
    println!("Comandi: clic = cattura il mouse, Esc = rilascia, WASD = cammina, Maiusc = corri,");
    println!("         Spazio = salta, F = volo (Spazio/Ctrl su e giù), Maiusc in volo = velocissimo.");
    println!("         T = ferma/riprendi il tempo, , e . = un'ora indietro/avanti.");
    println!("         M = apri/chiudi mappa dell'Iliac Bay.");
    println!("         L = mappa locale della città (taverne verdi, negozi rossi, gilde blu).");
    println!("         G = attiva/disattiva l'erba.");
    println!("         E = entra nella taverna vicina / esci vicino alla porta (Backspace = esci sempre).");

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
    .add_plugins(MaterialPlugin::<TerrainMaterial>::default())
    .add_plugins(ModTexturesPlugin {
        dir: args.mods_dir.clone(),
                 nearest_filter: false,
    })
    .insert_resource(ClearColor(SKY))
    .insert_resource(AmbientLight {
        color: Color::srgb(0.85, 0.9, 1.0),
                     brightness: 450.0,
    })
    .insert_resource(data)
    .insert_resource(HeightField::default())
    .insert_resource(StartInfo { mx, my })
    .insert_resource(RadiusSetting(args.radius))
    .insert_resource(NaturePool::default())
    .insert_resource(grass::GrassSettings::default())
    .insert_resource(wall_textures::WallTextures::default())
    .insert_resource(interior::InteriorState::default())
    .insert_resource(sky::SkyConfig {
        hour: args.hour,
        day_minutes: args.day_minutes,
    })
    // --- Musica di Daggerfall ---
    .add_event::<music::MusicEvent>()
    .insert_resource(music::MusicController::default())
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
    .add_systems(
        Update,
        (
            player::grab_cursor,
         player::player_look,
         interior::interior_enter_exit,
         interior::interior_move,
         interior::face_flats,
         tiles::update_npcs,  // <-- AGGIUNGI
         tiles::update_enemies,
         player::player_move,
         rebase_origin,
         stream_tiles,
         collect_tiles,
         sky::update_sky.run_if(interior::outside),
         map::toggle_map,
         map::update_map_marker,
         local_map::toggle_local_map,
         local_map::update_local_map,
         music_zones::update_music_zone,
         music::play_music,
         update_title,
        )
        .chain(),
    )
    .add_systems(
        Update,
        (grass::grass_toggle, grass::update_grass.run_if(interior::outside))
            .chain()
            .after(collect_tiles),
    )
    .run();
}

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
                                            near: 0.3,
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
                    Transform::from_xyz(MAP_PIXEL_SIZE / 2.0, ground + EYE_HEIGHT, -MAP_PIXEL_SIZE / 2.0),
                    Player::new(ground),
    ));
    let _ = CELL_SIZE;
}

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
