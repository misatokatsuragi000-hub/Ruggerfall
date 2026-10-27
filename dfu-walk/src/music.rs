//! Colonna sonora di Daggerfall: cambio traccia via eventi in base a situazione di gioco,
//! meteo e ora del giorno. Replica la logica di SongManager.cs di Daggerfall Unity.
//!
//! ## Ducking
//!
//! Quando un PNG parla (componente `tts::SpeechVoice` presente in scena), la musica scende
//! a `MUSIC_DUCKED_VOLUME`; quando la voce finisce o il dialogo viene chiuso, torna a
//! `MUSIC_BASE_VOLUME`. La transizione è interpolata in `MUSIC_DUCK_FADE_SECS` secondi,
//! così non ci sono stacchi bruschi.
//!
//! Richiede in `main.rs`:
//!   `.init_resource::<music::MusicDucking>()`
//!   e `music::duck_music` nella catena di Update, dopo `music::play_music`.

use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::LazyLock;

use crate::tts::SpeechVoice;

// =============================================================================
// Ducking: costanti, marker, risorsa, sistema
// =============================================================================

/// Volume normale della musica di sottofondo.
pub const MUSIC_BASE_VOLUME: f32 = 0.4;
/// Volume della musica mentre un PNG sta parlando (15% del normale).
pub const MUSIC_DUCKED_VOLUME: f32 = 0.06;
/// Durata della transizione base ↔ ducked (secondi).
pub const MUSIC_DUCK_FADE_SECS: f32 = 0.3;

/// Marca le entità della musica di sottofondo spawnate da `play_music`, così il
/// sistema di ducking sa quali `AudioSink` modulare (le voci TTS hanno il loro
/// `SpeechSource` e non vengono toccate).
#[derive(Component)]
pub struct MusicPlaying;

/// Stato del ducking, aggiornato ogni frame da `duck_music`.
#[derive(Resource)]
pub struct MusicDucking {
    /// Volume attuale applicato alle sink della musica.
    pub current: f32,
    /// Volume verso cui `current` sta interpolando.
    pub target: f32,
}

impl Default for MusicDucking {
    fn default() -> Self {
        Self { current: MUSIC_BASE_VOLUME, target: MUSIC_BASE_VOLUME }
    }
}

/// Sistema: se c'è almeno una `SpeechVoice` in scena, abbassa la musica; altrimenti
/// la riporta al volume normale. Interpola `MusicDucking.current` verso il target
/// con passo costante, poi applica il valore a ogni `AudioSink` della musica.
pub fn duck_music(
    time: Res<Time>,
    voices: Query<Entity, With<SpeechVoice>>,
    mut ducking: ResMut<MusicDucking>,
    sinks: Query<&AudioSink, With<MusicPlaying>>,
) {
    let speaking = !voices.is_empty();
    ducking.target = if speaking { MUSIC_DUCKED_VOLUME } else { MUSIC_BASE_VOLUME };

    let step = (MUSIC_BASE_VOLUME / MUSIC_DUCK_FADE_SECS) * time.delta_secs();
    if (ducking.current - ducking.target).abs() <= step {
        ducking.current = ducking.target;
    } else if ducking.current < ducking.target {
        ducking.current += step;
    } else {
        ducking.current -= step;
    }

    for sink in &sinks {
        sink.set_volume(ducking.current);
    }
}

// =============================================================================
// Playlist e contesto (invariati)
// =============================================================================

/// Situazione musicale (PlayerMusicEnvironment di DFU)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MusicEnvironment {
    Castle,
    City,
    DungeonExterior,
    DungeonInterior,
    Graveyard,
    MagesGuild,
    FighterTrainers,
    Interior,
    Palace,
    Shop,
    Tavern,
    TempleGood,
    TempleNeutral,
    TempleBad,
    Wilderness,
}

/// Meteo musicale (PlayerMusicWeather di DFU)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MusicWeather {
    Sunny,
    Cloudy,
    Overcast,
    Rain,
    Snow,
}

/// Ora del giorno (PlayerMusicTime di DFU)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MusicTime {
    Day,
    Night,
}

/// Contesto musicale completo
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MusicContext {
    pub environment: MusicEnvironment,
    pub weather: MusicWeather,
    pub time: MusicTime,
    pub game_days: u32,
}

/// Playlist di brani per una data situazione
#[derive(Clone, Copy, Debug)]
pub struct Playlist {
    pub tracks: &'static [&'static str],
}

impl Playlist {
    pub fn pick_track(&self, context: &MusicContext) -> &'static str {
        if self.tracks.is_empty() {
            return "music/silence.ogg";
        }
        // Taverna: cambia brano ogni giorno (come in DFU)
        if self.tracks.len() > 1 {
            let index = context.game_days as usize % self.tracks.len();
            self.tracks[index]
        } else {
            self.tracks[0]
        }
    }
}

/// Mappe delle playlist (semplificate rispetto a DFU per gestire solo i file che hai estratto)
static PLAYLISTS: LazyLock<HashMap<(MusicEnvironment, Option<MusicWeather>, Option<MusicTime>), Playlist>> = LazyLock::new(|| {
    let mut m = HashMap::new();

    // Città/Wilderness di giorno in base al meteo
    m.insert((MusicEnvironment::City, Some(MusicWeather::Sunny), Some(MusicTime::Day)),
             Playlist { tracks: &["music/gday___d.ogg", "music/swimming.ogg", "music/gsunny2.ogg", "music/sunnyday.ogg", "music/02.ogg", "music/03.ogg", "music/22.ogg"] });
    m.insert((MusicEnvironment::Wilderness, Some(MusicWeather::Sunny), Some(MusicTime::Day)),
             Playlist { tracks: &["music/gday___d.ogg", "music/swimming.ogg", "music/gsunny2.ogg", "music/sunnyday.ogg", "music/02.ogg", "music/03.ogg", "music/22.ogg"] });

    m.insert((MusicEnvironment::City, Some(MusicWeather::Cloudy), Some(MusicTime::Day)),
             Playlist { tracks: &["music/gday___d.ogg", "music/swimming.ogg", "music/gsunny2.ogg", "music/sunnyday.ogg", "music/02.ogg", "music/03.ogg", "music/22.ogg", "music/29.ogg", "music/12.ogg"] });
    m.insert((MusicEnvironment::Wilderness, Some(MusicWeather::Cloudy), Some(MusicTime::Day)),
             Playlist { tracks: &["music/gday___d.ogg", "music/swimming.ogg", "music/gsunny2.ogg", "music/sunnyday.ogg", "music/02.ogg", "music/03.ogg", "music/22.ogg", "music/29.ogg", "music/12.ogg"] });

    m.insert((MusicEnvironment::City, Some(MusicWeather::Overcast), Some(MusicTime::Day)),
             Playlist { tracks: &["music/29.ogg", "music/12.ogg", "music/13.ogg", "music/gpalac.ogg", "music/overcast.ogg"] });
    m.insert((MusicEnvironment::Wilderness, Some(MusicWeather::Overcast), Some(MusicTime::Day)),
             Playlist { tracks: &["music/29.ogg", "music/12.ogg", "music/13.ogg", "music/gpalac.ogg", "music/overcast.ogg"] });

    m.insert((MusicEnvironment::City, Some(MusicWeather::Rain), Some(MusicTime::Day)),
             Playlist { tracks: &["music/overlong.ogg", "music/raining.ogg", "music/08.ogg"] });
    m.insert((MusicEnvironment::Wilderness, Some(MusicWeather::Rain), Some(MusicTime::Day)),
             Playlist { tracks: &["music/overlong.ogg", "music/raining.ogg", "music/08.ogg"] });

    m.insert((MusicEnvironment::City, Some(MusicWeather::Snow), Some(MusicTime::Day)),
             Playlist { tracks: &["music/20.ogg", "music/gsnow__b.ogg", "music/oversnow.ogg"] });
    m.insert((MusicEnvironment::Wilderness, Some(MusicWeather::Snow), Some(MusicTime::Day)),
             Playlist { tracks: &["music/20.ogg", "music/gsnow__b.ogg", "music/oversnow.ogg"] });

    // Notte (indipendente dal meteo)
    m.insert((MusicEnvironment::City, None, Some(MusicTime::Night)),
             Playlist { tracks: &["music/10.ogg", "music/11.ogg", "music/gcurse.ogg", "music/geerie.ogg", "music/gruins.ogg", "music/18.ogg", "music/21.ogg"] });
    m.insert((MusicEnvironment::Wilderness, None, Some(MusicTime::Night)),
             Playlist { tracks: &["music/10.ogg", "music/11.ogg", "music/gcurse.ogg", "music/geerie.ogg", "music/gruins.ogg", "music/18.ogg", "music/21.ogg"] });

    // Ambienti specifici
    m.insert((MusicEnvironment::Tavern, None, None),
             Playlist { tracks: &["music/square_2.ogg", "music/tavern.ogg", "music/folk1.ogg", "music/folk2.ogg", "music/folk3.ogg"] });
    m.insert((MusicEnvironment::Shop, None, None),
             Playlist { tracks: &["music/gshop.ogg"] });
    m.insert((MusicEnvironment::MagesGuild, None, None),
             Playlist { tracks: &["music/gmage_3.ogg", "music/magic_2.ogg"] });
    m.insert((MusicEnvironment::Palace, None, None),
             Playlist { tracks: &["music/06.ogg"] });
    m.insert((MusicEnvironment::Castle, None, None),
             Playlist { tracks: &["music/gpalac.ogg"] });
    m.insert((MusicEnvironment::Interior, None, None),
             Playlist { tracks: &["music/23.ogg"] });
    m.insert((MusicEnvironment::TempleGood, None, None),
             Playlist { tracks: &["music/ggood.ogg"] });
    m.insert((MusicEnvironment::TempleNeutral, None, None),
             Playlist { tracks: &["music/gneut.ogg"] });
    m.insert((MusicEnvironment::TempleBad, None, None),
             Playlist { tracks: &["music/gbad.ogg"] });
    m.insert((MusicEnvironment::DungeonInterior, None, None),
             Playlist { tracks: &["music/dungeon.ogg", "music/dungeon5.ogg", "music/dungeon6.ogg", "music/dungeon7.ogg", "music/dungeon8.ogg", "music/dungeon9.ogg", "music/gdngn10.ogg", "music/gdngn11.ogg", "music/gdungn4.ogg", "music/gdungn9.ogg", "music/04.ogg", "music/05.ogg", "music/07.ogg", "music/15.ogg", "music/28.ogg"] });
    m.insert((MusicEnvironment::DungeonExterior, None, None),
             Playlist { tracks: &["music/10.ogg", "music/11.ogg", "music/gcurse.ogg", "music/geerie.ogg", "music/gruins.ogg", "music/18.ogg", "music/21.ogg"] });
    m.insert((MusicEnvironment::Graveyard, None, None),
             Playlist { tracks: &["music/10.ogg", "music/11.ogg", "music/gcurse.ogg", "music/geerie.ogg", "music/gruins.ogg", "music/18.ogg", "music/21.ogg"] });

    m
});

#[derive(Resource, Default)]
pub struct MusicController {
    pub current_context: Option<MusicContext>,
    pub current_track: Option<String>,
}

#[derive(Event)]
pub enum MusicEvent {
    SetContext(MusicContext),
    Stop,
}

/// Avvia la musica iniziale (città di giorno soleggiato)
pub fn start_music(mut events: EventWriter<MusicEvent>) {
    events.send(MusicEvent::SetContext(MusicContext {
        environment: MusicEnvironment::City,
        weather: MusicWeather::Sunny,
        time: MusicTime::Day,
        game_days: 0,
    }));
}

pub fn play_music(
    mut commands: Commands,
    mut events: EventReader<MusicEvent>,
    asset_server: Res<AssetServer>,
    mut controller: ResMut<MusicController>,
    ducking: Res<MusicDucking>,
    audio_players: Query<Entity, With<AudioPlayer>>,
) {
    for ev in events.read() {
        match ev {
            MusicEvent::SetContext(context) => {
                if controller.current_context == Some(*context) && controller.current_track.is_some() {
                    continue;
                }

                // Ferma la traccia precedente. Filtra solo `AudioPlayer<AudioSource>`:
                // le voci TTS usano `AudioPlayer<SpeechSource>` e non vengono toccate.
                for e in &audio_players {
                    commands.entity(e).despawn();
                }

                // Trova la playlist per il contesto
                let key = (context.environment, Some(context.weather), Some(context.time));
                let playlist = PLAYLISTS.get(&key)
                .or_else(|| PLAYLISTS.get(&(context.environment, None, Some(context.time))))
                .or_else(|| PLAYLISTS.get(&(context.environment, None, None)))
                .unwrap_or(&Playlist { tracks: &["music/gday___d.ogg"] });

                let path = playlist.pick_track(context);
                let handle = asset_server.load(path);
                // Se stiamo già abbassando la musica per una voce, la nuova traccia
                // parte già al volume ducked, così non c'è uno stacco.
                commands.spawn((
                    AudioPlayer::new(handle),
                                PlaybackSettings::LOOP
                                .with_volume(bevy::audio::Volume::new(ducking.current)),
                                MusicPlaying,
                ));

                controller.current_context = Some(*context);
                controller.current_track = Some(path.to_string());
                info!("Musica: {path}");
            }
            MusicEvent::Stop => {
                for e in &audio_players {
                    commands.entity(e).despawn();
                }
                controller.current_track = None;
                controller.current_context = None;
            }
        }
    }
}
