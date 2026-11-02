//! Menu di tuning (tasto F1), con tre schede separate:
//!
//! * **Finto rilievo** — uno slider per ogni parametro di `FakeRelief`.
//! * **Torcia** — sovrascritture di intensità, range, colore, flickering
//!   (`TorchTuning`), salvate su `torch_settings.txt`.
//! * **Colore** — saturazione, luminosità e contrasto applicati dopo tutti
//!   gli altri post-processing (in spazio gamma). Il contrasto ha due
//!   modalità: pivot fisso (default 0.5) oppure adaptive (pivot = luminanza
//!   media locale, così alzare il contrasto non sposta la luminosità media
//!   della zona: gli interni scuri restano scuri e gli esterni chiari
//!   restano chiari).
//!
//! ## Caricamento all'avvio
//!
//! `Plugin::build` legge **entrambi** i file:
//!
//! * `relief_settings.txt` in `SavedRelief`. Il sistema `apply_saved_on_add`
//!   (in `Update`) usa `Added<FakeRelief>`: si attiva nel frame in cui la
//!   camera riceve il componente da `attach_to_cameras` (`relief_post.rs`),
//!   indipendentemente dall'ordine dei sistemi, e applica il testo del file.
//! * `torch_settings.txt` in un `TorchTuning`, che **sostituisce** il default
//!   creato da `TorchPlugin::init_resource`. `insert_resource` sovrascrive
//!   senza conflitti (in `main.rs` `tuning_menu` viene aggiunto dopo `torch`).
//!
//! Non serve cliccare "Carica tutto" all'avvio. Se un file non esiste, il
//! log lo dice e si usano i valori predefiniti.
//!
//! ## Integrazione
//!
//! * `Cargo.toml`: `bevy_egui = "0.33"` (la tua API corrisponde a una versione
//!   <= 0.30: `EguiPlugin` senza campi, `ctx_mut()` restituisce `&mut Context`).
//! * `main.rs`: `mod tuning_menu;` e `.add_plugins(tuning_menu::TuningMenuPlugin)`.
//!
//! ## Cursore
//!
//! Mentre il menu e' aperto il cursore di sistema e' visibile e libero, e il
//! cursore custom (`custom_cursor.rs`) viene nascosto: bevy_egui disegna la sua
//! finestra *dopo* Bevy UI, quindi il nostro `ImageNode` sarebbe coperto.

use std::path::Path;

use crate::dialog::DialogState;
use crate::relief_post::FakeRelief;
use crate::torch::TorchTuning;
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, PrimaryWindow};
use bevy_egui::{egui, EguiContexts, EguiPlugin};

const SETTINGS_FILE: &str = "relief_settings.txt";
const TORCH_SETTINGS_FILE: &str = "torch_settings.txt";

const DEBUG_NAMES: [&str; 7] = [
    "0 Nessuna",
"1 Shading",
"2 Cavita'",
"3 Fattore bordi",
"4 Maschera pelle",
"5 Mappa bump",
"6 Normal map",
];

/// Stato del menu.
#[derive(Resource, Default)]
pub struct TuningMenuState {
    pub open: bool,
    /// `DialogState::open` prima che il menu lo forzasse a `true`. Serve a
    /// restituire il dialogo NPC alla chiusura del menu.
    was_dialog_open_before_menu: bool,
}

/// Testo del file di impostazioni letto all'avvio, da applicare alla camera
/// quando nasce (la camera non esiste ancora in `Startup`).
#[derive(Resource, Default)]
struct SavedRelief(Option<String>);

pub struct TuningMenuPlugin;

impl Plugin for TuningMenuPlugin {
    fn build(&self, app: &mut App) {
        app
        // Nella tua versione `EguiPlugin` e' una unit struct.
        .add_plugins(EguiPlugin)
        .init_resource::<TuningMenuState>()
        .insert_resource({
            // Log esplicito: utile quando "sembra che non carichi".
            let saved = std::fs::read_to_string(SETTINGS_FILE);
            match &saved {
                Ok(text) => info!(
                    "Tuning: {} trovato ({} byte), verra' applicato alla camera",
                                  SETTINGS_FILE,
                                  text.len()
                ),
                Err(e) => info!(
                    "Tuning: {} non trovato ({e}), uso i valori predefiniti",
                                SETTINGS_FILE
                ),
            }
            SavedRelief(saved.ok())
        })
        .insert_resource({
            // Torcia: leggi il file e sovrascrivi il `TorchTuning` di
            // default creato da `TorchPlugin::init_resource`.
            //
            // `insert_resource` rimpiazza la risorsa se esiste gia', la
            // crea se manca. In `main.rs` `tuning_menu` viene aggiunto
            // DOPO `torch`, quindi il default e' gia' presente e questo
            // `insert_resource` lo sovrascrive col file (se c'e').
            let path = Path::new(TORCH_SETTINGS_FILE);
            let torch = TorchTuning::load_or_default(path);
            if path.exists() {
                info!(
                    "Tuning: {} trovato, parametri torcia caricati",
                    TORCH_SETTINGS_FILE
                );
            } else {
                info!(
                    "Tuning: {} non trovato, uso i valori predefiniti della torcia",
                    TORCH_SETTINGS_FILE
                );
            }
            torch
        })
        .add_systems(
            PreUpdate,
            (
                // PRIMA di `update_dialog`: tocca solo `menu.open`.
                detect_f1.before(crate::dialog::update_dialog_followers),
             // Solo col menu aperto: mangia i tasti che `update_dialog`
             // e `dialog_menu` usererebbero per chiudere il dialogo o
             // fare azioni di quest.
             suppress_menu_keys
             .run_if(|m: Res<TuningMenuState>| m.open)
             .after(detect_f1)
             .before(crate::dialog::update_dialog_followers),
            ),
        )
        .add_systems(
            Update,
            (
                // Dopo `grab_cursor` (che ri-bloccava il mouse) e prima di
                // `player_look` (che riprenderebbe a girare la visuale).
                apply_menu_state
                .after(crate::player::grab_cursor)
                .before(crate::player::player_look),
             apply_saved_on_add,
             menu_ui,
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// Salvataggio su testo — FakeRelief
// ---------------------------------------------------------------------------

/// Genera `to_text` e `set_field` per i campi elencati (nome identico al campo
/// di `FakeRelief`). `near`, `tan_half_fov` e `mean_div` sono esclusi: li
/// gestisce la camera, sovrascriverli dal file romperebbe l'effetto.
///
/// `color_*` sono inclusi: fanno parte della stessa struct, quindi vanno
/// salvati insieme ai parametri del rilievo.
macro_rules! relief_fields {
    (floats: [$($f:ident),* $(,)?], flags: [$($i:ident),* $(,)?]) => {
        fn to_text(r: &FakeRelief) -> String {
            let mut s = String::from("# Parametri del finto rilievo (menu F1)\n");
            $( s.push_str(&format!("{} = {}\n", stringify!($f), r.$f)); )*
            $( s.push_str(&format!("{} = {}\n", stringify!($i), r.$i)); )*
            s
        }

        fn set_field(r: &mut FakeRelief, name: &str, value: &str) {
            $( if name == stringify!($f) { if let Ok(v) = value.parse::<f32>() { r.$f = v; } return; } )*
            $( if name == stringify!($i) { if let Ok(v) = value.parse::<u32>() { r.$i = v; } return; } )*
        }
    };
}

relief_fields! {
    floats: [
        strength, detail_size, cavity, light_scale, bump_contrast, bump_smooth,
        mortar, mortar_threshold, parallax, spec, shininess, light_az, light_el,
        torch_blend, skin_hue, skin_hue_tol, skin_sat, skin_sat_tol, skin_val,
        skin_val_tol, far_plane, fade_distance,
        // Colore (scheda "Colore")
        color_saturation, color_brightness, color_contrast, color_contrast_pivot
    ],
    flags: [mode, coarse_scale, skin_enable, debug, enabled, color_enabled,
    color_adaptive_contrast]
}

fn apply_text(relief: &mut FakeRelief, text: &str) {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, value)) = line.split_once('=') {
            set_field(relief, name.trim(), value.trim());
        }
    }
}

// ---------------------------------------------------------------------------
// Toggle (F1) — PreUpdate, prima di `update_dialog`
// ---------------------------------------------------------------------------

fn detect_f1(
    keys: Res<ButtonInput<KeyCode>>,
    mut menu: ResMut<TuningMenuState>,
    dialog: Res<DialogState>,
) {
    let f1 = keys.just_pressed(KeyCode::F1);
    let esc_close = menu.open && keys.just_pressed(KeyCode::Escape);
    if !(f1 || esc_close) {
        return;
    }
    if menu.open {
        menu.open = false;
    } else {
        menu.was_dialog_open_before_menu = dialog.open;
        menu.open = true;
    }
}

fn suppress_menu_keys(mut keys: ResMut<ButtonInput<KeyCode>>) {
    for key in [
        KeyCode::KeyE,
        KeyCode::Escape,
        KeyCode::Digit1,
        KeyCode::Numpad1,
        KeyCode::Digit2,
        KeyCode::Numpad2,
        KeyCode::KeyA,
        KeyCode::KeyR,
    ] {
        keys.clear_just_pressed(key);
    }
}

// ---------------------------------------------------------------------------
// Applica menu → dialog e cursore — Update
// ---------------------------------------------------------------------------

fn apply_menu_state(
    menu: Res<TuningMenuState>,
    mut dialog: ResMut<DialogState>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
    mut last_open: Local<Option<bool>>,
) {
    let prev = *last_open;
    *last_open = Some(menu.open);
    let Ok(mut window) = windows.get_single_mut() else { return };

    // Menu aperto: forza cursore libero e visibile OGNI frame.
    if menu.open {
        dialog.open = true;
        dialog.forced_by_menu = true;
        window.cursor_options.grab_mode = CursorGrabMode::None;
        window.cursor_options.visible = true;
        return;
    }

    // Non era aperto e non è aperto: niente da fare.
    if prev != Some(true) {
        return;
    }

    // Era aperto, ora è chiuso: ripristina.
    dialog.open = menu.was_dialog_open_before_menu;
    dialog.forced_by_menu = false;
    window.cursor_options.grab_mode = CursorGrabMode::Locked;
    window.cursor_options.visible = false;
}

/// Applica il file salvato alle camere **nel frame in cui ricevono
/// `FakeRelief`**. `Added<FakeRelief>` è vero solo per il frame di aggiunta
/// (più il successivo, per gestire i cambi di schedule in Bevy 0.15), quindi
/// l'applicazione avviene una volta sola senza `Local<bool>` da mantenere.
///
/// Funziona anche se `attach_to_cameras` (in `relief_post.rs`) gira dopo
/// questo sistema: il flag `Added` resta vero anche al frame successivo.
fn apply_saved_on_add(
    mut cameras: Query<&mut FakeRelief, Added<FakeRelief>>,
    saved: Res<SavedRelief>,
) {
    let Some(text) = &saved.0 else {
        return;
    };
    for mut relief in &mut cameras {
        apply_text(&mut relief, text);
        info!(
            "Tuning: impostazioni applicate da {} ({} byte)",
              SETTINGS_FILE,
              text.len()
        );
    }
}

// ---------------------------------------------------------------------------
// Interfaccia
// ---------------------------------------------------------------------------

fn slider(ui: &mut egui::Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>, name: &str) {
    ui.add(egui::Slider::new(value, range).text(name));
}

fn flag(ui: &mut egui::Ui, value: &mut u32, name: &str) {
    let mut on = *value != 0;
    ui.checkbox(&mut on, name);
    *value = on as u32;
}

#[allow(clippy::too_many_arguments)]
fn menu_ui(
    mut contexts: EguiContexts,
    menu: Res<TuningMenuState>,
    mut cameras: Query<&mut FakeRelief>,
    mut torch: ResMut<TorchTuning>,
    mut status: Local<String>,
    mut active_tab: Local<usize>,
) {
    if !menu.open {
        return;
    }
    // Nella tua versione `ctx_mut()` restituisce `&mut Context` direttamente.
    let ctx = contexts.ctx_mut();

    egui::Window::new("Tuning (F1)")
    .default_width(400.0)
    .show(ctx, |ui| {
        // --- Selettore schede ---
        ui.horizontal(|ui| {
            ui.selectable_value(&mut *active_tab, 0, "Finto rilievo");
            ui.selectable_value(&mut *active_tab, 1, "Torcia");
            ui.selectable_value(&mut *active_tab, 2, "Colore");
        });
        ui.separator();

        // --- Contenuto della scheda attiva ---
        match *active_tab {
            0 => relief_ui(ui, &mut cameras, &mut status),
          1 => torch_ui(ui, &mut torch, &mut status),
          2 => color_ui(ui, &mut cameras, &mut status),
          _ => {}
        }

        // --- Pulsanti comuni in fondo ---
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Ripristina scheda").clicked() {
                match *active_tab {
                    0 => {
                        if let Ok(mut r) = cameras.get_single_mut() {
                            let (near, tan_half_fov, mean_div) =
                            (r.near, r.tan_half_fov, r.mean_div);
                            *r = FakeRelief::default();
                            r.near = near;
                            r.tan_half_fov = tan_half_fov;
                            r.mean_div = mean_div;
                            *status = "Finto rilievo: valori predefiniti".into();
                        }
                    }
                    1 => {
                        *torch = TorchTuning::default();
                        *status = "Torcia: valori predefiniti".into();
                    }
                    2 => {
                        if let Ok(mut r) = cameras.get_single_mut() {
                            r.color_enabled = 0;
                            r.color_saturation = 1.0;
                            r.color_brightness = 1.0;
                            r.color_contrast = 1.0;
                            r.color_adaptive_contrast = 1;
                            r.color_contrast_pivot = 0.5;
                            *status = "Colore: valori predefiniti".into();
                        }
                    }
                    _ => {}
                }
            }
            if ui.button("Salva scheda").clicked() {
                // I campi colore sono dentro `FakeRelief`: si salvano
                // insieme ai parametri del rilievo, nello stesso file.
                match *active_tab {
                    0 | 2 => {
                        if let Ok(r) = cameras.get_single_mut() {
                            *status = match std::fs::write(SETTINGS_FILE, to_text(&r)) {
                                Ok(()) => format!("Salvato in {SETTINGS_FILE}"),
                      Err(e) => format!("Errore di salvataggio: {e}"),
                            };
                        }
                    }
                    1 => {
                        *status = match std::fs::write(TORCH_SETTINGS_FILE, torch.to_text()) {
                            Ok(()) => format!("Salvato in {TORCH_SETTINGS_FILE}"),
                      Err(e) => format!("Errore di salvataggio: {e}"),
                        };
                    }
                    _ => {}
                }
            }
            if ui.button("Carica scheda").clicked() {
                match *active_tab {
                    0 | 2 => {
                        *status = match std::fs::read_to_string(SETTINGS_FILE) {
                            Ok(text) => {
                                if let Ok(mut r) = cameras.get_single_mut() {
                                    apply_text(&mut r, &text);
                                }
                                format!("Caricato da {SETTINGS_FILE}")
                            }
                            Err(e) => format!("Errore di lettura: {e}"),
                        };
                    }
                    1 => {
                        *torch = TorchTuning::load_or_default(Path::new(TORCH_SETTINGS_FILE));
                        *status = format!("Caricato da {TORCH_SETTINGS_FILE}");
                    }
                    _ => {}
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Salva tutto").clicked() {
                let mut ok = true;
                if let Ok(r) = cameras.get_single_mut() {
                    ok &= std::fs::write(SETTINGS_FILE, to_text(&r)).is_ok();
                }
                ok &= std::fs::write(TORCH_SETTINGS_FILE, torch.to_text()).is_ok();
                *status = if ok {
                    "Salvati entrambi i file".into()
                } else {
                    "Errore di salvataggio".into()
                };
            }
            if ui.button("Carica tutto").clicked() {
                let mut ok = true;
                match std::fs::read_to_string(SETTINGS_FILE) {
                    Ok(text) => {
                        if let Ok(mut r) = cameras.get_single_mut() {
                            apply_text(&mut r, &text);
                        }
                    }
                    Err(_) => ok = false,
                }
                match std::fs::read_to_string(TORCH_SETTINGS_FILE) {
                    Ok(text) => *torch = TorchTuning::from_text(&text),
                      Err(_) => ok = false,
                }
                *status = if ok {
                    "Caricati entrambi i file".into()
                } else {
                    "Errore di lettura".into()
                };
            }
        });
        if !status.is_empty() {
            ui.label(status.as_str());
        }
    });
}

// ---------------------------------------------------------------------------
// Scheda "Finto rilievo"
// ---------------------------------------------------------------------------

fn relief_ui(ui: &mut egui::Ui, cameras: &mut Query<&mut FakeRelief>, _status: &mut String) {
    let Ok(mut r) = cameras.get_single_mut() else {
        ui.label("Nessuna camera con l'effetto.");
        return;
    };
    let r = &mut *r;

    egui::ScrollArea::vertical().max_height(540.0).show(ui, |ui| {
        egui::CollapsingHeader::new("Generale").default_open(true).show(ui, |ui| {
            flag(ui, &mut r.enabled, "Effetto attivo (F8)");
            let mut bump = r.mode == 1;
            ui.checkbox(&mut bump, "Modalita' bump map (F7)");
            r.mode = bump as u32;
            flag(ui, &mut r.coarse_scale, "Seconda scala (dettaglio grossolano)");
            egui::ComboBox::from_label("Vista debug (F9)")
            .selected_text(DEBUG_NAMES[r.debug as usize % DEBUG_NAMES.len()])
            .show_ui(ui, |ui| {
                for (i, name) in DEBUG_NAMES.iter().enumerate() {
                    ui.selectable_value(&mut r.debug, i as u32, *name);
                }
            });
        });

        egui::CollapsingHeader::new("Rilievo").default_open(true).show(ui, |ui| {
            slider(ui, &mut r.strength, -4.0..=4.0, "Forza");
            slider(ui, &mut r.detail_size, 0.005..=0.5, "Dimensione dettagli (unita')");
            slider(ui, &mut r.cavity, 0.0..=2.0, "Cavita'");
            slider(ui, &mut r.light_scale, 4.0..=300.0, "Scala luce locale (px)");
            slider(ui, &mut r.mortar, 0.0..=1.0, "Annerimento interstizi");
            slider(ui, &mut r.mortar_threshold, 0.0..=0.9, "Soglia interstizi");
        });

        egui::CollapsingHeader::new("Bump map (modalita' 1)").show(ui, |ui| {
            slider(ui, &mut r.bump_contrast, 0.5..=3.0, "Contrasto");
            slider(ui, &mut r.bump_smooth, 0.0..=4.0, "Ammorbidimento");
        });

        egui::CollapsingHeader::new("Luce").show(ui, |ui| {
            slider(ui, &mut r.light_az, -180.0..=180.0, "Azimut (gradi)");
            slider(ui, &mut r.light_el, -90.0..=90.0, "Elevazione (gradi)");
            slider(ui, &mut r.torch_blend, 0.0..=1.0, "Luce dalla camera");
        });

        egui::CollapsingHeader::new("Parallax e speculare").show(ui, |ui| {
            slider(ui, &mut r.parallax, 0.0..=12.0, "Parallax (px)");
            slider(ui, &mut r.spec, 0.0..=2.0, "Speculare");
            slider(ui, &mut r.shininess, 1.0..=128.0, "Lucentezza");
        });

        egui::CollapsingHeader::new("Distanza").show(ui, |ui| {
            slider(ui, &mut r.fade_distance, 5.0..=500.0, "Dissolvenza (unita')");
            slider(ui, &mut r.far_plane, 100.0..=5000.0, "Distanza cielo (unita')");
        });

        egui::CollapsingHeader::new("Filtro pelle").show(ui, |ui| {
            flag(ui, &mut r.skin_enable, "Escludi la pelle dall'effetto");
            slider(ui, &mut r.skin_hue, 0.0..=360.0, "Tonalita' (gradi)");
            slider(ui, &mut r.skin_hue_tol, 0.0..=180.0, "Tolleranza tonalita'");
            slider(ui, &mut r.skin_sat, 0.0..=1.0, "Saturazione");
            slider(ui, &mut r.skin_sat_tol, 0.0..=1.0, "Tolleranza saturazione");
            slider(ui, &mut r.skin_val, 0.0..=1.0, "Valore");
            slider(ui, &mut r.skin_val_tol, 0.0..=1.0, "Tolleranza valore");
        });
    });
}

// ---------------------------------------------------------------------------
// Scheda "Torcia"
// ---------------------------------------------------------------------------

fn torch_ui(ui: &mut egui::Ui, torch: &mut TorchTuning, _status: &mut String) {
    egui::ScrollArea::vertical().max_height(540.0).show(ui, |ui| {
        ui.label(
            "Il tasto X del gioco continua a funzionare (cambia il livello di \
`TorchState`). Se 'Intensità personalizzata' è attiva, l'override \
ha la precedenza sul livello.",
        );
        ui.add_space(6.0);

        // --- Intensità ---
        let mut use_intensity = torch.intensity_override.is_some();
        if ui
            .checkbox(&mut use_intensity, "Intensità personalizzata")
            .changed()
            {
                torch.intensity_override = use_intensity.then_some(30_000.0);
            }
            if let Some(v) = torch.intensity_override.as_mut() {
                ui.add(
                    egui::Slider::new(v, 0.0..=200_000.0)
                    .text("Intensità (lumen)")
                    .clamping(egui::SliderClamping::Never),
                );
            }

            // --- Range ---
            let mut use_range = torch.range_override.is_some();
            if ui
                .checkbox(&mut use_range, "Range personalizzato")
                .changed()
                {
                    torch.range_override = use_range.then_some(crate::torch::TORCH_RANGE);
                }
                if let Some(v) = torch.range_override.as_mut() {
                    ui.add(egui::Slider::new(v, 1.0..=100.0).text("Range (m)"));
                }

                // --- Colore ---
                ui.separator();
                ui.label("Colore della fiamma");
                let mut srgba = torch
                .color_override
                .unwrap_or(crate::torch::TORCH_COLOR)
                .to_srgba();
                let mut color_changed = false;
                ui.horizontal(|ui| {
                    color_changed |= ui
                    .add(egui::Slider::new(&mut srgba.red, 0.0..=1.0).text("R"))
                    .changed();
                    color_changed |= ui
                    .add(egui::Slider::new(&mut srgba.green, 0.0..=1.0).text("G"))
                    .changed();
                    color_changed |= ui
                    .add(egui::Slider::new(&mut srgba.blue, 0.0..=1.0).text("B"))
                    .changed();
                });
                if color_changed {
                    torch.color_override = Some(Color::srgb(srgba.red, srgba.green, srgba.blue));
                }
                if ui.button("Colore predefinito").clicked() {
                    torch.color_override = None;
                }

                // --- Flickering ---
                ui.separator();
                slider(ui, &mut torch.flicker_amp, 0.0..=0.5, "Ampiezza flickering");
                slider(ui, &mut torch.flicker_freq, 0.0..=20.0, "Frequenza flickering (Hz)");
                ui.label(
                    "Ampiezza 0 = nessuno sfarfallio; 0.10 = default; sopra 0.25 = molto marcato.",
                );

                // --- Forza accesa ---
                ui.separator();
                ui.checkbox(
                    &mut torch.force_on,
                    "Forza accesa sempre (anche di giorno all'aperto)",
                );
    });
}

// ---------------------------------------------------------------------------
// Scheda "Colore"
// ---------------------------------------------------------------------------

fn color_ui(ui: &mut egui::Ui, cameras: &mut Query<&mut FakeRelief>, _status: &mut String) {
    let Ok(mut r) = cameras.get_single_mut() else {
        ui.label("Nessuna camera con l'effetto.");
        return;
    };
    let r = &mut *r;

    egui::ScrollArea::vertical().max_height(540.0).show(ui, |ui| {
        ui.label(
            "Regolazioni di saturazione, luminosità e contrasto, applicate a \
TUTTO lo schermo dopo il finto rilievo (in spazio gamma). \
Funzionano anche con il rilievo spento.",
        );
        ui.add_space(6.0);

        flag(ui, &mut r.color_enabled, "Attivo");

        ui.add_space(6.0);
        slider(
            ui,
            &mut r.color_saturation,
            0.0..=2.0,
            "Saturazione (1.0 = neutro, 0 = bianco e nero)",
        );
        slider(
            ui,
            &mut r.color_brightness,
            0.0..=2.0,
            "Luminosità (1.0 = neutro)",
        );
        slider(
            ui,
            &mut r.color_contrast,
            0.0..=2.0,
            "Contrasto (1.0 = neutro)",
        );

        ui.add_space(4.0);
        flag(
            ui,
             &mut r.color_adaptive_contrast,
             "Contrasto adattivo (pivot = luminosità media locale)",
        );
        ui.label(
            "Attivo: il contrasto non sposta la luminosità media della zona, \
quindi gli interni scuri restano scuri e gli esterni chiari \
restano chiari anche con contrasto alto.\n\
Disattivato: pivot fisso (impostabile sotto).",
        );
        if r.color_adaptive_contrast == 0 {
            slider(
                ui,
                &mut r.color_contrast_pivot,
                0.0..=1.0,
                "Pivot fisso (0.5 = grigio medio)",
            );
        }

        ui.add_space(6.0);
        if ui.button("Tutti neutri").clicked() {
            r.color_saturation = 1.0;
            r.color_brightness = 1.0;
            r.color_contrast = 1.0;
            r.color_adaptive_contrast = 1;
            r.color_contrast_pivot = 0.5;
        }

        ui.separator();
        ui.label(
            "Nota: con i valori neutri (1.0) e 'Attivo' flaggato l'operazione è \
matematica no-op, ma consuma ~6 ALU per pixel (più un tap per il \
pivot adattivo). Se non ti serve, disattiva 'Attivo'.",
        );
    });
}
