# Contributing

ReInk controls persistent printer state. Contributions are welcome, but safety
and evidence boundaries take precedence over feature parity.

Before changing code, read:

- [`docs/MAINTAINING.md`](docs/MAINTAINING.md)
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
- [`docs/PLATFORM_CAPABILITIES.md`](docs/PLATFORM_CAPABILITIES.md)
- [`docs/PROTOCOL_PROVENANCE.md`](docs/PROTOCOL_PROVENANCE.md)

Use the pinned Rust toolchain and run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Do not attach raw USB/network traces, EEPROM images, printer serials, device
paths, addresses, credentials, or other private evidence to an issue or pull
request. Describe a reproducible sanitized observation instead.

Changes that expose writes or resets must preserve exact device/model
selection, bounded plans, durable complete backups, acknowledgements, read-back,
rollback, and cleanup reporting. Hardware access remains opt-in and must never
run in CI.
