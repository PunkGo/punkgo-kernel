//! PunkGo persistence layer — SQLite-backed state stores.
//!
//! - [`ActorStore`] — actor CRUD, status management, and lineage queries
//! - [`EnergyLedger`] — energy balance tracking with atomic reserve/settle
//! - [`EventLog`] — append-only event log with RFC 8785 canonical JSON hashing
//! - [`EnvelopeStore`] — authorization envelope lifecycle and budget consumption
//! - [`StateStore`] — database bootstrap and migrations
//! - [`BlobStore`] — content-addressable blob storage (Git-style filesystem CAS)

pub mod actor_store;
pub mod blob_store;
pub mod energy_ledger;
pub mod envelope_store;
pub mod event_log;
pub mod store;

pub use actor_store::ActorStore;
pub use blob_store::BlobStore;
pub use energy_ledger::{EnergyLedger, EnergyReservation};
pub use envelope_store::{EnvelopeStore, NewHoldRequest};
pub use event_log::{EventLog, EventRecord};
pub use store::{StatePaths, StateStore};
