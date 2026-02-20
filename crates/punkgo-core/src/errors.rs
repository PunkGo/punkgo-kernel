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
    #[error("sandbox error: {0}")]
    Sandbox(String),
    #[error("audit error: {0}")]
    Audit(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Convenience alias for `Result<T, KernelError>`.
pub type KernelResult<T> = Result<T, KernelError>;
