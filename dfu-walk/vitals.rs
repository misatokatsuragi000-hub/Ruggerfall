//! Interfaccia HUD minimale: 3 barre verticali al bordo sinistro (Salute, Stamina, Mana)
//! senza sfondo né scritte, con riempimento dal basso verso l'alto.
//!
//! ## Flash rosso quando il player è ferito
//!
//! Quando un nemico colpisce il player, `combat.rs::apply_enemy_damage_to_player`
//! emette un `PlayerDamaged { amount }`. `vitals.rs` lo consuma in
//! `handle_player_damaged` e alza `DamageFlash::alpha` a `DAMAGE_FLASH_MAX`
//! (default 0.35). `update_damage_flash` la fa scendere linearmente in
//! `DAMAGE_FLASH_DURATION_SECS` secondi e nasconde l'overlay a zero.
//!
//! L'overlay è un `Node` a tutto schermo con `BackgroundColor` rossa
//! trasparente, `GlobalZIndex(100)` (sopra le barre, sotto eventuali menu
//! egui). Non è un'immagine: un rettangolo colorato basta ed è gratis.
//!
//! ## Layout (perché le colonne sono `Absolute`)
//!
//! Ogni colonna è posizionata con `position_type: Absolute` e `left`/`bottom`
//! **espliciti**, dentro un container a tutto schermo che fa solo da radice di
//! z-index. Non c'è `gap` né `flex_direction` sul container.
//!
//! Motivo: con un layout flex, in Bevy 0.15 il `bottom` sulla radice sposta
//! le colonne (che sono in flusso) ma i `track` (che sono `Absolute`) possono
//! risultare ancorati alla radice invece che alla colonna, a seconda
//! dell'ordine di layout. Il risultato è che `HUD_BOTTOM_PX` sposta le PNG
//! ma non i fill colorati. Con `Absolute` esplicito su ogni colonna, ogni
//! barra è un blocco indipendente: `HUD_BOTTOM_PX` sposta la colonna **e** il
//! fill, `BAR_GAP_PX` cambia il `left` di ogni colonna **e** il fill segue,
//! `COLUMN_FILL_TOP_PX` sposta **solo il fill** dentro la colonna.
//!
//! ## Costanti di layout
//!
//! | costante | cosa sposta |
//! |---|---|
//! | `HUD_LEFT_PX` | tutto il gruppo, orizzontale |
//! | `HUD_BOTTOM_PX` | tutto il gruppo, verticale (colonne + fill) |
//! | `BAR_GAP_PX` | distanza tra le colonne (colonne + fill) |
//! | `COLUMN_W_PX` / `COLUMN_H_PX` | dimensione della colonna (e del bg) |
//! | `COLUMN_FILL_LEFT_PX` / `COLUMN_FILL_TOP_PX` | posizione del fill **dentro** la colonna |
//! | `BAR_WIDTH_PX` / `BAR_HEIGHT_PX` | dimensione del fill |
//!
//! Per **sollevare tutto** (colonne + fill), aumenta `HUD_BOTTOM_PX`.
//! Per **sollevare solo i fill** dentro le colonne, riduci `COLUMN_FILL_TOP_PX`.
//!
//! ## Bordo a colonna ionica (opzionale)
//!
//! Se `assets/textures/column.png` esiste, ogni barra è incorniciata da una
//! **colonna ionica**: la PNG viene mostrata come sfondo, e la barra colorata
//! (fill) viene disegnata dentro il fusto della colonna, scorrendo
//! verticalmente. Se la PNG manca, si usa il **bordo dorato procedurale a 3
//! strati**.
//!
//! ## Rigenerazione della stamina
//!
//! `PlayerVitals::stamina` viene rigenerata automaticamente da `regen_stamina`:
//!
//! 1. Un attacco (`fps_weapon.rs`) scrive `vitals.stamina -= costo`.
//! 2. `regen_stamina` se ne accorge al frame successivo (la stamina è diminuita)
//!    e azzera il timer `idle`.
//! 3. Trascorsi `STAMINA_REGEN_DELAY_SECS` senza altri consumi, la stamina
//!    cresce di `STAMINA_REGEN_PER_SEC * dt` fino a `max_stamina`.
//!
//! Con i default: un affondo da 5 si recupera in 1 s + 1 s di pausa = 2 s;
//! un power attack da 10 in ~2 s + 1 s = 3 s.

use bevy::prelude::*;

// =============================================================================
// Layout HUD
// =============================================================================
const HUD_LEFT_PX: f32 = 10.0;
const HUD_BOTTOM_PX: f32 = 0.0;

// =============================================================================
// Dimensioni del fill
// =============================================================================
const BAR_WIDTH_PX: f32 = 14.0;
const BAR_HEIGHT_PX: f32 = 110.0;
const BAR_GAP_PX: f32 = 0.0;

// =============================================================================
// Colori delle barre
// =============================================================================
const COLOR_HEALTH: Color = Color::srgb(0.85, 0.15, 0.15);
const COLOR_STAMINA: Color = Color::srgb(0.20, 0.75, 0.25);
const COLOR_MANA: Color = Color::srgb(0.18, 0.48, 0.90);

// =============================================================================
// Colonna ionica (PNG opzionale)
// =============================================================================
const COLUMN_PNG: &str = "textures/column.png";
const COLUMN_PNG_FS: &str = "assets/textures/column.png";

const COLUMN_W_PX: f32 = 33.0;
const COLUMN_H_PX: f32 = 140.0;

const COLUMN_FILL_LEFT_PX: f32 = 10.0;
const COLUMN_FILL_TOP_PX: f32 = 15.0;

// =============================================================================
// Bordo dorato procedurale (fallback quando manca la PNG)
// =============================================================================
const GOLD_SHADOW: Color = Color::srgba(0.10, 0.07, 0.02, 0.95);
const GOLD_DARK: Color = Color::srgba(0.30, 0.20, 0.05, 1.0);
const GOLD_MID: Color = Color::srgba(0.62, 0.44, 0.10, 1.0);
const GOLD_LIGHT: Color = Color::srgba(1.0, 0.86, 0.35, 1.0);
const TRACK_BG: Color = Color::srgba(0.0, 0.0, 0.0, 0.78);

const BORDER_OUTER_PX: f32 = 2.0;
const BORDER_OUTER_PAD_PX: f32 = 1.0;
const BORDER_MID_PX: f32 = 1.0;
const BORDER_INNER_PAD_PX: f32 = 1.0;
const BORDER_INNER_PX: f32 = 1.0;

const BAR_TOTAL_W: f32 = BAR_WIDTH_PX
+ 2.0 * (BORDER_OUTER_PX + BORDER_OUTER_PAD_PX + BORDER_MID_PX
+ BORDER_INNER_PAD_PX + BORDER_INNER_PX);
const BAR_TOTAL_H: f32 = BAR_HEIGHT_PX
+ 2.0 * (BORDER_OUTER_PX + BORDER_OUTER_PAD_PX + BORDER_MID_PX
+ BORDER_INNER_PAD_PX + BORDER_INNER_PX);

// =============================================================================
// Rigenerazione della stamina
// =============================================================================
const STAMINA_REGEN_PER_SEC: f32 = 5.0;
const STAMINA_REGEN_DELAY_SECS: f32 = 1.0;

// =============================================================================
// Flash rosso quando il player è ferito
// =============================================================================

/// Alpha massima dell'overlay rosso. 0.35 è visibile ma non accecante.
const DAMAGE_FLASH_MAX: f32 = 0.35;

/// Durata (secondi) del flash: alpha scende linearmente da `DAMAGE_FLASH_MAX`
/// a 0 in questo tempo.
const DAMAGE_FLASH_DURATION_SECS: f32 = 0.4;

/// Colore del flash (rosso sangue). L'alpha viene modulata a runtime.
const DAMAGE_FLASH_COLOR: Color = Color::srgb(0.8, 0.0, 0.0);

/// Evento emesso da `combat.rs::apply_enemy_damage_to_player` quando il player
/// subisce danno. `vitals.rs` lo consuma per il flash rosso.
///
/// Definito qui (e non in `combat.rs`) perché è un concetto UI: `combat.rs` lo
/// importa solo per emetterlo. Questo evita una dipendenza circolare
/// (`vitals` → `combat` per l'evento, `combat` → `vitals` per `PlayerVitals`):
/// ora è solo `combat` → `vitals`.
#[derive(Event, Debug, Clone, Copy)]
pub struct PlayerDamaged {
    pub amount: f32,
}

#[derive(Component)]
struct DamageFlashOverlay;

#[derive(Resource, Default)]
struct DamageFlash {
    alpha: f32,
}

// =============================================================================
// Risorsa valori vitali
// =============================================================================
#[derive(Resource, Debug, Clone)]
pub struct PlayerVitals {
    pub health: f32,
    pub max_health: f32,
    pub stamina: f32,
    pub max_stamina: f32,
    pub mana: f32,
    pub max_mana: f32,
}

impl Default for PlayerVitals {
    fn default() -> Self {
        Self {
            health: 100.0,
            max_health: 100.0,
            stamina: 100.0,
            max_stamina: 100.0,
            mana: 100.0,
            max_mana: 100.0,
        }
    }
}

#[derive(Component)]
struct HealthBarFill;

#[derive(Component)]
struct StaminaBarFill;

#[derive(Component)]
struct ManaBarFill;

pub struct VitalsPlugin;

impl Plugin for VitalsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerVitals>()
        .init_resource::<DamageFlash>()
        // `add_event` è idempotente: `combat.rs` la chiama già, ma se
        // `VitalsPlugin` è caricato senza `CombatPlugin` serve comunque.
        .add_event::<PlayerDamaged>()
        .add_systems(Startup, (setup_vitals_ui, setup_damage_flash))
        .add_systems(
            Update,
            (
                regen_stamina,
             update_vitals_ui,
             handle_player_damaged,
             update_damage_flash,
            )
            .chain(),
        );
    }
}

// =============================================================================
// Setup
// =============================================================================
fn setup_vitals_ui(mut commands: Commands, assets: Res<AssetServer>) {
    let column: Option<Handle<Image>> = if std::path::Path::new(COLUMN_PNG_FS).exists() {
        info!("Vitals: uso le colonne ioniche da {COLUMN_PNG} ({COLUMN_W_PX}×{COLUMN_H_PX})");
        Some(assets.load::<Image>(COLUMN_PNG))
    } else {
        info!(
            "Vitals: {COLUMN_PNG_FS} non trovato, uso il bordo dorato procedurale \
(droppa una PNG di colonna per le colonne ioniche)"
        );
        None
    };

    commands
    .spawn((
        Name::new("HUD Vitals Container"),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
            },
            GlobalZIndex(10),
    ))
    .with_children(|parent| {
        let advance = COLUMN_W_PX + BAR_GAP_PX;
        let left_0 = HUD_LEFT_PX;
        let left_1 = HUD_LEFT_PX + advance;
        let left_2 = HUD_LEFT_PX + 2.0 * advance;
        spawn_vitals_bar(parent, left_0, HUD_BOTTOM_PX, COLOR_HEALTH,  HealthBarFill,  column.as_ref());
        spawn_vitals_bar(parent, left_1, HUD_BOTTOM_PX, COLOR_STAMINA, StaminaBarFill, column.as_ref());
        spawn_vitals_bar(parent, left_2, HUD_BOTTOM_PX, COLOR_MANA,    ManaBarFill,    column.as_ref());
    });
}

/// Overlay rosso a tutto schermo. Inizialmente invisibile, alpha 0.
fn setup_damage_flash(mut commands: Commands) {
    commands.spawn((
        Name::new("Damage flash overlay"),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                    },
                    BackgroundColor(Color::srgba(
                        DAMAGE_FLASH_COLOR.to_srgba().red,
                                                 DAMAGE_FLASH_COLOR.to_srgba().green,
                                                 DAMAGE_FLASH_COLOR.to_srgba().blue,
                                                 0.0,
                    )),
                    Visibility::Hidden,
                    GlobalZIndex(100),
                    DamageFlashOverlay,
    ));
}

fn spawn_vitals_bar<F: Component>(
    parent: &mut ChildBuilder,
    left: f32,
    bottom: f32,
    fill_color: Color,
    fill_marker: F,
    column: Option<&Handle<Image>>,
) {
    parent
    .spawn((
        Name::new("bar_column"),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(left),
            bottom: Val::Px(bottom),
            width: Val::Px(COLUMN_W_PX),
            height: Val::Px(COLUMN_H_PX),
            ..default()
            },
    ))
    .with_children(|outer| {
        match column {
            Some(handle) => {
                outer.spawn((
                    Name::new("bar_column_bg"),
                             ImageNode::new(handle.clone()),
                             Node {
                                 position_type: PositionType::Absolute,
                                 left: Val::Px(0.0),
                             top: Val::Px(0.0),
                             width: Val::Percent(100.0),
                             height: Val::Percent(100.0),
                             ..default()
                             },
                ));
            }
            None => {
                let inner_left = (COLUMN_W_PX - BAR_TOTAL_W) * 0.5;
                let inner_top = (COLUMN_H_PX - BAR_TOTAL_H) * 0.5;
                outer
                .spawn((
                    Name::new("bar_gold_outer"),
                        Node {
                            position_type: PositionType::Absolute,
                            left: Val::Px(inner_left),
                        top: Val::Px(inner_top),
                        width: Val::Px(BAR_TOTAL_W),
                        height: Val::Px(BAR_TOTAL_H),
                        border: UiRect::all(Val::Px(BORDER_OUTER_PX)),
                        padding: UiRect::all(Val::Px(BORDER_OUTER_PAD_PX)),
                        ..default()
                        },
                        BackgroundColor(GOLD_SHADOW),
                        BorderColor(GOLD_DARK),
                        BorderRadius::all(Val::Px(4.0)),
                ))
                .with_children(|g_outer| {
                    g_outer
                    .spawn((
                        Node {
                            width: Val::Percent(100.0),
                            height: Val::Percent(100.0),
                            border: UiRect::all(Val::Px(BORDER_MID_PX)),
                            padding: UiRect::all(Val::Px(BORDER_INNER_PAD_PX)),
                            ..default()
                        },
                        BackgroundColor(GOLD_MID),
                            BorderColor(GOLD_MID),
                            BorderRadius::all(Val::Px(3.0)),
                    ))
                    .with_children(|g_mid| {
                        g_mid.spawn((
                            Node {
                                width: Val::Percent(100.0),
                                     height: Val::Percent(100.0),
                                     border: UiRect::all(Val::Px(BORDER_INNER_PX)),
                                     flex_direction: FlexDirection::Column,
                                     justify_content: JustifyContent::FlexEnd,
                                     overflow: Overflow::clip(),
                                     ..default()
                            },
                            BackgroundColor(TRACK_BG),
                                     BorderColor(GOLD_LIGHT),
                                     BorderRadius::all(Val::Px(2.0)),
                        ));
                    });
                });
            }
        }

        let (fill_left, fill_top) = match column {
            Some(_) => (COLUMN_FILL_LEFT_PX, COLUMN_FILL_TOP_PX),
                   None => {
                       let pad = BORDER_OUTER_PX + BORDER_OUTER_PAD_PX
                       + BORDER_MID_PX + BORDER_INNER_PAD_PX;
                       let left = (COLUMN_W_PX - BAR_TOTAL_W) * 0.5 + pad;
                       let top = (COLUMN_H_PX - BAR_TOTAL_H) * 0.5 + pad;
                       (left, top)
                   }
        };
        outer
        .spawn((
            Name::new("bar_track"),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(fill_left),
                top: Val::Px(fill_top),
                width: Val::Px(BAR_WIDTH_PX),
                height: Val::Px(BAR_HEIGHT_PX),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::FlexEnd,
                overflow: Overflow::clip(),
                ..default()
                },
        ))
        .with_children(|track| {
            track.spawn((
                Name::new("bar_fill"),
                         Node {
                             width: Val::Percent(100.0),
                         height: Val::Percent(100.0),
                         ..default()
                         },
                         BackgroundColor(fill_color),
                         BorderRadius::all(Val::Px(1.0)),
                         fill_marker,
            ));
        });
    });
}

// =============================================================================
// Regen stamina
// =============================================================================
fn regen_stamina(
    time: Res<Time>,
    mut vitals: ResMut<PlayerVitals>,
    mut last_stamina: Local<f32>,
    mut idle: Local<f32>,
) {
    let dt = time.delta_secs().min(0.1);

    let consumed = vitals.stamina < *last_stamina;
    *last_stamina = vitals.stamina;

    if consumed {
        *idle = 0.0;
        return;
    }

    *idle += dt;
    if *idle < STAMINA_REGEN_DELAY_SECS {
        return;
    }

    if vitals.stamina < vitals.max_stamina {
        let regen = STAMINA_REGEN_PER_SEC * dt;
        vitals.stamina = (vitals.stamina + regen).min(vitals.max_stamina);
        *last_stamina = vitals.stamina;
    }
}

// =============================================================================
// UI update
// =============================================================================
fn update_vitals_ui(
    vitals: Res<PlayerVitals>,
    mut health_bar: Query<&mut Node, (With<HealthBarFill>, Without<StaminaBarFill>, Without<ManaBarFill>)>,
                    mut stamina_bar: Query<&mut Node, (With<StaminaBarFill>, Without<HealthBarFill>, Without<ManaBarFill>)>,
                    mut mana_bar: Query<&mut Node, (With<ManaBarFill>, Without<HealthBarFill>, Without<StaminaBarFill>)>,
) {
    if !vitals.is_changed() {
        return;
    }

    let hp_pct = (vitals.health / vitals.max_health.max(1.0)).clamp(0.0, 1.0) * 100.0;
    if let Ok(mut node) = health_bar.get_single_mut() {
        node.height = Val::Percent(hp_pct);
    }

    let stam_pct = (vitals.stamina / vitals.max_stamina.max(1.0)).clamp(0.0, 1.0) * 100.0;
    if let Ok(mut node) = stamina_bar.get_single_mut() {
        node.height = Val::Percent(stam_pct);
    }

    let mana_pct = (vitals.mana / vitals.max_mana.max(1.0)).clamp(0.0, 1.0) * 100.0;
    if let Ok(mut node) = mana_bar.get_single_mut() {
        node.height = Val::Percent(mana_pct);
    }
}

// =============================================================================
// Flash rosso
// =============================================================================

/// Consuma i `PlayerDamaged`: se ce n'è almeno uno, alza l'alpha a
/// `DAMAGE_FLASH_MAX` e rende visibile l'overlay.
fn handle_player_damaged(
    mut events: EventReader<PlayerDamaged>,
    mut flash: ResMut<DamageFlash>,
    mut overlay: Query<&mut Visibility, With<DamageFlashOverlay>>,
) {
    let mut hit = false;
    for _ in events.read() {
        hit = true;
    }
    if !hit {
        return;
    }
    flash.alpha = DAMAGE_FLASH_MAX;
    for mut vis in &mut overlay {
        *vis = Visibility::Inherited;
    }
}

/// Fa scendere l'alpha del flash linearmente. A zero nasconde l'overlay.
fn update_damage_flash(
    time: Res<Time>,
    mut flash: ResMut<DamageFlash>,
    mut overlay: Query<(&mut BackgroundColor, &mut Visibility), With<DamageFlashOverlay>>,
) {
    if flash.alpha <= 0.0 {
        return;
    }
    let decay = DAMAGE_FLASH_MAX / DAMAGE_FLASH_DURATION_SECS.max(0.01);
    flash.alpha = (flash.alpha - decay * time.delta_secs()).max(0.0);

    let color = DAMAGE_FLASH_COLOR.to_srgba();
    for (mut bg, mut vis) in &mut overlay {
        bg.0 = Color::srgba(color.red, color.green, color.blue, flash.alpha);
        *vis = if flash.alpha > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}
