//! Interfaccia HUD minimale: 3 barre verticali al bordo sinistro (Salute, Stamina, Mana)
//! senza sfondo né scritte, con riempimento dal basso verso l'alto.

use bevy::prelude::*;

// Posizionamento al bordo sinistro
const HUD_LEFT_PX: f32 = 18.0;
const HUD_BOTTOM_PX: f32 = 18.0;

// Dimensioni delle 3 barre verticali
const BAR_WIDTH_PX: f32 = 14.0;
const BAR_HEIGHT_PX: f32 = 100.0;
const BAR_GAP_PX: f32 = 6.0;

// Colori classici
const COLOR_HEALTH: Color = Color::srgb(0.85, 0.15, 0.15);   // Rosso
const COLOR_STAMINA: Color = Color::srgb(0.20, 0.75, 0.25);  // Verde
const COLOR_MANA: Color = Color::srgb(0.18, 0.48, 0.90);     // Blu

// Incavo scuro della barra vuota e bordo sottile
const TRACK_BG: Color = Color::srgba(0.0, 0.0, 0.0, 0.65);
const TRACK_BORDER: Color = Color::srgba(0.35, 0.28, 0.18, 0.60);

/// Risorsa dei valori vitali (default 100/100).
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
        .add_systems(Startup, setup_vitals_ui)
        .add_systems(Update, update_vitals_ui);
    }
}

fn setup_vitals_ui(mut commands: Commands) {
    // Contenitore trasparente (nessuno sfondo) che allinea le 3 barre in orizzontale
    commands
    .spawn((
        Name::new("HUD Vertical Vitals"),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(HUD_LEFT_PX),
            bottom: Val::Px(HUD_BOTTOM_PX),
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(BAR_GAP_PX),
            ..default()
            },
            GlobalZIndex(10),
    ))
    .with_children(|parent| {
        // 1. Barra Salute (Rossa)
        spawn_vertical_bar(parent, COLOR_HEALTH, HealthBarFill);

        // 2. Barra Stamina (Verde)
        spawn_vertical_bar(parent, COLOR_STAMINA, StaminaBarFill);

        // 3. Barra Mana (Blu)
        spawn_vertical_bar(parent, COLOR_MANA, ManaBarFill);
    });
}

fn spawn_vertical_bar<F: Component>(
    parent: &mut ChildBuilder,
    fill_color: Color,
    fill_marker: F,
) {
    parent
    .spawn((
        // Binario/incavo verticale
        Node {
            width: Val::Px(BAR_WIDTH_PX),
            height: Val::Px(BAR_HEIGHT_PX),
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::FlexEnd, // Riempimento ancorato in basso
            border: UiRect::all(Val::Px(1.0)),
            padding: UiRect::all(Val::Px(1.0)),
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(TRACK_BG),
            BorderColor(TRACK_BORDER),
            BorderRadius::all(Val::Px(2.0)),
    ))
    .with_children(|track| {
        // Livello riempito (cresce verso l'alto con l'altezza)
        track.spawn((
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
}

fn update_vitals_ui(
    vitals: Res<PlayerVitals>,
    mut health_bar: Query<&mut Node, (With<HealthBarFill>, Without<StaminaBarFill>, Without<ManaBarFill>)>,
                    mut stamina_bar: Query<&mut Node, (With<StaminaBarFill>, Without<HealthBarFill>, Without<ManaBarFill>)>,
                    mut mana_bar: Query<&mut Node, (With<ManaBarFill>, Without<HealthBarFill>, Without<StaminaBarFill>)>,
) {
    if !vitals.is_changed() {
        return;
    }

    // Aggiornamento altezza Salute (0..100%)
    let hp_pct = (vitals.health / vitals.max_health.max(1.0)).clamp(0.0, 1.0) * 100.0;
    if let Ok(mut node) = health_bar.get_single_mut() {
        node.height = Val::Percent(hp_pct);
    }

    // Aggiornamento altezza Stamina (0..100%)
    let stam_pct = (vitals.stamina / vitals.max_stamina.max(1.0)).clamp(0.0, 1.0) * 100.0;
    if let Ok(mut node) = stamina_bar.get_single_mut() {
        node.height = Val::Percent(stam_pct);
    }

    // Aggiornamento altezza Mana (0..100%)
    let mana_pct = (vitals.mana / vitals.max_mana.max(1.0)).clamp(0.0, 1.0) * 100.0;
    if let Ok(mut node) = mana_bar.get_single_mut() {
        node.height = Val::Percent(mana_pct);
    }
}
