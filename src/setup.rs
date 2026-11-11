//! First-run flow: credentials → Google sign-in → Drive folder → osu! folder.
//! Runs on a background thread and reports progress via `UserEvent::Status`.

use anyhow::{Context, Result, bail};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

use crate::auth;
use crate::config::{self, Config};
use crate::drive::Drive;
use crate::ui::UserEvent;

pub type Notify = dyn Fn(String) + Send + Sync;

/// Runs the full setup. Skips steps that are already done, unless `force_login`.
pub fn run(force_login: bool, notify: &Notify) -> Result<()> {
    let mut cfg = Config::load()?;

    // 1. Google credentials (client_secret.json).
    let creds_dest = Config::config_dir()?.join("client_secret.json");
    if !creds_dest.exists() || cfg.credentials_path.is_none() {
        notify("Select the client_secret.json from Google Cloud".into());
        let picked = FileDialog::new()
            .set_title("Select the client_secret.json (Desktop app type)")
            .add_filter("JSON", &["json"])
            .pick_file()
            .context("no credentials file selected")?;
        // Validate before copying.
        auth::ClientSecretFile::load(&picked)?;
        config::ensure_dir(&Config::config_dir()?)?;
        std::fs::copy(&picked, &creds_dest)
            .with_context(|| format!("copying {}", picked.display()))?;
        cfg.credentials_path = Some(creds_dest.clone());
        cfg.save()?;
    }
    let app = auth::ClientSecretFile::load(&creds_dest)?;

    // 2. Google sign-in (opens the browser).
    if force_login || !auth::has_login() {
        notify("Sign in to Google in your browser".into());
        auth::login(&app)?;
    }

    // 3. Backup folder in Drive (created if missing).
    notify("Creating folder in Google Drive".into());
    let token = auth::access_token(&app)?;
    let folder_id = Drive::new(token)?.find_or_create_folder()?;
    cfg.drive_folder_id = Some(folder_id);

    // 4. osu!lazer folder: suggest the default and let the user confirm or change it.
    let default = config::default_osu_dir();
    let chosen = match default.as_ref().filter(|p| p.is_dir()) {
        Some(p) => {
            let answer = MessageDialog::new()
                .set_level(MessageLevel::Info)
                .set_title("osu!backup")
                .set_description(format!("Back up this osu!lazer folder?\n\n{}", p.display()))
                .set_buttons(MessageButtons::YesNo)
                .show();
            if answer == MessageDialogResult::Yes {
                p.clone()
            } else {
                pick_osu_dir(default.as_deref())?
            }
        }
        None => pick_osu_dir(default.as_deref())?,
    };
    cfg.osu_dir = Some(chosen);

    cfg.setup_done = true;
    cfg.save()?;
    Ok(())
}

fn pick_osu_dir(start: Option<&std::path::Path>) -> Result<std::path::PathBuf> {
    let mut dialog = FileDialog::new().set_title("Select the osu!lazer data folder");
    if let Some(s) = start.and_then(|p| p.parent()) {
        dialog = dialog.set_directory(s);
    }
    match dialog.pick_folder() {
        Some(p) => Ok(p),
        None => bail!("no osu!lazer folder selected"),
    }
}

/// Sends the status event to the tray.
pub fn status_notifier(
    proxy: tao::event_loop::EventLoopProxy<UserEvent>,
) -> impl Fn(String) + Send + Sync {
    move |text| {
        let _ = proxy.send_event(UserEvent::Status(text));
    }
}
