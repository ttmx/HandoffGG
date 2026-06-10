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
mod switch;
mod theme;
mod volume;
mod window;

#[cfg(windows)]
mod windows_audio;

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

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("HandoffGG")
                .build(),
        )
        .setup(|app| {
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
