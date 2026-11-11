//! Runs a backup: compares local files with Drive and uploads only what changed.

use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::auth;
use crate::backup::{LocalFile, collect_essential_files, encode_drive_name, md5_hex};
use crate::config::Config;
use crate::drive::Drive;

#[derive(Debug, Clone, Copy, Default)]
pub struct Report {
    pub uploaded: usize,
}

/// The Drive operations a backup needs, so the change detection can be tested
/// without network access.
pub trait RemoteFolder {
    /// Maps name -> (id, md5) for every file in the folder.
    fn folder_index(&self, folder_id: &str) -> Result<HashMap<String, (String, Option<String>)>>;
    /// Creates the file, or replaces it when `existing_id` is given.
    fn upload(
        &self,
        folder_id: &str,
        name: &str,
        existing_id: Option<&str>,
        content: &[u8],
    ) -> Result<()>;
}

impl RemoteFolder for Drive {
    fn folder_index(&self, folder_id: &str) -> Result<HashMap<String, (String, Option<String>)>> {
        Drive::folder_index(self, folder_id)
    }

    fn upload(
        &self,
        folder_id: &str,
        name: &str,
        existing_id: Option<&str>,
        content: &[u8],
    ) -> Result<()> {
        Drive::upload(self, folder_id, name, existing_id, content)
    }
}

pub fn run(cfg: &Config) -> Result<Report> {
    let osu_dir = cfg
        .osu_dir_or_default()
        .context("osu!lazer folder not configured")?;
    let folder_id = cfg
        .drive_folder_id
        .as_deref()
        .context("Drive backup folder not configured")?;
    let creds_path = cfg
        .credentials_path
        .as_deref()
        .context("Google credentials not configured")?;

    let app = auth::ClientSecretFile::load(creds_path)?;
    let token = auth::access_token(&app)?;
    let drive = Drive::new(token)?;

    let files = collect_essential_files(&osu_dir)?;
    upload_changes(&drive, folder_id, &files)
}

/// Uploads the files whose MD5 differs from the copy in the folder, or that are not there yet.
pub fn upload_changes(
    remote: &impl RemoteFolder,
    folder_id: &str,
    files: &[LocalFile],
) -> Result<Report> {
    let index = remote.folder_index(folder_id)?;

    let mut report = Report::default();
    for file in files {
        // Read once so the hash and the uploaded content come from the same moment.
        let bytes = std::fs::read(&file.path)
            .with_context(|| format!("reading {}", file.path.display()))?;
        let md5 = md5_hex(&bytes);

        let name = encode_drive_name(&file.rel);
        let existing = index.get(&name);
        if existing.and_then(|(_, remote_md5)| remote_md5.as_deref()) == Some(md5.as_str()) {
            continue;
        }

        let existing_id = existing.map(|(id, _)| id.as_str());
        remote.upload(folder_id, &name, existing_id, &bytes)?;
        report.uploaded += 1;
        tracing::info!("uploaded: {}", file.rel);
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::path::PathBuf;

    use super::*;

    #[derive(Default)]
    struct FakeFolder {
        index: HashMap<String, (String, Option<String>)>,
        uploads: RefCell<Vec<(String, Option<String>)>>,
    }

    impl RemoteFolder for FakeFolder {
        fn folder_index(
            &self,
            _folder_id: &str,
        ) -> Result<HashMap<String, (String, Option<String>)>> {
            Ok(self.index.clone())
        }

        fn upload(
            &self,
            _folder_id: &str,
            name: &str,
            existing_id: Option<&str>,
            _content: &[u8],
        ) -> Result<()> {
            self.uploads
                .borrow_mut()
                .push((name.to_string(), existing_id.map(str::to_string)));
            Ok(())
        }
    }

    fn local_file(dir: &std::path::Path, rel: &str, content: &[u8]) -> LocalFile {
        let path: PathBuf = dir.join(rel.replace('/', "_"));
        std::fs::write(&path, content).unwrap();
        LocalFile {
            rel: rel.to_string(),
            path,
        }
    }

    #[test]
    fn uploads_only_new_and_changed_files() {
        let dir = std::env::temp_dir().join("osubackup-test-sync");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let files = vec![
            local_file(&dir, "client.realm", b"changed"),
            local_file(&dir, "game.ini", b"same"),
            local_file(&dir, "skins/A/skin.ini", b"new"),
        ];
        let mut fake = FakeFolder::default();
        fake.index.insert(
            "client.realm".into(),
            ("id-realm".into(), Some(md5_hex(b"old"))),
        );
        fake.index
            .insert("game.ini".into(), ("id-ini".into(), Some(md5_hex(b"same"))));

        let report = upload_changes(&fake, "folder", &files).unwrap();

        assert_eq!(report.uploaded, 2);
        assert_eq!(
            fake.uploads.into_inner(),
            vec![
                ("client.realm".to_string(), Some("id-realm".to_string())),
                ("skins%2FA%2Fskin.ini".to_string(), None),
            ]
        );
    }
}
