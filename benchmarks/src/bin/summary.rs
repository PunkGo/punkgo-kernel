use punkgo_benchmarks::{read_result, write_result_string};

fn main() {
    let mut md = String::new();
    md.push_str("# PunkGo Kernel Benchmark Results Summary\n\n");

    // ===== Environment =====
    md.push_str("## Environment\n\n");
    if let Some(env) = read_result("environment.json") {
        md.push_str(&format!(
            "- **Date**: {}\n",
            env["benchmark"]["date"].as_str().unwrap_or("?")
        ));
        md.push_str(&format!(
            "- **OS**: {}\n",
            env["hardware"]["os"].as_str().unwrap_or("?")
        ));
        md.push_str(&format!(
            "- **CPU**: {}\n",
            env["hardware"]["cpu"].as_str().unwrap_or("?")
        ));
        md.push_str(&format!(
            "- **CPU Threads**: {}\n",
            env["hardware"]["cpu_threads"]
        ));
        md.push_str(&format!(
            "- **Rust**: {}\n",
            env["software"]["rust_version"].as_str().unwrap_or("?")
        ));
        md.push_str(&format!(
            "- **Git Commit**: {}\n",
            env["software"]["git_commit"].as_str().unwrap_or("?")
        ));
        md.push_str(&format!(
            "- **Storage Backend**: {}\n",
            env["software"]["storage_backend"].as_str().unwrap_or("?")
        ));
    } else {
        md.push_str("*environment.json not found*\n");
    }
    md.push('\n');

    // ===== Invariant Verification =====
    md.push_str("## Invariant Verification (Qualitative)\n\n");
    md.push_str("| Invariant | Status | Notes |\n");
    md.push_str("|-----------|--------|-------|\n");

    let inv_files = [
        ("INV-1 Append-Only", "inv1_append_only.json"),
        ("INV-2 Completeness", "inv2_completeness.json"),
        ("INV-3 Integrity", "inv3_integrity.json"),
        ("INV-4 Boundary", "inv4_boundary.json"),
        ("INV-5 Energy", "inv5_energy.json"),
    ];

    for (name, file) in &inv_files {
        if let Some(inv) = read_result(file) {
            let status = inv["status"].as_str().unwrap_or("?");
            let notes = if let Some(n) = inv["notes"].as_str() {
                n.chars().take(80).collect::<String>()
            } else {
                String::new()
            };
            md.push_str(&format!("| {name} | **{status}** | {notes} |\n"));
        } else {
            md.push_str(&format!("| {name} | *not run* | |\n"));
        }
    }
    md.push('\n');

    // ===== Pipeline Latency =====
    md.push_str("## Performance (Quantitative)\n\n");
    md.push_str("### Pipeline Latency\n\n");
    if let Some(lat) = read_result("pipeline_latency.json") {
        md.push_str("| Action Type | Median (us) | P95 (us) | Min (us) | Max (us) |\n");
        md.push_str("|-------------|-------------|----------|----------|----------|\n");
        for action_type in ["observe", "create", "mutate", "execute"] {
            if let Some(stats) = lat["results"].get(action_type) {
                md.push_str(&format!(
                    "| {} | {:.0} | {:.0} | {} | {} |\n",
                    action_type,
                    stats["median_us"].as_f64().unwrap_or(0.0),
                    stats["p95_us"].as_f64().unwrap_or(0.0),
                    stats["min_us"],
                    stats["max_us"],
                ));
            }
        }
        // read_events is a query, not a pipeline action — report separately
        if let Some(stats) = lat["results"].get("read_events") {
            md.push_str(&format!(
                "\n**Read query** (not a pipeline action): median {:.0} us, P95 {:.0} us\n",
                stats["median_us"].as_f64().unwrap_or(0.0),
                stats["p95_us"].as_f64().unwrap_or(0.0),
            ));
        }
    } else {
        md.push_str("*pipeline_latency.json not found*\n");
    }
    md.push('\n');

    // ===== Merkle Proof Scaling =====
    md.push_str("### Merkle Proof Scaling\n\n");
    if let Some(merkle) = read_result("merkle_performance.json") {
        md.push_str("| Log Size | Inclusion Time (us) | Proof Hashes | Proof Bytes |\n");
        md.push_str("|----------|---------------------|--------------|-------------|\n");
        if let Some(results) = merkle["results"].as_array() {
            for dp in results {
                let size = dp["log_size"].as_u64().unwrap_or(0);
                let inc = &dp["inclusion_proof_first_leaf"];
                let time = inc["generation_time_us"]["median_us"]
                    .as_f64()
                    .unwrap_or(0.0);
                let hashes = inc["proof_length_hashes"].as_u64().unwrap_or(0);
                let bytes = inc["proof_size_bytes"].as_u64().unwrap_or(0);
                md.push_str(&format!(
                    "| {size:>8} | {time:>19.0} | {hashes:>12} | {bytes:>11} |\n"
                ));
            }
        }
    } else {
        md.push_str("*merkle_performance.json not found*\n");
    }
    md.push('\n');

    // ===== Throughput =====
    md.push_str("### Throughput\n\n");
    if let Some(tp) = read_result("throughput.json") {
        md.push_str("| Scenario | Actions/sec | Total Actions |\n");
        md.push_str("|----------|-------------|---------------|\n");
        for scenario in [
            "observe_only",
            "create_only",
            "mutate_only",
            "execute_only",
            "mixed",
        ] {
            if let Some(s) = tp["results"].get(scenario) {
                md.push_str(&format!(
                    "| {} | {:.0} | {} |\n",
                    scenario,
                    s["throughput"].as_f64().unwrap_or(0.0),
                    s["actions_completed"],
                ));
            }
        }
    } else {
        md.push_str("*throughput.json not found*\n");
    }
    md.push('\n');

    // ===== Hold Workflow =====
    md.push_str("### Hold Workflow Latency\n\n");
    if let Some(hold) = read_result("hold_latency.json") {
        md.push_str("| Phase | Median (us) | Max (us) |\n");
        md.push_str("|-------|-------------|----------|\n");
        for phase in ["hold_trigger", "holds_pending_read", "approve", "reject"] {
            if let Some(s) = hold["results"].get(phase) {
                md.push_str(&format!(
                    "| {} | {:.0} | {} |\n",
                    phase,
                    s["median_us"].as_f64().unwrap_or(0.0),
                    s["max_us"],
                ));
            }
        }
        if let Some(e2e) = hold["results"]["end_to_end_trigger_to_approve_us"].as_f64() {
            md.push_str(&format!(
                "\n**End-to-end (trigger + approve)**: {e2e:.0} us\n"
            ));
        }
    } else {
        md.push_str("*hold_latency.json not found*\n");
    }
    md.push('\n');

    // ===== AIOS Comparison =====
    md.push_str("## AIOS Comparison\n\n");
    if let Some(comp) = read_result("aios_comparison.json") {
        md.push_str("| Dimension | AIOS | PunkGo |\n");
        md.push_str("|-----------|------|--------|\n");
        if let Some(dims) = comp["dimensions"].as_array() {
            for dim in dims {
                let name = dim["dimension"].as_str().unwrap_or("?");
                let aios = dim["aios"].as_str().unwrap_or("?");
                let punkgo = dim["punkgo_claim"].as_str().unwrap_or("?");
                md.push_str(&format!("| {name} | {aios} | {punkgo} |\n"));
            }
        }
    } else {
        md.push_str("*aios_comparison.json not found*\n");
    }
    md.push('\n');

    // ===== Implementation Gaps =====
    md.push_str("## Implementation Gaps\n\n");
    md.push_str("| Paper Concept | Status | Notes |\n");
    md.push_str("|---------------|--------|-------|\n");
    md.push_str("| prev_hash chain | Replaced by Merkle tree | Hash chain via tlog, not per-event prev_hash field |\n");
    md.push_str("| Hardware auto-detection (INT8 TOPS) | Config only | luminosity_source always 'config', no hardware probe |\n");
    md.push_str("| Execute backends | Actor responsibility (PIP-002) | Kernel validates payload, does not execute |\n");
    md.push_str(
        "| BlobStore | Removed from kernel (PIP-002) | Actor manages blob storage externally |\n",
    );
    md.push_str("| Snapshot refresh | Removed (v0.2.1) | Redundant with Merkle audit checkpoint; O(log n) replaces O(n) |\n");
    md.push_str(
        "| Formal verification (Verus) | Future work | Paper proofs only, no machine-checked |\n",
    );
    md.push_str("| TEE integration | Future work | No SGX/TDX support |\n");
    md.push_str("| Multi-Committer consensus | Future work | Single-writer only |\n");
    md.push('\n');

    // ===== Notes =====
    md.push_str("## Notes\n\n");
    md.push_str("- All benchmarks run with `--release` flag for accurate timing.\n");
    md.push_str("- Each benchmark uses a fresh kernel instance (clean SQLite database).\n");
    md.push_str("- Merkle proof generation time includes O(n) hash loading from SQLite (load_all_hashes). The actual proof path computation is O(log n).\n");
    md.push_str(
        "- PIP-002: Execute is actor-submitted result. Kernel validates payload format, no process spawn.\n",
    );
    md.push_str(
        "- v0.2.1: Audit checkpoint is now atomic with event commit (in-transaction). Snapshot refresh removed.\n",
    );
    md.push_str(
        "- Hold workflow timing excludes human decision time (measures mechanical latency only).\n",
    );

    md.push_str("\n---\n\n*Generated by punkgo-benchmarks summary tool*\n");

    write_result_string("SUMMARY.md", &md);
    println!("Summary: done — see results/SUMMARY.md");
}
