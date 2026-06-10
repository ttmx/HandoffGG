use crate::models::{AudioSession, ChatMixConfig, ChatMixRoute};
use std::collections::{HashMap, HashSet};

const VOLUME_EPSILON: f32 = 0.015;
const APPLIED_VOLUME_EPSILON: f32 = 0.035;

#[derive(Debug, Clone)]
struct SessionBaseline {
    baseline_volume: f32,
    last_applied_volume: Option<f32>,
}

#[derive(Debug, Default)]
pub struct ChatMixVolumeManager {
    baselines: HashMap<String, SessionBaseline>,
}

impl ChatMixVolumeManager {
    pub fn sync(
        &mut self,
        sessions: &[AudioSession],
        connected: bool,
        game_volume: Option<u8>,
        chat_volume: Option<u8>,
    ) -> Vec<VolumeChange> {
        let active_ids = sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<HashSet<_>>();
        self.baselines.retain(|id, _| active_ids.contains(id));

        if !connected {
            return self.restore(sessions);
        }

        let (Some(game_volume), Some(chat_volume)) = (game_volume, chat_volume) else {
            return self.restore(sessions);
        };

        let game_factor = factor(game_volume);
        let chat_factor = factor(chat_volume);

        if nearly_equal(game_factor, 1.0) && nearly_equal(chat_factor, 1.0) {
            return self.restore(sessions);
        }

        let mut changes = Vec::new();
        for session in sessions {
            let route_factor = match session.route {
                ChatMixRoute::Game => game_factor,
                ChatMixRoute::Chat => chat_factor,
            };
            let entry =
                self.baselines
                    .entry(session.id.clone())
                    .or_insert_with(|| SessionBaseline {
                        baseline_volume: session.volume,
                        last_applied_volume: None,
                    });

            if let Some(last_applied) = entry.last_applied_volume {
                if nearly_equal_with(session.volume, last_applied, APPLIED_VOLUME_EPSILON) {
                    let target = clamp_volume(entry.baseline_volume * route_factor);
                    if !nearly_equal_with(session.volume, target, APPLIED_VOLUME_EPSILON) {
                        entry.last_applied_volume = Some(target);
                        changes.push(VolumeChange {
                            session_id: session.id.clone(),
                            volume: target,
                        });
                    }
                    continue;
                }

                entry.baseline_volume = session.volume;
            } else {
                entry.baseline_volume = session.volume;
            }

            let target = clamp_volume(entry.baseline_volume * route_factor);
            if !nearly_equal(session.volume, target) {
                entry.last_applied_volume = Some(target);
                changes.push(VolumeChange {
                    session_id: session.id.clone(),
                    volume: target,
                });
            }
        }

        changes
    }

    fn restore(&mut self, sessions: &[AudioSession]) -> Vec<VolumeChange> {
        let mut changes = Vec::new();
        for session in sessions {
            let Some(entry) = self.baselines.get_mut(&session.id) else {
                continue;
            };
            if !nearly_equal(session.volume, entry.baseline_volume) {
                changes.push(VolumeChange {
                    session_id: session.id.clone(),
                    volume: entry.baseline_volume,
                });
            }
            entry.last_applied_volume = None;
        }
        changes
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VolumeChange {
    pub session_id: String,
    pub volume: f32,
}

pub fn route_for_app(
    app_id: &str,
    display_name: &str,
    executable_path: Option<&str>,
    config: &ChatMixConfig,
) -> (ChatMixRoute, String) {
    if let Some(saved) = config.app_routes.get(app_id) {
        return (saved.route, "manual".to_string());
    }

    let executable = executable_path
        .and_then(file_name)
        .unwrap_or(app_id)
        .to_ascii_lowercase();
    let name = display_name.to_ascii_lowercase();
    if is_chat_executable(&executable) || is_chat_name(&name) {
        (ChatMixRoute::Chat, "heuristic".to_string())
    } else {
        (ChatMixRoute::Game, "default".to_string())
    }
}

pub fn app_id_for_session(
    executable_path: Option<&str>,
    display_name: &str,
    process_id: u32,
) -> String {
    executable_path
        .map(normalize_app_key)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            let fallback = normalize_app_key(display_name);
            if fallback.is_empty() {
                format!("pid:{process_id}")
            } else {
                fallback
            }
        })
}

fn factor(value: u8) -> f32 {
    clamp_volume(value as f32 / 100.0)
}

fn clamp_volume(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

fn nearly_equal(a: f32, b: f32) -> bool {
    (a - b).abs() <= VOLUME_EPSILON
}

fn nearly_equal_with(a: f32, b: f32, epsilon: f32) -> bool {
    (a - b).abs() <= epsilon
}

fn normalize_app_key(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn file_name(path: &str) -> Option<&str> {
    path.rsplit(['\\', '/'])
        .next()
        .filter(|value| !value.is_empty())
}

fn is_chat_executable(executable: &str) -> bool {
    const CHAT_EXES: &[&str] = &[
        "discord.exe",
        "teams.exe",
        "msteams.exe",
        "slack.exe",
        "zoom.exe",
        "skype.exe",
        "telegram.exe",
        "whatsapp.exe",
        "signal.exe",
        "mumble.exe",
        "ts3client_win64.exe",
        "ts3client_win32.exe",
        "teamspeak.exe",
    ];
    CHAT_EXES.contains(&executable)
}

fn is_chat_name(name: &str) -> bool {
    [
        "discord",
        "teams",
        "slack",
        "zoom",
        "skype",
        "telegram",
        "whatsapp",
        "signal",
        "mumble",
        "teamspeak",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::{route_for_app, ChatMixVolumeManager};
    use crate::models::{AudioSession, ChatMixConfig, ChatMixRoute};

    fn session(id: &str, route: ChatMixRoute, volume: f32) -> AudioSession {
        AudioSession {
            id: id.to_string(),
            app_id: id.to_string(),
            display_name: id.to_string(),
            executable_path: None,
            process_id: 42,
            route,
            route_source: "test".to_string(),
            volume,
            muted: false,
        }
    }

    #[test]
    fn chat_heuristic_classifies_discord_as_chat() {
        let config = ChatMixConfig::default();
        assert_eq!(
            route_for_app(
                "discord.exe",
                "Discord",
                Some("C:\\Discord\\Discord.exe"),
                &config
            )
            .0,
            ChatMixRoute::Chat
        );
    }

    #[test]
    fn browser_defaults_to_game() {
        let config = ChatMixConfig::default();
        assert_eq!(
            route_for_app(
                "chrome.exe",
                "Google Chrome",
                Some("C:\\Chrome\\chrome.exe"),
                &config
            )
            .0,
            ChatMixRoute::Game
        );
    }

    #[test]
    fn unknown_apps_default_to_game() {
        let config = ChatMixConfig::default();
        assert_eq!(
            route_for_app("game.exe", "Some Game", None, &config).0,
            ChatMixRoute::Game
        );
    }

    #[test]
    fn baseline_math_preserves_user_volume() {
        let mut manager = ChatMixVolumeManager::default();
        let changes = manager.sync(
            &[session("discord", ChatMixRoute::Chat, 0.5)],
            true,
            Some(100),
            Some(40),
        );
        assert_eq!(changes.len(), 1);
        assert!((changes[0].volume - 0.2).abs() < 0.001);
    }

    #[test]
    fn external_volume_change_updates_baseline() {
        let mut manager = ChatMixVolumeManager::default();
        let first = manager.sync(
            &[session("discord", ChatMixRoute::Chat, 0.5)],
            true,
            Some(100),
            Some(50),
        );
        assert!((first[0].volume - 0.25).abs() < 0.001);

        let second = manager.sync(
            &[session("discord", ChatMixRoute::Chat, 0.8)],
            true,
            Some(100),
            Some(50),
        );
        assert!((second[0].volume - 0.4).abs() < 0.001);
    }

    #[test]
    fn applied_volume_rounding_does_not_decay_baseline() {
        let mut manager = ChatMixVolumeManager::default();
        let first = manager.sync(
            &[session("browser", ChatMixRoute::Game, 0.8)],
            true,
            Some(50),
            Some(100),
        );
        assert!((first[0].volume - 0.4).abs() < 0.001);

        let second = manager.sync(
            &[session("browser", ChatMixRoute::Game, 0.38)],
            true,
            Some(50),
            Some(100),
        );
        assert!(second.is_empty());
    }

    #[test]
    fn chatmix_factor_change_applies_after_previous_managed_volume() {
        let mut manager = ChatMixVolumeManager::default();
        let first = manager.sync(
            &[session("browser", ChatMixRoute::Game, 0.8)],
            true,
            Some(50),
            Some(100),
        );
        assert!((first[0].volume - 0.4).abs() < 0.001);

        let second = manager.sync(
            &[session("browser", ChatMixRoute::Game, 0.4)],
            true,
            Some(25),
            Some(100),
        );
        assert_eq!(second.len(), 1);
        assert!((second[0].volume - 0.2).abs() < 0.001);
    }

    #[test]
    fn disconnected_restores_baseline() {
        let mut manager = ChatMixVolumeManager::default();
        let first = manager.sync(
            &[session("game", ChatMixRoute::Game, 0.6)],
            true,
            Some(50),
            Some(100),
        );
        assert!((first[0].volume - 0.3).abs() < 0.001);

        let restore = manager.sync(
            &[session("game", ChatMixRoute::Game, 0.3)],
            false,
            Some(50),
            Some(100),
        );
        assert!((restore[0].volume - 0.6).abs() < 0.001);
    }
}
