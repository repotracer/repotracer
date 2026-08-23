use repotracer_core::RepotracerConfig;
use std::path::{Path, PathBuf};

pub fn default_config_path() -> PathBuf {
    if let Ok(p) = std::env::var("REPOTRACER_CONFIG") {
        return PathBuf::from(p);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".repotracer")
        .join("config.toml")
}

pub fn load_or_default(path: &Path) -> RepotracerConfig {
    if path.exists() {
        match RepotracerConfig::load_from(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("warning: failed to load {}: {e:#}", path.display());
                RepotracerConfig::default()
            }
        }
    } else {
        RepotracerConfig::default()
    }
}
