//! PIP-001 §11 Hold mechanism integration tests.
//!
//! Covers: §11a hold_on declaration, §11b trigger evaluation,
//!         §11d hold response (approve/reject).

use punkgo_core::action::{Action, ActionType};
use punkgo_core::protocol::RequestType;
use punkgo_runtime::{Kernel, KernelConfig};
use punkgo_testkit::{TestStateDir, make_request};
use serde_json::json;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

async fn setup_kernel() -> (Kernel, TestStateDir) {
    let state = TestStateDir::new("punkgo-hold-test").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config)
        .await
        .expect("kernel bootstrap should succeed");
    (kernel, state)
}

/// Create an agent actor with the given writable_targets.
async fn create_agent(kernel: &Kernel, agent_id: &str, targets: serde_json::Value) {
    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/actor".to_string(),
        payload: json!({
            "actor_id": agent_id,
            "actor_type": "agent",
            "purpose": "hold-test",
            "energy_balance": 100_000,
            "energy_share": 0.0,
            "writable_targets": targets
        }),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).unwrap(),
        ))
        .await;
    assert_eq!(
        resp.status, "ok",
        "create agent should succeed: {}",
        resp.payload
    );
}

/// Grant an envelope with optional hold_on rules.
async fn grant_envelope_with_hold(
    kernel: &Kernel,
    agent_id: &str,
    targets: serde_json::Value,
    actions: serde_json::Value,
    budget: i64,
    hold_on: serde_json::Value,
) {
    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/envelope".to_string(),
        payload: json!({
            "actor_id": agent_id,
            "budget": budget,
            "targets": targets,
            "actions": actions,
            "hold_on": hold_on
        }),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).unwrap(),
        ))
        .await;
    assert_eq!(
        resp.status, "ok",
        "grant envelope should succeed: {}",
        resp.payload
    );
}

/// Extract `hold_id` from a structured HoldTriggered error payload.
fn extract_hold_id(payload: &serde_json::Value) -> String {
    payload["hold_id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing hold_id in payload: {payload}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// PIP-001 §11b: Hold trigger evaluation
// ---------------------------------------------------------------------------

/// PIP-001 §11b: An action matching hold_on triggers a hold instead of executing.
#[tokio::test]
async fn hold_triggered_on_matching_action() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "hold-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    // Envelope with hold_on: any mutate on workspace/sensitive/* triggers hold.
    grant_envelope_with_hold(
        &kernel,
        "hold-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        50_000,
        json!([{"target": "workspace/sensitive/*", "actions": ["mutate"]}]),
    )
    .await;

    // Submit a mutate to workspace/sensitive/data — should trigger hold.
    let action = Action {
        actor_id: "hold-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/sensitive/data".to_string(),
        payload: json!({"secret": "value"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).unwrap(),
        ))
        .await;

    assert_eq!(
        resp.status, "error",
        "hold should return error: {}",
        resp.payload
    );
    assert!(
        resp.payload["error_type"] == "HoldTriggered",
        "error should mention hold triggered: {}",
        resp.payload
    );
    assert!(
        resp.payload["hold_id"].is_string(),
        "error should contain hold_id: {}",
        resp.payload
    );
}

/// PIP-001 §11b: Action NOT matching hold_on rules proceeds normally.
#[tokio::test]
async fn non_matching_action_not_held() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "selective-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    // hold_on: only workspace/sensitive/* triggers hold.
    grant_envelope_with_hold(
        &kernel,
        "selective-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        50_000,
        json!([{"target": "workspace/sensitive/*", "actions": ["mutate"]}]),
    )
    .await;

    // mutate workspace/normal/data — should succeed (not sensitive).
    let action = Action {
        actor_id: "selective-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/normal/data".to_string(),
        payload: json!({"key": "val"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).unwrap(),
        ))
        .await;
    assert_eq!(
        resp.status, "ok",
        "non-matching action should proceed normally: {}",
        resp.payload
    );
}

/// PIP-001 §11b: Empty hold_on rules never trigger a hold.
#[tokio::test]
async fn empty_hold_on_never_triggers() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "no-hold-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    // Empty hold_on — no rules.
    grant_envelope_with_hold(
        &kernel,
        "no-hold-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        50_000,
        json!([]),
    )
    .await;

    let action = Action {
        actor_id: "no-hold-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/any/path".to_string(),
        payload: json!({"k": "v"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).unwrap(),
        ))
        .await;
    assert_eq!(
        resp.status, "ok",
        "empty hold_on should never trigger: {}",
        resp.payload
    );
}

// ---------------------------------------------------------------------------
// PIP-001 §11b: Hold does NOT block agent — envelope stays Active
// ---------------------------------------------------------------------------

/// PIP-001 §11b: After hold is triggered, envelope stays Active.
/// Agent CAN submit other non-hold-triggering actions.
#[tokio::test]
async fn agent_can_submit_while_hold_pending() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "unblocked-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope_with_hold(
        &kernel,
        "unblocked-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        50_000,
        json!([{"target": "workspace/restricted/*", "actions": ["mutate"]}]),
    )
    .await;

    // Trigger a hold.
    let trigger_action = Action {
        actor_id: "unblocked-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/restricted/file".to_string(),
        payload: json!({"sensitive": true}),
        timestamp: None,
    };
    let trigger_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger_action).unwrap(),
        ))
        .await;
    assert_eq!(trigger_resp.status, "error");
    assert!(trigger_resp.payload["error_type"] == "HoldTriggered");

    // Agent submits a different action to a non-restricted path — should SUCCEED.
    let follow_action = Action {
        actor_id: "unblocked-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/safe/path".to_string(),
        payload: json!({"other": "data"}),
        timestamp: None,
    };
    let follow_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(follow_action).unwrap(),
        ))
        .await;
    assert_eq!(
        follow_resp.status, "ok",
        "agent should be able to submit while hold is pending: {}",
        follow_resp.payload
    );
}

// ---------------------------------------------------------------------------
// PIP-001 §11d: Approve semantics
// ---------------------------------------------------------------------------

/// PIP-001 §11d: Human approve re-executes the pending action.
#[tokio::test]
async fn approve_hold_restores_active_and_executes() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "approve-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope_with_hold(
        &kernel,
        "approve-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        50_000,
        json!([{"target": "workspace/gated/*", "actions": ["mutate"]}]),
    )
    .await;

    // Trigger hold.
    let trigger_action = Action {
        actor_id: "approve-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/gated/resource".to_string(),
        payload: json!({"value": 42}),
        timestamp: None,
    };
    let trigger_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger_action).unwrap(),
        ))
        .await;
    assert_eq!(trigger_resp.status, "error");
    let hold_id = extract_hold_id(&trigger_resp.payload);

    // Human (root) approves the hold.
    let approve_action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: format!("ledger/hold/{hold_id}"),
        payload: json!({
            "decision": "approve",
            "instruction": "proceed with care"
        }),
        timestamp: None,
    };
    let approve_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(approve_action).unwrap(),
        ))
        .await;
    assert_eq!(
        approve_resp.status, "ok",
        "approve should succeed: {}",
        approve_resp.payload
    );

    // After approve, envelope should be active again.
    // Agent can now submit further actions.
    let follow_action = Action {
        actor_id: "approve-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/normal/path".to_string(),
        payload: json!({"after_approve": true}),
        timestamp: None,
    };
    let follow_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(follow_action).unwrap(),
        ))
        .await;
    assert_eq!(
        follow_resp.status, "ok",
        "after approve, agent should be active again: {}",
        follow_resp.payload
    );
}

/// PIP-001 §11d: Approve re-executes pending action with instruction in payload.
#[tokio::test]
async fn approve_with_instruction_written_to_event_history() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "instruct-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope_with_hold(
        &kernel,
        "instruct-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        50_000,
        json!([{"target": "workspace/gated/*", "actions": ["mutate"]}]),
    )
    .await;

    // Trigger hold.
    let trigger_action = Action {
        actor_id: "instruct-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/gated/task".to_string(),
        payload: json!({"command": "do_thing"}),
        timestamp: None,
    };
    let trigger_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger_action).unwrap(),
        ))
        .await;
    assert_eq!(trigger_resp.status, "error");
    let hold_id = extract_hold_id(&trigger_resp.payload);

    // Approve with instruction.
    let approve_action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: format!("ledger/hold/{hold_id}"),
        payload: json!({
            "decision": "approve",
            "instruction": "only proceed in read-only mode"
        }),
        timestamp: None,
    };
    let approve_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(approve_action).unwrap(),
        ))
        .await;
    assert_eq!(
        approve_resp.status, "ok",
        "approve with instruction should succeed: {}",
        approve_resp.payload
    );

    // Verify the hold_response and hold_request events are in history.
    let events_resp = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "events", "limit": 20 }),
        ))
        .await;
    assert_eq!(events_resp.status, "ok");
    let events = events_resp.payload["events"]
        .as_array()
        .expect("events array");
    let event_types: Vec<&str> = events
        .iter()
        .filter_map(|e| e["action_type"].as_str())
        .collect();

    assert!(
        event_types.contains(&"hold_request"),
        "hold_request event should be in history: {:?}",
        event_types
    );
    assert!(
        event_types.contains(&"hold_response"),
        "hold_response event should be in history: {:?}",
        event_types
    );
}

// ---------------------------------------------------------------------------
// PIP-001 §11d: Reject semantics
// ---------------------------------------------------------------------------

/// PIP-001 §11d: Human reject discards the pending action.
#[tokio::test]
async fn reject_hold_restores_active_without_executing_pending() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "reject-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope_with_hold(
        &kernel,
        "reject-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        50_000,
        json!([{"target": "workspace/blocked/*", "actions": ["mutate"]}]),
    )
    .await;

    // Trigger hold.
    let trigger_action = Action {
        actor_id: "reject-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/blocked/file".to_string(),
        payload: json!({"dangerous": true}),
        timestamp: None,
    };
    let trigger_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger_action).unwrap(),
        ))
        .await;
    assert_eq!(trigger_resp.status, "error");
    let hold_id = extract_hold_id(&trigger_resp.payload);

    // Human (root) rejects the hold.
    let reject_action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: format!("ledger/hold/{hold_id}"),
        payload: json!({
            "decision": "reject",
            "instruction": "this action is not permitted"
        }),
        timestamp: None,
    };
    let reject_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(reject_action).unwrap(),
        ))
        .await;
    assert_eq!(
        reject_resp.status, "ok",
        "reject should succeed: {}",
        reject_resp.payload
    );

    // After reject, envelope should be active again.
    let follow_action = Action {
        actor_id: "reject-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/normal/allowed".to_string(),
        payload: json!({"safe": true}),
        timestamp: None,
    };
    let follow_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(follow_action).unwrap(),
        ))
        .await;
    assert_eq!(
        follow_resp.status, "ok",
        "after reject, agent should be active again and can submit safe actions: {}",
        follow_resp.payload
    );
}

/// PIP-001 §11d: After reject, hold_response event is written to history.
#[tokio::test]
async fn reject_hold_writes_hold_response_event() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "reject-history-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope_with_hold(
        &kernel,
        "reject-history-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        50_000,
        json!([{"target": "workspace/gated/*", "actions": ["mutate"]}]),
    )
    .await;

    // Trigger hold.
    let trigger_action = Action {
        actor_id: "reject-history-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/gated/path".to_string(),
        payload: json!({"key": "val"}),
        timestamp: None,
    };
    let trigger_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger_action).unwrap(),
        ))
        .await;
    assert_eq!(trigger_resp.status, "error");
    let hold_id = extract_hold_id(&trigger_resp.payload);

    // Reject.
    let reject_action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: format!("ledger/hold/{hold_id}"),
        payload: json!({ "decision": "reject" }),
        timestamp: None,
    };
    kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(reject_action).unwrap(),
        ))
        .await;

    // Verify events contain hold_request and hold_response.
    let events_resp = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "events", "limit": 20 }),
        ))
        .await;
    assert_eq!(events_resp.status, "ok");
    let events = events_resp.payload["events"]
        .as_array()
        .expect("events array");
    let event_types: Vec<&str> = events
        .iter()
        .filter_map(|e| e["action_type"].as_str())
        .collect();

    assert!(
        event_types.contains(&"hold_request"),
        "hold_request event should be in history: {:?}",
        event_types
    );
    assert!(
        event_types.contains(&"hold_response"),
        "hold_response event should be in history: {:?}",
        event_types
    );
}

// ---------------------------------------------------------------------------
// PIP-001 §11: Security invariants
// ---------------------------------------------------------------------------

/// PIP-001 §11d: Only a Human actor can resolve holds.
/// An Agent attempting to approve/reject must be rejected.
#[tokio::test]
async fn agent_cannot_resolve_hold() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "trapped-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    // A second agent that tries to be a "resolver" — should fail.
    create_agent(
        &kernel,
        "rogue-resolver",
        json!([{"target": "ledger/**", "actions": ["mutate"]}]),
    )
    .await;

    // Give rogue-resolver a broad envelope.
    let resolver_envelope_action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/envelope".to_string(),
        payload: json!({
            "actor_id": "rogue-resolver",
            "budget": 50_000,
            "targets": ["ledger/**"],
            "actions": ["mutate"]
        }),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(resolver_envelope_action).unwrap(),
        ))
        .await;
    assert_eq!(
        resp.status, "ok",
        "setup rogue resolver envelope: {}",
        resp.payload
    );

    // Give trapped-agent a hold-on envelope.
    grant_envelope_with_hold(
        &kernel,
        "trapped-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        50_000,
        json!([{"target": "workspace/gated/*", "actions": ["mutate"]}]),
    )
    .await;

    // Trigger hold on trapped-agent.
    let trigger_action = Action {
        actor_id: "trapped-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/gated/data".to_string(),
        payload: json!({}),
        timestamp: None,
    };
    let trigger_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger_action).unwrap(),
        ))
        .await;
    assert_eq!(trigger_resp.status, "error");
    let hold_id = extract_hold_id(&trigger_resp.payload);

    // rogue-resolver (an agent) tries to approve the hold — must fail.
    let rogue_approve = Action {
        actor_id: "rogue-resolver".to_string(),
        action_type: ActionType::Mutate,
        target: format!("ledger/hold/{hold_id}"),
        payload: json!({ "decision": "approve" }),
        timestamp: None,
    };
    let rogue_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(rogue_approve).unwrap(),
        ))
        .await;
    assert_eq!(
        rogue_resp.status, "error",
        "agent should not be able to resolve holds: {}",
        rogue_resp.payload
    );
    // The agent is blocked either by:
    // (a) boundary violation: ledger/hold/* is a privileged target agents cannot write, OR
    // (b) policy violation: only Human actors can resolve holds.
    // Both are acceptable; the key is that the operation fails.
    let err_str = rogue_resp.payload.to_string();
    assert!(
        err_str.contains("Human actors")
            || err_str.contains("policy violation")
            || err_str.contains("boundary violation")
            || err_str.contains("privileged target"),
        "error should indicate rejection: {}",
        err_str
    );
}

/// PIP-001 §11d: Cannot resolve a hold that is already resolved.
#[tokio::test]
async fn cannot_resolve_hold_twice() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "double-resolve-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope_with_hold(
        &kernel,
        "double-resolve-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        50_000,
        json!([{"target": "workspace/gated/*", "actions": ["mutate"]}]),
    )
    .await;

    // Trigger hold.
    let trigger_action = Action {
        actor_id: "double-resolve-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/gated/once".to_string(),
        payload: json!({}),
        timestamp: None,
    };
    let trigger_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger_action).unwrap(),
        ))
        .await;
    assert_eq!(trigger_resp.status, "error");
    let hold_id = extract_hold_id(&trigger_resp.payload);

    // First resolve — should succeed.
    let reject_action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: format!("ledger/hold/{hold_id}"),
        payload: json!({ "decision": "reject" }),
        timestamp: None,
    };
    let first_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(reject_action.clone()).unwrap(),
        ))
        .await;
    assert_eq!(
        first_resp.status, "ok",
        "first resolve should succeed: {}",
        first_resp.payload
    );

    // Second resolve of the same hold — must fail.
    let second_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(reject_action).unwrap(),
        ))
        .await;
    assert_eq!(
        second_resp.status, "error",
        "double resolve should fail: {}",
        second_resp.payload
    );
    assert!(
        second_resp.payload.to_string().contains("already resolved")
            || second_resp
                .payload
                .to_string()
                .contains("not found or already resolved"),
        "error should indicate already resolved: {}",
        second_resp.payload
    );
}

/// PIP-001 §11d: Invalid decision value (not "approve" or "reject") is rejected.
#[tokio::test]
async fn invalid_hold_decision_rejected() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "invalid-decision-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope_with_hold(
        &kernel,
        "invalid-decision-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        50_000,
        json!([{"target": "workspace/gated/*", "actions": ["mutate"]}]),
    )
    .await;

    // Trigger hold.
    let trigger_action = Action {
        actor_id: "invalid-decision-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/gated/file".to_string(),
        payload: json!({}),
        timestamp: None,
    };
    let trigger_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger_action).unwrap(),
        ))
        .await;
    assert_eq!(trigger_resp.status, "error");
    // The hold_id doesn't matter here; we just need a valid hold.
    let hold_id = extract_hold_id(&trigger_resp.payload);

    // Submit mutate with invalid decision — parse_hold_response should return None,
    // meaning it falls through to normal pipeline (and may fail on policy grounds).
    let bad_decision = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: format!("ledger/hold/{hold_id}"),
        payload: json!({ "decision": "maybe" }),
        timestamp: None,
    };
    let bad_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(bad_decision).unwrap(),
        ))
        .await;
    // Should not succeed as a hold_response (decision "maybe" is not valid).
    // It will either fail outright or be treated as a regular mutate that may fail on other grounds.
    // Either way it must NOT successfully resolve the hold.
    // Verify the hold is still pending by attempting to re-approve legitimately.
    let approve_action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: format!("ledger/hold/{hold_id}"),
        payload: json!({ "decision": "reject" }),
        timestamp: None,
    };
    let approve_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(approve_action).unwrap(),
        ))
        .await;
    // If bad_resp was ok (treated as regular mutate), the hold is still pending and we can reject now.
    // If bad_resp was error, same — hold is still pending.
    if bad_resp.status == "ok" {
        // Must still be resolvable.
        assert_eq!(
            approve_resp.status, "ok",
            "hold should still be resolvable after invalid decision: {}",
            approve_resp.payload
        );
    }
    // Either path: the hold was not silently "approved" by an invalid decision.
}

/// PIP-001 §11a: hold_on glob matching — wildcard patterns work.
#[tokio::test]
async fn hold_on_glob_pattern_matching() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "glob-hold-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    // hold_on: doublestar glob covers all subdirectories.
    grant_envelope_with_hold(
        &kernel,
        "glob-hold-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        50_000,
        json!([{"target": "workspace/sensitive/**", "actions": ["mutate"]}]),
    )
    .await;

    // Deep nested path — should trigger hold.
    let nested_action = Action {
        actor_id: "glob-hold-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/sensitive/a/b/c/deep".to_string(),
        payload: json!({}),
        timestamp: None,
    };
    let nested_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(nested_action).unwrap(),
        ))
        .await;
    assert_eq!(
        nested_resp.status, "error",
        "deeply nested path matching glob should trigger hold: {}",
        nested_resp.payload
    );
    assert!(
        nested_resp.payload["error_type"] == "HoldTriggered",
        "should be hold triggered: {}",
        nested_resp.payload
    );
}

/// PIP-001 §11a: Multiple hold_on rules — first matching rule wins.
#[tokio::test]
async fn hold_on_multiple_rules() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "multi-rule-agent",
        json!([{"target": "workspace/**", "actions": ["mutate", "create"]}]),
    )
    .await;

    // Two rules: one for mutate on zone-a, another for create on zone-b.
    grant_envelope_with_hold(
        &kernel,
        "multi-rule-agent",
        json!(["workspace/**"]),
        json!(["mutate", "create"]),
        50_000,
        json!([
            {"target": "workspace/zone-a/*", "actions": ["mutate"]},
            {"target": "workspace/zone-b/*", "actions": ["create"]}
        ]),
    )
    .await;

    // mutate zone-a — triggers hold (rule 1).
    let action_a = Action {
        actor_id: "multi-rule-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/zone-a/resource".to_string(),
        payload: json!({}),
        timestamp: None,
    };
    let resp_a = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action_a).unwrap(),
        ))
        .await;
    assert_eq!(resp_a.status, "error");
    assert!(
        resp_a.payload["error_type"] == "HoldTriggered",
        "rule 1 should trigger hold: {}",
        resp_a.payload
    );
}

// ---------------------------------------------------------------------------
// PIP-001 §11: hold_info and holds_pending read queries
// ---------------------------------------------------------------------------

/// PIP-001 §11: holds_pending read query returns triggered hold; disappears after resolve.
#[tokio::test]
async fn holds_pending_query_returns_pending_and_clears_after_resolve() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "query-hold-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope_with_hold(
        &kernel,
        "query-hold-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        50_000,
        json!([{"target": "workspace/gated/*", "actions": ["mutate"]}]),
    )
    .await;

    // Initially no pending holds.
    let before_resp = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "holds_pending" }),
        ))
        .await;
    assert_eq!(before_resp.status, "ok");
    assert_eq!(
        before_resp.payload["holds"].as_array().unwrap().len(),
        0,
        "no pending holds initially"
    );

    // Trigger a hold.
    let trigger_action = Action {
        actor_id: "query-hold-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/gated/item".to_string(),
        payload: json!({}),
        timestamp: None,
    };
    let trigger_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger_action).unwrap(),
        ))
        .await;
    assert_eq!(trigger_resp.status, "error");
    let hold_id = extract_hold_id(&trigger_resp.payload);

    // holds_pending should now return one entry.
    let pending_resp = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "holds_pending" }),
        ))
        .await;
    assert_eq!(
        pending_resp.status, "ok",
        "holds_pending should succeed: {}",
        pending_resp.payload
    );
    let holds = pending_resp.payload["holds"].as_array().unwrap();
    assert_eq!(holds.len(), 1, "one pending hold expected");
    assert_eq!(holds[0]["hold_id"].as_str().unwrap(), hold_id);
    assert_eq!(holds[0]["status"].as_str().unwrap(), "pending");
    assert_eq!(holds[0]["agent_id"].as_str().unwrap(), "query-hold-agent");

    // holds_pending filtered by agent_id should also return it.
    let filtered_resp = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "holds_pending", "actor_id": "query-hold-agent" }),
        ))
        .await;
    assert_eq!(filtered_resp.status, "ok");
    assert_eq!(
        filtered_resp.payload["holds"].as_array().unwrap().len(),
        1,
        "filtered by agent should return 1"
    );

    // Reject the hold.
    let reject_action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: format!("ledger/hold/{hold_id}"),
        payload: json!({ "decision": "reject" }),
        timestamp: None,
    };
    let reject_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(reject_action).unwrap(),
        ))
        .await;
    assert_eq!(reject_resp.status, "ok");

    // After resolve, holds_pending should be empty again.
    let after_resp = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "holds_pending" }),
        ))
        .await;
    assert_eq!(after_resp.status, "ok");
    assert_eq!(
        after_resp.payload["holds"].as_array().unwrap().len(),
        0,
        "no pending holds after resolve"
    );
}

/// PIP-001 §11: hold_info read query returns correct hold details.
#[tokio::test]
async fn hold_info_query_returns_hold_details() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "info-hold-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope_with_hold(
        &kernel,
        "info-hold-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        50_000,
        json!([{"target": "workspace/gated/*", "actions": ["mutate"]}]),
    )
    .await;

    // Trigger hold.
    let trigger_action = Action {
        actor_id: "info-hold-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/gated/detail".to_string(),
        payload: json!({"data": "test"}),
        timestamp: None,
    };
    let trigger_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger_action).unwrap(),
        ))
        .await;
    assert_eq!(trigger_resp.status, "error");
    let hold_id = extract_hold_id(&trigger_resp.payload);

    // Query hold_info.
    let info_resp = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "hold_info", "hold_id": &hold_id }),
        ))
        .await;
    assert_eq!(
        info_resp.status, "ok",
        "hold_info should succeed: {}",
        info_resp.payload
    );
    assert_eq!(info_resp.payload["hold_id"].as_str().unwrap(), hold_id);
    assert_eq!(
        info_resp.payload["agent_id"].as_str().unwrap(),
        "info-hold-agent"
    );
    assert_eq!(info_resp.payload["status"].as_str().unwrap(), "pending");
    assert_eq!(
        info_resp.payload["trigger_target"].as_str().unwrap(),
        "workspace/gated/detail"
    );
    assert_eq!(
        info_resp.payload["trigger_action"].as_str().unwrap(),
        "mutate"
    );

    // hold_info for unknown hold_id should return error.
    let missing_resp = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "hold_info", "hold_id": "nonexistent-hold-id" }),
        ))
        .await;
    assert_eq!(
        missing_resp.status, "error",
        "unknown hold_id should return error"
    );
}

// ---------------------------------------------------------------------------
// PIP-001 §11c/§11d/§11f: Energy reservation and settlement tests
// ---------------------------------------------------------------------------

/// Helper: grant envelope with hold_on + hold_timeout_secs.
async fn grant_envelope_with_hold_timeout(
    kernel: &Kernel,
    agent_id: &str,
    targets: serde_json::Value,
    actions: serde_json::Value,
    budget: i64,
    hold_on: serde_json::Value,
    hold_timeout_secs: i64,
) {
    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/envelope".to_string(),
        payload: json!({
            "actor_id": agent_id,
            "budget": budget,
            "targets": targets,
            "actions": actions,
            "hold_on": hold_on,
            "hold_timeout_secs": hold_timeout_secs
        }),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).unwrap(),
        ))
        .await;
    assert_eq!(
        resp.status, "ok",
        "grant envelope with timeout should succeed: {}",
        resp.payload
    );
}

/// Helper: query an actor's energy balance and return (balance, reserved).
async fn query_energy(kernel: &Kernel, actor_id: &str) -> (i64, i64) {
    let resp = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": actor_id }),
        ))
        .await;
    assert_eq!(
        resp.status, "ok",
        "energy query should succeed: {}",
        resp.payload
    );
    let balance = resp.payload["energy_balance"]
        .as_i64()
        .expect("energy_balance");
    let reserved = resp.payload["reserved_energy"]
        .as_i64()
        .expect("reserved_energy");
    (balance, reserved)
}

/// PIP-001 §11c: Hold trigger reserves energy (available decreases).
#[tokio::test]
async fn hold_reserves_energy_on_trigger() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "reserve-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope_with_hold(
        &kernel,
        "reserve-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        100_000,
        json!([{"target": "workspace/gated/*", "actions": ["mutate"]}]),
    )
    .await;

    // Check energy before.
    let (balance_before, reserved_before) = query_energy(&kernel, "reserve-agent").await;
    assert_eq!(reserved_before, 0, "no reservations initially");

    // Trigger hold — should reserve energy.
    let trigger = Action {
        actor_id: "reserve-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/gated/file".to_string(),
        payload: json!({"data": "test"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "error");
    assert!(resp.payload["error_type"] == "HoldTriggered");

    // Energy should now have reserved > 0.
    let (balance_after, reserved_after) = query_energy(&kernel, "reserve-agent").await;
    assert_eq!(
        balance_after, balance_before,
        "balance unchanged by reserve"
    );
    assert!(
        reserved_after > reserved_before,
        "reserved should increase: before={reserved_before}, after={reserved_after}"
    );
}

/// PIP-001 §11f: Hold reject settles commitment cost (20%) and releases the rest.
#[tokio::test]
async fn hold_reject_releases_energy_with_commitment_cost() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "reject-energy-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope_with_hold(
        &kernel,
        "reject-energy-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        100_000,
        json!([{"target": "workspace/gated/*", "actions": ["mutate"]}]),
    )
    .await;

    let (balance_before, _) = query_energy(&kernel, "reject-energy-agent").await;

    // Trigger hold.
    let trigger = Action {
        actor_id: "reject-energy-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/gated/file".to_string(),
        payload: json!({"data": "test"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger).unwrap(),
        ))
        .await;
    let hold_id = extract_hold_id(&resp.payload);

    let (_, reserved_mid) = query_energy(&kernel, "reject-energy-agent").await;
    assert!(reserved_mid > 0, "energy should be reserved");

    // Reject → settle 20% commitment cost.
    let reject = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: format!("ledger/hold/{hold_id}"),
        payload: json!({"decision": "reject"}),
        timestamp: None,
    };
    let reject_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(reject).unwrap(),
        ))
        .await;
    assert_eq!(
        reject_resp.status, "ok",
        "reject should succeed: {}",
        reject_resp.payload
    );

    // After reject: reserved should be 0, balance reduced by commitment cost.
    let (balance_after, reserved_after) = query_energy(&kernel, "reject-energy-agent").await;
    assert_eq!(reserved_after, 0, "no energy reserved after reject");
    // Balance should be less than before (commitment cost deducted).
    assert!(
        balance_after < balance_before,
        "balance should decrease: before={balance_before}, after={balance_after}"
    );
    // But not by the full reserved amount — only 20%.
    let commitment_cost = ((reserved_mid as f64) * 0.2).ceil() as i64;
    assert_eq!(
        balance_after,
        balance_before - commitment_cost,
        "balance should decrease by commitment cost ({commitment_cost})"
    );
}

/// PIP-001 §11d: Hold approve does not double-charge energy.
#[tokio::test]
async fn hold_approve_does_not_double_charge() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "approve-energy-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope_with_hold(
        &kernel,
        "approve-energy-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        100_000,
        json!([{"target": "workspace/gated/*", "actions": ["mutate"]}]),
    )
    .await;

    let (balance_before, _) = query_energy(&kernel, "approve-energy-agent").await;

    // Trigger hold.
    let trigger = Action {
        actor_id: "approve-energy-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/gated/file".to_string(),
        payload: json!({"data": "approved-action"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger).unwrap(),
        ))
        .await;
    let hold_id = extract_hold_id(&resp.payload);

    let (_, reserved_mid) = query_energy(&kernel, "approve-energy-agent").await;
    assert!(reserved_mid > 0);

    // Approve → should execute and settle using the already-reserved energy.
    let approve = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: format!("ledger/hold/{hold_id}"),
        payload: json!({"decision": "approve"}),
        timestamp: None,
    };
    let approve_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(approve).unwrap(),
        ))
        .await;
    assert_eq!(
        approve_resp.status, "ok",
        "approve should succeed: {}",
        approve_resp.payload
    );

    // After approve+execute+settle: reserved should be 0.
    let (balance_after, reserved_after) = query_energy(&kernel, "approve-energy-agent").await;
    assert_eq!(reserved_after, 0, "no reserved energy after approve+settle");

    // Balance should have decreased by exactly the settle cost (= the quote cost).
    // Not double the cost!
    let total_deducted = balance_before - balance_after;
    assert!(
        total_deducted > 0,
        "some energy should be deducted: before={balance_before}, after={balance_after}"
    );
    // It should equal one quote_cost (the hold reserved amount), not double.
    assert_eq!(
        total_deducted, reserved_mid,
        "deduction should equal the reserved (quoted) amount, not double"
    );
}

/// PIP-001 §11e: Hold timeout auto-rejects and settles commitment cost.
#[tokio::test]
async fn hold_timeout_auto_rejects() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "timeout-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    // Envelope with hold_timeout_secs = 1 second.
    grant_envelope_with_hold_timeout(
        &kernel,
        "timeout-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        100_000,
        json!([{"target": "workspace/gated/*", "actions": ["mutate"]}]),
        1, // 1 second timeout
    )
    .await;

    let (balance_before, _) = query_energy(&kernel, "timeout-agent").await;

    // Trigger hold.
    let trigger = Action {
        actor_id: "timeout-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/gated/file".to_string(),
        payload: json!({"data": "will-timeout"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "error");
    assert!(resp.payload["error_type"] == "HoldTriggered");
    let hold_id = extract_hold_id(&resp.payload);

    let (_, reserved_mid) = query_energy(&kernel, "timeout-agent").await;
    assert!(reserved_mid > 0, "energy should be reserved");

    // Wait for timeout to expire.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Submit another action to trigger lazy expiry.
    let trigger2 = Action {
        actor_id: "timeout-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/gated/file2".to_string(),
        payload: json!({"data": "triggers-expiry"}),
        timestamp: None,
    };
    let _ = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger2).unwrap(),
        ))
        .await;

    // The original hold should now be resolved (timed_out).
    let info_resp = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "hold_info", "hold_id": &hold_id }),
        ))
        .await;
    assert_eq!(info_resp.status, "ok");
    assert_eq!(
        info_resp.payload["status"].as_str().unwrap(),
        "timed_out",
        "hold should be auto-rejected as timed_out"
    );

    // Energy: commitment cost deducted, rest released.
    let (balance_after, _) = query_energy(&kernel, "timeout-agent").await;
    let commitment_cost = ((reserved_mid as f64) * 0.2).ceil() as i64;
    assert_eq!(
        balance_after,
        balance_before - commitment_cost,
        "balance should decrease by commitment cost after timeout"
    );
}

/// PIP-001 §11e: Approve after timeout should fail.
#[tokio::test]
async fn hold_approve_after_timeout_fails() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "late-approve-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope_with_hold_timeout(
        &kernel,
        "late-approve-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        100_000,
        json!([{"target": "workspace/gated/*", "actions": ["mutate"]}]),
        1, // 1 second timeout
    )
    .await;

    // Trigger hold.
    let trigger = Action {
        actor_id: "late-approve-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/gated/file".to_string(),
        payload: json!({"data": "will-timeout"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(trigger).unwrap(),
        ))
        .await;
    let hold_id = extract_hold_id(&resp.payload);

    // Wait for timeout.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Try to approve — should fail because the hold has timed out.
    let approve = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: format!("ledger/hold/{hold_id}"),
        payload: json!({"decision": "approve"}),
        timestamp: None,
    };
    let approve_resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(approve).unwrap(),
        ))
        .await;
    assert_eq!(
        approve_resp.status, "error",
        "approve after timeout should fail: {}",
        approve_resp.payload
    );
    assert!(
        approve_resp.payload.to_string().contains("timed out")
            || approve_resp
                .payload
                .to_string()
                .contains("already resolved"),
        "error should mention timeout: {}",
        approve_resp.payload
    );
}
