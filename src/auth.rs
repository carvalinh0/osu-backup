//! Google sign-in with OAuth2 (desktop app, PKCE + loopback redirect).
//! Uses the `drive.file` scope: the app can only see files it created itself.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

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
const ACCEPT_POLL: Duration = Duration::from_millis(100);
const TIMEOUT_MESSAGE: &str = "timed out waiting for the browser sign-in";

/// Contents of the client_secret.json downloaded from Google Cloud ("Desktop app" type).
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
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let file: ClientSecretFile =
            serde_json::from_str(&text).context("invalid client_secret.json")?;
        file.installed
            .or(file.web)
            .context("client_secret.json has no 'installed' entry (use the Desktop app type)")
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
    std::fs::write(&path, token).with_context(|| format!("writing {}", path.display()))?;
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
        .with_context(|| format!("reading {}", path.display()))?
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

/// Runs the interactive sign-in in the browser and stores the refresh token.
pub fn login(app: &OAuthApp) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0").context("opening local port")?;
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

    tracing::info!("opening the browser for Google sign-in");
    if open::that(url.as_str()).is_err() {
        tracing::warn!("could not open the browser; visit: {url}");
    }

    let code = wait_for_code(&listener, csrf.secret(), LOGIN_TIMEOUT)?;
    let token = client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(verifier)
        .request(&http_client()?)
        .map_err(|e| anyhow!("exchanging code for token: {e:?}"))?;

    let refresh = token
        .refresh_token()
        .context("Google returned no refresh_token; revoke the app's access and try again")?;
    save_refresh_token(refresh.secret())?;
    Ok(())
}

/// Waits for Google's redirect on the loopback port and extracts the `code`.
/// Gives up after `timeout`, so a sign-in the user abandons does not leave setup busy forever.
fn wait_for_code(
    listener: &TcpListener,
    expected_state: &str,
    timeout: Duration,
) -> Result<String> {
    listener.set_nonblocking(true)?;
    let deadline = Instant::now() + timeout;
    loop {
        let mut stream = accept_before(listener, deadline)?;
        let line = read_request_line(&stream, deadline)?;

        // E.g. "GET /?state=...&code=... HTTP/1.1"
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

        // Ignore requests that are not the redirect (e.g. favicon).
        if get("code").is_none() && get("error").is_none() {
            let _ = respond(&mut stream, "osu!backup: waiting for sign-in.");
            continue;
        }

        if let Some(err) = get("error") {
            let _ = respond(&mut stream, "Sign-in cancelled. You can close this tab.");
            bail!("sign-in denied: {err}");
        }
        if get("state").as_deref() != Some(expected_state) {
            let _ = respond(&mut stream, "Invalid state. Please try again.");
            bail!("invalid state parameter in Google's response");
        }
        let _ = respond(
            &mut stream,
            "Signed in. You can close this tab and return to the app.",
        );
        return get("code").context("missing code");
    }
}

/// A blocking `accept()` would never notice the deadline when the browser never connects,
/// so the listener is polled instead.
fn accept_before(listener: &TcpListener, deadline: Instant) -> Result<TcpStream> {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // Accepted sockets inherit non-blocking mode on Windows.
                stream.set_nonblocking(false)?;
                // A browser may open a connection early and send the redirect on it later,
                // so allow reads up to the deadline rather than a short fixed timeout.
                let remaining = deadline.saturating_duration_since(Instant::now());
                stream.set_read_timeout(Some(remaining.max(Duration::from_millis(1))))?;
                return Ok(stream);
            }
            Err(e) if e.kind() != ErrorKind::WouldBlock => {
                return Err(e).context("accepting the sign-in redirect");
            }
            Err(_) if Instant::now() >= deadline => bail!(TIMEOUT_MESSAGE),
            Err(_) => thread::sleep(ACCEPT_POLL),
        }
    }
}

fn read_request_line(stream: &TcpStream, deadline: Instant) -> Result<String> {
    let mut line = String::new();
    match BufReader::new(stream).read_line(&mut line) {
        Ok(_) => Ok(line),
        Err(_) if Instant::now() >= deadline => bail!(TIMEOUT_MESSAGE),
        Err(e) => Err(e).context("reading the sign-in redirect"),
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

/// Gets a new access token from the stored refresh token.
pub fn access_token(app: &OAuthApp) -> Result<String> {
    let refresh = load_refresh_token()?;
    let client = build_client(app)?;
    let token = client
        .exchange_refresh_token(&RefreshToken::new(refresh))
        .request(&http_client()?)
        .map_err(|e| anyhow!("refreshing Google token: {e:?}"))?;
    Ok(token.access_token().secret().to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    /// Runs `wait_for_code` on a thread so a hang fails the test instead of blocking it.
    fn wait_in_background(timeout: Duration) -> (u16, mpsc::Receiver<Result<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(wait_for_code(&listener, "abc", timeout));
        });
        (port, rx)
    }

    fn finished(rx: &mpsc::Receiver<Result<String>>) -> Result<String> {
        rx.recv_timeout(Duration::from_secs(5))
            .expect("wait_for_code did not return")
    }

    #[test]
    fn returns_code_from_redirect() {
        let (port, rx) = wait_in_background(Duration::from_secs(5));
        let mut browser = TcpStream::connect(("127.0.0.1", port)).unwrap();
        browser
            .write_all(b"GET /?state=abc&code=xyz HTTP/1.1\r\n\r\n")
            .unwrap();
        assert_eq!(finished(&rx).unwrap(), "xyz");
    }

    #[test]
    fn gives_up_when_browser_never_connects() {
        let (_port, rx) = wait_in_background(Duration::from_millis(200));
        let err = finished(&rx).unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err:#}");
    }

    #[test]
    fn gives_up_when_connection_sends_nothing() {
        let (port, rx) = wait_in_background(Duration::from_millis(200));
        let _silent = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let err = finished(&rx).unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err:#}");
    }
}
