use punkgo_core::action::{Action, ActionType};
use punkgo_core::protocol::RequestType;
use punkgo_core::stellar::StellarConfig;
use punkgo_kernel::testkit::{TestStateDir, make_request};
use punkgo_kernel::{EnergyProducer, Kernel, KernelConfig};
use serde_json::json;

/// Helper: bootstrap kernel and return everything needed for energy tests.
async fn setup_kernel() -> (Kernel, TestStateDir) {
    let state = TestStateDir::new("punkgo-energy-test").expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config)
        .await
        .expect("kernel bootstrap should succeed");
    (kernel, state)
}

/// PIP-001 §1: stellar configuration is loaded and accessible via read query.
#[tokio::test]
async fn stellar_config_loaded_on_bootstrap() {
    let (kernel, _state) = setup_kernel().await;

    let resp = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "stellar_info" }),
        ))
        .await;
    assert_eq!(resp.status, "ok");

    // Default config: int8_tops=100, energy_per_tick=100
    let energy_per_tick = resp.payload["energy_per_tick"].as_i64().unwrap();
    assert!(
        energy_per_tick >= 26,
        "energy_per_tick must meet PIP-001 §4 minimum"
    );
    assert_eq!(energy_per_tick, 100, "default should compute to 100");
}

/// PIP-001 §2, §3: energy production credits actors proportionally.
#[tokio::test]
async fn energy_production_credits_actors() {
    let (kernel, _state) = setup_kernel().await;

    // Record root's initial energy
    let root_before = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "root" }),
        ))
        .await;
    let root_balance_before = root_before.payload["energy_balance"].as_i64().unwrap();

    // Create energy producer with known config
    let config = StellarConfig {
        energy_per_tick: Some(100),
        ..Default::default()
    };
    let producer = EnergyProducer::new(
        kernel.pool(),
        kernel.actor_store().clone(),
        kernel.energy_ledger().clone(),
        config,
    );

    // Run one tick — root has energy_share=100.0, so gets all energy
    let result = producer
        .produce_tick(100)
        .await
        .expect("tick should succeed");
    assert_eq!(result.total_energy_produced, 100);
    assert_eq!(result.actors_credited, 1);
    assert_eq!(result.total_shares, 100.0);

    // Verify root got credited
    let root_after = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "root" }),
        ))
        .await;
    let root_balance_after = root_after.payload["energy_balance"].as_i64().unwrap();
    assert_eq!(
        root_balance_after,
        root_balance_before + 100,
        "root should receive all 100 energy units"
    );
}

/// PIP-001 §3: two actors with different shares receive proportional energy.
#[tokio::test]
async fn energy_distribution_proportional_to_shares() {
    let (kernel, _state) = setup_kernel().await;

    // Create a second actor with energy_share = 50.0
    // root has energy_share = 100.0
    let create_action = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/actor".to_string(),
        payload: json!({
            "actor_id": "agent-alpha",
            "actor_type": "agent",
            "purpose": "test-energy",
            "energy_balance": 1000,
            "energy_share": 50.0
        }),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(create_action).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "ok", "actor creation should succeed");

    // Record balances before tick
    let root_before = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "root" }),
        ))
        .await;
    let root_balance_before = root_before.payload["energy_balance"].as_i64().unwrap();

    let agent_before = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "agent-alpha" }),
        ))
        .await;
    let agent_balance_before = agent_before.payload["energy_balance"].as_i64().unwrap();

    // Produce one tick with 150 energy units
    // root share=100.0, agent share=50.0, total=150.0
    // root gets 100/150 * 150 = 100, agent gets 50/150 * 150 = 50
    let config = StellarConfig {
        energy_per_tick: Some(150),
        ..Default::default()
    };
    let producer = EnergyProducer::new(
        kernel.pool(),
        kernel.actor_store().clone(),
        kernel.energy_ledger().clone(),
        config,
    );

    let result = producer
        .produce_tick(150)
        .await
        .expect("tick should succeed");
    assert_eq!(result.actors_credited, 2);
    assert_eq!(
        result.total_energy_produced, 150,
        "energy neutrality: all energy distributed"
    );

    // Verify proportional distribution
    let root_after = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "root" }),
        ))
        .await;
    let agent_after = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "agent-alpha" }),
        ))
        .await;

    let root_gained = root_after.payload["energy_balance"].as_i64().unwrap() - root_balance_before;
    let agent_gained =
        agent_after.payload["energy_balance"].as_i64().unwrap() - agent_balance_before;

    assert_eq!(root_gained, 100, "root (2/3 share) should get 100");
    assert_eq!(agent_gained, 50, "agent (1/3 share) should get 50");
}

/// Energy neutrality (implementation detail): kernel energy neutrality — total produced == total distributed.
#[tokio::test]
async fn energy_neutral_kernel() {
    let (kernel, _state) = setup_kernel().await;

    // Create two actors with fractional shares that don't divide evenly
    let create_a = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/actor".to_string(),
        payload: json!({
            "actor_id": "actor-a",
            "actor_type": "agent",
            "purpose": "test-neutral-a",
            "energy_balance": 500,
            "energy_share": 33.3
        }),
        timestamp: None,
    };
    let create_b = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/actor".to_string(),
        payload: json!({
            "actor_id": "actor-b",
            "actor_type": "agent",
            "purpose": "test-neutral-b",
            "energy_balance": 500,
            "energy_share": 33.3
        }),
        timestamp: None,
    };

    let resp_a = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(create_a).unwrap(),
        ))
        .await;
    assert_eq!(resp_a.status, "ok");
    let resp_b = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(create_b).unwrap(),
        ))
        .await;
    assert_eq!(resp_b.status, "ok");

    // Use a prime energy_per_tick to test remainder distribution
    let config = StellarConfig {
        energy_per_tick: Some(97),
        ..Default::default()
    };
    let producer = EnergyProducer::new(
        kernel.pool(),
        kernel.actor_store().clone(),
        kernel.energy_ledger().clone(),
        config,
    );

    let result = producer
        .produce_tick(97)
        .await
        .expect("tick should succeed");

    // Energy neutrality: total_energy_produced must exactly equal energy_per_tick
    assert_eq!(
        result.total_energy_produced, 97,
        "kernel must distribute exactly energy_per_tick (no retention)"
    );
}

/// PIP-001 §4: don't-starve constraint — energy_per_tick >= max basic operation cost.
#[tokio::test]
async fn dont_starve_minimum() {
    // compute_energy_per_tick should enforce minimum even with low TOPS
    let config = StellarConfig {
        int8_tops: 1.0,
        energy_per_tick: None,
        ..Default::default()
    };
    let ept = config.effective_energy_per_tick();
    assert!(
        ept >= 26,
        "PIP-001 §4: energy_per_tick ({ept}) must be >= 26 (max basic cost)"
    );
}

/// Zero-share actors don't receive energy.
#[tokio::test]
async fn zero_share_actors_get_no_energy() {
    let (kernel, _state) = setup_kernel().await;

    // Create actor with energy_share = 0.0
    let create = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/actor".to_string(),
        payload: json!({
            "actor_id": "zero-share",
            "actor_type": "agent",
            "purpose": "test-zero",
            "energy_balance": 1000,
            "energy_share": 0.0
        }),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(create).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "ok");

    let before = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "zero-share" }),
        ))
        .await;
    let balance_before = before.payload["energy_balance"].as_i64().unwrap();

    let config = StellarConfig {
        energy_per_tick: Some(100),
        ..Default::default()
    };
    let producer = EnergyProducer::new(
        kernel.pool(),
        kernel.actor_store().clone(),
        kernel.energy_ledger().clone(),
        config,
    );
    producer
        .produce_tick(100)
        .await
        .expect("tick should succeed");

    let after = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "zero-share" }),
        ))
        .await;
    let balance_after = after.payload["energy_balance"].as_i64().unwrap();

    assert_eq!(
        balance_before, balance_after,
        "zero-share actor should not receive any energy"
    );
}
