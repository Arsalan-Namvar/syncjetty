use async_trait::async_trait;
use std::{sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}}, process::Command as Process};
use syncjetty::*;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Keyring(Mutex<Option<Credentials>>);
impl CredentialStore for Keyring {
    fn load(&self, _: &str) -> anyhow::Result<Credentials> { self.0.lock().unwrap().clone().ok_or_else(|| anyhow::anyhow!("locked")) }
    fn save(&self, _: &str, value: &Credentials) -> anyhow::Result<()> { *self.0.lock().unwrap() = Some(value.clone()); Ok(()) }
}
struct DriveFixture(AtomicUsize);
#[async_trait]
impl Drive for DriveFixture {
    async fn authorize(&self, cancel: CancellationToken) -> Result<(Identity, Credentials), ProviderError> {
        match self.0.fetch_add(1, Ordering::SeqCst) {
            0 => { cancel.cancelled().await; Err(ProviderError::Cancelled) },
            1 => Err(ProviderError::Unavailable),
            _ => Ok((Identity { id: "alice".into(), email: "alice@example.test".into(), name: "名前-é-Alice".into(), root_id: "my-root".into() }, Credentials { access_token: "access".into(), refresh_token: "refresh".into() })),
        }
    }
    async fn refresh(&self, value: &Credentials) -> Result<Credentials, ProviderError> { Ok(value.clone()) }
    async fn list(&self, _: &Credentials, folder: &str, page: Option<&str>) -> Result<Page, ProviderError> {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        let (id, name, mime, next) = if folder == "folder" { ("nested", "nested.txt", "text/plain", None) }
            else if page.is_some() { ("second", "second-page.txt", "text/plain", None) }
            else { ("folder", "Documents", "application/vnd.google-apps.folder", Some("page-2".into())) };
        Ok(Page { entries: vec![Entry { id: id.into(), name: name.into(), mime_type: mime.into(), parents: vec![folder.into()], size: None, modified_time: None }], next_page: next, incomplete: folder == "my-root" && page.is_none() })
    }
}

#[tokio::test]
async fn keyboard_onboarding_browsing_and_restart_on_real_terminal() {
    if std::env::var_os("SYNCJETTY_TEST_TUI_CHILD").is_some() {
        let daemon = ipc::Daemon::acquire().unwrap();
        let engine = Engine::open(daemon.state_path(), Dependencies { drive: Arc::new(DriveFixture(AtomicUsize::new(0))), credentials: Arc::new(Keyring::default()) }).unwrap();
        let serve = tokio::spawn(daemon.serve(engine));
        tui::run(ipc::Client::new().unwrap()).await.unwrap();
        // Closing the interface must leave controls usable until daemon shutdown.
        assert!(!ipc::Client::new().unwrap().request(Command::Snapshot).await.unwrap().authorizing);
        serve.abort();
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let output = Process::new("python").arg("tests/support/tui.py")
        .arg(std::env::current_exe().unwrap()).arg(temp.path())
        .env("SYNCJETTY_TEST_TUI_CHILD", "1").output().unwrap();
    assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert_eq!(std::fs::read(temp.path().join("populated/keep.txt")).unwrap(), b"untouched user contents");
}
