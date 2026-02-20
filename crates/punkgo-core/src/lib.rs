//! Core types and protocol definitions for the PunkGo kernel.
//!
//! This crate defines the foundational primitives of the PunkGo system:
//!
//! - [`action`] — Action types (`observe`, `create`, `mutate`, `execute`) and cost quoting
//! - [`actor`] — Actor model: `Human` (unconditional) and `Agent` (conditional), with lineage
//! - [`boundary`] — Writable-target enforcement via glob patterns (PIP-001 §8–§10)
//! - [`consent`] — Authorization envelopes with budget, checkpoint levels, and lifecycle (PIP-001 §11)
//! - [`stellar`] — Energy production config anchored to hardware INT8 TOPS (PIP-001 §1)
//! - [`protocol`] — IPC request/response envelope format
//! - [`policy`] — Action validation and visibility rules
//! - [`errors`] — Unified kernel error types
//!
//! All types in this crate are serializable and form the shared vocabulary between
//! the kernel runtime, state persistence, and CLI client.

pub mod action;
pub mod actor;
pub mod boundary;
pub mod consent;
pub mod errors;
pub mod policy;
pub mod protocol;
pub mod stellar;
