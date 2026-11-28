# Syncjetty

Native Rust Google Drive client for Arch Linux. The current slice provides browser-based account setup and paginated **My Drive metadata browsing** through a keyboard-first TUI and a persistent background engine. It does **not** mount a filesystem or upload, download, or delete file contents.

```sh
cargo build --locked
cargo run -- tui
```

Packaged builds require publisher-owned Google desktop OAuth configuration. A source build without it launches normally but explains that authorization is unavailable. End users are not asked to create Google projects or edit configuration. See [publisher provisioning, runtime requirements, and authorized live acceptance](docs/publisher-oauth.md).

The TUI shows keyboard shortcuts, account identity, activation review, loading/error state, and pagination. Closing it leaves the engine running. `syncjetty status` reports the shared engine state; `syncjetty daemon` starts the engine directly. An optional [systemd user service](packaging/syncjetty.service) supports background startup.

```sh
cargo check --all-targets --locked
cargo test --locked
```

The [Rust CI workflow](.github/workflows/rust.yml) runs these exact commands on Ubuntu 24.04 with Rust 1.100.0 for pull requests targeting `main` and pushes to `main`. Its stable merge-check name is `Rust checks`; either command failing fails that job. The job has a 15-minute timeout and read-only repository permissions.

Deterministic engine, IPC, authorization, and Python 3 PTY tests use isolated external adapters without publisher OAuth values, Google access, a real desktop session, or the user's Secret Service. No secrets or live consent are needed for CI. These tests do not replace opt-in live Google, Secret Service, or desktop acceptance: follow [authorized live acceptance](docs/publisher-oauth.md) only with explicit authorization. See [verification evidence](docs/verification-ticket-2.md) for observed live checks and remaining prerequisites.