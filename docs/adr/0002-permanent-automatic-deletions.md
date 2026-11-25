# Permanently propagate confirmed deletions without a batch-size approval threshold

After approval of initial reconciliation, Syncjetty will automatically propagate confirmed, nonconflicting deletions in both directions, using permanent deletion rather than Drive Trash or a local recovery area. The user selected this policy over recoverable deletion and explicitly declined an additional mass-deletion approval threshold; this prioritizes automation and leaves Syncjetty without a recovery copy, including for large batches and shared content.

This policy does not convert uncertainty into deletion: unavailable roots, lost access, and uncertain remote disappearance pause synchronization for the affected storage scope while preserving files. Delete-versus-edit remains a pending conflict requiring an explicit choice, and operations lacking permanent-delete permission remain pending with an explanation rather than substituting trashing or removing access.

The later streaming-only decision in [ADR-0003](0003-streaming-only-desktop-product.md) replaces mirrored folder pairs, not this permanent-deletion policy. The original initial-reconciliation preview requirement must be redefined for streamed-drive onboarding; cache eviction is not deletion and never authorizes deleting remote content.
