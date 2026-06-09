use crate::models::{now_ms, PresenceSnapshot};
use hidapi::{DeviceInfo, HidApi, HidDevice};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

pub trait HeadsetPresenceBackend: Send {
    fn snapshot(&mut self) -> PresenceSnapshot;
    fn wait_for_event(&mut self, timeout: Duration) -> Option<PresenceSnapshot>;
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
    let connected = match parse_connected(&in_report[..read]) {
        Some(connected) => connected,
        None => {
            return PresenceSnapshot {
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
            }
        }
    };

    let chatmix = parse_chatmix(&in_report[..read]);

    PresenceSnapshot {
        connected,
        has_connection_status: true,
        mic_muted: None,
        battery_percent: parse_battery_percent(&in_report[..read]),
        game_volume: chatmix.map(|chatmix| chatmix.0),
        chat_volume: chatmix.map(|chatmix| chatmix.1),
        raw_response: Some(raw_response),
        device_path: Some(device_path),
        error: None,
        observed_at_ms: now_ms(),
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
    let raw_response = hex_bytes(report);

    if let Some(connected) = parse_connected(report) {
        let chatmix = parse_chatmix(report);
        return Some(PresenceSnapshot {
            connected,
            has_connection_status: true,
            mic_muted: None,
            battery_percent: parse_battery_percent(report),
            game_volume: chatmix.map(|chatmix| chatmix.0),
            chat_volume: chatmix.map(|chatmix| chatmix.1),
            raw_response: Some(raw_response),
            device_path: Some(device_path.to_string()),
            error: None,
            observed_at_ms: now_ms(),
        });
    }

    if let Some(chatmix) = parse_chatmix(report) {
        return Some(PresenceSnapshot {
            connected: false,
            has_connection_status: false,
            mic_muted: None,
            battery_percent: None,
            game_volume: Some(chatmix.0),
            chat_volume: Some(chatmix.1),
            raw_response: Some(raw_response),
            device_path: Some(device_path.to_string()),
            error: None,
            observed_at_ms: now_ms(),
        });
    }

    if let Some(mic_muted) = parse_mic_muted(report) {
        return Some(PresenceSnapshot {
            connected: false,
            has_connection_status: false,
            mic_muted: Some(mic_muted),
            battery_percent: None,
            game_volume: None,
            chat_volume: None,
            raw_response: Some(raw_response),
            device_path: Some(device_path.to_string()),
            error: None,
            observed_at_ms: now_ms(),
        });
    }

    if let Some(battery_percent) = parse_battery_percent(report) {
        return Some(PresenceSnapshot {
            connected: false,
            has_connection_status: false,
            mic_muted: None,
            battery_percent: Some(battery_percent),
            game_volume: None,
            chat_volume: None,
            raw_response: Some(raw_response),
            device_path: Some(device_path.to_string()),
            error: None,
            observed_at_ms: now_ms(),
        });
    }

    None
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

fn parse_connected(report: &[u8]) -> Option<bool> {
    if let Some(b9_index) = report.iter().position(|byte| *byte == 0xB9) {
        return match *report.get(b9_index + 1)? {
            0x02 => Some(false),
            0x03 => Some(true),
            _ => None,
        };
    }

    let b0_index = report.iter().position(|byte| *byte == 0xB0)?;
    match *report.get(b0_index + 3)? {
        0x00 => Some(false),
        0x01 | 0x03 => Some(true),
        _ => None,
    }
}

fn parse_chatmix(report: &[u8]) -> Option<(u8, u8)> {
    if let Some(report_index) = report.iter().position(|byte| *byte == 0x45) {
        return Some((
            *report.get(report_index + 1)?,
            *report.get(report_index + 2)?,
        ));
    }

    let b0_index = report.iter().position(|byte| *byte == 0xB0)?;
    Some((*report.get(b0_index + 4)?, *report.get(b0_index + 5)?))
}

fn parse_mic_muted(report: &[u8]) -> Option<bool> {
    let report_index = report.iter().position(|byte| *byte == 0x52)?;
    let mute_status = *report.get(report_index + 2)?;

    match mute_status {
        0x00 => Some(false),
        0x01 => Some(true),
        _ => None,
    }
}

fn parse_battery_percent(report: &[u8]) -> Option<u8> {
    let percent = if let Some(report_index) = report.iter().position(|byte| *byte == 0xB7) {
        *report.get(report_index + 1)?
    } else {
        let b0_index = report.iter().position(|byte| *byte == 0xB0)?;
        *report.get(b0_index + 2)?
    };
    (percent <= 100).then_some(percent)
}

#[cfg(test)]
mod tests {
    use super::{parse_battery_percent, parse_chatmix, parse_connected, parse_mic_muted};

    #[test]
    fn parses_nova_7_disconnected_report_with_hid_report_id() {
        let report = [0x00, 0xB0, 0x02, 0x59, 0x00, 0x64, 0x64];
        assert_eq!(parse_connected(&report), Some(false));
    }

    #[test]
    fn parses_nova_7_disconnected_event_without_hid_report_id() {
        let report = [0xB0, 0x02, 0x58, 0x00, 0x64, 0x64];
        assert_eq!(parse_connected(&report), Some(false));
    }

    #[test]
    fn parses_nova_7_discharging_report_with_hid_report_id() {
        let report = [0x00, 0xB0, 0x03, 0x59, 0x03, 0x64, 0x64];
        assert_eq!(parse_connected(&report), Some(true));
    }

    #[test]
    fn parses_nova_7_discharging_event_without_hid_report_id() {
        let report = [0xB0, 0x03, 0x58, 0x03, 0x64, 0x64];
        assert_eq!(parse_connected(&report), Some(true));
    }

    #[test]
    fn parses_nova_7_charging_report_with_hid_report_id() {
        let report = [0x00, 0xB0, 0x01, 0x59, 0x01, 0x64, 0x64];
        assert_eq!(parse_connected(&report), Some(true));
    }

    #[test]
    fn returns_none_for_unknown_status() {
        let report = [0x00, 0xB0, 0x01, 0x59, 0x02, 0x64, 0x64];
        assert_eq!(parse_connected(&report), None);
    }

    #[test]
    fn parses_nova_7_connected_power_event() {
        let report = [0xB9, 0x03, 0x00, 0x00];
        assert_eq!(parse_connected(&report), Some(true));
    }

    #[test]
    fn parses_nova_7_disconnected_power_event() {
        let report = [0xB9, 0x02, 0x00, 0x00];
        assert_eq!(parse_connected(&report), Some(false));
    }

    #[test]
    fn returns_none_for_unknown_power_event() {
        let report = [0xB9, 0x04, 0x00, 0x00];
        assert_eq!(parse_connected(&report), None);
    }

    #[test]
    fn parses_chatmix_from_status_event() {
        let report = [0xB0, 0x03, 0x58, 0x03, 0x63, 0x64];
        assert_eq!(parse_chatmix(&report), Some((0x63, 0x64)));
    }

    #[test]
    fn parses_chatmix_from_wheel_event() {
        let report = [0x45, 0x64, 0x00];
        assert_eq!(parse_chatmix(&report), Some((0x64, 0x00)));
    }

    #[test]
    fn parses_mic_muted_from_mute_event() {
        let report = [0x52, 0x00, 0x01];
        assert_eq!(parse_mic_muted(&report), Some(true));
    }

    #[test]
    fn parses_mic_unmuted_from_mute_event() {
        let report = [0x52, 0x00, 0x00];
        assert_eq!(parse_mic_muted(&report), Some(false));
    }

    #[test]
    fn parses_battery_percent_event() {
        let report = [0xB7, 0x4B, 0x00, 0x00];
        assert_eq!(parse_battery_percent(&report), Some(75));
    }

    #[test]
    fn parses_battery_percent_from_status_report() {
        let report = [0x00, 0xB0, 0x02, 0x4C, 0x00, 0x64, 0x64];
        assert_eq!(parse_battery_percent(&report), Some(76));
    }

    #[test]
    fn does_not_parse_chatmix_as_mic_mute() {
        let report = [0x45, 0x64, 0x1A];
        assert_eq!(parse_mic_muted(&report), None);
    }
}
