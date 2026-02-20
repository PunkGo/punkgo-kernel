//! Consent and authorization — budget envelope system.
//!
//! Covers: PIP-001 §11 (envelope: budget + targets + actions + duration + checkpoint + hold),
//! §5/§6 (human unconditional / agent conditional), §12 (execute encapsulation).
//! Whitepaper §2: E'=E-cost(A) — every action has a cost quote.
//! Whitepaper §3 invariant 6: authorization changes enter event history.

use serde::{Deserialize, Serialize};

use crate::actor::{ActorRecord, ActorType};
use crate::errors::{KernelError, KernelResult};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

/// PIP-001 §5/§6: Two authorization modes.
/// - ManInTheLoop: Human directly submitting — always authorized.
/// - ManOnTheLoop: Agent with an active envelope — authorized within envelope scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthorizationMode {
    ManInTheLoop,
    ManOnTheLoop,
}

/// Envelope lifecycle status.
///
/// Active → Expired (budget exhausted or duration elapsed) or Revoked (human revocation).
/// Hold does NOT change envelope status — agent can continue submitting while holds are pending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvelopeStatus {
    Active,
    Expired,
    Revoked,
}

impl EnvelopeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(Self::Active),
            "expired" => Some(Self::Expired),
            "revoked" => Some(Self::Revoked),
            _ => None,
        }
    }
}

/// PIP-001 §11 checkpoint: Two-level checkpoint types.
///
/// Report: summary event logged, agent continues.
/// Halt: budget exhausted, envelope expires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointLevel {
    /// Report: agent continues; summary event logged for human review.
    Report,
    /// Halt: envelope budget exhausted; no further actions allowed.
    Halt,
}

// ---------------------------------------------------------------------------
// PIP-001 §11a: HoldRule
// ---------------------------------------------------------------------------

/// PIP-001 §11a: A single rule in the `hold_on` field of an envelope.
///
/// If an action's (target, action_type) matches this rule, the kernel
/// triggers a hold before execution and waits for human judgment.
/// The envelope stays Active — agent can continue submitting other actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HoldRule {
    /// Glob pattern matched against the action's target.
    pub target: String,
    /// Action types that trigger hold when target matches (e.g. ["execute"]).
    pub actions: Vec<String>,
}

/// PIP-001 §11b: Check whether an action triggers a hold based on the envelope's hold_on rules.
///
/// Returns true if the action matches any hold_on rule.
pub fn check_hold_trigger(hold_on: &[HoldRule], target: &str, action_type: &str) -> bool {
    hold_on.iter().any(|rule| {
        crate::boundary::glob_match(&rule.target, target)
            && rule.actions.iter().any(|a| a == action_type)
    })
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

/// Specification for creating a new envelope (PIP-001 §11).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeSpec {
    /// The agent receiving this envelope.
    pub actor_id: String,
    /// The human or parent-agent granting the envelope.
    pub grantor_id: String,
    /// Energy consumption budget (upper bound).
    pub budget: i64,
    /// Allowed target patterns (glob).
    pub targets: Vec<String>,
    /// Allowed action types within this envelope.
    pub actions: Vec<String>,
    /// Duration in seconds; None = no time limit.
    pub duration_secs: Option<i64>,
    /// PIP-001 §11 checkpoint: Report every N energy units consumed. None = no reporting.
    pub report_every: Option<i64>,
    /// PIP-001 §11a: Rules that trigger a hold before action execution.
    #[serde(default)]
    pub hold_on: Vec<HoldRule>,
    /// PIP-001 §11e: Seconds before an unresponded hold auto-rejects. None = no timeout.
    pub hold_timeout_secs: Option<i64>,
}

/// Persisted envelope record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeRecord {
    pub envelope_id: String,
    pub actor_id: String,
    pub grantor_id: String,
    pub parent_envelope_id: Option<String>,
    pub budget: i64,
    pub budget_consumed: i64,
    pub targets: Vec<String>,
    pub actions: Vec<String>,
    pub duration_secs: Option<i64>,
    pub report_every: Option<i64>,
    /// PIP-001 §11a: Rules that trigger a hold before action execution.
    #[serde(default)]
    pub hold_on: Vec<HoldRule>,
    /// PIP-001 §11e: Seconds before an unresponded hold auto-rejects.
    pub hold_timeout_secs: Option<i64>,
    pub status: EnvelopeStatus,
    pub last_report_at: i64,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Authorization logic
// ---------------------------------------------------------------------------

/// PIP-001 §5/§6: Determine the authorization mode for an actor.
///
/// | Actor type | Behavior |
/// |-----------|----------|
/// | Human     | ManInTheLoop — always authorized |
/// | Agent     | ManOnTheLoop — requires active envelope |
pub fn determine_authorization_mode(actor: &ActorRecord) -> AuthorizationMode {
    match actor.actor_type {
        ActorType::Human => AuthorizationMode::ManInTheLoop,
        ActorType::Agent => AuthorizationMode::ManOnTheLoop,
    }
}

/// PIP-001 §11: Check if an agent has authorization via an active envelope.
///
/// For ManInTheLoop (humans), always returns Ok.
/// For ManOnTheLoop (agents), requires an active, non-expired envelope.
pub fn check_authorization(
    actor: &ActorRecord,
    envelope: Option<&EnvelopeRecord>,
) -> KernelResult<AuthorizationMode> {
    let mode = determine_authorization_mode(actor);

    match mode {
        AuthorizationMode::ManInTheLoop => Ok(mode),
        AuthorizationMode::ManOnTheLoop => {
            let envelope = envelope.ok_or_else(|| {
                KernelError::AuthorizationRequired(format!(
                    "agent {} requires an active envelope to perform state-changing actions (PIP-001 §11)",
                    actor.actor_id
                ))
            })?;

            if envelope.status != EnvelopeStatus::Active {
                return Err(KernelError::AuthorizationRequired(format!(
                    "agent {} envelope {} is {} (not active)",
                    actor.actor_id,
                    envelope.envelope_id,
                    envelope.status.as_str()
                )));
            }

            Ok(mode)
        }
    }
}

/// PIP-001 §11: Check if an envelope covers the requested target and action.
pub fn check_envelope_covers(
    envelope: &EnvelopeRecord,
    target: &str,
    action: &str,
) -> KernelResult<()> {
    // Check action is in envelope's allowed actions
    if !envelope.actions.iter().any(|a| a == action) {
        return Err(KernelError::AuthorizationRequired(format!(
            "envelope {} does not permit action '{}' (allowed: {:?})",
            envelope.envelope_id, action, envelope.actions
        )));
    }

    // Check target matches at least one envelope target pattern
    let target_matched = envelope
        .targets
        .iter()
        .any(|pattern| crate::boundary::glob_match(pattern, target));

    if !target_matched {
        return Err(KernelError::AuthorizationRequired(format!(
            "envelope {} does not cover target '{}' (allowed patterns: {:?})",
            envelope.envelope_id, target, envelope.targets
        )));
    }

    // Check budget not exhausted
    let remaining = envelope.budget - envelope.budget_consumed;
    if remaining <= 0 {
        return Err(KernelError::AuthorizationRequired(format!(
            "envelope {} budget exhausted (budget={}, consumed={})",
            envelope.envelope_id, envelope.budget, envelope.budget_consumed
        )));
    }

    Ok(())
}

/// PIP-001 §11 checkpoint: Check if a checkpoint should be triggered after consuming energy.
///
/// Returns the highest-level checkpoint triggered (Halt > Report),
/// or None if no checkpoint is due.
pub fn check_checkpoint(envelope: &EnvelopeRecord, new_consumed: i64) -> Option<CheckpointLevel> {
    let total_consumed = envelope.budget_consumed + new_consumed;

    // Halt: budget exhausted
    if total_consumed >= envelope.budget {
        return Some(CheckpointLevel::Halt);
    }

    // Report: crossed a report threshold
    if let Some(report_interval) = envelope.report_every
        && report_interval > 0
    {
        let prev_report_count = envelope.budget_consumed / report_interval;
        let new_report_count = total_consumed / report_interval;
        if new_report_count > prev_report_count {
            return Some(CheckpointLevel::Report);
        }
    }

    None
}

/// PIP-001 §11: Validate that a child envelope's parameters don't exceed the parent's.
///
/// Authorization chain must be monotonically non-increasing:
/// - child.budget <= parent remaining budget
/// - child.targets must be covered by parent.targets
/// - child.actions must be subset of parent.actions
pub fn validate_envelope_reduction(
    parent: &EnvelopeRecord,
    child: &EnvelopeSpec,
) -> KernelResult<()> {
    // Budget: child cannot exceed parent's remaining budget
    let parent_remaining = parent.budget - parent.budget_consumed;
    if child.budget > parent_remaining {
        return Err(KernelError::AuthorizationRequired(format!(
            "child envelope budget {} exceeds parent remaining {} (PIP-001 §11)",
            child.budget, parent_remaining
        )));
    }

    // Actions: child actions must be subset of parent actions
    for child_action in &child.actions {
        if !parent.actions.iter().any(|pa| pa == child_action) {
            return Err(KernelError::AuthorizationRequired(format!(
                "child envelope action '{}' not in parent actions {:?} (PIP-001 §11)",
                child_action, parent.actions
            )));
        }
    }

    // Targets: each child target must be covered by at least one parent target
    for child_target in &child.targets {
        let covered = parent.targets.iter().any(|parent_target| {
            crate::boundary::glob_match(parent_target, child_target)
                || parent_target == "**"
                || parent_target == child_target
        });
        if !covered {
            return Err(KernelError::AuthorizationRequired(format!(
                "child envelope target '{}' not covered by parent targets {:?} (PIP-001 §11)",
                child_target, parent.targets
            )));
        }
    }

    Ok(())
}

/// Check if an envelope has expired by time (lazy expiry).
/// Returns true if the envelope should be marked as expired.
pub fn is_envelope_expired(envelope: &EnvelopeRecord, now_millis: u64) -> bool {
    if let Some(ref expires_at) = envelope.expires_at
        && let Ok(expires_ms) = expires_at.parse::<u64>()
    {
        return now_millis >= expires_ms;
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::{ActorStatus, ActorType, WritableTarget};

    fn make_human(id: &str) -> ActorRecord {
        ActorRecord {
            actor_id: id.to_string(),
            actor_type: ActorType::Human,
            creator_id: None,
            lineage: vec![],
            purpose: None,
            status: ActorStatus::Active,
            writable_targets: vec![WritableTarget {
                target: "**".to_string(),
                actions: vec![
                    "create".to_string(),
                    "mutate".to_string(),
                    "execute".to_string(),
                ],
            }],
            energy_share: 100.0,
            reduction_policy: "none".to_string(),
            created_at: "0".to_string(),
            updated_at: "0".to_string(),
        }
    }

    fn make_agent(id: &str) -> ActorRecord {
        ActorRecord {
            actor_id: id.to_string(),
            actor_type: ActorType::Agent,
            creator_id: Some("root".to_string()),
            lineage: vec!["root".to_string()],
            purpose: Some("test".to_string()),
            status: ActorStatus::Active,
            writable_targets: vec![WritableTarget {
                target: "workspace/*".to_string(),
                actions: vec!["create".to_string(), "mutate".to_string()],
            }],
            energy_share: 0.0,
            reduction_policy: "none".to_string(),
            created_at: "0".to_string(),
            updated_at: "0".to_string(),
        }
    }

    fn make_envelope(id: &str, actor_id: &str) -> EnvelopeRecord {
        EnvelopeRecord {
            envelope_id: id.to_string(),
            actor_id: actor_id.to_string(),
            grantor_id: "root".to_string(),
            parent_envelope_id: None,
            budget: 10000,
            budget_consumed: 0,
            targets: vec!["workspace/*".to_string()],
            actions: vec!["create".to_string(), "mutate".to_string()],
            duration_secs: None,
            report_every: None,
            hold_on: vec![],
            hold_timeout_secs: None,
            status: EnvelopeStatus::Active,
            last_report_at: 0,
            created_at: "0".to_string(),
            expires_at: None,
            updated_at: "0".to_string(),
        }
    }

    #[test]
    fn human_always_man_in_the_loop() {
        let human = make_human("root");
        let mode = determine_authorization_mode(&human);
        assert_eq!(mode, AuthorizationMode::ManInTheLoop);
    }

    #[test]
    fn agent_is_man_on_the_loop() {
        let agent = make_agent("agent-1");
        let mode = determine_authorization_mode(&agent);
        assert_eq!(mode, AuthorizationMode::ManOnTheLoop);
    }

    #[test]
    fn human_authorized_without_envelope() {
        let human = make_human("root");
        let result = check_authorization(&human, None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), AuthorizationMode::ManInTheLoop);
    }

    #[test]
    fn agent_rejected_without_envelope() {
        let agent = make_agent("agent-1");
        let result = check_authorization(&agent, None);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("requires an active envelope"));
    }

    #[test]
    fn agent_authorized_with_active_envelope() {
        let agent = make_agent("agent-1");
        let envelope = make_envelope("env-1", "agent-1");
        let result = check_authorization(&agent, Some(&envelope));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), AuthorizationMode::ManOnTheLoop);
    }

    #[test]
    fn envelope_covers_matching_target_and_action() {
        let envelope = make_envelope("env-1", "agent-1");
        let result = check_envelope_covers(&envelope, "workspace/readme", "create");
        assert!(result.is_ok());
    }

    #[test]
    fn envelope_rejects_wrong_action() {
        let envelope = make_envelope("env-1", "agent-1");
        let result = check_envelope_covers(&envelope, "workspace/readme", "execute");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("does not permit action")
        );
    }

    #[test]
    fn envelope_rejects_wrong_target() {
        let envelope = make_envelope("env-1", "agent-1");
        let result = check_envelope_covers(&envelope, "data/secret", "create");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("does not cover target")
        );
    }

    #[test]
    fn envelope_rejects_exhausted_budget() {
        let mut envelope = make_envelope("env-1", "agent-1");
        envelope.budget_consumed = envelope.budget; // fully consumed
        let result = check_envelope_covers(&envelope, "workspace/a", "create");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("budget exhausted"));
    }

    #[test]
    fn checkpoint_halt_on_budget_exhaustion() {
        let mut envelope = make_envelope("env-1", "agent-1");
        envelope.budget = 100;
        envelope.budget_consumed = 90;
        let level = check_checkpoint(&envelope, 15);
        assert_eq!(level, Some(CheckpointLevel::Halt));
    }

    #[test]
    fn checkpoint_report_on_threshold() {
        let mut envelope = make_envelope("env-1", "agent-1");
        envelope.budget = 10000;
        envelope.budget_consumed = 490;
        envelope.report_every = Some(500);
        let level = check_checkpoint(&envelope, 15);
        assert_eq!(level, Some(CheckpointLevel::Report));
    }

    #[test]
    fn checkpoint_none_when_no_threshold_crossed() {
        let mut envelope = make_envelope("env-1", "agent-1");
        envelope.budget = 10000;
        envelope.budget_consumed = 100;
        envelope.report_every = Some(500);
        let level = check_checkpoint(&envelope, 10);
        assert_eq!(level, None);
    }

    #[test]
    fn envelope_reduction_budget_exceeds_parent() {
        let parent = make_envelope("parent-env", "parent-agent");
        let child_spec = EnvelopeSpec {
            actor_id: "child-agent".to_string(),
            grantor_id: "parent-agent".to_string(),
            budget: 20000, // exceeds parent's 10000
            targets: vec!["workspace/*".to_string()],
            actions: vec!["create".to_string()],
            duration_secs: None,
            report_every: None,
            hold_on: vec![],
            hold_timeout_secs: None,
        };
        let result = validate_envelope_reduction(&parent, &child_spec);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("exceeds parent remaining")
        );
    }

    #[test]
    fn envelope_reduction_action_not_in_parent() {
        let parent = make_envelope("parent-env", "parent-agent");
        let child_spec = EnvelopeSpec {
            actor_id: "child-agent".to_string(),
            grantor_id: "parent-agent".to_string(),
            budget: 5000,
            targets: vec!["workspace/*".to_string()],
            actions: vec!["execute".to_string()], // not in parent's [create, mutate]
            duration_secs: None,
            report_every: None,
            hold_on: vec![],
            hold_timeout_secs: None,
        };
        let result = validate_envelope_reduction(&parent, &child_spec);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not in parent actions")
        );
    }

    #[test]
    fn envelope_reduction_valid_subset() {
        let parent = make_envelope("parent-env", "parent-agent");
        let child_spec = EnvelopeSpec {
            actor_id: "child-agent".to_string(),
            grantor_id: "parent-agent".to_string(),
            budget: 5000,
            targets: vec!["workspace/docs".to_string()],
            actions: vec!["create".to_string()],
            duration_secs: None,
            report_every: None,
            hold_on: vec![],
            hold_timeout_secs: None,
        };
        let result = validate_envelope_reduction(&parent, &child_spec);
        assert!(result.is_ok());
    }

    #[test]
    fn lazy_expiry_not_expired() {
        let envelope = make_envelope("env-1", "agent-1");
        // No expires_at → not expired
        assert!(!is_envelope_expired(&envelope, 999999));
    }

    #[test]
    fn lazy_expiry_expired() {
        let mut envelope = make_envelope("env-1", "agent-1");
        envelope.expires_at = Some("1000".to_string()); // expires at 1000ms
        assert!(is_envelope_expired(&envelope, 2000));
        assert!(!is_envelope_expired(&envelope, 500));
    }

    // PIP-001 §11a: Hold mechanism tests

    #[test]
    fn hold_trigger_matches_target_and_action() {
        let rules = vec![HoldRule {
            target: "external/*".to_string(),
            actions: vec!["execute".to_string()],
        }];
        assert!(check_hold_trigger(&rules, "external/api", "execute"));
    }

    #[test]
    fn hold_trigger_no_match_wrong_action() {
        let rules = vec![HoldRule {
            target: "external/*".to_string(),
            actions: vec!["execute".to_string()],
        }];
        assert!(!check_hold_trigger(&rules, "external/api", "mutate"));
    }

    #[test]
    fn hold_trigger_no_match_wrong_target() {
        let rules = vec![HoldRule {
            target: "external/*".to_string(),
            actions: vec!["execute".to_string()],
        }];
        assert!(!check_hold_trigger(&rules, "workspace/api", "execute"));
    }

    #[test]
    fn hold_trigger_empty_rules_never_triggers() {
        assert!(!check_hold_trigger(&[], "external/api", "execute"));
    }

    #[test]
    fn hold_trigger_multiple_rules_first_match() {
        let rules = vec![
            HoldRule {
                target: "system/*".to_string(),
                actions: vec!["mutate".to_string()],
            },
            HoldRule {
                target: "external/*".to_string(),
                actions: vec!["execute".to_string()],
            },
        ];
        assert!(check_hold_trigger(&rules, "external/openai", "execute"));
        assert!(check_hold_trigger(&rules, "system/config", "mutate"));
        assert!(!check_hold_trigger(&rules, "workspace/foo", "execute"));
    }
}
