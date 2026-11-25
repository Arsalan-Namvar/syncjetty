# Ticket #2 verification

Branch: `ticket/2-google-tui`. Platform: Arch Linux x86-64; Rust 1.100.0. This is metadata-only slice 1: no FUSE implementation, content transfer, reconciliation, or remote mutations. No mounted-filesystem check applies yet; no active mount is claimed.

## Observed development checks

- `cargo check --all-targets --locked`: passed after integration.
- `cargo test --test engine --test authorization --locked`: passed (3 tests). Observed activation rejection preserving exact local bytes, paginated discovery failure and restart retention, no deletion from absence, cancellation, revoked authorization, rejected wrong-identity reconnection, and four-account credential isolation.
- `cargo test --test tui --locked -- --nocapture`: passed. A Python standard-library PTY driver launched the actual Rust TUI over the actual Unix socket/SQLite engine with an isolated external Drive adapter. Keyboard actions exercised cancellation, provider error, populated-directory rejection, activation warning/approval, configurable 2 GiB cache, loading, pagination, nested navigation, close, and restart retention. This is not a mocked renderer or source-text assertion.
- `cargo build --locked` and `target/debug/syncjetty --help`: passed; runnable CLI exposed TUI, daemon, status, and an explanation that remote quit is unsupported.
- Production binary smoke in isolated temporary XDG runtime/state directories: launched `target/debug/syncjetty daemon`, opened `target/debug/syncjetty tui` in a PTY, pressed `c`, observed missing publisher provisioning guidance, pressed `q`, then successfully ran `target/debug/syncjetty status`. A second daemon was rejected by the exclusive lock. SIGTERM stopped the owned daemon and removed its socket.
- Secret Service smoke: created a disposable HOME/data/config/cache and session bus using `dbus-run-session`, unlocked an isolated `gnome-keyring-daemon --unlock --components=secrets`, and ran a temporary Rust example calling production `SecretService.save/load`. Observed an exact refresh-token round trip and an empty loaded access token. No real Google tokens or existing user keyring were used. The example and temporary keyring were removed afterward. This proves isolated Secret Service integration, not unattended authorization with Google.

Initial dependency acquisition exceeded a 300-second command deadline. The next build exposed a Rust temporary-borrow error in the OAuth callback host; binding the host string fixed it. Subsequent checks passed; these initial attempts are not counted as successful verification.

## Review corrections

Two independent read-only review axes inspected standards/security and ticket conformance. Findings addressed:

- Full snapshots above 4 MiB could disable all controls: bounded command frames now have independent chunked response transport; regression crosses the public IPC/engine seam with 15,000 long-named entries, restart, and a subsequent control command.
- Earlier `incompleteSearch` could be lost on the final page: enumeration-level uncertainty is durable until a fresh successful enumeration; regression covers continuation across restart, and actual TUI coverage verifies incomplete status rather than successful emptiness.
- Unicode activation text could be silently byte-clipped: wrapping/clipping/approval now use terminal display columns consistently; PTY coverage uses non-ASCII identity and mount text.

## Not verified / release prerequisites

No publisher Google OAuth client or explicitly authorized live test account/storage was supplied. **Live Google browser sign-in, real provider identity/listing, production consent verification, real revocation, and unattended Google refresh remain unrun.** This ticket is not a production-sign-in completion claim. Follow [publisher provisioning and authorized live acceptance](publisher-oauth.md).

Actual Wayland/X11 browser launching, desktop-window rendering in both sessions, packaged systemd startup, and AUR packaging/publication were not verified. PTY operation is actual terminal behavior, not desktop-session coverage. No live writes/deletes were attempted or authorized. Later filesystem and desktop slices remain separate from this implementation.


Final verification: `cargo check --all-targets --locked && cargo test --locked` passed: **6 tests**, including the 15,000-entry IPC regression, durable incomplete-page regression, and actual PTY TUI with Unicode account/mount review and incomplete discovery. An intermediate TUI run overlapped fixture edits and failed on an incorrectly ordered incomplete-state expectation; the assertion now follows the final-page action and the final full suite passed. The final suite included all library, binary, integration, and doc-test targets.