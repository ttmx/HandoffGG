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

Current parser rule:

- Find byte `B0`.
- Read connection/charging status at `B0 + 3`.
- Interpret:
  - `0x00` = headset disconnected/off
  - `0x01` = connected and charging
  - `0x03` = connected and discharging

The `B0` report also includes ChatMix-like values at:

```text
B0 + 4  game
B0 + 5  chat
```

These are useful as an initial/fallback value, but live ChatMix movement is better represented by `45` reports.

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

Current parser rule:

- Find byte `45`.
- Read:
  - `45 + 1` = game value
  - `45 + 2` = chat value
- Store both values for diagnostics/UI only.
- Do not make any audio-routing decision from ChatMix in V1.

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

Current parser rule:

- Find byte `52`.
- Read mute status at `52 + 2`.
- Interpret:
  - `0x00` = unmuted
  - `0x01` = muted

This should be validated with more live toggles, but it matches the current capture better than the old `45` parser.

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

Parser tests currently cover:

- disconnected `B0` report with HID report ID
- disconnected `B0` event without HID report ID
- connected/discharging `B0` report with HID report ID
- connected/discharging `B0` event without HID report ID
- connected/charging `B0` report
- unknown connection status
- connected `B9 03` power event
- disconnected `B9 02` power event
- battery from `B7 4B`
- ChatMix from `B0`
- ChatMix from `45`
- muted from `52 00 01`
- unmuted from `52 00 00`
- confirmation that `45` ChatMix reports are not parsed as mic mute

## Caveats

- HID interface numbers and report shapes may vary across SteelSeries models and firmware versions.
- `52` mute parsing is based on one pcap and should be validated live.
- Listener threads currently open the HID devices once. Dongle unplug/replug or USB re-enumeration may require listener restart logic.
- The UI only shows the latest raw response, so a fast sequence of reports can overwrite useful evidence. A report history panel would help further debugging.
- ChatMix is displayed only; Autoswapper does not route or alter audio based on it.

## Useful References

- Tauri tray docs: https://v2.tauri.app/learn/system-tray/
- Microsoft Core Audio device events: https://learn.microsoft.com/en-us/windows/win32/coreaudio/device-events
- Microsoft device roles: https://learn.microsoft.com/en-us/windows/win32/coreaudio/device-roles
- Arctis Nova 7 HID reverse-engineering writeup: https://aarol.dev/posts/arctis-hid/
- HeadsetControl: https://github.com/Sapd/HeadsetControl
- arctis-usb-finder: https://github.com/richrace/arctis-usb-finder
