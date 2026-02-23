use serde::{Deserialize, Serialize};
use serde_json::Value;

/// IPC request envelope — the top-level structure sent from CLI to kernel daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub request_id: String,
    #[serde(rename = "type")]
    pub request_type: RequestType,
    #[serde(default)]
    pub payload: Value,
}

/// The three request types supported by the kernel.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RequestType {
    Quote,
    Submit,
    Read,
}

/// IPC response envelope — returned from kernel daemon to CLI client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub request_id: String,
    pub status: String,
    pub payload: Value,
}

impl ResponseEnvelope {
    pub fn ok(request_id: String, payload: Value) -> Self {
        Self {
            request_id,
            status: "ok".to_string(),
            payload,
        }
    }

    /// Create an error response from a `KernelError` with structured payload.
    pub fn err_structured(request_id: String, error: &crate::errors::KernelError) -> Self {
        Self {
            request_id,
            status: "error".to_string(),
            payload: error.to_error_payload(),
        }
    }

    /// Create an error response from a plain string (for non-KernelError cases
    /// like JSON parse failures at the IPC layer).
    pub fn err(request_id: String, message: String) -> Self {
        Self {
            request_id,
            status: "error".to_string(),
            payload: serde_json::json!({
                "error_type": "Unknown",
                "message": message,
            }),
        }
    }
}
