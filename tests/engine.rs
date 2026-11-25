mod support;
use support::settled as settle;
use async_trait::async_trait;
use std::{collections::HashMap, sync::{Arc, Mutex}};
use syncjetty::*;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Vault(Mutex<HashMap<String, Credentials>>);
impl CredentialStore for Vault {
    fn load(&self, account: &str) -> anyhow::Result<Credentials> {
        self.0.lock().unwrap().get(account).cloned().ok_or_else(|| anyhow::anyhow!("locked or absent"))
    }
    fn save(&self, account: &str, credentials: &Credentials) -> anyhow::Result<()> {
        self.0.lock().unwrap().insert(account.to_owned(), credentials.clone());
        Ok(())
    }
}
struct Provider {
    second_page_fails: Mutex<bool>,
    empty: Mutex<bool>,
    incomplete: Mutex<bool>,
}
#[async_trait]
impl Drive for Provider {
    async fn start_changes(&self, _: &Credentials) -> Result<String, ProviderError> { Ok("baseline".into()) }
    async fn changes(&self, _: &Credentials, _: &str) -> Result<ChangesPage, ProviderError> {
        Ok(ChangesPage { changes: vec![], next_page: None, new_start: Some("caught-up".into()) })
    }
    async fn authorize(&self, _: CancellationToken) -> Result<(Identity, Credentials), ProviderError> {
        Ok((Identity { id: "alice-id".into(), email: "alice@example.test".into(), name: "Alice".into(), root_id: "root-alice".into() }, Credentials { access_token: "access-secret".into(), refresh_token: "refresh-secret".into() }))
    }
    async fn refresh(&self, credentials: &Credentials) -> Result<Credentials, ProviderError> {
        Ok(credentials.clone())
    }
    async fn list(&self, _: &Credentials, folder: &str, page: Option<&str>) -> Result<Page, ProviderError> {
        assert_eq!(folder, "root-alice");
        if *self.empty.lock().unwrap() {
            return Ok(Page { entries: vec![], next_page: None, incomplete: false });
        }
        if page.is_some() && *self.second_page_fails.lock().unwrap() {
            return Err(ProviderError::Unavailable);
        }
        Ok(Page {
            entries: vec![Entry { id: if page.is_none() { "file-one" } else { "file-two" }.into(), name: if page.is_none() { "budget.txt" } else { "notes.txt" }.into(), mime_type: "text/plain".into(), parents: vec![folder.into()], modified_time: None, size: Some("12".into()) }],
            next_page: if page.is_none() { Some("page-two".into()) } else { None },
            incomplete: page.is_none() && *self.incomplete.lock().unwrap(),
        })
    }
}

#[tokio::test]
async fn activation_and_paginated_metadata_survive_failed_discovery_and_restart() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("state.sqlite");
    let mount = temp.path().join("drive");
    std::fs::create_dir(&mount).unwrap();
    let provider = Arc::new(Provider { second_page_fails: Mutex::new(true), empty: Mutex::new(false), incomplete: Mutex::new(false) });
    let vault = Arc::new(Vault::default());
    let open = || Engine::open(&database, Dependencies { drive: provider.clone(), credentials: vault.clone() }).unwrap();
    let engine = open();
    engine.command(Command::Connect { reconnect: None }).await.unwrap();
    let pending = settle(&engine).await;
    assert_eq!(pending.accounts[0].identity.email, "alice@example.test");
    assert!(!pending.accounts[0].activated);
    let activate = || Command::Activate { account_id: "alice-id".into(), mount_path: mount.clone(), cache_bytes: DEFAULT_CACHE_BYTES, accept_deletion: true };
    std::fs::write(mount.join("keep.txt"), b"user contents").unwrap();
    assert!(engine.command(activate()).await.is_err());
    assert_eq!(std::fs::read(mount.join("keep.txt")).unwrap(), b"user contents");
    std::fs::remove_file(mount.join("keep.txt")).unwrap();
    engine.command(activate()).await.unwrap();
    engine.command(Command::Browse { account_id: "alice-id".into(), folder_id: "root-alice".into(), more: false }).await.unwrap();
    let first = settle(&engine).await;
    assert_eq!(first.accounts[0].listings[0].state, Discovery::Partial);
    assert_eq!(first.accounts[0].listings[0].entries[0].name, "budget.txt");
    engine.command(Command::Browse { account_id: "alice-id".into(), folder_id: "root-alice".into(), more: true }).await.unwrap();
    let failed = settle(&engine).await;
    assert!(matches!(failed.accounts[0].listings[0].state, Discovery::Failed(_)));
    assert_eq!(failed.accounts[0].listings[0].next_page.as_deref(), Some("page-two"));
    drop(engine);
    let restarted = open();
    let retained = restarted.command(Command::Snapshot).await.unwrap();
    assert_eq!(retained.accounts[0].listings[0].entries[0].id, "file-one");
    assert_eq!(retained.accounts[0].cache_bytes, DEFAULT_CACHE_BYTES);
    *provider.second_page_fails.lock().unwrap() = false;
    restarted.command(Command::Browse { account_id: "alice-id".into(), folder_id: "root-alice".into(), more: true }).await.unwrap();
    let complete = settle(&restarted).await;
    assert_eq!(complete.accounts[0].listings[0].state, Discovery::Complete);
    assert_eq!(complete.accounts[0].listings[0].entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["budget.txt", "notes.txt"]);
    *provider.empty.lock().unwrap() = true;
    restarted.command(Command::Browse { account_id: "alice-id".into(), folder_id: "root-alice".into(), more: false }).await.unwrap();
    let uncertain_absence = settle(&restarted).await;
    assert_eq!(uncertain_absence.accounts[0].listings[0].entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["file-one", "file-two"]);
    assert!(std::fs::read_dir(&mount).unwrap().next().is_none());
    assert!(!serde_json::to_string(&complete).unwrap().contains("secret"));
}

#[tokio::test]
async fn incomplete_first_page_stays_partial_after_restart_and_final_page() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("state");
    let provider = Arc::new(Provider { second_page_fails: Mutex::new(false), empty: Mutex::new(false), incomplete: Mutex::new(true) });
    let vault = Arc::new(Vault::default());
    let open = || Engine::open(&database, Dependencies { drive: provider.clone(), credentials: vault.clone() }).unwrap();
    let engine = open();
    engine.command(Command::Connect { reconnect: None }).await.unwrap();
    settle(&engine).await;
    engine.command(Command::Activate { account_id: "alice-id".into(), mount_path: temp.path().join("drive"), cache_bytes: DEFAULT_CACHE_BYTES, accept_deletion: true }).await.unwrap();
    engine.command(Command::Browse { account_id: "alice-id".into(), folder_id: "root-alice".into(), more: false }).await.unwrap();
    assert!(settle(&engine).await.accounts[0].listings[0].incomplete);
    drop(engine);
    let engine = open();
    engine.command(Command::Browse { account_id: "alice-id".into(), folder_id: "root-alice".into(), more: true }).await.unwrap();
    let final_page = settle(&engine).await;
    assert_eq!(final_page.accounts[0].listings[0].state, Discovery::Partial);
    assert!(final_page.accounts[0].listings[0].next_page.is_none());
    *provider.incomplete.lock().unwrap() = false;
    engine.command(Command::Browse { account_id: "alice-id".into(), folder_id: "root-alice".into(), more: false }).await.unwrap();
    settle(&engine).await;
    engine.command(Command::Browse { account_id: "alice-id".into(), folder_id: "root-alice".into(), more: true }).await.unwrap();
    assert_eq!(settle(&engine).await.accounts[0].listings[0].state, Discovery::Complete);
}

struct MovingDrive {
    listed: std::sync::atomic::AtomicBool,
    fail_replay: std::sync::atomic::AtomicBool,
}
#[async_trait]
impl Drive for MovingDrive {
    async fn authorize(&self, _: CancellationToken) -> Result<(Identity, Credentials), ProviderError> {
        Ok((Identity { id: "moving".into(), email: "moving@example.test".into(), name: "Moving".into(), root_id: "root".into() }, Credentials { access_token: "access".into(), refresh_token: "refresh".into() }))
    }
    async fn refresh(&self, value: &Credentials) -> Result<Credentials, ProviderError> { Ok(value.clone()) }
    async fn start_changes(&self, _: &Credentials) -> Result<String, ProviderError> {
        // A cursor captured after listing cannot see the intervening arrival.
        Ok(if self.listed.load(std::sync::atomic::Ordering::SeqCst) { "too-late" } else { "before-enumeration" }.into())
    }
    async fn list(&self, _: &Credentials, _: &str, page: Option<&str>) -> Result<Page, ProviderError> {
        self.listed.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(Page { entries: vec![], next_page: page.is_none().then(|| "last-page".into()), incomplete: false })
    }
    async fn changes(&self, _: &Credentials, cursor: &str) -> Result<ChangesPage, ProviderError> {
        if self.fail_replay.load(std::sync::atomic::Ordering::SeqCst) { return Err(ProviderError::Unavailable); }
        let changes = match cursor {
            "before-enumeration" => vec![MetadataChange { id: "arrival".into(), entry: Some(Entry { id: "arrival".into(), name: "arrived-during-enumeration.txt".into(), mime_type: "text/plain".into(), parents: vec!["root".into()], modified_time: None, size: None }), removed: false }],
            "replay-page-2" => vec![MetadataChange { id: "arrival".into(), entry: Some(Entry { id: "arrival".into(), name: "renamed-during-replay.txt".into(), mime_type: "text/plain".into(), parents: vec!["root".into()], modified_time: None, size: None }), removed: false }],
            _ => vec![],
        };
        Ok(ChangesPage { changes, next_page: (cursor == "before-enumeration").then(|| "replay-page-2".into()), new_start: (cursor != "before-enumeration").then(|| "caught-up".into()) })
    }
}

#[tokio::test]
async fn paginated_discovery_replays_intervening_changes_and_recovers_after_failure() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("state");
    let provider = Arc::new(MovingDrive { listed: std::sync::atomic::AtomicBool::new(false), fail_replay: std::sync::atomic::AtomicBool::new(true) });
    let vault = Arc::new(Vault::default());
    let open = || Engine::open(&database, Dependencies { drive: provider.clone(), credentials: vault.clone() }).unwrap();
    let engine = open();
    engine.command(Command::Connect { reconnect: None }).await.unwrap(); settle(&engine).await;
    engine.command(Command::Activate { account_id: "moving".into(), mount_path: temp.path().join("mount"), cache_bytes: DEFAULT_CACHE_BYTES, accept_deletion: true }).await.unwrap();
    let browse = |more| Command::Browse { account_id: "moving".into(), folder_id: "root".into(), more };
    engine.command(browse(false)).await.unwrap();
    assert_eq!(settle(&engine).await.accounts[0].listings[0].state, Discovery::Partial);
    drop(engine);
    let engine = open();
    engine.command(browse(true)).await.unwrap();
    let failed = settle(&engine).await;
    assert!(matches!(failed.accounts[0].listings[0].state, Discovery::Failed(_)), "Unreplayed concurrent changes must not be reported as a complete empty folder");
    assert!(failed.accounts[0].listings[0].replay_pending);
    drop(engine);
    provider.fail_replay.store(false, std::sync::atomic::Ordering::SeqCst);
    let engine = open();
    engine.command(browse(true)).await.unwrap();
    let complete = settle(&engine).await;
    assert_eq!(complete.accounts[0].listings[0].state, Discovery::Complete);
    assert_eq!(complete.accounts[0].listings[0].entries[0].name, "renamed-during-replay.txt");
    assert_eq!(complete.accounts[0].listings[0].change_cursor.as_deref(), Some("caught-up"));
}
