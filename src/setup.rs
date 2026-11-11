//! Fluxo de primeira execução: credenciais → login Google → pasta no Drive → pasta do osu!.
//! Roda numa thread de fundo e reporta o progresso via `UserEvent::Status`.

use anyhow::{Context, Result, bail};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

use crate::auth;
use crate::config::{self, Config};
use crate::drive::Drive;
use crate::ui::UserEvent;

pub type Notify = dyn Fn(String) + Send + Sync;

/// Executa a configuração completa. Pula etapas que já estão prontas, a menos que `force_login`.
pub fn run(force_login: bool, notify: &Notify) -> Result<()> {
    let mut cfg = Config::load()?;

    // 1. Credenciais do Google (client_secret.json).
    let creds_dest = Config::config_dir()?.join("client_secret.json");
    if !creds_dest.exists() || cfg.credentials_path.is_none() {
        notify("Selecione o client_secret.json do Google Cloud".into());
        let picked = FileDialog::new()
            .set_title("Selecione o client_secret.json (tipo Desktop app)")
            .add_filter("JSON", &["json"])
            .pick_file()
            .context("nenhum arquivo de credenciais selecionado")?;
        // Valida antes de copiar.
        auth::ClientSecretFile::load(&picked)?;
        config::ensure_dir(&Config::config_dir()?)?;
        std::fs::copy(&picked, &creds_dest)?;
        cfg.credentials_path = Some(creds_dest.clone());
        cfg.save()?;
    }
    let app = auth::ClientSecretFile::load(&creds_dest)?;

    // 2. Login no Google (abre o navegador).
    if force_login || !auth::has_login() {
        notify("Faça login no Google no navegador".into());
        auth::login(&app)?;
    }

    // 3. Pasta de backup no Drive (cria se não existir).
    notify("Criando pasta no Google Drive".into());
    let token = auth::access_token(&app)?;
    let folder_id = Drive::new(token)?.find_or_create_folder()?;
    cfg.drive_folder_id = Some(folder_id);

    // 4. Pasta do osu!lazer: sugere a padrão e deixa o usuário confirmar ou trocar.
    let default = config::default_osu_dir();
    let chosen = match default.as_ref().filter(|p| p.is_dir()) {
        Some(p) => {
            let answer = MessageDialog::new()
                .set_level(MessageLevel::Info)
                .set_title("osu!backup")
                .set_description(format!(
                    "Usar esta pasta do osu!lazer para o backup?\n\n{}",
                    p.display()
                ))
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
    let mut dialog = FileDialog::new().set_title("Selecione a pasta de dados do osu!lazer");
    if let Some(s) = start.and_then(|p| p.parent()) {
        dialog = dialog.set_directory(s);
    }
    match dialog.pick_folder() {
        Some(p) => Ok(p),
        None => bail!("nenhuma pasta do osu!lazer selecionada"),
    }
}

/// Envia o evento de status para a bandeja.
pub fn status_notifier(
    proxy: tao::event_loop::EventLoopProxy<UserEvent>,
) -> impl Fn(String) + Send + Sync {
    move |text| {
        let _ = proxy.send_event(UserEvent::Status(text));
    }
}
