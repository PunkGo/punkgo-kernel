use punkgo_core::action::{Action, ActionType};
use punkgo_core::protocol::RequestType;
use punkgo_runtime::{Kernel, KernelConfig};
use punkgo_testkit::{TestStateDir, make_request};
use serde_json::json;

/// Helper: a valid PIP-002 execute payload.
fn valid_execute_payload() -> serde_json::Value {
    json!({
        "input_oid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "output_oid": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "exit_code": 0,
        "artifact_hash": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        "output_bytes": 512
    })
}

#[tokio::test]
async fn submit_mutate_charges_energy_and_appends_event() {
    let state = TestStateDir::new("punkgo-runtime-mutate").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config)
        .await
        .expect("kernel bootstrap should succeed");

    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: "workspace/a".to_string(),
        payload: json!({ "k": "v" }),
        timestamp: None,
    };

    let submit = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).expect("serialize action"),
        ))
        .await;
    assert_eq!(
        submit.status, "ok",
        "submit payload should be ok but got: {}",
        submit.payload
    );
    assert_eq!(
        submit.payload["log_index"],
        json!(0),
        "first event log_index should be 0"
    );
    assert!(
        submit.payload["event_hash"]
            .as_str()
            .map(|s| s.len() == 64)
            .unwrap_or(false),
        "event_hash should be a 64-char hex string"
    );

    let energy = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "root" }),
        ))
        .await;
    assert_eq!(energy.status, "ok");
    assert_eq!(energy.payload["energy_balance"], json!(999984));
    assert_eq!(energy.payload["reserved_energy"], json!(0));

    let events = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "events", "limit": 1 }),
        ))
        .await;
    assert_eq!(events.status, "ok");
    assert_eq!(events.payload["events"][0]["action_type"], json!("mutate"));
}

// ===== PIP-002: Execute Submission Tests =====

#[tokio::test]
async fn execute_with_valid_payload_succeeds() {
    let state = TestStateDir::new("pip002-exec-ok").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config).await.expect("bootstrap");

    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Execute,
        target: "workspace/a".to_string(),
        payload: valid_execute_payload(),
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
        "valid execute should succeed: {}",
        resp.payload
    );
    assert_eq!(
        resp.payload["artifact_hash"],
        json!("sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc")
    );
}

#[tokio::test]
async fn execute_missing_input_oid_rejected() {
    let state = TestStateDir::new("pip002-no-input").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config).await.expect("bootstrap");

    let mut payload = valid_execute_payload();
    payload.as_object_mut().unwrap().remove("input_oid");

    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Execute,
        target: "workspace/a".to_string(),
        payload,
        timestamp: None,
    };

    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "error");
    assert_eq!(resp.payload["error_type"], json!("ExecutePayloadInvalid"));
}

#[tokio::test]
async fn execute_missing_output_oid_rejected() {
    let state = TestStateDir::new("pip002-no-output").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config).await.expect("bootstrap");

    let mut payload = valid_execute_payload();
    payload.as_object_mut().unwrap().remove("output_oid");

    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Execute,
        target: "workspace/a".to_string(),
        payload,
        timestamp: None,
    };

    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "error");
    assert_eq!(resp.payload["error_type"], json!("ExecutePayloadInvalid"));
}

#[tokio::test]
async fn execute_missing_exit_code_rejected() {
    let state = TestStateDir::new("pip002-no-exit").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config).await.expect("bootstrap");

    let mut payload = valid_execute_payload();
    payload.as_object_mut().unwrap().remove("exit_code");

    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Execute,
        target: "workspace/a".to_string(),
        payload,
        timestamp: None,
    };

    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "error");
    assert_eq!(resp.payload["error_type"], json!("ExecutePayloadInvalid"));
}

#[tokio::test]
async fn execute_missing_artifact_hash_rejected() {
    let state = TestStateDir::new("pip002-no-hash").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config).await.expect("bootstrap");

    let mut payload = valid_execute_payload();
    payload.as_object_mut().unwrap().remove("artifact_hash");

    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Execute,
        target: "workspace/a".to_string(),
        payload,
        timestamp: None,
    };

    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "error");
    assert_eq!(resp.payload["error_type"], json!("ExecutePayloadInvalid"));
}

#[tokio::test]
async fn execute_invalid_oid_format_rejected() {
    let state = TestStateDir::new("pip002-bad-oid").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config).await.expect("bootstrap");

    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Execute,
        target: "workspace/a".to_string(),
        payload: json!({
            "input_oid": "bad-format-not-sha256",
            "output_oid": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "exit_code": 0,
            "artifact_hash": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        }),
        timestamp: None,
    };

    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "error");
    assert_eq!(resp.payload["error_type"], json!("ExecutePayloadInvalid"));
}

#[tokio::test]
async fn execute_cost_uses_output_bytes() {
    let state = TestStateDir::new("pip002-cost").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config).await.expect("bootstrap");

    // output_bytes = 512 → cost = 25 + 512/256 = 27
    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Execute,
        target: "workspace/a".to_string(),
        payload: valid_execute_payload(), // output_bytes: 512
        timestamp: None,
    };

    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "ok", "{}", resp.payload);
    assert_eq!(resp.payload["settled_cost"], json!(28));

    let energy = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "root" }),
        ))
        .await;
    // Initial: 1_000_000 - 28 = 999972 (action=27 + append=1)
    assert_eq!(energy.payload["energy_balance"], json!(999972));
}

#[tokio::test]
async fn execute_without_output_bytes_costs_base_25() {
    let state = TestStateDir::new("pip002-base-cost").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config).await.expect("bootstrap");

    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Execute,
        target: "workspace/a".to_string(),
        payload: json!({
            "input_oid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "output_oid": "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "exit_code": 0,
            "artifact_hash": "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        }),
        timestamp: None,
    };

    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "ok", "{}", resp.payload);
    assert_eq!(resp.payload["settled_cost"], json!(26));
}

#[tokio::test]
async fn execute_records_artifact_hash_in_event() {
    let state = TestStateDir::new("pip002-event-hash").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config).await.expect("bootstrap");

    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Execute,
        target: "workspace/a".to_string(),
        payload: valid_execute_payload(),
        timestamp: None,
    };

    kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).unwrap(),
        ))
        .await;

    let events = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "events", "limit": 1 }),
        ))
        .await;
    assert_eq!(events.status, "ok");
    assert_eq!(
        events.payload["events"][0]["artifact_hash"],
        json!("sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc")
    );
    assert_eq!(events.payload["events"][0]["action_type"], json!("execute"));
}

#[tokio::test]
async fn seed_actor_create_action_works() {
    let state = TestStateDir::new("punkgo-runtime-seed").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config)
        .await
        .expect("kernel bootstrap should succeed");

    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/actor".to_string(),
        payload: json!({
            "actor_id": "alice",
            "energy_balance": 5000
        }),
        timestamp: None,
    };

    let submit = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).expect("serialize action"),
        ))
        .await;
    assert_eq!(submit.status, "ok");

    let alice = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "alice" }),
        ))
        .await;
    assert_eq!(alice.status, "ok");
    assert_eq!(alice.payload["energy_balance"], json!(5000));
    assert_eq!(alice.payload["reserved_energy"], json!(0));
}
