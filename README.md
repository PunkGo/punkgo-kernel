# PunkGo Kernel

[Website](https://punkgo.ai) | [Whitepaper](docs/PunkGo_Whitepaper_EN.md) | [PIP-001](docs/PIP-001_EN.md) | [PIP-002](docs/PIP-002_EN.md)

[![CI](https://github.com/PunkGo/punkgo-kernel/actions/workflows/ci.yml/badge.svg)](https://github.com/PunkGo/punkgo-kernel/actions/workflows/ci.yml)

> When AI does everything for you, who proves you still exist?

**PunkGo** is a local sovereignty compute kernel: giving individuals the inalienable right to write history on their own hardware.

## What Is This?

PunkGo implements the **Right to History** — the principle that every actor (human or agent) can spend energy to perform actions, commit results to verifiable world state, and leave an indelible trace.

The kernel is a single-writer, append-only event system built on three primitives:

| Primitive | Description |
|-----------|-------------|
| **P** (Actor) | An actor — human or delegated agent |
| **E** (Energy) | Energy — the universal cost metric for actions |
| **S** (State) | State — all persistent reality and history, including the verifiable structure of how history was written |

State transitions follow:

```
S' = f(S, A, E)
E' = E - cost(A)
```

where **A** is an atomic action initiated by an actor: `observe`, `create`, `mutate`, or `execute`.

## Architecture

```
punkgo-kernel/
  bins/
    punkgo-cli/        # CLI client (IPC over Unix socket / Windows named pipe)
    punkgo-kerneld/    # Kernel daemon (tokio + SQLite)
  crates/
    punkgo-core/       # Core types: Actor, Energy, Action, Boundary, Consent
    punkgo-runtime/    # Kernel: 7-step submit pipeline, lifecycle, energy producer
    punkgo-state/      # Persistence: ActorStore, EnergyLedger, EventLog, EnvelopeStore, BlobStore
    punkgo-audit/      # Audit: Merkle tree, inclusion/consistency proofs, C2SP checkpoints
    punkgo-testkit/    # Test utilities: temp state dirs, request builders
```

### Submit Pipeline

Every action goes through a **7-step pipeline**:

```
validate → quote → reserve → validate_payload → settle → append → receipt
```

1. **Validate** — check actor status, boundary permissions, action well-formedness
2. **Quote** — compute energy cost for the action
3. **Reserve** — lock energy from actor's balance
4. **Validate Payload** — for `execute` actions, validate the actor-submitted result (PIP-002 §2)
5. **Settle** — finalize energy accounting
6. **Append** — write the committed event to the append-only log with cryptographic hash
7. **Receipt** — return a verifiable receipt (event ID, log index, event hash)

### Energy System

Energy is produced continuously, anchored to the machine's hardware compute power (INT8 TOPS). Each tick, energy is distributed proportionally among active actors based on their shares. The kernel itself holds no energy (energy neutrality).

### Actor Model

- **Human** — unconditional existence, cannot be deleted or downgraded
- **Agent** — conditional existence, created by humans with declared purpose and bounded permissions
- Only humans can create agents (PIP-001 §5/§6)
- Agents require authorization envelopes with budget, checkpoint levels, and expiry
- Agents can be supervised via **hold rules** (`hold_on`) at envelope creation — matching actions are intercepted for Human judgment before execution (PIP-001 §11)

### Boundary Enforcement

Every actor declares `writable_targets` (glob pattern + action types) at creation time. Access is default-deny. Privileged targets (`system/*`, `ledger/*`) are restricted to root. Child actor boundaries must be a subset of their parent's boundary.

### Execute Submission (PIP-002)

The kernel is a **committer, not an executor**. For `execute` actions, the actor performs the execution externally and submits the result. The kernel validates the payload format and records the event.

The execute payload must contain:

| Field | Format | Description |
|-------|--------|-------------|
| `input_oid` | `sha256:<64 hex>` | Reference to serialized input |
| `output_oid` | `sha256:<64 hex>` | Reference to serialized output |
| `exit_code` | integer | Process exit code |
| `artifact_hash` | `sha256:<64 hex>` | SHA-256 of raw output bytes |
| `output_bytes` | integer (optional) | Output size for IO cost calculation |

Energy cost: `25 + output_bytes / 256`. The kernel does not verify whether OID references point to actual content — it records what the actor claims, consistent with `mutate` behavior.

### Audit Trail

Every committed event is added to a Merkle tree (Google tlog algorithm):

- **Inclusion proof** — prove a specific event exists in the tree (RFC 6962)
- **Consistency proof** — prove the tree is append-only between two sizes (RFC 6962)
- **C2SP checkpoints** — signed tree heads for independent verification

These proofs enable whitepaper invariants §3.5 (append-only provable) and §3.7 (independently verifiable).

## Quick Start

### Prerequisites

- Rust 2024 edition (1.85+)

### Build & Test

```bash
cargo build --workspace
cargo test --workspace
```

### Start the Kernel Daemon

```bash
cargo run --bin punkgo-kerneld
```

### Use the CLI

```bash
# Check kernel health
cargo run --bin punkgo-cli -- read health

# Create a new actor
cargo run --bin punkgo-cli -- seed-actor alice --energy 5000

# Check actor energy
cargo run --bin punkgo-cli -- read actor_energy --actor-id root

# Submit an action (JSON payload)
cargo run --bin punkgo-cli -- submit '{"actor_id":"root","action_type":"mutate","target":"workspace/hello","payload":{"msg":"world"}}'

# View recent events
cargo run --bin punkgo-cli -- read events --limit 5

# Query the audit trail
cargo run --bin punkgo-cli -- audit checkpoint
cargo run --bin punkgo-cli -- audit proof 0
```

## Governance

PunkGo is governed by two documents:

- **PunkGo Whitepaper** ([EN](docs/PunkGo_Whitepaper_EN.md) | [ZH](docs/PunkGo_Whitepaper_ZH.md)) — foundational axioms, world model, and 7 invariants
- **PIP-001: Action** ([EN](docs/PIP-001_EN.md) | [ZH](docs/PIP-001_ZH.md)) — energy source, actor types, writability boundaries, and hold mechanism (§11)
- **PIP-002: Execute Submission** ([EN](docs/PIP-002_EN.md) | [ZH](docs/PIP-002_ZH.md)) — actor executes, kernel records (supersedes PIP-001 §12)

All governance changes enter the event history (whitepaper §3, invariant 6).

New rules are introduced exclusively through **PIPs** (PunkGo Improvement Proposals) — they fill gaps in the whitepaper without overriding it.

## Design Philosophy

- **The kernel is a committer, not a judge** — it provides a single linearization point for actions, not moral authority
- **No a-priori restrictions** — design is opt-in, not pre-emptive
- **Append-only history** — errors are corrected by appending compensating events, never by rewriting
- **Hardware-anchored economics** — energy production is tied to physical compute, not arbitrary tokens
- **Right to History** — every individual can write to the world state through their own hardware

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for build instructions, code style, and how to propose PIPs.

## License

[MIT](LICENSE)
