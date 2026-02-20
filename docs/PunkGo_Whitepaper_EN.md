# PunkGo Whitepaper

> When AI does everything for you, who proves you still exist?

PunkGo is a local sovereignty compute kernel: giving individuals the inalienable right to write history on their own hardware.

---

## §1 Rights

**Right to History** — An actor spends its own energy to initiate actions, commits results to verifiable world state, and leaves an indelible trace.

---

## §2 System

The system is composed of three primitive objects:

- **P (Actor)** — An actor (human or agent)
- **E (Energy)** — Energy, the universal cost metric for actions
- **S (State)** — State, all persistent reality and history, including the verifiable structure of how history was written

The system advances by action, not by time:

- **S' = f(S, A, E)**
- **E' = E - cost(A)**

where A is an atomic action initiated by P: observe (read), create (create), mutate (modify), execute (execute, must be encapsulated with recordable input/output).

The system MUST have a unique **Committer** — a structural role, not a moral authority — responsible for turning Actions into committed events, providing a single linearization commit point.

---

## §3 Invariants

1. **Single Committer** — A single history domain has exactly one commit authority
2. **Total Order Log** — Committed events form a single total-order sequence
3. **State Reconstructability** — S can be derived by replaying events
4. **Semantic Append-Only** — Committed events cannot be rewritten; corrections are made only by appending compensating events
5. **Provable Append-Only** — The append-only property of history can be independently verified
6. **Governance Auditable** — Governance rules and their changes enter the event history
7. **Evidence Independently Verifiable** — Third parties can independently verify history integrity
