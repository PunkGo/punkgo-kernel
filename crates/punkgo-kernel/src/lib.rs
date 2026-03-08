//! PunkGo sovereignty engine — cryptographic audit trails for AI agent accountability.
//!
//! This crate combines the full kernel stack:
//!
//! - [`audit`] — Merkle tree proofs and C2SP checkpoints (RFC 6962)
//! - [`state`] — SQLite persistence: event log, energy ledger, actor store, envelopes
//! - [`runtime`] — 7-step submit pipeline, energy production, actor lifecycle
//! - [`testkit`] — test utilities (temp directories, request builders)
//!
//! The kernel is a **committer, not a judge** — it provides a single linearization
//! point for actions and ensures the 7 invariants defined in the whitepaper §3.

pub mod audit;
pub mod runtime;
pub mod state;

pub mod testkit;

// Re-export the most commonly used types at crate root.
pub use audit::{AuditCheckpoint, AuditError, AuditLog};
pub use runtime::{EnergyProducer, Kernel, KernelConfig, SubmitReceipt};
pub use state::{
    ActorStore, BlobStore, EnergyLedger, EnergyReservation, EnvelopeStore, EventLog, EventRecord,
    NewHoldRequest, StatePaths, StateStore,
};
