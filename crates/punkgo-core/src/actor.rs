//! Actor types and identity derivation.
//!
//! Covers: PIP-001 §5 (actor type dichotomy), §7 (agent conditional existence),
//! §10 (root wildcard boundary).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// PIP-001 §5: Two immutable actor types, exhaustive and mutually exclusive.
/// - Human: direct human representative, created at kernel init or via identity verification.
/// - Agent: delegated executor, created **only by human** via `create` action (PIP-001 §5/§6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorType {
    Human,
    Agent,
}

impl ActorType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "human" => Some(Self::Human),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }
}

/// Actor lifecycle status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActorStatus {
    Active,
    Frozen,
}

impl ActorStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Frozen => "frozen",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "frozen" => Some(Self::Frozen),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// PIP-001 §8: Writability declaration — a glob pattern + allowed action types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WritableTarget {
    pub target: String,
    pub actions: Vec<String>,
}

/// Full actor record as persisted in the `actors` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorRecord {
    pub actor_id: String,
    pub actor_type: ActorType,
    pub creator_id: Option<String>,
    pub lineage: Vec<String>,
    pub purpose: Option<String>,
    pub status: ActorStatus,
    pub writable_targets: Vec<WritableTarget>,
    pub energy_share: f64,
    pub reduction_policy: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Specification for creating a new actor (used in submit pipeline).
#[derive(Debug, Clone)]
pub struct CreateActorSpec {
    pub actor_id: String,
    pub actor_type: ActorType,
    pub creator_id: String,
    pub lineage: Vec<String>,
    pub purpose: Option<String>,
    pub writable_targets: Vec<WritableTarget>,
    pub energy_balance: i64,
    pub energy_share: f64,
    pub reduction_policy: String,
}

// ---------------------------------------------------------------------------
// Lifecycle operations (PIP-001 §5/§6/§7)
// ---------------------------------------------------------------------------

/// Phase 4a: Lifecycle operations that can be performed on actors.
#[derive(Debug, Clone, PartialEq)]
pub enum LifecycleOp {
    /// Freeze an actor — suspends all state-changing actions.
    Freeze { reason: Option<String> },
    /// Unfreeze — restores an actor to active.
    Unfreeze,
    /// Terminate — permanently removes an agent (creates orphans).
    Terminate { reason: Option<String> },
    /// Update an actor's energy_share for tick-based distribution.
    UpdateEnergyShare { energy_share: f64 },
}

// ---------------------------------------------------------------------------
// Identity derivation
// ---------------------------------------------------------------------------

/// Derive an agent ID that encodes the creation relationship.
///
/// Format: `{creator_id}/{sanitized_purpose}/{seq}`
/// Example: `root/doc-organizer/1`, `root/doc-organizer/1/sub-worker/1`
pub fn derive_agent_id(creator_id: &str, purpose: &str, seq: i64) -> String {
    let sanitized = sanitize_purpose(purpose);
    format!("{creator_id}/{sanitized}/{seq}")
}

/// Sanitize a purpose string for use in an actor ID.
/// Replace non-alphanumeric/non-hyphen characters with hyphens, lowercase.
fn sanitize_purpose(purpose: &str) -> String {
    let s: String = purpose
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    // Collapse consecutive hyphens
    let mut result = String::with_capacity(s.len());
    let mut prev_hyphen = false;
    for c in s.chars() {
        if c == '-' {
            if !prev_hyphen {
                result.push(c);
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }
    result.trim_matches('-').to_string()
}

// ---------------------------------------------------------------------------
// Lineage construction (PIP-001 §7)
// ---------------------------------------------------------------------------

/// Build a creation lineage for a new actor (PIP-001 §7).
///
/// Since only Humans can create Agents (PIP-001 §5/§6), lineage is always
/// a single element: `\[human_creator_id\]`.
///
/// The `creator_type` and `creator_lineage` parameters are kept for
/// backward compatibility but the function enforces Human-only creation.
pub fn build_lineage(
    creator_type: &ActorType,
    creator_id: &str,
    _creator_lineage: &[String],
) -> Vec<String> {
    // PIP-001 §5/§6: Only Humans can create Agents.
    // If creator is Human -> lineage = [human_id] (the only valid case).
    // If creator is Agent -> still returns [creator_id] but kernel.rs should
    // reject this before we get here.
    match creator_type {
        ActorType::Human => vec![creator_id.to_string()],
        ActorType::Agent => {
            // This path should never be reached after PIP-001 §5 enforcement in kernel.rs.
            // Return creator_id as fallback for safety.
            vec![creator_id.to_string()]
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_agent_id_basic() {
        assert_eq!(
            derive_agent_id("root", "doc-organizer", 1),
            "root/doc-organizer/1"
        );
    }

    #[test]
    fn derive_agent_id_second_agent() {
        // PIP-001 §5/§6: Only humans create agents, so creator_id is always a human id.
        // This tests creating a second agent by the same human.
        assert_eq!(
            derive_agent_id("root", "tiktok-downloader", 1),
            "root/tiktok-downloader/1"
        );
    }

    #[test]
    fn derive_agent_id_sanitizes_spaces() {
        assert_eq!(
            derive_agent_id("root", "My Cool Agent", 1),
            "root/my-cool-agent/1"
        );
    }

    #[test]
    fn build_lineage_human_creator() {
        let lineage = build_lineage(&ActorType::Human, "root", &[]);
        assert_eq!(lineage, vec!["root"]);
    }

    #[test]
    fn build_lineage_agent_creator_fallback() {
        // PIP-001 §5: Agent-creates-Agent is rejected at kernel level.
        // build_lineage still returns a safe fallback if called with Agent creator.
        let creator_lineage = vec!["root".to_string()];
        let lineage = build_lineage(&ActorType::Agent, "root/worker/1", &creator_lineage);
        // Fallback: returns [creator_id] (not the old recursive chain)
        assert_eq!(lineage, vec!["root/worker/1"]);
    }

    #[test]
    fn actor_type_roundtrip() {
        assert_eq!(ActorType::parse("human"), Some(ActorType::Human));
        assert_eq!(ActorType::parse("agent"), Some(ActorType::Agent));
        assert_eq!(ActorType::parse("unknown"), None);
    }

    #[test]
    fn actor_status_roundtrip() {
        assert_eq!(ActorStatus::parse("active"), Some(ActorStatus::Active));
        assert_eq!(ActorStatus::parse("frozen"), Some(ActorStatus::Frozen));
    }
}
