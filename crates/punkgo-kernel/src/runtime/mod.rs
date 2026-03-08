//! PunkGo kernel runtime — the 7-step submit pipeline and energy production.
//!
//! - [`Kernel`] — bootstraps state, handles requests, and runs the submit pipeline:
//!   `validate → quote → reserve → validate_payload → settle → append → post-commit`
//! - [`EnergyProducer`] — background task distributing energy per tick,
//!   anchored to hardware compute power (PIP-001 §1–§2)
//! - [`lifecycle`] — actor lifecycle operations: freeze, unfreeze, terminate
//! - [`SubmitReceipt`] — cryptographic receipt returned after successful commit

pub mod energy_producer;
mod kernel;
pub mod lifecycle;

pub use energy_producer::EnergyProducer;
pub use kernel::{Kernel, KernelConfig, SubmitReceipt};
