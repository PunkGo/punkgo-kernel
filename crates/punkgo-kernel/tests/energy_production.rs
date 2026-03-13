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

/// PIP-001 §2, §3: energy production credits agents (not humans).
///
/// Only agents participate in tick-based energy distribution. Humans (including root)
/// receive a one-time initial balance and do not compete for ongoing production.
#[tokio::test]
async fn energy_production_credits_actors() {
    let (kernel, _state) = setup_kernel().await;

    // Create an agent — only agents receive tick energy
    let create_agent = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/actor".to_string(),
        payload: json!({
            "actor_id": "agent-tick",
            "actor_type": "agent",
            "purpose": "test-tick",
            "energy_balance": 1000,
            "energy_share": 50.0
        }),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(create_agent).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "ok");

    // Record agent's initial energy
    let agent_before = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "agent-tick" }),
        ))
        .await;
    let agent_balance_before = agent_before.payload["energy_balance"].as_i64().unwrap();

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

    // Run one tick — only the agent participates, gets all 100 energy
    let result = producer
        .produce_tick(100)
        .await
        .expect("tick should succeed");
    assert_eq!(result.total_energy_produced, 100);
    assert_eq!(result.actors_credited, 1);
    assert_eq!(result.total_shares, 50.0);

    // Verify agent got credited, root did not
    let agent_after = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "agent-tick" }),
        ))
        .await;
    let agent_balance_after = agent_after.payload["energy_balance"].as_i64().unwrap();
    assert_eq!(
        agent_balance_after,
        agent_balance_before + 100,
        "agent should receive all 100 energy units"
    );

    // Root (human) should NOT have gained energy from tick
    let root_before = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "root" }),
        ))
        .await;
    let root_balance = root_before.payload["energy_balance"].as_i64().unwrap();
    // Root starts with 1_000_000, minus energy spent on create action
    assert!(
        root_balance < 1_000_000,
        "root should not gain energy from tick distribution"
    );
}

/// PIP-001 §3: two agents with different shares receive proportional energy.
/// Humans (root) do not participate in energy distribution.
#[tokio::test]
async fn energy_distribution_proportional_to_shares() {
    let (kernel, _state) = setup_kernel().await;

    // Create two agents with different energy_share values
    let create_alpha = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/actor".to_string(),
        payload: json!({
            "actor_id": "agent-alpha",
            "actor_type": "agent",
            "purpose": "test-energy-a",
            "energy_balance": 1000,
            "energy_share": 100.0
        }),
        timestamp: None,
    };
    let create_beta = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/actor".to_string(),
        payload: json!({
            "actor_id": "agent-beta",
            "actor_type": "agent",
            "purpose": "test-energy-b",
            "energy_balance": 1000,
            "energy_share": 50.0
        }),
        timestamp: None,
    };
    let resp_a = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(create_alpha).unwrap(),
        ))
        .await;
    assert_eq!(resp_a.status, "ok");
    let resp_b = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(create_beta).unwrap(),
        ))
        .await;
    assert_eq!(resp_b.status, "ok");

    // Record balances before tick
    let alpha_before = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "agent-alpha" }),
        ))
        .await;
    let alpha_balance_before = alpha_before.payload["energy_balance"].as_i64().unwrap();

    let beta_before = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "agent-beta" }),
        ))
        .await;
    let beta_balance_before = beta_before.payload["energy_balance"].as_i64().unwrap();

    // Produce one tick with 150 energy units
    // alpha share=100.0, beta share=50.0, total=150.0
    // alpha gets 100/150 * 150 = 100, beta gets 50/150 * 150 = 50
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
    let alpha_after = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "agent-alpha" }),
        ))
        .await;
    let beta_after = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "agent-beta" }),
        ))
        .await;

    let alpha_gained =
        alpha_after.payload["energy_balance"].as_i64().unwrap() - alpha_balance_before;
    let beta_gained = beta_after.payload["energy_balance"].as_i64().unwrap() - beta_balance_before;

    assert_eq!(alpha_gained, 100, "alpha (2/3 share) should get 100");
    assert_eq!(beta_gained, 50, "beta (1/3 share) should get 50");
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

/// Humans do not receive tick energy — only agents participate in distribution.
#[tokio::test]
async fn humans_excluded_from_energy_distribution() {
    let (kernel, _state) = setup_kernel().await;

    let root_before = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "root" }),
        ))
        .await;
    let root_balance_before = root_before.payload["energy_balance"].as_i64().unwrap();

    // No agents exist — tick should produce energy but credit nobody
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

    let result = producer
        .produce_tick(100)
        .await
        .expect("tick should succeed");
    assert_eq!(result.actors_credited, 0, "no agents to credit");
    assert_eq!(result.total_energy_produced, 0, "no energy distributed");

    // Root balance unchanged
    let root_after = kernel
        .handle_request(make_request(
            RequestType::Read,
            json!({ "kind": "actor_energy", "actor_id": "root" }),
        ))
        .await;
    let root_balance_after = root_after.payload["energy_balance"].as_i64().unwrap();
    assert_eq!(
        root_balance_before, root_balance_after,
        "root (human) must not receive tick energy"
    );
}

/// update_energy_share lifecycle operation changes an agent's share.
#[tokio::test]
async fn update_energy_share_via_lifecycle() {
    let (kernel, _state) = setup_kernel().await;

    // Create an agent with initial share
    let create = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Create,
        target: "ledger/actor".to_string(),
        payload: json!({
            "actor_id": "agent-share",
            "actor_type": "agent",
            "purpose": "test-share-update",
            "energy_balance": 1000,
            "energy_share": 10.0
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

    // Update energy_share via mutate lifecycle op
    let update = Action {
        actor_id: "root".to_string(),
        action_type: ActionType::Mutate,
        target: "actor/agent-share".to_string(),
        payload: json!({
            "op": "update_energy_share",
            "energy_share": 75.0
        }),
        timestamp: None,
    };
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(update).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "ok", "update_energy_share should succeed");

    // Verify the share was updated by running a tick
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
    let result = producer
        .produce_tick(100)
        .await
        .expect("tick should succeed");
    assert_eq!(
        result.total_shares, 75.0,
        "share should reflect updated value"
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
