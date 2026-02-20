# Changelog

All notable changes to PunkGo Kernel will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-02-21

### Added

- **Whitepaper** — foundational axioms, P/E/S world model, and 7 invariants
- **PIP-001: Action** — energy source, actor types, writability boundary, and hold mechanism (§11)
- **7-step submit pipeline** — validate, quote, reserve, execute, settle, append, receipt
- **Energy system** — tick-based production anchored to hardware INT8 TOPS (stellar model)
- **Actor model** — Human (unconditional) and Agent (conditional) with lineage tracking
- **Boundary enforcement** — glob-based writable_targets with default-deny
- **Authorization envelopes** — budget tracking, two-level checkpoints (report/halt), hold rules
- **Hold mechanism** (PIP-001 §11) — `hold_on` rules, energy reservation on trigger, approve/reject/timeout with commitment cost
- **Actor lifecycle** — freeze, unfreeze, terminate with cascade
- **Execution sandbox** — ProcessBackend with timeout, output capture, artifact hashing; content mode for binary blobs
- **BlobStore** — content-addressable filesystem storage (Git-style CAS, OID `sha256:<hex>`)
- **Cryptographic audit trail** — Merkle tree (tlog), C2SP checkpoints, inclusion/consistency proofs
- **IPC daemon** (`punkgo-kerneld`) — Unix socket and Windows named pipe support
- **CLI client** (`punkgo-cli`) — read, quote, submit, seed-actor, audit commands
- **SQLite persistence** — actors, energy_ledger, envelopes, events, hold_requests, audit tables
- 149 tests across 6 crates
