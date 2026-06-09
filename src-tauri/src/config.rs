use crate::models::AppConfig;
use anyhow::Context;
use std::fs;
use std::path::PathBuf;

pub fn load(path: &PathBuf) -> anyhow::Result<AppConfig> {
    if !path.exists() {
        return Ok(AppConfig::default());
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read config from {}", path.display()))?;
    let config = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse config from {}", path.display()))?;
    Ok(config)
}

pub fn save(path: &PathBuf, config: &AppConfig) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let content = serde_json::to_string_pretty(config)?;
    fs::write(path, content)
        .with_context(|| format!("failed to write config to {}", path.display()))
}

#[cfg(test)]
mod tests {
    use crate::models::AppConfig;

    #[test]
    fn deserializes_old_config_with_empty_chatmix() {
        let json = r#"{
            "autoswitchEnabled": true,
            "output": { "priorities": [] },
            "input": { "priorities": [] }
        }"#;

        let config: AppConfig = serde_json::from_str(json).expect("old config should load");
        assert!(config.chatmix.app_routes.is_empty());
    }
}
