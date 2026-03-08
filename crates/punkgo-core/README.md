# PunkGo Core

[![crates.io](https://img.shields.io/crates/v/punkgo-core.svg)](https://crates.io/crates/punkgo-core)

Core type definitions for [PunkGo](https://punkgo.ai): actors, actions, energy, boundaries, consent, and IPC protocol.

This crate contains no runtime logic — it exists so lightweight consumers (like [punkgo-jack](https://github.com/PunkGo/punkgo-jack)) can depend on protocol types without pulling in the full kernel.

For the full engine, see **[punkgo-kernel](https://crates.io/crates/punkgo-kernel)**.

## License

[MIT](../../LICENSE)
