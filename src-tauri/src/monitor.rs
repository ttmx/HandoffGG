//! The background monitor threads: the event-driven HID presence loop and the Windows
//! audio-device-change loop, plus the snapshot pipeline that reacts to both.

use crate::app_state::{push_event, AppState, SharedState};
use crate::models::{now_ms, DiagnosticEvent, PresenceSnapshot};
use crate::switch::{apply_decision, decide_current};
use crate::volume::{log_chatmix_wheel, sync_chatmix};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub(crate) fn start_monitor(app: AppHandle, state: SharedState) {
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
pub(crate) fn start_audio_device_monitor(app: AppHandle, state: SharedState) {
    let (tx, rx) = std::sync::mpsc::channel();
    crate::windows_audio::start_endpoint_notification_listener(tx);
    run_audio_device_monitor(app, state, rx);
}

#[cfg(target_os = "linux")]
pub(crate) fn start_audio_device_monitor(app: AppHandle, state: SharedState) {
    // The PipeWire backend fires this channel on sink/source add/remove and default
    // changes — the Linux equivalent of the Windows IMMNotificationClient.
    let Some(rx) = state.audio.take_change_receiver() else {
        return;
    };
    run_audio_device_monitor(app, state, rx);
}

#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) fn start_audio_device_monitor(_app: AppHandle, _state: SharedState) {}

/// Shared audio-device-change loop: on each notification (debounced over a 200ms burst),
/// re-run the switch decision and re-apply ChatMix against the current presence, then
/// nudge the UI. Driven by the Windows endpoint listener and the Linux PipeWire backend.
#[cfg(any(windows, target_os = "linux"))]
fn run_audio_device_monitor(
    app: AppHandle,
    state: SharedState,
    rx: std::sync::mpsc::Receiver<()>,
) {
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
                        DiagnosticEvent::warn(format!("ChatMix apply failed: {error}"))
                            .category("chatmix"),
                    );
                }
            }

            let _ = app.emit("autoswapper://state-changed", now_ms());
        }
    });
}

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

    // A report that carries the wheel position is a chatmix event (0x45 wheel turn).
    // Log the raw reading before merging so the actual reported value is visible.
    if snapshot.game_volume.is_some() || snapshot.chat_volume.is_some() {
        log_chatmix_wheel(state, "wheel", snapshot.game_volume, snapshot.chat_volume);
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
        let connected = merged_snapshot.connected;
        handle_presence_snapshot(app, state, merged_snapshot, false);
        // The connection event itself carries no chatmix wheel position, so once we
        // know the headset just connected, actively read the current wheel state.
        if connected {
            refresh_chatmix_on_connect(app, state);
        }
    } else {
        let _ = app.emit("autoswapper://state-changed", now_ms());
    }
}

/// Re-query the status interface after a connect to seed the chatmix wheel position.
/// The dongle only pushes the wheel state when it moves, so without this the app keeps
/// stale (or unknown) game/chat values until the user touches the wheel.
fn refresh_chatmix_on_connect(app: &AppHandle, state: &AppState) {
    let status = state.presence.lock().refresh_status();
    let (Some(game), Some(chat)) = (status.game_volume, status.chat_volume) else {
        return;
    };
    log_chatmix_wheel(state, "connect", Some(game), Some(chat));

    let updated = {
        let mut guard = state.last_presence.lock();
        match guard.as_mut() {
            // Ignore the late wheel read if the headset has since disconnected.
            Some(current) if current.connected => {
                current.game_volume = Some(game);
                current.chat_volume = Some(chat);
                Some(current.clone())
            }
            _ => None,
        }
    };

    let Some(presence) = updated else {
        return;
    };
    if let Err(error) = sync_chatmix(state, &presence, "connect") {
        push_event(
            state,
            DiagnosticEvent::warn(format!("ChatMix apply failed: {error}")),
        );
    }
    let _ = app.emit("autoswapper://state-changed", now_ms());
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
    if initial && (snapshot.game_volume.is_some() || snapshot.chat_volume.is_some()) {
        log_chatmix_wheel(state, "initial", snapshot.game_volume, snapshot.chat_volume);
    }
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
