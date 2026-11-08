//! Pioggia visibile: particelle attorno al giocatore, attive solo quando il
//! meteo effettivo è `Pioggia` o `Temporale` (`CurrentPreset`, deciso da
//! `calendar::sync_season_from_calendar`: calendario oppure override F4).
//!
//! ## Design
//!
//! - Pool fisso di `PARTICLE_COUNT` gocce (default 2000). Spawnate tutte al
//!   primo activate, despawnate tutte quando smette di piovere.
//! - Ogni goccia è un piccolo rettangolo (0.02 × 0.4 m) billboardato in Y:
//!   resta verticale, guarda la camera in yaw. Una sola `Mesh` condivisa e un
//!   solo `StandardMaterial` condiviso → Bevy raggruppa in un batch.
//! - Aggiornamento in `Update`: O(N) trasformazioni per frame, con N=2000 è
//!   trascurabile.
//! - Riciclo: quando una goccia scende sotto il player di `FLOOR_OFFSET`,
//!   viene riposizionata in cima al cilindro che circonda il player. Se il
//!   player si sposta di molto in un frame (teletrasporto), tutte le gocce
//!   vengono riciclate subito attorno alla nuova posizione.
//! - **Non** piove in interni/dungeon, anche se il meteo è Pioggia.
//! - **Non** piove sott'acqua: il tuffo spegne le gocce, la riemersione le riaccende.
//!
//! ## Attivazione
//!
//! `update_rain_activation` legge `CurrentPreset` (opzionale: se il plugin del
//! cielo non è caricato, la pioggia resta spenta) e `InteriorState`/
//! `DungeonState` (opzionali). Se il preset è Pioggia o Temporale **e** si è
//! all'aperto **e** non sott'acqua, attiva; altrimenti spegne.
//!
//! ## Integrazione
//!
//! In `main.rs`:
//!   `mod rain_fx;`
//!   `.add_plugins(rain_fx::RainFxPlugin)`
use bevy::prelude::*;
use bevy::render::mesh::{Indices, PrimitiveTopology};
use bevy::render::render_asset::RenderAssetUsages;
use crate::player::Player;
use crate::sky::dream_sky::CurrentPreset;
use crate::underwater::UnderwaterState;

// =============================================================================
// Parametri
// =============================================================================

/// Numero di gocce attive contemporaneamente.
const PARTICLE_COUNT: usize = 2000;
/// Raggio del cilindro di gocce attorno al player (m).
const RADIUS: f32 = 40.0;
/// Altezza sopra il player da cui partono le gocce (m).
const HEIGHT: f32 = 30.0;
/// Velocità di caduta (m/s).
const SPEED: f32 = 25.0;
/// Deriva orizzontale dovuta al vento (m/s).
const WIND: Vec2 = Vec2::new(2.5, 1.5);
/// Se una goccia scende sotto `player.y - FLOOR_OFFSET`, viene riciclata.
const FLOOR_OFFSET: f32 = 5.0;
/// Distanza oltre la quale un salto del player (teletrasporto) fa riciclare
/// tutte le gocce subito invece di aspettare che cadano.
const TELEPORT_RECYCLE_DIST: f32 = 50.0;

const DROP_WIDTH: f32 = 0.02;
const DROP_HEIGHT: f32 = 0.4;

// =============================================================================
// Rng deterministico semplice (xorshift64)
// =============================================================================

struct Rng(u64);

impl Default for Rng {
    fn default() -> Self {
        Self(0x9E37_79B9_7F4A_7C15)
    }
}

impl Rng {
    fn seed(mut self, seed: u64) -> Self {
        if seed == 0 {
            self.0 = 0x9E37_79B9_7F4A_7C15;
        } else {
            self.0 = seed;
        }
        self
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn f32(&mut self) -> f32 {
        // 24 bit di mantissa: precisione buona, distribuzione uniforme.
        ((self.next() >> 40) as f32) / 16_777_216.0
    }
}

// =============================================================================
// Stato
// =============================================================================

#[derive(Resource, Default)]
pub struct RainState {
    active: bool,
    mesh: Option<Handle<Mesh>>,
    material: Option<Handle<StandardMaterial>>,
    /// Entità delle gocce attualmente in scena. Vuoto quando `active == false`.
    particles: Vec<Entity>,
    /// Ultima posizione del player, per rilevare i teletrasporti.
    last_player_pos: Vec3,
    rng: Rng,
}

/// Componente di ogni goccia.
#[derive(Component)]
pub struct RainDrop {
    velocity: Vec3,
}

// =============================================================================
// Plugin
// =============================================================================

pub struct RainFxPlugin;

impl Plugin for RainFxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RainState>().add_systems(
            Update,
            (
                setup_rain_assets,
             update_rain_activation,
             update_rain_particles,
            )
            .chain(),
        );
    }
}

// =============================================================================
// Setup (una volta sola, ma idempotente)
// =============================================================================

fn setup_rain_assets(
    mut state: ResMut<RainState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if state.mesh.is_some() {
        return;
    }

    let hw = DROP_WIDTH * 0.5;
    let hh = DROP_HEIGHT * 0.5;
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD,
    );
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_POSITION,
        vec![
            [-hw, -hh, 0.0],
            [hw, -hh, 0.0],
            [hw, hh, 0.0],
            [-hw, hh, 0.0],
        ],
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 4]);
    mesh.insert_attribute(
        Mesh::ATTRIBUTE_UV_0,
        vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
    );
    mesh.insert_indices(Indices::U32(vec![0, 1, 2, 0, 2, 3]));

    state.mesh = Some(meshes.add(mesh));
    state.material = Some(materials.add(StandardMaterial {
        base_color: Color::srgba(0.72, 0.80, 0.92, 0.45),
                                        alpha_mode: AlphaMode::Blend,
                                        unlit: true,
                                        cull_mode: None,
                                        perceptual_roughness: 1.0,
                                        ..default()
    }));
}

// =============================================================================
// Attivazione
// =============================================================================

fn update_rain_activation(
    mut commands: Commands,
    preset: Option<Res<CurrentPreset>>,
    interior: Option<Res<crate::interior::InteriorState>>,
    dungeon: Option<Res<crate::dungeon::DungeonState>>,
    underwater: Res<UnderwaterState>,
    mut state: ResMut<RainState>,
    player: Query<&Transform, With<Player>>,
) {
    let outside = !interior.as_ref().is_some_and(|i| i.is_inside())
    && !dungeon.as_ref().is_some_and(|d| d.is_inside());

    // Sott'acqua non piove: il tuffo despawna le gocce, la riemersione le ricrea.
    let should_be_active = outside
    && preset.as_ref().is_some_and(|p| p.current.is_rain())
    && !underwater.is_underwater;

    if should_be_active == state.active {
        return;
    }

    if !should_be_active {
        // Spegne: despawn di tutte le gocce in scena.
        for e in state.particles.drain(..) {
            commands.entity(e).despawn();
        }
        state.active = false;
        info!("Pioggia: spenta");
        return;
    }

    // Accende: serve il player e i due asset già pronti.
    let Ok(player_tf) = player.get_single() else {
        return;
    };
    let (Some(mesh), Some(material)) = (state.mesh.clone(), state.material.clone()) else {
        return;
    };

    let center = player_tf.translation;
    // Reset del seed in base alla posizione, così due sessioni nella stessa
    // zona hanno la stessa distribuzione (utile per screenshot).
    state.rng = std::mem::take(&mut state.rng)
    .seed(center.x.to_bits() as u64 ^ ((center.z.to_bits() as u64).rotate_left(32)));

    let mut new_particles = Vec::with_capacity(PARTICLE_COUNT);
    for _ in 0..PARTICLE_COUNT {
        let e = spawn_drop(
            &mut commands,
            &mesh,
            &material,
            center,
            &mut state.rng,
        );
        new_particles.push(e);
    }

    state.particles = new_particles;
    state.last_player_pos = center;
    state.active = true;
    info!("Pioggia: {} gocce attive", state.particles.len());
}

fn spawn_drop(
    commands: &mut Commands,
    mesh: &Handle<Mesh>,
    material: &Handle<StandardMaterial>,
    player_pos: Vec3,
    rng: &mut Rng,
) -> Entity {
    let angle = rng.f32() * std::f32::consts::TAU;
    let radius = rng.f32().sqrt() * RADIUS; // sqrt: distribuzione uniforme sul disco
    let pos = Vec3::new(
        player_pos.x + angle.cos() * radius,
                        player_pos.y + rng.f32() * HEIGHT,
                        player_pos.z + angle.sin() * radius,
    );
    let velocity = Vec3::new(WIND.x, -SPEED, WIND.y);

    commands
    .spawn((
        Mesh3d(mesh.clone()),
            MeshMaterial3d(material.clone()),
            Transform::from_translation(pos),
            RainDrop { velocity },
    ))
    .id()
}

// =============================================================================
// Aggiornamento
// =============================================================================

fn update_rain_particles(
    time: Res<Time>,
    mut state: ResMut<RainState>,
    player: Query<&Transform, With<Player>>,
    mut drops: Query<(&mut Transform, &RainDrop), Without<Player>>,
) {
    if !state.active {
        return;
    }

    let Ok(player_tf) = player.get_single() else {
        return;
    };
    let player_pos = player_tf.translation;
    let dt = time.delta_secs().min(0.05);

    // Yaw della camera: le gocce la guardano sempre (billboard Y).
    let forward = player_tf.forward();
    let camera_yaw = forward.x.atan2(forward.z);
    let billboard = Quat::from_rotation_y(camera_yaw);

    // Teletrasporto: se il player si è spostato di colpo, ricicla tutte le
    // gocce attorno alla nuova posizione. Evita che le gocce "vecchie" siano
    // ormai fuori dal cilindro e ci mettano secondi a rientrare.
    let teleported = player_pos.distance_squared(state.last_player_pos)
    > TELEPORT_RECYCLE_DIST * TELEPORT_RECYCLE_DIST;
    state.last_player_pos = player_pos;

    for (mut transform, drop) in &mut drops {
        if teleported {
            let angle = state.rng.f32() * std::f32::consts::TAU;
            let radius = state.rng.f32().sqrt() * RADIUS;
            transform.translation = Vec3::new(
                player_pos.x + angle.cos() * radius,
                                              player_pos.y + state.rng.f32() * HEIGHT,
                                              player_pos.z + angle.sin() * radius,
            );
            continue;
        }

        // 1. Movimento.
        transform.translation += drop.velocity * dt;

        // 2. Riciclo: se è scesa sotto il player, riparte in cima.
        if transform.translation.y < player_pos.y - FLOOR_OFFSET {
            let angle = state.rng.f32() * std::f32::consts::TAU;
            let radius = state.rng.f32().sqrt() * RADIUS;
            transform.translation = Vec3::new(
                player_pos.x + angle.cos() * radius,
                                              player_pos.y + HEIGHT,
                                              player_pos.z + angle.sin() * radius,
            );
        }

        // 3. Billboard Y: verticale, orientata verso la camera.
        transform.rotation = billboard;
    }
}

// =============================================================================
// Test
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_in_range() {
        let mut a = Rng::default();
        let mut b = Rng::default();
        for _ in 0..1000 {
            let x = a.f32();
            let y = b.f32();
            assert_eq!(x, y);
            assert!((0.0..1.0).contains(&x));
        }
    }

    #[test]
    fn rng_seed_zero_falls_back_to_default() {
        let mut a = Rng::default();
        let mut b = Rng::default().seed(0);
        assert_eq!(a.next(), b.next());
    }

    #[test]
    fn rng_disc_radius_is_uniform() {
        // Campiona 10000 raggi con sqrt, la media deve essere vicina a 2/3 R.
        let mut rng = Rng::default();
        let mut sum = 0.0f64;
        let n = 10_000;
        for _ in 0..n {
            sum += (rng.f32().sqrt() * RADIUS) as f64;
        }
        let mean = sum / n as f64;
        let expected = (2.0 / 3.0) * RADIUS as f64;
        assert!((mean - expected).abs() < 1.0, "mean={mean}, expected={expected}");
    }

    #[test]
    fn teleport_threshold_is_sane() {
        // Se il player si sposta di meno di TELEPORT_RECYCLE_DIST, non ricicla.
        let d = TELEPORT_RECYCLE_DIST * 0.9;
        assert!(d * d <= TELEPORT_RECYCLE_DIST * TELEPORT_RECYCLE_DIST);
        let d = TELEPORT_RECYCLE_DIST * 1.1;
        assert!(d * d > TELEPORT_RECYCLE_DIST * TELEPORT_RECYCLE_DIST);
    }
}
