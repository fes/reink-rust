# ReInk

> [!IMPORTANT]
> This Rust port is 100% AI-written and remains experimental. Review the source,
> validate behavior against your hardware, and retain complete backups before
> any persistent printer operation.

ReInk is a Rust port of
[ReInkPy](https://codeberg.org/atufi/reinkpy) for inspecting and maintaining
selected Epson printers. It separates protocol, model, transport, application,
and UI layers so most behavior can be tested without a printer.

ReInk does **not** replace physical waste-ink pads or other required
maintenance.

## Status

Implemented surfaces include:

- IEEE 1284 identity parsing and IEEE 1284.4/D4 sessions;
- Epson model metadata, status, EEPROM reads, writes, restore, and declared
  counter-reset plans;
- Linux, macOS, and Windows libusb adapters;
- a separate Windows USBPRINT backend;
- SNMP reads and mDNS discovery;
- CLI, read-only TUI, guarded GUI, and physical evidence driver;
- durable complete backups, read-back verification, and rollback; and
- protocol-aware GUI tracing with private evidence controls.

Platform implementation and validation status is maintained in
[`docs/PLATFORM_CAPABILITIES.md`](docs/PLATFORM_CAPABILITIES.md).

## Safety model

ReInk never performs a persistent operation automatically. A supported write,
restore, or reset requires:

1. one exact device and expected model;
2. an exact D4 identity match;
3. model-bounded addresses;
4. a new complete backup synchronized before writing;
5. the command-specific acknowledgement;
6. read-back verification and rollback; and
7. explicit transport, D4, and driver cleanup reporting.

Windows USBPRINT mutation is separately named, requires an additional
experimental acknowledgement, and remains physically unvalidated.

ReInk never installs a driver. Linux can temporarily hand off one selected
interface. macOS handoff captures and re-enumerates the entire selected USB
device and normally requires `sudo`. Windows libusb only claims an already
accessible interface; the USBPRINT backend uses the installed stock driver.

## Build

The repository pins its Rust toolchain. Install platform prerequisites, then:

```bash
cargo build --workspace
cargo test --workspace
```

### Linux

Install a C toolchain, `pkg-config`, libusb, and libudev development headers:

```bash
sudo apt-get install build-essential pkg-config libusb-1.0-0-dev libudev-dev
```

### macOS

Install Xcode command-line tools and Rust. No driver installation is required.
Device capture may prompt for `sudo` only when a selected operation needs it.

### Windows

Install Rust with the MSVC toolchain plus Visual Studio Build Tools **Desktop
development with C++** and a Windows SDK. Run builds from an x64 Native Tools
shell when the linker is not already configured.

## Applications

Run the guarded GUI:

```bash
cargo run -p reink-gui
```

Inspect models and offline data:

```bash
cargo run -p reink-cli -- models
cargo run -p reink-cli -- model L1300
cargo run -p reink-cli -- parse-id '<IEEE-1284-device-id>'
cargo run -p reink-cli -- analyze-binary <private-local-file>
```

List descriptor-only USB printer candidates without opening a device:

```bash
cargo run -p reink-hardware-test -- usb-candidates
```

Inspect exact driver ownership without claiming, detaching, or sending printer
traffic:

```bash
cargo run -p reink-hardware-test -- usb-driver-state \
  --vendor-id 0x04b8 --product-id <product-id> --interface <interface> \
  --bus-number <bus> --device-address <address>
```

Use `--help` on `reink`, `reink-hardware-test`, or a subcommand for the current
complete argument set. Hardware evidence is intentionally separated from normal
application workflows; follow
[`docs/HARDWARE_CAPTURE_GUIDE.md`](docs/HARDWARE_CAPTURE_GUIDE.md) and the
companion `reink-results` runners.

## Workspace

| Crate | Responsibility |
| --- | --- |
| `reink-platform` | Transport, control-channel, selector, discovery, and error contracts |
| `reink-platform-test` | Deterministic platform test doubles |
| `reink-d4` | IEEE 1284.4 framing and lifecycle |
| `reink-core` | Identity, models, Epson commands, replies, and reset plans |
| `reink-app` | D4 sessions, durable images, plans, verification, and cleanup |
| `reink-usb` | libusb and Windows USBPRINT adapters |
| `reink-snmp` | Synchronous SNMP reads |
| `reink-discovery` | mDNS and Linux device-file discovery |
| `reink-cli` | Inspection and explicit maintenance commands |
| `reink-tui` | Read-only terminal browser |
| `reink-gui` | Guarded graphical workflow and tracing |
| `reink-hardware-test` | Opt-in physical and reversible-write evidence |

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for dependency and lifecycle
rules.

## Documentation

- [Maintainer guide](docs/MAINTAINING.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Platform capabilities](docs/PLATFORM_CAPABILITIES.md)
- [Protocol provenance](docs/PROTOCOL_PROVENANCE.md)
- [Hardware capture guide](docs/HARDWARE_CAPTURE_GUIDE.md)
- [GUI design](docs/UI_DESIGN.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

Raw captures, EEPROM images, serial numbers, native device paths, network
addresses, credentials, and host-specific data must remain outside Git.

## Development checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI runs these checks on Linux, macOS, and Windows. Physical hardware access is
manual and never runs in CI.

## License

ReInk is licensed under
[GNU Affero General Public License v3.0 or later](LICENSE), consistent with
ReInkPy.
