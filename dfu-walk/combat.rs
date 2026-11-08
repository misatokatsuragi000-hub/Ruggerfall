//! Combattimento base: danno agli NPC, ai follower, ai nemici dungeon, al
//! player e **parata** (player ↔ nemici).
//!
//! ## Direzione dell'attacco (fix: colpire in alto/basso a scelta)
//!
//! In passato `apply_player_strike` calcolava la direzione dell'attacco come
//! `Vec3::new(-player.yaw.sin(), 0.0, -player.yaw.cos())`, cioè **puramente
//! orizzontale**: il `pitch` della camera veniva ignorato. Il risultato era che
//! l'affondo (RMB) colpiva sempre dritto davanti alla camera, e `Y_TOLERANCE`
//! fungeva solo da "tetto passivo": se il nemico era più di 3 m sopra o sotto
//! il piano orizzontale della camera, veniva scartato. Non c'era modo di
//! *scegliere* di colpire in alto o in basso.
//!
//! Ora la direzione è `*ptf.forward()` (che include yaw **e** pitch; in Bevy
//! 0.15 `Transform::forward()` è un `Dir3`, quindi va dereferenziato per
//! ottenere un `Vec3`) e la distanza è quella **3D** (`to.length()`), quindi:
//!
//! - guardando in alto si colpisce in alto (utile per nemici volanti);
//! - guardando in basso si colpisce in basso (utili ai piedi);
//! - il cono `ATTACK_ARC_DEG` attorno allo sguardo scarta i target fuori mira.
//!
//! `Y_TOLERANCE` è rimossa: con il cono 3D la verticalità è già gestita
//! dall'angolo (se guardi in alto e il nemico è a terra, `dir.dot(forward)`
//! è basso e lo scarta da solo).
//!
//! **Nota**: la spada in `fps_weapon.rs` va animata di conseguenza (segui
//! `player.pitch` durante RMB), altrimenti la collisione punta in alto mentre
//! la mesh resta in basso. Vedi il commento in `update_weapon_pose`.
//!
//! ## Colpi mirati con la punta della spada (affondo, power attack, dall'alto)
//!
//! Durante l'affondo (`StrikeKind::Thrust`) la punta della spada (angolo in
//! alto a sinistra della texture) segue il cursore su **tutto lo schermo**
//! (vedi `fps_weapon::update_weapon_pose`). La direzione di quella punta, in
//! spazio camera, viene pubblicata nella resource `SwordTip`; per power
//! attack (MMB) e fendente dall'alto la direzione è quella del **cursore**.
//! Per questi tre colpi (`StrikeKind::uses_tip`) il colpo parte lungo quel
//! raggio: viene colpito solo chi si trova **sotto la punta**, cioè dentro un
//! tubo sottile attorno al raggio (raggio
//! `TIP_HIT_RADIUS_MIN + TIP_HIT_RADIUS_PER_M * distanza`), entro
//! `ATTACK_REACH` dal player. Il raggio è 3D (segue anche il pitch della
//! camera), quindi i **nemici volanti in alto** si colpiscono portando il
//! cursore su di loro. Il corpo del target viene controllato a più altezze
//! (`TARGET_SAMPLE_HEIGHTS`, coprono pivot ai piedi e al centro) e vince il
//! più vicino al raggio. Se la punta punta altrove, nessun colpo. Solo il
//! fendente laterale (RMB+LMB) resta nel cono centrato sullo sguardo.
//!
//! ## Parata del player (locazionale)
//!
//! Quando un nemico colpisce il player (`EnemyAttackEvent`) il danno **non
//! è immediato**:
//!
//! 1. se il nemico è nel **cono frontale** (`PARRY_ARC_DEG`), appare un
//!    **alone rosso sfumato** (`HALO_VISUAL_PX`) in un punto dello schermo
//!    (`ImageNode` con gradiente radiale, alpha che cresce con smoothstep);
//! 2. dopo `PARRY_TELEGRAPH_SECS` (1 s) il colpo "atterra" in quel punto;
//! 3. se in quel momento la **texture dell'arma** copre il punto
//!    (`fps_weapon::WeaponCover::covers`, alpha-aware) e la stamina è
//!    `>= PARRY_STAMINA_COST`, la parata riesce: stamina consumata, danno
//!    azzerato (`PARRY_DAMAGE_MULT = 0.0`), `PlayerParried` emesso (suono +
//!    flash bianco); altrimenti danno pieno e `PlayerDamaged`.
//!
//! Colpi **da dietro** (fuori dal cono) non hanno alone e colpiscono subito.
//! Con `PARRY_REQUIRES_LMB = true` serve anche LMB tenuto premuto.
//!
//! ## Parata dei nemici (delegata a `DungeonEnemy::try_parry`)
//!
//! Il calcolo della parata è **centralizzato** in `DungeonEnemy::try_parry`
//! (vedi `dungeon_enemies.rs`). `apply_player_strike` si limita a:
//!
//! 1. calcolare la direzione "player → nemico" per capire se il nemico è
//!    rivolto verso il player (cono `ENEMY_PARRY_ARC_DEG`);
//! 2. chiamare `enemy.try_parry(facing_player)`;
//! 3. se il nemico para, ridurre il danno di `ENEMY_PARRY_DAMAGE_MULT` (25%)
//!    ed emettere `EnemyParried`; altrimenti danno pieno.
//!
//! ## Feedback visivo (`damage_fx.rs`)
//!
//! Per **ogni colpo del player** che trova un target valido (NPC o nemico
//! dungeon, parato o no) viene emesso `FloatingDamageEvent { world_pos,
//! damage, parried }`. `damage_fx::spawn_floating_damage` proietta
//! `world_pos` sullo schermo e spawna un numero fluttuante: rosso se il
//! colpo è pieno, arancione se è stato parato. Se il target è dietro la
//! camera o fuori schermo, il numero appare clampato ai bordi.
//!
//! Il flash bianco della parata è gestito direttamente ascoltando
//! `PlayerParried` e `EnemyParried` (nessun evento nuovo).
//!
//! ## Suoni
//!
//! - `play_parry_sound` ← `PlayerParried` → `music/parry.mp3`
//! - `play_hit_sound`   ← `PlayerHit`     → `music/hit.mp3`
//!
//! ## HP
//!
//! | Categoria | HP |
//! |---|---|
//! | Civile (`NpcWalker`, `InteriorNpc`) | 20 |
//! | Follower | 50 |
//! | Nemico dungeon | `DungeonEnemy.health` (specie) |
//!
//! ## Danno per tipo di attacco
//!
//! | Tipo | Danno |
//! |---|---|
//! | Affondo (`RMB`) | 10 |
//! | Fendente (`RMB+LMB`) | 15 |
//! | Fendente dall'alto (`RMB+LMB`, cursore in alto) | 20 |
//! | Power attack (`MMB`) | 25 |

use bevy::prelude::*;
use bevy::render::render_asset::RenderAssetUsages;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::window::PrimaryWindow;

use crate::damage_fx::FloatingDamageEvent;
use crate::dungeon_enemies::{
    DamageDungeonEnemy, DungeonEnemy, EnemyAttackEvent, ENEMY_PARRY_ARC_DEG,
    ENEMY_PARRY_DAMAGE_MULT,
};
use crate::fps_weapon::WeaponCover;
use crate::interior::{CustomNpcName, InteriorNpc};
use crate::main_quest::{QbnFoe, QbnInput};
use crate::player::Player;
use crate::tiles::{Follower, NpcWalker};
use crate::vitals::{PlayerDamaged, PlayerVitals};

// =========================================================================
// Costanti di combattimento
// =========================================================================
pub const ATTACK_REACH: f32 = 2.5;
pub const ATTACK_ARC_DEG: f32 = 90.0;

/// Zona colpita dall'affondo attorno al raggio della punta: tubo di raggio
/// `TIP_HIT_RADIUS_MIN + TIP_HIT_RADIUS_PER_M * distanza_lungo_il_raggio`
/// (metri). Più piccoli = serve mirare con più precisione.
pub const TIP_HIT_RADIUS_MIN: f32 = 0.3;
pub const TIP_HIT_RADIUS_PER_M: f32 = 0.1;

/// Altezze (metri rispetto al pivot del target) a cui si controlla se il
/// corpo è sotto la punta. Coprono sia pivot ai piedi (nemici a terra) sia
/// al centro del corpo (nemici volanti): basta che un punto cada nel tubo.
const TARGET_SAMPLE_HEIGHTS: [f32; 5] = [-0.4, 0.0, 0.6, 1.2, 1.8];

/// Direzione della punta della spada **in spazio camera/player** (versore,
/// -Z = avanti), scritta ogni frame da `fps_weapon::update_weapon_pose`
/// finché l'affondo RMB è attivo. `None` = mira standard (sguardo).
#[derive(Resource, Default)]
pub struct SwordTip {
    pub dir: Option<Vec3>,
}

// `Y_TOLERANCE` rimossa: la direzione dell'attacco ora è 3D (segue il pitch
// della camera), quindi la verticalità è gestita da `ATTACK_ARC_DEG` stesso.
// Se guardi in alto, `*ptf.forward()` ha Y > 0 e `dir.dot(forward)` scarta i
// target a terra. Se guardi in basso, il contrario. La vecchia costante
// tagliava in modo simmetrico ±3 m dal piano orizzontale e non permetteva
// di scegliere.
//
// pub const Y_TOLERANCE: f32 = 3.0;

// =========================================================================
// Costanti della parata del player
// =========================================================================
pub const PARRY_ARC_DEG: f32 = 120.0;
pub const PARRY_DAMAGE_MULT: f32 = 0.0;
pub const PARRY_STAMINA_COST: f32 = 3.0;

/// Secondi tra la comparsa dell'alone e il colpo reale.
pub const PARRY_TELEGRAPH_SECS: f32 = 1.0;

/// Diametro visivo dell'alone, in pixel logici. Il gradiente ha alpha 0 al
/// bordo, quindi la parte "piena" è circa `HALO_VISUAL_PX * 0.5`; per
/// avere un alone leggibile ma non invadente 130 px va bene.
pub const HALO_VISUAL_PX: f32 = 130.0;

/// Alpha massima del tint rosso applicato all'alone (raggiunta a fine
/// telegraph, prima del colpo).
const HALO_RED: [f32; 3] = [1.0, 0.08, 0.08];
const HALO_ALPHA_MIN: f32 = 0.0;
const HALO_ALPHA_MAX: f32 = 0.85;

/// Se `true` la parata richiede anche LMB tenuto premuto.
pub const PARRY_REQUIRES_LMB: bool = false;

/// Zona dello schermo (NDC, y verso l'alto) in cui può comparire l'alone:
/// quasi tutto lo schermo, con un margine per non far uscire l'alone
/// (`HALO_VISUAL_PX`) dai bordi.
const HALO_NDC_X: (f32, f32) = (-0.85, 0.85);
const HALO_NDC_Y: (f32, f32) = (-0.70, 0.80);

// =========================================================================
// Suoni (relativi a `assets/`)
// =========================================================================
const PARRY_SOUND: &str = "music/parry.mp3";
const HIT_SOUND: &str = "music/hit.mp3";

// =========================================================================
// HP
// =========================================================================
#[derive(Component, Clone, Debug)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

impl Health {
    pub fn new(max: f32) -> Self {
        Self { current: max, max }
    }
    pub fn civilian() -> Self {
        Self::new(20.0)
    }
    pub fn follower() -> Self {
        Self::new(50.0)
    }
    pub fn is_dead(&self) -> bool {
        self.current <= 0.0
    }
}

// =========================================================================
// Eventi
// =========================================================================
#[derive(Event, Debug, Clone, Copy)]
pub struct PlayerStrike {
    pub kind: StrikeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrikeKind {
    Thrust,
    Swing,
    PowerAttack,
    /// Fendente dall'alto (LMB+RMB con il cursore in alto).
    Overhead,
}

impl StrikeKind {
    pub fn damage(self) -> f32 {
        match self {
            StrikeKind::Thrust => 10.0,
            StrikeKind::Swing => 15.0,
            StrikeKind::PowerAttack => 25.0,
            StrikeKind::Overhead => 20.0,
        }
    }
    /// `true` per i colpi mirati con la punta/cursore (`SwordTip`); il
    /// fendente laterale (`Swing`) resta nel cono attorno allo sguardo.
    pub fn uses_tip(self) -> bool {
        matches!(
            self,
            StrikeKind::Thrust | StrikeKind::PowerAttack | StrikeKind::Overhead
        )
    }
    pub fn label(self) -> &'static str {
        match self {
            StrikeKind::Thrust => "affondo",
            StrikeKind::Swing => "fendente",
            StrikeKind::PowerAttack => "power attack",
            StrikeKind::Overhead => "fendente dall'alto",
        }
    }
}

#[derive(Event, Debug, Clone, Copy)]
pub struct PlayerParried {
    pub amount: f32,
}

/// Emesso quando un colpo del player trova un target valido. Consumato da
/// `play_hit_sound`. Non viene emesso sui colpi a vuoto.
#[derive(Event, Debug, Clone, Copy)]
pub struct PlayerHit;

/// Emesso quando un **nemico** para il colpo del player. Consumato da
/// `play_hit_sound` per riprodurre comunque il "clang" metallico, e da
/// `damage_fx::handle_parry_flash` per il flash bianco.
#[derive(Event, Debug, Clone, Copy)]
pub struct EnemyParried {
    pub enemy: Entity,
}

/// Colpo nemico in attesa di essere risolto (parata locazionale).
/// Se l'entità ha anche un `ImageNode` UI, quello è l'alone radiale.
#[derive(Component)]
struct PendingEnemyStrike {
    remaining: f32,
    damage: f32,
    mobile_id: String,
    /// Punto bersaglio in NDC; `None` = colpo non parabile (alle spalle).
    target_ndc: Option<Vec2>,
}

// =========================================================================
// Alone radiale: texture riutilizzabile
// =========================================================================

/// Handle della texture radiale (bianca al centro, trasparente al bordo).
/// Il tint rosso viene applicato da `ImageNode.color`, non dalla texture.
#[derive(Resource)]
pub struct ParryHaloTexture(pub Handle<Image>);

/// Genera una texture RGBA `size × size` con gradiente radiale: alpha 1 al
/// centro, alpha 0 al bordo, caduta `smoothstep` per un alone morbido senza
/// scalini. Il colore è bianco puro (modulabile via `ImageNode.color`).
fn generate_radial_halo(size: u32) -> Image {
    let s = size as usize;
    let mut data = Vec::with_capacity(s * s * 4);
    let c = (s as f32 - 1.0) * 0.5;
    for y in 0..s {
        for x in 0..s {
            let dx = (x as f32 - c) / c;
            let dy = (y as f32 - c) / c;
            let r = (dx * dx + dy * dy).sqrt();
            let a = if r >= 1.0 {
                0.0
            } else {
                let t = 1.0 - r;
                t * t * (3.0 - 2.0 * t)
            };
            data.extend_from_slice(&[255, 255, 255, (a * 255.0) as u8]);
        }
    }
    let mut image = Image::new(
        Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::default(),
    );
    // Gradiente morbido: sampling lineare, clamp (l'alpha 0 al bordo evita
    // bleed indesiderato già di suo, ma clamp è più sicuro se la texture
    // viene scalata).
    image.sampler = bevy::image::ImageSampler::linear();
    image
}

fn setup_parry_halo(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    commands.insert_resource(ParryHaloTexture(images.add(generate_radial_halo(128))));
}

// =========================================================================
// Plugin
// =========================================================================
pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SwordTip>()
        .add_event::<PlayerStrike>()
        .add_event::<PlayerParried>()
        .add_event::<PlayerHit>()
        .add_event::<EnemyParried>()
        .add_event::<PlayerDamaged>()
        .add_event::<DamageDungeonEnemy>()
        .add_systems(Startup, setup_parry_halo)
        .add_systems(
            Update,
            (
                ensure_health,
             apply_player_strike.after(crate::fps_weapon::emit_strikes),
             telegraph_enemy_attacks
             .after(crate::fps_weapon::update_weapon_input),
             resolve_pending_strikes
             .after(crate::fps_weapon::update_weapon_pose),
             play_parry_sound,
             play_hit_sound,
            )
            .chain(),
        );
    }
}

// =========================================================================
// Sistemi
// =========================================================================

fn ensure_health(
    mut commands: Commands,
    walkers: Query<Entity, (With<NpcWalker>, Without<Health>)>,
                 interior_npcs: Query<Entity, (With<InteriorNpc>, Without<Health>)>,
                 followers: Query<Entity, (With<Follower>, Without<Health>)>,
) {
    for e in &walkers {
        commands.entity(e).insert(Health::civilian());
    }
    for e in &interior_npcs {
        commands.entity(e).insert(Health::civilian());
    }
    for e in &followers {
        commands.entity(e).insert(Health::follower());
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_player_strike(
    mut strikes: EventReader<PlayerStrike>,
    tip: Res<SwordTip>,
    player: Query<(&Player, &Transform)>,
                       mut targets: Query<(
                           Entity,
                           &GlobalTransform,
                           Option<&mut Health>,
                           Option<&CustomNpcName>,
                           Option<&Follower>,
                           Option<&NpcWalker>,
                           Option<&InteriorNpc>,
                           Option<&mut DungeonEnemy>,
                           Option<&QbnFoe>,
                       )>,
                       mut commands: Commands,
                       mut enemy_damage: EventWriter<DamageDungeonEnemy>,
                       mut player_hit: EventWriter<PlayerHit>,
                       mut enemy_parried: EventWriter<EnemyParried>,
                       mut floating_damage: EventWriter<FloatingDamageEvent>,
                       mut qbn_input: EventWriter<QbnInput>,
) {
    let Ok((player, ptf)) = player.get_single() else {
        for _ in strikes.read() {}
        return;
    };

    // Direzione dell'attacco in **3D**: `forward()` combina yaw e pitch della
    // camera, quindi segue lo sguardo. Guardando in alto colpisci in alto,
    // guardando in basso colpisci in basso.
    //
    // NOTA: in Bevy 0.15 `Transform::forward()` restituisce `Dir3`, non
    // `Vec3`. `Vec3::dot()` vuole un `Vec3`, quindi dereferenziamo con `*`
    // (Dir3 implementa `Deref<Target = Vec3>`).
    let forward: Vec3 = *ptf.forward();
    let player_pos = ptf.translation;
    let arc_cos = (ATTACK_ARC_DEG.to_radians() * 0.5).cos();
    let enemy_parry_arc_cos = (ENEMY_PARRY_ARC_DEG.to_radians() * 0.5).cos();

    for strike in strikes.read() {
        // Affondo, power attack e dall'alto: mira lungo la punta/cursore.
        let tip_dir: Option<Vec3> = strike.kind.uses_tip()
        .then_some(tip.dir)
        .flatten()
        .map(|d| ptf.rotation * d);

        let mut best: Option<(f32, Entity, bool)> = None;
        for (entity, gtf, health, _, _, _, _, dungeon_enemy, _qbn_foe) in targets.iter_mut() {
            if let Some(h) = &health {
                if h.is_dead() {
                    continue;
                }
            }
            if health.is_none() && dungeon_enemy.is_none() {
                continue;
            }

            let pos = gtf.translation();
            let to = pos - player_pos;

            // Distanza **3D** reale, non proiezione XZ: se guardi in alto e il
            // nemico è in alto, la distanza euclidea è corretta.
            let dist = to.length();
            if dist > ATTACK_REACH {
                continue;
            }

            // `score`: più basso = meglio.
            let score = if let Some(aim) = tip_dir {
                // Mira con la punta: il target è colpito solo se un punto del
                // suo corpo cade nel tubo attorno al raggio; score = il punto
                // più vicino al raggio.
                let mut best_perp: Option<f32> = None;
                for h in TARGET_SAMPLE_HEIGHTS {
                    let p = to + Vec3::Y * h;
                    let along = p.dot(aim);
                    if along <= 0.0 {
                        continue;
                    }
                    let perp = (p - aim * along).length();
                    if perp <= TIP_HIT_RADIUS_MIN + TIP_HIT_RADIUS_PER_M * along {
                        best_perp = Some(best_perp.map_or(perp, |b| b.min(perp)));
                    }
                }
                match best_perp {
                    Some(p) => p,
                    None => continue,
                }
            } else {
                // Cono attorno allo sguardo in 3D. Senza il vecchio check
                // `to.y.abs() > Y_TOLERANCE`: la verticalità è già filtrata da
                // `dir.dot(forward) < arc_cos`.
                if dist > 0.1 {
                    let dir = to / dist;
                    if dir.dot(forward) < arc_cos {
                        continue;
                    }
                }
                dist
            };

            let is_enemy = dungeon_enemy.is_some();
            if best.map_or(true, |(bs, _, _)| score < bs) {
                best = Some((score, entity, is_enemy));
            }
        }

        let Some((_, entity, is_enemy)) = best else {
            info!(
                "Colpo {}: nessun target entro {:.1} m",
                strike.kind.label(),
                  ATTACK_REACH
            );
            continue;
        };

        player_hit.send(PlayerHit);

        if is_enemy {
            let Ok((_, gtf, _, _, _, _, _, dungeon_enemy_opt, _qbn_foe)) = targets.get_mut(entity) else {
                continue;
            };
            let Some(mut enemy) = dungeon_enemy_opt else { continue };

            let enemy_pos = gtf.translation();
            // La parata del nemico resta su XZ: il nemico è rivolto verso il
            // player sul piano orizzontale, non in 3D (non ha "pitch").
            let to_player = Vec2::new(player_pos.x - enemy_pos.x, player_pos.z - enemy_pos.z);
            let to_player_dir = to_player.normalize_or_zero();
            let facing_player = enemy.heading.dot(to_player_dir) >= enemy_parry_arc_cos;

            let original = strike.kind.damage() as i32;
            let parried = enemy.try_parry(facing_player);
            let effective = if parried {
                let reduced = ((original as f32) * ENEMY_PARRY_DAMAGE_MULT).round() as i32;
                let reduced = reduced.max(1);
                let level = crate::dungeon_enemies::SPECIES
                .get(enemy.species)
                .map(|s| s.level)
                .unwrap_or(1);
                info!(
                    "Nemico (liv {}) para il colpo! Danno {} → {} (stamina {:.0}/{:.0})",
                      level,
                      original,
                      reduced,
                      enemy.parry_stamina,
                      crate::dungeon_enemies::ENEMY_PARRY_STAMINA_MAX
                );
                enemy_parried.send(EnemyParried { enemy: entity });
                reduced
            } else {
                original
            };

            floating_damage.send(FloatingDamageEvent {
                world_pos: enemy_pos,
                damage: effective,
                parried,
            });

            enemy_damage.send(DamageDungeonEnemy {
                enemy: entity,
                amount: effective,
            });
            info!(
                "Colpo {} su nemico dungeon: -{} HP",
                strike.kind.label(),
                  effective
            );
        } else {
            let Ok((_, gtf, health, custom, follower, walker, interior, _, qbn_foe)) =
            targets.get_mut(entity)
            else {
                continue;
            };
            let Some(mut health) = health else { continue };
            let name = custom
            .map(|c| c.name.clone())
            .or_else(|| follower.map(|_| "follower".to_string()))
            .or_else(|| walker.map(|_| "civile".to_string()))
            .or_else(|| interior.map(|_| "NPC interno".to_string()))
            .unwrap_or_else(|| "sconosciuto".to_string());

            let dmg = strike.kind.damage();
            health.current -= dmg;

            // Nemico di quest: il runtime QBN vuole sapere che è stato ferito
            // (`injured _F.00_ saying 1025`).
            if let Some(qf) = qbn_foe {
                qbn_input.send(QbnInput::FoeInjured(qf.symbol.clone()));
            }

            floating_damage.send(FloatingDamageEvent {
                world_pos: gtf.translation(),
                                 damage: dmg as i32,
                                 parried: false,
            });

            info!(
                "Colpo {} su {}: -{:.0} HP → {:.0}/{:.0}",
                strike.kind.label(),
                  name,
                  dmg,
                  health.current,
                  health.max
            );

            if health.is_dead() {
                info!("{} è stato ucciso!", name);
                commands.entity(entity).despawn_recursive();
            }
        }
    }
}

/// Piccolo xorshift (evita la dipendenza da `rand`). Ritorna [0, 1).
fn next_rand(state: &mut u64) -> f32 {
    if *state == 0 {
        *state = 0x9E37_79B9_7F4A_7C15;
    }
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    ((*state >> 40) as f32) / ((1u64 << 24) as f32)
}

/// Intercetta `EnemyAttackEvent`: invece di applicare subito il danno crea
/// un `PendingEnemyStrike` (con alone radiale se il colpo è parabile).
fn telegraph_enemy_attacks(
    mut attacks: EventReader<EnemyAttackEvent>,
    mut commands: Commands,
    vitals: Option<Res<PlayerVitals>>,
    player_q: Query<&Player>,
    windows: Query<&Window, With<PrimaryWindow>>,
    time: Res<Time>,
    halo_tex: Res<ParryHaloTexture>,
    mut rng: Local<u64>,
) {
    if vitals.is_none() {
        for _ in attacks.read() {}
        return;
    }

    let forward = player_q
    .get_single()
    .map(|p| Vec3::new(-p.yaw.sin(), 0.0, -p.yaw.cos()))
    .unwrap_or(Vec3::Z);
    let parry_arc_cos = (PARRY_ARC_DEG.to_radians() * 0.5).cos();
    let win = windows
    .get_single()
    .ok()
    .map(|w| Vec2::new(w.width(), w.height()));

    *rng ^= time.elapsed().subsec_nanos() as u64;

    for ev in attacks.read() {
        let in_front = forward.dot(ev.direction) >= parry_arc_cos;

        let target_ndc = match (in_front, win) {
            (true, Some(_)) => Some(Vec2::new(
                HALO_NDC_X.0 + next_rand(&mut rng) * (HALO_NDC_X.1 - HALO_NDC_X.0),
                                              HALO_NDC_Y.0 + next_rand(&mut rng) * (HALO_NDC_Y.1 - HALO_NDC_Y.0),
            )),
            _ => None,
        };

        let mut entity = commands.spawn(PendingEnemyStrike {
            remaining: if target_ndc.is_some() { PARRY_TELEGRAPH_SECS } else { 0.0 },
                                        damage: ev.damage as f32,
                                        mobile_id: ev.mobile_id.to_string(),
                                        target_ndc,
        });

        if let (Some(ndc), Some(size)) = (target_ndc, win) {
            let cx = (ndc.x * 0.5 + 0.5) * size.x;
            let cy = (0.5 - ndc.y * 0.5) * size.y;
            let half = HALO_VISUAL_PX * 0.5;
            entity.insert((
                Name::new("Parry Halo"),
                           ImageNode {
                               image: halo_tex.0.clone(),
                           // L'alpha cresce in `resolve_pending_strikes`.
                           color: Color::srgba(HALO_RED[0], HALO_RED[1], HALO_RED[2], HALO_ALPHA_MIN),
                           ..default()
                           },
                           Node {
                               position_type: PositionType::Absolute,
                               left: Val::Px(cx - half),
                           top: Val::Px(cy - half),
                           width: Val::Px(HALO_VISUAL_PX),
                           height: Val::Px(HALO_VISUAL_PX),
                           ..default()
                           },
                           GlobalZIndex(50),
            ));
            info!(
                "Colpo di mobile {} in arrivo: alone a NDC ({:.2}, {:.2}), {:.1}s",
                  ev.mobile_id, ndc.x, ndc.y, PARRY_TELEGRAPH_SECS
            );
        }
    }
}

/// Fa avanzare i colpi in attesa; allo scadere decide parata/danno in base
/// a se l'arma copre il punto dell'alone.
#[allow(clippy::too_many_arguments)]
fn resolve_pending_strikes(
    time: Res<Time>,
    mut commands: Commands,
    mut pending: Query<(Entity, &mut PendingEnemyStrike, Option<&mut ImageNode>)>,
                           mut vitals: Option<ResMut<PlayerVitals>>,
                           cover: WeaponCover,
                           mut damaged: EventWriter<PlayerDamaged>,
                           mut parried: EventWriter<PlayerParried>,
                           mut was_dead: Local<bool>,
) {
    let Some(vitals) = vitals.as_deref_mut() else {
        for (e, _, _) in &pending {
            commands.entity(e).despawn_recursive();
        }
        return;
    };

    let dt = time.delta_secs();

    for (entity, mut strike, img) in &mut pending {
        strike.remaining -= dt;

        if strike.remaining > 0.0 {
            if let Some(mut img) = img {
                // smoothstep: all'inizio invisibile, a fine telegraph
                // `HALO_ALPHA_MAX`. Nessun bordo netto: la texture ha già
                // alpha 0 al bordo del cerchio.
                let t = (1.0 - strike.remaining / PARRY_TELEGRAPH_SECS).clamp(0.0, 1.0);
                let a = t * t * (3.0 - 2.0 * t);
                let alpha = HALO_ALPHA_MIN + (HALO_ALPHA_MAX - HALO_ALPHA_MIN) * a;
                img.color = Color::srgba(HALO_RED[0], HALO_RED[1], HALO_RED[2], alpha);
            }
            continue;
        }

        // --- Il colpo atterra ---
        commands.entity(entity).despawn_recursive();

        let original = strike.damage;
        let mut effective = original;
        let mut parried_ok = false;

        match strike.target_ndc {
            None => info!("Parata impossibile: colpo alle spalle"),
            Some(ndc) => {
                let covered = cover.covers(ndc) && (!PARRY_REQUIRES_LMB || cover.lmb_held());
                if !covered {
                    info!("Parata fallita: l'arma non copriva ({:.2}, {:.2})", ndc.x, ndc.y);
                } else if vitals.stamina < PARRY_STAMINA_COST {
                    info!(
                        "Parata fallita: stamina insufficiente ({:.1}/{:.1}, serve {:.0})",
                          vitals.stamina, vitals.max_stamina, PARRY_STAMINA_COST
                    );
                } else {
                    vitals.stamina -= PARRY_STAMINA_COST;
                    effective = (original * PARRY_DAMAGE_MULT).max(0.0);
                    parried_ok = true;
                    info!(
                        "Parata riuscita: danno bloccato (stamina -{:.0} → {:.0}/{:.0})",
                          PARRY_STAMINA_COST, vitals.stamina, vitals.max_stamina
                    );
                    parried.send(PlayerParried { amount: effective });
                }
            }
        }

        if effective > 0.0 {
            vitals.health = (vitals.health - effective).max(0.0);
            info!(
                "Nemico (mobile {}) ti colpisce: -{:.1} HP → {:.1}/{:.1}",
                  strike.mobile_id, effective, vitals.health, vitals.max_health
            );
            damaged.send(PlayerDamaged { amount: effective });
        } else if parried_ok {
            info!("Colpo completamente parato: 0 danni");
        }

        if vitals.health <= 0.0 && !*was_dead {
            info!("Sei morto!");
            *was_dead = true;
        }
        if vitals.health > 0.0 {
            *was_dead = false;
        }
    }
}

fn play_parry_sound(
    mut events: EventReader<PlayerParried>,
    assets: Res<AssetServer>,
    mut commands: Commands,
    mut handle: Local<Option<Handle<AudioSource>>>,
) {
    let count = events.read().count();
    if count == 0 {
        return;
    }

    let h = handle.get_or_insert_with(|| assets.load(PARRY_SOUND));

    for _ in 0..count {
        commands.spawn((
            AudioPlayer::new(h.clone()),
                        PlaybackSettings::DESPAWN,
        ));
    }
}

/// Suona `hit.mp3` quando un colpo del player trova un target valido
/// (NPC o `DungeonEnemy`, indipendentemente dal fatto che il nemico abbia
/// parato: il "clang" metallico è lo stesso).
fn play_hit_sound(
    mut events: EventReader<PlayerHit>,
    assets: Res<AssetServer>,
    mut commands: Commands,
    mut handle: Local<Option<Handle<AudioSource>>>,
) {
    let count = events.read().count();
    if count == 0 {
        return;
    }

    let h = handle.get_or_insert_with(|| assets.load(HIT_SOUND));

    for _ in 0..count {
        commands.spawn((
            AudioPlayer::new(h.clone()),
                        PlaybackSettings::DESPAWN,
        ));
    }
}
