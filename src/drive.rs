//! Cliente mínimo da API do Google Drive v3 (somente o necessário para o backup).

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::config::DRIVE_FOLDER_NAME;

const FILES_URL: &str = "https://www.googleapis.com/drive/v3/files";
const UPLOAD_URL: &str = "https://www.googleapis.com/upload/drive/v3/files";
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";

pub struct Drive {
    http: reqwest::blocking::Client,
    token: String,
}

#[derive(Deserialize)]
struct FileList {
    #[serde(default)]
    files: Vec<RemoteFile>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct RemoteFile {
    id: String,
    name: String,
    #[serde(rename = "md5Checksum")]
    md5_checksum: Option<String>,
}

#[derive(Deserialize)]
struct IdOnly {
    id: String,
}

impl Drive {
    pub fn new(access_token: String) -> Result<Self> {
        Ok(Self {
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(600))
                .build()?,
            token: access_token,
        })
    }

    fn check(resp: reqwest::blocking::Response) -> Result<reqwest::blocking::Response> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body = resp.text().unwrap_or_default();
        bail!("Google Drive respondeu {status}: {body}");
    }

    /// Acha a pasta de backup na raiz do Drive ou cria se não existir. Retorna o ID.
    pub fn find_or_create_folder(&self) -> Result<String> {
        let q = format!(
            "name = '{DRIVE_FOLDER_NAME}' and mimeType = '{FOLDER_MIME}' and 'root' in parents and trashed = false"
        );
        let list = self.list(&q)?;
        if let Some(f) = list.first() {
            return Ok(f.id.clone());
        }

        let resp = self
            .http
            .post(FILES_URL)
            .bearer_auth(&self.token)
            .query(&[("fields", "id")])
            .json(&serde_json::json!({ "name": DRIVE_FOLDER_NAME, "mimeType": FOLDER_MIME }))
            .send()?;
        let created: IdOnly = Self::check(resp)?.json()?;
        Ok(created.id)
    }

    fn list(&self, q: &str) -> Result<Vec<RemoteFile>> {
        let mut all = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut params = vec![
                ("q", q.to_string()),
                (
                    "fields",
                    "nextPageToken,files(id,name,md5Checksum)".to_string(),
                ),
                ("pageSize", "1000".to_string()),
            ];
            if let Some(p) = &page {
                params.push(("pageToken", p.clone()));
            }
            let resp = self
                .http
                .get(FILES_URL)
                .bearer_auth(&self.token)
                .query(&params)
                .send()?;
            let parsed: FileList = Self::check(resp)?.json()?;
            all.extend(parsed.files);
            match parsed.next_page_token {
                Some(p) => page = Some(p),
                None => break,
            }
        }
        Ok(all)
    }

    /// Mapeia nome -> md5 de todos os arquivos dentro da pasta de backup.
    pub fn folder_index(
        &self,
        folder_id: &str,
    ) -> Result<HashMap<String, (String, Option<String>)>> {
        let q = format!("'{folder_id}' in parents and trashed = false");
        Ok(self
            .list(&q)?
            .into_iter()
            .map(|f| (f.name, (f.id, f.md5_checksum)))
            .collect())
    }

    /// Envia (cria ou substitui) um arquivo dentro da pasta de backup.
    /// `existing_id` é o ID do arquivo já existente no Drive, se houver.
    pub fn upload(
        &self,
        folder_id: &str,
        name: &str,
        existing_id: Option<&str>,
        content: &[u8],
    ) -> Result<()> {
        let boundary = "osubackup_boundary_7d3f";
        let metadata = match existing_id {
            Some(_) => serde_json::json!({ "name": name }),
            None => serde_json::json!({ "name": name, "parents": [folder_id] }),
        };

        let mut body = Vec::with_capacity(content.len() + 512);
        body.extend_from_slice(
            format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(metadata.to_string().as_bytes());
        body.extend_from_slice(
            format!("\r\n--{boundary}\r\nContent-Type: application/octet-stream\r\n\r\n")
                .as_bytes(),
        );
        body.extend_from_slice(content);
        body.extend_from_slice(format!("\r\n--{boundary}--").as_bytes());

        let req = match existing_id {
            Some(id) => self.http.patch(format!("{UPLOAD_URL}/{id}")),
            None => self.http.post(UPLOAD_URL),
        };
        let resp = req
            .bearer_auth(&self.token)
            .query(&[("uploadType", "multipart")])
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/related; boundary={boundary}"),
            )
            .body(body)
            .send()
            .with_context(|| format!("enviando {name}"))?;
        Self::check(resp).with_context(|| format!("enviando {name}"))?;
        Ok(())
    }
}
