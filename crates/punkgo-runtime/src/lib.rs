//! PunkGo kernel runtime — the 7-step submit pipeline and energy production.
//!
//! This crate contains the kernel's core logic:
//!
//! - [`Kernel`] — bootstraps state, handles requests, and runs the submit pipeline:
//!   `validate → quote → reserve → execute → settle → append → post-commit`
//! - [`EnergyProducer`] — background task distributing energy per tick,
//!   anchored to hardware compute power (PIP-001 §1–§2)
//! - [`lifecycle`] — actor lifecycle operations: freeze, unfreeze, terminate
//! - [`SubmitReceipt`] — cryptographic receipt returned after successful commit
//!
//! The kernel is a **committer, not a judge** — it provides a single linearization
//! point for actions and ensures the 7 invariants defined in the whitepaper §3.

pub mod energy_producer;
mod kernel;
pub mod lifecycle;

pub use energy_producer::EnergyProducer;
pub use kernel::{Kernel, KernelConfig, SubmitReceipt};
