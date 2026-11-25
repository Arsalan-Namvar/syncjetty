# Issue tracker: GitHub

Issues and specs live in GitHub Issues for `Arsalan-Namvar/syncjetty`. Use the `gh` CLI from this clone.

## Target repository

Before the first issue creation of a session, run `gh repo view --json nameWithOwner`. If the result is not `Arsalan-Namvar/syncjetty`, stop and resolve the target with the user before writing.

## Issue operations

- Create: `gh issue create --title "..." --body-file -`, supplying a heredoc for multiline bodies.
- Read: `gh issue view <number> --json number,title,body,labels,comments`.
- List: `gh issue list --state open --json number,title,body,labels`; add label and state filters as needed.
- Comment: `gh issue comment <number> --body "..."`.
- Label: `gh issue edit <number> --add-label "..."` or `--remove-label "..."`. Read `docs/agents/triage-labels.md` for role mappings.
- Close: post the explanation with `gh issue comment`, then run `gh issue close`. This also preserves explanations for already-closed tickets.

When a skill says to publish to the issue tracker, create a GitHub issue. When it says to fetch a ticket, read the corresponding issue.

## Parent issues and blockers

Use GitHub native sub-issues and issue dependencies when available:

- Link a child: `gh api --method POST repos/Arsalan-Namvar/syncjetty/issues/<parent-number>/sub_issues -F sub_issue_id=<child-database-id>`.
- Add a blocker: `gh api --method POST repos/Arsalan-Namvar/syncjetty/issues/<child-number>/dependencies/blocked_by -F issue_id=<blocker-database-id>`.
- Resolve an issue database ID: `gh api repos/Arsalan-Namvar/syncjetty/issues/<number> --jq .id`. Use this numeric ID, not the issue number or node ID, in relationship requests.
- Count open blockers: `gh api repos/Arsalan-Namvar/syncjetty/issues/<number> --jq .issue_dependencies_summary.blocked_by`.

If native relationships are unavailable, put `Part of #<parent>` and `Blocked by: #<number>` at the top of child bodies, and maintain a child task list in the parent. A ticket is startable only when all blockers are closed.

## Pull requests as a triage surface

**PRs as a request surface: no.**

## Wayfinding

For `/wayfinder`, the map is a GitHub issue labeled `wayfinder:map`. Link its child tickets as sub-issues, or use the parent task-list fallback above. Label children `wayfinder:research`, `wayfinder:prototype`, `wayfinder:grilling`, or `wayfinder:task` as appropriate.

Choose the first open, unassigned child in map order with no open blocker. Claim it with `gh issue edit <number> --add-assignee @me` as the session's first write. Resolve it by commenting with the answer, closing it, and appending a gist and link to the map's Decisions-so-far section.
