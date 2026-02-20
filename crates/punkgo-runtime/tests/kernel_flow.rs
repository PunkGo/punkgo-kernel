use punkgo_core::action::{Action, ActionType};
use punkgo_core::protocol::RequestType;
use punkgo_runtime::{Kernel, KernelConfig};
use punkgo_testkit::{TestStateDir, make_request};
use serde_json::json;

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
    assert_eq!(energy.payload["energy_balance"], json!(999985));
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

#[tokio::test]
async fn execute_failure_still_appends_event_and_clears_reservation() {
    let state = TestStateDir::new("punkgo-runtime-fail").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config)
        .await
        .expect("kernel bootstrap should succeed");

    let before = kernel
        .handle_request(make_request(RequestType::Read, json!({ "kind": "stats" })))
        .await;
    let before_count = before.payload["event_count"]
        .as_i64()
        .expect("event_count should be number");

    let action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Execute,
        target: "workspace/a".to_string(),
        payload: json!({
            "command": "definitely_nonexistent_punkgo_command_xyz",
            "timeout_ms": 100
        }),
        timestamp: None,
    };

    let submit = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(action).expect("serialize action"),
        ))
        .await;
    assert_eq!(submit.status, "error");

    let energy = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "root" }),
        ))
        .await;
    assert_eq!(energy.status, "ok");
    assert_eq!(energy.payload["reserved_energy"], json!(0));

    let after = kernel
        .handle_request(make_request(RequestType::Read, json!({ "kind": "stats" })))
        .await;
    let after_count = after.payload["event_count"]
        .as_i64()
        .expect("event_count should be number");
    assert_eq!(after_count, before_count + 1);

    let events = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "events", "limit": 1 }),
        ))
        .await;
    assert_eq!(events.status, "ok");
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
