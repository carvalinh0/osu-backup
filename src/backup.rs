use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use md5::{Digest, Md5};
use walkdir::WalkDir;

/// Local file to be uploaded to Drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFile {
    /// Path relative to the osu! folder, with '/' as the separator.
    pub rel: String,
    pub path: PathBuf,
}

/// Collects only the essential osu!lazer data:
/// - client.realm (scores, beatmaps, collections, settings)
/// - skins/ and exports/
/// - root-level .ini/.json files
///
/// Skips `files/` (beatmaps and audio, which can be re-downloaded), locks, logs and caches.
pub fn collect_essential_files(osu_dir: &Path) -> Result<Vec<LocalFile>> {
    let mut out = Vec::new();

    let realm = osu_dir.join("client.realm");
    if realm.is_file() {
        out.push(LocalFile {
            rel: "client.realm".into(),
            path: realm,
        });
    }

    for entry in
        std::fs::read_dir(osu_dir).with_context(|| format!("reading {}", osu_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext.eq_ignore_ascii_case("ini") || ext.eq_ignore_ascii_case("json") {
            out.push(LocalFile { rel: name, path });
        }
    }

    for dir in ["skins", "exports"] {
        let base = osu_dir.join(dir);
        if !base.is_dir() {
            continue;
        }
        for entry in WalkDir::new(&base).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel_path = entry.path().strip_prefix(osu_dir)?;
            let rel = rel_path
                .components()
                .map(|c| c.as_os_str().to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("/");
            out.push(LocalFile {
                rel,
                path: entry.path().to_path_buf(),
            });
        }
    }

    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

/// File name in Drive. The Drive folder is flat, so '%' and '/' are escaped
/// reversibly to preserve the original structure.
pub fn encode_drive_name(rel: &str) -> String {
    rel.replace('%', "%25").replace('/', "%2F")
}

pub fn md5_hex(bytes: &[u8]) -> String {
    let mut hasher = Md5::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("osubackup-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn encoding_is_reversible() {
        let enc = encode_drive_name("skins/My%Skin/a/b.png");
        assert_eq!(enc, "skins%2FMy%25Skin%2Fa%2Fb.png");
    }

    #[test]
    fn md5_known_value() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn collects_only_essentials() {
        let dir = tmpdir("collect");
        std::fs::write(dir.join("client.realm"), b"realm").unwrap();
        std::fs::write(dir.join("client.realm.lock"), b"").unwrap();
        std::fs::write(dir.join("framework.ini"), b"x").unwrap();
        std::fs::create_dir_all(dir.join("skins/Test")).unwrap();
        std::fs::write(dir.join("skins/Test/skin.ini"), b"s").unwrap();
        std::fs::create_dir_all(dir.join("files/ab")).unwrap();
        std::fs::write(dir.join("files/ab/huge.mp3"), b"m").unwrap();
        std::fs::create_dir_all(dir.join("logs")).unwrap();
        std::fs::write(dir.join("logs/runtime.log"), b"l").unwrap();

        let rels: Vec<String> = collect_essential_files(&dir)
            .unwrap()
            .into_iter()
            .map(|f| f.rel)
            .collect();
        assert_eq!(
            rels,
            vec!["client.realm", "framework.ini", "skins/Test/skin.ini"]
        );
    }
}
