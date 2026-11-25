use async_trait::async_trait;
use std::{sync::Arc, process::Command as Process};
use syncjetty::*;
use tokio_util::sync::CancellationToken;
struct Provider;
#[async_trait]
impl Drive for Provider {
    async fn authorize(&self, _: CancellationToken) -> Result<(Identity, Credentials), ProviderError> {
        Ok((Identity { id: "account".into(), email: "account@example.test".into(), name: "Account".into(), root_id: "root".into() }, Credentials { access_token: "access".into(), refresh_token: "refresh".into() }))
    }
    async fn refresh(&self, value: &Credentials) -> Result<Credentials, ProviderError> { Ok(value.clone()) }
    async fn list(&self, _: &Credentials, _: &str, page: Option<&str>) -> Result<Page, ProviderError> {
        let page: usize = page.unwrap_or("0").parse().unwrap();
        Ok(Page { entries: (0..1000).map(|index| Entry { id: format!("{page}-{index}"), name: format!("{page}-{index}-{}", "x".repeat(240)), mime_type: "text/plain".into(), parents: vec!["root".into()], modified_time: None, size: None }).collect(), next_page: (page < 14).then(|| (page + 1).to_string()), incomplete: false })
    }
}
struct Vault;
impl CredentialStore for Vault {
    fn load(&self, _: &str) -> anyhow::Result<Credentials> { Ok(Credentials { access_token: "access".into(), refresh_token: "refresh".into() }) }
    fn save(&self, _: &str, _: &Credentials) -> anyhow::Result<()> { Ok(()) }
}
async fn settle(client: &ipc::Client) -> Snapshot {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let state = client.request(Command::Snapshot).await.unwrap();
            if !state.authorizing && !state.accounts.iter().flat_map(|a| &a.listings).any(|l| l.state == Discovery::Loading) { return state; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.unwrap()
}
#[tokio::test]
async fn large_paginated_inventory_remains_controllable_after_restart() {
    if std::env::var_os("SYNCJETTY_IPC_CHILD").is_none() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        std::fs::create_dir(&runtime).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
        let output = Process::new(std::env::current_exe().unwrap()).args(["--exact", "large_paginated_inventory_remains_controllable_after_restart", "--nocapture"])
            .env("SYNCJETTY_IPC_CHILD", "1").env("XDG_RUNTIME_DIR", runtime).env("XDG_STATE_HOME", temp.path().join("state")).env("SYNCJETTY_MOUNT", temp.path().join("mount")).output().unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    let deps = || Dependencies { drive: Arc::new(Provider), credentials: Arc::new(Vault) };
    let daemon = ipc::Daemon::acquire().unwrap();
    let path = daemon.state_path().to_owned();
    let engine = Engine::open(&path, deps()).unwrap();
    let task = tokio::spawn(daemon.serve(engine));
    let client = ipc::Client::new().unwrap();
    client.request(Command::Connect { reconnect: None }).await.unwrap(); settle(&client).await;
    client.request(Command::Activate { account_id: "account".into(), mount_path: std::env::var_os("SYNCJETTY_MOUNT").unwrap().into(), cache_bytes: DEFAULT_CACHE_BYTES, accept_deletion: true }).await.unwrap();
    for page in 0..15 {
        client.request(Command::Browse { account_id: "account".into(), folder_id: "root".into(), more: page > 0 }).await.unwrap(); settle(&client).await;
    }
    let snapshot = client.request(Command::Snapshot).await.unwrap();
    assert!(serde_json::to_vec(&snapshot).unwrap().len() > 4 * 1024 * 1024);
    assert_eq!(snapshot.accounts[0].listings[0].entries.last().unwrap().id, "14-999");
    task.abort(); let _ = task.await;
    let daemon = ipc::Daemon::acquire().unwrap();
    let engine = Engine::open(&path, deps()).unwrap();
    let task = tokio::spawn(daemon.serve(engine));
    let retained = client.request(Command::Cancel).await.unwrap();
    assert_eq!(retained.accounts[0].listings[0].entries.last().unwrap().name, format!("14-999-{}", "x".repeat(240)));
    assert_eq!(retained.accounts[0].listings[0].state, Discovery::Complete);
    task.abort(); let _ = task.await;
}
