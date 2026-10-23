//! Rileva la situazione di gioco e cambia musica di conseguenza,
//! replicando la logica del SongManager di Daggerfall Unity.
use crate::interior::InteriorState;
use crate::music::{MusicContext, MusicEnvironment, MusicEvent, MusicTime, MusicWeather};
use crate::sky::SkyClock;
use crate::tiles::climate_of;
use crate::world::{GameData, Origin, map_pixel_of};
use bevy::prelude::*;

/// Sistema che determina la zona musicale in base allo stato del gioco.
/// Da aggiungere in Update, dopo i sistemi di movimento e interior.
pub fn update_music_zone(
    mut events: EventWriter<MusicEvent>,
    clock: Res<SkyClock>,
    interior: Res<InteriorState>,
    controller: Res<crate::music::MusicController>,
    data: Res<GameData>,
    origin: Res<Origin>,
    player: Query<&Transform, With<crate::player::Player>>,
) {
    let Ok(player_tf) = player.get_single() else {
        return;
    };

    // Determina l'ambiente
    let environment = if interior.is_inside() {
        // Per ora, dentro una taverna
        MusicEnvironment::Tavern
    } else {
        // Fuori: determina se è in città o wilderness
        let (mx, my) = map_pixel_of(&origin, player_tf.translation);
        if data.locations.contains_key(&(mx, my)) {
            MusicEnvironment::City
        } else {
            MusicEnvironment::Wilderness
        }
    };

    // Determina l'ora del giorno
    let hour = clock.hours;
    let time = if hour >= 6.0 && hour < 19.0 {
        MusicTime::Day
    } else {
        MusicTime::Night
    };

    // Determina il meteo (semplificato: usa il clima del map pixel)
    let (mx, my) = map_pixel_of(&origin, player_tf.translation);
    let climate = climate_of(&data, mx, my);
    let weather = match climate {
        224 | 225 => MusicWeather::Sunny, // Deserto
        226 => MusicWeather::Overcast,    // Montagna
        227 => MusicWeather::Rain,        // Foresta pluviale
        228 => MusicWeather::Overcast,    // Palude
        229 => MusicWeather::Sunny,       // Sub-tropicale
        230 | 231 | 232 => MusicWeather::Cloudy, // Boschi
        _ => MusicWeather::Sunny,
    };

    // Calcola i giorni di gioco (semplificato: usa l'ora corrente)
    let game_days = (clock.hours / 24.0).floor() as u32;

    let context = MusicContext {
        environment,
        weather,
        time,
        game_days,
    };

    // Invia l'evento solo se il contesto è cambiato
    if controller.current_context != Some(context) {
        events.send(MusicEvent::SetContext(context));
    }
}