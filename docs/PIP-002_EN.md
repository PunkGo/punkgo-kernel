# PIP-002: Execute Submission

| Field | Value |
|-------|-------|
| PIP | 002 |
| Title | Execute Submission |
| Status | Draft |
| Fills | Whitepaper §2 (execute action recordability) |
| Supersedes | PIP-001 §12 |

---

> This PIP redefines how execute actions are committed. PIP-001 §12 conflated execution with recording. This PIP separates the two: the actor executes, the kernel records.

### Keyword Convention

The keywords MUST, MUST NOT, SHALL, SHOULD, and MAY in this document are to be interpreted as described in [RFC 2119](https://datatracker.ietf.org/doc/html/rfc2119).

---

## Motivation

PIP-001 §12 states:

> The execute action's input/output MUST be encapsulated as a recordable structure: working directory isolation, output capture, and kernel timeout circuit-breaker.

This was implemented as the kernel spawning OS processes, waiting for completion, and capturing output. Three problems arise:

1. **Role confusion** — The kernel is a committer (whitepaper §2), not an executor. A committer provides a linearization point for actions; it does not perform them.
2. **Single-point risk** — A long-running command blocks the kernel's submit pipeline. A 300-second timeout is a structural liability, not a safety net.
3. **Asymmetry** — For mutate, the actor writes files and submits metadata (content_oid). For execute, the kernel performs the work. There is no principled reason for this difference.

This PIP restores symmetry: all four action types follow the same pattern — the actor acts, the kernel records.

---

## Specification

**§1 Actor Executes, Kernel Records** — The actor MUST perform the execution and submit the result. The kernel MUST NOT spawn, run, or wait for external processes on behalf of any actor.

**§2 Execute Payload Format** — An execute action's payload MUST contain:

| Field | Type | Description |
|-------|------|-------------|
| `input_oid` | OID (`sha256:<hex>`) | Reference to serialized input (command, args, env, working directory) |
| `output_oid` | OID (`sha256:<hex>`) | Reference to serialized output (stdout, stderr) |
| `exit_code` | integer | Process exit code |
| `artifact_hash` | string (`sha256:<hex>`) | SHA-256 of the raw output bytes |

The kernel MUST reject an execute submission that is missing any of these fields or uses an invalid OID format.

**§3 Kernel Validation Scope** — The kernel MUST validate:

- The payload contains all required fields with correct types
- The actor has sufficient energy and valid authorization (boundary + envelope)
- OID format is valid (`sha256:` followed by 64 hex characters)

The kernel MUST NOT validate whether the OID references point to actual content. The kernel records what the actor claims, consistent with mutate behavior.

**§4 Cost Calibration (amends PIP-001 §4)** — The execute base cost is 25. The IO surcharge is computed from the `output_bytes` field in the payload, reported by the actor. The formula remains: execute = 25 + output_bytes / 256.

**§5 PIP-001 §12 Superseded** — PIP-001 §12 is replaced in its entirety by this PIP. The terms "working directory isolation", "output capture", and "kernel timeout circuit-breaker" no longer apply to the kernel. Execution environment management is the actor's responsibility.

---

*PIP-002 Draft.*
