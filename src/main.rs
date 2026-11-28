use anyhow::{Result, bail};
use std::sync::Arc;
use syncjetty::{Command, Dependencies, Engine, google::{Google, SecretService}, ipc::{Client, Daemon}, tui};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("syncjetty: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "tui".into());
    if args.next().is_some() { bail!("Usage: syncjetty [tui|daemon|status|quit]"); }
    match mode.as_str() {
        "daemon" => {
            let daemon = Daemon::acquire()?;
            let dependencies = Dependencies {
                drive: Arc::new(Google::new()?), credentials: Arc::new(SecretService),
            };
            let engine = Engine::open(daemon.state_path(), dependencies)?;
            daemon.serve(engine).await
        }
        "tui" => tui::run(Client::new()?).await,
        "status" => {
            let snapshot = Client::new()?.request(Command::Snapshot).await?;
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
            Ok(())
        }
        "quit" => bail!("Daemon shutdown via the control socket is not supported. Closing the TUI with q leaves it running. Stop the user service with `systemctl --user stop syncjetty.service` (or send SIGTERM to the daemon). This slice has no mounted filesystem."),
        "--help" | "-h" => {
            println!("Usage: syncjetty [tui|daemon|status|quit]\n\n  tui     Keyboard interface (default); auto-starts the daemon\n  daemon  Run background metadata engine in the foreground\n  status  Print a running daemon's snapshot as JSON\n  quit    Explain safe shutdown; remote shutdown is unsupported\n\nRequires a private XDG_RUNTIME_DIR. State: $XDG_STATE_HOME/syncjetty\nor $HOME/.local/state/syncjetty. Metadata only: no mount or file transfer.");
            Ok(())
        }
        _ => bail!("Unknown command {mode:?}. Usage: syncjetty [tui|daemon|status|quit]"),
    }
}
