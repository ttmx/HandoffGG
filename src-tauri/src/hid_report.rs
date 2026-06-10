//! Typed parsing for the SteelSeries Arctis Nova 7 dongle's HID reports.
//!
//! The byte layout is decoded with [`deku`]; this module then layers the device's
//! semantics (state-byte → bool, range checks) on top and exposes a clean
//! [`HidReport`].
//!
//! These are **unnumbered** HID reports: `hidapi` delivers the opcode at byte 0. A
//! live probe (`get_report_descriptor` + raw reads on MI_03 and MI_05) confirmed this
//! for every report type — there is no report-ID byte. The Windows HID stack can, on
//! some paths, prepend a single report-ID `0x00` byte, so we strip one leading `0x00`
//! before decoding. That is a deterministic report-ID normalization (the `0x00` is the
//! HID unnumbered-report sentinel; none of our opcodes are `0x00`), **not** a scan for
//! opcode bytes. Every field then sits at a fixed offset from the opcode — so a value
//! byte can never be mistaken for an opcode (e.g. a 69% battery `0x45` is decoded as
//! battery, never as the ChatMix opcode).
//!
//! Frame layouts (offsets relative to the opcode at index 0, after report-ID strip):
//!
//! ```text
//! 0xB0 status  (MI_03):  +2 battery%   +3 connection/charge   +4 game   +5 chat
//! 0xB9 power   (MI_05):  +1 connection (0x02 off, 0x03 on)
//! 0x45 chatmix (MI_05):  +1 game       +2 chat                (each 0..=100)
//! 0x52 mute    (MI_05):  +2 muted      (0x00 unmuted, 0x01 muted)
//! 0xB7 battery (MI_05):  +1 battery%
//! ```

use deku::prelude::*;

/// Maximum chatmix wheel value (`0x64`); both game and chat saturate here.
const CHATMIX_MAX: u8 = 100;

/// Byte-level decode of a report, dispatched on the leading opcode byte. The fields
/// are raw bytes; [`HidReport`] applies the semantics. `pad_bytes_before` skips the
/// device's reserved bytes so each field lands at its true offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, DekuRead)]
#[deku(id_type = "u8")]
enum RawReport {
    #[deku(id = "0xB0")]
    Status {
        #[deku(pad_bytes_before = "1")] // +1 reserved
        battery: u8,                    // +2
        charge: u8,                     // +3
        game: u8,                       // +4
        chat: u8,                       // +5
    },
    #[deku(id = "0xB9")]
    Power { state: u8 }, // +1
    #[deku(id = "0x45")]
    ChatMix { game: u8, chat: u8 }, // +1, +2
    #[deku(id = "0x52")]
    Mute {
        #[deku(pad_bytes_before = "1")] // +1 reserved
        muted: u8,                      // +2
    },
    #[deku(id = "0xB7")]
    Battery { percent: u8 }, // +1
}

/// A decoded, semantically-validated HID report. Each variant carries exactly the
/// fields its opcode defines; `Status` is the only report that bundles several (it is
/// the `0xB0` poll reply).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidReport {
    /// `0xB0` status poll reply (MI_03). `connected` is `None` when the charge byte is
    /// an unrecognised value, so battery/chatmix are still reported.
    Status {
        connected: Option<bool>,
        battery: Option<u8>,
        chatmix: Option<(u8, u8)>,
    },
    /// `0xB9` unsolicited power on/off event (MI_05).
    Connection { connected: bool },
    /// `0x45` chatmix wheel movement (MI_05).
    ChatMix { game: u8, chat: u8 },
    /// `0x52` microphone mute toggle (MI_05).
    Mute { muted: bool },
    /// `0xB7` battery level update (MI_05).
    Battery { percent: u8 },
}

impl HidReport {
    /// Parse a raw HID report. Returns `None` when the opcode is unknown, the report is
    /// truncated, or a state byte holds an unrecognised value.
    pub fn parse(report: &[u8]) -> Option<HidReport> {
        let body = strip_report_id(report);
        let (_, raw) = RawReport::from_bytes((body, 0)).ok()?;
        Some(match raw {
            RawReport::Status {
                battery,
                charge,
                game,
                chat,
            } => HidReport::Status {
                connected: connection_state(charge),
                battery: percent(battery),
                chatmix: chatmix(game, chat),
            },
            RawReport::Power { state } => HidReport::Connection {
                connected: power_state(state)?,
            },
            RawReport::ChatMix { game, chat } => {
                let (game, chat) = chatmix(game, chat)?;
                HidReport::ChatMix { game, chat }
            }
            RawReport::Mute { muted } => HidReport::Mute {
                muted: mute_state(muted)?,
            },
            RawReport::Battery { percent: raw } => HidReport::Battery {
                percent: percent(raw)?,
            },
        })
    }
}

/// Strip a single leading report-ID `0x00` byte if present (HID unnumbered-report
/// convention). None of our opcodes are `0x00`, so this is unambiguous.
fn strip_report_id(report: &[u8]) -> &[u8] {
    match report.first() {
        Some(0x00) => &report[1..],
        _ => report,
    }
}

/// `0xB0` charge/connection byte: `0x00` off, `0x01` charging, `0x03` on battery.
fn connection_state(byte: u8) -> Option<bool> {
    match byte {
        0x00 => Some(false),
        0x01 | 0x03 => Some(true),
        _ => None,
    }
}

/// `0xB9` power-event state byte: `0x02` off, `0x03` on.
fn power_state(byte: u8) -> Option<bool> {
    match byte {
        0x02 => Some(false),
        0x03 => Some(true),
        _ => None,
    }
}

/// `0x52` mute byte: `0x00` unmuted, `0x01` muted.
fn mute_state(byte: u8) -> Option<bool> {
    match byte {
        0x00 => Some(false),
        0x01 => Some(true),
        _ => None,
    }
}

/// Accept a battery percentage only when it is a sane `0..=100`.
fn percent(byte: u8) -> Option<u8> {
    (byte <= 100).then_some(byte)
}

/// Pair the game/chat bytes, accepting only in-range `0..=100` values so a malformed
/// or truncated report does not surface a bogus wheel position.
fn chatmix(game: u8, chat: u8) -> Option<(u8, u8)> {
    (game <= CHATMIX_MAX && chat <= CHATMIX_MAX).then_some((game, chat))
}

#[cfg(test)]
mod tests {
    use super::HidReport;

    /// Pad an opcode + leading bytes into a full 64-byte report, exactly as `hidapi`
    /// delivers it from the dongle (the rest is zero padding).
    fn frame(prefix: &[u8]) -> [u8; 64] {
        let mut buf = [0u8; 64];
        buf[..prefix.len()].copy_from_slice(prefix);
        buf
    }

    // --- Full 64-byte frames captured live from the dongle via the descriptor probe ---

    #[test]
    fn captured_power_events() {
        assert_eq!(
            HidReport::parse(&frame(&[0xB9, 0x02])),
            Some(HidReport::Connection { connected: false })
        );
        assert_eq!(
            HidReport::parse(&frame(&[0xB9, 0x03])),
            Some(HidReport::Connection { connected: true })
        );
    }

    #[test]
    fn captured_mute_events() {
        assert_eq!(
            HidReport::parse(&frame(&[0x52, 0x00, 0x01])),
            Some(HidReport::Mute { muted: true })
        );
        assert_eq!(
            HidReport::parse(&frame(&[0x52, 0x00, 0x00])),
            Some(HidReport::Mute { muted: false })
        );
    }

    #[test]
    fn captured_status_frame() {
        // Exact MI_03 status report: connected, 68% battery, wheel at game=99 chat=100.
        // The stray 0x01 at +9 and the trailing padding must not affect parsing.
        let report = frame(&[0xB0, 0x03, 0x44, 0x03, 0x63, 0x64, 0x00, 0x00, 0x00, 0x01]);
        assert_eq!(
            HidReport::parse(&report),
            Some(HidReport::Status {
                connected: Some(true),
                battery: Some(68),
                chatmix: Some((99, 100)),
            })
        );
    }

    #[test]
    fn captured_chatmix_wheel_sweep() {
        // Every distinct 0x45 frame seen while turning the wheel: toward game (chat
        // drops, game pinned at 100) and toward chat (game drops, chat pinned at 100).
        let captured: &[(u8, u8)] = &[
            (0x64, 0x64),
            (0x64, 0x59),
            (0x64, 0x58),
            (0x64, 0x56),
            (0x64, 0x4A),
            (0x64, 0x49),
            (0x64, 0x46),
            (0x64, 0x3A),
            (0x64, 0x39),
            (0x64, 0x2B),
            (0x64, 0x0F),
            (0x64, 0x00),
            (0x63, 0x64),
            (0x61, 0x64),
            (0x58, 0x64),
            (0x3D, 0x64),
            (0x24, 0x64),
        ];
        for &(game, chat) in captured {
            assert_eq!(
                HidReport::parse(&frame(&[0x45, game, chat])),
                Some(HidReport::ChatMix { game, chat }),
                "frame 45 {game:02X} {chat:02X}",
            );
        }
    }

    // Real frames captured from the dongle via the report-descriptor probe (64-byte
    // reports; trailing zero padding omitted for readability).

    #[test]
    fn parses_power_off_and_on_events() {
        assert_eq!(
            HidReport::parse(&[0xB9, 0x02, 0x00, 0x00]),
            Some(HidReport::Connection { connected: false })
        );
        assert_eq!(
            HidReport::parse(&[0xB9, 0x03, 0x00, 0x00]),
            Some(HidReport::Connection { connected: true })
        );
    }

    #[test]
    fn unknown_power_state_is_rejected() {
        assert_eq!(HidReport::parse(&[0xB9, 0x04, 0x00, 0x00]), None);
    }

    #[test]
    fn parses_mute_toggle() {
        assert_eq!(
            HidReport::parse(&[0x52, 0x00, 0x01]),
            Some(HidReport::Mute { muted: true })
        );
        assert_eq!(
            HidReport::parse(&[0x52, 0x00, 0x00]),
            Some(HidReport::Mute { muted: false })
        );
    }

    #[test]
    fn parses_chatmix_wheel_sweep() {
        // Centered, toward-game (chat drops), and toward-chat (game drops).
        assert_eq!(
            HidReport::parse(&[0x45, 0x64, 0x64]),
            Some(HidReport::ChatMix { game: 100, chat: 100 })
        );
        assert_eq!(
            HidReport::parse(&[0x45, 0x64, 0x00]),
            Some(HidReport::ChatMix { game: 100, chat: 0 })
        );
        assert_eq!(
            HidReport::parse(&[0x45, 0x24, 0x64]),
            Some(HidReport::ChatMix { game: 36, chat: 100 })
        );
    }

    #[test]
    fn parses_status_report() {
        // B0 03 44 03 63 64 ... — connected, 68% battery, wheel at game=99 chat=100.
        assert_eq!(
            HidReport::parse(&[0xB0, 0x03, 0x44, 0x03, 0x63, 0x64, 0x00]),
            Some(HidReport::Status {
                connected: Some(true),
                battery: Some(68),
                chatmix: Some((99, 100)),
            })
        );
    }

    #[test]
    fn parses_battery_event() {
        assert_eq!(
            HidReport::parse(&[0xB7, 0x4B, 0x00, 0x00]),
            Some(HidReport::Battery { percent: 75 })
        );
    }

    #[test]
    fn strips_leading_report_id_byte() {
        // Same frames with a Windows-style report-ID 0x00 prefix parse identically.
        assert_eq!(
            HidReport::parse(&[0x00, 0x45, 0x24, 0x64]),
            Some(HidReport::ChatMix { game: 36, chat: 100 })
        );
        assert_eq!(
            HidReport::parse(&[0x00, 0xB0, 0x03, 0x44, 0x03, 0x63, 0x64]),
            Some(HidReport::Status {
                connected: Some(true),
                battery: Some(68),
                chatmix: Some((99, 100)),
            })
        );
    }

    #[test]
    fn battery_value_colliding_with_chatmix_opcode_is_not_a_wheel_event() {
        // 69% battery encodes as 0x45 — the ChatMix opcode. Fixed offsets decode it as
        // battery at +2; it is never mistaken for a wheel event.
        assert_eq!(
            HidReport::parse(&[0xB0, 0x03, 0x45, 0x03, 0x64, 0x64]),
            Some(HidReport::Status {
                connected: Some(true),
                battery: Some(69),
                chatmix: Some((100, 100)),
            })
        );
    }

    #[test]
    fn battery_value_colliding_with_mute_opcode_is_not_a_mute_event() {
        // 82% battery encodes as 0x52 — the mute opcode.
        assert_eq!(
            HidReport::parse(&[0xB0, 0x03, 0x52, 0x03, 0x64, 0x64]),
            Some(HidReport::Status {
                connected: Some(true),
                battery: Some(82),
                chatmix: Some((100, 100)),
            })
        );
    }

    #[test]
    fn disconnected_status_reports_connection_and_battery() {
        // B0 02 44 00 ... — headset off; +3 charge byte 0x00.
        assert_eq!(
            HidReport::parse(&[0xB0, 0x02, 0x44, 0x00, 0x64, 0x64]),
            Some(HidReport::Status {
                connected: Some(false),
                battery: Some(68),
                chatmix: Some((100, 100)),
            })
        );
    }

    #[test]
    fn unknown_opcode_is_rejected() {
        assert_eq!(HidReport::parse(&[0x11, 0x22, 0x33]), None);
        assert_eq!(HidReport::parse(&[]), None);
    }
}
