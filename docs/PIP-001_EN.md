# PIP-001: Action

| Field | Value |
|-------|-------|
| PIP | 001 |
| Title | Action |
| Status | Draft |
| Fills | Whitepaper §2 (world model), §3 (invariant 6: governance auditable) |

---

> This PIP fills three gaps left unspecified by the whitepaper: **the source of E**, **the kinds of P**, and **the form of writability boundaries**.

### Keyword Convention

The keywords MUST, MUST NOT, SHALL, SHOULD, and MAY in this document are to be interpreted as described in [RFC 2119](https://datatracker.ietf.org/doc/html/rfc2119).

---

## Chapter 1: Energy Source

Whitepaper §2 gives E' = E - cost(A), but does not specify where E comes from or how it is distributed.

**§1 Stellar Luminosity** — At startup, the kernel MUST read the local hardware compute power (INT8 TOPS) as the material anchor for energy production.

**§2 Continuous Production** — The kernel MUST produce energy continuously per tick, at a rate determined by stellar luminosity. Each production event MUST be written to history.

**§3 Share Distribution** — Every Agent MUST have an energy share. Energy produced each tick is distributed proportionally by share among active agents. Humans do not participate in tick-based distribution — they receive a one-time initial balance. Shares are set at Actor creation time and can be updated via the `update_energy_share` lifecycle operation.

**§4 Cost Calibration** — Base costs: observe=0, create=10, mutate=15, execute=25+IO. The production rate MUST be greater than or equal to the cost of any base operation, ensuring the Right to History is never defeated by energy design.

---

## Chapter 2: Actor Types

Whitepaper §2 says "actor (human or agent)" but does not elaborate on the distinction or existence conditions.

**§5 Type Dichotomy** — An Actor MUST be exactly one of Human or Agent, exhaustive and mutually exclusive.

**§6 Human Unconditional Existence** — A Human Actor MUST NOT be deleted and MUST NOT be downgraded.

**§7 Agent Conditional Existence** — An Agent MUST declare its creator and purpose at creation time. The creator MAY set a time limit.

---

## Chapter 3: Writability Boundary

Whitepaper §3 invariant 6 requires governance to be auditable, but does not specify the concrete form of writability boundaries.

**§8 Writability Declaration** — Every Actor MUST declare writable_targets (target pattern + action types) at creation time. Default deny — if no writable_target matches, the action is rejected.

**§9 Privileged Targets** — `system/*` and `ledger/*` MUST be writable only by root. Root MAY delegate.

**§10 Root Wildcard** — Root's initial writable_targets MUST be `**`.

**§11 Envelope** — Temporary authorization MUST be issued via envelopes: budget + targets + actions + duration + checkpoint + hold_on + hold_timeout_secs. Envelope permissions MUST be a subset of the issuer's writable_targets.

> §11a **hold_on Declaration** — An Envelope MAY carry a `hold_on` rule set (target pattern + action type) at creation time.
>
> §11b **Trigger** — When an Action's (target, action_type) matches a hold_on rule, the Kernel MUST: (1) Quote the cost; (2) Reserve energy (lock to prevent overdraft); (3) Write Action info + reserved_cost as a hold_request event; (4) Return HoldTriggered. The Envelope stays Active — the Agent MAY continue submitting.
>
> §11c **Energy Lock** — Energy locked during a hold counts toward the used budget. Agent available energy = budget - consumed - reserved.
>
> §11d **Response** — A Human responds via mutate `ledger/hold/<hold_id>`: approve → skip quote/reserve → proceed to execute → settle → append; reject → settle commitment cost → release remainder → write hold_response event.
>
> §11e **Timeout** — An Envelope MAY set hold_timeout_secs. Timeout is equivalent to reject (partial deduction + release remainder), using lazy-check pattern.
>
> §11f **Reject/Timeout Energy Settlement** — On reject/timeout, the Kernel MUST settle commitment cost (20% of reserved_cost) and release the remainder. The commitment itself consumed hardware resources (validate + quote + reserve + write event), so work done incurs cost.

**§12 Execute Encapsulation** — The execute action's input/output MUST be encapsulated as a recordable structure: working directory isolation, output capture, and kernel timeout circuit-breaker.

---

*PIP-001 Draft.*
