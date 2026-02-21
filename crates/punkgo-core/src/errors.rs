use serde_json::Value;
use thiserror::Error;

/// Unified error type for all kernel operations.
#[derive(Debug, Error)]
pub enum KernelError {
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("policy violation: {0}")]
    PolicyViolation(String),
    #[error("actor not found: {0}")]
    ActorNotFound(String),
    #[error("insufficient energy: actor={actor_id}, required={required}, available={available}")]
    InsufficientEnergy {
        actor_id: String,
        required: i64,
        available: i64,
    },
    #[error("actor frozen: {0}")]
    ActorFrozen(String),
    #[error("boundary violation: {0}")]
    BoundaryViolation(String),
    #[error("authorization required: {0}")]
    AuthorizationRequired(String),
    #[error("hold triggered: hold_id={hold_id}, agent={agent_id}")]
    HoldTriggered { hold_id: String, agent_id: String },
    #[error("execute payload invalid: {0}")]
    ExecutePayloadInvalid(String),
    #[error("audit error: {0}")]
    Audit(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl KernelError {
    /// Structured error payload for IPC responses.
    ///
    /// Returns a JSON object with `error_type` (machine-readable variant name),
    /// `message` (human-readable, backward-compatible), and structured fields.
    pub fn to_error_payload(&self) -> Value {
        match self {
            KernelError::InvalidRequest(detail) => serde_json::json!({
                "error_type": "InvalidRequest",
                "message": self.to_string(),
                "detail": detail,
            }),
            KernelError::PolicyViolation(detail) => serde_json::json!({
                "error_type": "PolicyViolation",
                "message": self.to_string(),
                "detail": detail,
            }),
            KernelError::ActorNotFound(actor_id) => serde_json::json!({
                "error_type": "ActorNotFound",
                "message": self.to_string(),
                "actor_id": actor_id,
            }),
            KernelError::InsufficientEnergy {
                actor_id,
                required,
                available,
            } => serde_json::json!({
                "error_type": "InsufficientEnergy",
                "message": self.to_string(),
                "actor_id": actor_id,
                "required": required,
                "available": available,
            }),
            KernelError::ActorFrozen(actor_id) => serde_json::json!({
                "error_type": "ActorFrozen",
                "message": self.to_string(),
                "actor_id": actor_id,
            }),
            KernelError::BoundaryViolation(detail) => serde_json::json!({
                "error_type": "BoundaryViolation",
                "message": self.to_string(),
                "detail": detail,
            }),
            KernelError::AuthorizationRequired(detail) => serde_json::json!({
                "error_type": "AuthorizationRequired",
                "message": self.to_string(),
                "detail": detail,
            }),
            KernelError::HoldTriggered { hold_id, agent_id } => serde_json::json!({
                "error_type": "HoldTriggered",
                "message": self.to_string(),
                "hold_id": hold_id,
                "agent_id": agent_id,
            }),
            KernelError::ExecutePayloadInvalid(detail) => serde_json::json!({
                "error_type": "ExecutePayloadInvalid",
                "message": self.to_string(),
                "detail": detail,
            }),
            KernelError::Audit(detail) => serde_json::json!({
                "error_type": "Audit",
                "message": self.to_string(),
                "detail": detail,
            }),
            KernelError::Io(err) => serde_json::json!({
                "error_type": "Io",
                "message": err.to_string(),
            }),
            KernelError::Db(err) => serde_json::json!({
                "error_type": "Db",
                "message": err.to_string(),
            }),
            KernelError::Json(err) => serde_json::json!({
                "error_type": "Json",
                "message": err.to_string(),
            }),
        }
    }
}

/// Convenience alias for `Result<T, KernelError>`.
pub type KernelResult<T> = Result<T, KernelError>;
