use punkgo_benchmarks::*;
use punkgo_core::action::ActionType;
use serde_json::json;
use std::time::{Duration, Instant};

const DURATION_SECS: u64 = 5;

#[tokio::main]
async fn main() {
    println!("Throughput Benchmark — {DURATION_SECS}s per scenario");

    // ===== Observe throughput =====
    println!("  Measuring Observe throughput...");
    let (kernel, _state) = bootstrap_kernel("bench-tp-observe").await;
    let deadline = Instant::now() + Duration::from_secs(DURATION_SECS);
    let mut observe_count: u64 = 0;
    let obs_start = Instant::now();
    while Instant::now() < deadline {
        let _ = submit_raw(
            &kernel,
            make_action(
                "root",
                ActionType::Observe,
                &format!("workspace/tp/{observe_count}"),
                json!({}),
            ),
        )
        .await;
        observe_count += 1;
    }
    let obs_elapsed = obs_start.elapsed();
    let observe_tps = observe_count as f64 / obs_elapsed.as_secs_f64();
    drop(_state);

    // ===== Mutate throughput =====
    println!("  Measuring Mutate throughput...");
    let (kernel, _state) = bootstrap_kernel("bench-tp-mutate").await;
    let deadline = Instant::now() + Duration::from_secs(DURATION_SECS);
    let mut mutate_count: u64 = 0;
    let mut_start = Instant::now();
    while Instant::now() < deadline {
        let _ = submit_raw(
            &kernel,
            make_action(
                "root",
                ActionType::Mutate,
                &format!("workspace/tp/{mutate_count}"),
                json!({"i": mutate_count}),
            ),
        )
        .await;
        mutate_count += 1;
    }
    let mut_elapsed = mut_start.elapsed();
    let mutate_tps = mutate_count as f64 / mut_elapsed.as_secs_f64();
    drop(_state);

    // ===== Create throughput =====
    println!("  Measuring Create throughput...");
    let (kernel, _state) = bootstrap_kernel("bench-tp-create").await;
    let deadline = Instant::now() + Duration::from_secs(DURATION_SECS);
    let mut create_count: u64 = 0;
    let cre_start = Instant::now();
    while Instant::now() < deadline {
        let _ = submit_raw(
            &kernel,
            make_action(
                "root",
                ActionType::Create,
                &format!("workspace/tp/{create_count}"),
                json!({"i": create_count}),
            ),
        )
        .await;
        create_count += 1;
    }
    let cre_elapsed = cre_start.elapsed();
    let create_tps = create_count as f64 / cre_elapsed.as_secs_f64();
    drop(_state);

    // ===== Execute throughput (PIP-002: payload validation only) =====
    println!("  Measuring Execute throughput...");
    let (kernel, _state) = bootstrap_kernel("bench-tp-execute").await;
    let deadline = Instant::now() + Duration::from_secs(DURATION_SECS);
    let mut execute_count: u64 = 0;
    let exec_start = Instant::now();
    while Instant::now() < deadline {
        let _ = submit_raw(
            &kernel,
            make_action(
                "root",
                ActionType::Execute,
                &format!("workspace/tp/{execute_count}"),
                json!({
                    "input_oid": format!("sha256:{:0>64x}", execute_count),
                    "output_oid": format!("sha256:{:0>64x}", execute_count + 1_000_000),
                    "exit_code": 0,
                    "artifact_hash": format!("sha256:{:0>64x}", execute_count + 2_000_000),
                    "output_bytes": 1024
                }),
            ),
        )
        .await;
        execute_count += 1;
    }
    let exec_elapsed = exec_start.elapsed();
    let execute_tps = execute_count as f64 / exec_elapsed.as_secs_f64();
    drop(_state);

    // ===== Mixed throughput =====
    println!("  Measuring Mixed throughput...");
    let (kernel, _state) = bootstrap_kernel("bench-tp-mixed").await;
    let deadline = Instant::now() + Duration::from_secs(DURATION_SECS);
    let mut mixed_count: u64 = 0;
    let mix_start = Instant::now();
    let types = [
        ActionType::Observe,
        ActionType::Create,
        ActionType::Mutate,
        ActionType::Execute,
    ];
    while Instant::now() < deadline {
        let t = &types[(mixed_count % 4) as usize];
        let payload = if matches!(t, ActionType::Execute) {
            json!({
                "input_oid": format!("sha256:{:0>64x}", mixed_count),
                "output_oid": format!("sha256:{:0>64x}", mixed_count + 1_000_000),
                "exit_code": 0,
                "artifact_hash": format!("sha256:{:0>64x}", mixed_count + 2_000_000),
                "output_bytes": 1024
            })
        } else {
            json!({"i": mixed_count})
        };
        let _ = submit_raw(
            &kernel,
            make_action(
                "root",
                t.clone(),
                &format!("workspace/tp/{mixed_count}"),
                payload,
            ),
        )
        .await;
        mixed_count += 1;
    }
    let mix_elapsed = mix_start.elapsed();
    let mixed_tps = mixed_count as f64 / mix_elapsed.as_secs_f64();

    let result = json!({
        "test": "Throughput",
        "unit": "actions_per_second",
        "duration_secs": DURATION_SECS,
        "results": {
            "observe_only": {
                "actions_completed": observe_count,
                "total_time_ms": obs_elapsed.as_millis(),
                "throughput": observe_tps
            },
            "create_only": {
                "actions_completed": create_count,
                "total_time_ms": cre_elapsed.as_millis(),
                "throughput": create_tps
            },
            "mutate_only": {
                "actions_completed": mutate_count,
                "total_time_ms": mut_elapsed.as_millis(),
                "throughput": mutate_tps
            },
            "execute_only": {
                "actions_completed": execute_count,
                "total_time_ms": exec_elapsed.as_millis(),
                "throughput": execute_tps
            },
            "mixed": {
                "actions_completed": mixed_count,
                "total_time_ms": mix_elapsed.as_millis(),
                "throughput": mixed_tps
            }
        },
        "notes": "Sequential single-writer model (single Committer). SQLite WAL mode. Performance includes validate + quote + reserve + validate_payload + settle + append + receipt + post-commit audit (Merkle checkpoint)."
    });

    write_result("throughput.json", &result);
    println!("Throughput: done");
    println!(
        "  Observe: {observe_tps:.0} actions/sec ({observe_count} in {:.1}s)",
        obs_elapsed.as_secs_f64()
    );
    println!(
        "  Create:  {create_tps:.0} actions/sec ({create_count} in {:.1}s)",
        cre_elapsed.as_secs_f64()
    );
    println!(
        "  Mutate:  {mutate_tps:.0} actions/sec ({mutate_count} in {:.1}s)",
        mut_elapsed.as_secs_f64()
    );
    println!(
        "  Execute: {execute_tps:.0} actions/sec ({execute_count} in {:.1}s)",
        exec_elapsed.as_secs_f64()
    );
    println!(
        "  Mixed:   {mixed_tps:.0} actions/sec ({mixed_count} in {:.1}s)",
        mix_elapsed.as_secs_f64()
    );
}
