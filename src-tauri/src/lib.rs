mod app_state;
mod audio;
mod chatmix;
mod commands;
mod config;
mod hid_report;
mod models;
mod monitor;
mod presence;
mod rules;
mod status_file;
mod switch;
mod theme;
mod volume;
mod window;

#[cfg(windows)]
mod windows_audio;

#[cfg(target_os = "linux")]
mod pipewire_audio;

#[cfg(target_os = "linux")]
mod background;

#[cfg(test)]
mod pipeline_tests;

use crate::app_state::{config_path, push_event, AppState};
use crate::audio::NativeAudioBackend;
use crate::chatmix::ChatMixVolumeManager;
use crate::models::{AppConfig, DiagnosticEvent};
use crate::monitor::{start_audio_device_monitor, start_monitor};
use crate::presence::SteelSeriesHidPresenceBackend;
use crate::window::build_tray;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::{Manager, RunEvent};

/// Flag the autostart launcher passes so a login-time start stays in the background
/// (no settings window), while a manual launch from the app menu opens the window.
const HIDDEN_FLAG: &str = "--hidden";
/// Flag (used by the `.desktop` Quit action / `handoffgg --quit`) that asks a running
/// instance to exit.
const QUIT_FLAG: &str = "--quit";

pub fn run() {
    tauri::Builder::default()
        // Must be registered first. A second launch (the app-menu icon, or a `.desktop`
        // action) routes its argv here instead of starting a duplicate: `--quit` exits the
        // running instance, anything else opens/focuses the settings window. This is what
        // makes the app usable on GNOME, which has no system tray.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if argv.iter().any(|arg| arg == QUIT_FLAG) {
                app.exit(0);
                return;
            }
            let _ = crate::window::show_main_window(app);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("HandoffGG")
                .args([HIDDEN_FLAG])
                .build(),
        )
        .setup(|app| {
            // `handoffgg --quit` with no other instance running: nothing to do, just exit.
            if std::env::args().any(|arg| arg == QUIT_FLAG) {
                app.handle().exit(0);
                return Ok(());
            }

            let config_path = config_path(app.handle())?;
            let loaded_config = config::load(&config_path).unwrap_or_else(|error| {
                eprintln!("failed to load config: {error}");
                AppConfig::default()
            });

            let state = Arc::new(AppState {
                config_path,
                config: Mutex::new(loaded_config),
                audio: Arc::new(NativeAudioBackend::new()?),
                presence: Mutex::new(Box::new(SteelSeriesHidPresenceBackend::arctis_nova_7())),
                chatmix: Mutex::new(ChatMixVolumeManager::default()),
                last_presence: Mutex::new(None),
                diagnostics: Mutex::new(VecDeque::new()),
                settings_window_pending_show: AtomicBool::new(false),
            });

            push_event(&state, DiagnosticEvent::info("HandoffGG started"));
            app.manage(state.clone());
            build_tray(app.handle())?;
            start_audio_device_monitor(app.handle().clone(), state.clone());
            start_monitor(app.handle().clone(), state);

            // On Linux, register as a background app so GNOME (which has no tray) lists us
            // under Quick Settings → Background Apps.
            #[cfg(target_os = "linux")]
            background::request_background();

            // A normal launch from the app menu opens settings straight away; an autostart
            // launch (passes `--hidden`) stays quietly in the background. The tray, where
            // present (KDE/XFCE/GNOME-with-extension), still opens settings on click.
            if !std::env::args().any(|arg| arg == HIDDEN_FLAG) {
                let _ = window::show_main_window(app.handle());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::list_endpoints,
            commands::list_audio_sessions,
            commands::get_presence,
            commands::get_diagnostics,
            commands::apply_now,
            commands::sync_chatmix_now,
            commands::set_autoswitch_enabled,
            commands::set_app_chatmix_route,
            commands::open_settings,
            commands::settings_ready,
            commands::get_accent_color
        ])
        .build(tauri::generate_context!())
        .expect("error while building HandoffGG")
        .run(|_, event| {
            if let RunEvent::ExitRequested { api, code, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
        });
}
