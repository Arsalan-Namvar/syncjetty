use crate::{Account, Command, DELETION_WARNING, Discovery, Snapshot, ipc::Client};
use anyhow::{Context, Result, bail};
use crossterm::{
    cursor::{Hide, MoveTo, Show}, event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute, queue, terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::{collections::{HashMap, HashSet}, io::{self, IsTerminal, Write}, path::PathBuf, time::{Duration, Instant}};
use tokio::sync::mpsc;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

struct Terminal;
impl Terminal {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let guard = Self;
        execute!(io::stdout(), EnterAlternateScreen, Hide)?;
        Ok(guard)
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

#[derive(Clone, Copy)]
enum Field { Mount, Cache }
struct Review {
    account_id: String,
    mount: String,
    cache_gib: String,
    editing: Option<Field>,
    submitting: bool,
}
struct Reply { activation: bool, result: Result<Snapshot> }

struct View {
    snapshot: Snapshot,
    account: usize,
    selected: usize,
    folders: HashMap<String, Vec<(String, String)>>,
    requested: HashSet<(String, String)>,
    review: Option<Review>,
    error: Option<String>,
    review_visible: bool,
}

impl View {
    fn account(&self) -> Option<&Account> { self.snapshot.accounts.get(self.account) }
    fn folder(&self) -> Option<(&str, &str)> {
        let account = self.account()?;
        self.folders.get(&account.identity.id).and_then(|stack| stack.last())
            .map(|(id, name)| (id.as_str(), name.as_str()))
            .or(Some((account.identity.root_id.as_str(), "My Drive")))
    }
    fn review(&mut self) {
        let Some(account) = self.account() else { self.review = None; return; };
        if account.activated { self.review = None; return; }
        if self.review.as_ref().is_some_and(|review| review.account_id == account.identity.id) { return; }
        self.review = Some(Review { account_id: account.identity.id.clone(),
            mount: account.mount_path.to_string_lossy().into_owned(),
            cache_gib: (account.cache_bytes / (1024 * 1024 * 1024)).to_string(), editing: None, submitting: false });
    }
    fn send(&mut self, sender: &mpsc::Sender<Command>, command: Command) -> bool {
        match sender.try_send(command) {
            Ok(()) => { self.error = None; true }
            Err(_) => { self.error = Some("Control request queue is busy; try again shortly".into()); false }
        }
    }
    fn browse(&mut self, sender: &mpsc::Sender<Command>, more: bool) {
        let Some(account) = self.account().filter(|account| account.activated) else { return; };
        let account_id = account.identity.id.clone();
        let folder_id = self.folder().unwrap().0.to_owned();
        if more && !account.listings.iter().any(|listing| listing.folder_id == folder_id && listing.next_page.is_some()) {
            self.error = Some("No next page is available".into()); return;
        }
        if self.send(sender, Command::Browse { account_id: account_id.clone(), folder_id: folder_id.clone(), more }) {
            self.requested.insert((account_id, folder_id));
        }
    }
    fn key(&mut self, key: KeyEvent, sender: &mpsc::Sender<Command>) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') { return false; }
        if let Some(review) = self.review.as_mut().filter(|review| review.editing.is_some()) {
            let field = review.editing.unwrap();
            let text = match field { Field::Mount => &mut review.mount, Field::Cache => &mut review.cache_gib };
            match key.code {
                KeyCode::Esc | KeyCode::Enter => review.editing = None,
                KeyCode::Backspace => { text.pop(); }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => text.clear(),
                KeyCode::Char(character) if !character.is_control() && text.len() < 4096 => {
                    if matches!(field, Field::Mount) || character.is_ascii_digit() { text.push(character); }
                }
                _ => {}
            }
            return true;
        }
        match key.code {
            KeyCode::Char('q') => return false,
            KeyCode::Char('c') => { self.send(sender, Command::Connect { reconnect: None }); }
            KeyCode::Esc => { self.send(sender, Command::Cancel); }
            KeyCode::Char('r') => {
                if let Some(account) = self.account() {
                    self.send(sender, Command::Connect { reconnect: Some(account.identity.id.clone()) });
                }
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let count = self.snapshot.accounts.len();
                if count > 0 {
                    self.account = if key.code == KeyCode::BackTab { (self.account + count - 1) % count } else { (self.account + 1) % count };
                    self.selected = 0; self.review = None; self.review();
                }
            }
            KeyCode::Char('m') => {
                if let Some(review) = self.review.as_mut().filter(|review| !review.submitting) { review.editing = Some(Field::Mount); }
            }
            KeyCode::Char('b') => {
                if let Some(review) = self.review.as_mut().filter(|review| !review.submitting) { review.editing = Some(Field::Cache); }
            }
            KeyCode::Char('y') => {
                if let Some(review) = self.review.as_ref().filter(|review| !review.submitting) {
                    if !self.review_visible {
                        self.error = Some("Enlarge the terminal to read the complete activation review before approval".into());
                        return true;
                    }
                    let cache = review.cache_gib.parse::<u64>().ok().and_then(|value| value.checked_mul(1024 * 1024 * 1024));
                    let Some(cache_bytes) = cache.filter(|value| *value > 0) else {
                        self.error = Some("Cache budget must be a positive whole number of GiB".into()); return true;
                    };
                    let command = Command::Activate { account_id: review.account_id.clone(), mount_path: PathBuf::from(&review.mount), cache_bytes, accept_deletion: true };
                    if self.send(sender, command) { self.review.as_mut().unwrap().submitting = true; }
                }
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => {
                if let Some(account) = self.account() {
                    let count = self.folder().and_then(|(id, _)| account.listings.iter().find(|listing| listing.folder_id == id)).map_or(0, |listing| listing.entries.len());
                    self.selected = (self.selected + 1).min(count.saturating_sub(1));
                }
            }
            KeyCode::Enter => {
                if let Some(account) = self.account().filter(|account| account.activated) {
                    let entry = self.folder().and_then(|(id, _)| account.listings.iter().find(|listing| listing.folder_id == id))
                        .and_then(|listing| listing.entries.get(self.selected)).filter(|entry| entry.is_folder());
                    if let Some(entry) = entry {
                        let account_id = account.identity.id.clone();
                        let root = account.identity.root_id.clone();
                        let entry = (entry.id.clone(), entry.name.clone());
                        self.folders.entry(account_id).or_insert_with(|| vec![(root, "My Drive".into())]).push(entry);
                        self.selected = 0;
                        self.browse(sender, false);
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(account) = self.account() {
                    let id = account.identity.id.clone();
                    if let Some(stack) = self.folders.get_mut(&id).filter(|stack| stack.len() > 1) { stack.pop(); }
                    self.selected = 0;
                }
            }
            KeyCode::Char('n') => self.browse(sender, true),
            KeyCode::Char('f') => self.browse(sender, false),
            _ => {}
        }
        true
    }
}

fn safe(text: &str) -> String {
    text.chars().map(|character| {
        if character.is_control() || character.width().is_none() || matches!(character, '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}') { ' ' } else { character }
    }).collect()
}
fn wrap(lines: &mut Vec<String>, text: &str, width: usize) {
    let text = safe(text);
    let mut line = String::new();
    let mut count = 0;
    for character in text.chars() {
        let columns = character.width().unwrap_or(0);
        if columns > 0 && count + columns > width && !line.is_empty() {
            lines.push(std::mem::take(&mut line)); count = 0;
        }
        line.push(character); count += columns;
    }
    if !line.is_empty() { lines.push(line); }
}

fn clipped_end(text: &str, width: usize) -> usize {
    let mut columns = 0;
    let mut end = 0;
    for (offset, character) in text.char_indices() {
        columns += character.width().unwrap_or(0);
        if columns > width { break; }
        end = offset + character.len_utf8();
    }
    end
}

fn draw(view: &mut View) -> Result<()> {
    let (width, height) = terminal::size()?;
    let width = usize::from(width.saturating_sub(1));
    let height = usize::from(height);
    let mut lines = Vec::new();
    for heading in ["Syncjetty | My Drive metadata only | no mounted filesystem",
        "c connect | Esc cancel | r reconnect | Tab account | q close interface",
        "Up/Down select | Enter folder | Backspace parent | n next page | f retry/refresh"] {
        wrap(&mut lines, heading, width);
    }
    view.review_visible = false;
    if view.snapshot.authorizing { wrap(&mut lines, "Authorizing: finish sign-in in your browser. Esc cancels.", width); }
    if let Some(notice) = &view.snapshot.notice { wrap(&mut lines, &format!("Status: {notice}"), width); }
    if let Some(error) = &view.error { wrap(&mut lines, &format!("Error: {error}"), width); }
    if let Some(account) = view.account() {
        wrap(&mut lines, &format!("Account {}/{}: {} ({})", view.account + 1, view.snapshot.accounts.len(), account.identity.email, account.identity.name), width);
        wrap(&mut lines, &format!("Stable account ID: {}", account.identity.id), width);
        if account.needs_reconnect { wrap(&mut lines, "Authorization expired or revoked. Press r to reconnect this account.", width); }
        if let Some(review) = &view.review {
            wrap(&mut lines, "PENDING ACTIVATION - explicit approval required", width);
            wrap(&mut lines, "Exposed storage: My Drive metadata only. Future intent: streamed access; nothing is mounted or downloaded now.", width);
            wrap(&mut lines, DELETION_WARNING, width);
            wrap(&mut lines, &format!("Mount location (future, dedicated empty directory): {}", review.mount), width);
            wrap(&mut lines, &format!("Evictable cache budget: {} GiB (default 10 GiB)", review.cache_gib), width);
            wrap(&mut lines, "m edit mount path | b edit cache GiB | y explicitly approve warning and activate metadata", width);
            if review.submitting { wrap(&mut lines, "Submitting approval; activation is not accepted until the daemon succeeds.", width); }
            if let Some(field) = review.editing {
                wrap(&mut lines, match field { Field::Mount => "Editing mount path: Ctrl-U clears; Backspace deletes; type path; Enter/Esc finishes.", Field::Cache => "Editing cache GiB: Ctrl-U clears; Backspace deletes; type digits; Enter/Esc finishes." }, width);
            }
            view.review_visible = width > 0 && lines.len() <= height && lines.iter().all(|line|
                UnicodeWidthStr::width(line.as_str()) <= width && clipped_end(line, width) == line.len());
            if !view.review_visible && height > 0 {
                lines.truncate(height.saturating_sub(1));
                lines.push("Enlarge terminal to read complete review; approval is disabled.".into());
            }
        } else if let Some((folder, name)) = view.folder() {
            wrap(&mut lines, &format!("Configured mount location (future): {}", account.mount_path.display()), width);
            wrap(&mut lines, &format!("Configured evictable cache budget: {} bytes", account.cache_bytes), width);
            let hierarchy = view.folders.get(&account.identity.id)
                .map(|stack| stack.iter().map(|(_, name)| name.as_str()).collect::<Vec<_>>().join(" / "))
                .unwrap_or_else(|| name.to_owned());
            wrap(&mut lines, &format!("Folder: {hierarchy} | stable ID: {folder}"), width);
            let listing = account.listings.iter().find(|listing| listing.folder_id == folder);
            match listing {
                None => lines.push("Not discovered yet; f starts discovery.".into()),
                Some(listing) => {
                    let status = match &listing.state {
                        Discovery::NotStarted => "Not discovered; f starts discovery".to_owned(),
                        Discovery::Loading => "Loading metadata (previous entries may remain visible)".to_owned(),
                        Discovery::Partial if listing.next_page.is_none() => "Incomplete discovery - more entries may exist; f retries discovery".to_owned(),
                        Discovery::Partial => "Partial discovery - more entries may exist; n next page, f retry".to_owned(),
                        Discovery::Complete if listing.entries.is_empty() => "Complete - this folder is empty".to_owned(),
                        Discovery::Complete => "Complete discovery".to_owned(),
                        Discovery::Failed(error) => format!("Discovery failed: {error}. Previous entries are retained; f retries"),
                    };
                    wrap(&mut lines, &status, width);
                    lines.push(format!("{} entries | next page: {}", listing.entries.len(), if listing.next_page.is_some() { "available (n)" } else { "none" }));
                    wrap(&mut lines, &format!("Last successful page check (Unix seconds): {}", listing.last_success.map_or_else(|| "never".into(), |time| time.to_string())), width);
                    if let Some(entry) = listing.entries.get(view.selected.min(listing.entries.len().saturating_sub(1))) {
                        wrap(&mut lines, &format!("Selected ID: {} | type: {} | size: {} | modified: {}", entry.id, entry.mime_type, entry.size.as_deref().unwrap_or("n/a"), entry.modified_time.as_deref().unwrap_or("unknown")), width);
                    }
                    let visible = height.saturating_sub(lines.len() + 1).max(1);
                    let selected = view.selected.min(listing.entries.len().saturating_sub(1));
                    let start = selected.saturating_sub(visible.saturating_sub(1));
                    for (index, entry) in listing.entries.iter().enumerate().skip(start).take(visible) {
                        lines.push(format!("{} {} {}", if index == selected { ">" } else { " " }, if entry.is_folder() { "[folder]" } else { "[file]" }, entry.name));
                    }
                    if listing.entries.len() > visible { lines.push(format!("Scroll with Up/Down ({}/{})", selected + 1, listing.entries.len())); }
                }
            }
        }
    } else {
        wrap(&mut lines, "No connected accounts. Press c to connect Google Drive (requires a provisioned Google OAuth client and desktop Secret Service).", width);
    }
    let mut stdout = io::stdout().lock();
    queue!(stdout, MoveTo(0, 0), Clear(ClearType::All))?;
    for (row, line) in lines.iter().take(height).enumerate() {
        queue!(stdout, MoveTo(0, row as u16))?;
        let line = safe(line);
        let end = clipped_end(&line, width);
        stdout.write_all(line[..end].as_bytes())?;
    }
    stdout.flush()?;
    Ok(())
}

/// Runs the actual keyboard terminal; dropping it leaves the daemon alive.
pub async fn run(client: Client) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() { bail!("The TUI requires a terminal; use `syncjetty status` for JSON output"); }
    let snapshot = client.ensure_running().await?;
    let _terminal = Terminal::enter().context("Enter terminal raw mode")?;
    let (commands, mut requests) = mpsc::channel::<Command>(16);
    let (replies, mut responses) = mpsc::channel::<Reply>(16);
    let worker = tokio::spawn(async move {
        let mut poll = tokio::time::interval(Duration::from_millis(250));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            let command = tokio::select! {
                biased;
                command = requests.recv() => match command { Some(command) => command, None => break },
                _ = poll.tick() => Command::Snapshot,
            };
            let activation = matches!(command, Command::Activate { .. });
            let result = client.request(command).await;
            if replies.send(Reply { activation, result }).await.is_err() { break; }
        }
    });
    let mut view = View { snapshot, account: 0, selected: 0, folders: HashMap::new(), requested: HashSet::new(), review: None, error: None, review_visible: false };
    let result = async {
        let mut last_draw = Instant::now() - Duration::from_secs(1);
        loop {
            while let Ok(reply) = responses.try_recv() {
                if reply.activation {
                    if let Some(review) = view.review.as_mut() { review.submitting = false; }
                }
                match reply.result {
                    Ok(snapshot) => { view.snapshot = snapshot; view.account = view.account.min(view.snapshot.accounts.len().saturating_sub(1)); }
                    Err(error) => view.error = Some(format!("{error:#}")),
                }
            }
            view.review();
            if let Some(account) = view.account().filter(|account| account.activated) {
                let folder = view.folder().unwrap().0.to_owned();
                if !view.requested.contains(&(account.identity.id.clone(), folder.clone()))
                    && !account.listings.iter().any(|listing| listing.folder_id == folder && !matches!(listing.state, Discovery::NotStarted)) {
                    view.browse(&commands, false);
                }
            }
            if last_draw.elapsed() >= Duration::from_millis(75) { draw(&mut view)?; last_draw = Instant::now(); }
            while event::poll(Duration::ZERO)? {
                match event::read()? {
                    Event::Key(key) if key.kind != KeyEventKind::Release => {
                        if !view.key(key, &commands) { return Ok(()); }
                        draw(&mut view)?; last_draw = Instant::now();
                    }
                    Event::Resize(_, _) => { draw(&mut view)?; last_draw = Instant::now(); }
                    _ => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }.await;
    worker.abort();
    result
}
