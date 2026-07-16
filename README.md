# ReInk

ReInk is an in-progress Rust port of
[ReInkPy](https://codeberg.org/atufi/reinkpy), a utility for inspecting and
resetting waste-ink counters on selected Epson printers.

The port aims to preserve the useful behavior of the Python implementation
while making its protocol layers independently testable and supporting Linux
and Windows through explicit OS adapters. It does **not** replace physical
waste-ink pads.

> [!WARNING]
> ReInk will eventually write persistent printer state. EEPROM writes and
> counter resets must remain explicit user actions, verify their result by
> default, and be used only after appropriate physical maintenance. Before the
> first connected-printer write in a session, ReInk must offer an EEPROM backup;
> declining it requires a separate explicit acknowledgement.

## Status

The workspace contains platform contracts, deterministic test doubles, Epson
domain logic, and a read-only application service that composes a selected
transport with the Epson D4 and control-channel layers. It includes read-only
CLI and terminal UI surfaces; its scripted D4 entry exchange is not
hardware-ready.

| Area | Status |
| --- | --- |
| OS-neutral transport and discovery contracts | Implemented |
| Scripted test transports, control channels, and discovery | Implemented |
| IEEE 1284 identity parsing | Implemented |
| Epson model database, command encoding, and EEPROM reply parsing | Implemented |
| IEEE 1284.4 framing, transactions, and service channels | Implemented |
| Epson command execution and EEPROM read/write orchestration | Implemented with scripted transports |
| Read-only Epson D4 application service | Implemented with scripted transports |
| Linux and macOS USB bulk transport, descriptor selection, and candidate enumeration | Implemented; no hardware validation claimed |
| SNMP control adapter and mDNS discovery | Implemented with deterministic tests |
| Windows native USB | Planned |
| Read-only CLI | Implemented |
| Read-only terminal UI model browser | Implemented |

## Porting approach

The Python source is being ported from the inside out:

1. Establish platform-neutral interfaces and hardware-free test doubles.
2. Port pure codecs and state machines with captured or scripted protocol
   fixtures.
3. Port Epson model configuration and EEPROM behavior over an abstract control
   channel.
4. Compose selected transports with application services, then add concrete
   device-file, SNMP, and mDNS adapters.
5. Build the CLI and optional terminal UI on the application-facing API.

Protocol and domain crates must not depend on USB, SNMP, terminal, or
OS-specific APIs. Hardware integration is confined to outer adapter crates.
This makes behavior reproducible in unit tests and prevents Windows or Linux
details from leaking into Epson protocol code.

### Execution model

The initial protocol and transport contracts are synchronous and blocking. D4
communication is a strictly ordered sequence of writes and replies, so a
blocking model keeps its state machine deterministic and its tests simple.

Async execution may be introduced at outer application boundaries when it
provides a concrete benefit, such as keeping a terminal UI responsive,
cancelling discovery or I/O, or scanning multiple devices concurrently. It
must not require the initial D4 or Epson protocol implementations to depend on
an async runtime.

## Architecture

```text
CLI / terminal UI / fixture GUI
        |
application service
        |
Epson domain commands ---- IEEE 1284.4 protocol
        |                         |
        +---------- platform contracts
                              |
            USB | device file | SNMP | mDNS adapters
```

Current and planned workspace crates:

| Crate | Responsibility |
| --- | --- |
| `reink-platform` | `ByteTransport`, `ControlChannel`, discovery contracts, selected-device types, and typed errors |
| `reink-platform-test` | Strict scripted transports, control channels, and discovery fakes for downstream tests |
| `reink-d4` | IEEE 1284.4 packet framing, revision negotiation, transaction/service channels, credits, close, and exit |
| `reink-core` | IEEE 1284 identity parsing, Epson model database, command encoding, and EEPROM reply parsing |
| `reink-app` | Read-only Epson D4 session and entry-probe application services |
| `reink-usb` | Read-only Linux and macOS libusb bulk transport, generic bounded exchange probes, and USB printer-interface selection |
| `reink-snmp` | Synchronous SNMP v1/v2c/v3 Epson control-channel adapter |
| `reink-discovery` | mDNS printer discovery and Linux read-only device-file enumeration |
| `reink-hardware-test` | Opt-in Linux and macOS read-only validation driver; hardware validation remains unclaimed |
| `reink-cli` | Read-only model, identity, and mDNS discovery commands |
| `reink-tui` | Read-only keyboard-driven model browser and workflow guide |
| `reink-gui` | Optional descriptor-only graphical read-only UI with a session-only future transport trace sink |

`reink-platform` supports USB selectors, explicit device paths, and IPv4/IPv6
network locations. Linux discovery enumerates `/dev/lp*` and `/dev/usb/lp*`
without opening devices. Windows support must use a proper USB backend rather
than assuming a POSIX-style device path.

### USB backend and driver policy

Linux and macOS USB support use a libusb-based adapter through `rusb` to select
printer-class interfaces and exchange bulk-endpoint traffic. Linux permissions
and kernel-driver handling remain the responsibility of that adapter.

Windows support will first investigate a native printer-stack adapter. It must
prove that a normally installed printer can carry the D4 entry sequence,
IEEE 1284.4 negotiation, and repeated bidirectional control-channel traffic
without creating print jobs or disrupting ordinary printing. Until that
prototype succeeds, Windows native USB access is not a supported transport.

ReInk must **never** install, replace, detach, rebind, or restore a Windows
device driver. A future libusb Windows adapter may support an interface that a
user has already configured with a compatible driver, but it must neither
perform nor prompt for that driver modification. If the selected adapter
cannot access the device, it reports a precise error rather than silently
switching transport methods.

All concrete transports normalize their native implementation to
`reink-platform::ByteTransport`. The D4 layer receives only ordered blocking
reads and writes; it must not depend on libusb transfer boundaries, Linux
device paths, or Windows printer handles. Native Windows discovery may
identify a printer by an installed-printer or device-interface handle rather
than USB vendor/product attributes.

### `reink-usb`

`reink-usb` uses `rusb` on Linux and macOS. It selects alternate-setting-zero USB
printer-class interfaces with both bulk-IN and bulk-OUT endpoints and
implements `ByteTransport` with bounded bulk I/O. Its optional bounded exchange
probe is protocol-neutral: callers provide request bytes, expected reply bytes,
and a read limit. By default, it refuses a Linux interface with an active
kernel driver. The read-only hardware driver can opt in with
`--allow-driver-handoff`, which temporarily detaches that selected Linux
interface's driver, then releases and reattaches only the driver it detached.
Reattachment failures are reported and may require recovery or a reboot. macOS
access uses only libusb's normal read/claim operations; the handoff flag does
not modify a macOS driver.

The Windows build contains no libusb transport and no driver-management code.
Windows support remains contingent on the native printer-stack prototype
described above.

The concrete Linux transport must be built and exercised on Linux (or in
Linux CI with libusb development headers). The macOS adapter is compiled in
macOS CI using `rusb`'s vendored libusb support; this is compilation and
scripted-test coverage only, not hardware validation. Cross-compiling the Linux
adapter from Windows also requires a Linux C compiler and sysroot for
`libusb1-sys`; the pure descriptor-selection tests remain host-independent.

For native Linux or WSL development, install `build-essential`, `pkg-config`,
`libusb-1.0-0-dev`, and `libudev-dev`, then use the stable Linux Rust
toolchain.

For macOS development, use the stable Xcode command-line tools and Rust
toolchain. No driver installation, detachment, rebinding, or manual workaround
is part of setup. Use `system_profiler SPUSBDataType` to obtain the selected
printer's vendor/product IDs and, when duplicate IDs are attached, its
libusb-visible bus and address. A libusb claim failure is a stop condition.

### `reink-core`

`reink-core` embeds the upstream `epson.toml` model database and exposes
typed, transport-independent operations:

- parse IEEE 1284 identity fields and standard aliases (`MFG`, `MDL`, `CMD`);
- load, validate, and look up Epson model specifications;
- derive merged waste-counter reset operations from model metadata;
- encode regular Epson commands and factory EEPROM read/write commands;
- parse EEPROM read replies without treating malformed responses as successful
  values;
- execute identity, EEPROM read, EEPROM write, and waste-counter reset
  operations through an abstract `ControlChannel`.

EEPROM writes use read-back verification by default. Atomic writes read all
original values before changing printer state and restore prior values after a
failed write. These operations are covered by scripted control-channel tests;
they are not authorization to write a physical printer without the evidence
and user-confirmation requirements below.

The database keeps upstream ordering semantics: if a model occurs in more than
one group, the later group overrides an earlier one. Some source entries have
minimum-counter metadata without explicit reset values. The initial port
retains that minimum metadata and uses zero reset bytes where the upstream
aggregate reset behavior does so; execution policy is not implemented yet.

The Python parser has an ambiguity for one-byte EEPROM addresses: it consumes
the first two bytes of a six-hex-digit reply. The Rust parser intentionally
preserves that behavior pending sanitized traffic fixtures from a real
one-byte-address printer.

### `reink-d4`

`reink-d4` consumes only `reink-platform::ByteTransport` and exposes an
`EPSON-CTRL` service channel as `ControlChannel`. It supports packet framing
across fragmented reads, transaction protocol revisions `0x10` and `0x20`,
revision fallback, service lookup/opening, and channel credit accounting.
The crate has no USB, OS, or UI dependency.

Build and test this layer independently with:

```powershell
cargo build -p reink-d4
cargo test -p reink-d4
```

### `reink-app`

`reink-app::EpsonD4Session` is the application-service boundary between a
selected `ByteTransport` and the Epson controller. It sends the source-
compatible Epson D4 entry exchange, initializes D4, opens `EPSON-CTRL`, and
exposes read-only identity and EEPROM operations. It also closes the service
channel and terminates D4 through `shutdown()`.

On Linux and macOS, `probe_epson_d4_entry` is the separate safe entry-probe API used by
the CLI and hardware-test driver. It owns the Epson request and reply
semantics while delegating bounded USB I/O to `reink-usb`; it stops before D4
Init or service setup. The D4 entry exchange is tested only with scripted
transports. Do not use it against hardware until the evidence requirements in
the protocol provenance plan have been met for the selected printer family.

Build and test the service independently with:

```powershell
cargo build -p reink-app
cargo test -p reink-app
```

### Network adapters

`reink-snmp` provides a synchronous SNMP v1/v2c/v3 adapter. It maps Epson
control requests to the vendor enterprise OID and reads the printer's IEEE
1284 device ID through the documented Printer-MIB extension OID. The library
redacts communities and USM credentials from debug output. `SnmpConfig` can
load credentials from `REINK_SNMP_*` environment variables so read-only host
applications and the CLI do not place secrets in command arguments.

`reink-discovery` browses `_ipp._tcp.local.`, `_ipps._tcp.local.`, and
`_printer._tcp.local.` using mDNS. Discovery results are network locations;
they are not proof of printer model or supported control access.

On Linux, it also enumerates `/dev/lp*` and `/dev/usb/lp*` character-device
nodes as explicit device-file selection candidates. Enumeration never opens a
device, sends traffic, or changes a driver binding.

### `reink-cli`

`reink-cli` contains only read-only commands. It does not accept write keys,
reset counters, or send EEPROM write requests.

`parse-id`, `snmp-id`, and `usb-id` report the parsed IEEE 1284 fields together
with a detected model candidate and any match in the built-in model database.
This model resolution is local metadata lookup; it neither opens a device for
`parse-id` nor grants write capability to any command.

```powershell
cargo run -p reink-cli -- models
cargo run -p reink-cli -- model C90
cargo run -p reink-cli -- parse-id "MFG:EPSON;MDL:C90;"
cargo run -p reink-cli -- discover --timeout-seconds 3
cargo run -p reink-cli -- --json models
```

On Linux, list local device-file candidates without opening them:

```powershell
cargo run -p reink-cli -- local-devices
```

For a standard USB Printer Class identity read, select the exact device and
interface. The command never enters Epson D4 mode and refuses to detach an
active Linux kernel driver. If multiple devices share vendor/product IDs, add
both `--bus-number` and `--device-address`; ReInk refuses to choose one
arbitrarily:

```powershell
cargo run -p reink-cli -- usb-id --vendor-id 0x04b8 --product-id <product-id> --interface <number>
```

Use your platform's USB listing tool to obtain the product, interface, and any
needed location values. Do not guess them and do not use this command on
Windows; its USB path is not supported. An active Linux kernel driver or a
failed macOS claim is a deliberate stop condition for these `reink-cli`
commands: they will not detach, rebind, install, or work around a driver.

`usb-d4-probe` is a separate, opt-in capture-only command. It sends the
source-compatible Epson entry sequence and stops before D4 Init, service
opening, EEPROM access, writes, or resets. It reports only a recognized reply
or a bounded byte count:

```powershell
cargo run -p reink-cli -- usb-d4-probe --vendor-id 0x04b8 --product-id <product-id> --interface <number>
```

Before selecting a device for a hardware-test command, Linux and macOS can
list descriptor-only USB printer candidates:

```powershell
cargo run -p reink-hardware-test -- usb-candidates
```

This command only reads libusb descriptors; it does not open or claim a
device, hand off a driver, send a control request, or send D4 traffic. Each
result alias (`usb-1`, and so on) is stable only within that report. Select a
candidate later with its complete shown selector. `model_hints` are bundled
database label/filter hints for an exact vendor/product mapping, not identity
or automatic selection; they may be empty. A later IEEE 1284 identity read
confirms the model. Windows returns the standard unsupported USB error.

For a complete Linux evidence run, the companion `reink-results` repository
provides `run-linux-read-evidence.sh`. With the repositories checked out as
siblings, run it from `reink-results`:

```bash
./run-linux-read-evidence.sh --allow-driver-handoff
```

The script selects a candidate only when exactly one descriptor candidate and
one exact bundled model hint exist; otherwise it requires
`--candidate-alias` and/or `--model`. It derives the model range, preserves
preflight, identity, selected-read, dump, and boundary-probe results in an
ignored timestamped private directory, and never performs a write or reset.
The identity result remains the authoritative model confirmation.

For a single structured read-only preflight report, use:

```powershell
cargo run -p reink-hardware-test -- read-sequence --vendor-id 0x04b8 --product-id <product-id> --interface <number>
```

`read-sequence` records USB device-ID, device-ID parsing, model resolution, and
the capture-only D4 entry probe as ordered `steps`. `d4-identity` records the
D4 session, identity read, and orderly shutdown; `d4-eeprom-read` does the same
for explicitly selected EEPROM addresses:

```powershell
cargo run -p reink-hardware-test -- d4-identity --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model>
cargo run -p reink-hardware-test -- d4-eeprom-read --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model> --address 0x000c
```

After successful identity and selected-address validation, `d4-eeprom-dump`
reads the selected model's declared EEPROM range one address at a time. It is
read-only, bounded to `mem_low..=mem_high`, and may be narrowed with
`--start-address` and `--end-address`. Preserve a successful dump privately:
EEPROM data may contain device-specific information and is not permission to
restore or write it.

```powershell
cargo run -p reink-hardware-test -- d4-eeprom-dump --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model>
```

For private protocol evidence, `d4-identity`, `d4-eeprom-read`,
`d4-eeprom-dump`, and the boundary probe accept an explicit `--trace-file
<outside-repository-path>`; those commands also accept `--report-file
<outside-repository-path>`. The report file contains exactly the structured
JSON printed on stdout on success. Both paths refuse overwrite and require an
existing parent directory.
The file is written only after D4 shutdown and USB close/driver-handoff cleanup
have been attempted, refuses to overwrite an existing path, and requires an
existing parent directory. It contains ordered `tx`/`rx` byte events as
uppercase hex in a versioned read-only JSON schema; it is not added to normal
stdout reports. Original traces are private, potentially device-specific
evidence and must remain outside and never be committed to this repository.

After manually redacting and reviewing a private trace, an operator can create a
local Rust transcript template. This command does **not** sanitize traffic: the
exact confirmation acknowledges that the operator already removed and reviewed
device-specific data. It validates the trace schema and byte-event boundaries,
refuses to overwrite the output path, and preserves every event in order. The
template is not automatically committable source; review every byte, add
assertions for the protected behavior, and review the resulting test before
adding it to the repository.

```powershell
cargo run -p reink-hardware-test -- trace-to-transcript --trace-file <reviewed-private-trace> --output-file <new-local-template.rs> --confirmation I_CONFIRM_TRACE_IS_SANITIZED --description "sanitized fixture"
```

```powershell
cargo run -p reink-hardware-test -- d4-identity --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model> --trace-file <outside-repository-path>
cargo run -p reink-hardware-test -- d4-eeprom-read --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model> --address 0x000c --report-file <outside-repository-path>
```

All successful reports use schema version 2 with `mode: "read_only"` and
ordered step objects (`name`, `status`, and `result`). Preserve those reports as
hardware evidence. The driver performs no physical write or reset operation;
its `write-sequence` command is deliberately unavailable. Failure reports are
also schema version 2 and record the failing stage without raw trace bytes or
invented successful EEPROM values.

Normal `d4-eeprom-read` rejects addresses outside the selected model's
`mem_low..=mem_high` range before opening USB. The separate
`d4-eeprom-boundary-probe` performs exactly one explicitly acknowledged
out-of-range read; it has no default address and rejects in-range addresses.
Its result is observed behavior only, never proof that an out-of-range read is
safe.

```powershell
cargo run -p reink-hardware-test -- d4-eeprom-boundary-probe --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model> --address 0xffff --confirm-out-of-range-read I_CONFIRM_THIS_IS_A_READ_ONLY_BOUNDARY_PROBE --report-file <outside-repository-path>
```

By default, `read-sequence`, `d4-identity`, `d4-eeprom-read`, and
`d4-eeprom-dump` refuse an interface with an active Linux kernel driver.
`--allow-driver-handoff` is an explicit maintenance acknowledgement for those
read-only commands only: on Linux it temporarily detaches, claims, releases,
then reattaches only the driver ReInk detached. D4 reports retain
`driver_handoff_enabled` and add `driver_handoff` with requested, detached, and
reattached (or not-applicable) outcomes; they never contain raw traffic. If reattachment
fails, recover the driver manually and a reboot may be required. On macOS the
flag does not attempt a kernel-driver handoff and normal claiming continues.

Concrete commands return nonzero for operational failures. When `--report-file`
is supplied after a D4 operation begins, they preserve a structured failure
report before returning nonzero. In particular, a failed EEPROM dump records
only completed-address count and failed address, never a partial values list.
If its explicit trace capture succeeds, the process still fails and labels that
file as incomplete private evidence.

`write-validation-plan` is a separate **non-executable** safety-gate report.
It never selects a USB device, opens a session, queues a write, or resets a
printer. It accepts only the SHA-256 reference of a separately retained,
sanitized read-only report and an exact acknowledgement:

```powershell
cargo run -p reink-hardware-test -- write-validation-plan --evidence-sha256 <64-hex-character-sha256> --confirmation I_CONFIRM_THIS_DOES_NOT_EXECUTE_WRITES
```

The report always has `execution: "disabled"`. Even when the sanitized-evidence
reference and acknowledgement gates are satisfied, its mandatory
`separate-write-safety-review` gate remains blocked. No current command can
turn that plan into a physical EEPROM write or reset.

`snmp-id` reads and parses an IEEE 1284 device ID through SNMP. It only reads
credentials from the process environment:

```powershell
$env:REINK_SNMP_HOST = "printer.example"
$env:REINK_SNMP_VERSION = "2c"
$env:REINK_SNMP_COMMUNITY = "<set outside shell history>"
cargo run -p reink-cli -- snmp-id
```

`REINK_SNMP_PORT` and `REINK_SNMP_TIMEOUT_SECONDS` are optional and default to
`161` and `2`. For SNMPv3, set `REINK_SNMP_USERNAME`; optionally set both
`REINK_SNMP_AUTH_PROTOCOL` and `REINK_SNMP_AUTH_PASSWORD`, and, when privacy
is needed, both `REINK_SNMP_PRIVACY_PROTOCOL` and
`REINK_SNMP_PRIVACY_PASSWORD`. Supported authentication algorithms are `md5`,
`sha1`, `sha224`, `sha256`, `sha384`, and `sha512`; privacy algorithms are
`des`, `aes128`, `aes192`, and `aes256`.

The CLI never accepts credentials as arguments, emits no credentials in JSON
or text output, and has no write/reset command.

### `reink-tui`

`reink-tui` is an interactive, read-only terminal UI. It browses the built-in
model database and can locally inspect a typed IEEE 1284 device ID against that
database; it does not open devices or expose EEPROM-write or counter-reset
actions.

```powershell
cargo run -p reink-tui
```

Use `Enter` or `M` to browse models, arrow keys or `J`/`K` to select a model,
and `Esc` or `Q` to return or exit. Use `I` to type and locally parse an IEEE
1284 ID; this only resolves bundled metadata and sends no traffic. `H` shows
the separate CLI discovery and Linux hardware-preflight workflows, but never
runs them or opens a device.

### `reink-gui`

`reink-gui` is an optional native GUI built with `egui`/`eframe`. It defaults to
descriptor/real mode with **no printer selected** and is separate from
`reink-tui`. On Linux and macOS it asynchronously lists
printer-class USB **descriptor-only candidates** with
`reink_usb::list_printer_candidates()`. This scan reads descriptors only: it
does not open or claim a device, detach or hand off a driver, send control, D4,
or EEPROM traffic, or enable writes. Candidates use a session-only alias and
show only VID/PID, bus/address, interface/alternate setting, and exact bundled
VID/PID model hints; hints are not identity confirmation. The GUI has no
driver-handoff control. Identity and EEPROM reads require a future explicit
read-only operation. On Windows, USB descriptor enumeration is unavailable and
no printer is selected by default. Default mode never opens or claims a device,
hands off a driver, or sends traffic.

Raw EEPROM files remain available above persistent `Status`, `EEPROM`, and
`Tools` tabs. Bundled fixtures are hidden unless explicitly enabled with
`--fixtures`; only that opt-in mode resolves fixture identity and runs
deterministic fixture validation. Local raw EEPROM images are inspected
read-only after an explicit model selection. The editing, reset, backup, and
restore controls remain unavailable; the GUI contains no write or reset path.

Its persistent shell and tab-specific sub-pane rules are documented in
[UI design](docs/UI_DESIGN.md).

The GUI also has a bottom **Debug traffic** panel: a live, bounded in-memory,
session-only sink for future recorded-session TX/RX events. Capture is disabled
until explicitly enabled. Selecting a descriptor candidate alone produces no
traffic, and no current GUI operation emits events or exports them.

The GUI is excluded from the workspace default members, so base CLI builds do
not require it. Build and run it explicitly on Windows, Linux, or macOS:

```powershell
cargo run -p reink-gui
cargo run -p reink-gui -- --fixtures
```

## Protocol provenance

The port distinguishes standards conformance from source-compatible or
reverse-engineered behavior. See
[protocol provenance and conformance plan](docs/PROTOCOL_PROVENANCE.md) for
the source-to-implementation map, evidence levels, D4 review checklist, and
vendor-command safety gate. No hardware adapter may treat the current D4
implementation as IEEE-conformant until that review is complete.

When hardware is available, follow the
[hardware capture and fixture guide](docs/HARDWARE_CAPTURE_GUIDE.md). It
defines the required read-only evidence, redaction rules, and strict transcript
replay approach before any hardware-derived fixture is committed.

## Fresh-system setup

These instructions assume a stock operating system and a new checkout. Linux
hardware maintenance may explicitly opt into the documented read-only driver
handoff; ordinary development does not modify USB drivers.

### Windows: build, test, and read-only UIs

Windows supports the workspace's pure crates, CLI, terminal UI, descriptor/real
GUI (with explicit fixture opt-in), mDNS, and SNMP paths. Native Windows USB
access is **not supported**. Do not install, replace,
detach, rebind, or restore a printer driver for ReInk.

1. Install [Git for Windows](https://git-scm.com/download/win).
2. Install the current stable Rust MSVC toolchain from
   [rustup](https://rustup.rs/).
3. Install Visual Studio 2022 Build Tools. Select **Desktop development with
   C++**, including the MSVC x64/x86 build tools and a Windows SDK.
4. Open **x64 Native Tools Command Prompt for VS 2022**. Do not use a normal
   Command Prompt unless the matching MSVC linker is already configured.
5. Run:

```powershell
rustup default stable-x86_64-pc-windows-msvc
rustup component add rustfmt clippy
git clone https://github.com/fes/reink-rust.git
cd reink-rust
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Run the safe local tools:

```powershell
cargo run -p reink-cli -- models
cargo run -p reink-cli -- --json models
cargo run -p reink-cli -- discover --timeout-seconds 3
cargo run -p reink-tui
cargo run -p reink-gui
```

`local-devices` and `usb-id` return an unsupported-platform error on Windows;
that is intentional.

### macOS: descriptor-only GUI candidates

The optional GUI can list descriptor-only USB printer candidates on macOS
without opening or configuring a printer. Install the current stable Xcode
command-line tools and Rust toolchain, then run:

```bash
cargo run -p reink-gui
```

### Linux: build, test, and read-only USB preflight

Use native Linux for direct USB work. WSL can build and test pure Rust code,
but is not the supported environment for the USB-printer preflight.

On Debian or Ubuntu, install the build dependencies:

```bash
sudo apt update
sudo apt install -y build-essential pkg-config libusb-1.0-0-dev libudev-dev git curl
```

On other distributions, install the equivalents of a C/C++ build toolchain,
`pkg-config`, libusb development headers, libudev development headers, Git,
and curl. For example, Fedora provides `libusb1-devel` and `systemd-devel`.

Install Rust and create the checkout:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
. "$HOME/.cargo/env"
rustup component add rustfmt clippy
git clone https://github.com/fes/reink-rust.git
cd reink-rust
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Run the safe local tools:

```bash
cargo run -p reink-cli -- models
cargo run -p reink-cli -- local-devices
cargo run -p reink-tui
cargo run -p reink-gui
```

For a physical USB printer, follow
[the Linux read-only USB checklist](docs/LINUX_USB_READONLY_COMMANDS.txt).
The only current USB request is the standard Printer Class device-ID read.
It requires an exact vendor/product/interface selection and refuses to detach
an active kernel driver.

### Instructions for coding agents and automation

1. Run the build, format, Clippy, and test commands above after source changes.
2. Treat all printer access as opt-in. Never run `usb-id`, D4, EEPROM, or reset
   commands against a device unless the user explicitly selects it.
3. Never install, detach, unload, rebind, replace, or restore a USB/printer
   driver. An active Linux kernel driver is a stop condition, not an error to
   work around.
4. Never commit raw captures, serial numbers, USB paths, IP addresses, SNMP
   credentials, or other device-specific data. Use sanitized transcripts only.
5. Do not add write/reset commands until the protocol-provenance safety gate
   and hardware evidence requirements are satisfied.

## Build, test, and lint

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The current hardware-free suite covers identity parsing, model-database
validation and counter merging, Epson command encoding, EEPROM reply parsing,
D4 state transitions, strict malformed transaction rejection, deterministic
packet-fragmentation matrices, transcript-replayed read-only sessions, the
read-only D4 application session, SNMP OID mapping, mDNS result conversion,
CLI argument parsing, and platform test doubles.
Hardware smoke tests, when added, will be opt-in and require an explicitly
selected device.

GitHub Actions runs this same formatting, Clippy, and test sequence on current
Linux, macOS, and Windows runners. Tagged `v*` revisions and manual dispatch create
release-build artifacts for the read-only CLI, hardware-test driver, and TUI;
publishing a release remains a maintainer action after reviewing those
artifacts.

## Application-service workflow

For the current D4 application-service workflow, embed `EpsonD4Session` in a
host application after that application has selected and opened a
`ByteTransport`:

```rust,no_run
let spec = reink_core::ModelDatabase::builtin()?
    .get("C90")
    .ok_or("unknown model")?
    .clone();
let mut session = reink_app::EpsonD4Session::connect(transport, spec)?;
let identity = session.read_identity()?;
let values = session.read_eeprom(&[0x000c])?;
session.shutdown()?;
```

`transport` must come from an explicitly selected adapter such as the Linux
USB backend. This API is read-only; no CLI command or application-level
hardware write/reset path exists yet.

## Compatibility and safety

The behavioral target is the current ReInkPy source, including Epson model
metadata and the D4/SNMP control paths. The Python repository has no unit-test
suite, so each ported behavior is documented through Rust tests and small,
sanitized fixtures.

Do not commit captured traffic containing printer serial numbers, IP addresses,
or other device-specific information. Do not add a USB driver installation or
rebind a physical printer as part of ordinary development setup. EEPROM writes
and reset operations must require explicit user confirmation, use read-back
verification by default, and report any rollback failure clearly.

## License

This port is licensed under the AGPL-3.0-or-later, consistent with ReInkPy.
