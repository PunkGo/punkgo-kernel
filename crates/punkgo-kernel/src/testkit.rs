//! PunkGo test utilities — helpers for integration and unit tests.
//!
//! - [`TestStateDir`] — creates a temporary state directory with platform-specific
//!   IPC endpoint (Unix socket or Windows named pipe), auto-cleaned on drop
//! - [`make_request`] — builds a [`RequestEnvelope`] with a random request ID

use std::path::{Path, PathBuf};

use punkgo_core::protocol::{RequestEnvelope, RequestType};
use serde_json::Value;
use uuid::Uuid;

pub struct TestStateDir {
    path: PathBuf,
}

impl TestStateDir {
    pub fn new(prefix: &str) -> std::io::Result<Self> {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir)?;
        Ok(Self { path: dir })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn ipc_endpoint(&self) -> String {
        format!("punkgo-test-{}", Uuid::new_v4())
    }
}

impl Drop for TestStateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub fn make_request(request_type: RequestType, payload: Value) -> RequestEnvelope {
    RequestEnvelope {
        request_id: Uuid::new_v4().to_string(),
        request_type,
        payload,
    }
}
