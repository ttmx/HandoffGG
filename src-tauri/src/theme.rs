//! System accent color detection.
//!
//! Returns a `#rrggbb` string, or `None` when the platform has no reliable accent
//! source (the frontend then falls back to a built-in accent).

#[cfg(windows)]
pub fn accent_color() -> Option<String> {
    use windows::UI::ViewManagement::{UIColorType, UISettings};

    let settings = UISettings::new().ok()?;
    let color = settings.GetColorValue(UIColorType::Accent).ok()?;
    Some(format!("#{:02x}{:02x}{:02x}", color.R, color.G, color.B))
}

#[cfg(not(windows))]
pub fn accent_color() -> Option<String> {
    // Best-effort GNOME accent via gsettings. Other desktops have no portable
    // accent source, so we return None and let the frontend use its fallback.
    use std::process::Command;

    let output = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "accent-color"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8(output.stdout).ok()?;
    let name = raw.trim().trim_matches('\'');
    gnome_accent_hex(name).map(str::to_owned)
}

#[cfg(not(windows))]
fn gnome_accent_hex(name: &str) -> Option<&'static str> {
    // GNOME 47+ named accents mapped to their palette hex values.
    Some(match name {
        "blue" => "#3584e4",
        "teal" => "#2190a4",
        "green" => "#3a944a",
        "yellow" => "#c88800",
        "orange" => "#ed5b00",
        "red" => "#e62d42",
        "pink" => "#d56199",
        "purple" => "#9141ac",
        "slate" => "#6f8396",
        _ => return None,
    })
}
