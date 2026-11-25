use async_trait::async_trait;
use std::{collections::HashMap, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}};
use syncjetty::*;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Secrets(Mutex<HashMap<String, Credentials>>);
impl CredentialStore for Secrets {
    fn load(&self, id: &str) -> anyhow::Result<Credentials> { self.0.lock().unwrap().get(id).cloned().ok_or_else(|| anyhow::anyhow!("locked")) }
    fn save(&self, id: &str, value: &Credentials) -> anyhow::Result<()> { self.0.lock().unwrap().insert(id.into(), value.clone()); Ok(()) }
}
struct Google {
    identity: Mutex<String>,
    wait_for_cancel: AtomicBool,
    revoked: AtomicBool,
}
#[async_trait]
impl Drive for Google {
    async fn authorize(&self, cancel: CancellationToken) -> Result<(Identity, Credentials), ProviderError> {
        if self.wait_for_cancel.load(Ordering::SeqCst) {
            cancel.cancelled().await;
            return Err(ProviderError::Cancelled);
        }
        let id = self.identity.lock().unwrap().clone();
        Ok((Identity { root_id: format!("root-{id}"), email: format!("{id}@example.test"), name: id.clone(), id: id.clone() }, Credentials { access_token: id.clone(), refresh_token: id }))
    }
    async fn refresh(&self, value: &Credentials) -> Result<Credentials, ProviderError> {
        if self.revoked.load(Ordering::SeqCst) { Err(ProviderError::Unauthorized) } else { Ok(value.clone()) }
    }
    async fn list(&self, value: &Credentials, folder: &str, _: Option<&str>) -> Result<Page, ProviderError> {
        assert_eq!(folder, format!("root-{}", value.access_token));
        Ok(Page { entries: vec![Entry { id: format!("file-{}", value.access_token), name: "same-name.txt".into(), mime_type: "text/plain".into(), parents: vec![folder.into()], modified_time: None, size: Some("7".into()) }], next_page: None, incomplete: false })
    }
}
async fn settled(engine: &Engine) -> Snapshot {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let snapshot = engine.command(Command::Snapshot).await.unwrap();
            if !snapshot.authorizing && !snapshot.accounts.iter().flat_map(|a| &a.listings).any(|l| l.state == Discovery::Loading) { return snapshot; }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.unwrap()
}

#[tokio::test]
async fn cancellation_revocation_and_wrong_identity_reconnect_preserve_account_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let provider = Arc::new(Google { identity: Mutex::new("alice".into()), wait_for_cancel: AtomicBool::new(true), revoked: AtomicBool::new(false) });
    let engine = Engine::open(&temp.path().join("state"), Dependencies { drive: provider.clone(), credentials: Arc::new(Secrets::default()) }).unwrap();
    engine.command(Command::Connect { reconnect: None }).await.unwrap();
    let cancelled = engine.command(Command::Cancel).await.unwrap();
    assert!(!cancelled.authorizing);
    assert!(cancelled.accounts.is_empty());
    provider.wait_for_cancel.store(false, Ordering::SeqCst);
    engine.command(Command::Connect { reconnect: None }).await.unwrap();
    settled(&engine).await;
    engine.command(Command::Activate { account_id: "alice".into(), mount_path: temp.path().join("mount"), cache_bytes: DEFAULT_CACHE_BYTES, accept_deletion: true }).await.unwrap();
    let browse = || Command::Browse { account_id: "alice".into(), folder_id: "root-alice".into(), more: false };
    engine.command(browse()).await.unwrap();
    assert_eq!(settled(&engine).await.accounts[0].listings[0].entries[0].id, "file-alice");
    provider.revoked.store(true, Ordering::SeqCst);
    engine.command(browse()).await.unwrap();
    let revoked = settled(&engine).await;
    assert!(revoked.accounts[0].needs_reconnect);
    assert!(matches!(revoked.accounts[0].listings[0].state, Discovery::Failed(_)));
    assert_eq!(revoked.accounts[0].listings[0].entries[0].id, "file-alice");
    *provider.identity.lock().unwrap() = "bob".into();
    engine.command(Command::Connect { reconnect: Some("alice".into()) }).await.unwrap();
    let rejected = settled(&engine).await;
    assert_eq!(rejected.accounts[0].identity.id, "alice");
    assert!(rejected.accounts[0].needs_reconnect);
    assert_eq!(rejected.accounts.len(), 1);
    *provider.identity.lock().unwrap() = "alice".into();
    provider.revoked.store(false, Ordering::SeqCst);
    engine.command(Command::Connect { reconnect: Some("alice".into()) }).await.unwrap();
    assert!(!settled(&engine).await.accounts[0].needs_reconnect);
    engine.command(browse()).await.unwrap();
    assert_eq!(settled(&engine).await.accounts[0].listings[0].entries[0].id, "file-alice");
}

#[tokio::test]
async fn four_accounts_keep_same_named_files_and_credentials_isolated() {
    let temp = tempfile::tempdir().unwrap();
    let provider = Arc::new(Google { identity: Mutex::new(String::new()), wait_for_cancel: AtomicBool::new(false), revoked: AtomicBool::new(false) });
    let engine = Engine::open(&temp.path().join("state"), Dependencies { drive: provider.clone(), credentials: Arc::new(Secrets::default()) }).unwrap();
    for id in ["alice", "bob", "carol", "dave"] {
        *provider.identity.lock().unwrap() = id.into();
        engine.command(Command::Connect { reconnect: None }).await.unwrap();
        settled(&engine).await;
        engine.command(Command::Activate { account_id: id.into(), mount_path: temp.path().join(id), cache_bytes: DEFAULT_CACHE_BYTES, accept_deletion: true }).await.unwrap();
        engine.command(Command::Browse { account_id: id.into(), folder_id: format!("root-{id}"), more: false }).await.unwrap();
        settled(&engine).await;
    }
    assert!(engine.command(Command::Connect { reconnect: None }).await.is_err());
    assert!(engine.command(Command::Browse { account_id: "alice".into(), folder_id: "root-bob".into(), more: false }).await.is_err());
    let snapshot = engine.command(Command::Snapshot).await.unwrap();
    for account in snapshot.accounts {
        assert_eq!(account.listings[0].entries[0].id, format!("file-{}", account.identity.id));
    }
}
