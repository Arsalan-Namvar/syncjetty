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

Tests cross the public engine control/state seam using isolated external adapters. They do not replace live Google, Secret Service, or desktop acceptance. See [verification evidence](docs/verification-ticket-2.md) for observed checks and remaining prerequisites.