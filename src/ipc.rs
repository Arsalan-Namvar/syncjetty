use crate::{Command, Engine, Snapshot};
use anyhow::{Context, Result, bail, ensure};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    env, fs::{self, File, OpenOptions},
    os::unix::{fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt}, process::CommandExt},
    path::{Path, PathBuf}, process::Stdio, time::Duration,
};
use tokio::{io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader}, net::{UnixListener, UnixStream}, time::{sleep, timeout}};

const MAX_FRAME: usize = 4 * 1024 * 1024;
const RESPONSE_CHUNK: usize = 64 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Serialize, Deserialize)]
enum Response {
    Ok(Snapshot),
    Error(String),
}

fn uid() -> Result<u32> {
    Ok(fs::metadata("/proc/self").context("Cannot determine current Linux user")?.uid())
}

fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("Create private directory {}", path.display()))?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(metadata.is_dir() && !metadata.file_type().is_symlink(), "{} must be a real directory", path.display());
    ensure!(metadata.uid() == uid()?, "{} must belong to the current user", path.display());
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn runtime_dir() -> Result<PathBuf> {
    let root = env::var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty())
        .context("XDG_RUNTIME_DIR is unset. Start Syncjetty in a user login session with a private runtime directory (normally /run/user/$UID). No /tmp fallback is used.")?;
    let root = PathBuf::from(root);
    ensure!(root.is_absolute(), "XDG_RUNTIME_DIR must be an absolute path");
    let metadata = fs::symlink_metadata(&root).context("XDG_RUNTIME_DIR is unavailable; start a user login session")?;
    ensure!(metadata.is_dir() && metadata.uid() == uid()? && metadata.mode() & 0o077 == 0,
        "XDG_RUNTIME_DIR must be a directory owned by this user with permissions 0700");
    let directory = root.join("syncjetty");
    private_dir(&directory)?;
    Ok(directory)
}

fn state_dir() -> Result<PathBuf> {
    let base = match env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(env::var_os("HOME").context("Set HOME or XDG_STATE_HOME to store Syncjetty state")?).join(".local/state"),
    };
    ensure!(base.is_absolute(), "State directory must be an absolute path");
    let directory = base.join("syncjetty");
    private_dir(&directory)?;
    Ok(directory)
}

fn lock(path: &Path) -> Result<File> {
    // Linux O_NOFOLLOW prevents a pre-existing lock symlink from redirecting writes.
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .mode(0o600).custom_flags(0x20000).open(path)?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file() && metadata.uid() == uid()?, "Lock file must be owned by this user");
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.try_lock_exclusive().with_context(|| format!("Syncjetty daemon already owns {}", path.display()))?;
    Ok(file)
}

async fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>> {
    let mut reader = BufReader::new(stream);
    let mut frame = Vec::new();
    // fill_buf/consume bounds allocation even if a peer never sends a newline.
    timeout(IO_TIMEOUT, async {
        loop {
            let bytes = reader.fill_buf().await?;
            ensure!(!bytes.is_empty(), "Control connection closed before a complete JSON line");
            let count = bytes.iter().position(|byte| *byte == b'\n').map_or(bytes.len(), |index| index + 1);
            ensure!(frame.len() + count <= MAX_FRAME, "Control JSON line exceeds {} bytes", MAX_FRAME);
            let complete = bytes[count - 1] == b'\n';
            frame.extend_from_slice(&bytes[..count]);
            reader.consume(count);
            if complete { return Ok::<_, anyhow::Error>(()); }
        }
    }).await.context("Timed out reading daemon control request")??;
    Ok(frame)
}

async fn write_frame<T: Serialize>(stream: &mut UnixStream, value: &T) -> Result<()> {
    let mut frame = serde_json::to_vec(value)?;
    ensure!(frame.len() < MAX_FRAME, "Control JSON line exceeds {} bytes", MAX_FRAME);
    frame.push(b'\n');
    timeout(IO_TIMEOUT, stream.write_all(&frame)).await.context("Timed out writing daemon control request")??;
    Ok(())
}

// Commands remain bounded newline-delimited JSON. Responses are JSON split into
// chunks, each prefixed by a big-endian u32 byte length, ending in a zero length.
// There is no total response cap: a full inventory may exceed MAX_FRAME. Both
// peers use this protocol exclusively, including error responses.
async fn read_response(stream: &mut UnixStream) -> Result<Response> {
    let mut frame = Vec::new();
    loop {
        let length = timeout(IO_TIMEOUT, stream.read_u32()).await
            .context("Timed out reading daemon response chunk length")?? as usize;
        ensure!(length <= RESPONSE_CHUNK, "Daemon response chunk exceeds {} bytes", RESPONSE_CHUNK);
        if length == 0 { break; }
        let start = frame.len();
        frame.try_reserve(length).context("Cannot allocate daemon response chunk")?;
        frame.resize(start + length, 0);
        // Read exactly the payload so no buffered read-ahead loses the next header.
        timeout(IO_TIMEOUT, stream.read_exact(&mut frame[start..])).await
            .context("Timed out reading daemon response chunk")??;
    }
    Ok(serde_json::from_slice(&frame)?)
}

async fn write_response(stream: &mut UnixStream, response: &Response) -> Result<()> {
    let frame = serde_json::to_vec(response)?;
    for chunk in frame.chunks(RESPONSE_CHUNK) {
        timeout(IO_TIMEOUT, async {
            stream.write_u32(chunk.len() as u32).await?;
            stream.write_all(chunk).await
        }).await.context("Timed out writing daemon response chunk")??;
    }
    timeout(IO_TIMEOUT, stream.write_u32(0)).await
        .context("Timed out ending daemon response")??;
    Ok(())
}

/// One bounded JSON request per same-user Unix connection.
#[derive(Clone)]
pub struct Client {
    socket: PathBuf,
}

impl Client {
    pub fn new() -> Result<Self> {
        Ok(Self::at(runtime_dir()?.join("control.sock")))
    }

    /// Explicit socket location for callers that own an isolated runtime directory.
    pub fn at(socket: PathBuf) -> Self { Self { socket } }

    pub async fn request(&self, command: Command) -> Result<Snapshot> {
        let mut stream = timeout(IO_TIMEOUT, UnixStream::connect(&self.socket)).await
            .context("Timed out connecting to Syncjetty daemon")?
            .with_context(|| format!("Connect to daemon at {}", self.socket.display()))?;
        ensure!(stream.peer_cred()?.uid() == uid()?, "Refusing a daemon owned by another user");
        write_frame(&mut stream, &command).await?;
        match read_response(&mut stream).await? {
            Response::Ok(snapshot) => Ok(snapshot),
            Response::Error(error) => bail!("{error}"),
        }
    }

    pub async fn ensure_running(&self) -> Result<Snapshot> {
        if let Ok(snapshot) = self.request(Command::Snapshot).await { return Ok(snapshot); }
        let mut child = std::process::Command::new(env::current_exe()?)
            .arg("daemon").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .process_group(0).spawn().context("Start background Syncjetty daemon")?;
        let mut last_error = None;
        for _ in 0..50 {
            sleep(Duration::from_millis(100)).await;
            match self.request(Command::Snapshot).await {
                Ok(snapshot) => {
                    // Reap the child later; dropping the TUI must not terminate the daemon.
                    std::thread::spawn(move || { let _ = child.wait(); });
                    return Ok(snapshot);
                }
                Err(error) => last_error = Some(error),
            }
            if let Some(status) = child.try_wait()? {
                // A competing TUI may have won the daemon lock.
                if let Ok(snapshot) = self.request(Command::Snapshot).await { return Ok(snapshot); }
                bail!("Daemon exited with {status}. Run `syncjetty daemon` in a terminal to see the setup error. {}", last_error.as_ref().unwrap());
            }
        }
        std::thread::spawn(move || { let _ = child.wait(); });
        bail!("Daemon did not become ready. Run `syncjetty daemon` to inspect the error: {}", last_error.unwrap());
    }
}

/// Owns both the socket lock and the SQLite-state lock for its entire lifetime.
pub struct Daemon {
    listener: UnixListener,
    socket: PathBuf,
    socket_identity: (u64, u64),
    state_path: PathBuf,
    _runtime_lock: File,
    _state_lock: File,
}

impl Daemon {
    pub fn acquire() -> Result<Self> {
        let runtime = runtime_dir()?;
        let state = state_dir()?;
        let runtime_lock = lock(&runtime.join("daemon.lock"))?;
        let state_lock = lock(&state.join("engine.lock"))?;
        let socket = runtime.join("control.sock");
        match fs::symlink_metadata(&socket) {
            Ok(metadata) => {
                ensure!(metadata.file_type().is_socket() && metadata.uid() == uid()?, "Refusing to remove a non-owned or non-socket control path");
                // Only the owner of both locks can recover a socket after a crash.
                fs::remove_file(&socket)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let listener = UnixListener::bind(&socket)?;
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
        let metadata = fs::symlink_metadata(&socket)?;
        Ok(Self { listener, socket, socket_identity: (metadata.dev(), metadata.ino()),
            state_path: state.join("metadata.sqlite3"), _runtime_lock: runtime_lock, _state_lock: state_lock })
    }

    pub fn state_path(&self) -> &Path { &self.state_path }

    pub async fn serve(self, engine: Engine) -> Result<()> {
        let capacity = std::sync::Arc::new(tokio::sync::Semaphore::new(32));
        let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => break,
                _ = terminate.recv() => break,
                connection = self.listener.accept() => {
                    let (mut stream, _) = connection?;
                    if stream.peer_cred()?.uid() != uid()? { continue; }
                    let Ok(permit) = capacity.clone().try_acquire_owned() else { continue; };
                    let engine = engine.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let result: Result<Snapshot> = async {
                            let command: Command = serde_json::from_slice(&read_frame(&mut stream).await?)?;
                            timeout(IO_TIMEOUT, engine.command(command)).await.context("Daemon command timed out")?
                        }.await;
                        let response = match result {
                            Ok(snapshot) => Response::Ok(snapshot),
                            Err(error) => Response::Error(format!("{error:#}")),
                        };
                        // A failed write may have sent a partial chunk; close rather
                        // than append another response to the incomplete message.
                        let _ = write_response(&mut stream, &response).await;
                    });
                }
            }
        }
        Ok(())
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.socket) {
            if (metadata.dev(), metadata.ino()) == self.socket_identity {
                let _ = fs::remove_file(&self.socket);
            }
        }
    }
}
