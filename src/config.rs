use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::{BaseDirs, ProjectDirs};
use serde::{Deserialize, Serialize};

/// Nome da pasta criada no Google Drive.
pub const DRIVE_FOLDER_NAME: &str = "osu-lazer-backup";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Pasta de dados do osu!lazer que será copiada.
    pub osu_dir: Option<PathBuf>,
    /// ID da pasta criada no Google Drive.
    pub drive_folder_id: Option<String>,
    /// Caminho do client_secret.json baixado do Google Cloud.
    pub credentials_path: Option<PathBuf>,
    /// Configuração inicial já foi concluída pelo usuário.
    pub setup_done: bool,
}

impl Config {
    pub fn config_dir() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("", "", "osubackup")
            .context("não foi possível determinar a pasta de configuração")?;
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
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("lendo {}", path.display()))?;
        Ok(toml::from_str(&text)?)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Pasta do osu!lazer a ser usada: a configurada ou a padrão do sistema.
    pub fn osu_dir_or_default(&self) -> Option<PathBuf> {
        self.osu_dir.clone().or_else(default_osu_dir)
    }
}

/// Pasta padrão de dados do osu!lazer.
/// Windows: %APPDATA%\osu — Linux: ~/.local/share/osu
pub fn default_osu_dir() -> Option<PathBuf> {
    BaseDirs::new().map(|b| b.data_dir().join("osu"))
}

/// Retorna true se a configuração mínima existe (credenciais + pasta no Drive).
pub fn is_setup_complete(cfg: &Config) -> bool {
    cfg.setup_done && cfg.drive_folder_id.is_some() && cfg.osu_dir_or_default().is_some()
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("criando {}", path.display()))
}
