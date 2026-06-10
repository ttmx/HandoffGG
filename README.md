# HandoffGG

![HandoffGG settings window](docs/assets/handoffgg-screenshot.png)

HandoffGG is a small Windows tray app for automatically switching audio devices when a SteelSeries wireless headset is actually connected or disconnected.

This was almost entirely vibecoded, with some manual cleanup here and there.

## What It Does

- Allows you to set a priority list of audio devices for output/input. When your SteelSeries headset disconnects, it moves down the list to the next available device, same thing happens if a device is unplugged. When the headset reconnects, it moves back up to the headset devices.
- Allows you to use the headset's chatmix wheel to adjust the volume of apps configured as "chat" and "game". Apps are assigned to chat automatically via Windows audio sessions, but you can also set them manually.
- DOESN'T KEEP RESETTING YOUR DEVICE PREFERENCES. 
- DOESN'T RANDOMLY STOP OPENING.
- 6MB memory usage when in the tray.

## Limitations
This is a first version, tested only on my machine, on Windows 11, with a SteelSeries Arctis Nova 7. Very likely not to work with most other models, if there is interest in supporting more models, PRs are welcome.

## Installation

Windows installers are published from GitHub Releases:

https://github.com/ttmx/HandoffGG/releases

The release workflow currently builds:

- NSIS setup executable
- MSI installer

## Development

This project uses:

- Tauri 2 for the desktop shell and tray integration
- Rust for Windows audio, HID, and native app behavior
- SvelteKit, Svelte 5, TypeScript, Vite, and Tailwind CSS for the UI

Install dependencies:

```powershell
npm ci
```

Run the app in development:

```powershell
npm run tauri -- dev
```

Run checks:

```powershell
npm run check
npm run lint
cd src-tauri
cargo check --locked
```

Build installers locally:

```powershell
npm run tauri -- build
```

Installer output is written under:

```text
src-tauri/target/release/bundle/
```