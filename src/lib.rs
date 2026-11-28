mod engine;
pub mod google;
pub mod ipc;
pub mod tui;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub use engine::Engine;

pub const DEFAULT_CACHE_BYTES: u64 = 10 * 1024 * 1024 * 1024;
pub const DELETION_WARNING: &str = "Confirmed nonconflicting deletions will be permanent and automatic. Syncjetty keeps no recovery copy. Shared-content deletion can affect others. Cache eviction is never deletion. This metadata-only slice cannot delete or mount files.";

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct Identity {
    pub id: String,
    pub email: String,
    pub name: String,
    pub root_id: String,
}

// Never serialize secrets into control responses or include them in Debug output.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Credentials {
    pub access_token: String,
    pub refresh_token: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub mime_type: String,
    pub parents: Vec<String>,
    pub modified_time: Option<String>,
    pub size: Option<String>,
}
impl Entry {
    pub fn is_folder(&self) -> bool {
        self.mime_type == "application/vnd.google-apps.folder"
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Page {
    pub entries: Vec<Entry>,
    pub next_page: Option<String>,
    pub incomplete: bool,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct MetadataChange {
    pub id: String,
    pub entry: Option<Entry>,
    pub removed: bool,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ChangesPage {
    pub changes: Vec<MetadataChange>,
    pub next_page: Option<String>,
    pub new_start: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub enum Discovery {
    NotStarted,
    Loading,
    Partial,
    Complete,
    Failed(String),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Listing {
    pub folder_id: String,
    pub entries: Vec<Entry>,
    pub state: Discovery,
    pub next_page: Option<String>,
    pub last_success: Option<u64>,
    #[serde(default)]
    pub incomplete: bool,
    #[serde(default)]
    pub change_cursor: Option<String>,
    #[serde(default)]
    pub replay_pending: bool,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Account {
    pub identity: Identity,
    pub mount_path: PathBuf,
    pub cache_bytes: u64,
    pub activated: bool,
    pub needs_reconnect: bool,
    pub listings: Vec<Listing>,
}

#[derive(Clone, Serialize, Deserialize, Debug, Default)]
pub struct Snapshot {
    pub accounts: Vec<Account>,
    pub authorizing: bool,
    pub notice: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum Command {
    Connect { reconnect: Option<String> },
    Cancel,
    Activate { account_id: String, mount_path: PathBuf, cache_bytes: u64, accept_deletion: bool },
    Browse { account_id: String, folder_id: String, more: bool },
    Snapshot,
}

#[derive(Clone, Debug)]
pub enum ProviderError {
    Cancelled,
    Unauthorized,
    Unavailable,
    InvalidResponse,
    Configuration(String),
}
impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "Authorization cancelled"),
            Self::Unauthorized => write!(f, "Authorization expired or revoked; reconnect this account"),
            Self::Unavailable => write!(f, "Google is unavailable; retry when online"),
            Self::InvalidResponse => write!(f, "Google returned incomplete or invalid metadata"),
            Self::Configuration(message) => write!(f, "{message}"),
        }
    }
}
impl std::error::Error for ProviderError {}

/// External Google adapter. Only metadata operations exist in this slice.
#[async_trait]
pub trait Drive: Send + Sync {
    async fn authorize(&self, cancel: CancellationToken) -> Result<(Identity, Credentials), ProviderError>;
    async fn refresh(&self, credentials: &Credentials) -> Result<Credentials, ProviderError>;
    async fn list(&self, credentials: &Credentials, folder: &str, page: Option<&str>) -> Result<Page, ProviderError>;
    async fn start_changes(&self, credentials: &Credentials) -> Result<String, ProviderError>;
    async fn changes(&self, credentials: &Credentials, cursor: &str) -> Result<ChangesPage, ProviderError>;
}

/// Secret Service in production; isolated credential storage in acceptance tests.
pub trait CredentialStore: Send + Sync {
    fn load(&self, account: &str) -> anyhow::Result<Credentials>;
    fn save(&self, account: &str, credentials: &Credentials) -> anyhow::Result<()>;
}

pub struct Dependencies {
    pub drive: Arc<dyn Drive>,
    pub credentials: Arc<dyn CredentialStore>,
}
