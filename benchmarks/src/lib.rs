use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use punkgo_core::action::{Action, ActionType};
use punkgo_core::protocol::{RequestType, ResponseEnvelope};
use punkgo_runtime::{Kernel, KernelConfig};
use punkgo_testkit::{TestStateDir, make_request};
use serde_json::{Value, json};

/// Bootstrap a fresh kernel in a temp directory.
pub async fn bootstrap_kernel(prefix: &str) -> (Kernel, TestStateDir) {
    let state = TestStateDir::new(prefix).expect("create temp state dir");
    let config = KernelConfig {
        state_dir: state.path().to_path_buf(),
        ipc_endpoint: state.ipc_endpoint(),
    };
    let kernel = Kernel::bootstrap(&config).await.expect("kernel bootstrap");
    (kernel, state)
}

/// Submit an action, assert "ok", return the response payload.
pub async fn submit_ok(kernel: &Kernel, action: Action) -> Value {
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(&action).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "ok", "expected ok: {}", resp.payload);
    resp.payload
}

/// Submit an action, return full response (don't assert).
pub async fn submit_raw(kernel: &Kernel, action: Action) -> ResponseEnvelope {
    kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(&action).unwrap(),
        ))
        .await
}

/// Submit an action expected to fail, return payload.
pub async fn submit_err(kernel: &Kernel, action: Action) -> Value {
    let resp = kernel
        .handle_request(make_request(
            RequestType::Submit,
            serde_json::to_value(&action).unwrap(),
        ))
        .await;
    assert_eq!(resp.status, "error", "expected error: {}", resp.payload);
    resp.payload
}

/// Issue a Read query, assert "ok", return payload.
pub async fn read_ok(kernel: &Kernel, query: Value) -> Value {
    let resp = kernel
        .handle_request(make_request(RequestType::Read, query))
        .await;
    assert_eq!(resp.status, "ok", "read failed: {}", resp.payload);
    resp.payload
}

/// Create a human actor.
pub async fn create_human(kernel: &Kernel, actor_id: &str, energy: i64) {
    let action = Action {
        actor_id: "root".into(),
        action_type: ActionType::Create,
        target: "ledger/actor".into(),
        payload: json!({
            "actor_id": actor_id,
            "energy_balance": energy
        }),
        timestamp: None,
    };
    submit_ok(kernel, action).await;
}

/// Create an agent actor.
pub async fn create_agent(kernel: &Kernel, agent_id: &str, energy: i64, targets: Value) {
    let action = Action {
        actor_id: "root".into(),
        action_type: ActionType::Create,
        target: "ledger/actor".into(),
        payload: json!({
            "actor_id": agent_id,
            "actor_type": "agent",
            "purpose": "benchmark",
            "energy_balance": energy,
            "energy_share": 0.0,
            "writable_targets": targets
        }),
        timestamp: None,
    };
    submit_ok(kernel, action).await;
}

/// Grant an envelope to an agent.
pub async fn grant_envelope(
    kernel: &Kernel,
    agent_id: &str,
    budget: i64,
    targets: Value,
    actions: Value,
    hold_on: Option<Value>,
    hold_timeout_secs: Option<i64>,
) {
    let mut payload = json!({
        "actor_id": agent_id,
        "budget": budget,
        "targets": targets,
        "actions": actions
    });
    if let Some(hold) = hold_on {
        payload["hold_on"] = hold;
    }
    if let Some(timeout) = hold_timeout_secs {
        payload["hold_timeout_secs"] = json!(timeout);
    }
    let action = Action {
        actor_id: "root".into(),
        action_type: ActionType::Create,
        target: "ledger/envelope".into(),
        payload,
        timestamp: None,
    };
    submit_ok(kernel, action).await;
}

/// Query an actor's energy balance. Returns (balance, reserved).
pub async fn query_energy(kernel: &Kernel, actor_id: &str) -> (i64, i64) {
    let p = read_ok(
        kernel,
        json!({"kind": "actor_energy", "actor_id": actor_id}),
    )
    .await;
    (
        p["energy_balance"].as_i64().expect("energy_balance"),
        p["reserved_energy"].as_i64().expect("reserved_energy"),
    )
}

/// Extract hold_id from a HoldTriggered error payload.
pub fn extract_hold_id(payload: &Value) -> String {
    let s = payload.to_string();
    s.split("hold_id=")
        .nth(1)
        .and_then(|rest| rest.split(',').next())
        .map(|id| id.trim().trim_matches('"').to_string())
        .unwrap_or_else(|| panic!("could not extract hold_id from: {s}"))
}

/// Build a simple action.
pub fn make_action(
    actor_id: &str,
    action_type: ActionType,
    target: &str,
    payload: Value,
) -> Action {
    Action {
        actor_id: actor_id.into(),
        action_type,
        target: target.into(),
        payload,
        timestamp: None,
    }
}

/// Compute statistics from a sorted list of microsecond samples.
#[derive(Debug, Clone)]
pub struct BenchStats {
    pub n: usize,
    pub median_us: f64,
    pub mean_us: f64,
    pub p95_us: f64,
    pub min_us: u64,
    pub max_us: u64,
}

impl BenchStats {
    pub fn from_micros(mut samples: Vec<u64>) -> Self {
        assert!(!samples.is_empty(), "no samples");
        samples.sort_unstable();
        let n = samples.len();
        let mean = samples.iter().sum::<u64>() as f64 / n as f64;
        let median = if n % 2 == 0 {
            (samples[n / 2 - 1] + samples[n / 2]) as f64 / 2.0
        } else {
            samples[n / 2] as f64
        };
        let p95_idx = ((n as f64 * 0.95) as usize).min(n - 1);
        let p95 = samples[p95_idx] as f64;
        Self {
            n,
            median_us: median,
            mean_us: mean,
            p95_us: p95,
            min_us: samples[0],
            max_us: *samples.last().unwrap(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "n": self.n,
            "median_us": self.median_us,
            "mean_us": self.mean_us,
            "p95_us": self.p95_us,
            "min_us": self.min_us,
            "max_us": self.max_us
        })
    }
}

/// Write a JSON value to benchmarks/results/<filename>.
pub fn write_result(filename: &str, value: &Value) {
    let results_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("results");
    fs::create_dir_all(&results_dir).expect("create results dir");
    let path = results_dir.join(filename);
    let content = serde_json::to_string_pretty(value).unwrap();
    fs::write(&path, &content).expect("write result file");
    println!("[OK] Wrote: {}", path.display());
}

/// Write a string to benchmarks/results/<filename>.
pub fn write_result_string(filename: &str, content: &str) {
    let results_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("results");
    fs::create_dir_all(&results_dir).expect("create results dir");
    let path = results_dir.join(filename);
    fs::write(&path, content).expect("write result file");
    println!("[OK] Wrote: {}", path.display());
}

/// Read a JSON result file.
pub fn read_result(filename: &str) -> Option<Value> {
    let results_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("results");
    let path = results_dir.join(filename);
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Time an async operation, return (result, duration).
pub async fn timed<F, Fut, T>(f: F) -> (T, Duration)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let start = Instant::now();
    let result = f().await;
    (result, start.elapsed())
}
