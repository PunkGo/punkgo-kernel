# Contributing to PunkGo Kernel

Thanks for your interest in PunkGo! This document explains how to contribute.

## Getting Started

### Prerequisites

- Rust 2024 edition (1.85+)
- SQLite (bundled via sqlx)

### Build & Test

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## How to Contribute

### Report Bugs

Open a GitHub issue with:
- Steps to reproduce
- Expected vs. actual behavior
- OS and Rust version

### Propose Changes

1. Fork the repository
2. Create a feature branch from `main`
3. Make your changes with tests
4. Run `cargo test --workspace && cargo clippy --workspace`
5. Submit a pull request

### Propose Governance Changes (PIPs)

PunkGo is governed by the **whitepaper** and **PIPs** (PunkGo Improvement Proposals).

- The whitepaper defines foundational axioms and invariants — it is not amended lightly.
- PIPs fill gaps in the whitepaper without overriding it.
- To propose a new PIP: open an issue titled `PIP-NNN: <title>` with a draft following the format in `docs/PIP-001_EN.md`.

All governance changes enter the event history (whitepaper §3, invariant 6).

## Code Style

- Follow existing patterns in the codebase
- Add `///` doc comments to all public types and functions
- Add `//!` module-level docs to new modules
- Use `KernelResult<T>` for fallible operations
- No `unwrap()` in production code — use `?` or `unwrap_or_default()`
- Tests go in the same file (`#[cfg(test)]` module) or in `tests/` for integration tests

## Architecture

```
bins/           CLI client + kernel daemon
crates/
  punkgo-core/      Core types (shared vocabulary)
  punkgo-runtime/   Kernel logic (7-step pipeline)
  punkgo-state/     SQLite persistence
  punkgo-sandbox/   Execution backends
  punkgo-audit/     Merkle tree proofs
  punkgo-testkit/   Test utilities
```

Key principle: **the kernel is a committer, not a judge.** It provides a single linearization point for actions, not moral authority.

## License

By contributing, you agree that your contributions will be licensed under the MIT License.
