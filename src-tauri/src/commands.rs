//! Tauri command handlers — the IPC surface the Svelte frontend calls.

use crate::app_state::{push_event, SharedState};
use crate::models::{
    AppConfig, AudioEndpoint, AudioSession, ChatMixAppRoute, ChatMixRoute, DiagnosticEvent,
    PresenceSnapshot, SwitchDecision,
};
use crate::switch::{apply_decision, decide_current};
use crate::volume::sync_chatmix;
use crate::window::show_main_window;
use crate::{config, theme};
use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager, State};

#[tauri::command]
pub(crate) fn get_config(state: State<'_, SharedState>) -> AppConfig {
    state.config.lock().clone()
}

#[tauri::command]
pub(crate) fn save_config(
    new_config: AppConfig,
    state: State<'_, SharedState>,
) -> Result<AppConfig, String> {
    config::save(&state.config_path, &new_config).map_err(|error| error.to_string())?;
    *state.config.lock() = new_config.clone();
    push_event(&state, DiagnosticEvent::info("Configuration saved"));
    Ok(new_config)
}

#[tauri::command]
pub(crate) fn list_endpoints(state: State<'_, SharedState>) -> Result<Vec<AudioEndpoint>, String> {
    state.audio.endpoints().map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn list_audio_sessions(
    state: State<'_, SharedState>,
) -> Result<Vec<AudioSession>, String> {
    let config = state.config.lock().clone();
    state
        .audio
        .render_sessions(&config.chatmix)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn get_presence(state: State<'_, SharedState>) -> PresenceSnapshot {
    state
        .last_presence
        .lock()
        .clone()
        .unwrap_or_else(|| PresenceSnapshot::error("Presence has not been initialized yet"))
}

#[tauri::command]
pub(crate) fn get_diagnostics(state: State<'_, SharedState>) -> Vec<DiagnosticEvent> {
    state.diagnostics.lock().iter().cloned().collect()
}

#[tauri::command]
pub(crate) fn apply_now(state: State<'_, SharedState>) -> Result<SwitchDecision, String> {
    let presence = state.last_presence.lock().clone();
    let connected = presence.as_ref().map(|p| p.connected).unwrap_or(false);
    let has_status = presence
        .as_ref()
        .map(|p| p.has_connection_status)
        .unwrap_or(false);

    let decision =
        decide_current(&state, connected, has_status).map_err(|error| error.to_string())?;
    apply_decision(&state, &decision).map_err(|error| error.to_string())?;
    push_event(
        &state,
        DiagnosticEvent::info(format!("Apply now: {}", decision.reason)),
    );
    Ok(decision)
}

#[tauri::command]
pub(crate) fn set_autoswitch_enabled(
    enabled: bool,
    state: State<'_, SharedState>,
) -> Result<AppConfig, String> {
    let mut config = state.config.lock().clone();
    config.autoswitch_enabled = enabled;
    save_config(config, state)
}

#[tauri::command]
pub(crate) fn set_app_chatmix_route(
    app_id: String,
    route: ChatMixRoute,
    display_name: String,
    state: State<'_, SharedState>,
) -> Result<AppConfig, String> {
    let mut config = state.config.lock().clone();
    config.chatmix.app_routes.insert(
        app_id,
        ChatMixAppRoute {
            route,
            display_name,
        },
    );
    config::save(&state.config_path, &config).map_err(|error| error.to_string())?;
    *state.config.lock() = config.clone();

    if let Some(presence) = state.last_presence.lock().clone() {
        if let Err(error) = sync_chatmix(&state, &presence, "route_change") {
            push_event(
                &state,
                DiagnosticEvent::warn(format!("ChatMix apply failed: {error}")).category("chatmix"),
            );
        }
    }

    push_event(&state, DiagnosticEvent::info("ChatMix app route saved"));
    Ok(config)
}

#[tauri::command]
pub(crate) fn open_settings(app: AppHandle) -> Result<(), String> {
    show_main_window(&app).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn settings_ready(app: AppHandle, state: State<'_, SharedState>) -> Result<(), String> {
    if !state
        .settings_window_pending_show
        .swap(false, Ordering::SeqCst)
    {
        return Ok(());
    }

    if let Some(window) = app.get_webview_window("main") {
        window.show().map_err(|error| error.to_string())?;
        window.set_focus().map_err(|error| error.to_string())?;
    }

    Ok(())
}

#[tauri::command]
pub(crate) fn get_accent_color() -> Option<String> {
    theme::accent_color()
}

#[tauri::command]
pub(crate) fn sync_chatmix_now(state: State<'_, SharedState>) -> Result<(), String> {
    let Some(presence) = state.last_presence.lock().clone() else {
        return Ok(());
    };
    sync_chatmix(&state, &presence, "manual_debug").map_err(|error| error.to_string())
}
