use punkgo_benchmarks::*;
use punkgo_core::action::ActionType;
use serde_json::json;
use std::time::Instant;

const PROOF_ITERATIONS: usize = 10;

#[tokio::main]
async fn main() {
    println!("Merkle Performance Benchmark");

    let log_sizes: Vec<usize> = vec![10, 50, 100, 500, 1000];
    let mut data_points: Vec<serde_json::Value> = Vec::new();

    for &target_size in &log_sizes {
        println!("  Log size: {target_size}...");
        let (kernel, _state) = bootstrap_kernel(&format!("bench-merkle-{target_size}")).await;

        // Fill log to target_size
        let fill_start = Instant::now();
        for i in 0..target_size {
            submit_ok(
                &kernel,
                make_action(
                    "root",
                    ActionType::Mutate,
                    &format!("workspace/fill/{i}"),
                    json!({"i": i}),
                ),
            )
            .await;
        }
        let fill_time_ms = fill_start.elapsed().as_millis();
        println!("    Filled in {fill_time_ms}ms");

        // Ensure checkpoint exists
        let cp = read_ok(&kernel, json!({"kind": "audit_checkpoint"})).await;
        let tree_size = cp["tree_size"].as_i64().unwrap();

        // Measure inclusion proof generation — first leaf
        let mut first_leaf_samples: Vec<u64> = Vec::new();
        let mut proof_length_first = 0;
        for _ in 0..PROOF_ITERATIONS {
            let start = Instant::now();
            let resp = read_ok(
                &kernel,
                json!({
                    "kind": "audit_inclusion_proof",
                    "log_index": 0,
                    "tree_size": tree_size
                }),
            )
            .await;
            first_leaf_samples.push(start.elapsed().as_micros() as u64);
            proof_length_first = resp["proof"].as_array().unwrap().len();
        }
        let first_stats = BenchStats::from_micros(first_leaf_samples);

        // Measure inclusion proof generation — last leaf
        let last_idx = target_size as i64 - 1;
        let mut last_leaf_samples: Vec<u64> = Vec::new();
        let mut proof_length_last = 0;
        for _ in 0..PROOF_ITERATIONS {
            let start = Instant::now();
            let resp = read_ok(
                &kernel,
                json!({
                    "kind": "audit_inclusion_proof",
                    "log_index": last_idx,
                    "tree_size": tree_size
                }),
            )
            .await;
            last_leaf_samples.push(start.elapsed().as_micros() as u64);
            proof_length_last = resp["proof"].as_array().unwrap().len();
        }
        let last_stats = BenchStats::from_micros(last_leaf_samples);

        // Measure consistency proof (half → full)
        let old_size = (target_size / 2).max(1) as i64;
        let mut consistency_samples: Vec<u64> = Vec::new();
        let mut consistency_proof_length = 0;
        for _ in 0..PROOF_ITERATIONS {
            let start = Instant::now();
            let resp = read_ok(
                &kernel,
                json!({
                    "kind": "audit_consistency_proof",
                    "old_size": old_size,
                    "tree_size": tree_size
                }),
            )
            .await;
            consistency_samples.push(start.elapsed().as_micros() as u64);
            consistency_proof_length = resp["proof"].as_array().unwrap().len();
        }
        let consistency_stats = BenchStats::from_micros(consistency_samples);

        // Proof size in bytes: each hash is 32 bytes (64 hex chars)
        let inclusion_proof_bytes = proof_length_first * 32;
        let consistency_proof_bytes = consistency_proof_length * 32;

        data_points.push(json!({
            "log_size": target_size,
            "fill_time_ms": fill_time_ms,
            "inclusion_proof_first_leaf": {
                "generation_time_us": first_stats.to_json(),
                "proof_length_hashes": proof_length_first,
                "proof_size_bytes": inclusion_proof_bytes,
            },
            "inclusion_proof_last_leaf": {
                "generation_time_us": last_stats.to_json(),
                "proof_length_hashes": proof_length_last,
                "proof_size_bytes": proof_length_last * 32,
            },
            "consistency_proof": {
                "old_size": old_size,
                "new_size": tree_size,
                "generation_time_us": consistency_stats.to_json(),
                "proof_length_hashes": consistency_proof_length,
                "proof_size_bytes": consistency_proof_bytes,
            }
        }));

        println!(
            "    Inclusion (first): {:.0}us, {} hashes",
            first_stats.median_us, proof_length_first
        );
        println!(
            "    Inclusion (last):  {:.0}us, {} hashes",
            last_stats.median_us, proof_length_last
        );
        println!(
            "    Consistency:       {:.0}us, {} hashes",
            consistency_stats.median_us, consistency_proof_length
        );
    }

    let result = json!({
        "test": "Merkle Proof Performance",
        "unit": "microseconds",
        "proof_iterations": PROOF_ITERATIONS,
        "results": data_points,
        "expected_complexity": "O(log n) for proof length, O(n) for generation time due to load_all_hashes()",
        "notes": "Proof generation time is dominated by loading all hash nodes from SQLite (O(n_nodes)). The actual proof path computation is O(log n). Proof SIZE (number of hashes) grows as O(log n) as expected by RFC 6962."
    });

    write_result("merkle_performance.json", &result);
    println!("Merkle Performance: done");
}
