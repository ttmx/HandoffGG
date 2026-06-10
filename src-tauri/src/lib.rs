mod audio;
mod chatmix;
mod config;
mod models;
mod presence;
mod rules;
mod theme;

#[cfg(windows)]
mod windows_audio;

use crate::audio::{AudioBackend, NativeAudioBackend};
use crate::chatmix::ChatMixVolumeManager;
use crate::models::{
    now_ms, AppConfig, AudioEndpoint, AudioSession, ChatMixAppRoute, ChatMixRoute, DecisionAction,
    DiagnosticEvent, EndpointFlow, EndpointState, PresenceSnapshot, SwitchDecision,
};
use crate::presence::{HeadsetPresenceBackend, SteelSeriesHidPresenceBackend};
use crate::rules::decide_switch;
use parking_lot::Mutex;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::window::Color;
use tauri::{
    AppHandle, Emitter, Manager, RunEvent, State, Theme, WebviewUrl, WebviewWindowBuilder,
};

const MAX_EVENTS: usize = 200;
const WINDOW_BACKGROUND: Color = Color(0x1b, 0x1d, 0x20, 0xff);

struct AppState {
    config_path: PathBuf,
    config: Mutex<AppConfig>,
    audio: Arc<dyn AudioBackend>,
    presence: Mutex<Box<dyn HeadsetPresenceBackend>>,
    chatmix: Mutex<ChatMixVolumeManager>,
    last_presence: Mutex<Option<PresenceSnapshot>>,
    diagnostics: Mutex<VecDeque<DiagnosticEvent>>,
    settings_window_pending_show: AtomicBool,
}

type SharedState = Arc<AppState>;

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
            get_config,
            save_config,
            list_endpoints,
            list_audio_sessions,
            get_presence,
            get_diagnostics,
            apply_now,
            sync_chatmix_now,
            set_autoswitch_enabled,
            set_app_chatmix_route,
            open_settings,
            settings_ready,
            get_accent_color
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

#[tauri::command]
fn get_config(state: State<'_, SharedState>) -> AppConfig {
    state.config.lock().clone()
}

#[tauri::command]
fn save_config(new_config: AppConfig, state: State<'_, SharedState>) -> Result<AppConfig, String> {
    config::save(&state.config_path, &new_config).map_err(|error| error.to_string())?;
    *state.config.lock() = new_config.clone();
    push_event(&state, DiagnosticEvent::info("Configuration saved"));
    Ok(new_config)
}

#[tauri::command]
fn list_endpoints(state: State<'_, SharedState>) -> Result<Vec<AudioEndpoint>, String> {
    state.audio.endpoints().map_err(|error| error.to_string())
}

#[tauri::command]
fn list_audio_sessions(state: State<'_, SharedState>) -> Result<Vec<AudioSession>, String> {
    let config = state.config.lock().clone();
    state
        .audio
        .render_sessions(&config.chatmix)
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn get_presence(state: State<'_, SharedState>) -> PresenceSnapshot {
    state
        .last_presence
        .lock()
        .clone()
        .unwrap_or_else(|| PresenceSnapshot::error("Presence has not been initialized yet"))
}

#[tauri::command]
fn get_diagnostics(state: State<'_, SharedState>) -> Vec<DiagnosticEvent> {
    state.diagnostics.lock().iter().cloned().collect()
}

#[tauri::command]
fn apply_now(state: State<'_, SharedState>) -> Result<SwitchDecision, String> {
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
fn set_autoswitch_enabled(
    enabled: bool,
    state: State<'_, SharedState>,
) -> Result<AppConfig, String> {
    let mut config = state.config.lock().clone();
    config.autoswitch_enabled = enabled;
    save_config(config, state)
}

#[tauri::command]
fn set_app_chatmix_route(
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
                DiagnosticEvent::warn(format!("ChatMix apply failed: {error}")),
            );
        }
    }

    push_event(&state, DiagnosticEvent::info("ChatMix app route saved"));
    Ok(config)
}

#[tauri::command]
fn open_settings(app: AppHandle) -> Result<(), String> {
    show_main_window(&app).map_err(|error| error.to_string())
}

#[tauri::command]
fn settings_ready(app: AppHandle, state: State<'_, SharedState>) -> Result<(), String> {
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
fn get_accent_color() -> Option<String> {
    theme::accent_color()
}

#[tauri::command]
fn sync_chatmix_now(state: State<'_, SharedState>) -> Result<(), String> {
    let Some(presence) = state.last_presence.lock().clone() else {
        return Ok(());
    };
    sync_chatmix(&state, &presence, "manual_debug").map_err(|error| error.to_string())
}

fn start_monitor(app: AppHandle, state: SharedState) {
    thread::spawn(move || {
        let initial_snapshot = state.presence.lock().snapshot();
        let mut stable_connected = Some(initial_snapshot.connected);
        handle_presence_snapshot(&app, &state, initial_snapshot, true);

        // Fully event-driven: the dongle pushes unsolicited reports for connection /
        // battery (MI_03) and mute / chatmix (MI_05). We block on those and react.
        loop {
            let Some(snapshot) = state
                .presence
                .lock()
                .wait_for_event(Duration::from_millis(1_000))
            else {
                continue;
            };
            process_snapshot(&app, &state, snapshot, &mut stable_connected);
        }
    });
}

#[cfg(windows)]
fn start_audio_device_monitor(app: AppHandle, state: SharedState) {
    let (tx, rx) = mpsc::channel();
    crate::windows_audio::start_endpoint_notification_listener(tx);
    thread::spawn(move || {
        while rx.recv().is_ok() {
            while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}

            let presence = state.last_presence.lock().clone();
            let connected = presence.as_ref().map(|p| p.connected).unwrap_or(false);
            let has_status = presence
                .as_ref()
                .map(|p| p.has_connection_status)
                .unwrap_or(false);

            match decide_current(&state, connected, has_status)
                .and_then(|decision| apply_decision(&state, &decision).map(|()| decision))
            {
                Ok(decision) => push_event(
                    &state,
                    DiagnosticEvent::info(format!("Audio device change: {}", decision.reason)),
                ),
                Err(error) => push_event(
                    &state,
                    DiagnosticEvent::warn(format!("Audio device refresh failed: {error}")),
                ),
            }

            if let Some(presence) = presence {
                if let Err(error) = sync_chatmix(&state, &presence, "audio_device") {
                    push_event(
                        &state,
                        DiagnosticEvent::warn(format!("ChatMix apply failed: {error}")),
                    );
                }
            }

            let _ = app.emit("autoswapper://state-changed", now_ms());
        }
    });
}

#[cfg(not(windows))]
fn start_audio_device_monitor(_app: AppHandle, _state: SharedState) {}

fn process_snapshot(
    app: &AppHandle,
    state: &AppState,
    snapshot: PresenceSnapshot,
    stable_connected: &mut Option<bool>,
) {
    if snapshot.error.is_some() {
        handle_presence_snapshot(app, state, snapshot, false);
        return;
    }

    let merged_snapshot = merge_presence_snapshot(state, snapshot);
    if !presence_changed(state, &merged_snapshot) {
        *state.last_presence.lock() = Some(merged_snapshot);
        return;
    }

    let connection_changed = merged_snapshot.has_connection_status
        && *stable_connected != Some(merged_snapshot.connected);
    *state.last_presence.lock() = Some(merged_snapshot.clone());
    if let Err(error) = sync_chatmix(state, &merged_snapshot, "hid_event") {
        push_event(
            state,
            DiagnosticEvent::warn(format!("ChatMix apply failed: {error}")),
        );
    }

    if connection_changed {
        *stable_connected = Some(merged_snapshot.connected);
        handle_presence_snapshot(app, state, merged_snapshot, false);
    } else {
        let _ = app.emit("autoswapper://state-changed", now_ms());
    }
}

fn merge_presence_snapshot(state: &AppState, snapshot: PresenceSnapshot) -> PresenceSnapshot {
    let previous = state.last_presence.lock().clone();
    let Some(previous) = previous else {
        return snapshot;
    };

    PresenceSnapshot {
        connected: if snapshot.has_connection_status {
            snapshot.connected
        } else {
            previous.connected
        },
        has_connection_status: snapshot.has_connection_status || previous.has_connection_status,
        mic_muted: snapshot.mic_muted.or(previous.mic_muted),
        battery_percent: snapshot.battery_percent.or(previous.battery_percent),
        game_volume: snapshot.game_volume.or(previous.game_volume),
        chat_volume: snapshot.chat_volume.or(previous.chat_volume),
        raw_response: snapshot.raw_response,
        device_path: snapshot.device_path.or(previous.device_path),
        error: snapshot.error,
        observed_at_ms: snapshot.observed_at_ms,
    }
}

fn presence_changed(state: &AppState, next: &PresenceSnapshot) -> bool {
    let previous = state.last_presence.lock().clone();
    let Some(previous) = previous else {
        return true;
    };

    previous.connected != next.connected
        || previous.mic_muted != next.mic_muted
        || previous.battery_percent != next.battery_percent
        || previous.game_volume != next.game_volume
        || previous.chat_volume != next.chat_volume
        || previous.error != next.error
}

fn handle_presence_snapshot(
    app: &AppHandle,
    state: &AppState,
    snapshot: PresenceSnapshot,
    initial: bool,
) {
    let connected = snapshot.connected;
    *state.last_presence.lock() = Some(snapshot.clone());
    if let Err(error) = sync_chatmix(
        state,
        &snapshot,
        if initial { "initial" } else { "presence" },
    ) {
        push_event(
            state,
            DiagnosticEvent::warn(format!("ChatMix apply failed: {error}")),
        );
    }

    if let Some(error) = snapshot.error {
        push_event(
            state,
            DiagnosticEvent::warn(format!("Presence event failed: {error}")),
        );
        let _ = app.emit("autoswapper://state-changed", now_ms());
        return;
    }

    let decision = match decide_current(state, connected, snapshot.has_connection_status) {
        Ok(decision) => decision,
        Err(error) => {
            push_event(
                state,
                DiagnosticEvent::warn(format!("Could not enumerate audio endpoints: {error}")),
            );
            let _ = app.emit("autoswapper://state-changed", now_ms());
            return;
        }
    };
    match apply_decision(state, &decision) {
        Ok(()) => push_event(
            state,
            DiagnosticEvent::info(format!(
                "{} autoswitch applied: {} ({})",
                if initial { "Initial" } else { "Event" },
                decision.reason,
                if connected {
                    "connected"
                } else {
                    "disconnected"
                }
            )),
        ),
        Err(error) => push_event(
            state,
            DiagnosticEvent::warn(format!("Autoswitch failed: {error}")),
        ),
    }
    let _ = app.emit("autoswapper://state-changed", now_ms());
}

/// Enumerate the current endpoints, compute which are available given the headset
/// presence, and run the priority rules against the saved config.
fn decide_current(
    state: &AppState,
    headset_connected: bool,
    has_status: bool,
) -> anyhow::Result<SwitchDecision> {
    let endpoints = state.audio.endpoints()?;
    let (render, capture) = available_ids(&endpoints, headset_connected, has_status);
    let config = state.config.lock().clone();
    let mut decision = decide_switch(&config, &render, &capture);
    let original_action_count = decision.actions.len();
    decision.actions.retain(|action| match action {
        DecisionAction::SetRenderDefault { endpoint_id } => {
            !is_default_for_all_roles(&endpoints, endpoint_id, EndpointFlow::Render)
        }
        DecisionAction::SetCaptureDefault { endpoint_id } => {
            !is_default_for_all_roles(&endpoints, endpoint_id, EndpointFlow::Capture)
        }
    });

    if original_action_count > 0 && decision.actions.is_empty() {
        decision.reason = format!("{}; already default", decision.reason);
    }

    Ok(decision)
}

/// An endpoint is available when it is Active and — for presence-tracked devices
/// (SteelSeries dongles) — only when we positively know the headset is connected.
/// When the connection state is unknown we keep tracked devices available so we
/// never switch away from a headset just because HID has not reported yet.
fn available_ids(
    endpoints: &[AudioEndpoint],
    headset_connected: bool,
    has_status: bool,
) -> (HashSet<String>, HashSet<String>) {
    let mut render = HashSet::new();
    let mut capture = HashSet::new();
    for endpoint in endpoints {
        if endpoint.state != EndpointState::Active {
            continue;
        }
        if endpoint.is_presence_tracked && has_status && !headset_connected {
            continue;
        }
        match endpoint.flow {
            EndpointFlow::Render => render.insert(endpoint.id.clone()),
            EndpointFlow::Capture => capture.insert(endpoint.id.clone()),
        };
    }
    (render, capture)
}

fn apply_decision(state: &AppState, decision: &SwitchDecision) -> anyhow::Result<()> {
    for action in &decision.actions {
        match action {
            DecisionAction::SetRenderDefault { endpoint_id } => {
                push_event(
                    state,
                    DiagnosticEvent::info(format!("Setting render default: {endpoint_id}")),
                );
                state
                    .audio
                    .set_default(endpoint_id, crate::models::EndpointFlow::Render)?;
            }
            DecisionAction::SetCaptureDefault { endpoint_id } => {
                push_event(
                    state,
                    DiagnosticEvent::info(format!("Setting capture default: {endpoint_id}")),
                );
                state
                    .audio
                    .set_default(endpoint_id, crate::models::EndpointFlow::Capture)?;
            }
        }
    }
    Ok(())
}

fn is_default_for_all_roles(
    endpoints: &[AudioEndpoint],
    endpoint_id: &str,
    flow: EndpointFlow,
) -> bool {
    endpoints
        .iter()
        .find(|endpoint| endpoint.id == endpoint_id && endpoint.flow == flow)
        .map(|endpoint| {
            endpoint.is_default_console
                && endpoint.is_default_multimedia
                && endpoint.is_default_communications
        })
        .unwrap_or(false)
}

fn sync_chatmix(state: &AppState, presence: &PresenceSnapshot, reason: &str) -> anyhow::Result<()> {
    let config = state.config.lock().clone();
    let sessions = state.audio.render_sessions(&config.chatmix)?;
    let changes = if config.debug.chatmix_enabled {
        state.chatmix.lock().sync(
            &sessions,
            presence.connected && presence.has_connection_status,
            presence.game_volume,
            presence.chat_volume,
        )
    } else {
        state.chatmix.lock().sync(&sessions, false, None, None)
    };
    for change in changes {
        let session = sessions
            .iter()
            .find(|session| session.id == change.session_id);
        let app = session
            .map(|session| session.display_name.as_str())
            .unwrap_or("unknown session");
        let old_volume = session.map(|session| session.volume).unwrap_or_default();
        let route = session
            .map(|session| format!("{:?}", session.route))
            .unwrap_or_else(|| "Unknown".to_string());
        let mode = if config.debug.chatmix_enabled {
            "ChatMix"
        } else {
            "ChatMix disabled restore"
        };
        let message = format!(
            "{mode} {reason}: {app} {route} {:.0}% -> {:.0}% (game={:?}, chat={:?})",
            old_volume * 100.0,
            change.volume * 100.0,
            presence.game_volume,
            presence.chat_volume
        );
        if config.debug.chatmix_dry_run {
            push_event(state, DiagnosticEvent::info(format!("dry-run {message}")));
        } else {
            push_event(state, DiagnosticEvent::info(message));
            state
                .audio
                .set_session_volume(&change.session_id, change.volume)?;
        }
    }
    Ok(())
}

fn push_event(state: &AppState, event: DiagnosticEvent) {
    let mut diagnostics = state.diagnostics.lock();
    diagnostics.push_back(event);
    while diagnostics.len() > MAX_EVENTS {
        diagnostics.pop_front();
    }
}

fn config_path(app: &AppHandle) -> anyhow::Result<PathBuf> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|error| anyhow::anyhow!("failed to resolve app config directory: {error}"))?;
    Ok(dir.join("config.json"))
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    let mut tray = TrayIconBuilder::new()
        .tooltip("HandoffGG")
        .menu(&menu)
        .show_menu_on_left_click(true);
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }

    tray.on_menu_event(|app, event| match event.id.as_ref() {
        "open" => {
            let _ = show_main_window(app);
        }
        "quit" => app.exit(0),
        _ => {}
    })
    .build(app)?;

    Ok(())
}

fn show_main_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window.show()?;
        window.set_focus()?;
    } else {
        if let Some(state) = app.try_state::<SharedState>() {
            state
                .settings_window_pending_show
                .store(true, Ordering::SeqCst);
        }
        WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
            .title("HandoffGG")
            .inner_size(983.0, 667.0)
            .min_inner_size(760.0, 560.0)
            .decorations(false)
            .theme(Some(Theme::Dark))
            .background_color(WINDOW_BACKGROUND)
            .visible(false)
            .build()?;
    }
    Ok(())
}
