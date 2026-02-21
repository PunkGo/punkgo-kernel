use punkgo_benchmarks::*;
use punkgo_core::action::ActionType;
use serde_json::json;
use std::time::Instant;

const WARMUP: usize = 20;
const MEASURED: usize = 200;
const TOTAL: usize = WARMUP + MEASURED;

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
            "read_events": read_stats.to_json(),
        },
        "per_step_breakdown": {
            "available": false,
            "note": "Kernel does not expose per-step tracing. End-to-end latency includes: validate + quote + reserve + execute + settle + append + post-commit audit + snapshot refresh."
        },
        "notes": "Execute action excluded: OS process spawn time (ms-scale) dominates kernel overhead (us-scale). Each action type measured with a fresh kernel to avoid log-size dependent snapshot refresh degradation."
    });

    write_result("pipeline_latency.json", &result);
    println!("Pipeline Latency: done");
    println!("  Observe median: {:.0} us", observe_stats.median_us);
    println!("  Create  median: {:.0} us", create_stats.median_us);
    println!("  Mutate  median: {:.0} us", mutate_stats.median_us);
    println!("  Read    median: {:.0} us", read_stats.median_us);
}
