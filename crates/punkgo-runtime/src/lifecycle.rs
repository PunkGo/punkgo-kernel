//! Actor lifecycle operations — freeze, unfreeze, terminate.
//!
//! Covers: PIP-001 §7 (agent conditional existence), §5/§6 (actor types),
//! whitepaper §3 invariant 6 (governance auditable).
//!
//! PIP-001 §5/§6: Only humans create agents. Agents cannot manage other agents.
//! §6 corollary: Orphan problem eliminated (human creators cannot be terminated).
//!
//! Lifecycle operations are triggered by submitting actions with:
//!   target = "actor/{actor_id}"
//!   action_type = Mutate
//!   payload.op = "freeze" | "unfreeze" | "terminate"

use punkgo_core::actor::{ActorRecord, ActorStatus, ActorType, LifecycleOp};
use punkgo_core::errors::{KernelError, KernelResult};
use punkgo_state::ActorStore;

/// Parse a lifecycle operation from an action target + payload.
///
/// Returns None if this is not a lifecycle operation.
pub fn parse_lifecycle_op(
    target: &str,
    payload: &serde_json::Value,
) -> Option<(String, LifecycleOp)> {
    // target must be "actor/{actor_id}"
    let actor_id = target.strip_prefix("actor/")?;
    if actor_id.is_empty() {
        return None;
    }

    let op_str = payload.get("op")?.as_str()?;
    let reason = payload
        .get("reason")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let op = match op_str {
        "freeze" => LifecycleOp::Freeze { reason },
        "unfreeze" => LifecycleOp::Unfreeze,
        "terminate" => LifecycleOp::Terminate { reason },
        _ => return None,
    };

    Some((actor_id.to_string(), op))
}

/// Validate that the initiator is authorized to perform a lifecycle operation
/// on the target actor (PIP-001 §5/§6 authorization rules).
///
/// PIP-001 §5/§6: Only Humans can create Agents, so only Humans can manage Agents.
///
/// | Initiator  | Target     | Allowed |
/// |-----------|------------|---------|
/// | human     | own agent  | yes     |
/// | agent     | any        | no      |
/// | root      | any agent  | yes     |
pub async fn validate_lifecycle_authorization(
    initiator: &ActorRecord,
    target: &ActorRecord,
    _op: &LifecycleOp,
) -> KernelResult<()> {
    // Cannot perform lifecycle ops on humans
    if target.actor_type == ActorType::Human {
        return Err(KernelError::PolicyViolation(
            "cannot perform lifecycle operations on human actors".to_string(),
        ));
    }

    // Root can do anything
    if initiator.actor_id == "root" {
        return Ok(());
    }

    // PIP-001 §5: Agents cannot manage other agents.
    if initiator.actor_type == ActorType::Agent {
        return Err(KernelError::PolicyViolation(format!(
            "agent {} cannot perform lifecycle operations — only humans can manage agents (PIP-001 §5)",
            initiator.actor_id
        )));
    }

    // Human initiator: target must be created by this human (lineage contains the human)
    if initiator.actor_type == ActorType::Human {
        if target.lineage.contains(&initiator.actor_id) {
            return Ok(());
        }
        return Err(KernelError::PolicyViolation(format!(
            "human {} cannot manage actor {} (not in lineage)",
            initiator.actor_id, target.actor_id
        )));
    }

    Err(KernelError::PolicyViolation(
        "lifecycle authorization denied".to_string(),
    ))
}

/// Execute a freeze operation: set target to frozen status, cascade to dependents.
///
/// Freezing suspends all state-changing actions.
/// PIP-001 §5/§6: Agents have no children, but when a Human freezes,
/// all agents they created (whose lineage contains the human_id) are also frozen.
pub async fn execute_freeze(
    actor_store: &ActorStore,
    pool: &sqlx::SqlitePool,
    target_id: &str,
) -> KernelResult<Vec<String>> {
    let mut tx = pool.begin().await?;
    let mut frozen_ids = Vec::new();

    // Freeze the target
    actor_store
        .set_status_in_tx(&mut tx, target_id, &ActorStatus::Frozen)
        .await?;
    frozen_ids.push(target_id.to_string());

    // Cascade: freeze all descendants (actors whose lineage contains target_id)
    let descendants = actor_store.list_descendants(target_id).await?;
    for descendant in descendants {
        actor_store
            .set_status_in_tx(&mut tx, &descendant.actor_id, &ActorStatus::Frozen)
            .await?;
        frozen_ids.push(descendant.actor_id);
    }

    tx.commit().await?;
    Ok(frozen_ids)
}

/// Execute an unfreeze operation: set target to active status.
///
/// Note: unfreeze does NOT cascade — each agent must be individually unfrozen.
/// This is deliberate: the human must consciously decide to restore each agent.
pub async fn execute_unfreeze(
    actor_store: &ActorStore,
    pool: &sqlx::SqlitePool,
    target_id: &str,
) -> KernelResult<()> {
    let mut tx = pool.begin().await?;
    actor_store
        .set_status_in_tx(&mut tx, target_id, &ActorStatus::Active)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Check existence conditions for an actor (PIP-001 §7).
///
/// Four conditions that must hold for an actor to exist:
/// 1. Energy >= 0 — checked naturally by reserve/settle
/// 2. Creator active — the human creator must be active (PIP-001 §7)
/// 3. Writable boundary non-empty — checked by Phase 3 boundary enforcement
/// 4. Not frozen — checked by Phase 1 frozen status
///
/// This function checks condition 2 (creator activity).
/// PIP-001 §5/§6: lineage is always single-element \[human_creator_id\],
/// so this is effectively "is my creator active?"
pub async fn check_lineage_active(
    actor_store: &ActorStore,
    lineage: &[String],
) -> KernelResult<()> {
    for ancestor_id in lineage {
        let is_active = actor_store.is_active(ancestor_id).await?;
        if !is_active {
            return Err(KernelError::PolicyViolation(format!(
                "lineage ancestor {} is not active (PIP-001 §7: delegator absence)",
                ancestor_id
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_freeze_op() {
        let (id, op) =
            parse_lifecycle_op("actor/agent-1", &json!({"op": "freeze", "reason": "test"}))
                .expect("should parse");
        assert_eq!(id, "agent-1");
        assert!(matches!(op, LifecycleOp::Freeze { reason: Some(r) } if r == "test"));
    }

    #[test]
    fn parse_unfreeze_op() {
        let (id, op) =
            parse_lifecycle_op("actor/agent-1", &json!({"op": "unfreeze"})).expect("should parse");
        assert_eq!(id, "agent-1");
        assert!(matches!(op, LifecycleOp::Unfreeze));
    }

    #[test]
    fn parse_non_lifecycle_target() {
        let result = parse_lifecycle_op("workspace/a", &json!({"op": "freeze"}));
        assert!(result.is_none(), "non-actor target should return None");
    }

    #[test]
    fn parse_unknown_op() {
        let result = parse_lifecycle_op("actor/agent-1", &json!({"op": "destroy"}));
        assert!(result.is_none(), "unknown op should return None");
    }
}
