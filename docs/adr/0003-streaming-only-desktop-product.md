# Ship streaming-only storage with TUI and desktop controls

The first release will provide on-demand streamed storage rather than mirrored folder pairs, superseding the earlier mirrored-folder design and single-account restriction. The user chose broader Google Drive for Windows-style functionality and then explicitly selected streaming only, with up to four simultaneous Google accounts, offline pinning and edits, filename search across accounts including cloud-only entries, and support for both shared folders and Workspace Shared Drives.

The product includes a Rust/Linux TUI, a tray menu, and a graphical settings window; the latter two were explicitly selected together rather than limiting desktop integration to notifications and opening files. Google Docs/Sheets/Slides appear as browser-opening entries, not exported local documents or a Syncjetty-native offline editor. File-content search remains outside the confirmed search scope.

Locally modified contents remain pending uploads during outages and cannot be evicted before upload. Native reconciliation and explicit conflict resolution remain required, as does the permanent-deletion policy in [ADR-0002](0002-permanent-automatic-deletions.md); cache eviction must never propagate as deletion.

Each account exposes My Drive, Shared Drives, and a separate shared-with-you folders section. Read-only content remains browsable, readable, and pinnable, with unsupported writes rejected explicitly. Drive shortcuts appear as links to targets with broken or inaccessible targets visible; deleting a shortcut never deletes its target.

An account activation review replaces the original mirrored-folder merge preview: show the account, dedicated empty mount location, exposed storage, cache settings, and permanent-deletion warning before activation. Initial discovery does not upload, delete, or bulk-download file contents. Both the TUI and graphical window support account setup, settings, pinning, and conflict resolution; the tray provides status and quick actions.

The configurable default evictable cache is 10 GiB per account, reclaiming least-recently-used clean, unpinned content. Pinned contents and pending uploads are protected outside that budget with disk usage shown separately; low disk space blocks further downloads or writes visibly instead of discarding protected data. Unpinning permits eviction and does not delete remote content.

Pausing an account stops background uploads, remote checks, and pin downloads but permits explicit on-demand opens; a separate Work offline setting prevents all network access for that account. Folder pinning includes future ordinary-file descendants, reports progress and exceptions, and marks supported accessible contents available offline only after downloads complete. Native Google documents are browser entries, not offline-editable cached documents.

Shortcut targets are accessed only through the shortcut's account, without silently switching connected accounts. Pinning a shortcut requests reachable-target availability with cycle detection and explicit inaccessible-target errors. Google Photos backup, Office/Outlook integration, file-content search, and a built-in Google-document editor are outside the first release; desktop notifications and default-application opening are included.
