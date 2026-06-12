//! Applying ChatMix to per-app session volumes, and the diagnostics that go with it.

use crate::app_state::{push_event, AppState};
use crate::models::{DiagnosticCategory, DiagnosticEvent, PresenceSnapshot};

/// [`try_sync_chatmix`], with failures logged to the diagnostics panel (under the
/// `chatmix` category) instead of returned — every background caller handles them
/// identically, so the logging lives here.
pub(crate) fn sync_chatmix(state: &AppState, presence: &PresenceSnapshot, reason: &str) {
    if let Err(error) = try_sync_chatmix(state, presence, reason) {
        push_event(
            state,
            DiagnosticEvent::warn(format!("ChatMix apply failed: {error}"))
                .category(DiagnosticCategory::Chatmix),
        );
    }
}

/// Recompute per-session volumes from the current ChatMix wheel position and apply the
/// changes (or restore baselines when ChatMix is disabled). Each adjustment is logged
/// to the diagnostics panel under the `chatmix` category.
pub(crate) fn try_sync_chatmix(
    state: &AppState,
    presence: &PresenceSnapshot,
    reason: &str,
) -> anyhow::Result<()> {
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
            push_event(
                state,
                DiagnosticEvent::info(format!("dry-run {message}"))
                    .category(DiagnosticCategory::Chatmix),
            );
        } else {
            push_event(
                state,
                DiagnosticEvent::info(message).category(DiagnosticCategory::Chatmix),
            );
            state
                .audio
                .set_session_volume(&change.session_id, change.volume)?;
        }
    }
    Ok(())
}

/// Surface a chatmix wheel reading in the diagnostics log. `source` records where the
/// value came from (a physical wheel turn, the on-connect re-query, or the startup poll)
/// so the "starts at the wrong level" class of bugs is visible from the UI alone.
pub(crate) fn log_chatmix_wheel(
    state: &AppState,
    source: &str,
    game: Option<u8>,
    chat: Option<u8>,
) {
    let fmt = |value: Option<u8>| value.map(|v| v.to_string()).unwrap_or_else(|| "?".into());
    push_event(
        state,
        DiagnosticEvent::info(format!(
            "ChatMix wheel ({source}): game={}, chat={}",
            fmt(game),
            fmt(chat)
        ))
        .category(DiagnosticCategory::Chatmix),
    );
}
