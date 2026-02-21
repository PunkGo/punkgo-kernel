use punkgo_benchmarks::*;
use punkgo_core::action::ActionType;
use serde_json::json;
use std::time::Instant;

const WARMUP: usize = 2;
const MEASURED: usize = 8;
const TOTAL: usize = WARMUP + MEASURED;

#[tokio::main]
async fn main() {
    println!("Hold Latency Benchmark — {MEASURED} iterations + {WARMUP} warmup");

    // ===== Phase A: Trigger latency =====
    // Each iteration needs a fresh kernel (hold consumes energy and creates state)
    println!("  Measuring Hold Trigger latency...");
    let mut trigger_samples: Vec<u64> = Vec::new();
    for i in 0..TOTAL {
        let (kernel, _state) = bootstrap_kernel(&format!("bench-hold-trigger-{i}")).await;
        create_agent(
            &kernel,
            "hold-agent",
            1_000_000,
            json!([{"target": "workspace/**", "actions": ["mutate"]}]),
        )
        .await;
        grant_envelope(
            &kernel,
            "hold-agent",
            500_000,
            json!(["workspace/**"]),
            json!(["mutate"]),
            Some(json!([{"target": "workspace/gated/*", "actions": ["mutate"]}])),
            None,
        )
        .await;

        let action = make_action(
            "hold-agent",
            ActionType::Mutate,
            "workspace/gated/file",
            json!({"data": i}),
        );
        let start = Instant::now();
        let _ = submit_raw(&kernel, action).await;
        let elapsed = start.elapsed().as_micros() as u64;
        if i >= WARMUP {
            trigger_samples.push(elapsed);
        }
    }
    let trigger_stats = BenchStats::from_micros(trigger_samples);

    // ===== Phase B: holds_pending read latency =====
    println!("  Measuring holds_pending read latency...");
    let (kernel, _state) = bootstrap_kernel("bench-hold-read").await;
    create_agent(
        &kernel,
        "hold-agent",
        1_000_000,
        json!([{"target": "workspace/**", "actions": ["mutate"]}]),
    )
    .await;
    grant_envelope(
        &kernel,
        "hold-agent",
        500_000,
        json!(["workspace/**"]),
        json!(["mutate"]),
        Some(json!([{"target": "workspace/gated/*", "actions": ["mutate"]}])),
        None,
    )
    .await;
    // Trigger one hold to have data
    let _ = submit_raw(
        &kernel,
        make_action(
            "hold-agent",
            ActionType::Mutate,
            "workspace/gated/file",
            json!({}),
        ),
    )
    .await;

    let mut read_samples: Vec<u64> = Vec::new();
    for i in 0..TOTAL {
        let start = Instant::now();
        let _ = read_ok(&kernel, json!({"kind": "holds_pending"})).await;
        let elapsed = start.elapsed().as_micros() as u64;
        if i >= WARMUP {
            read_samples.push(elapsed);
        }
    }
    let read_stats = BenchStats::from_micros(read_samples);
    drop(_state);

    // ===== Phase C: Approve latency =====
    println!("  Measuring Approve latency...");
    let mut approve_samples: Vec<u64> = Vec::new();
    for i in 0..TOTAL {
        let (kernel, _state) = bootstrap_kernel(&format!("bench-hold-approve-{i}")).await;
        create_agent(
            &kernel,
            "hold-agent",
            1_000_000,
            json!([{"target": "workspace/**", "actions": ["mutate"]}]),
        )
        .await;
        grant_envelope(
            &kernel,
            "hold-agent",
            500_000,
            json!(["workspace/**"]),
            json!(["mutate"]),
            Some(json!([{"target": "workspace/gated/*", "actions": ["mutate"]}])),
            None,
        )
        .await;

        let resp = submit_raw(
            &kernel,
            make_action(
                "hold-agent",
                ActionType::Mutate,
                "workspace/gated/file",
                json!({"data": i}),
            ),
        )
        .await;
        let hold_id = extract_hold_id(&resp.payload);

        let approve_action = make_action(
            "root",
            ActionType::Mutate,
            &format!("ledger/hold/{hold_id}"),
            json!({"decision": "approve"}),
        );
        let start = Instant::now();
        let _ = submit_raw(&kernel, approve_action).await;
        let elapsed = start.elapsed().as_micros() as u64;
        if i >= WARMUP {
            approve_samples.push(elapsed);
        }
    }
    let approve_stats = BenchStats::from_micros(approve_samples);

    // ===== Phase D: Reject latency =====
    println!("  Measuring Reject latency...");
    let mut reject_samples: Vec<u64> = Vec::new();
    for i in 0..TOTAL {
        let (kernel, _state) = bootstrap_kernel(&format!("bench-hold-reject-{i}")).await;
        create_agent(
            &kernel,
            "hold-agent",
            1_000_000,
            json!([{"target": "workspace/**", "actions": ["mutate"]}]),
        )
        .await;
        grant_envelope(
            &kernel,
            "hold-agent",
            500_000,
            json!(["workspace/**"]),
            json!(["mutate"]),
            Some(json!([{"target": "workspace/gated/*", "actions": ["mutate"]}])),
            None,
        )
        .await;

        let resp = submit_raw(
            &kernel,
            make_action(
                "hold-agent",
                ActionType::Mutate,
                "workspace/gated/file",
                json!({"data": i}),
            ),
        )
        .await;
        let hold_id = extract_hold_id(&resp.payload);

        let reject_action = make_action(
            "root",
            ActionType::Mutate,
            &format!("ledger/hold/{hold_id}"),
            json!({"decision": "reject"}),
        );
        let start = Instant::now();
        let _ = submit_raw(&kernel, reject_action).await;
        let elapsed = start.elapsed().as_micros() as u64;
        if i >= WARMUP {
            reject_samples.push(elapsed);
        }
    }
    let reject_stats = BenchStats::from_micros(reject_samples);

    let end_to_end = trigger_stats.median_us + approve_stats.median_us;

    let result = json!({
        "test": "Hold Workflow Latency",
        "unit": "microseconds",
        "warmup": WARMUP,
        "measured": MEASURED,
        "results": {
            "hold_trigger": trigger_stats.to_json(),
            "holds_pending_read": read_stats.to_json(),
            "approve": approve_stats.to_json(),
            "reject": reject_stats.to_json(),
            "end_to_end_trigger_to_approve_us": end_to_end,
        },
        "notes": "Each phase uses a fresh kernel instance. Trigger includes: validate + boundary + auth + hold_trigger + atomic(reserve + hold_event + hold_request). Approve includes: validate + hold_lookup + re-submit + settle + 2 events. Does NOT include human decision time."
    });

    write_result("hold_latency.json", &result);
    println!("Hold Latency: done");
    println!("  Trigger median: {:.0} us", trigger_stats.median_us);
    println!("  Read    median: {:.0} us", read_stats.median_us);
    println!("  Approve median: {:.0} us", approve_stats.median_us);
    println!("  Reject  median: {:.0} us", reject_stats.median_us);
    println!("  End-to-end (trigger+approve): {end_to_end:.0} us");
}
