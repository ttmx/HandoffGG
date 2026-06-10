# SteelSeries Arctis Nova 7 HID Findings

This document records the current reverse-engineering notes for Autoswapper V1.
The target hardware is a SteelSeries wireless headset dongle that remains USB-connected while the headset itself turns on and off.

## Summary

Windows Core Audio is not a reliable presence signal for this headset. With the dongle still plugged in, Windows can continue to expose the headset output and microphone endpoints as active even when the headset is off.

The reliable signal comes from SteelSeries vendor HID reports.

For V1:

- Use an initial HID status query to discover current headset state.
- Then listen to HID input reports for live updates.
- Do not rely on a debounce timer.
- Do not poll continuously after startup unless listener recovery requires it later.
- Treat SteelSeries Sonar devices as out of scope.

## Device Matching

SteelSeries vendor ID:

```text
0x1038
```

Known or relevant product IDs:

```text
0x2202  Arctis Nova 7
0x2206  Arctis Nova 7X
0x220A  Arctis Nova 7P
0x223A  Arctis Nova 7 Diablo IV
0x2258  Arctis Nova 7X V2
0x22A1  Seen locally on an active dongle interface after pairing/firmware variation
```

Relevant HID interfaces observed so far:

```text
MI_03 / interface 3  status reports, B0 poll responses
MI_05 / interface 5  headset controls, ChatMix, mute, connection, battery reports
```

Relevant interrupt endpoints from the pcap:

```text
0x84  B0/status-side input reports
0x86  control-side input reports, including 45, 52, B9, and B7 reports
```

## Frame Layout (authoritative)

Earlier notes reverse-engineered the reports from pcaps and parsed them by *scanning*
for a known opcode byte. That is fragile: opcode bytes collide with value bytes (a 69%
battery is `0x45`, the ChatMix opcode; 82% is `0x52`, the mute opcode), which produced
a phantom `game=3, chat=100` wheel event. The layout below was instead confirmed
directly from the device's **HID report descriptor** (`hid_get_report_descriptor`) plus
raw reads captured live on both interfaces.

**Wireshark vs hidapi.** A Wireshark USB capture shows the transport layer — the URB
header (~27 bytes), and for the `0xB0` poll the control-transfer setup stage (which is
where the report-ID lives, inside `wValue`). `hidapi` strips all of that; our code only
ever sees the HID **report payload**. So reason about offsets from the hidapi buffer,
not the Wireshark frame.

**These reports are unnumbered.** hidapi delivers the opcode at byte 0 on both MI_03
and MI_05 — there is no report-ID byte (the Windows stack may prepend a single `0x00`,
which we strip; none of our opcodes are `0x00`). All reports are 64 bytes, zero-padded.
Fixed offsets from the opcode at index 0:

```text
0xB0 status  (MI_03):  +2 battery%   +3 connection/charge (00 off, 01 charging, 03 on battery)   +4 game   +5 chat
0xB9 power   (MI_05):  +1 connection (02 off, 03 on)
0x45 chatmix (MI_05):  +1 game       +2 chat              (each 0..=100, 0x64 = max)
0x52 mute    (MI_05):  +2 muted      (00 unmuted, 01 muted)
0xB7 battery (MI_05):  +1 battery%
```

Parsing lives in `src-tauri/src/hid_report.rs`: the byte layout is decoded with the
`deku` derive at these fixed offsets, then device semantics (state byte → bool, range
checks) are applied. Because fields are read at fixed positions, a value byte can never
be mistaken for an opcode — the collision class above is structurally impossible.

## Initial Status Query

Send the output report:

```text
00 B0
```

Observed status responses:

```text
00 B0 03 59 03 64 64 ...
00 B0 02 59 00 64 64 ...
```

Input events can omit the leading HID report ID:

```text
B0 03 58 03 64 64 ...
B0 02 58 00 64 64 ...
```

Parser rule: see [Frame Layout (authoritative)](#frame-layout-authoritative). The
`0xB0` reply carries connection/charge at `+3`, battery at `+2`, and the ChatMix wheel
position at `+4` (game) / `+5` (chat). The wheel position is read from this reply to
seed the state from the initial query and from the active re-query issued when the
headset connects — the dongle only pushes the wheel position unsolicited (`45` reports)
when it actually moves, so a fresh connect would otherwise have no value.

## PCAP Sections

File:

```text
mute-unmute-chatmix-on-off.pcapng
```

Packet comments found in the capture:

```text
2178  End of mute/unmute
4892  End of chat wheel mix
7806  End of turning off/on
```

Interpreted sections:

```text
1..2178      init plus mute/unmute activity
2179..4892   ChatMix wheel activity
4893..7806   headset off/on activity
7807..end    trailing idle capture
```

## ChatMix Reports

Live ChatMix wheel movement appears on the control interface as `45` reports:

```text
45 <game> <chat> 00 00 ...
```

Examples from the pcap:

```text
45 63 64 ...
45 64 64 ...
45 62 64 ...
45 00 64 ...
45 64 00 ...
45 64 01 ...
45 64 1A ...
45 64 36 ...
45 64 55 ...
45 64 56 ...
```

Parser rule: opcode `45` at offset 0, then `+1` = game, `+2` = chat (both `0..=100`).
See [Frame Layout (authoritative)](#frame-layout-authoritative). ChatMix is displayed
and drives the per-app volume scaling; it is not an audio-routing signal.

> **Opcode collision (historical).** Opcodes share the byte space with value bytes: a
> 69% battery is `0x45` (the ChatMix opcode), 82% is `0x52` (the mute opcode). The old
> parser scanned the whole report for an opcode, so it matched the *battery byte* of a
> `B0` status report and misparsed its neighbours — a phantom `game=3, chat=100` wheel
> event at startup when the battery sat at 69%. The current fixed-offset parser
> (`hid_report.rs`, via `deku`) reads each field at a known position, so this is
> structurally impossible. The Wireshark dissector's `find_opcode` likewise restricts
> to offsets 0/1.

Earlier parser mistake:

- We initially treated `45 + 2` as mic mute.
- That was wrong; it was parsing ChatMix movement as mute state.

## Mute Reports

Mute/unmute appears as separate `52` reports:

```text
52 00 00 ...
52 00 01 ...
```

Examples from the pcap:

```text
52 00 00 ...
52 00 01 ...
52 00 00 ...
52 00 01 ...
```

Parser rule: opcode `52` at offset 0, mute status at `+2` (`0x00` unmuted, `0x01`
muted). See [Frame Layout (authoritative)](#frame-layout-authoritative).

## Connection Reports

The startup/current-state query returns `B0` reports on the status interface.

Examples:

```text
B0 02 58 00 64 64 ...  disconnected
B0 03 58 03 64 64 ...  connected/discharging
```

Live headset off/on changes emit unsolicited `B9` reports on the control interface
with no preceding `SET_REPORT`.

Examples from `on-off.pcapng`:

```text
B9 03 ...  connected/on
B9 02 ...  disconnected/off
```

Observed sequence:

```text
6155  endpoint 0x86  B9 03
6159  endpoint 0x86  B9 02
6161  endpoint 0x86  B9 03
6165  endpoint 0x86  B9 02
```

Current behavior:

- Connection changes trigger autoswitch immediately.
- Non-connection HID events such as ChatMix, mute, and battery update diagnostics/UI only.
- The last known connection state is retained when a partial report only contains ChatMix, mute, or battery.

## Battery Reports

Battery level updates appear on the control interface as `B7` reports:

```text
B7 4B ...  75%
```

## Current Autoswapper Behavior

Backend:

- `SteelSeriesHidPresenceBackend::snapshot()` performs the initial `00 B0` query.
- `wait_for_event()` starts listener threads for interfaces 3 and 5.
- The app merges partial HID snapshots:
  - `B0` startup reports and `B9` events update connection state
  - `45` reports update ChatMix values
  - `52` reports update mic mute state
  - `B7` reports update battery percent
- `get_presence` returns the cached monitor state instead of doing synchronous HID I/O.

This avoids the settings page lag caused by repeated synchronous HID reads.

Autoswitch:

- Only connection transitions apply switch decisions.
- Output and microphone rules are still independent.
- Defaults should be set across console, multimedia, and communications roles in the Windows backend.

## Tests Added

Parser tests live in `src-tauri/src/hid_report.rs` and run against frames captured
live from the dongle. They cover power on/off (`B9`), mute toggle (`52`), the ChatMix
wheel sweep (`45`), connected and disconnected status (`B0`), battery (`B7`), the
leading report-ID strip, unknown opcode/state rejection, and — as regressions — the
69% (`0x45`) and 82% (`0x52`) battery levels that the old scanning parser misread as
ChatMix/mute opcodes.

## Caveats

- HID interface numbers and report shapes may vary across SteelSeries models and firmware versions. The fixed offsets here were confirmed on this dongle via `hid_get_report_descriptor` + live reads.
- Listener threads currently open the HID devices once. Dongle unplug/replug or USB re-enumeration may require listener restart logic.
- The UI only shows the latest raw response, so a fast sequence of reports can overwrite useful evidence. A report history panel would help further debugging.

## Useful References

- Tauri tray docs: https://v2.tauri.app/learn/system-tray/
- Microsoft Core Audio device events: https://learn.microsoft.com/en-us/windows/win32/coreaudio/device-events
- Microsoft device roles: https://learn.microsoft.com/en-us/windows/win32/coreaudio/device-roles
- Arctis Nova 7 HID reverse-engineering writeup: https://aarol.dev/posts/arctis-hid/
- HeadsetControl: https://github.com/Sapd/HeadsetControl
- arctis-usb-finder: https://github.com/richrace/arctis-usb-finder
