# Changelog

All notable changes to PunkGo Kernel will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.1] - 2026-03-17

### Fixed

- **Windows daemon.addr unreadable** — exclusive flock on `daemon.addr` blocked other processes from reading it on Windows (mandatory locks), causing jack to fall back to a stale default endpoint. Fix: flock moved to separate `daemon.lock` file; `daemon.addr` is now always readable for endpoint discovery

## [0.5.0] - 2026-03-16

### Added

- **PIP-003: Ed25519 checkpoint signing** — every Merkle checkpoint is now signed with a locally generated Ed25519 keypair. Signature format: `sig/ed25519:<pubkey_hex>:<sig_hex>` in the C2SP checkpoint extension line
- **Signing key auto-generation** — keypair created on first boot at `{state_dir}/signing_key`, loaded on subsequent starts
- **`signing_pubkey` read kind** — IPC clients can query the kernel's Ed25519 public key for offline verification
- **`audit_tsa_tokens` table** — reserved schema for jack-side RFC 3161 TSA timestamp storage (forward-compatible, kernel does not make HTTP calls)
- **`verify_checkpoint_signature()` and `parse_sig_extension()`** — public functions for third-party offline signature verification

### Dependencies

- Added: `ed25519-dalek` v2 (with `rand_core`), `rand` v0.8, `hex` v0.4

## [0.4.0] - 2026-03-13

### Changed

- **IPC lifecycle redesign** — daemon now uses per-PID socket/pipe names (`daemon-{pid}.sock` / `\\.\pipe\punkgo-kernel-{pid}`), eliminating stale socket/pipe issues after crashes on all platforms
- **Single-instance guard** — replaced PID lockfile with `flock` on `daemon.addr` (auto-released on process death by OS)
- **Service discovery** — daemon writes `daemon.addr` file with PID + endpoint; clients read it to find the daemon
- **`--replace` flag** — gracefully stops old daemon via IPC shutdown command, then takes over
- **IPC shutdown command** — `kind: "shutdown"` request triggers graceful daemon shutdown

### Removed

- PID lockfile (`daemon.pid`) — superseded by flock on `daemon.addr`
- Interactive "Kill it? [y/N]" prompt — replaced by `--replace` flag

## [0.3.0] - 2026-03-13

### Fixed

- **Energy starvation bug** — agents received 0 energy per tick due to `floor()` rounding when root's `energy_share` dominated the total. Distribution now targets agents only (`actor_type = 'agent'`); humans receive one-time initial balance
- **Windows IPC "Access Denied"** — default endpoint changed to file-path pipe (`\\.\pipe\punkgo-kernel`) to avoid `GenericNamespaced` permission issues
- **NaN/Infinity validation** — `update_energy_share` rejects non-finite values that would poison tick distribution

### Added

- **`update_energy_share` lifecycle operation** — runtime adjustment of an agent's energy share via `mutate` action
- **PID lockfile** (`daemon.pid`) — daemon startup detects and prompts to kill existing instances
- **`PUNKGO_IPC_ENDPOINT` env var** — override the default IPC endpoint

### Changed

- **Root is genesis-only** — removed hardcoded root exemption from lifecycle authorization; root now follows lineage-based checks like any human
- **Root default `energy_share`** — 100.0 → 0.0 for new databases (existing databases unaffected due to `ON CONFLICT DO NOTHING`)
- **PIP-001 §3** — clarified that share distribution applies to agents only

## [0.2.1] - 2026-02-22

### Fixed

- **Audit atomicity** — event commit and Merkle tree update (append_leaf + make_checkpoint) now execute in the same transaction across all commit paths (finalize, hold_request, hold_response, hold_timeout), fixing a race condition where concurrent writes could produce an inconsistent audit tree (whitepaper §3 invariant 5)

### Removed

- **State snapshot** — removed redundant O(n) integrity hash (`refresh_snapshot`, `compute_snapshot_from_events`, `SnapshotInfo`, `snapshots_root`); audit checkpoint (Merkle tree root) provides strictly stronger guarantees with O(log n) updates. The `snapshot` read query now returns audit checkpoint data for backward compatibility.
- **PIP-003 draft** — concurrent access is handled by the existing daemon architecture (`punkgo-kerneld`); the remaining audit atomicity fix and snapshot removal are bug fixes, not a new specification

## [0.2.0] - 2026-02-22

### Added

- **PIP-002: Execute Submission** ([EN](docs/PIP-002_EN.md) | [ZH](docs/PIP-002_ZH.md)) — actor executes externally, kernel validates and records
- **Execute payload validation** — kernel validates `input_oid`, `output_oid`, `exit_code`, `artifact_hash` format (PIP-002 §2–§3)
- **IO-based execute cost** — `25 + output_bytes / 256`, replacing payload-size formula (PIP-002 §4)
- **Benchmark suite** — pipeline_latency, throughput, hold_latency, inv4_boundary, aios_comparison, summary (paper §6 evaluation)

### Changed

- **Submit pipeline step 4** — renamed from "execute" to "validate_payload"; kernel no longer spawns OS processes
- **Kernel role** — pure committer: validates format + authorization, does not verify OID content or execute commands
- **`ExecutePayloadInvalid` error** — replaces `Sandbox` error variant for structured error responses
- **Execute cost formula** — `quote_cost(Execute)` now uses actor-reported `output_bytes` instead of serialized payload size

### Removed

- **`punkgo-sandbox` crate** — removed from workspace; execution is now actor responsibility (PIP-002 §1)
- **BlobStore dependency in kernel** — kernel no longer manages content storage; actors handle blob lifecycle
- **`SYSTEM_TIMEOUT_HARD_LIMIT_MS`** — no longer needed without process execution
- **Sandbox-related kernel fields** — `backend_registry`, `sandbox_config`, `blob_store`

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
