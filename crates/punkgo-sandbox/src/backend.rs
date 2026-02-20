use std::path::PathBuf;

use async_trait::async_trait;

use punkgo_core::errors::KernelResult;

use crate::executor::SandboxRunResult;

/// Opaque handle returned by `snapshot()`, passed through the lifecycle.
/// For ProcessBackend this is just a workspace path;
/// for a future VM backend it could be a VM ID + snapshot reference.
#[derive(Debug)]
pub struct SnapshotHandle {
    pub workspace: PathBuf,
    /// Backend-specific opaque data (unused by ProcessBackend).
    pub extra: Option<String>,
}

/// Parameters for a single execution, built from Action payload + system limits.
/// The Kernel constructs this by taking `min(actor_request, system_hard_limit)`
/// for every numeric field — the committer's structural duty (whitepaper §2).
///
/// Two mutually exclusive modes:
/// - **Command mode**: `command` is Some → spawn process → capture output → blob
/// - **Content mode**: `content` is Some → store content as blob directly (no spawn)
#[derive(Debug, Clone)]
pub struct ExecutionEnvelope {
    pub actor_id: String,
    /// Command to spawn (None in content mode).
    pub command: Option<String>,
    pub args: Vec<String>,
    /// Raw binary content to store directly (None in command mode).
    /// Binary-safe: supports text, images, video, audio, etc.
    pub content: Option<Vec<u8>>,
    /// Effective timeout = min(actor payload timeout, system hard limit).
    pub timeout_ms: u64,
    pub workspace_root: PathBuf,

    // --- Resource limits (Phase 2) ---
    // All default to 0 = unlimited (opt-in enforcement via config).
    /// Max bytes for stdout and stderr (each). 0 = unlimited.
    pub output_max_bytes: u64,
    /// Extra filesystem paths beyond workspace. Empty = workspace only.
    pub filesystem_allowlist: Vec<PathBuf>,
}

/// Lifecycle protocol for execution backends.
///
/// Inspired by E2B / Firecracker's snapshot → restore → execute → destroy
/// pattern, but the trait itself is technology-neutral.
///
/// - ProcessBackend: bare OS process (current implementation)
/// - Future: DockerBackend, FirecrackerBackend, etc.
#[async_trait]
pub trait ExecutionBackend: Send + Sync {
    /// Human-readable name (e.g. "process", "docker", "firecracker").
    fn name(&self) -> &str;

    /// Phase 1 — Prepare the execution environment.
    /// ProcessBackend: create workspace directory.
    /// VM backend: restore a snapshot.
    async fn snapshot(&self, envelope: &ExecutionEnvelope) -> KernelResult<SnapshotHandle>;

    /// Phase 2 — Configure the environment before execution.
    /// ProcessBackend: no-op.
    /// VM backend: configure networking, mount volumes.
    async fn restore(
        &self,
        handle: &SnapshotHandle,
        envelope: &ExecutionEnvelope,
    ) -> KernelResult<()>;

    /// Phase 3 — Run the command and capture output.
    async fn execute(
        &self,
        handle: &SnapshotHandle,
        envelope: &ExecutionEnvelope,
    ) -> KernelResult<SandboxRunResult>;

    /// Phase 4 — Cleanup (ALWAYS called, even if earlier phases failed).
    /// ProcessBackend: no-op (workspace preserved for audit).
    /// VM backend: destroy the VM.
    async fn destroy(&self, handle: &SnapshotHandle) -> KernelResult<()>;
}
