# Own reconciliation and integrate directly with Google Drive

Syncjetty will implement reconciliation and Google Drive integration in Rust rather than depend on rclone for its first release. Stock bisync mutates conflicting files before a user's decision and automatically resolves deletion-versus-edit cases, which violates Syncjetty's explicit pending-conflict contract; an application-owned reconciler with rclone only as transport was considered, but direct integration was selected to retain control over durable Drive change tracking and the user experience.

This decision accepts responsibility for provider integration, OAuth, reconciliation safety, and recovery. Both initial and incremental speed matter, but no native-versus-rclone performance advantage has been measured or promised; representative benchmarking is required. Supporting other providers later remains a product goal, not a first-release deliverable.

Evidence: [rclone suitability research](../research/rclone-sync-fit.md) and [native Drive API research](../research/drive-native-fit.md).
