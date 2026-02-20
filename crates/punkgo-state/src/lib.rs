//! PunkGo persistence layer — SQLite-backed state stores.
//!
//! This crate provides all persistent storage for the kernel:
//!
//! - [`ActorStore`] — actor CRUD, status management, and lineage queries
//! - [`EnergyLedger`] — energy balance tracking with atomic reserve/settle
//! - [`EventLog`] — append-only event log with RFC 8785 canonical JSON hashing
//! - [`EnvelopeStore`] — authorization envelope lifecycle and budget consumption
//! - [`StateStore`] — database bootstrap, migrations, and snapshot management
//! - [`SnapshotInfo`] — cryptographic state snapshot (event count + root hash)
//! - [`BlobStore`] — content-addressable blob storage (Git-style filesystem CAS)
//!
//! All writes go through SQLite transactions to maintain the kernel's
//! atomicity guarantees. Event hashes follow RFC 6962 (0x00 prefix + SHA-256).

pub mod actor_store;
pub mod blob_store;
pub mod energy_ledger;
pub mod envelope_store;
pub mod event_log;
pub mod snapshot;
pub mod store;

pub use actor_store::ActorStore;
pub use blob_store::BlobStore;
pub use energy_ledger::{EnergyLedger, EnergyReservation};
pub use envelope_store::{EnvelopeStore, NewHoldRequest};
pub use event_log::{EventLog, EventRecord};
pub use snapshot::SnapshotInfo;
pub use store::{StatePaths, StateStore};
