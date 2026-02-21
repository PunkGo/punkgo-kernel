use punkgo_benchmarks::write_result;
use serde_json::json;

fn main() {
    let result = json!({
        "comparison": "PunkGo Kernel vs AIOS",
        "dimensions": [
            {
                "dimension": "Trusted Computing Base (TCB)",
                "aios": "LLM (non-deterministic, unverifiable)",
                "punkgo_claim": "Kernel (deterministic, formally verifiable)",
                "punkgo_evidence": "Kernel is a Rust user-space process. All security decisions (boundary check, energy check, hold trigger) are deterministic code paths in kernel.rs. No LLM in the TCB. Pipeline: validate -> quote -> reserve -> payload_validate -> settle -> append is a fixed, synchronous, linear sequence (PIP-002: kernel does not execute, only records).",
                "code_references": [
                    "crates/punkgo-runtime/src/kernel.rs — submit_action() pipeline",
                    "crates/punkgo-core/src/boundary.rs — check_writable_boundary()",
                    "crates/punkgo-core/src/policy.rs — validate_action()"
                ]
            },
            {
                "dimension": "Audit Log",
                "aios": "None",
                "punkgo_claim": "RFC 6962 Merkle tree (append-only, hash-chained)",
                "punkgo_evidence": "Event log uses SHA-256 leaf hashes (0x00 prefix, RFC 6962) stored in SQLite. Merkle tree maintained via tlog_tiles crate (Google tlog algorithm). Every committed action produces an EventRecord with event_hash. Append-only: no DELETE or UPDATE operations exist in the codebase for events table.",
                "code_references": [
                    "crates/punkgo-state/src/event_log.rs — compute_event_hash() with RFC 6962 0x00 prefix",
                    "crates/punkgo-audit/src/lib.rs — AuditLog: append_leaf, make_checkpoint",
                    "crates/punkgo-state/src/store.rs — events table DDL (CREATE TABLE)"
                ]
            },
            {
                "dimension": "Inclusion Proof",
                "aios": "None",
                "punkgo_claim": "O(log n) — RFC 6962 Merkle audit path",
                "punkgo_evidence": "AuditLog::inclusion_proof() calls tlog::prove_record(), returns Vec<Hash> of O(log n) sibling hashes. Third party can verify event existence using only the root hash and proof path. Proof size: ceil(log2(n)) * 32 bytes.",
                "code_references": [
                    "crates/punkgo-audit/src/lib.rs — inclusion_proof()",
                    "crates/punkgo-runtime/src/kernel.rs — 'audit_inclusion_proof' read query"
                ]
            },
            {
                "dimension": "Consistency Proof",
                "aios": "None",
                "punkgo_claim": "O(log n) — proves log is append-only extension",
                "punkgo_evidence": "AuditLog::consistency_proof() calls tlog::prove_tree(), returns proof that old tree is prefix of new tree. Detects log forking (equivocation). C2SP tlog-checkpoint format for signed tree heads.",
                "code_references": [
                    "crates/punkgo-audit/src/lib.rs — consistency_proof()",
                    "crates/punkgo-runtime/src/kernel.rs — 'audit_consistency_proof' read query"
                ]
            },
            {
                "dimension": "Behavior Isolation",
                "aios": "Permission group hashmap (coarse-grained, no formal model)",
                "punkgo_claim": "Capability-based, default deny, glob-pattern writable_targets",
                "punkgo_evidence": "check_writable_boundary() enforces default-deny: agent cannot access any target not in its writable_targets. Glob patterns with literal_separator (/ is significant). Privileged targets (system/*, ledger/*) require root wildcard. Observe is always exempt. Child agent targets must be subset of parent (validate_child_targets).",
                "code_references": [
                    "crates/punkgo-core/src/boundary.rs — check_writable_boundary(), glob_match()",
                    "crates/punkgo-core/src/boundary.rs — validate_child_targets()",
                    "crates/punkgo-runtime/tests/boundary_enforcement.rs — 5 integration tests"
                ]
            },
            {
                "dimension": "Resource Governance",
                "aios": "Token scheduling (LLM token limits)",
                "punkgo_claim": "Energy budget + envelope (hardware-anchored stamina)",
                "punkgo_evidence": "Energy is derived from INT8 TOPS (stellar.toml). Tick-based production distributes energy proportionally by share. Envelope grants bounded budget to agents. Reserve step checks available energy before execution. InsufficientEnergy error prevents overdraft. Balance never goes negative.",
                "code_references": [
                    "crates/punkgo-core/src/stellar.rs — compute_energy_per_tick(), StellarConfig",
                    "crates/punkgo-state/src/energy_ledger.rs — reserve(), settle()",
                    "crates/punkgo-runtime/src/energy_producer.rs — produce_tick()"
                ]
            },
            {
                "dimension": "Human Approval",
                "aios": "None",
                "punkgo_claim": "hold_on rules + 20% commitment cost",
                "punkgo_evidence": "Envelope can declare hold_on rules (glob patterns). Matching actions trigger atomic hold: reserve energy + write hold_request event + create hold_requests row in single SQLite transaction. Human approves/rejects via mutate ledger/hold/<id>. Reject charges 20% commitment cost (ceil rounding). Timeout auto-rejects with same cost. Agent is NOT blocked during hold (envelope stays Active).",
                "code_references": [
                    "crates/punkgo-core/src/consent.rs — check_hold_trigger(), HoldRule",
                    "crates/punkgo-runtime/src/kernel.rs — hold trigger block, execute_hold_response()",
                    "crates/punkgo-runtime/tests/hold_tests.rs — 17 integration tests"
                ]
            },
            {
                "dimension": "Formal Invariants",
                "aios": "None",
                "punkgo_claim": "5 invariants with semi-formal proofs",
                "punkgo_evidence": "Paper defines 5 invariants: (1) Append-Only — no delete/modify operations exist; (2) Completeness — every pipeline path ends in append; (3) Integrity — Merkle root = MerkleRoot(log); (4) Boundary Enforcement — validate is first pipeline step, default deny; (5) Energy Conservation — reserve before settle, balance >= 0. All verified by integration tests.",
                "code_references": [
                    "crates/punkgo-runtime/tests/ — 120+ tests covering all invariants",
                    "Paper §4.6 — formal statement and proofs"
                ]
            }
        ],
        "notes": "AIOS assessment based on Mei et al. 'AIOS: LLM Agent Operating System' (COLM 2025). PunkGo evidence is from direct source code inspection of the v0.1.0 codebase."
    });

    write_result("aios_comparison.json", &result);
    println!("AIOS Comparison: done");
}
