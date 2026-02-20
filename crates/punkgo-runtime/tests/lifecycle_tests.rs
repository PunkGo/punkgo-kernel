use punkgo_core::action::{Action, ActionType};
use punkgo_core::protocol::RequestType;
use punkgo_runtime::{Kernel, KernelConfig};
use punkgo_testkit::{TestStateDir, make_request};
use serde_json::json;

async fn setup_kernel() -> (Kernel, TestStateDir) {
    let state = TestStateDir::new("punkgo-lifecycle-test").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config)
        .await
        .expect("kernel bootstrap should succeed");
    (kernel, state)
}

async fn create_agent(kernel: &Kernel, agent_id: &str, targets: serde_json::Value) {
    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/actor".to_string(),
        payload: json!({
            "actor_id": agent_id,
            "actor_type": "agent",
            "purpose": "lifecycle-test",
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

/// Helper: grant an envelope to an agent (Phase 4b — required for agents to act).
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

/// PIP-001 §5/§6 (lifecycle): Freezing blocks state-changing actions.
#[tokio::test]
async fn freeze_blocks_state_changes() {
    let (kernel, _state) = setup_kernel().await;

    // Create an agent
    create_agent(
        &kernel,
        "test-agent",
        json!([{"target": "workspace/**", "actions": ["create", "mutate"]}]),
    )
    .await;

    // Phase 4b: Grant envelope so agent can act
    grant_envelope(
        &kernel,
        "test-agent",
        json!(["workspace/**"]),
        json!(["create", "mutate"]),
        50000,
    )
    .await;

    // Verify agent can mutate before freeze
    let action = Action {
        actor_id: "test-agent".to_string(),
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
        resp.status, "ok",
        "agent should be able to mutate before freeze: {}",
        resp.payload
    );

    // Root freezes the agent
    let freeze_action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: "actor/test-agent".to_string(),
        payload: json!({"op": "freeze", "reason": "testing"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(freeze_action).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "ok", "freeze should succeed: {}", resp.payload);

    // Verify agent cannot mutate after freeze
    let action = Action {
        actor_id: "test-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/a".to_string(),
        payload: json!({"k": "v2"}),
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
        "frozen agent should not be able to mutate"
    );
    assert!(
        resp.payload.to_string().contains("frozen"),
        "error should mention frozen status: {}",
        resp.payload
    );

    // But observe should still work
    let observe = Action {
        actor_id: "test-agent".to_string(),
        action_type: ActionType::Observe,
        target: "workspace/a".to_string(),
        payload: json!({}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(observe).unwrap(),
        ))
        .await;
    assert_eq!(
        resp.status, "ok",
        "frozen agent should still observe: {}",
        resp.payload
    );
}

/// PIP-001 §5: Agent cannot create another Agent.
#[tokio::test]
async fn agent_cannot_create_agent() {
    let (kernel, _state) = setup_kernel().await;

    // Create an agent with full access (including ledger/)
    create_agent(
        &kernel,
        "parent-agent",
        json!([{"target": "**", "actions": ["create", "mutate", "execute"]}]),
    )
    .await;

    // Grant envelope so agent can attempt creation
    grant_envelope(
        &kernel,
        "parent-agent",
        json!(["**"]),
        json!(["create", "mutate", "execute"]),
        50000,
    )
    .await;

    // Agent tries to create another agent — should be rejected (PIP-001 §5)
    let create_child = Action {
        actor_id: "parent-agent".to_string(),
        action_type: ActionType::Create,
        target: "ledger/actor".to_string(),
        payload: json!({
            "actor_id": "parent-agent/child/1",
            "actor_type": "agent",
            "purpose": "child",
            "energy_balance": 5000,
            "writable_targets": [{"target": "workspace/child/**", "actions": ["mutate"]}]
        }),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(create_child).unwrap(),
        ))
        .await;
    assert_eq!(
        resp.status, "error",
        "agent creating agent should be rejected"
    );
    assert!(
        resp.payload.to_string().contains("PIP-001"),
        "error should reference PIP-001: {}",
        resp.payload
    );
}

/// PIP-001 §5/§6 (lifecycle): Human creates multiple agents; freezing one does not affect siblings.
/// Agent lineage is always [root], so freezing agent-a doesn't cascade to agent-b.
#[tokio::test]
async fn freeze_agent_does_not_cascade_to_siblings() {
    let (kernel, _state) = setup_kernel().await;

    // Root creates two agents
    create_agent(
        &kernel,
        "agent-a",
        json!([{"target": "workspace/a/**", "actions": ["mutate"]}]),
    )
    .await;
    create_agent(
        &kernel,
        "agent-b",
        json!([{"target": "workspace/b/**", "actions": ["mutate"]}]),
    )
    .await;

    // Verify both are active
    let info_a = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_info", "actor_id": "agent-a" }),
        ))
        .await;
    assert_eq!(info_a.payload["status"], "active");

    // Freeze agent-a
    let freeze = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: "actor/agent-a".to_string(),
        payload: json!({"op": "freeze", "reason": "test isolation"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(freeze).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "ok", "freeze should succeed");

    // Verify agent-a is frozen
    let info_a = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_info", "actor_id": "agent-a" }),
        ))
        .await;
    assert_eq!(info_a.payload["status"], "frozen");

    // Verify agent-b is NOT frozen (siblings are independent)
    let info_b = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_info", "actor_id": "agent-b" }),
        ))
        .await;
    assert_eq!(
        info_b.payload["status"], "active",
        "sibling agent should remain active when another is frozen"
    );
}

/// PIP-001 §5/§6 (lifecycle): Unfreeze restores active status (does not cascade).
#[tokio::test]
async fn unfreeze_restores_active() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "freeze-target",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    // Freeze
    let freeze = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: "actor/freeze-target".to_string(),
        payload: json!({"op": "freeze"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(freeze).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "ok");

    // Verify frozen
    let info = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_info", "actor_id": "freeze-target" }),
        ))
        .await;
    assert_eq!(info.payload["status"], "frozen");

    // Unfreeze
    let unfreeze = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: "actor/freeze-target".to_string(),
        payload: json!({"op": "unfreeze"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(unfreeze).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "ok", "unfreeze should succeed");

    // Verify active again
    let info = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_info", "actor_id": "freeze-target" }),
        ))
        .await;
    assert_eq!(
        info.payload["status"], "active",
        "should be active after unfreeze"
    );
}

/// PIP-001 §5: agent cannot freeze any other agent (not just siblings).
#[tokio::test]
async fn agent_cannot_freeze_sibling() {
    let (kernel, _state) = setup_kernel().await;

    // Create two sibling agents
    create_agent(
        &kernel,
        "sibling-a",
        json!([{"target": "workspace/a/**", "actions": ["mutate"]}]),
    )
    .await;
    create_agent(
        &kernel,
        "sibling-b",
        json!([{"target": "workspace/b/**", "actions": ["mutate"]}]),
    )
    .await;

    // sibling-a tries to freeze sibling-b — should fail
    // Note: sibling-a can't write to actor/* (PIP-001 §9 privileged target)
    let freeze = Action {
        actor_id: "sibling-a".to_string(),
        action_type: ActionType::Mutate,
        target: "actor/sibling-b".to_string(),
        payload: json!({"op": "freeze"}),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(freeze).unwrap(),
        ))
        .await;
    assert_eq!(
        resp.status, "error",
        "sibling should not be able to freeze sibling"
    );
}

/// PIP-001 §7: Agent with active human creator can perform state-changing actions.
#[tokio::test]
async fn agent_with_active_creator_can_act() {
    let (kernel, _state) = setup_kernel().await;

    // Root (human) creates an agent
    create_agent(
        &kernel,
        "worker",
        json!([{"target": "workspace/worker/**", "actions": ["mutate"]}]),
    )
    .await;

    // Grant envelope so agent can act
    grant_envelope(
        &kernel,
        "worker",
        json!(["workspace/worker/**"]),
        json!(["mutate"]),
        50000,
    )
    .await;

    // Check agent's lineage is ["root"] (PIP-001 §5: single-layer lineage)
    let info = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_info", "actor_id": "worker" }),
        ))
        .await;
    assert_eq!(info.payload["status"], "active");
    let lineage = info.payload["lineage"].as_array().unwrap();
    assert_eq!(
        lineage.len(),
        1,
        "lineage should be single-layer: {:?}",
        lineage
    );
    assert_eq!(lineage[0].as_str().unwrap(), "root");

    // Agent can act because creator (root) is active
    let action = Action {
        actor_id: "worker".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/worker/file".to_string(),
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
        "agent with active human creator should succeed: {}",
        resp.payload
    );
}
