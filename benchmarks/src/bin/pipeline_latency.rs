use punkgo_benchmarks::*;
use punkgo_core::action::ActionType;
use serde_json::json;
use std::time::Instant;

const WARMUP: usize = 20;
const MEASURED: usize = 200;
const TOTAL: usize = WARMUP + MEASURED;

/// PIP-002 §2 valid execute payload.
fn execute_payload(i: usize) -> serde_json::Value {
    json!({
        "input_oid": format!("sha256:{:0>64x}", i),
        "output_oid": format!("sha256:{:0>64x}", i + 1_000_000),
        "exit_code": 0,
        "artifact_hash": format!("sha256:{:0>64x}", i + 2_000_000),
        "output_bytes": 1024
    })
}

#[tokio::main]
async fn main() {
    println!("Pipeline Latency Benchmark — {MEASURED} iterations + {WARMUP} warmup");

    // ===== Observe latency =====
    println!("  Measuring Observe...");
    let (kernel, _state) = bootstrap_kernel("bench-latency-obs").await;
    let mut observe_samples: Vec<u64> = Vec::with_capacity(MEASURED);
    for i in 0..TOTAL {
        let action = make_action(
            "root",
            ActionType::Observe,
            &format!("workspace/lat/{i}"),
            json!({}),
        );
        let start = Instant::now();
        let _ = submit_raw(&kernel, action).await;
        let elapsed = start.elapsed().as_micros() as u64;
        if i >= WARMUP {
            observe_samples.push(elapsed);
        }
    }
    let observe_stats = BenchStats::from_micros(observe_samples);
    drop(_state);

    // ===== Create latency =====
    println!("  Measuring Create...");
    let (kernel, _state) = bootstrap_kernel("bench-latency-create").await;
    let mut create_samples: Vec<u64> = Vec::with_capacity(MEASURED);
    for i in 0..TOTAL {
        let action = make_action(
            "root",
            ActionType::Create,
            &format!("workspace/lat/create/{i}"),
            json!({"data": i}),
        );
        let start = Instant::now();
        let _ = submit_raw(&kernel, action).await;
        let elapsed = start.elapsed().as_micros() as u64;
        if i >= WARMUP {
            create_samples.push(elapsed);
        }
    }
    let create_stats = BenchStats::from_micros(create_samples);
    drop(_state);

    // ===== Mutate latency =====
    println!("  Measuring Mutate...");
    let (kernel, _state) = bootstrap_kernel("bench-latency-mutate").await;
    let mut mutate_samples: Vec<u64> = Vec::with_capacity(MEASURED);
    for i in 0..TOTAL {
        let action = make_action(
            "root",
            ActionType::Mutate,
            &format!("workspace/lat/{i}"),
            json!({"val": i}),
        );
        let start = Instant::now();
        let _ = submit_raw(&kernel, action).await;
        let elapsed = start.elapsed().as_micros() as u64;
        if i >= WARMUP {
            mutate_samples.push(elapsed);
        }
    }
    let mutate_stats = BenchStats::from_micros(mutate_samples);
    drop(_state);

    // ===== Execute latency (PIP-002: payload validation only, no process spawn) =====
    println!("  Measuring Execute (PIP-002 submission)...");
    let (kernel, _state) = bootstrap_kernel("bench-latency-exec").await;
    let mut execute_samples: Vec<u64> = Vec::with_capacity(MEASURED);
    for i in 0..TOTAL {
        let action = make_action(
            "root",
            ActionType::Execute,
            &format!("workspace/lat/exec/{i}"),
            execute_payload(i),
        );
        let start = Instant::now();
        let _ = submit_raw(&kernel, action).await;
        let elapsed = start.elapsed().as_micros() as u64;
        if i >= WARMUP {
            execute_samples.push(elapsed);
        }
    }
    let execute_stats = BenchStats::from_micros(execute_samples);
    drop(_state);

    // ===== Read query latency =====
    println!("  Measuring Read (events query)...");
    let (kernel, _state) = bootstrap_kernel("bench-latency-read").await;
    // Seed some events first
    for i in 0..50 {
        submit_ok(
            &kernel,
            make_action(
                "root",
                ActionType::Mutate,
                &format!("workspace/seed/{i}"),
                json!({}),
            ),
        )
        .await;
    }
    let mut read_samples: Vec<u64> = Vec::with_capacity(MEASURED);
    for i in 0..TOTAL {
        let start = Instant::now();
        let _ = read_ok(&kernel, json!({"kind": "events", "limit": 10})).await;
        let elapsed = start.elapsed().as_micros() as u64;
        if i >= WARMUP {
            read_samples.push(elapsed);
        }
    }
    let read_stats = BenchStats::from_micros(read_samples);

    let result = json!({
        "test": "Pipeline Latency",
        "unit": "microseconds",
        "warmup_iterations": WARMUP,
        "measurement_iterations": MEASURED,
        "results": {
            "observe": observe_stats.to_json(),
            "create": create_stats.to_json(),
            "mutate": mutate_stats.to_json(),
            "execute": execute_stats.to_json(),
            "read_events": read_stats.to_json(),
        },
        "per_step_breakdown": {
            "available": false,
            "note": "Kernel does not expose per-step tracing. End-to-end latency includes: validate + quote + reserve + payload_validate + settle + append + post-commit audit + snapshot refresh."
        },
        "notes": "PIP-002: Execute is now actor-submitted result (no OS process spawn). Kernel validates payload format (input_oid, output_oid, exit_code, artifact_hash) and records the event. Each action type measured with a fresh kernel to avoid log-size dependent snapshot refresh degradation."
    });

    write_result("pipeline_latency.json", &result);
    println!("Pipeline Latency: done");
    println!("  Observe median: {:.0} us", observe_stats.median_us);
    println!("  Create  median: {:.0} us", create_stats.median_us);
    println!("  Mutate  median: {:.0} us", mutate_stats.median_us);
    println!("  Execute median: {:.0} us", execute_stats.median_us);
    println!("  Read    median: {:.0} us", read_stats.median_us);
}
