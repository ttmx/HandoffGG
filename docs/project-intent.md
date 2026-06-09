# Autoswapper Project Intent

Autoswapper is intended to be a small, reliable replacement for SteelSeries GG's headset/speaker autoswitching behavior.

## Problem

SteelSeries GG currently manages audio device switching between SteelSeries wireless headphones and speakers, but it is unreliable for this workflow:

- it sometimes fails to switch when the headset turns on
- it resets preferred devices
- it may switch to the wrong endpoints
- it depends on SteelSeries/Sonar behavior that is not wanted for this app

Windows also keeps the USB dongle audio endpoints available even when the headset itself is turned off, so normal Core Audio endpoint presence is not enough to determine whether audio will actually be heard through the headset.

## Goal

Build a tiny Windows-first tray app that switches audio defaults based on the real wireless headset state, not just the USB dongle or Windows audio endpoint state.

The app should:

- detect whether the headset is actually connected or disconnected
- switch output devices between headset and fallback speakers
- switch microphone devices independently between headset and fallback mic
- avoid SteelSeries Sonar devices for V1
- run quietly from the system tray
- expose a small settings/diagnostics window
- be structured so Linux support can be added later

## Product Shape

The app is intentionally small and utility-focused.

Expected tray actions:

- enable or disable autoswitching
- manually switch to headset
- manually switch to fallback devices
- open settings
- quit

Expected settings:

- choose headset output
- choose fallback output
- choose headset microphone
- choose fallback microphone
- enable or disable output and microphone rules independently
- show HID diagnostics
- show current headset connection state
- show current mute state
- show current ChatMix values

## Technical Direction

The chosen stack is:

- Tauri 2 for the desktop shell and tray integration
- Rust for native Windows audio and HID work
- Svelte/TypeScript for the settings UI

Backend boundaries should stay clean:

- `AudioBackend` handles endpoint enumeration, default endpoint state, and switching.
- `HeadsetPresenceBackend` handles headset presence and HID-derived state.
- Windows-specific audio code should stay isolated from the higher-level rule engine.

This keeps the future Linux path realistic, where the equivalent backend would likely use PipeWire/WirePlumber plus HIDAPI.

## Detection Intent

The app should use the SteelSeries vendor HID interface as the source of truth.

Current intended behavior:

- fetch initial headset status once at startup
- listen to HID input reports after startup
- avoid continuous polling where possible
- switch immediately on real connection transitions
- track mute and ChatMix for diagnostics/UI
- do not use ChatMix to make routing decisions in V1

## Switching Intent

Autoswitching should be deterministic and conservative:

- headset connected means switch configured output/mic rules to headset endpoints
- headset disconnected means switch configured output/mic rules to fallback endpoints
- output and microphone rules are independent
- switching should apply across Windows console, multimedia, and communications roles
- missing or unconfigured endpoints should not cause unrelated switching

## Near-Term Priorities

Useful next work:

- add a HID report history panel to make parser debugging easier
- validate mute state startup behavior on live hardware
- improve dongle unplug/replug recovery
- test sleep/wake behavior
- test with SteelSeries GG installed and uninstalled
- make diagnostics clearer when HID interfaces are missing or unreadable

## Non-Goals For V1

V1 should not attempt to:

- replace SteelSeries Sonar
- implement an EQ
- support every SteelSeries model
- support Linux immediately
- provide a large always-open desktop interface

The priority is a dependable tray utility that solves the headset/speaker switching problem first.
