//! Shared application state and the small helpers that operate directly on it.

use crate::audio::AudioBackend;
use crate::chatmix::ChatMixVolumeManager;
use crate::models::{AppConfig, DiagnosticEvent, PresenceSnapshot};
use crate::presence::HeadsetPresenceBackend;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tauri::{AppHandle, Manager};

/// Upper bound on the diagnostics ring buffer.
const MAX_EVENTS: usize = 200;

/// Everything the Tauri commands and the background monitor threads share. Managed by
/// Tauri as application state and cloned into the monitor threads as a [`SharedState`].
pub(crate) struct AppState {
    pub(crate) config_path: PathBuf,
    pub(crate) config: Mutex<AppConfig>,
    pub(crate) audio: Arc<dyn AudioBackend>,
    pub(crate) presence: Mutex<Box<dyn HeadsetPresenceBackend>>,
    pub(crate) chatmix: Mutex<ChatMixVolumeManager>,
    pub(crate) last_presence: Mutex<Option<PresenceSnapshot>>,
    pub(crate) diagnostics: Mutex<VecDeque<DiagnosticEvent>>,
    pub(crate) settings_window_pending_show: AtomicBool,
}

pub(crate) type SharedState = Arc<AppState>;

/// Append a diagnostic event, trimming the oldest once the ring buffer is full.
pub(crate) fn push_event(state: &AppState, event: DiagnosticEvent) {
    let mut diagnostics = state.diagnostics.lock();
    diagnostics.push_back(event);
    while diagnostics.len() > MAX_EVENTS {
        diagnostics.pop_front();
    }
}

/// Resolve the on-disk path of the app's `config.json`.
pub(crate) fn config_path(app: &AppHandle) -> anyhow::Result<PathBuf> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|error| anyhow::anyhow!("failed to resolve app config directory: {error}"))?;
    Ok(dir.join("config.json"))
}
