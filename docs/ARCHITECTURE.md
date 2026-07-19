# Architecture

ReInk isolates protocol behavior from operating-system and user-interface code.
Dependencies flow downward only:

```text
reink-cli / reink-tui / reink-gui / reink-hardware-test
                         |
                      reink-app
                         |
             reink-core + reink-d4
                         |
                   reink-platform
                         |
      reink-usb / reink-snmp / reink-discovery
```

`reink-platform-test` provides deterministic adapters for tests and must not
become a production dependency.

## Crate ownership

| Crate | Owns |
| --- | --- |
| `reink-platform` | Transport, control-channel, discovery, selector, and error contracts |
| `reink-platform-test` | Strict scripted transports and discovery doubles |
| `reink-d4` | IEEE 1284.4 framing, transactions, channels, credit, and lifecycle |
| `reink-core` | Identity parsing, model metadata, Epson commands, replies, and reset plans |
| `reink-app` | D4 sessions, identity validation, durable images, write plans, and cleanup |
| `reink-usb` | libusb adapters and the isolated Windows USBPRINT adapter |
| `reink-snmp` | Synchronous SNMP transport for supported read operations |
| `reink-discovery` | mDNS and Linux device-file discovery |
| `reink-cli` | User-facing command-line workflows |
| `reink-tui` | Read-only terminal browser |
| `reink-gui` | Guarded native application and protocol-aware tracing |
| `reink-hardware-test` | Opt-in physical evidence and reversible write evidence |

## Binary module map

The largest application binaries keep entry-point orchestration in `main.rs`
while isolating cohesive detail:

| Binary | Focused modules |
| --- | --- |
| `reink-cli` | `offline.rs` for bounded private binary analysis; `tests.rs` for command and rendering tests |
| `reink-gui` | `view_helpers.rs` for EEPROM rendering and cleanup presentation; `tests.rs` for UI guard tests |
| `reink-hardware-test` | `evidence_files.rs` for trace/report/private-file handling; `tests.rs` for evidence contracts |

Continue this pattern when a new cohesive responsibility would otherwise make
an entry point harder to inspect. Do not split a safety lifecycle merely to
reduce line count.

## Architectural rules

1. Protocol and domain crates do not depend on USB, SNMP, operating-system, or
   UI implementations.
2. Concrete adapters normalize native I/O to `reink-platform` contracts.
3. A device is selected before it is opened. Ambiguous selectors fail.
4. The D4 lifecycle remains synchronous and ordered. Concurrency belongs at
   application boundaries.
5. Persistent operations use model-bounded plans assembled by `reink-app`.
6. Platform capability differences remain explicit; no backend is selected as
   a silent fallback.
7. Raw captures, device paths, serials, EEPROM images, network addresses, and
   credentials never enter Git.

## Persistent-operation lifecycle

Every supported EEPROM write, restore, or reset follows the same shape:

1. resolve one exact device and expected model;
2. read D4 identity and require an exact model match;
3. validate all addresses against model metadata;
4. create and synchronize a complete new backup;
5. require the operation-specific acknowledgement;
6. apply the bounded plan with read-back verification;
7. roll back atomically when verification fails;
8. close D4 and restore any driver ReInk handed off; and
9. report both operation and cleanup failures.

Windows USBPRINT mutation has an additional acknowledgement because its physical
behavior remains experimental.

## Evidence boundary

Protocol claims use the levels defined in
[`PROTOCOL_PROVENANCE.md`](PROTOCOL_PROVENANCE.md). Physical evidence is first
kept private, then represented in Git only by deliberately sanitized summaries
or fixtures. Hardware-test commands are evidence collectors, not automatic
product smoke tests.
