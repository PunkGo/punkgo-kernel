//! PunkGo execution sandbox — isolated action execution with output capture.
//!
//! This crate implements the `execute` action backend:
//!
//! - [`ExecutionBackend`] — trait defining the lifecycle protocol:
//!   `snapshot → restore → execute → destroy`
//! - [`ProcessBackend`] — current implementation: OS process spawning with
//!   timeout enforcement, output capture, and SHA-256 artifact hashing
//! - [`run_lifecycle`] — orchestrates the full backend lifecycle with cleanup guarantees
//! - [`BackendRegistry`] — registry for pluggable backends (future: Docker, Firecracker)
//! - [`SandboxConfig`] — config-driven resource limits (output truncation, filesystem boundary)
//!
//! Design: artifact hashes are computed **before** truncation to preserve
//! audit integrity. The system hard timeout (300s) is a committer structural duty.

pub mod backend;
pub mod config;
pub mod executor;
pub mod orchestrator;
pub mod registry;

pub use backend::{ExecutionBackend, ExecutionEnvelope, SnapshotHandle};
pub use config::{SandboxConfig, load_sandbox_config};
pub use executor::{ProcessBackend, SandboxExecutor, SandboxRunResult};
pub use orchestrator::run_lifecycle;
pub use registry::BackendRegistry;
