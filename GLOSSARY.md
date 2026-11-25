# Syncjetty

Syncjetty provides streamed access to cloud storage with bidirectional synchronization and offline availability.

## Language

**Conflict**:
Incompatible changes to corresponding local and remote items that require a user's choice before either version is overwritten or deleted. This includes an edit on one side and a deletion on the other.

**Streamed drive**:
A local filesystem view of a connected Google account's cloud files, including cloud-only entries whose contents are downloaded on demand.

**Pending conflict**:
A conflict awaiting an explicit user choice, retained across restarts while unrelated files remain eligible for synchronization.

**Conflict resolution**:
An explicit user choice of which conflicting state to preserve: the local state, the remote state, or both versions when two versions survive. For a deletion versus an edit, the choices are to delete both or preserve the edited file on both sides.

**Pinned item**:
A file or folder selected for continued offline availability. Pinning a folder includes its ordinary-file descendants.

**Pending upload**:
A local change that has not yet been successfully synchronized to cloud storage.

**Cache eviction**:
Removal of a downloaded local copy while retaining its cloud entry and contents. It is not a synchronized deletion and cannot remove pinned contents or the only copy of pending uploads.
