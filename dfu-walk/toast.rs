//! Notifiche a schermo (toast): una riga di testo in alto, una alla volta, in coda.
//!
//! Uso da qualsiasi sistema: `toasts.push("Hai ricevuto una lettera.")` con `ResMut<Toasts>`.
//!
//! In `main.rs`: `mod toast;` e `.add_plugins(toast::ToastPlugin)`.

use bevy::prelude::*;
use std::collections::VecDeque;

const BG: Color = Color::srgba(0.10, 0.075, 0.055, 0.90);
const BORDER: Color = Color::srgba(0.75, 0.58, 0.28, 0.60);
const TEXT: Color = Color::srgb(0.96, 0.93, 0.85);

#[derive(Resource, Default)]
pub struct Toasts {
    queue: VecDeque<String>,
    current: Option<(String, f32)>,
}

impl Toasts {
    pub fn push(&mut self, text: impl Into<String>) {
        let t = text.into();
        let t = t.split_whitespace().collect::<Vec<_>>().join(" ");
        if !t.is_empty() && self.queue.back() != Some(&t) {
            self.queue.push_back(t);
        }
    }
}

/// Secondi di permanenza: 3 piu' un po' per ogni carattere, al massimo 12.
fn duration_for(text: &str) -> f32 {
    (3.0 + text.chars().count() as f32 * 0.04).min(12.0)
}

#[derive(Component)]
struct ToastRoot;
#[derive(Component)]
struct ToastText;

pub struct ToastPlugin;

impl Plugin for ToastPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Toasts>()
            .add_systems(Startup, setup_toast)
            .add_systems(Update, update_toast);
    }
}

fn setup_toast(mut commands: Commands) {
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(10.0),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
            GlobalZIndex(95),
            Visibility::Hidden,
            ToastRoot,
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    max_width: Val::Px(640.0),
                    padding: UiRect::axes(Val::Px(18.0), Val::Px(10.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    ..default()
                },
                BackgroundColor(BG),
                BorderColor(BORDER),
                BorderRadius::all(Val::Px(12.0)),
            ))
            .with_children(|b| {
                b.spawn((
                    Text::new(""),
                    TextFont { font_size: 16.0, ..default() },
                    TextColor(TEXT),
                    ToastText,
                ));
            });
        });
}

fn update_toast(
    time: Res<Time>,
    mut toasts: ResMut<Toasts>,
    mut root: Query<&mut Visibility, With<ToastRoot>>,
    mut text: Query<&mut Text, With<ToastText>>,
) {
    // niente `ResMut` deref quando non c'e' nulla da fare
    if toasts.current.is_none() && toasts.queue.is_empty() {
        return;
    }
    let dt = time.delta_secs();

    let mut expired = false;
    if let Some((_, left)) = toasts.current.as_mut() {
        *left -= dt;
        expired = *left <= 0.0;
    }
    if expired {
        toasts.current = None;
    }
    if toasts.current.is_none() {
        if let Some(next) = toasts.queue.pop_front() {
            let secs = duration_for(&next);
            for mut t in &mut text {
                t.0 = next.clone();
            }
            toasts.current = Some((next, secs));
        }
    }
    let show = toasts.current.is_some();
    for mut v in &mut root {
        *v = if show { Visibility::Visible } else { Visibility::Hidden };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_normalises_and_skips_repeats() {
        let mut t = Toasts::default();
        t.push("  Hai   ricevuto\nuna lettera. ");
        t.push("Hai ricevuto una lettera.");
        t.push("");
        assert_eq!(t.queue.len(), 1);
        assert_eq!(t.queue[0], "Hai ricevuto una lettera.");
    }

    #[test]
    fn longer_text_stays_longer_but_capped() {
        assert!(duration_for("ciao") < duration_for(&"x".repeat(100)));
        assert_eq!(duration_for(&"x".repeat(10_000)), 12.0);
    }
}
