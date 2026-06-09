# SteelSeries Arctis Nova 7 - Wireshark Dissector

`steelseries_nova7.lua` is a Wireshark/tshark Lua dissector for the Arctis Nova 7
wireless dongle's HID protocol. It decodes the reports Autoswapper relies on and
explicitly marks every byte we do not yet understand.

## Install

The plugin is already installed for the current user at:

```text
%APPDATA%\Wireshark\plugins\steelseries_nova7.lua
```

To reinstall, copy `steelseries_nova7.lua` into your Wireshark Personal Lua
Plugins folder, then reload Lua plugins. The canonical copy lives here in the repo.

## Use

1. Capture with USBPcap and open the capture.
2. Apply the display filter `nova7` to see decoded reports. Useful columns:
   `nova7.connection`, `nova7.connection_event`, `nova7.battery_percent`,
   `nova7.chatmix.game`, `nova7.chatmix.chat`, `nova7.mic_muted`.
3. Analyze -> Expert Information lists every uncertain or undecoded item.

The dissector auto-attaches to VID `0x1038` plus the known product IDs, runs a
heuristic on USB interrupt/control transfers, and also post-dissects `usbhid.data`
when Wireshark's generic HID dissector claims the interrupt payload first.

## Protocol

Interfaces by endpoint in a USBPcap capture:

```text
MI_03 / 0x84  status reports and startup B0 poll responses
MI_05 / 0x86  control, connection, and battery events
```

### `0xB0` - Battery / Connection Status (MI_03)

```text
offset  value         meaning
  +0     B0           opcode
  +1     ??           unconfirmed charge flag
  +2     ??           unconfirmed battery-like value
  +3     00|01|03     connection: 00 off, 01 charging, 03 on battery
  +4     00..64       ChatMix game level
  +5     00..64       ChatMix chat level
  +6..   00..         undecoded
```

Observed as the initial poll response. Example: `b0 02 58 00 64 64 ...` is
disconnected; `b0 03 58 03 64 64 ...` is connected.

### `0x45` - ChatMix Wheel (MI_05)

```text
  +0  45        opcode
  +1  00..64    ChatMix game
  +2  00..64    ChatMix chat
```

### `0x52` - Microphone Mute (MI_05)

```text
  +0  52        opcode
  +1  ??        unconfirmed
  +2  00|01     mic muted: 00 unmuted, 01 muted
```

### `0xB9` - Connection / Power Event (MI_05)

```text
  +0  B9        opcode
  +1  02|03     connection event: 02 off/disconnected, 03 on/connected
```

This is pushed unsolicited when the headset is powered on/off. In `on-off.pcapng`,
the MI_05 sequence is `B9 03`, `B9 02`, `B9 03`, `B9 02`, matching the toggles.

### `0xB7` - Battery Level Event (MI_05)

```text
  +0  B7        opcode
  +1  00..64    battery percent
```

Observed example: `b7 4b` = 75%.

### Status Poll Request

`00 B0 00 ...` written to MI_03 requests a `0xB0` status report. Autoswapper uses
this once at startup; live connection changes use unsolicited `0xB9` events.

This mirrors the parsers in `../src-tauri/src/presence.rs`; keep the two in sync
as more bytes are decoded.
