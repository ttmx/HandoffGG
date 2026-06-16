//! Standalone demo: set the SteelSeries Arctis Nova 7 mic **sidetone** level.
//!
//! Decoded from a USB capture: the level is a HID SET_REPORT (Output report) sent to
//! the MI_03 interface. The 64-byte report is all zeros except:
//!   byte[0] = 0x39   (sidetone opcode)
//!   byte[1] = level  (0x00 off, 0x01 low, 0x02 medium, 0x03 high)
//!
//! hidapi's `write` takes a leading report-ID byte (0x00 here, an unnumbered report),
//! so the buffer we hand it is `[0x00, 0x39, level, 0x00 * 62]` — matching the existing
//! status-poll code in `presence.rs`.
//!
//! Run:  cargo run --example sidetone -- high
//!       cargo run --example sidetone -- off|low|medium|high

use hidapi::HidApi;

/// SteelSeries vendor id (same constant `presence.rs` filters on).
const VENDOR_ID: u16 = 0x1038;
/// The status / control interface the sidetone report is sent to (capture: wIndex=0x0003).
const SIDETONE_INTERFACE: i32 = 3;
/// Sidetone command opcode (capture: report byte 0).
const SIDETONE_OPCODE: u8 = 0x39;

fn level_from_arg(arg: &str) -> Option<u8> {
    match arg.to_ascii_lowercase().as_str() {
        "off" | "0" => Some(0x00),
        "low" | "1" => Some(0x01),
        "medium" | "med" | "2" => Some(0x02),
        "high" | "3" => Some(0x03),
        _ => None,
    }
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_default();
    let Some(level) = level_from_arg(&arg) else {
        eprintln!("usage: cargo run --example sidetone -- off|low|medium|high");
        std::process::exit(2);
    };

    let api = HidApi::new().expect("failed to init hidapi");

    // Same selector as presence.rs: SteelSeries vendor + the MI_03 interface. There can
    // be more than one matching path; try each until a write succeeds.
    let candidates: Vec<_> = api
        .device_list()
        .filter(|d| d.vendor_id() == VENDOR_ID && d.interface_number() == SIDETONE_INTERFACE)
        .collect();

    if candidates.is_empty() {
        eprintln!("No SteelSeries MI_03 interface found — is the dongle plugged in?");
        std::process::exit(1);
    }

    // [report-id 0x00][opcode][level][zero padding]. hidapi strips the leading 0x00 for
    // this unnumbered report; the 65-byte length mirrors the 64-byte wLength + report id.
    let mut report = [0u8; 65];
    report[1] = SIDETONE_OPCODE;
    report[2] = level;

    for info in candidates {
        let path = info.path().to_string_lossy().to_string();
        match info.open_device(&api) {
            Ok(device) => match device.write(&report) {
                Ok(n) => {
                    println!("Set sidetone to '{arg}' (0x{level:02X}) — wrote {n} bytes via {path}");
                    return;
                }
                Err(e) => eprintln!("write failed on {path}: {e}"),
            },
            Err(e) => eprintln!("open failed on {path}: {e}"),
        }
    }

    eprintln!("Could not write the sidetone report to any matching interface.");
    std::process::exit(1);
}
