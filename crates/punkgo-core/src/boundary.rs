//! Boundary enforcement — writable_targets glob matching and access control.
//!
//! Covers: PIP-001 §8 (writability declaration), §9 (privileged targets),
//! §10 (root wildcard), §11 (envelope boundary reduction).
//! Whitepaper §3 invariant 6 (governance auditable): boundary changes logged as events.

use globset::GlobBuilder;

use crate::action::ActionType;
use crate::actor::{ActorRecord, WritableTarget};
use crate::errors::{KernelError, KernelResult};

/// Privileged target prefixes that are restricted to root (PIP-001 §9).
const PRIVILEGED_PREFIXES: &[&str] = &["system/", "ledger/"];

/// Check whether an actor's writable_targets boundary permits the given action
/// on the given target.
///
/// PIP-001 §8: observe is always exempt from writability checks.
/// PIP-001 §8 default deny: default deny — if no writable_target matches, reject.
pub fn check_writable_boundary(
    actor: &ActorRecord,
    target: &str,
    action_type: &ActionType,
) -> KernelResult<()> {
    // PIP-001 §8: observe is exempt from writability checks.
    if matches!(action_type, ActionType::Observe) {
        return Ok(());
    }

    // PIP-001 §9: check privileged target access.
    if is_privileged_target(target) {
        check_privileged_access(actor, target)?;
        return Ok(());
    }

    // PIP-001 §8 default deny: default deny — check writable_targets for a match.
    let action_str = action_type.as_str();
    for wt in &actor.writable_targets {
        if glob_match(&wt.target, target) && wt.actions.iter().any(|a| a == action_str) {
            return Ok(());
        }
    }

    Err(KernelError::BoundaryViolation(format!(
        "actor {} has no writable_target granting {} on {}",
        actor.actor_id, action_str, target
    )))
}

/// Check if a target path is a privileged target (PIP-001 §9).
pub fn is_privileged_target(target: &str) -> bool {
    PRIVILEGED_PREFIXES
        .iter()
        .any(|prefix| target.starts_with(prefix))
}

/// Check if an actor has access to a privileged target (PIP-001 §9).
///
/// Only root (with "**" wildcard) is allowed to write privileged targets.
/// Non-root actors are always denied.
fn check_privileged_access(actor: &ActorRecord, target: &str) -> KernelResult<()> {
    // Root check: root's writable_targets should contain "**"
    for wt in &actor.writable_targets {
        if wt.target == "**" {
            return Ok(());
        }
    }

    Err(KernelError::BoundaryViolation(format!(
        "actor {} denied access to privileged target {}",
        actor.actor_id, target
    )))
}

/// Validate that a child actor's writable_targets are a subset of the parent's
/// boundary (PIP-001 §11: boundary defined by human/parent).
///
/// Every target pattern in `child_targets` must be covered by at least one
/// pattern in the parent's writable_targets, and every action the child claims
/// must also be permitted by the matching parent target.
pub fn validate_child_targets(
    parent: &ActorRecord,
    child_targets: &[WritableTarget],
) -> KernelResult<()> {
    for child_wt in child_targets {
        let mut covered = false;

        for parent_wt in &parent.writable_targets {
            // Check if parent pattern covers child pattern
            if pattern_covers(&parent_wt.target, &child_wt.target) {
                // Check if parent actions are a superset of child actions
                let all_actions_covered = child_wt
                    .actions
                    .iter()
                    .all(|child_action| parent_wt.actions.iter().any(|pa| pa == child_action));

                if all_actions_covered {
                    covered = true;
                    break;
                }
            }
        }

        if !covered {
            return Err(KernelError::BoundaryViolation(format!(
                "child writable_target '{}' with actions {:?} exceeds parent boundary",
                child_wt.target, child_wt.actions
            )));
        }
    }

    Ok(())
}

/// Check if a glob pattern matches a target string.
///
/// Uses the `globset` crate with `literal_separator(true)` for proper
/// path-like glob semantics:
/// - `*` matches any sequence of non-separator characters (NOT `/`)
/// - `**` matches any sequence of characters including `/`
pub fn glob_match(pattern: &str, target: &str) -> bool {
    let glob = match GlobBuilder::new(pattern).literal_separator(true).build() {
        Ok(g) => g,
        Err(_) => return false,
    };
    let matcher = glob.compile_matcher();
    matcher.is_match(target)
}

/// Check if `parent_pattern` covers (is equal to or a superset of) `child_pattern`.
///
/// Conservative approach:
/// - "**" covers everything
/// - exact match covers itself
/// - A parent pattern with "*" or "**" that matches the child pattern literal
///   is considered covering
/// - For safety, we require that the parent pattern matches the child pattern
///   when treated as a literal string, OR the parent is a strict superset glob.
fn pattern_covers(parent_pattern: &str, child_pattern: &str) -> bool {
    // "**" covers everything
    if parent_pattern == "**" {
        return true;
    }

    // Exact match
    if parent_pattern == child_pattern {
        return true;
    }

    // Parent glob matches child pattern literal (e.g., "workspace/*" covers "workspace/a")
    glob_match(parent_pattern, child_pattern)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{ActorStatus, ActorType};

    fn make_actor(actor_id: &str, targets: Vec<WritableTarget>) -> ActorRecord {
        ActorRecord {
            actor_id: actor_id.to_string(),
            actor_type: ActorType::Agent,
            creator_id: Some("root".to_string()),
            lineage: vec!["root".to_string()],
            purpose: Some("test".to_string()),
            status: ActorStatus::Active,
            writable_targets: targets,
            energy_share: 0.0,
            reduction_policy: "none".to_string(),
            created_at: "0".to_string(),
            updated_at: "0".to_string(),
        }
    }

    fn wt(target: &str, actions: &[&str]) -> WritableTarget {
        WritableTarget {
            target: target.to_string(),
            actions: actions.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn boundary_observe_exempt() {
        // PIP-001 §8: observe should always pass, even with empty writable_targets
        let actor = make_actor("agent-1", vec![]);
        let result = check_writable_boundary(&actor, "workspace/secret", &ActionType::Observe);
        assert!(
            result.is_ok(),
            "observe should be exempt from boundary check"
        );
    }

    #[test]
    fn boundary_default_deny() {
        // PIP-001 §8 default deny: no writable_target → deny
        let actor = make_actor("agent-1", vec![]);
        let result = check_writable_boundary(&actor, "workspace/a", &ActionType::Mutate);
        assert!(result.is_err(), "empty writable_targets should deny mutate");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("BoundaryViolation") || err.contains("no writable_target"));
    }

    #[test]
    fn boundary_exact_match() {
        let actor = make_actor("agent-1", vec![wt("workspace/a", &["mutate"])]);
        let result = check_writable_boundary(&actor, "workspace/a", &ActionType::Mutate);
        assert!(result.is_ok(), "exact target match should pass");
    }

    #[test]
    fn boundary_glob_star() {
        // * matches single level
        let actor = make_actor("agent-1", vec![wt("workspace/*", &["create", "mutate"])]);
        assert!(
            check_writable_boundary(&actor, "workspace/foo", &ActionType::Create).is_ok(),
            "workspace/* should match workspace/foo"
        );
        // * should NOT match nested paths
        assert!(
            check_writable_boundary(&actor, "workspace/foo/bar", &ActionType::Create).is_err(),
            "workspace/* should NOT match workspace/foo/bar"
        );
    }

    #[test]
    fn boundary_glob_doublestar() {
        // ** matches multiple levels
        let actor = make_actor("agent-1", vec![wt("workspace/**", &["create", "mutate"])]);
        assert!(
            check_writable_boundary(&actor, "workspace/foo", &ActionType::Create).is_ok(),
            "workspace/** should match workspace/foo"
        );
        assert!(
            check_writable_boundary(&actor, "workspace/foo/bar/baz", &ActionType::Mutate).is_ok(),
            "workspace/** should match workspace/foo/bar/baz"
        );
    }

    #[test]
    fn boundary_action_mismatch() {
        let actor = make_actor("agent-1", vec![wt("workspace/*", &["create"])]);
        let result = check_writable_boundary(&actor, "workspace/a", &ActionType::Mutate);
        assert!(
            result.is_err(),
            "should deny mutate when only create is allowed"
        );
    }

    #[test]
    fn boundary_privileged_target_denied() {
        // PIP-001 §9: non-root actor cannot access system/* or ledger/*
        let actor = make_actor("agent-1", vec![wt("workspace/*", &["create"])]);
        let result = check_writable_boundary(&actor, "system/config", &ActionType::Create);
        assert!(result.is_err(), "non-root should be denied system/* access");
    }

    #[test]
    fn boundary_privileged_target_root_allowed() {
        // PIP-001 §9: root (with "**") can access system/* and ledger/*
        let root = ActorRecord {
            actor_id: "root".to_string(),
            actor_type: ActorType::Human,
            creator_id: None,
            lineage: vec![],
            purpose: None,
            status: ActorStatus::Active,
            writable_targets: vec![wt("**", &["create", "mutate", "execute"])],
            energy_share: 100.0,
            reduction_policy: "none".to_string(),
            created_at: "0".to_string(),
            updated_at: "0".to_string(),
        };
        assert!(
            check_writable_boundary(&root, "system/config", &ActionType::Create).is_ok(),
            "root should access system/*"
        );
        assert!(
            check_writable_boundary(&root, "ledger/actor", &ActionType::Create).is_ok(),
            "root should access ledger/*"
        );
    }

    #[test]
    fn boundary_child_subset_enforced() {
        // PIP-001 §11: child targets must be subset of parent
        let parent = make_actor("root", vec![wt("workspace/*", &["create", "mutate"])]);
        let child_ok = vec![wt("workspace/a", &["create"])];
        assert!(
            validate_child_targets(&parent, &child_ok).is_ok(),
            "child subset should pass"
        );

        let child_bad_target = vec![wt("data/secret", &["create"])];
        assert!(
            validate_child_targets(&parent, &child_bad_target).is_err(),
            "child target outside parent boundary should fail"
        );

        let child_bad_action = vec![wt("workspace/a", &["execute"])];
        assert!(
            validate_child_targets(&parent, &child_bad_action).is_err(),
            "child action not in parent should fail"
        );
    }

    #[test]
    fn boundary_doublestar_parent_covers_all() {
        let parent = make_actor("root", vec![wt("**", &["create", "mutate", "execute"])]);
        let child = vec![wt("workspace/deep/nested/path", &["create", "mutate"])];
        assert!(
            validate_child_targets(&parent, &child).is_ok(),
            "** parent should cover any child target"
        );
    }

    #[test]
    fn is_privileged_target_detection() {
        assert!(is_privileged_target("system/config"));
        assert!(is_privileged_target("system/policy"));
        assert!(is_privileged_target("ledger/actor"));
        assert!(is_privileged_target("ledger/energy"));
        assert!(!is_privileged_target("workspace/a"));
        assert!(!is_privileged_target("data/file"));
    }

    #[test]
    fn glob_match_various_patterns() {
        assert!(glob_match("**", "anything/at/all"));
        assert!(glob_match("workspace/*", "workspace/a"));
        assert!(!glob_match("workspace/*", "workspace/a/b"));
        assert!(glob_match("workspace/**", "workspace/a/b/c"));
        assert!(glob_match("data/*.json", "data/file.json"));
        assert!(!glob_match("data/*.json", "data/file.txt"));
    }
}
