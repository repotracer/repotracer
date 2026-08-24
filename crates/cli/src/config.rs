use repotracer_core::RepoTracerConfig;
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

pub fn load_or_default(path: &Path) -> RepoTracerConfig {
    if path.exists() {
        match RepoTracerConfig::load_from(path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("warning: failed to load {}: {e:#}", path.display());
                RepoTracerConfig::default()
            }
        }
    } else {
        RepoTracerConfig::default()
    }
}
