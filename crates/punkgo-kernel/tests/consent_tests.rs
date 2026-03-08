use punkgo_core::action::{Action, ActionType};
use punkgo_core::protocol::RequestType;
use punkgo_kernel::{Kernel, KernelConfig};
use punkgo_kernel::testkit::{TestStateDir, make_request};
use serde_json::json;

async fn setup_kernel() -> (Kernel, TestStateDir) {
    let state = TestStateDir::new("punkgo-consent-test").expect("create temp state dir");
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
            "purpose": "consent-test",
            "energy_balance": 100000,
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

async fn grant_envelope(
    kernel: &Kernel,
    agent_id: &str,
    targets: serde_json::Value,
    actions: serde_json::Value,
    budget: i64,
) {
    grant_envelope_full(kernel, agent_id, targets, actions, budget, None).await;
}

async fn grant_envelope_full(
    kernel: &Kernel,
    agent_id: &str,
    targets: serde_json::Value,
    actions: serde_json::Value,
    budget: i64,
    report_every: Option<i64>,
) {
    let mut payload = json!({
        "actor_id": agent_id,
        "budget": budget,
        "targets": targets,
        "actions": actions
    });
    if let Some(r) = report_every {
        payload["report_every"] = json!(r);
    }

    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/envelope".to_string(),
        payload,
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

/// PIP-001 §11: Agent without envelope is rejected for state-changing actions.
#[tokio::test]
async fn agent_without_envelope_rejected() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "no-envelope-agent",
        json!([{"target": "workspace/**", "actions": ["create", "mutate"]}]),
    )
    .await;

    // Try to mutate without envelope — should be rejected
    let action = Action {
        actor_id: "no-envelope-agent".to_string(),
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
        "agent without envelope should be rejected"
    );
    assert!(
        resp.payload.to_string().contains("authorization required")
            || resp
                .payload
                .to_string()
                .contains("requires an active envelope"),
        "error should mention authorization: {}",
        resp.payload
    );
}

/// PIP-001 §5/§6: Human is always authorized (ManInTheLoop) without envelope.
#[tokio::test]
async fn human_always_authorized() {
    let (kernel, _state) = setup_kernel().await;

    // Root (human) should be able to act without any envelope
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
        "human should always be authorized: {}",
        resp.payload
    );
}

/// PIP-001 §11: Envelope authorizes agent for state-changing actions.
#[tokio::test]
async fn envelope_authorizes_agent() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "envelope-agent",
        json!([{"target": "workspace/**", "actions": ["create", "mutate"]}]),
    )
    .await;

    // Grant envelope
    grant_envelope(
        &kernel,
        "envelope-agent",
        json!(["workspace/**"]),
        json!(["create", "mutate"]),
        50000,
    )
    .await;

    // Agent should now be able to mutate
    let action = Action {
        actor_id: "envelope-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/test".to_string(),
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
        "agent with envelope should succeed: {}",
        resp.payload
    );
}

/// PIP-001 §11: Envelope target mismatch rejects action.
#[tokio::test]
async fn envelope_target_mismatch_rejects() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "narrow-agent",
        json!([{"target": "workspace/**", "actions": ["create", "mutate"]}]),
    )
    .await;

    // Grant envelope for workspace/docs/* only
    grant_envelope(
        &kernel,
        "narrow-agent",
        json!(["workspace/docs/*"]),
        json!(["create", "mutate"]),
        50000,
    )
    .await;

    // Agent tries to mutate workspace/code — envelope doesn't cover this
    let action = Action {
        actor_id: "narrow-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/code/main.rs".to_string(),
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
        "envelope target mismatch should reject"
    );
    assert!(
        resp.payload.to_string().contains("does not cover target"),
        "error should mention target mismatch: {}",
        resp.payload
    );
}

/// PIP-001 §11: Envelope action mismatch rejects action.
#[tokio::test]
async fn envelope_action_mismatch_rejects() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "action-agent",
        json!([{"target": "workspace/**", "actions": ["create", "mutate", "execute"]}]),
    )
    .await;

    // Grant envelope with only "create" action
    grant_envelope(
        &kernel,
        "action-agent",
        json!(["workspace/**"]),
        json!(["create"]),
        50000,
    )
    .await;

    // Agent tries to mutate — envelope only allows create
    let action = Action {
        actor_id: "action-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/test".to_string(),
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
        "envelope action mismatch should reject"
    );
    assert!(
        resp.payload.to_string().contains("does not permit action"),
        "error should mention action mismatch: {}",
        resp.payload
    );
}

/// PIP-001 §11 (checkpoint): Budget exhaustion halts the envelope.
#[tokio::test]
async fn envelope_budget_exhaustion_halts() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "budget-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    // Grant a very small budget — just enough for one mutate (action=15 + append=1 = 16)
    grant_envelope(
        &kernel,
        "budget-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        16, // exactly one mutate
    )
    .await;

    // First mutate should succeed (and exhaust/halt the budget)
    let action = Action {
        actor_id: "budget-agent".to_string(),
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
        "first mutate should succeed: {}",
        resp.payload
    );

    // Second mutate should fail — envelope expired (halted)
    let action2 = Action {
        actor_id: "budget-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/b".to_string(),
        payload: json!({"k": "v2"}),
        timestamp: None,
    };
    let resp2 = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action2).unwrap(),
        ))
        .await;
    assert_eq!(
        resp2.status, "error",
        "second mutate should fail after budget exhaustion"
    );
    assert!(
        resp2.payload.to_string().contains("authorization required")
            || resp2
                .payload
                .to_string()
                .contains("requires an active envelope")
            || resp2.payload.to_string().contains("not active"),
        "error should indicate no active envelope: {}",
        resp2.payload
    );
}

/// Whitepaper §3 invariant 6: Envelope info readable via read query.
#[tokio::test]
async fn envelope_info_readable() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "info-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope(
        &kernel,
        "info-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        5000,
    )
    .await;

    // Read envelope info
    let resp = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "envelope_info", "actor_id": "info-agent" }),
        ))
        .await;
    assert_eq!(resp.status, "ok");
    assert_eq!(resp.payload["actor_id"], "info-agent");
    assert_eq!(resp.payload["grantor_id"], "root");
    assert_eq!(resp.payload["budget"], 5000);
    assert_eq!(resp.payload["budget_consumed"], 0);
    assert_eq!(resp.payload["status"], "active");
}

/// PIP-001 §11: Envelope revocation prevents further actions.
#[tokio::test]
async fn envelope_revoke_blocks_actions() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(
        &kernel,
        "revoke-agent",
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;

    grant_envelope(
        &kernel,
        "revoke-agent",
        json!(["workspace/**"]),
        json!(["mutate"]),
        50000,
    )
    .await;

    // Verify agent can act
    let action = Action {
        actor_id: "revoke-agent".to_string(),
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
        "agent should work with active envelope: {}",
        resp.payload
    );

    // Read envelope ID
    let env_resp = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "envelope_info", "actor_id": "revoke-agent" }),
        ))
        .await;
    let envelope_id = env_resp.payload["envelope_id"].as_str().unwrap();

    // Revoke the envelope directly via store
    kernel
        .envelope_store()
        .set_status(envelope_id, &punkgo_core::consent::EnvelopeStatus::Revoked)
        .await
        .expect("revoke should succeed");

    // Agent should now fail
    let action2 = Action {
        actor_id: "revoke-agent".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/b".to_string(),
        payload: json!({"k": "v2"}),
        timestamp: None,
    };
    let resp2 = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action2).unwrap(),
        ))
        .await;
    assert_eq!(
        resp2.status, "error",
        "agent should fail after envelope revocation"
    );
}

/// Observe is exempt from authorization (agents can always observe).
#[tokio::test]
async fn observe_exempt_from_authorization() {
    let (kernel, _state) = setup_kernel().await;

    create_agent(&kernel, "observe-agent", json!([])).await;
    // No envelope granted

    // Observe should succeed despite no envelope
    let action = Action {
        actor_id: "observe-agent".to_string(),
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
        "observe should be exempt from authorization: {}",
        resp.payload
    );
}
