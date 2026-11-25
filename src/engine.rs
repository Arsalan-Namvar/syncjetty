use crate::*;
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, params};
use std::{fs, os::unix::fs::{OpenOptionsExt, PermissionsExt}, path::{Path, Component}, time::{SystemTime, UNIX_EPOCH}};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct Engine(Arc<Shared>);
struct Shared {
    state: Mutex<State>,
    dependencies: Dependencies,
}
struct State {
    database: Connection,
    snapshot: Snapshot,
    authorization: CancellationToken,
    generation: u64,
}
impl State {
    fn persist(&mut self) -> Result<()> {
        let json = serde_json::to_string(&self.snapshot)?;
        if let Err(error) = self.database.execute("INSERT INTO state(id, snapshot) VALUES(1, ?1) ON CONFLICT(id) DO UPDATE SET snapshot=excluded.snapshot", params![json]) {
            // Never expose approval or completed discovery that was not committed.
            let durable: Option<String> = self.database.query_row("SELECT snapshot FROM state WHERE id=1", [], |row| row.get(0)).ok();
            if let Some(durable) = durable {
                self.snapshot = serde_json::from_str(&durable)?;
            } else {
                self.snapshot = Snapshot::default();
            }
            self.authorization.cancel();
            self.generation += 1;
            self.snapshot.authorizing = false;
            for listing in self.snapshot.accounts.iter_mut().flat_map(|a| &mut a.listings) {
                if listing.state == Discovery::Loading {
                    listing.state = Discovery::Failed("Local storage commit failed; last durable entries retained. Retry after fixing storage.".into());
                }
            }
            return Err(error.into());
        }
        Ok(())
    }
}

impl Engine {
    /// The daemon owns the exclusive state-directory lock. No network work during open.
    pub fn open(path: &Path, dependencies: Dependencies) -> Result<Self> {
        if !path.exists() {
            fs::OpenOptions::new().create_new(true).write(true).mode(0o600).open(path)?;
        }
        let metadata = fs::symlink_metadata(path)?;
        anyhow::ensure!(metadata.is_file() && !metadata.file_type().is_symlink(), "State database must be a regular file");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        let database = Connection::open(path)?;
        database.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS state (id INTEGER PRIMARY KEY CHECK(id=1), snapshot TEXT NOT NULL);")?;
        let mut snapshot: Snapshot = {
            let mut statement = database.prepare("SELECT snapshot FROM state WHERE id=1")?;
            let mut rows = statement.query([])?;
            match rows.next()? {
                Some(row) => serde_json::from_str(&row.get::<_, String>(0)?).context("State cannot be decoded; preserved without resetting")?,
                None => Snapshot::default(),
            }
        };
        snapshot.authorizing = false;
        for listing in snapshot.accounts.iter_mut().flat_map(|a| &mut a.listings) {
            if listing.state == Discovery::Loading {
                listing.state = Discovery::Failed("Discovery interrupted; retry. Last-known entries are retained.".into());
            }
        }
        let mut state = State { database, snapshot, authorization: CancellationToken::new(), generation: 0 };
        state.persist()?;
        Ok(Self(Arc::new(Shared { state: Mutex::new(state), dependencies })))
    }

    /// Connect and Browse enqueue bounded work; consumers poll Snapshot for progress.
    pub async fn command(&self, command: Command) -> Result<Snapshot> {
        let mut state = self.0.state.lock().await;
        match command {
            Command::Snapshot => {},
            Command::Cancel => {
                state.authorization.cancel();
                state.generation += 1;
                state.snapshot.authorizing = false;
                state.snapshot.notice = Some("Authorization cancelled; existing accounts are unchanged".into());
                state.persist()?;
            },
            Command::Connect { reconnect } => {
                anyhow::ensure!(!state.snapshot.accounts.iter().flat_map(|a| &a.listings).any(|l| l.state == Discovery::Loading), "Wait for metadata discovery to finish before connecting");
                anyhow::ensure!(!state.snapshot.authorizing, "Authorization already in progress; Esc cancels");
                if let Some(id) = &reconnect {
                    anyhow::ensure!(state.snapshot.accounts.iter().any(|a| &a.identity.id == id), "Unknown account");
                } else {
                    anyhow::ensure!(state.snapshot.accounts.len() < 4, "At most four accounts can be connected");
                }
                state.authorization = CancellationToken::new();
                state.generation += 1;
                let generation = state.generation;
                let cancel = state.authorization.clone();
                state.snapshot.authorizing = true;
                state.snapshot.notice = Some("Complete authorization in the system browser; Esc cancels".into());
                state.persist()?;
                let engine = self.clone();
                tokio::spawn(async move { engine.authorize(generation, cancel, reconnect).await; });
            },
            Command::Activate { account_id, mount_path, cache_bytes, accept_deletion } => {
                anyhow::ensure!(accept_deletion, "Explicit permanent-deletion acknowledgement required");
                anyhow::ensure!(cache_bytes > 0 && cache_bytes <= i64::MAX as u64, "Cache budget must be a positive byte count");
                let index = state.snapshot.accounts.iter().position(|a| a.identity.id == account_id).context("Unknown account")?;
                anyhow::ensure!(!state.snapshot.accounts[index].activated, "Account already activated");
                let mount_path = empty_mount(&mount_path)?;
                anyhow::ensure!(!state.snapshot.accounts.iter().any(|a| a.activated && (a.mount_path.starts_with(&mount_path) || mount_path.starts_with(&a.mount_path))), "Mount locations must be separate dedicated directories");
                let account = &mut state.snapshot.accounts[index];
                account.mount_path = mount_path;
                account.cache_bytes = cache_bytes;
                account.activated = true;
                state.snapshot.notice = Some("Metadata browsing enabled. Filesystem NOT mounted; no file contents downloaded.".into());
                state.persist()?;
            },
            Command::Browse { account_id, folder_id, more } => {
                anyhow::ensure!(!state.snapshot.authorizing, "Finish or cancel authorization before browsing");
                let account = state.snapshot.accounts.iter_mut().find(|a| a.identity.id == account_id).context("Unknown account")?;
                anyhow::ensure!(account.activated, "Review and activate the account first");
                anyhow::ensure!(!account.needs_reconnect, "Reconnect this account before retrying");
                anyhow::ensure!(!account.listings.iter().any(|l| l.state == Discovery::Loading), "Account discovery is already loading; wait before browsing another folder");
                anyhow::ensure!(folder_id == account.identity.root_id || account.listings.iter().flat_map(|l| &l.entries).any(|e| e.id == folder_id && e.is_folder()), "Folder not discovered in this account");
                let index = match account.listings.iter().position(|l| l.folder_id == folder_id) {
                    Some(index) => index,
                    None => {
                        account.listings.push(Listing { folder_id: folder_id.clone(), entries: Vec::new(), state: Discovery::NotStarted, next_page: None, last_success: None, incomplete: false });
                        account.listings.len() - 1
                    }
                };
                let listing = &mut account.listings[index];
                anyhow::ensure!(listing.state != Discovery::Loading, "This folder is already loading");
                let page = if more { Some(listing.next_page.clone().context("No next page; refresh instead")?) } else { None };
                if !more { listing.next_page = None; listing.incomplete = false; }
                listing.state = Discovery::Loading;
                state.persist()?;
                let engine = self.clone();
                tokio::spawn(async move { engine.browse(account_id, folder_id, page).await; });
            },
        }
        Ok(state.snapshot.clone())
    }

    async fn authorize(&self, generation: u64, cancel: CancellationToken, reconnect: Option<String>) {
        let result = async {
            let (identity, credentials) = self.0.dependencies.drive.authorize(cancel.clone()).await?;
            anyhow::ensure!(!identity.id.is_empty() && !identity.root_id.is_empty() && !identity.email.is_empty(), "Google returned incomplete account identity");
            if let Some(id) = &reconnect {
                anyhow::ensure!(id == &identity.id, "Reconnection returned a different Google account; existing account is unchanged");
            }
            if cancel.is_cancelled() { bail!("Authorization cancelled"); }
            let store = self.0.dependencies.credentials.clone();
            let id = identity.id.clone();
            tokio::task::spawn_blocking(move || store.save(&id, &credentials)).await?
                .context("Cannot securely save credentials; unlock the desktop keyring and retry")?;
            Ok::<_, anyhow::Error>(identity)
        }.await;
        let mut state = self.0.state.lock().await;
        if state.generation != generation || cancel.is_cancelled() { return; }
        state.snapshot.authorizing = false;
        let result: Result<()> = (|| {
            let identity = result?;
            if let Some(account) = state.snapshot.accounts.iter_mut().find(|a| a.identity.id == identity.id) {
                account.identity = identity;
                account.needs_reconnect = false;
            } else {
                anyhow::ensure!(state.snapshot.accounts.len() < 4, "At most four accounts can be connected");
                state.snapshot.accounts.push(Account { identity, mount_path: PathBuf::new(), cache_bytes: DEFAULT_CACHE_BYTES, activated: false, needs_reconnect: false, listings: Vec::new() });
            }
            Ok(())
        })();
        state.snapshot.notice = Some(match result {
            Ok(()) => "Account authorized. Review activation before browsing; filesystem NOT mounted.".into(),
            Err(error) => error.to_string(),
        });
        if state.persist().is_err() {
            state.snapshot.notice = Some("Cannot persist account state; check local storage before continuing".into());
        }
    }

    async fn browse(&self, account_id: String, folder_id: String, page: Option<String>) {
        let result = async {
            let store = self.0.dependencies.credentials.clone();
            let id = account_id.clone();
            let credentials = tokio::task::spawn_blocking(move || store.load(&id)).await
                .map_err(|_| ProviderError::Unavailable)?
                .map_err(|_| ProviderError::Configuration("Unlock the desktop keyring, then retry or reconnect".into()))?;
            let credentials = self.0.dependencies.drive.refresh(&credentials).await?;
            let store = self.0.dependencies.credentials.clone();
            let id = account_id.clone();
            let saved = credentials.clone();
            tokio::task::spawn_blocking(move || store.save(&id, &saved)).await
                .map_err(|_| ProviderError::Unavailable)?
                .map_err(|_| ProviderError::Configuration("Cannot securely save refreshed authorization; unlock the desktop keyring".into()))?;
            self.0.dependencies.drive.list(&credentials, &folder_id, page.as_deref()).await
        }.await;
        let mut state = self.0.state.lock().await;
        let Some(account) = state.snapshot.accounts.iter_mut().find(|a| a.identity.id == account_id) else { return; };
        let Some(listing) = account.listings.iter_mut().find(|l| l.folder_id == folder_id) else { return; };
        match result {
            Ok(page) => {
                // Absence is not deletion. Retain last-known entries even after re-enumeration.
                if page.entries.iter().any(|entry| entry.id.is_empty() || entry.name.is_empty() || !entry.parents.contains(&folder_id)) {
                    listing.state = Discovery::Failed("Invalid metadata; last-known entries retained. Retry discovery.".into());
                    let _ = state.persist();
                    return;
                }
                let mut positions: std::collections::HashMap<String, usize> = listing.entries.iter().enumerate().map(|(index, entry)| (entry.id.clone(), index)).collect();
                for entry in page.entries {
                    if let Some(&index) = positions.get(&entry.id) { listing.entries[index] = entry; }
                    else {
                        positions.insert(entry.id.clone(), listing.entries.len());
                        listing.entries.push(entry);
                    }
                }
                listing.next_page = page.next_page;
                listing.incomplete |= page.incomplete;
                listing.state = if listing.incomplete || listing.next_page.is_some() { Discovery::Partial } else { Discovery::Complete };
                listing.last_success = Some(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs());
            },
            Err(error) => {
                if matches!(error, ProviderError::Unauthorized) { account.needs_reconnect = true; }
                listing.state = Discovery::Failed(error.to_string());
            },
        }
        if state.persist().is_err() {
            state.snapshot.notice = Some("Cannot persist discovery progress; check local storage and retry".into());
        }
    }
}

fn empty_mount(path: &Path) -> Result<PathBuf> {
    anyhow::ensure!(path.is_absolute(), "Mount location must be an absolute directory path");
    let mut current = PathBuf::new();
    for component in path.components() {
        if matches!(component, Component::ParentDir) { bail!("Mount path cannot contain parent-directory traversal"); }
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => bail!("Mount path cannot traverse symbolic links"),
            Ok(_) => {},
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir(&current).context("Cannot create dedicated mount directory")?,
            Err(error) => return Err(error.into()),
        }
    }
    let path = fs::canonicalize(path).context("Create a dedicated empty directory before activation")?;
    anyhow::ensure!(path.is_dir(), "Mount location must be a directory");
    anyhow::ensure!(fs::read_dir(&path)?.next().is_none(), "Mount location is populated; select a dedicated empty directory");
    anyhow::ensure!(path.parent().is_some(), "Filesystem root is not a dedicated mount location");
    Ok(path)
}
