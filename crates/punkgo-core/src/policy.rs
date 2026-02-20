use crate::action::{Action, ActionType, quote_cost};
use crate::errors::{KernelError, KernelResult};

pub fn validate_action(action: &Action) -> KernelResult<()> {
    if action.actor_id.trim().is_empty() {
        return Err(KernelError::PolicyViolation(
            "actor_id cannot be empty".to_string(),
        ));
    }
    if action.target.trim().is_empty() {
        return Err(KernelError::PolicyViolation(
            "target cannot be empty".to_string(),
        ));
    }
    if action.action_type.is_state_changing() && quote_cost(action) == 0 {
        return Err(KernelError::PolicyViolation(
            "state-changing action must have positive cost".to_string(),
        ));
    }

    // Visibility boundary check for observe actions.
    // Currently all-transparent (single-user scenario). When multi-user
    // collaboration is introduced, this function will enforce
    // readable_targets per Actor. See PIP-001 §8 (writable_targets).
    if matches!(action.action_type, ActionType::Observe) {
        check_visibility(&action.actor_id, &action.target)?;
    }

    Ok(())
}

/// Visibility boundary gate for observe actions.
///
/// Current policy: all-transparent — every Actor can observe any target.
/// This is the designated extension point for readable_targets enforcement.
/// When activated, this should check `actor.readable_targets` contains the
/// requested target, mirroring the writable_targets check for state-changing
/// actions.
pub fn check_visibility(_actor_id: &str, _target: &str) -> KernelResult<()> {
    // TODO(multi-user): enforce readable_targets here
    Ok(())
}

/// Read-query access gate.
///
/// Current policy: all-transparent — any requester can read any data.
/// This is the designated extension point for read-path access control
/// (events, actor_energy, paths, etc.). When multi-user collaboration is
/// introduced, this should verify the requester's readable scope covers
/// the requested query kind and parameters.
pub fn check_read_access(_actor_id: &str, _query_kind: &str) -> KernelResult<()> {
    // TODO(multi-user): enforce read access control here
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::action::{Action, ActionType};

    use super::validate_action;

    #[test]
    fn reject_empty_actor() {
        let action = Action {
            actor_id: "".to_string(),
            action_type: ActionType::Mutate,
            target: "t".to_string(),
            payload: json!({}),
            timestamp: None,
        };
        assert!(validate_action(&action).is_err());
    }

    #[test]
    fn reject_empty_target() {
        let action = Action {
            actor_id: "root".to_string(),
            action_type: ActionType::Mutate,
            target: "".to_string(),
            payload: json!({}),
            timestamp: None,
        };
        assert!(validate_action(&action).is_err());
    }

    #[test]
    fn accept_valid_mutate() {
        let action = Action {
            actor_id: "root".to_string(),
            action_type: ActionType::Mutate,
            target: "workspace/a".to_string(),
            payload: json!({ "name": "ok" }),
            timestamp: None,
        };
        assert!(validate_action(&action).is_ok());
    }
}
