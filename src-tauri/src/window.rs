//! System tray and the settings window.

use crate::app_state::SharedState;
use std::sync::atomic::Ordering;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::window::Color;
use tauri::{AppHandle, Manager, Theme, WebviewUrl, WebviewWindowBuilder};

/// Native pre-paint backgrounds matching the stylesheet's `--bg` for each scheme, so
/// resizing never flashes a mismatched color behind the webview.
const DARK_BACKGROUND: Color = Color(0x1b, 0x1d, 0x20, 0xff);
const LIGHT_BACKGROUND: Color = Color(0xf6, 0xf7, 0xf9, 0xff);

pub(crate) fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &quit])?;

    // Left-click opens settings directly; the menu (Open settings / Quit) stays on
    // right-click via `show_menu_on_left_click(false)`.
    let mut tray = TrayIconBuilder::new()
        .tooltip("HandoffGG")
        .menu(&menu)
        .show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }

    tray.on_menu_event(|app, event| match event.id.as_ref() {
        "open" => {
            let _ = show_main_window(app);
        }
        "quit" => app.exit(0),
        _ => {}
    })
    .on_tray_icon_event(|tray, event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            let _ = show_main_window(tray.app_handle());
        }
    })
    .build(app)?;

    Ok(())
}

pub(crate) fn show_main_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        window.show()?;
        window.set_focus()?;
    } else {
        if let Some(state) = app.try_state::<SharedState>() {
            state
                .settings_window_pending_show
                .store(true, Ordering::SeqCst);
        }
        // No explicit theme: the webview follows the system scheme and the stylesheet
        // carries both palettes. The window stays invisible until the frontend calls
        // `settings_ready`, so correcting the pre-paint background after creation
        // (the builder cannot know the system theme yet) never causes a flash.
        let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::default())
            .title("HandoffGG")
            .inner_size(983.0, 667.0)
            .min_inner_size(760.0, 560.0)
            .decorations(false)
            .background_color(DARK_BACKGROUND)
            .visible(false)
            .build()?;
        if matches!(window.theme(), Ok(Theme::Light)) {
            let _ = window.set_background_color(Some(LIGHT_BACKGROUND));
        }
    }
    Ok(())
}
