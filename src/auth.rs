//! Login no Google com OAuth2 (app desktop, PKCE + redirect loopback).
//! Usa o escopo `drive.file`: o app só enxerga arquivos que ele mesmo criou.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use oauth2::basic::BasicClient;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge, RedirectUrl,
    RefreshToken, Scope, TokenResponse, TokenUrl,
};
use serde::Deserialize;

use crate::config;

pub const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// Conteúdo do client_secret.json baixado do Google Cloud (tipo "Desktop app").
#[derive(Debug, Deserialize)]
pub struct ClientSecretFile {
    installed: Option<OAuthApp>,
    web: Option<OAuthApp>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OAuthApp {
    pub client_id: String,
    pub client_secret: String,
    pub auth_uri: String,
    pub token_uri: String,
}

impl ClientSecretFile {
    pub fn load(path: &Path) -> Result<OAuthApp> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("lendo {}", path.display()))?;
        let file: ClientSecretFile =
            serde_json::from_str(&text).context("client_secret.json inválido")?;
        file.installed
            .or(file.web)
            .context("client_secret.json não contém 'installed' (use o tipo Desktop app)")
    }
}

fn refresh_token_path() -> Result<PathBuf> {
    Ok(config::Config::config_dir()?.join("refresh_token"))
}

pub fn has_login() -> bool {
    refresh_token_path().map(|p| p.exists()).unwrap_or(false)
}

fn save_refresh_token(token: &str) -> Result<()> {
    let path = refresh_token_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn load_refresh_token() -> Result<String> {
    let path = refresh_token_path()?;
    Ok(std::fs::read_to_string(&path)
        .with_context(|| format!("lendo {}", path.display()))?
        .trim()
        .to_string())
}

fn http_client() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

fn build_client(
    app: &OAuthApp,
) -> Result<
    BasicClient<
        oauth2::EndpointSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointNotSet,
        oauth2::EndpointSet,
    >,
> {
    Ok(BasicClient::new(ClientId::new(app.client_id.clone()))
        .set_client_secret(ClientSecret::new(app.client_secret.clone()))
        .set_auth_uri(AuthUrl::new(app.auth_uri.clone())?)
        .set_token_uri(TokenUrl::new(app.token_uri.clone())?))
}

/// Faz o login interativo no navegador e guarda o refresh token.
pub fn login(app: &OAuthApp) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").context("abrindo porta local")?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}");

    let client = build_client(app)?.set_redirect_uri(RedirectUrl::new(redirect)?);
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (url, csrf) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new(DRIVE_SCOPE.to_string()))
        .add_extra_param("access_type", "offline")
        .add_extra_param("prompt", "consent")
        .set_pkce_challenge(challenge)
        .url();

    tracing::info!("abrindo o navegador para login no Google");
    if open::that(url.as_str()).is_err() {
        tracing::warn!("não consegui abrir o navegador; acesse: {url}");
    }

    let code = wait_for_code(&listener, csrf.secret())?;
    let token = client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(verifier)
        .request(&http_client()?)
        .map_err(|e| anyhow!("trocando código por token: {e:?}"))?;

    let refresh = token
        .refresh_token()
        .context("Google não devolveu refresh_token; revogue o acesso do app e tente de novo")?;
    save_refresh_token(refresh.secret())?;
    Ok(())
}

/// Espera o redirect do Google na porta loopback e extrai o `code`.
fn wait_for_code(listener: &TcpListener, expected_state: &str) -> Result<String> {
    listener.set_nonblocking(false)?;
    let deadline = std::time::Instant::now() + LOGIN_TIMEOUT;
    loop {
        if std::time::Instant::now() > deadline {
            bail!("tempo esgotado esperando o login no navegador");
        }
        let (mut stream, _) = listener.accept()?;
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line)?;

        // Ex.: "GET /?state=...&code=... HTTP/1.1"
        let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
        let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
        let params: Vec<(&str, &str)> = query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .collect();

        let get = |k: &str| {
            params
                .iter()
                .find(|(key, _)| *key == k)
                .map(|(_, v)| v.to_string())
        };

        // Ignora requisições que não são o redirect (ex.: favicon).
        if get("code").is_none() && get("error").is_none() {
            let _ = respond(&mut stream, "osu!backup: aguardando login.");
            continue;
        }

        if let Some(err) = get("error") {
            let _ = respond(&mut stream, "Login cancelado. Pode fechar esta aba.");
            bail!("login recusado: {err}");
        }
        if get("state").as_deref() != Some(expected_state) {
            let _ = respond(&mut stream, "Estado inválido. Tente novamente.");
            bail!("parâmetro state inválido no retorno do Google");
        }
        let _ = respond(
            &mut stream,
            "Login concluído. Pode fechar esta aba e voltar ao app.",
        );
        return get("code").context("código ausente");
    }
}

fn respond(stream: &mut impl Write, body: &str) -> std::io::Result<()> {
    let html = format!(
        "<!doctype html><meta charset=utf-8><body style='font-family:sans-serif;padding:2em'>{body}</body>"
    );
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    )
}

/// Gera um access token novo a partir do refresh token salvo.
pub fn access_token(app: &OAuthApp) -> Result<String> {
    let refresh = load_refresh_token()?;
    let client = build_client(app)?;
    let token = client
        .exchange_refresh_token(&RefreshToken::new(refresh))
        .request(&http_client()?)
        .map_err(|e| anyhow!("renovando token do Google: {e:?}"))?;
    Ok(token.access_token().secret().to_string())
}
