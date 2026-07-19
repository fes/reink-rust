# Maintaining ReInk

## Toolchain and validation

The repository pins Rust in `rust-toolchain.toml`. Do not advance it as part of
an unrelated change.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

For a focused change, run the smallest affected package test first, then the
workspace commands before merging. Physical hardware commands are manual and
opt-in; CI must never open a printer.

## Sources of truth

| Question | Canonical document |
| --- | --- |
| Crate boundaries and safety lifecycle | [`ARCHITECTURE.md`](ARCHITECTURE.md) |
| Platform/backend support | [`PLATFORM_CAPABILITIES.md`](PLATFORM_CAPABILITIES.md) |
| Protocol evidence | [`PROTOCOL_PROVENANCE.md`](PROTOCOL_PROVENANCE.md) |
| Physical evidence collection | [`HARDWARE_CAPTURE_GUIDE.md`](HARDWARE_CAPTURE_GUIDE.md) |
| GUI behavior | [`UI_DESIGN.md`](UI_DESIGN.md) |

The README is an entry point, not a second source of detailed platform status.

## Change checklist

1. Identify the owning crate before editing.
2. Reuse `reink-app` plans and lifecycle helpers instead of rebuilding safety
   logic in a CLI or GUI.
3. Preserve exact selection and platform capability reporting.
4. Add deterministic tests for behavior changes.
5. Update the canonical document only when platform or evidence status changes.
6. Keep raw evidence and all device-specific identifiers outside Git.
7. Do not broaden mutation reachability as incidental cleanup.

## Report compatibility

Hardware and trace reports carry `schema_version`. Add fields compatibly where
possible, retain explicitly documented legacy fields, and increment a schema
only for a real incompatible shape change. Tests should assert safety-relevant
fields rather than whole unstable JSON strings.

## Releases

Release automation builds the user-facing CLI, TUI, GUI, and hardware evidence
driver on the declared platform matrix. A release artifact is not a claim of
physical validation; the capability matrix remains authoritative.

## Efficient AI-assisted maintenance

- Start with this file and the owning crate rather than reading the full
  repository.
- Use `rg` for a symbol before opening large `main.rs` files.
- Keep one task within one architectural workstream when practical.
- Record durable platform/evidence decisions in the canonical documents.
- Delegate only independent investigations or noisy test execution.
- Prefer small behavior-preserving extractions over broad rewrites.
