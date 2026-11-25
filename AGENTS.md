## Agent skills

### Issue tracker

Track issues and specs in GitHub Issues for Arsalan-Namvar/syncjetty. Before reading or changing tickets, read `docs/agents/issue-tracker.md`.

### Triage labels

Use the five default triage labels. Before classifying or labeling tickets, read `docs/agents/triage-labels.md`.

### Domain docs

Use a single-context layout. Before exploring domain code or proposing domain changes, read `docs/agents/domain.md`.

### Ticket branches

Before implementing a ticket, create a dedicated branch from `main` named `ticket/<issue-number>-<short-slug>`, or switch to that ticket's existing branch when resuming. Keep each ticket's changes and commits on its own branch; never implement tickets directly on `main` or reuse another ticket's branch. This applies to `/implement` and orchestrated ticket work; merge completed ticket branches through the integration workflow rather than committing multiple tickets on one branch.
