# PIP-003: Checkpoint Authentication

> Status: **Implemented** (kernel v0.5.0)
> Fills: Whitepaper §3, Invariant 5 (append-only provable)

## Abstract

PIP-003 adds cryptographic identity binding to Merkle checkpoints. Every checkpoint is signed with a locally generated Ed25519 keypair, embedded as a C2SP extension line. This is the first layer of a three-layer trust architecture.

## Motivation

The Merkle tree (PIP-001) guarantees append-only integrity: no event can be silently deleted or modified. However, without signing, anyone who can access the database could rebuild the entire tree from scratch and claim it as authentic. PIP-003 adds identity binding: you can now verify *which kernel instance* produced a given checkpoint.

## Specification

### §1 Signing Key Lifecycle

On first boot, the kernel generates an Ed25519 keypair and stores the 32-byte seed at `{state_dir}/signing_key`. On subsequent boots, it loads the existing key.

- Key format: Ed25519 (RFC 8032), 32-byte seed
- Storage: plaintext file at `{state_dir}/signing_key`
- Generation: automatic on first boot, no user interaction required

### §2 Checkpoint Signing

Every C2SP checkpoint includes a signature extension line:

```
punkgo/kernel
{tree_size}
{root_hash}

sig/ed25519:{pubkey_hex}:{signature_hex}
```

The signature covers the checkpoint body (origin + tree_size + root_hash, newline-delimited). The extension line format is compatible with C2SP tlog-checkpoint and Go's `sum.golang.org`.

### §3 Verification

Third-party verification requires only the public key and the checkpoint text:

```rust
verify_checkpoint_signature(pubkey_hex, checkpoint_body, sig_hex) -> bool
```

The `signing_pubkey` IPC read kind exposes the kernel's current public key.

### §4 IPC Interface

New read kind:

```json
{ "kind": "signing_pubkey" }
→ { "pubkey_hex": "...", "algorithm": "ed25519" }
```

### §5 Trust Model

Ed25519 signing provides **identity binding**, not tamper-proofing.

| Attack | Prevented? | Why |
|--------|-----------|-----|
| Silent event modification | Yes | Merkle tree detects it |
| Checkpoint forgery by third party | Yes | Requires private key |
| Full database rebuild by root operator | **No** | Root has the signing key |
| Backdating a checkpoint | **No** | Requires time binding (TSA) |

The honest limitation: a root operator with database access *and* the signing key can rebuild history. This is the single-machine trust boundary — sufficient for local sovereignty (you trust your own hardware), but not for trustless third-party audit.

### §6 Trust Layer Architecture

PIP-003 establishes the first two layers of a three-layer trust architecture:

| Layer | Mechanism | Provides | Status |
|-------|-----------|----------|--------|
| Merkle | tlog / RFC 6962 | Ordering + integrity | Implemented (v0.1.0) |
| Ed25519 | Checkpoint signing | Identity binding | **Implemented (v0.5.0)** |
| TSA | RFC 3161 timestamp | Time binding | Implemented in punkgo-jack |

Each layer strictly adds to the previous — no layer replaces another.

Future layers (PunkGo Pool for collective witnessing) will extend this architecture without modifying existing layers.

## Dependencies

- `ed25519-dalek` v2 (with `rand_core`)
- `rand` v0.8
- `hex` v0.4

## Backward Compatibility

- Existing checkpoints (pre-v0.5.0) remain valid; they simply lack a signature extension line
- The signing key is generated automatically; no migration or user action required
- `CREATE TABLE IF NOT EXISTS audit_tsa_tokens` is included for forward compatibility with jack-side TSA storage
