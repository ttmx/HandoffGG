use crate::hid_report::HidReport;
use crate::models::{now_ms, PresenceSnapshot};
use hidapi::{DeviceInfo, HidApi, HidDevice};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

pub trait HeadsetPresenceBackend: Send {
    fn snapshot(&mut self) -> PresenceSnapshot;
    fn wait_for_event(&mut self, timeout: Duration) -> Option<PresenceSnapshot>;

    /// Actively re-query the status interface (the `0xB0` poll). The dongle does not
    /// push the chatmix wheel position on connect — it only emits it unsolicited when
    /// the wheel moves — so we read it back out of the status report on demand.
    fn refresh_status(&mut self) -> PresenceSnapshot;
}

pub struct SteelSeriesHidPresenceBackend {
    vendor_id: u16,
    product_ids: Vec<u16>,
    status_interface_number: i32,
    mute_interface_number: i32,
    listener_rx: Option<Receiver<PresenceSnapshot>>,
    preferred_path: Option<String>,
}

impl SteelSeriesHidPresenceBackend {
    pub fn arctis_nova_7() -> Self {
        Self {
            vendor_id: 0x1038,
            product_ids: vec![
                0x2202, // Arctis Nova 7
                0x2206, // Arctis Nova 7X
                0x220A, // Arctis Nova 7P
                0x223A, // Arctis Nova 7 Diablo IV
                0x2258, // Arctis Nova 7X V2
                0x22A1, // Seen on some active dongle interfaces after pairing/firmware updates.
            ],
            status_interface_number: 3,
            mute_interface_number: 5,
            listener_rx: None,
            preferred_path: None,
        }
    }

    fn matching_devices<'a>(&self, api: &'a HidApi, interface_number: i32) -> Vec<&'a DeviceInfo> {
        api.device_list()
            .filter(|device| device.vendor_id() == self.vendor_id)
            .filter(|device| self.product_ids.contains(&device.product_id()))
            .filter(|device| device.interface_number() == interface_number)
            .collect()
    }

    fn start_event_listeners(&mut self) -> anyhow::Result<Receiver<PresenceSnapshot>> {
        let api = HidApi::new()?;

        // The dongle pushes unsolicited reports on both HID interfaces: connection /
        // battery status (0xB0) on MI_03, and mute (0x52) / chatmix-wheel (0x45) on
        // MI_05. Listening on both gives us fully event-driven presence — no polling.
        let interfaces = [self.status_interface_number, self.mute_interface_number];
        let any_present = interfaces
            .iter()
            .any(|&iface| !self.matching_devices(&api, iface).is_empty());
        if !any_present {
            anyhow::bail!("No SteelSeries HID event interfaces were found");
        }

        let (tx, rx) = mpsc::channel();
        for interface_number in interfaces {
            let spec = ListenerSpec {
                vendor_id: self.vendor_id,
                product_ids: self.product_ids.clone(),
                interface_number,
            };
            let tx = tx.clone();
            thread::spawn(move || event_reader_loop(spec, tx));
        }

        Ok(rx)
    }
}

impl HeadsetPresenceBackend for SteelSeriesHidPresenceBackend {
    fn snapshot(&mut self) -> PresenceSnapshot {
        let mut snapshot = self.status_snapshot();
        if snapshot.error.is_none() && snapshot.raw_response.is_some() {
            if let Ok(api) = HidApi::new() {
                if let Some(control_snapshot) = self.snapshot_control_interface(&api) {
                    snapshot = merge_partial_snapshot(snapshot, control_snapshot);
                }
            }
            self.preferred_path = snapshot.device_path.clone();
        }
        snapshot
    }

    fn refresh_status(&mut self) -> PresenceSnapshot {
        self.status_snapshot()
    }

    fn wait_for_event(&mut self, timeout: Duration) -> Option<PresenceSnapshot> {
        if self.listener_rx.is_none() {
            self.listener_rx = self.start_event_listeners().ok();
        }

        match self.listener_rx.as_ref()?.recv_timeout(timeout) {
            Ok(snapshot) => Some(snapshot),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                self.listener_rx = None;
                None
            }
        }
    }
}

impl SteelSeriesHidPresenceBackend {
    /// One-shot active query of the status interface (MI_03): write the `0xB0`
    /// request and read the reply. Used only to seed the initial state at startup;
    /// thereafter connection changes arrive unsolicited via the event listeners.
    fn status_snapshot(&self) -> PresenceSnapshot {
        let api = match HidApi::new() {
            Ok(api) => api,
            Err(error) => return PresenceSnapshot::error(format!("HID init failed: {error}")),
        };

        let candidates = self.matching_devices(&api, self.status_interface_number);
        if candidates.is_empty() {
            return PresenceSnapshot::error("SteelSeries HID interface MI_03 was not found");
        }

        let mut last_error = None;
        for device_info in candidates {
            let device_path = device_info.path().to_string_lossy().to_string();
            let device = match device_info.open_device(&api) {
                Ok(device) => device,
                Err(error) => {
                    last_error = Some(format!("{device_path}: {error}"));
                    continue;
                }
            };

            let snapshot = query_status(device, device_path);
            if snapshot.error.is_none() && snapshot.raw_response.is_some() {
                return snapshot;
            }
            last_error = snapshot.error;
        }

        PresenceSnapshot::error(
            last_error.unwrap_or_else(|| "No SteelSeries HID status response parsed".to_string()),
        )
    }

    fn snapshot_control_interface(&self, api: &HidApi) -> Option<PresenceSnapshot> {
        for device_info in self.matching_devices(api, self.mute_interface_number) {
            let path = device_info.path().to_string_lossy().to_string();
            let Ok(device) = device_info.open_device(api) else {
                continue;
            };

            if let Some(snapshot) = query_initial_control_state(device, path) {
                return Some(snapshot);
            }
        }

        None
    }
}

fn query_status(device: HidDevice, device_path: String) -> PresenceSnapshot {
    if let Err(error) = device.set_blocking_mode(false) {
        return PresenceSnapshot {
            connected: false,
            has_connection_status: false,
            mic_muted: None,
            battery_percent: None,
            game_volume: None,
            chat_volume: None,
            raw_response: None,
            device_path: Some(device_path),
            error: Some(format!("HID nonblocking mode failed: {error}")),
            observed_at_ms: now_ms(),
        };
    }

    let mut out_report = [0_u8; 65];
    out_report[1] = 0xB0;
    if let Err(error) = device.write(&out_report) {
        return PresenceSnapshot {
            connected: false,
            has_connection_status: false,
            mic_muted: None,
            battery_percent: None,
            game_volume: None,
            chat_volume: None,
            raw_response: None,
            device_path: Some(device_path),
            error: Some(format!("HID status write failed: {error}")),
            observed_at_ms: now_ms(),
        };
    }

    let mut in_report = [0_u8; 65];
    let read = match device.read_timeout(
        &mut in_report,
        Duration::from_millis(700).as_millis() as i32,
    ) {
        Ok(read) => read,
        Err(error) => {
            return PresenceSnapshot {
                connected: false,
                has_connection_status: false,
                mic_muted: None,
                battery_percent: None,
                game_volume: None,
                chat_volume: None,
                raw_response: None,
                device_path: Some(device_path),
                error: Some(format!("HID status read failed: {error}")),
                observed_at_ms: now_ms(),
            }
        }
    };

    if read == 0 {
        return PresenceSnapshot {
            connected: false,
            has_connection_status: false,
            mic_muted: None,
            battery_percent: None,
            game_volume: None,
            chat_volume: None,
            raw_response: None,
            device_path: Some(device_path),
            error: Some("HID status read timed out".to_string()),
            observed_at_ms: now_ms(),
        };
    }

    let raw_response = hex_bytes(&in_report[..read]);
    match HidReport::parse(&in_report[..read]) {
        Some(parsed @ HidReport::Status { .. }) => {
            snapshot_from_report(parsed, raw_response, &device_path)
        }
        _ => PresenceSnapshot {
            connected: false,
            has_connection_status: false,
            mic_muted: None,
            battery_percent: None,
            game_volume: None,
            chat_volume: None,
            raw_response: Some(raw_response),
            device_path: Some(device_path),
            error: Some("HID status response did not match Nova 7 parser".to_string()),
            observed_at_ms: now_ms(),
        },
    }
}

fn query_initial_control_state(device: HidDevice, device_path: String) -> Option<PresenceSnapshot> {
    let started_at = Instant::now();
    let timeout = Duration::from_millis(500);
    let mut snapshot = PresenceSnapshot {
        connected: false,
        has_connection_status: false,
        mic_muted: None,
        battery_percent: None,
        game_volume: None,
        chat_volume: None,
        raw_response: None,
        device_path: Some(device_path.clone()),
        error: None,
        observed_at_ms: now_ms(),
    };

    while started_at.elapsed() < timeout {
        let mut in_report = [0_u8; 65];
        let read = match device.read_timeout(&mut in_report, 75) {
            Ok(0) => continue,
            Ok(read) => read,
            Err(_) => break,
        };

        let Some(next) = parse_event_snapshot(&in_report[..read], &device_path) else {
            continue;
        };

        snapshot = merge_partial_snapshot(snapshot, next);
        if snapshot.mic_muted.is_some() {
            break;
        }
    }

    if snapshot.mic_muted.is_some()
        || snapshot.battery_percent.is_some()
        || snapshot.game_volume.is_some()
        || snapshot.chat_volume.is_some()
    {
        Some(snapshot)
    } else {
        None
    }
}

/// Identifies a single HID interface to listen on, so the reader thread can reopen
/// the device on its own after a transient failure (headset sleep, dongle re-enumerate).
struct ListenerSpec {
    vendor_id: u16,
    product_ids: Vec<u16>,
    interface_number: i32,
}

/// Self-healing reader: open the interface, stream unsolicited reports until the
/// device errors, then back off and reopen. Exits only when the receiver is gone
/// (app shutdown). A read error no longer kills presence detection — it just
/// triggers a reconnect — and transient errors are not surfaced as diagnostics.
fn event_reader_loop(spec: ListenerSpec, tx: mpsc::Sender<PresenceSnapshot>) {
    loop {
        if let Some((device, device_path)) = open_listener_device(&spec) {
            loop {
                let mut in_report = [0_u8; 65];
                match device.read_timeout(&mut in_report, 30_000) {
                    Ok(0) => continue,
                    Ok(read) => {
                        if let Some(snapshot) =
                            parse_event_snapshot(&in_report[..read], &device_path)
                        {
                            if tx.send(snapshot).is_err() {
                                return;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        }

        // Device unavailable or errored; wait before retrying so a missing dongle
        // does not spin the CPU.
        thread::sleep(Duration::from_millis(1_000));
    }
}

fn open_listener_device(spec: &ListenerSpec) -> Option<(HidDevice, String)> {
    let api = HidApi::new().ok()?;
    let device_info = api.device_list().find(|device| {
        device.vendor_id() == spec.vendor_id
            && spec.product_ids.contains(&device.product_id())
            && device.interface_number() == spec.interface_number
    })?;
    let device_path = device_info.path().to_string_lossy().to_string();
    let device = device_info.open_device(&api).ok()?;
    Some((device, device_path))
}

fn parse_event_snapshot(report: &[u8], device_path: &str) -> Option<PresenceSnapshot> {
    let parsed = HidReport::parse(report)?;
    Some(snapshot_from_report(parsed, hex_bytes(report), device_path))
}

/// Map a decoded [`HidReport`] onto a partial [`PresenceSnapshot`]. Each report fills
/// only the fields it carries; `merge_partial_snapshot` / `merge_presence_snapshot`
/// stitch successive partials into the full picture.
fn snapshot_from_report(
    report: HidReport,
    raw_response: String,
    device_path: &str,
) -> PresenceSnapshot {
    let base = PresenceSnapshot {
        connected: false,
        has_connection_status: false,
        mic_muted: None,
        battery_percent: None,
        game_volume: None,
        chat_volume: None,
        raw_response: Some(raw_response),
        device_path: Some(device_path.to_string()),
        error: None,
        observed_at_ms: now_ms(),
    };

    match report {
        HidReport::Status {
            connected,
            battery,
            chatmix,
        } => PresenceSnapshot {
            connected: connected.unwrap_or(false),
            has_connection_status: connected.is_some(),
            battery_percent: battery,
            game_volume: chatmix.map(|(game, _)| game),
            chat_volume: chatmix.map(|(_, chat)| chat),
            ..base
        },
        HidReport::Connection { connected } => PresenceSnapshot {
            connected,
            has_connection_status: true,
            ..base
        },
        HidReport::ChatMix { game, chat } => PresenceSnapshot {
            game_volume: Some(game),
            chat_volume: Some(chat),
            ..base
        },
        HidReport::Mute { muted } => PresenceSnapshot {
            mic_muted: Some(muted),
            ..base
        },
        HidReport::Battery { percent } => PresenceSnapshot {
            battery_percent: Some(percent),
            ..base
        },
    }
}

fn merge_partial_snapshot(
    previous: PresenceSnapshot,
    snapshot: PresenceSnapshot,
) -> PresenceSnapshot {
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
        raw_response: snapshot.raw_response.or(previous.raw_response),
        device_path: snapshot.device_path.or(previous.device_path),
        error: snapshot.error.or(previous.error),
        observed_at_ms: snapshot.observed_at_ms,
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}