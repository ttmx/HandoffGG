//! End-to-end tests that inject raw SteelSeries HID report frames (the same bytes the
//! parser tests in `hid_report.rs` and the captures in `docs/hid-findings.md` use) through
//! the **platform-independent decision pipeline** the Linux PipeWire backend feeds:
//! `parse_event_snapshot` → `decide_current`/`apply_decision` (autoswitch) and
//! `sync_chatmix` (per-app volume).
//!
//! Real PipeWire I/O can't run in CI (no daemon), so the audio backend is a
//! [`MockAudioBackend`] that records the calls the live `PipewireAudioBackend` would make.
//! This validates that, given a USB packet, the app reaches the correct audio action —
//! independent of OS. The actual PipeWire backend is exercised by an ignored smoke test
//! (`pipewire_smoke`, run manually) and by hand on a live system.

use crate::app_state::AppState;
use crate::audio::AudioBackend;
use crate::chatmix::ChatMixVolumeManager;
use crate::models::{
    AppConfig, AudioEndpoint, AudioSession, ChatMixConfig, ChatMixRoute, DevicePref, EndpointFlow,
    EndpointState, FlowConfig, PresenceSnapshot,
};
use crate::presence::{merge_partial_snapshot, parse_event_snapshot, HeadsetPresenceBackend};
use crate::switch::{apply_decision, decide_current};
use crate::volume::try_sync_chatmix;
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

/// Records the side effects (`set_default`, `set_session_volume`) the real backend would
/// apply, and serves a fixed endpoint/session graph.
struct MockAudioBackend {
    endpoints: Vec<AudioEndpoint>,
    /// Mutable so applied volume changes are reflected in later `render_sessions` calls,
    /// exactly as the real backend reads back the live PipeWire `channelVolumes`.
    sessions: Mutex<Vec<AudioSession>>,
    default_calls: Mutex<Vec<(String, EndpointFlow)>>,
    volume_calls: Mutex<Vec<(String, f32)>>,
}

impl MockAudioBackend {
    fn new(endpoints: Vec<AudioEndpoint>, sessions: Vec<AudioSession>) -> Self {
        Self {
            endpoints,
            sessions: Mutex::new(sessions),
            default_calls: Mutex::new(Vec::new()),
            volume_calls: Mutex::new(Vec::new()),
        }
    }
}

impl AudioBackend for MockAudioBackend {
    fn endpoints(&self) -> anyhow::Result<Vec<AudioEndpoint>> {
        Ok(self.endpoints.clone())
    }

    fn set_default(&self, endpoint_id: &str, flow: EndpointFlow) -> anyhow::Result<()> {
        self.default_calls
            .lock()
            .push((endpoint_id.to_string(), flow));
        Ok(())
    }

    fn render_sessions(&self, _chatmix: &ChatMixConfig) -> anyhow::Result<Vec<AudioSession>> {
        Ok(self.sessions.lock().clone())
    }

    fn set_session_volume(&self, session_id: &str, volume: f32) -> anyhow::Result<()> {
        self.volume_calls
            .lock()
            .push((session_id.to_string(), volume));
        if let Some(session) = self
            .sessions
            .lock()
            .iter_mut()
            .find(|session| session.id == session_id)
        {
            session.volume = volume;
        }
        Ok(())
    }
}

/// A presence backend stub — the tests drive snapshots directly, so this is never polled.
struct MockPresence;

impl HeadsetPresenceBackend for MockPresence {
    fn snapshot(&mut self) -> PresenceSnapshot {
        PresenceSnapshot::error("mock presence")
    }
    fn wait_for_event(&mut self, _timeout: Duration) -> Option<PresenceSnapshot> {
        None
    }
    fn refresh_status(&mut self) -> PresenceSnapshot {
        PresenceSnapshot::error("mock presence")
    }
}

fn endpoint(id: &str, flow: EndpointFlow, presence_tracked: bool) -> AudioEndpoint {
    AudioEndpoint {
        id: id.to_string(),
        name: id.to_string(),
        flow,
        state: EndpointState::Active,
        is_presence_tracked: presence_tracked,
        is_default_console: false,
        is_default_multimedia: false,
        is_default_communications: false,
    }
}

fn session(id: &str, route: ChatMixRoute, volume: f32) -> AudioSession {
    AudioSession {
        id: id.to_string(),
        app_id: id.to_string(),
        display_name: id.to_string(),
        executable_path: None,
        process_id: 1234,
        route,
        route_source: "test".to_string(),
        volume,
        muted: false,
    }
}

fn pref(id: &str) -> DevicePref {
    DevicePref {
        id: id.to_string(),
        name: id.to_string(),
        excluded: false,
    }
}

fn make_state(audio: Arc<dyn AudioBackend>, config: AppConfig) -> AppState {
    AppState {
        config_path: std::env::temp_dir().join("handoffgg-pipeline-test-config.json"),
        config: Mutex::new(config),
        audio,
        presence: Mutex::new(Box::new(MockPresence)),
        chatmix: Mutex::new(ChatMixVolumeManager::default()),
        last_presence: Mutex::new(None),
        diagnostics: Mutex::new(VecDeque::new()),
        settings_window_pending_show: AtomicBool::new(false),
    }
}

/// Priority config: presence-tracked headset first, then a generic fallback, for both flows.
fn switch_config() -> AppConfig {
    AppConfig {
        autoswitch_enabled: true,
        output: FlowConfig {
            priorities: vec![pref("arctis-out"), pref("speaker-out")],
        },
        input: FlowConfig {
            priorities: vec![pref("arctis-mic"), pref("fallback-mic")],
        },
        chatmix: ChatMixConfig::default(),
        low_battery_percent: 0,
        debug: Default::default(),
    }
}

fn switch_backend() -> Arc<MockAudioBackend> {
    Arc::new(MockAudioBackend::new(
        vec![
            endpoint("arctis-out", EndpointFlow::Render, true),
            endpoint("speaker-out", EndpointFlow::Render, false),
            endpoint("arctis-mic", EndpointFlow::Capture, true),
            endpoint("fallback-mic", EndpointFlow::Capture, false),
        ],
        Vec::new(),
    ))
}

/// Run a presence snapshot through the autoswitch pipeline exactly as the monitor does.
fn apply_presence(state: &AppState, snapshot: &PresenceSnapshot) {
    let decision = decide_current(state, snapshot.connected, snapshot.has_connection_status)
        .expect("decide_current");
    apply_decision(state, &decision).expect("apply_decision");
}

#[test]
fn power_off_packet_switches_away_from_headset() {
    let backend = switch_backend();
    let state = make_state(backend.clone(), switch_config());

    // `B9 02` — unsolicited power-off event on MI_05.
    let snapshot = parse_event_snapshot(&[0xB9, 0x02], "test").expect("power-off snapshot");
    assert!(!snapshot.connected);
    assert!(snapshot.has_connection_status);

    apply_presence(&state, &snapshot);

    let calls = backend.default_calls.lock().clone();
    assert_eq!(
        calls,
        vec![
            ("speaker-out".to_string(), EndpointFlow::Render),
            ("fallback-mic".to_string(), EndpointFlow::Capture),
        ],
        "headset is presence-tracked and now off, so both flows fall through to the fallback"
    );
}

#[test]
fn power_on_packet_switches_to_headset() {
    let backend = switch_backend();
    let state = make_state(backend.clone(), switch_config());

    // `B9 03` — unsolicited power-on event on MI_05.
    let snapshot = parse_event_snapshot(&[0xB9, 0x03], "test").expect("power-on snapshot");
    assert!(snapshot.connected);
    assert!(snapshot.has_connection_status);

    apply_presence(&state, &snapshot);

    let calls = backend.default_calls.lock().clone();
    assert_eq!(
        calls,
        vec![
            ("arctis-out".to_string(), EndpointFlow::Render),
            ("arctis-mic".to_string(), EndpointFlow::Capture),
        ],
        "headset is connected, so it wins both priority lists"
    );
}

#[test]
fn status_poll_reply_seeds_connected_state() {
    let backend = switch_backend();
    let state = make_state(backend.clone(), switch_config());

    // `B0 03 44 03 63 64` — connected status reply (battery 68%, wheel game=99 chat=100).
    let snapshot = parse_event_snapshot(&[0xB0, 0x03, 0x44, 0x03, 0x63, 0x64], "test")
        .expect("status snapshot");
    assert!(snapshot.connected);
    assert!(snapshot.has_connection_status);
    assert_eq!(snapshot.battery_percent, Some(68));

    apply_presence(&state, &snapshot);
    assert_eq!(
        backend.default_calls.lock().first().map(|c| c.0.clone()),
        Some("arctis-out".to_string())
    );
}

#[test]
fn chatmix_wheel_packet_scales_app_volume() {
    // One "game"-routed app sitting at 80% volume.
    let backend = Arc::new(MockAudioBackend::new(
        Vec::new(),
        vec![session("game-app", ChatMixRoute::Game, 0.8)],
    ));
    let state = make_state(backend.clone(), switch_config());

    // Connected (`B9 03`) merged with a wheel turn `45 32 64` (game=50, chat=100), exactly
    // as the monitor stitches a connection event together with the chatmix report.
    let connected = parse_event_snapshot(&[0xB9, 0x03], "test").expect("connected");
    let wheel = parse_event_snapshot(&[0x45, 0x32, 0x64], "test").expect("wheel");
    let merged = merge_partial_snapshot(connected, wheel);
    assert!(merged.connected && merged.has_connection_status);
    assert_eq!(merged.game_volume, Some(50));
    assert_eq!(merged.chat_volume, Some(100));

    try_sync_chatmix(&state, &merged, "test").expect("sync_chatmix");

    let calls = backend.volume_calls.lock().clone();
    assert_eq!(calls.len(), 1, "exactly one volume change applied");
    assert_eq!(calls[0].0, "game-app");
    // game factor = 50/100 = 0.5; baseline 0.8 → target 0.4.
    assert!(
        (calls[0].1 - 0.4).abs() < 0.001,
        "expected ~0.4, got {}",
        calls[0].1
    );
}

#[test]
fn chatmix_restores_baseline_when_headset_disconnects() {
    let backend = Arc::new(MockAudioBackend::new(
        Vec::new(),
        vec![session("game-app", ChatMixRoute::Game, 0.4)],
    ));
    let state = make_state(backend.clone(), switch_config());

    // Scale down while connected: baseline 0.4 captured, game=50 → applied 0.2.
    let connected = parse_event_snapshot(&[0xB9, 0x03], "test").unwrap();
    let wheel = parse_event_snapshot(&[0x45, 0x32, 0x64], "test").unwrap();
    let merged = merge_partial_snapshot(connected, wheel);
    try_sync_chatmix(&state, &merged, "test").unwrap();

    // Then a power-off (`B9 02`) must restore the captured baseline (0.4) for the session.
    let off = parse_event_snapshot(&[0xB9, 0x02], "test").unwrap();
    try_sync_chatmix(&state, &off, "test").unwrap();

    let calls = backend.volume_calls.lock().clone();
    let last = calls.last().expect("a restore volume change");
    assert_eq!(last.0, "game-app");
    assert!(
        (last.1 - 0.4).abs() < 0.001,
        "baseline restored to 0.4, got {}",
        last.1
    );
}

#[test]
fn mute_packet_decodes_through_event_pipeline() {
    // `52 00 01` — mic mute toggle on MI_05. Carries no connection status; updates mic only.
    let snapshot = parse_event_snapshot(&[0x52, 0x00, 0x01], "test").expect("mute snapshot");
    assert_eq!(snapshot.mic_muted, Some(true));
    assert!(!snapshot.has_connection_status);

    let unmute = parse_event_snapshot(&[0x52, 0x00, 0x00], "test").expect("unmute snapshot");
    assert_eq!(unmute.mic_muted, Some(false));
}

/// Manual smoke test against a live PipeWire daemon: constructs the real backend and lists
/// endpoints. Ignored by default (needs a running PipeWire session); run with
/// `cargo test --lib -- --ignored pipewire_smoke`.
#[cfg(target_os = "linux")]
#[test]
#[ignore]
fn pipewire_smoke() {
    use crate::audio::NativeAudioBackend;
    let backend = NativeAudioBackend::new().expect("connect to PipeWire");
    let endpoints = backend.endpoints().expect("list endpoints");
    println!("PipeWire endpoints: {endpoints:#?}");
    assert!(
        endpoints.iter().any(|e| e.flow == EndpointFlow::Render),
        "expected at least one output endpoint"
    );

    // Give the client `info` events (binary/pid) a moment to land, then dump the resolved
    // sessions so we can eyeball that names/PIDs come through instead of "Stream N"/"Process 0".
    std::thread::sleep(std::time::Duration::from_millis(500));
    let sessions = backend
        .render_sessions(&crate::models::ChatMixConfig::default())
        .expect("list sessions");
    println!("PipeWire sessions ({}):", sessions.len());
    for session in &sessions {
        println!(
            "  - {:?} | exe={:?} | pid={} | app_id={:?} | vol={:.2} muted={}",
            session.display_name,
            session.executable_path,
            session.process_id,
            session.app_id,
            session.volume,
            session.muted,
        );
    }
}
