use std::sync::Arc;

use punkgo_core::errors::KernelResult;

use crate::backend::{ExecutionBackend, ExecutionEnvelope, SnapshotHandle};
use crate::executor::SandboxRunResult;

/// Runs the full snapshot → restore → execute → destroy lifecycle.
///
/// `destroy()` is always called regardless of earlier failures,
/// mirroring a `finally` block — the committer must clean up.
pub async fn run_lifecycle(
    backend: &Arc<dyn ExecutionBackend>,
    envelope: &ExecutionEnvelope,
) -> KernelResult<SandboxRunResult> {
    let handle = backend.snapshot(envelope).await?;

    if let Err(err) = backend.restore(&handle, envelope).await {
        let _ = backend.destroy(&handle).await;
        return Err(err);
    }

    let result = backend.execute(&handle, envelope).await;

    // Always destroy, even if execute failed.
    let _ = backend.destroy(&handle).await;

    result
}

/// Runs destroy only — used when the Kernel needs to force-kill
/// an execution that has already been snapshotted.
pub async fn force_destroy(
    backend: &Arc<dyn ExecutionBackend>,
    handle: &SnapshotHandle,
) -> KernelResult<()> {
    backend.destroy(handle).await
}
