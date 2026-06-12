//! Machine-readable headset status for external consumers (waybar/polybar modules,
//! GNOME extensions, scripts) — the no-tray answer to "what's my battery at?".
//!
//! On every presence change the monitor writes a small JSON file; consumers poll or
//! watch it. The file may be stale after a crash, so `observedAtMs` is included for
//! consumers that care. Writes are atomic (temp file + rename) so readers never see
//! a half-written document.

use crate::models::PresenceSnapshot;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusFile<'a> {
    connected: bool,
    battery_percent: Option<u8>,
    mic_muted: Option<bool>,
    game_volume: Option<u8>,
    chat_volume: Option<u8>,
    error: Option<&'a str>,
    observed_at_ms: u128,
}

/// `$XDG_RUNTIME_DIR/handoffgg/status.json` where a runtime dir exists (Linux),
/// otherwise `<temp>/handoffgg/status.json` (the Windows location).
fn status_file_path() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("handoffgg").join("status.json")
}

/// Mirror the snapshot to the status file. Failures are non-fatal: the file is a
/// convenience for external tooling, never load-bearing for the app itself.
pub(crate) fn write(presence: &PresenceSnapshot) {
    if let Err(error) = try_write(presence) {
        eprintln!("failed to write status file: {error}");
    }
}

fn try_write(presence: &PresenceSnapshot) -> std::io::Result<()> {
    let path = status_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(&StatusFile {
        connected: presence.connected,
        battery_percent: presence.battery_percent,
        mic_muted: presence.mic_muted,
        game_volume: presence.game_volume,
        chat_volume: presence.chat_volume,
        error: presence.error.as_deref(),
        observed_at_ms: presence.observed_at_ms,
    })?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)
}
