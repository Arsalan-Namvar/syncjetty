use syncjetty::{Command, Discovery, Engine, Snapshot};

pub async fn settled(engine: &Engine) -> Snapshot {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let snapshot = engine.command(Command::Snapshot).await.unwrap();
            if !snapshot.authorizing && !snapshot.accounts.iter().flat_map(|a| &a.listings).any(|l| l.state == Discovery::Loading) {
                return snapshot;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }).await.unwrap()
}
