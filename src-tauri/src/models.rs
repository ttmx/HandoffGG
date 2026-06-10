use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EndpointFlow {
    Render,
    Capture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EndpointState {
    Active,
    Disabled,
    NotPresent,
    Unplugged,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioEndpoint {
    pub id: String,
    pub name: String,
    pub flow: EndpointFlow,
    pub state: EndpointState,
    /// True for endpoints (e.g. SteelSeries/Arctis) whose real availability depends
    /// on the wireless HID presence rather than the Windows endpoint state, because
    /// their USB dongle stays present even while the headset is powered off.
    pub is_presence_tracked: bool,
    pub is_default_console: bool,
    pub is_default_multimedia: bool,
    pub is_default_communications: bool,
}

/// One entry in a flow's priority list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicePref {
    /// Windows endpoint id (stable identifier).
    pub id: String,
    /// Last-known friendly name, retained so offline devices still render.
    pub name: String,
    /// When true the device sits in the "ignore" section and is never selected.
    #[serde(default)]
    pub excluded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowConfig {
    /// Ordered priority list: index 0 is the highest priority.
    #[serde(default)]
    pub priorities: Vec<DevicePref>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub autoswitch_enabled: bool,
    #[serde(default)]
    pub output: FlowConfig,
    #[serde(default)]
    pub input: FlowConfig,
    #[serde(default)]
    pub chatmix: ChatMixConfig,
    #[serde(default)]
    pub debug: DebugConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            autoswitch_enabled: true,
            output: FlowConfig::default(),
            input: FlowConfig::default(),
            chatmix: ChatMixConfig::default(),
            debug: DebugConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DebugConfig {
    #[serde(default = "default_true")]
    pub chatmix_enabled: bool,
    #[serde(default)]
    pub chatmix_dry_run: bool,
    #[serde(default = "default_true")]
    pub audio_session_polling_enabled: bool,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            chatmix_enabled: true,
            chatmix_dry_run: false,
            audio_session_polling_enabled: true,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChatMixRoute {
    Game,
    Chat,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMixAppRoute {
    pub route: ChatMixRoute,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMixConfig {
    #[serde(default)]
    pub app_routes: HashMap<String, ChatMixAppRoute>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSession {
    pub id: String,
    pub app_id: String,
    pub display_name: String,
    pub executable_path: Option<String>,
    pub process_id: u32,
    pub route: ChatMixRoute,
    pub route_source: String,
    pub volume: f32,
    pub muted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceSnapshot {
    pub connected: bool,
    pub has_connection_status: bool,
    pub mic_muted: Option<bool>,
    pub battery_percent: Option<u8>,
    pub game_volume: Option<u8>,
    pub chat_volume: Option<u8>,
    pub raw_response: Option<String>,
    pub device_path: Option<String>,
    pub error: Option<String>,
    pub observed_at_ms: u128,
}

impl PresenceSnapshot {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            connected: false,
            has_connection_status: false,
            mic_muted: None,
            battery_percent: None,
            game_volume: None,
            chat_volume: None,
            raw_response: None,
            device_path: None,
            error: Some(message.into()),
            observed_at_ms: now_ms(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DecisionAction {
    SetRenderDefault { endpoint_id: String },
    SetCaptureDefault { endpoint_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchDecision {
    pub reason: String,
    pub actions: Vec<DecisionAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticEvent {
    pub timestamp_ms: u128,
    pub level: String,
    /// Coarse grouping used by the diagnostics UI filter. "general" by default;
    /// chatmix-related events use "chatmix" so they can be shown/hidden on their own.
    pub category: String,
    pub message: String,
}

impl DiagnosticEvent {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            timestamp_ms: now_ms(),
            level: "info".to_string(),
            category: "general".to_string(),
            message: message.into(),
        }
    }

    pub fn warn(message: impl Into<String>) -> Self {
        Self {
            timestamp_ms: now_ms(),
            level: "warn".to_string(),
            category: "general".to_string(),
            message: message.into(),
        }
    }

    /// Tag this event with a category (e.g. "chatmix") for UI filtering.
    pub fn category(mut self, category: impl Into<String>) -> Self {
        self.category = category.into();
        self
    }
}

pub fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}
