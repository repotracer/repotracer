use grephound_core::GrephoundConfig;
use std::path::{Path, PathBuf};

pub fn default_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("GREPHOUND_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".grephound")
        .join("config.toml")
}

pub fn load_or_default(path: &Path) -> GrephoundConfig {
    if path.exists() {
        match GrephoundConfig::load_from(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("warning: failed to load {}: {e:#}", path.display());
                GrephoundConfig::default()
            }
        }
    } else {
        GrephoundConfig::default()
    }
}
