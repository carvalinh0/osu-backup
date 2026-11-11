//! Executa um backup: compara os arquivos locais com o Drive e envia só o que mudou.

use anyhow::{Context, Result};

use crate::auth;
use crate::backup::{collect_essential_files, encode_drive_name, md5_hex};
use crate::config::Config;
use crate::drive::Drive;

#[derive(Debug, Clone, Copy, Default)]
pub struct Report {
    pub uploaded: usize,
}

pub fn run(cfg: &Config) -> Result<Report> {
    let osu_dir = cfg
        .osu_dir_or_default()
        .context("pasta do osu!lazer não configurada")?;
    let folder_id = cfg
        .drive_folder_id
        .as_deref()
        .context("pasta de backup no Drive não configurada")?;
    let creds_path = cfg
        .credentials_path
        .as_deref()
        .context("credenciais do Google não configuradas")?;

    let app = auth::ClientSecretFile::load(creds_path)?;
    let token = auth::access_token(&app)?;
    let drive = Drive::new(token)?;

    let index = drive.folder_index(folder_id)?;
    let files = collect_essential_files(&osu_dir)?;

    let mut report = Report::default();
    for file in &files {
        // Lê uma única vez: o hash e o conteúdo enviado são do mesmo instante.
        let bytes =
            std::fs::read(&file.path).with_context(|| format!("lendo {}", file.path.display()))?;
        let md5 = md5_hex(&bytes);

        let name = encode_drive_name(&file.rel);
        let existing = index.get(&name);
        if existing.and_then(|(_, remote_md5)| remote_md5.as_deref()) == Some(md5.as_str()) {
            continue;
        }

        let existing_id = existing.map(|(id, _)| id.as_str());
        drive.upload(folder_id, &name, existing_id, &bytes)?;
        report.uploaded += 1;
        tracing::info!("enviado: {}", file.rel);
    }

    Ok(report)
}
