use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::warn;

// ── Top-level config ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub general: GeneralConfig,
    pub timeouts: TimeoutConfig,
    pub appearance: AppearanceConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            timeouts: TimeoutConfig::default(),
            appearance: AppearanceConfig::default(),
        }
    }
}

impl Config {
    /// Load from `~/.config/rnd/config.toml`, falling back to defaults.
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => match toml::from_str::<Config>(&s) {
                Ok(cfg) => {
                    tracing::info!("Loaded config from {:?}", path);
                    cfg
                }
                Err(e) => {
                    warn!("Config parse error in {:?}: {}. Using defaults.", path, e);
                    Config::default()
                }
            },
            Err(_) => {
                tracing::info!("No config at {:?}, using defaults.", path);
                Config::default()
            }
        }
    }

    /// Path to the user CSS override file.
    pub fn css_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("rnd")
            .join("style.css")
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rnd")
        .join("config.toml")
}

// ── Sub-sections ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct GeneralConfig {
    /// Maximum simultaneous on-screen notifications (oldest removed first).
    pub max_visible: usize,
    /// How many entries to keep in history.
    pub history_size: usize,
    /// Write history to disk.
    pub persist_history: bool,
    /// Corner to anchor notifications.
    pub anchor: Anchor,
    /// Gap between the screen edge and the first notification (pixels).
    pub margin: i32,
    /// Vertical gap between notification cards (pixels).
    pub gap: i32,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            max_visible: 5,
            history_size: 100,
            persist_history: false,
            anchor: Anchor::TopRight,
            margin: 12,
            gap: 6,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct TimeoutConfig {
    /// Default timeout for low-urgency notifications (ms).
    pub low: u32,
    /// Default timeout for normal-urgency notifications (ms).
    pub normal: u32,
    // Critical notifications never auto-dismiss unless expire_timeout is set.
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            low: 4_000,
            normal: 6_000,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AppearanceConfig {
    /// Minimum width of the notification popup (pixels).
    pub min_width: i32,
    /// Maximum width of the notification popup (pixels).
    pub max_width: i32,
    /// Show the application name inside each card.
    pub show_app_name: bool,
    /// Show action buttons on the card.
    pub show_actions: bool,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            min_width: 300,
            max_width: 420,
            show_app_name: true,
            show_actions: true,
        }
    }
}

// ── Anchor ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    TopRight,
    TopLeft,
    TopCenter,
    BottomRight,
    BottomLeft,
    BottomCenter,
}
