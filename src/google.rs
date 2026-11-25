//! Google Drive metadata-only integration and the Linux desktop Secret Service.
//!
//! OAuth uses the system browser, PKCE and a bounded IPv4 loopback callback.
//! Publisher credentials are embedded at build time; users never edit OAuth config.
//! Protocol reference: https://developers.google.com/identity/protocols/oauth2/native-app

use crate::{CredentialStore, Credentials, Drive, Entry, Identity, Page, ProviderError};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand::{RngCore, rngs::OsRng};
use reqwest::{Client, Response, StatusCode};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    process::{Command as ProcessCommand, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    process::Command as AsyncCommand,
};
use tokio_util::sync::CancellationToken;
use url::Url;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const DRIVE_URL: &str = "https://www.googleapis.com/drive/v3";
const METADATA_SCOPE: &str = "https://www.googleapis.com/auth/drive.metadata.readonly";
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(2);
const KEYRING_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CALLBACK: usize = 16 * 1024;
const MAX_TOKEN_RESPONSE: usize = 64 * 1024;
const MAX_METADATA_RESPONSE: usize = 8 * 1024 * 1024;
const MAX_SECRET: usize = 16 * 1024;

pub struct Google {
    client: Client,
    client_id: Option<&'static str>,
    client_secret: Option<&'static str>,
}

impl Google {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .https_only(true)
                .build()?,
            client_id: option_env!("SYNCJETTY_GOOGLE_CLIENT_ID").filter(|id| !id.trim().is_empty()),
            client_secret: option_env!("SYNCJETTY_GOOGLE_CLIENT_SECRET")
                .filter(|secret| !secret.is_empty()),
        })
    }

    fn client_id(&self) -> Result<&'static str, ProviderError> {
        self.client_id.ok_or_else(|| {
            ProviderError::Configuration(
                "This Syncjetty build is not provisioned for Google sign-in. Obtain a publisher-provisioned release; the publisher must enable Drive, configure consent and embed a Desktop OAuth client. No user configuration file is required.".into(),
            )
        })
    }

    async fn authorize_inner(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(Identity, Credentials), ProviderError> {
        let client_id = self.client_id()?;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| ProviderError::Configuration("Cannot open the local Google sign-in callback. Allow loopback networking and retry.".into()))?;
        let address = listener.local_addr().map_err(|_| ProviderError::Unavailable)?;
        let redirect = format!("http://{address}/oauth2/callback");
        let verifier = random_token()?;
        let state = random_token()?;
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut authorization = Url::parse("https://accounts.google.com/o/oauth2/v2/auth")
            .map_err(|_| ProviderError::InvalidResponse)?;
        authorization.query_pairs_mut().extend_pairs([
            ("client_id", client_id),
            ("redirect_uri", redirect.as_str()),
            ("response_type", "code"),
            ("scope", METADATA_SCOPE),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", state.as_str()),
            ("access_type", "offline"),
            ("prompt", "consent select_account"),
        ]);
        let mut browser = AsyncCommand::new("xdg-open")
            .arg(authorization.as_str())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| browser_error())?;
        let host = address.to_string();
        let callback = receive_callback(&listener, &host, &state);
        tokio::pin!(callback);
        // xdg-open may exit immediately or stay alive until its browser exits.
        // Neither case should prevent receiving the redirect.
        let code = tokio::select! {
            result = &mut callback => result?,
            result = browser.wait() => {
                if !result.map_err(|_| browser_error())?.success() {
                    return Err(browser_error());
                }
                callback.await?
            }
            _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
        };
        let mut form = vec![
            ("client_id", client_id),
            ("code", code.as_str()),
            ("code_verifier", verifier.as_str()),
            ("redirect_uri", redirect.as_str()),
            ("grant_type", "authorization_code"),
        ];
        if let Some(secret) = self.client_secret {
            form.push(("client_secret", secret));
        }
        let response = self.client.post(TOKEN_URL).form(&form).send().await
            .map_err(|_| ProviderError::Unavailable)?;
        let token = decode_token(response).await?;
        let credentials = token.credentials(None)?;
        let identity = self.identity(&credentials).await?;
        Ok((identity, credentials))
    }

    async fn identity(&self, credentials: &Credentials) -> Result<Identity, ProviderError> {
        let about = self.client.get(format!("{DRIVE_URL}/about"))
            .bearer_auth(&credentials.access_token)
            .query(&[("fields", "user(permissionId,emailAddress,displayName)")])
            .send().await.map_err(|_| ProviderError::Unavailable)?;
        let about: About = metadata_json(about).await?;
        let root = self.client.get(format!("{DRIVE_URL}/files/root"))
            .bearer_auth(&credentials.access_token)
            .query(&[("fields", "id,mimeType"), ("supportsAllDrives", "true")])
            .send().await.map_err(|_| ProviderError::Unavailable)?;
        let root: Root = metadata_json(root).await?;
        if !valid_id(&about.user.permission_id) || !valid_id(&root.id)
            || about.user.email_address.is_empty() || about.user.display_name.is_empty()
            || root.mime_type != "application/vnd.google-apps.folder"
        {
            return Err(ProviderError::InvalidResponse);
        }
        Ok(Identity {
            id: about.user.permission_id,
            email: about.user.email_address,
            name: about.user.display_name,
            root_id: root.id,
        })
    }
}

#[async_trait]
impl Drive for Google {
    async fn authorize(&self, cancel: CancellationToken) -> Result<(Identity, Credentials), ProviderError> {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ProviderError::Cancelled),
            result = tokio::time::timeout(AUTH_TIMEOUT, self.authorize_inner(&cancel)) => {
                result.unwrap_or_else(|_| Err(ProviderError::Configuration(
                    "Google sign-in timed out. Choose Connect to try again.".into(),
                )))
            }
        }
    }

    async fn refresh(&self, credentials: &Credentials) -> Result<Credentials, ProviderError> {
        let client_id = self.client_id()?;
        if credentials.refresh_token.is_empty() {
            return Err(ProviderError::Unauthorized);
        }
        let mut form = vec![
            ("client_id", client_id),
            ("refresh_token", credentials.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
        ];
        if let Some(secret) = self.client_secret {
            form.push(("client_secret", secret));
        }
        let response = self.client.post(TOKEN_URL).form(&form).send().await
            .map_err(|_| ProviderError::Unavailable)?;
        decode_token(response).await?.credentials(Some(&credentials.refresh_token))
    }

    async fn list(&self, credentials: &Credentials, folder: &str, page: Option<&str>) -> Result<Page, ProviderError> {
        // Strict IDs exclude quote/backslash characters, so none can escape the
        // Drive query literal or introduce another query predicate.
        if !valid_id(folder) || page.is_some_and(|p| p.is_empty() || p.len() > 8192 || p.chars().any(char::is_control)) {
            return Err(ProviderError::InvalidResponse);
        }
        if credentials.access_token.is_empty() {
            return Err(ProviderError::Unauthorized);
        }
        let query = format!("'{folder}' in parents and trashed = false");
        let mut request = self.client.get(format!("{DRIVE_URL}/files"))
            .bearer_auth(&credentials.access_token)
            .query(&[
                ("q", query.as_str()),
                ("fields", "files(id,name,mimeType,parents,modifiedTime,size),nextPageToken,incompleteSearch"),
                ("pageSize", "1000"),
                ("spaces", "drive"),
                ("corpora", "user"),
                ("supportsAllDrives", "true"),
                ("includeItemsFromAllDrives", "true"),
            ]);
        if let Some(page) = page {
            request = request.query(&[("pageToken", page)]);
        }
        let response = request.send().await.map_err(|_| ProviderError::Unavailable)?;
        let listing: FileList = metadata_json(response).await?;
        if listing.files.len() > 1000 || listing.files.iter().any(|file| {
            !valid_id(&file.id) || file.mime_type.is_empty()
                || file.parents.iter().any(|id| !valid_id(id))
                || !file.parents.iter().any(|id| id == folder)
        }) || listing.next_page_token.as_ref().is_some_and(|p| p.is_empty() || p.len() > 8192 || p.chars().any(char::is_control)) {
            return Err(ProviderError::InvalidResponse);
        }
        Ok(Page {
            entries: listing.files.into_iter().map(|file| Entry {
                id: file.id,
                name: file.name,
                mime_type: file.mime_type,
                parents: file.parents,
                modified_time: file.modified_time,
                size: file.size,
            }).collect(),
            next_page: listing.next_page_token,
            incomplete: listing.incomplete_search,
        })
    }
}

fn browser_error() -> ProviderError {
    ProviderError::Configuration("Cannot launch the system browser with xdg-open. Install/configure a default desktop browser and retry Connect.".into())
}

fn random_token() -> Result<Zeroizing<String>, ProviderError> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    OsRng.try_fill_bytes(bytes.as_mut()).map_err(|_| ProviderError::Unavailable)?;
    Ok(Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_ref())))
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 256
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

async fn receive_callback(listener: &TcpListener, host: &str, state: &str) -> Result<Zeroizing<String>, ProviderError> {
    loop {
        let (mut socket, peer) = listener.accept().await.map_err(|_| ProviderError::Unavailable)?;
        if !peer.ip().is_loopback() {
            continue;
        }
        // Ignore malformed/unrelated local traffic, including browser favicon
        // requests. Only a callback bearing this flow's state may terminate it.
        if let Ok(Ok(Some(result))) = tokio::time::timeout(
            CALLBACK_TIMEOUT, read_callback(&mut socket, host, state),
        ).await {
            return result;
        }
    }
}

async fn read_callback(socket: &mut TcpStream, host: &str, state: &str)
    -> std::io::Result<Option<Result<Zeroizing<String>, ProviderError>>>
{
    let mut request = Zeroizing::new(Vec::with_capacity(2048));
    let mut chunk = [0u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = socket.read(&mut chunk).await?;
        if count == 0 { return Ok(None); }
        if request.len() + count > MAX_CALLBACK {
            chunk.zeroize();
            send_callback_reply(socket, false).await?;
            return Ok(None);
        }
        request.extend_from_slice(&chunk[..count]);
        chunk.zeroize();
    }
    let Ok(text) = std::str::from_utf8(&request) else {
        send_callback_reply(socket, false).await?;
        return Ok(None);
    };
    let mut lines = text.split("\r\n");
    let mut first = lines.next().unwrap_or_default().split(' ');
    let method = first.next();
    let target = first.next().unwrap_or_default();
    let version = first.next();
    let mut hosts = lines.take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.trim());
    if method != Some("GET") || !matches!(version, Some("HTTP/1.1" | "HTTP/1.0"))
        || first.next().is_some() || hosts.next() != Some(host) || hosts.next().is_some()
    {
        send_callback_reply(socket, false).await?;
        return Ok(None);
    }
    let Some(("/oauth2/callback", query)) = target.split_once('?') else {
        send_callback_reply(socket, false).await?;
        return Ok(None);
    };
    let mut received_state = None;
    let mut code = None;
    let mut error = None;
    let mut duplicate = false;
    for (name, value) in url::form_urlencoded::parse(query.as_bytes()) {
        let slot = match name.as_ref() {
            "state" => &mut received_state,
            "code" => &mut code,
            "error" => &mut error,
            _ => continue,
        };
        if slot.is_some() { duplicate = true; }
        *slot = Some(Zeroizing::new(value.into_owned()));
    }
    if duplicate || received_state.as_deref().map(String::as_str) != Some(state)
        || code.is_some() == error.is_some()
    {
        send_callback_reply(socket, false).await?;
        return Ok(None);
    }
    let result = if let Some(error) = error {
        if error.as_str() == "access_denied" {
            Err(ProviderError::Cancelled)
        } else {
            Err(ProviderError::Configuration("Google could not authorize this sign-in. Retry Connect; if it persists, contact the publisher to check OAuth consent and Desktop client provisioning.".into()))
        }
    } else if let Some(code) = code.filter(|code| !code.is_empty()) {
        Ok(code)
    } else {
        send_callback_reply(socket, false).await?;
        return Ok(None);
    };
    // A disconnected browser must not discard a valid authorization response.
    let _ = send_callback_reply(socket, result.is_ok()).await;
    Ok(Some(result))
}

async fn send_callback_reply(socket: &mut TcpStream, accepted: bool) -> std::io::Result<()> {
    let (status, body) = if accepted {
        ("200 OK", "Google sign-in response received. Return to Syncjetty to check connection status. You can close this tab.")
    } else {
        ("400 Bad Request", "This sign-in response was not accepted. Return to Syncjetty; retry Connect if necessary.")
    };
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n{body}", body.len());
    socket.write_all(response.as_bytes()).await?;
    socket.shutdown().await
}

#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    refresh_token: Option<String>,
    scope: Option<String>,
}

impl TokenResponse {
    fn credentials(mut self, previous: Option<&str>) -> Result<Credentials, ProviderError> {
        if !self.token_type.eq_ignore_ascii_case("Bearer") || self.access_token.is_empty()
            || self.access_token.len() > MAX_SECRET
            || self.scope.as_ref().is_some_and(|scopes| !scopes.split_whitespace().any(|scope| scope == METADATA_SCOPE))
        {
            return Err(ProviderError::InvalidResponse);
        }
        let refresh_token = self.refresh_token.take()
            .filter(|token| !token.is_empty())
            .or_else(|| previous.map(str::to_owned))
            .ok_or(ProviderError::Unauthorized)?;
        if refresh_token.len() > MAX_SECRET {
            let mut refresh_token = refresh_token;
            refresh_token.zeroize();
            return Err(ProviderError::InvalidResponse);
        }
        Ok(Credentials { access_token: std::mem::take(&mut self.access_token), refresh_token })
    }
}

#[derive(Deserialize)]
struct OAuthError { error: String }

async fn decode_token(response: Response) -> Result<TokenResponse, ProviderError> {
    let status = response.status();
    let body = bounded_body(response, MAX_TOKEN_RESPONSE).await?;
    if status.is_success() {
        return serde_json::from_slice(&body).map_err(|_| ProviderError::InvalidResponse);
    }
    let code = serde_json::from_slice::<OAuthError>(&body).ok().map(|error| error.error);
    match code.as_deref() {
        Some("invalid_grant") => Err(ProviderError::Unauthorized),
        Some("invalid_client" | "unauthorized_client" | "deleted_client") => Err(ProviderError::Configuration(
            "Google rejected this build's OAuth client. Contact the Syncjetty publisher for a correctly provisioned release.".into(),
        )),
        _ if status == StatusCode::UNAUTHORIZED => Err(ProviderError::Unauthorized),
        _ if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS => Err(ProviderError::Unavailable),
        _ => Err(ProviderError::InvalidResponse),
    }
}

async fn bounded_body(mut response: Response, max: usize) -> Result<Zeroizing<Vec<u8>>, ProviderError> {
    if response.content_length().is_some_and(|size| size > max as u64) {
        return Err(ProviderError::InvalidResponse);
    }
    let mut body = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(|_| ProviderError::Unavailable)? {
        if chunk.len() > max - body.len() { return Err(ProviderError::InvalidResponse); }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn metadata_json<T: serde::de::DeserializeOwned>(response: Response) -> Result<T, ProviderError> {
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED { return Err(ProviderError::Unauthorized); }
    if !status.is_success() {
        return Err(if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::FORBIDDEN {
            ProviderError::Unavailable
        } else { ProviderError::InvalidResponse });
    }
    let body = bounded_body(response, MAX_METADATA_RESPONSE).await?;
    serde_json::from_slice(&body).map_err(|_| ProviderError::InvalidResponse)
}

#[derive(Deserialize)]
struct About { user: User }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct User { permission_id: String, email_address: String, display_name: String }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Root { id: String, mime_type: String }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileList {
    #[serde(default)]
    files: Vec<DriveFile>,
    next_page_token: Option<String>,
    incomplete_search: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveFile {
    id: String,
    name: String,
    mime_type: String,
    #[serde(default)]
    parents: Vec<String>,
    modified_time: Option<String>,
    size: Option<String>,
}

/// Stores only the long-lived refresh token in the desktop's Secret Service.
/// Access tokens remain in memory and are renewed after loading credentials.
#[derive(Default)]
pub struct SecretService;

impl CredentialStore for SecretService {
    fn load(&self, account: &str) -> anyhow::Result<Credentials> {
        validate_account(account)?;
        let mut output = secret_tool(&["lookup", "application", "syncjetty", "account", account], None)?;
        let mut refresh_token = match String::from_utf8(std::mem::take(&mut *output)) {
            Ok(token) => Zeroizing::new(token),
            Err(error) => {
                let _invalid = Zeroizing::new(error.into_bytes());
                anyhow::bail!("The desktop keyring returned an invalid Syncjetty credential; reconnect this account.");
            }
        };
        // secret-tool writes one record terminator, which is not part of the secret.
        if refresh_token.ends_with('\n') { refresh_token.pop(); }
        if refresh_token.ends_with('\r') { refresh_token.pop(); }
        validate_secret(&refresh_token)?;
        Ok(Credentials { access_token: String::new(), refresh_token: std::mem::take(&mut *refresh_token) })
    }

    fn save(&self, account: &str, credentials: &Credentials) -> anyhow::Result<()> {
        validate_account(account)?;
        validate_secret(&credentials.refresh_token)?;
        secret_tool(&["store", "--label=Syncjetty Google Drive", "application", "syncjetty", "account", account], Some(&credentials.refresh_token))?;
        Ok(())
    }
}

fn validate_account(account: &str) -> anyhow::Result<()> {
    anyhow::ensure!(valid_id(account), "Cannot address the desktop keyring with an invalid Google account identifier");
    Ok(())
}

fn validate_secret(secret: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!secret.is_empty() && secret.len() <= MAX_SECRET && !secret.chars().any(char::is_control),
        "The desktop keyring credential is missing or invalid; reconnect this account");
    Ok(())
}

fn secret_tool(args: &[&str], secret: Option<&str>) -> anyhow::Result<Zeroizing<Vec<u8>>> {
    let mut child = ProcessCommand::new("secret-tool").args(args)
        .stdin(if secret.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .map_err(|_| anyhow::anyhow!("Cannot open the desktop Secret Service. Install secret-tool (libsecret), run a desktop keyring service, and unlock it; Syncjetty does not store credentials in plaintext."))?;
    let deadline = Instant::now() + KEYRING_TIMEOUT;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (out_tx, out_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Zeroizing::new(Vec::new());
        let result = stdout.take((MAX_SECRET + 3) as u64).read_to_end(&mut bytes);
        let _ = out_tx.send((result, bytes));
    });
    // Capture bounded diagnostics but never propagate raw helper output: it can
    // contain secret material or untrusted D-Bus service text.
    let (err_tx, err_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Zeroizing::new(Vec::new());
        let _ = stderr.take(4097).read_to_end(&mut bytes);
        let _ = err_tx.send(bytes);
    });
    let (input_tx, input_rx) = mpsc::channel();
    if let Some(secret) = secret {
        let mut stdin = child.stdin.take().expect("piped stdin");
        let secret = Zeroizing::new(secret.as_bytes().to_vec());
        std::thread::spawn(move || {
            let result = stdin.write_all(&secret);
            drop(stdin);
            let _ = input_tx.send(result);
        });
    }
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("Desktop keyring access timed out or failed. Start/unlock the Secret Service and retry; no plaintext credential was saved.");
            }
        }
    };
    // Also bound inherited pipes; an unexpectedly lingering helper cannot block
    // the engine indefinitely after secret-tool has exited.
    let remaining = || deadline.saturating_duration_since(Instant::now());
    let (read_result, output) = out_rx.recv_timeout(remaining())
        .map_err(|_| anyhow::anyhow!("Desktop keyring did not finish returning credentials; unlock it and retry"))?;
    let _diagnostics = err_rx.recv_timeout(remaining())
        .map_err(|_| anyhow::anyhow!("Desktop keyring did not finish responding; unlock it and retry"))?;
    anyhow::ensure!(status.success(), "Desktop keyring access failed. Unlock the Secret Service and retry; if this account has no saved credential, reconnect it. No plaintext fallback is used.");
    anyhow::ensure!(read_result.is_ok() && output.len() <= MAX_SECRET + 2,
        "Desktop keyring returned an invalid or oversized credential; reconnect this account");
    if secret.is_some() {
        anyhow::ensure!(input_rx.recv_timeout(remaining()).is_ok_and(|result| result.is_ok()),
            "Could not send credentials securely to the desktop keyring; unlock it and retry");
    }
    Ok(output)
}
