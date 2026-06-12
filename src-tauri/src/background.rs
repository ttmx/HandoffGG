//! GNOME (and other XDG-portal desktops) integration for running as a background app.
//!
//! Modern GNOME has no system tray; instead, apps that keep running without a window
//! register through the `org.freedesktop.portal.Background` portal and then appear under
//! **Quick Settings → Background Apps**, where the user can open or quit them. This module
//! issues that registration. It is best-effort: any failure (no portal, user declines) is
//! logged and ignored — HandoffGG keeps working, just without the Quick Settings entry.

/// Ask the desktop portal to treat HandoffGG as a running background app. Runs on its own
/// thread because the portal call can block on a user permission dialog.
pub fn request_background() {
    std::thread::spawn(|| {
        if let Err(error) = try_request_background() {
            eprintln!("background portal request failed (non-fatal): {error}");
        }
    });
}

fn try_request_background() -> zbus::Result<()> {
    use std::collections::HashMap;
    use zbus::blocking::Connection;
    use zbus::zvariant::Value;

    let connection = Connection::session()?;

    let mut options: HashMap<&str, Value> = HashMap::new();
    options.insert(
        "reason",
        Value::from("HandoffGG keeps your audio devices and ChatMix in sync in the background."),
    );
    // Autostart is handled separately by tauri-plugin-autostart; here we only declare that
    // we are a background app so GNOME lists us in Quick Settings.
    options.insert("autostart", Value::from(false));
    options.insert("commandline", Value::from(vec!["handoffgg".to_string()]));

    // RequestBackground(parent_window: s, options: a{sv}) -> handle: o
    connection.call_method(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        Some("org.freedesktop.portal.Background"),
        "RequestBackground",
        &(String::new(), options),
    )?;

    Ok(())
}
