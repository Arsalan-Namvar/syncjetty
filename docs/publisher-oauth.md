# Publisher OAuth provisioning

Ticket #2 implements metadata-only My Drive browsing. It does not mount a streamed drive, transfer contents, or mutate Drive. An account marked activated has approved metadata discovery, not an active filesystem mount.

## Publisher prerequisites (not end-user setup)

1. Use a publisher-controlled Google Cloud project. Enable Drive API. Configure the application's branding, support contact, verified domains, privacy policy, and terms as required by Google.
2. Create an OAuth client of type **Desktop app**, with loopback IP redirects. Do not create a web-client secret flow or use the deprecated out-of-band redirect.
3. Request `https://www.googleapis.com/auth/drive.metadata.readonly` for this slice. Review the current [Drive scope classifications and verification requirements](https://developers.google.com/workspace/drive/api/guides/api-specific-auth). Metadata-wide access is not the narrow per-file `drive.file` scope. Complete applicable restricted-scope verification/security assessment and production publishing before distributing an unattended release. Do not represent a Testing-mode authorization as production verification.
4. Build the distributed binary with the publisher's desktop client configuration:

   ```sh
   SYNCJETTY_GOOGLE_CLIENT_ID='publisher-client-id.apps.googleusercontent.com' \
   SYNCJETTY_GOOGLE_CLIENT_SECRET='publisher-desktop-client-secret' \
   cargo build --release --locked
   ```

   These are build-time values, not user configuration files. Desktop clients are public clients: the embedded client secret is not confidential. PKCE S256, fresh state, a random loopback port, and a system browser protect the authorization flow. Never embed user refresh tokens in a build.
5. Distribute the resulting binary and user service. A source build without these values remains runnable and reports missing publisher provisioning when Connect is selected; it cannot authorize. There is no fake client ID or plaintext credential fallback.

Google documents desktop authorization in [OAuth for native applications](https://developers.google.com/identity/protocols/oauth2/native-app), and refresh-token limitations in [OAuth expiration](https://developers.google.com/identity/protocols/oauth2#expiration). External projects in Testing commonly issue seven-day refresh tokens for scopes beyond basic identity. Production status does not promise tokens never expire: users and administrators can revoke access, quotas and inactivity can invalidate tokens, and reconnect must remain available.

## End-user runtime

Install a Secret Service provider (for example GNOME Keyring or KeePassXC with Secret Service enabled), `libsecret` (`secret-tool`), and `xdg-utils`. The desktop login must provide a session bus, unlocked keyring, and `XDG_RUNTIME_DIR`. Syncjetty sends refresh tokens to Secret Service through standard input, never process arguments, SQLite, control responses, or logs. Keyring failure blocks account completion rather than falling back to plaintext.

Run `syncjetty` or `syncjetty tui`. The TUI starts the background engine if necessary. Closing the TUI leaves the engine running. Optionally install `packaging/syncjetty.service` into the user's systemd unit directory and run:

```sh
systemctl --user daemon-reload
systemctl --user enable --now syncjetty.service
```

The engine uses a private runtime directory/socket and a private state directory under `$XDG_STATE_HOME/syncjetty` (default `~/.local/state/syncjetty`). Metadata is stored in SQLite with FULL synchronous WAL commits. Credentials belong to the desktop keyring. `syncjetty status` reads the same public control/state interface as the TUI. Stop the background engine with `systemctl --user stop syncjetty.service` when systemd-managed, or SIGTERM when manually launched. This slice has no mounted filesystem to unmount.

## Activation and discovery

Authorize in the system browser, then review account identity, a dedicated empty absolute directory, exposed-storage intent, per-account evictable cache budget (default 10 GiB), and the permanent-deletion warning. The engine creates a missing dedicated directory; it never adopts populated storage. Populated paths, symlink paths, overlapping account locations, and missing acknowledgement are rejected. This slice exposes **My Drive metadata only**, with future streamed-storage intent stated separately; Shared Drives and shared-with-you browsing belong to later slices.

Browse folder children by stable Drive ID; names are labels, not identities. Pagination is explicit. Loading, partial, failed, and complete discovery are distinct from an empty successful listing. Partial pages and last-known entries persist across restart. Refresh merges known IDs without interpreting absence as deletion. Last-known metadata can therefore retain entries that have moved or disappeared; it is not a reconciled filesystem inventory or a deletion baseline. No remote delete, upload, download, or cache eviction operation exists in this slice.

## Authorized live acceptance (human-controlled prerequisite)

Use only an explicitly authorized dedicated test account/storage. Do not borrow unrelated personal/shared data or create a developer project for the end user. Record the production project status without recording client secrets, authorization codes, access tokens, or refresh tokens.

- Launch the actual TUI in the supported desktop session and select Connect. Confirm the system browser displays the publisher identity and expected read-only scope.
- Cancel in the browser and from the TUI; verify no new account activates. Repeat and approve consent.
- Review the returned account identity. Attempt activation using a populated directory and verify its exact contents remain intact. Then approve an empty dedicated directory and a chosen cache budget. Confirm the TUI says the filesystem is not mounted.
- Browse a real folder with enough entries for pagination and a nested folder. Confirm names/IDs against authorized Drive metadata, with no content downloads or remote mutations.
- Close/reopen the TUI, restart the daemon, and verify account, review settings, and known metadata remain. Confirm a subsequent browse refreshes unattended via the keyring.
- Revoke the application's access in Google account settings. Browse again: authorization failure must retain metadata and offer reconnection. Attempt reconnection using a different account; it must not replace the existing account's identity/state.
- Repeat after network interruption and interrupted enumeration. An error/incomplete state is not a successfully empty account. Do not infer deletion from absence.

A controlled adapter/PTY test does not prove Google sign-in, Secret Service behavior on the user's desktop, production verification, or Wayland/X11 browser integration. Record these checks separately; do not mark them passed without running them.
