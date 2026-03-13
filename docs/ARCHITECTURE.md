# PunkGo Kernel Architecture

> Version: 0.2.2

## Overview

PunkGo Kernel is a local sovereignty compute kernel implementing the **Right to History**. It is a single-writer, append-only event system built on three primitives: **P** (Actor), **E** (Energy), **S** (State).

The kernel is a **committer, not an executor** — it validates, records, and proves actions. It does not run commands, serve as a blob store, or manage storage on behalf of actors.

## Crate Structure

```
punkgo-kernel/
  bins/
    punkgo-cli/          CLI client (IPC over Unix socket / Windows named pipe)
    punkgo-kerneld/      Kernel daemon (tokio + SQLite)
  crates/
    punkgo-core/         Core types: Actor, Action, Energy, Boundary, Consent (Envelope/Hold)
    punkgo-runtime/      Kernel: 7-step submit pipeline, lifecycle, energy producer
    punkgo-state/        Persistence: ActorStore, EnergyLedger, EventLog, EnvelopeStore, BlobStore
    punkgo-audit/        Audit: Merkle tree (tlog), inclusion/consistency proofs, C2SP checkpoints
    punkgo-testkit/      Test utilities: temp state dirs, request builders
  specs/
    kernel-tools.json    MCP-compatible tool definitions (machine-readable, no running kernel required)
```

## Submit Pipeline

Every action passes through a deterministic **7-step pipeline**:

```
validate → quote → reserve → validate_payload → settle → append → receipt
```

| Step | Responsibility |
|------|---------------|
| **Validate** | Check actor status (active/frozen), boundary permissions (PIP-001 §8), envelope authorization |
| **Quote** | Compute energy cost: observe=0, create=10, mutate=15, execute=25+output_bytes/256 |
| **Reserve** | Lock energy from actor's balance to prevent overdraft |
| **Validate Payload** | For execute actions: validate input_oid, output_oid, exit_code, artifact_hash (PIP-002 §2-§3) |
| **Settle** | Finalize energy accounting (reserved → consumed) |
| **Append** | Write committed event to append-only log with SHA-256 hash chain |
| **Receipt** | Return verifiable receipt: event_id, log_index, event_hash |

### Hold Integration

When an action matches a `hold_on` rule (PIP-001 §11):

```
validate → quote → reserve → write hold_request → return HoldTriggered
                              ↓ (envelope stays Active)
                    human approve/reject/timeout
                              ↓
              approve: validate_payload → settle → append → receipt
              reject:  settle 20% commitment cost → release remainder
              timeout: same as reject (lazy-check pattern)
```

The agent is **not blocked** — it may continue submitting other actions while holds are pending.

## Consent / Envelope System (PIP-001 §11)

Agents require **authorization envelopes** to perform state-changing actions. An envelope grants bounded permission from a Human (or parent agent) to an Agent.

### Envelope Fields

| Field | Type | Description |
|-------|------|-------------|
| `actor_id` | string | Agent receiving authorization |
| `grantor_id` | string | Human/parent granting authorization |
| `budget` | integer | Energy consumption upper bound |
| `targets` | string[] | Allowed target glob patterns |
| `actions` | string[] | Allowed action types |
| `duration_secs` | integer? | Time limit (null = no expiry) |
| `report_every` | integer? | Checkpoint reporting interval (energy units) |
| `hold_on` | HoldRule[] | Rules that intercept matching actions for human judgment |
| `hold_timeout_secs` | integer? | Auto-reject timeout for unresponded holds |
| `status` | enum | active / expired / revoked |

### Hold Rules

A `HoldRule` matches (target glob, action types). When an action matches:

1. Kernel quotes cost and reserves energy
2. Writes `hold_request` event
3. Returns `HoldTriggered` error with `hold_id`
4. Human responds via `mutate ledger/hold/<hold_id>` with `{decision: "approve"|"reject"}`
5. On reject/timeout: 20% commitment cost settled, remainder released

### Envelope Hierarchy

- Envelope permissions MUST be a subset of the grantor's writable_targets
- Parent envelope ID is tracked for chain-of-delegation auditing
- Budget is charged against the grantor's energy balance

## Energy System (PIP-001 §1-§4)

Energy is the universal cost metric, anchored to physical hardware:

- **Stellar Luminosity**: Kernel reads INT8 TOPS at startup as material anchor
- **Continuous Production**: Energy produced per tick, distributed proportionally by actor shares
- **Energy Neutrality**: Kernel holds no energy; all energy belongs to actors
- **Conservation**: `E' = E - cost(A)` — no energy created except through hardware ticks

## Actor Model (PIP-001 §5-§7)

| Property | Human | Agent |
|----------|-------|-------|
| Existence | Unconditional | Conditional (creator sets terms) |
| Deletion | Forbidden | Allowed (terminate) |
| Downgrade | Forbidden | Allowed (freeze) |
| Can create agents | Yes | No (PIP-001 §5) |
| Needs envelope | No | Yes (for state-changing actions) |

Lineage is tracked: every agent records its full creation chain back to the originating human.

## Boundary Enforcement (PIP-001 §8-§10)

- **Default deny**: If no `writable_target` matches the action's (target, action_type), it is rejected
- **Observe exempt**: Read operations bypass boundary checks
- **Privileged targets**: `system/*` and `ledger/*` are restricted to root (actors with `**` wildcard)
- **Subset enforcement**: Child actor boundaries must be a subset of parent's boundaries

## Execute Submission (PIP-002)

The kernel does **not** execute commands. The actor performs execution externally and submits the result:

```
Actor executes externally → Actor submits payload → Kernel validates format → Kernel records event
```

Required payload fields: `input_oid`, `output_oid`, `exit_code`, `artifact_hash` (all OIDs must be `sha256:<64 hex>`). The kernel validates format but does **not** verify OID content exists.

## Audit Trail

Every committed event enters a Merkle tree (Google tlog / RFC 6962):

- **Inclusion proof**: Prove a specific event exists in the tree
- **Consistency proof**: Prove the tree is append-only between two sizes
- **C2SP checkpoints**: Signed tree heads for independent third-party verification

## IPC Transport

Each daemon instance binds a unique per-PID address and writes it to `state/daemon.addr` for service discovery. Clients read this file to find the current daemon.

- Unix: `state/daemon-{pid}.sock` (Unix domain socket)
- Windows: `\\.\pipe\punkgo-kernel-{pid}` (named pipe)

Single-instance guarantee via `flock` on `daemon.addr` (auto-released on process death).

Request/response format:
```json
{
  "request_id": "uuid",
  "type": "quote | submit | read",
  "payload": { ... }
}
```

## Key Architectural Rules

1. **Kernel is ONLY a committer** — no BlobStore IPC, no general-purpose backend, no storage management for actors
2. **No a-priori restrictions** — design is opt-in, not pre-emptive
3. **Append-only history** — errors corrected by appending compensating events, never rewriting
4. **Hardware-anchored economics** — energy tied to physical compute, not arbitrary tokens
5. **Actor executes, kernel records** — symmetric across all action types (PIP-002)
