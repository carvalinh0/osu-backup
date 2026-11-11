use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::{BaseDirs, ProjectDirs};
use serde::{Deserialize, Serialize};

/// Name of the folder created in Google Drive.
pub const DRIVE_FOLDER_NAME: &str = "osu-lazer-backup";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// osu!lazer data folder to back up.
    pub osu_dir: Option<PathBuf>,
    /// ID of the folder created in Google Drive.
    pub drive_folder_id: Option<String>,
    /// Path to the client_secret.json downloaded from Google Cloud.
    pub credentials_path: Option<PathBuf>,
    /// The user has finished the first-run setup.
    pub setup_done: bool,
}

impl Config {
    pub fn config_dir() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("", "", "osubackup")
            .context("could not determine the config folder")?;
        Ok(dirs.config_dir().to_path_buf())
    }

    pub fn path() -> Result<PathBuf> {
        Ok(Self::config_dir()?.join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, toml::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// osu!lazer folder to use: the configured one or the system default.
    pub fn osu_dir_or_default(&self) -> Option<PathBuf> {
        self.osu_dir.clone().or_else(default_osu_dir)
    }
}

/// Default osu!lazer data folder.
/// Windows: %APPDATA%\osu — Linux: ~/.local/share/osu
pub fn default_osu_dir() -> Option<PathBuf> {
    BaseDirs::new().map(|b| b.data_dir().join("osu"))
}

/// Returns true if the minimum configuration exists (setup finished + Drive folder).
pub fn is_setup_complete(cfg: &Config) -> bool {
    cfg.setup_done && cfg.drive_folder_id.is_some() && cfg.osu_dir_or_default().is_some()
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))
}
