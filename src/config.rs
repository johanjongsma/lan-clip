//! Optional `~/.config/lan-clip/config.toml`. Everything has a default; peers
//! and trust come from lan-mouse, not from here.

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    /// TCP port, on every machine. lan-mouse itself uses 4242/udp.
    pub port: u16,
    /// Largest clip synced, all formats together.
    pub max_bytes: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config { port: 4243, max_bytes: 32 * 1024 * 1024 }
    }
}

impl Config {
    pub fn load() -> Result<Config> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| crate::lanmouse::home().join(".config"));
        let path = base.join("lan-clip/config.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("parsing {}", path.display())),
            Err(_) => Ok(Config::default()),
        }
    }
}
