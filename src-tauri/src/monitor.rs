//! The background monitor threads: the event-driven HID presence loop and the Windows
//! audio-device-change loop, plus the snapshot pipeline that reacts to both.

use crate::app_state::{push_event, AppState, SharedState};
use crate::models::{now_ms, DiagnosticEvent, PresenceSnapshot};
use crate::presence::merge_partial_snapshot;
use crate::status_file;
use crate::switch::{apply_decision, decide_current};
use crate::volume::{log_chatmix_wheel, sync_chatmix};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;

/// Emitted (with a `now_ms` payload) whenever backend state changed enough that the
/// settings UI should refetch. Mirrored by `STATE_CHANGED_EVENT` in `native.svelte.ts`.
pub(crate) const STATE_CHANGED_EVENT: &str = "autoswapper://state-changed";

pub(crate) fn start_monitor(app: AppHandle, state: SharedState) {
    thread::spawn(move || {
        let initial_snapshot = state.presence.lock().snapshot();
        let mut stable_connected = Some(initial_snapshot.connected);
        process_snapshot(&app, &state, initial_snapshot, &mut stable_connected, true);

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
            process_snapshot(&app, &state, snapshot, &mut stable_connected, false);
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
fn run_audio_device_monitor(app: AppHandle, state: SharedState, rx: std::sync::mpsc::Receiver<()>) {
    thread::spawn(move || {
        while rx.recv().is_ok() {
            while rx.recv_timeout(Duration::from_millis(200)).is_ok() {}

            let presence = state.last_presence.lock().clone();
            let connected = presence.as_ref().is_some_and(|p| p.connected);
            let has_status = presence.as_ref().is_some_and(|p| p.has_connection_status);
            decide_and_apply(&state, connected, has_status, "Device-change");

            if let Some(presence) = presence {
                sync_chatmix(&state, &presence, "audio_device");
            }

            let _ = app.emit(STATE_CHANGED_EVENT, now_ms());
        }
    });
}

/// The single snapshot pipeline, for both the startup probe (`initial`) and every
/// subsequent HID event: log the wheel, merge into the stored state, then run the
/// side effects (ChatMix, switch decision, UI nudge) the change calls for.
fn process_snapshot(
    app: &AppHandle,
    state: &AppState,
    snapshot: PresenceSnapshot,
    stable_connected: &mut Option<bool>,
    initial: bool,
) {
    // A report that carries the wheel position is a chatmix event (0x45 wheel turn or
    // the startup 0xB0 poll). Log the raw reading before merging so it stays visible.
    if snapshot.game_volume.is_some() || snapshot.chat_volume.is_some() {
        log_chatmix_wheel(
            state,
            if initial { "initial" } else { "wheel" },
            snapshot.game_volume,
            snapshot.chat_volume,
        );
    }

    // An error snapshot (e.g. the startup probe found no readable interface) replaces
    // the stored state wholesale — there is nothing to merge and no decision to make.
    if let Some(error) = snapshot.error.clone() {
        *state.last_presence.lock() = Some(snapshot.clone());
        status_file::write(&snapshot);
        sync_chatmix(
            state,
            &snapshot,
            if initial { "initial" } else { "presence" },
        );
        push_event(
            state,
            DiagnosticEvent::warn(format!("Presence event failed: {error}")),
        );
        let _ = app.emit(STATE_CHANGED_EVENT, now_ms());
        return;
    }

    // Merge the (partial) report into the stored state under a single lock, and
    // classify what changed while both old and new are at hand.
    let (merged, changed, connection_changed, previous_battery) = {
        let mut guard = state.last_presence.lock();
        let (merged, changed, previous_battery) = match guard.take() {
            Some(previous) => {
                let merged = merge_event_snapshot(&previous, snapshot);
                let changed = previous.connected != merged.connected
                    || previous.mic_muted != merged.mic_muted
                    || previous.battery_percent != merged.battery_percent
                    || previous.game_volume != merged.game_volume
                    || previous.chat_volume != merged.chat_volume
                    || previous.error != merged.error;
                (merged, changed, previous.battery_percent)
            }
            None => (snapshot, true, None),
        };
        let connection_changed =
            merged.has_connection_status && *stable_connected != Some(merged.connected);
        *guard = Some(merged.clone());
        (merged, changed, connection_changed, previous_battery)
    };

    // Repeated reports with no observable change need no side effects at all.
    if !changed && !initial {
        return;
    }

    status_file::write(&merged);
    maybe_notify_low_battery(app, state, previous_battery, &merged);
    sync_chatmix(
        state,
        &merged,
        if initial { "initial" } else { "hid_event" },
    );

    if initial || connection_changed {
        *stable_connected = Some(merged.connected);
        decide_and_apply(
            state,
            merged.connected,
            merged.has_connection_status,
            if initial { "Initial" } else { "Event" },
        );
        // The connection event itself carries no chatmix wheel position, so once we
        // know the headset just connected, actively read the current wheel state.
        if !initial && merged.connected {
            refresh_chatmix_on_connect(state);
        }
    }

    let _ = app.emit(STATE_CHANGED_EVENT, now_ms());
}

/// One-shot desktop notification when the battery crosses down to the configured
/// threshold while connected. Crossing-based, so charging back above the threshold
/// re-arms it, and a startup that already sees a low battery notifies once.
fn maybe_notify_low_battery(
    app: &AppHandle,
    state: &AppState,
    previous_battery: Option<u8>,
    merged: &PresenceSnapshot,
) {
    let threshold = state.config.lock().low_battery_percent;
    if threshold == 0 || !merged.connected {
        return;
    }
    let Some(battery) = merged.battery_percent else {
        return;
    };
    let already_notified = previous_battery.is_some_and(|previous| previous <= threshold);
    if battery > threshold || already_notified {
        return;
    }

    match app
        .notification()
        .builder()
        .title("Headset battery low")
        .body(format!("{battery}% battery remaining"))
        .show()
    {
        Ok(()) => push_event(
            state,
            DiagnosticEvent::info(format!("Low battery notification sent ({battery}%)")),
        ),
        Err(error) => push_event(
            state,
            DiagnosticEvent::warn(format!("Battery notification failed: {error}")),
        ),
    }
}

/// Run the switch decision for the given presence and apply it, logging the outcome
/// under the given label ("Initial", "Event", "Device-change").
fn decide_and_apply(state: &AppState, connected: bool, has_status: bool, label: &str) {
    let decision = match decide_current(state, connected, has_status) {
        Ok(decision) => decision,
        Err(error) => {
            push_event(
                state,
                DiagnosticEvent::warn(format!("Could not enumerate audio endpoints: {error}")),
            );
            return;
        }
    };
    match apply_decision(state, &decision) {
        Ok(()) => push_event(
            state,
            DiagnosticEvent::info(format!(
                "{label} autoswitch applied: {} ({})",
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
}

/// Re-query the status interface after a connect to seed the chatmix wheel position.
/// The dongle only pushes the wheel state when it moves, so without this the app keeps
/// stale (or unknown) game/chat values until the user touches the wheel.
fn refresh_chatmix_on_connect(state: &AppState) {
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

    if let Some(presence) = updated {
        sync_chatmix(state, &presence, "connect");
    }
}

/// Merge an event report into the previous state: fields the report does not carry are
/// kept from before (see [`merge_partial_snapshot`]), but `raw_response` and `error`
/// always reflect the newest event, so a stale error never outlives the report that
/// superseded it.
fn merge_event_snapshot(previous: &PresenceSnapshot, next: PresenceSnapshot) -> PresenceSnapshot {
    PresenceSnapshot {
        raw_response: next.raw_response.clone(),
        error: next.error.clone(),
        ..merge_partial_snapshot(previous.clone(), next)
    }
}
