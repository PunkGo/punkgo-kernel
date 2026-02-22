# PunkGo Kernel Benchmarks

Standalone benchmark suite for the PunkGo Kernel, producing quantitative and qualitative data for the "Right to History" paper §6 Evaluation.

## Structure

```
benchmarks/
├── src/
│   ├── lib.rs                    # Shared helpers (bootstrap, submit, stats)
│   └── bin/
│       ├── environment.rs        # Environment info collection
│       ├── inv1_append_only.rs   # INV-1: Append-only verification
│       ├── inv2_completeness.rs  # INV-2: Completeness verification
│       ├── inv3_integrity.rs     # INV-3: Merkle integrity verification
│       ├── inv4_boundary.rs      # INV-4: Boundary enforcement verification
│       ├── inv5_energy.rs        # INV-5: Energy conservation verification
│       ├── pipeline_latency.rs   # Pipeline latency per action type
│       ├── merkle_performance.rs # Merkle proof scaling (O(log n))
│       ├── throughput.rs         # Sustained throughput (actions/sec)
│       ├── hold_latency.rs       # Hold workflow latency breakdown
│       ├── aios_comparison.rs    # AIOS qualitative comparison (8 dimensions)
│       └── summary.rs           # Aggregates all results into SUMMARY.md
├── results/                      # Output directory (git-ignored)
├── Cargo.toml
└── .gitignore
```

## Usage

This is a standalone Cargo project (not part of the workspace). Run from the `benchmarks/` directory:

```bash
cd benchmarks

# Run all benchmarks
cargo run --bin environment --release
cargo run --bin inv1_append_only --release
cargo run --bin inv2_completeness --release
cargo run --bin inv3_integrity --release
cargo run --bin inv4_boundary --release
cargo run --bin inv5_energy --release
cargo run --bin pipeline_latency --release
cargo run --bin merkle_performance --release
cargo run --bin throughput --release
cargo run --bin hold_latency --release
cargo run --bin aios_comparison --release
cargo run --bin summary --release
```

Results are written to `results/` as JSON files, plus a `SUMMARY.md` overview.

## Design

- **Read-only**: No kernel source code is modified. All benchmarks use the public Kernel API.
- **Fresh state**: Each benchmark bootstraps a fresh kernel instance with a clean SQLite database.
- **Independent verification**: INV-1 and INV-3 include independent RFC 6962 Merkle proof verifiers that do not use kernel code.
- **CI-compatible**: Code passes `cargo fmt`, `cargo clippy -- -D warnings`, and `RUSTFLAGS=-Dwarnings cargo check`.

## Invariant Tests

| Test | Verifies |
|------|----------|
| INV-1 | Append-only: direct SQLite tamper is detected via Merkle proof invalidation |
| INV-2 | Completeness: every successful action has a corresponding event record |
| INV-3 | Integrity: all inclusion/consistency proofs verified by independent RFC 6962 verifier |
| INV-4 | Boundary: default-deny enforcement, observe exemption, privileged target protection |
| INV-5 | Energy conservation: balance never negative, 20% commitment cost on hold reject |

## Performance Benchmarks

| Benchmark | Measures |
|-----------|----------|
| pipeline_latency | Per-action-type latency including execute (median, P95, min, max) |
| merkle_performance | Proof generation time and size across log sizes 10–1000 |
| throughput | Sustained actions/sec (observe, create, mutate, execute, mixed) over 5-second windows |
| hold_latency | Hold trigger, read, approve, reject latency breakdown |
