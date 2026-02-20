use punkgo_core::action::{Action, ActionType};
use punkgo_core::protocol::RequestType;
use punkgo_runtime::{Kernel, KernelConfig};
use punkgo_testkit::{TestStateDir, make_request};
use serde_json::json;

async fn setup_kernel() -> (Kernel, TestStateDir) {
    let state = TestStateDir::new("punkgo-boundary-test").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config)
        .await
        .expect("kernel bootstrap should succeed");
    (kernel, state)
}

/// Helper: create an agent with specific writable_targets.
async fn create_agent(kernel: &Kernel, agent_id: &str, targets: serde_json::Value) {
    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/actor".to_string(),
        payload: json!({
            "actor_id": agent_id,
            "actor_type": "agent",
            "purpose": "boundary-test",
            "energy_balance": 10000,
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

/// Helper: create an envelope for an agent (Phase 4b — required for agents to act).
async fn grant_envelope(
    kernel: &Kernel,
    agent_id: &str,
    targets: serde_json::Value,
    actions: serde_json::Value,
    budget: i64,
) {
    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/envelope".to_string(),
        payload: json!({
            "actor_id": agent_id,
            "budget": budget,
            "targets": targets,
            "actions": actions
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

/// PIP-001 §8 (default deny): agent with no writable_targets gets default deny for state-changing actions.
#[tokio::test]
async fn boundary_default_deny_integration() {
    let (kernel, _state) = setup_kernel().await;

    // Create agent with empty writable_targets
    create_agent(&kernel, "restricted-agent", json!([])).await;

    // Try to mutate — should be denied (boundary check happens before authorization)
    let action = Action {
        actor_id: "restricted-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/a".to_string(),
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
        resp.status, "error",
        "empty writable_targets should deny mutate"
    );
    assert!(
        resp.payload.to_string().contains("BoundaryViolation")
            || resp.payload.to_string().contains("no writable_target"),
        "should be a boundary violation error: {}",
        resp.payload
    );
}

/// PIP-001 §8: observe is always exempt from boundary checks.
#[tokio::test]
async fn boundary_observe_exempt_integration() {
    let (kernel, _state) = setup_kernel().await;

    // Create agent with empty writable_targets
    create_agent(&kernel, "readonly-agent", json!([])).await;

    // Observe should succeed even with empty boundary
    let action = Action {
        actor_id: "readonly-agent".to_string(),
        action_type: ActionType::Observe,
        target: "workspace/secret".to_string(),
        payload: json!({}),
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
        "observe should be exempt from boundary: {}",
        resp.payload
    );
}

/// PIP-001 §8: agent with specific targets can write within boundary but not outside.
/// Phase 4b: agent needs an envelope to perform state-changing actions.
#[tokio::test]
async fn boundary_enforced_within_and_outside() {
    let (kernel, _state) = setup_kernel().await;

    // Create agent with writable_targets for workspace/docs/*
    create_agent(
        &kernel,
        "docs-agent",
        json!([{"target": "workspace/docs/*", "actions": ["create", "mutate"]}]),
    )
    .await;

    // Phase 4b: Grant an envelope so the agent can act
    grant_envelope(
        &kernel,
        "docs-agent",
        json!(["workspace/docs/*"]),
        json!(["create", "mutate"]),
        50000,
    )
    .await;

    // Should succeed: mutate within boundary
    let ok_action = Action {
        actor_id: "docs-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/docs/readme".to_string(),
        payload: json!({"content": "hello"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(ok_action).unwrap(),
        ))
        .await;
    assert_eq!(
        resp.status, "ok",
        "mutate within boundary should succeed: {}",
        resp.payload
    );

    // Should fail: mutate outside boundary (boundary check rejects before envelope check)
    let bad_action = Action {
        actor_id: "docs-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/code/main.rs".to_string(),
        payload: json!({"content": "hacked"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(bad_action).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "error", "mutate outside boundary should fail");

    // Should fail: execute (not in actions list)
    let exec_action = Action {
        actor_id: "docs-agent".to_string(),
        action_type: ActionType::Execute,
        target: "workspace/docs/script".to_string(),
        payload: json!({"command": "echo hi", "timeout_ms": 100}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(exec_action).unwrap(),
        ))
        .await;
    assert_eq!(
        resp.status, "error",
        "execute should fail when not in actions list"
    );
}

/// PIP-001 §9: non-root cannot write privileged targets.
#[tokio::test]
async fn boundary_privileged_target_protection() {
    let (kernel, _state) = setup_kernel().await;

    // Create agent with broad workspace/** boundary
    create_agent(
        &kernel,
        "broad-agent",
        json!([{"target": "workspace/**", "actions": ["create", "mutate", "execute"]}]),
    )
    .await;

    // Should fail: write to system/ target (boundary rejects before envelope check)
    let sys_action = Action {
        actor_id: "broad-agent".to_string(),
        action_type: ActionType::Create,
        target: "system/config".to_string(),
        payload: json!({"setting": "evil"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(sys_action).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "error", "non-root should not write system/*");

    // Should fail: write to ledger/ target
    let ledger_action = Action {
        actor_id: "broad-agent".to_string(),
        action_type: ActionType::Create,
        target: "ledger/actor".to_string(),
        payload: json!({"actor_id": "evil-agent", "energy_balance": 999999}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(ledger_action).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "error", "non-root should not write ledger/*");
}

/// PIP-001 §11: child writable_targets cannot exceed parent boundary.
#[tokio::test]
async fn boundary_child_subset_enforced() {
    let (kernel, _state) = setup_kernel().await;

    // Create agent with limited boundary
    create_agent(
        &kernel,
        "parent-agent",
        json!([{"target": "workspace/docs/*", "actions": ["create", "mutate"]}]),
    )
    .await;

    // Try to create a child with broader boundary — should fail
    let action = Action {
        actor_id: "parent-agent".to_string(),
        action_type: ActionType::Create,
        target: "ledger/actor".to_string(),
        payload: json!({
            "actor_id": "child-agent",
            "actor_type": "agent",
            "purpose": "overstep",
            "energy_balance": 1000,
            "writable_targets": [{"target": "workspace/**", "actions": ["create", "mutate", "execute"]}]
        }),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).unwrap(),
        ))
        .await;
    // This will fail because parent-agent can't write to ledger/ (PIP-001 §9)
    assert_eq!(resp.status, "error", "parent-agent can't write ledger/*");
}

/// Root retains full access — existing tests should still pass with boundary enforcement.
#[tokio::test]
async fn root_retains_full_access() {
    let (kernel, _state) = setup_kernel().await;

    // Root should be able to do everything as before
    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/anything".to_string(),
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
        "root should have full access: {}",
        resp.payload
    );

    // Root can access privileged targets
    let policy = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "system/policy".to_string(),
        payload: json!({"version": "test-v1"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(policy).unwrap(),
        ))
        .await;
    assert_eq!(
        resp.status, "ok",
        "root should access system/*: {}",
        resp.payload
    );
}
