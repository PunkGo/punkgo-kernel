use std::path::{Path, PathBuf};
use std::process::Stdio;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::process::Command;
use tokio::time::{Duration, timeout};
use uuid::Uuid;

use punkgo_core::action::Action;
use punkgo_core::errors::{KernelError, KernelResult};

use crate::backend::{ExecutionBackend, ExecutionEnvelope, SnapshotHandle};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxRunResult {
    pub run_id: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// SHA-256(stdout_bytes || stderr_bytes) — hex-encoded.
    /// Computed from the **full** output (before truncation) so the
    /// artifact hash is always verifiable against the actual execution.
    pub artifact_hash: String,
    pub timed_out: bool,
    /// True if stdout was truncated to `output_max_bytes`.
    pub stdout_truncated: bool,
    /// True if stderr was truncated to `output_max_bytes`.
    pub stderr_truncated: bool,
    pub workspace: String,
}

/// Bare OS-process execution backend.
///
/// This is the original SandboxExecutor renamed, implementing the
/// ExecutionBackend lifecycle trait. Core logic is unchanged:
/// spawn → capture → hash.
#[derive(Clone)]
pub struct ProcessBackend {
    workspace_root: PathBuf,
}

/// Backward-compatible alias.
pub type SandboxExecutor = ProcessBackend;

impl ProcessBackend {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            workspace_root: workspace_root.as_ref().to_path_buf(),
        }
    }

    /// Legacy entry point: extracts envelope from Action payload and runs.
    /// Kernel Step 2 (Kernel integration) will switch to `run_lifecycle()`.
    pub async fn execute_action(&self, action: &Action) -> KernelResult<SandboxRunResult> {
        let envelope = Self::envelope_from_action(action, &self.workspace_root, None)?;
        let handle = self.snapshot(&envelope).await?;
        self.execute(&handle, &envelope).await
    }

    /// Build an ExecutionEnvelope from an Action payload.
    /// `system_timeout_limit` caps the actor-requested timeout (committer structural duty, whitepaper §2).
    ///
    /// Two mutually exclusive modes:
    /// - `{ "command": "echo", "args": ["hi"] }` → command mode (spawn process)
    /// - `{ "content": "<base64>" }` → content mode (store blob directly)
    pub fn envelope_from_action(
        action: &Action,
        workspace_root: &Path,
        system_timeout_limit: Option<u64>,
    ) -> KernelResult<ExecutionEnvelope> {
        let payload = action.payload.as_object().ok_or_else(|| {
            KernelError::Sandbox("execute payload must be JSON object".to_string())
        })?;

        let has_command = payload.get("command").is_some();
        let has_content = payload.get("content").is_some();

        if has_command && has_content {
            return Err(KernelError::Sandbox(
                "payload.command and payload.content are mutually exclusive".to_string(),
            ));
        }
        if !has_command && !has_content {
            return Err(KernelError::Sandbox(
                "payload must have either command or content".to_string(),
            ));
        }

        let actor_timeout = payload
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(30_000);

        let timeout_ms = match system_timeout_limit {
            Some(limit) => actor_timeout.min(limit),
            None => actor_timeout,
        };

        if has_content {
            // Content mode: decode base64 content → binary
            let content_str = payload
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    KernelError::Sandbox("payload.content must be a string".to_string())
                })?;
            let content_bytes = content_str.as_bytes().to_vec();

            return Ok(ExecutionEnvelope {
                actor_id: action.actor_id.clone(),
                command: None,
                args: Vec::new(),
                content: Some(content_bytes),
                timeout_ms,
                workspace_root: workspace_root.to_path_buf(),
                output_max_bytes: 0,
                filesystem_allowlist: Vec::new(),
            });
        }

        // Command mode
        let command = payload
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| KernelError::Sandbox("payload.command is required".to_string()))?;
        if command.trim().is_empty() {
            return Err(KernelError::Sandbox(
                "payload.command cannot be empty".to_string(),
            ));
        }

        let args: Vec<String> = payload
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default();

        Ok(ExecutionEnvelope {
            actor_id: action.actor_id.clone(),
            command: Some(command.to_string()),
            args,
            content: None,
            timeout_ms,
            workspace_root: workspace_root.to_path_buf(),
            output_max_bytes: 0,
            filesystem_allowlist: Vec::new(),
        })
    }
}

#[async_trait]
impl ExecutionBackend for ProcessBackend {
    fn name(&self) -> &str {
        "process"
    }

    async fn snapshot(&self, envelope: &ExecutionEnvelope) -> KernelResult<SnapshotHandle> {
        let workspace = envelope.workspace_root.join(&envelope.actor_id);
        std::fs::create_dir_all(&workspace)?;
        Ok(SnapshotHandle {
            workspace,
            extra: None,
        })
    }

    async fn restore(
        &self,
        handle: &SnapshotHandle,
        envelope: &ExecutionEnvelope,
    ) -> KernelResult<()> {
        // Filesystem boundary: verify the resolved workspace is inside workspace_root.
        // This prevents path-traversal attacks where actor_id contains "../".
        let canonical_root = envelope
            .workspace_root
            .canonicalize()
            .map_err(|e| KernelError::Sandbox(format!("cannot resolve workspace_root: {e}")))?;
        let canonical_ws = handle
            .workspace
            .canonicalize()
            .map_err(|e| KernelError::Sandbox(format!("cannot resolve workspace: {e}")))?;
        if !canonical_ws.starts_with(&canonical_root) {
            return Err(KernelError::Sandbox(format!(
                "workspace {} escapes workspace_root {} (path traversal denied)",
                canonical_ws.display(),
                canonical_root.display()
            )));
        }
        Ok(())
    }

    async fn execute(
        &self,
        handle: &SnapshotHandle,
        envelope: &ExecutionEnvelope,
    ) -> KernelResult<SandboxRunResult> {
        let run_id = Uuid::new_v4().to_string();

        // Content mode: no process spawn, just hash the content.
        // Blob storage is handled by the caller (Kernel), not the backend.
        if let Some(ref content) = envelope.content {
            let artifact_hash = compute_artifact_hash(content, b"");
            return Ok(SandboxRunResult {
                run_id,
                exit_code: Some(0),
                stdout: String::new(),
                stderr: String::new(),
                artifact_hash,
                timed_out: false,
                stdout_truncated: false,
                stderr_truncated: false,
                workspace: handle.workspace.display().to_string(),
            });
        }

        // Command mode: spawn process.
        let command = envelope
            .command
            .as_deref()
            .ok_or_else(|| KernelError::Sandbox("command mode requires command".to_string()))?;
        let mut cmd = Command::new(command);
        cmd.args(&envelope.args)
            .current_dir(&handle.workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        cmd.env_clear();
        if let Some(path) = std::env::var_os("PATH") {
            cmd.env("PATH", path);
        }
        // Filesystem boundary: set HOME to workspace so `~` resolves inside it.
        cmd.env("HOME", &handle.workspace);
        // Windows equivalent
        cmd.env("USERPROFILE", &handle.workspace);

        let output = timeout(Duration::from_millis(envelope.timeout_ms), cmd.output()).await;

        match output {
            Ok(Ok(out)) => {
                // artifact_hash is computed from the FULL output (before truncation)
                // to preserve verifiability.
                let artifact_hash = compute_artifact_hash(&out.stdout, &out.stderr);

                // Truncate output if limit is configured.
                let (stdout_slice, stdout_truncated) =
                    truncate_output(&out.stdout, envelope.output_max_bytes);
                let (stderr_slice, stderr_truncated) =
                    truncate_output(&out.stderr, envelope.output_max_bytes);

                Ok(SandboxRunResult {
                    run_id,
                    exit_code: out.status.code(),
                    stdout: String::from_utf8_lossy(stdout_slice).to_string(),
                    stderr: String::from_utf8_lossy(stderr_slice).to_string(),
                    artifact_hash,
                    timed_out: false,
                    stdout_truncated,
                    stderr_truncated,
                    workspace: handle.workspace.display().to_string(),
                })
            }
            Ok(Err(err)) => Err(KernelError::Sandbox(format!("process spawn failed: {err}"))),
            Err(_) => Err(KernelError::Sandbox(format!(
                "sandbox run timed out after {}ms",
                envelope.timeout_ms
            ))),
        }
    }

    async fn destroy(&self, _handle: &SnapshotHandle) -> KernelResult<()> {
        // ProcessBackend: workspace preserved for audit trail.
        Ok(())
    }
}

/// Truncate output bytes to `limit`. Returns `(bytes, was_truncated)`.
/// If `limit == 0`, no truncation is applied (unlimited).
fn truncate_output(raw: &[u8], limit: u64) -> (&[u8], bool) {
    if limit == 0 || (raw.len() as u64) <= limit {
        (raw, false)
    } else {
        (&raw[..limit as usize], true)
    }
}

/// Computes SHA-256(stdout_bytes || stderr_bytes) and returns as hex.
fn compute_artifact_hash(stdout: &[u8], stderr: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(stdout);
    hasher.update(stderr);
    bytes_to_hex(&hasher.finalize())
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    const LUT: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(LUT[(b >> 4) as usize] as char);
        out.push(LUT[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use serde_json::json;

    use punkgo_core::action::{Action, ActionType};

    use super::*;
    use crate::backend::ExecutionBackend;
    use crate::orchestrator::run_lifecycle;
    use crate::registry::BackendRegistry;

    fn make_execute_action(command: &str, args: &[&str], timeout_ms: u64) -> Action {
        Action {
            actor_id: "test-actor".to_string(),
            action_type: ActionType::Execute,
            target: "workspace/test".to_string(),
            payload: json!({
                "command": command,
                "args": args,
                "timeout_ms": timeout_ms
            }),
            timestamp: None,
        }
    }

    #[test]
    fn envelope_from_action_caps_timeout_with_system_limit() {
        let action = make_execute_action("echo", &["hello"], 60_000);
        let workspace = PathBuf::from("/tmp/test");

        // Actor requests 60s, system limit 5s → effective 5s
        let envelope =
            ProcessBackend::envelope_from_action(&action, &workspace, Some(5_000)).unwrap();
        assert_eq!(envelope.timeout_ms, 5_000);

        // Actor requests 60s, no system limit → 60s
        let envelope = ProcessBackend::envelope_from_action(&action, &workspace, None).unwrap();
        assert_eq!(envelope.timeout_ms, 60_000);

        // Actor requests 2s, system limit 5s → 2s (actor is stricter)
        let action_short = make_execute_action("echo", &["hello"], 2_000);
        let envelope =
            ProcessBackend::envelope_from_action(&action_short, &workspace, Some(5_000)).unwrap();
        assert_eq!(envelope.timeout_ms, 2_000);
    }

    #[test]
    fn envelope_from_action_rejects_empty_command() {
        let action = Action {
            actor_id: "test-actor".to_string(),
            action_type: ActionType::Execute,
            target: "workspace/test".to_string(),
            payload: json!({ "command": "  ", "timeout_ms": 100 }),
            timestamp: None,
        };
        let workspace = PathBuf::from("/tmp/test");
        let result = ProcessBackend::envelope_from_action(&action, &workspace, None);
        assert!(result.is_err());
    }

    #[test]
    fn envelope_from_action_default_timeout() {
        let action = Action {
            actor_id: "test-actor".to_string(),
            action_type: ActionType::Execute,
            target: "workspace/test".to_string(),
            payload: json!({ "command": "echo" }),
            timestamp: None,
        };
        let workspace = PathBuf::from("/tmp/test");
        let envelope = ProcessBackend::envelope_from_action(&action, &workspace, None).unwrap();
        assert_eq!(envelope.timeout_ms, 30_000); // default 30s
    }

    #[test]
    fn process_backend_name() {
        let backend = ProcessBackend::new("/tmp/test");
        assert_eq!(backend.name(), "process");
    }

    #[test]
    fn backend_registry_basic_operations() {
        let mut registry = BackendRegistry::new();
        assert!(registry.default_backend().is_none());

        let backend = Arc::new(ProcessBackend::new("/tmp/test"));
        registry.register(backend);

        assert!(registry.default_backend().is_some());
        assert_eq!(registry.default_backend().unwrap().name(), "process");
        assert!(registry.get("process").is_some());
        assert!(registry.get("docker").is_none());
        assert_eq!(registry.list_backends().len(), 1);
    }

    #[tokio::test]
    async fn run_lifecycle_snapshot_creates_workspace() {
        let temp_dir = std::env::temp_dir().join("punkgo-sandbox-test-lifecycle");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let backend: Arc<dyn ExecutionBackend> = Arc::new(ProcessBackend::new(&temp_dir));

        let envelope = ExecutionEnvelope {
            actor_id: "lifecycle-test".to_string(),
            command: Some(if cfg!(windows) {
                "cmd".to_string()
            } else {
                "echo".to_string()
            }),
            args: if cfg!(windows) {
                vec!["/C".to_string(), "echo".to_string(), "hello".to_string()]
            } else {
                vec!["hello".to_string()]
            },
            content: None,
            timeout_ms: 5_000,
            workspace_root: temp_dir.clone(),
            output_max_bytes: 0,
            filesystem_allowlist: Vec::new(),
        };

        let result = run_lifecycle(&backend, &envelope).await;
        assert!(
            result.is_ok(),
            "run_lifecycle should succeed: {:?}",
            result.err()
        );

        let run = result.unwrap();
        assert!(run.stdout.contains("hello"));
        assert!(!run.artifact_hash.is_empty());

        // Workspace should exist (ProcessBackend preserves for audit)
        assert!(temp_dir.join("lifecycle-test").exists());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn truncate_output_unlimited() {
        let data = b"hello world";
        let (slice, truncated) = truncate_output(data, 0);
        assert_eq!(slice, data);
        assert!(!truncated);
    }

    #[test]
    fn truncate_output_under_limit() {
        let data = b"hello";
        let (slice, truncated) = truncate_output(data, 100);
        assert_eq!(slice, data);
        assert!(!truncated);
    }

    #[test]
    fn truncate_output_at_limit() {
        let data = b"hello";
        let (slice, truncated) = truncate_output(data, 5);
        assert_eq!(slice, data);
        assert!(!truncated);
    }

    #[test]
    fn truncate_output_over_limit() {
        let data = b"hello world, this is a long message";
        let (slice, truncated) = truncate_output(data, 5);
        assert_eq!(slice, b"hello");
        assert!(truncated);
    }

    #[tokio::test]
    async fn output_truncation_applied_when_configured() {
        let temp_dir = std::env::temp_dir().join("punkgo-sandbox-test-truncation");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let backend: Arc<dyn ExecutionBackend> = Arc::new(ProcessBackend::new(&temp_dir));

        // Generate output longer than the limit.
        // "echo AAAAAAAAAA" produces 10+ bytes of stdout.
        let envelope = ExecutionEnvelope {
            actor_id: "truncation-test".to_string(),
            command: Some(if cfg!(windows) {
                "cmd".to_string()
            } else {
                "echo".to_string()
            }),
            args: if cfg!(windows) {
                vec![
                    "/C".to_string(),
                    "echo".to_string(),
                    "AAAAAAAAAA".to_string(),
                ]
            } else {
                vec!["AAAAAAAAAA".to_string()]
            },
            content: None,
            timeout_ms: 5_000,
            workspace_root: temp_dir.clone(),
            output_max_bytes: 5, // truncate to 5 bytes
            filesystem_allowlist: Vec::new(),
        };

        let result = run_lifecycle(&backend, &envelope).await;
        assert!(result.is_ok(), "should succeed: {:?}", result.err());

        let run = result.unwrap();
        // stdout should be truncated to 5 bytes
        assert!(
            run.stdout.len() <= 5,
            "stdout should be truncated: len={}",
            run.stdout.len()
        );
        assert!(run.stdout_truncated, "stdout_truncated should be true");
        // artifact_hash should still be computed from full output (non-empty)
        assert!(!run.artifact_hash.is_empty());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn output_not_truncated_when_unlimited() {
        let temp_dir = std::env::temp_dir().join("punkgo-sandbox-test-no-truncation");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let backend: Arc<dyn ExecutionBackend> = Arc::new(ProcessBackend::new(&temp_dir));

        let envelope = ExecutionEnvelope {
            actor_id: "no-trunc-test".to_string(),
            command: Some(if cfg!(windows) {
                "cmd".to_string()
            } else {
                "echo".to_string()
            }),
            args: if cfg!(windows) {
                vec![
                    "/C".to_string(),
                    "echo".to_string(),
                    "AAAAAAAAAA".to_string(),
                ]
            } else {
                vec!["AAAAAAAAAA".to_string()]
            },
            content: None,
            timeout_ms: 5_000,
            workspace_root: temp_dir.clone(),
            output_max_bytes: 0, // unlimited
            filesystem_allowlist: Vec::new(),
        };

        let result = run_lifecycle(&backend, &envelope).await;
        assert!(result.is_ok(), "should succeed: {:?}", result.err());

        let run = result.unwrap();
        assert!(run.stdout.contains("AAAAAAAAAA"));
        assert!(!run.stdout_truncated);
        assert!(!run.stderr_truncated);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn restore_rejects_path_traversal() {
        let temp_dir = std::env::temp_dir().join("punkgo-sandbox-test-traversal");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let backend = ProcessBackend::new(&temp_dir);

        // actor_id with path traversal: "../" escapes workspace_root
        let envelope = ExecutionEnvelope {
            actor_id: "../../etc".to_string(),
            command: Some("echo".to_string()),
            args: vec![],
            content: None,
            timeout_ms: 5_000,
            workspace_root: temp_dir.clone(),
            output_max_bytes: 0,
            filesystem_allowlist: Vec::new(),
        };

        // snapshot() creates the directory (even if traversed)
        let handle = backend.snapshot(&envelope).await.unwrap();
        // restore() should detect the traversal and reject
        let result = backend.restore(&handle, &envelope).await;
        assert!(result.is_err(), "path traversal should be rejected");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("path traversal") || err_msg.contains("escapes"),
            "error should mention traversal: {err_msg}"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[tokio::test]
    async fn execute_sets_home_to_workspace() {
        let temp_dir = std::env::temp_dir().join("punkgo-sandbox-test-home");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let backend: Arc<dyn ExecutionBackend> = Arc::new(ProcessBackend::new(&temp_dir));

        // Print HOME (or USERPROFILE on Windows) to verify it's set to workspace.
        let envelope = ExecutionEnvelope {
            actor_id: "home-test".to_string(),
            command: Some(if cfg!(windows) {
                "cmd".to_string()
            } else {
                "sh".to_string()
            }),
            args: if cfg!(windows) {
                vec![
                    "/C".to_string(),
                    "echo".to_string(),
                    "%USERPROFILE%".to_string(),
                ]
            } else {
                vec!["-c".to_string(), "echo $HOME".to_string()]
            },
            content: None,
            timeout_ms: 5_000,
            workspace_root: temp_dir.clone(),
            output_max_bytes: 0,
            filesystem_allowlist: Vec::new(),
        };

        let result = run_lifecycle(&backend, &envelope).await;
        assert!(result.is_ok(), "should succeed: {:?}", result.err());

        let run = result.unwrap();
        let expected_workspace = temp_dir.join("home-test");
        assert!(
            run.stdout
                .contains(&expected_workspace.display().to_string()),
            "HOME should point to workspace: stdout='{}', expected contains '{}'",
            run.stdout.trim(),
            expected_workspace.display()
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn envelope_from_action_content_mode() {
        let action = Action {
            actor_id: "test-actor".to_string(),
            action_type: ActionType::Execute,
            target: "workspace/test".to_string(),
            payload: json!({ "content": "hello world" }),
            timestamp: None,
        };
        let workspace = PathBuf::from("/tmp/test");
        let envelope = ProcessBackend::envelope_from_action(&action, &workspace, None).unwrap();
        assert!(envelope.command.is_none());
        assert_eq!(envelope.content.as_deref(), Some(b"hello world".as_slice()));
    }

    #[test]
    fn envelope_from_action_rejects_both_command_and_content() {
        let action = Action {
            actor_id: "test-actor".to_string(),
            action_type: ActionType::Execute,
            target: "workspace/test".to_string(),
            payload: json!({ "command": "echo", "content": "hello" }),
            timestamp: None,
        };
        let workspace = PathBuf::from("/tmp/test");
        let result = ProcessBackend::envelope_from_action(&action, &workspace, None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("mutually exclusive"), "err: {err}");
    }

    #[test]
    fn envelope_from_action_rejects_neither_command_nor_content() {
        let action = Action {
            actor_id: "test-actor".to_string(),
            action_type: ActionType::Execute,
            target: "workspace/test".to_string(),
            payload: json!({ "timeout_ms": 1000 }),
            timestamp: None,
        };
        let workspace = PathBuf::from("/tmp/test");
        let result = ProcessBackend::envelope_from_action(&action, &workspace, None);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn content_mode_returns_artifact_hash() {
        let temp_dir = std::env::temp_dir().join("punkgo-sandbox-test-content-mode");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let backend: Arc<dyn ExecutionBackend> = Arc::new(ProcessBackend::new(&temp_dir));

        let content = b"hello world content mode";
        let envelope = ExecutionEnvelope {
            actor_id: "content-test".to_string(),
            command: None,
            args: Vec::new(),
            content: Some(content.to_vec()),
            timeout_ms: 5_000,
            workspace_root: temp_dir.clone(),
            output_max_bytes: 0,
            filesystem_allowlist: Vec::new(),
        };

        let result = run_lifecycle(&backend, &envelope).await;
        assert!(result.is_ok(), "should succeed: {:?}", result.err());

        let run = result.unwrap();
        assert_eq!(run.exit_code, Some(0));
        assert!(run.stdout.is_empty());
        assert!(run.stderr.is_empty());
        assert!(!run.artifact_hash.is_empty());
        assert!(!run.timed_out);

        // Verify artifact_hash is deterministic for same content
        let result2 = backend
            .execute(&backend.snapshot(&envelope).await.unwrap(), &envelope)
            .await
            .unwrap();
        assert_eq!(run.artifact_hash, result2.artifact_hash);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
