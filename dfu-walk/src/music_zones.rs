//! Rileva la situazione di gioco e cambia musica di conseguenza.
use crate::data::city_map::PoiKind;
use crate::interior::InteriorState;
use crate::music::{MusicContext, MusicEnvironment, MusicEvent, MusicTime, MusicWeather};
use crate::sky::SkyClock;
use crate::tiles::climate_of;
use crate::world::{map_pixel_of, GameData, Origin};
use bevy::prelude::*;

pub fn update_music_zone(
    mut events: EventWriter<MusicEvent>,
    clock: Res<SkyClock>,
    interior: Res<InteriorState>,
    dungeon: Res<crate::dungeon::DungeonState>, // <-- AGGIUNTO
    controller: Res<crate::music::MusicController>,
    data: Res<GameData>,
    origin: Res<Origin>,
    player: Query<&Transform, With<crate::player::Player>>,
) {
    let Ok(player_tf) = player.get_single() else {
        return;
    };

    // Determina l'ambiente sonoro
    let environment = if dungeon.is_inside() {
        // Musica ufficiale del dungeon!
        MusicEnvironment::DungeonInterior
    } else if interior.is_inside() {
        match interior.kind() {
            Some(PoiKind::Shop) => MusicEnvironment::Shop,
            _ => MusicEnvironment::Tavern,
        }
    } else {
        let (mx, my) = map_pixel_of(&origin, player_tf.translation);
        if data.locations.contains_key(&(mx, my)) {
            MusicEnvironment::City
        } else {
            MusicEnvironment::Wilderness
        }
    };

    let hour = clock.hours;
    let time = if hour >= 6.0 && hour < 19.0 {
        MusicTime::Day
    } else {
        MusicTime::Night
    };

    let (mx, my) = map_pixel_of(&origin, player_tf.translation);
    let climate = climate_of(&data, mx, my);
    let weather = match climate {
        224 | 225 => MusicWeather::Sunny,
        226 => MusicWeather::Overcast,
        227 => MusicWeather::Rain,
        228 => MusicWeather::Overcast,
        229 => MusicWeather::Sunny,
        230 | 231 | 232 => MusicWeather::Cloudy,
        _ => MusicWeather::Sunny,
    };

    let game_days = (clock.hours / 24.0).floor() as u32;

    let context = MusicContext {
        environment,
        weather,
        time,
        game_days,
    };

    if controller.current_context != Some(context) {
        events.send(MusicEvent::SetContext(context));
    }
}
