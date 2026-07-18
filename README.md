# ReInk

> [!IMPORTANT]
> This Rust port is 100% AI-written. Treat it as experimental software: review
> the source, validate behavior against hardware, and retain backups before any
> persistent printer operation.

ReInk is an in-progress Rust port of
[ReInkPy](https://codeberg.org/atufi/reinkpy), a utility for inspecting and
resetting waste-ink counters on selected Epson printers.

The port aims to preserve the useful behavior of the Python implementation
while making its protocol layers independently testable and supporting Linux
and Windows through explicit OS adapters. It does **not** replace physical
waste-ink pads.

> [!WARNING]
> When ReInk writes persistent printer state, EEPROM writes and counter resets
> must remain explicit user actions, verify their result by default, and be used
> only after appropriate physical maintenance. A physical write requires an
> explicitly authorized target operation, a complete backup, and the
> command-specific acknowledgements.

## Status

The workspace contains platform contracts, deterministic test doubles, Epson
domain logic, and an application service that composes a selected transport
with the Epson D4 and control-channel layers. It includes explicit EEPROM
operations and a dedicated reversible physical write-evidence command. No
hardware write is automatic: it requires a selected target and every command
specific gate.

| Area | Status |
| --- | --- |
| OS-neutral transport and discovery contracts | Implemented |
| Scripted test transports, control channels, and discovery | Implemented |
| IEEE 1284 identity parsing | Implemented |
| Epson model database, command encoding, printer status, and EEPROM reply parsing | Implemented |
| IEEE 1284.4 framing, transactions, and service channels | Implemented |
| Epson command execution, read-only status, and EEPROM read/write orchestration | Implemented with scripted tests, read-back verification, and atomic rollback |
| Epson D4 application service with status and write plans | Implemented; used by explicit read workflows, confirmed CLI mutations, and gated hardware write evidence |
| Linux, macOS, and Windows USB bulk transport, descriptor selection, and candidate enumeration | Implemented; used by explicit read workflows and gated write evidence |
| SNMP control adapter, safe Epson status/EEPROM inspection, and mDNS discovery | Implemented with deterministic tests |
| Windows native USB sessions | Implemented through libusb; no automatic write path |
| CLI inspection, offline binary analysis, selected USB status, SNMP EEPROM inspection, and explicit EEPROM operations | Implemented with confirmation and backup safeguards |
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
CLI / terminal UI / guarded GUI
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
| `reink-app` | Epson D4 session, entry-probe, and validated write-plan application services |
| `reink-usb` | Linux, macOS, and Windows libusb bulk transport, generic bounded exchange probes, and USB printer-interface selection |
| `reink-snmp` | Synchronous SNMP v1/v2c/v3 Epson control-channel adapter |
| `reink-discovery` | mDNS printer discovery and Linux read-only device-file enumeration |
| `reink-hardware-test` | Opt-in Linux, macOS, and Windows validation driver, including a gated reversible single-byte write-evidence command |
| `reink-cli` | Model, identity, discovery, and explicit USB EEPROM commands |
| `reink-tui` | Read-only keyboard-driven model browser and workflow guide |
| `reink-gui` | Optional guarded graphical UI for selected USB status, durable EEPROM images, and explicitly confirmed EEPROM operations |

`reink-platform` supports USB selectors, explicit device paths, and IPv4/IPv6
network locations. Linux discovery enumerates `/dev/lp*` and `/dev/usb/lp*`
without opening devices. Windows support must use a proper USB backend rather
than assuming a POSIX-style device path.

### USB backend and driver policy

Linux, macOS, and Windows USB support use a libusb-based adapter through
`rusb` to select printer-class interfaces and exchange bulk-endpoint traffic.
Linux permissions and kernel-driver handling remain the responsibility of that
adapter.

ReInk does not install drivers. On Windows and macOS it only attempts a libusb
claim on an already accessible explicitly selected interface. It never
installs, detaches, rebinds, changes, or restores a driver association on those
platforms; a claim failure is a safe stop and ReInk does not silently switch
transport methods.

All concrete transports normalize their native implementation to
`reink-platform::ByteTransport`. The D4 layer receives only ordered blocking
reads and writes; it must not depend on libusb transfer boundaries, Linux
device paths, or Windows printer handles. Native Windows discovery may
identify a printer by an installed-printer or device-interface handle rather
than USB vendor/product attributes.

### `reink-usb`

`reink-usb` uses `rusb` on Linux, macOS, and Windows. It selects
alternate-setting-zero USB printer-class interfaces with both bulk-IN and bulk-OUT endpoints and
implements `ByteTransport` with bounded bulk I/O. Its optional bounded exchange
probe is protocol-neutral: callers provide request bytes, expected reply bytes,
and a read limit. After a Linux interface is explicitly selected, ReInk
automatically detaches an active driver for that interface, then releases and
reattaches only the driver it detached after each operation. Reattachment
failures report recovery guidance: reconnect or power-cycle the printer, then
reboot the host if needed before retrying. macOS access currently uses only
libusb's normal read/claim operations. Windows uses the same normal
read/claim/release lifecycle and never performs driver installation, detach,
rebind, or restoration.

The concrete Linux transport must be built and exercised on Linux. The macOS
and Windows adapters use
`rusb`'s vendored libusb support. This is compilation and scripted-test
coverage only, not hardware validation. Cross-compiling the Linux adapter from
Windows also requires a Linux C compiler and sysroot for `libusb1-sys`; the
pure descriptor-selection tests remain host-independent.

For native Linux or WSL development, install `build-essential`, `pkg-config`,
`libusb-1.0-0-dev`, and `libudev-dev`, then use the stable Linux Rust
toolchain.

For macOS development, use the stable Xcode command-line tools and Rust
toolchain. No driver installation is part of setup. Use
`system_profiler SPUSBDataType` to obtain the selected printer's vendor/product
IDs and, when duplicate IDs are attached, its libusb-visible bus and address.
On macOS and Windows, a claim failure is a safe stop: do not install or change
a driver to work around it.

### `reink-core`

`reink-core` embeds the upstream `epson.toml` model database and exposes
typed, transport-independent operations:

- parse IEEE 1284 identity fields and standard aliases (`MFG`, `MDL`, `CMD`);
- load, validate, and look up Epson model specifications;
- derive model-aware waste and platen-pad reset plans using only explicitly
  declared reset bytes;
- encode regular Epson commands and factory EEPROM read/write commands;
- parse EEPROM read replies without treating malformed responses as successful
  values;
- execute identity, raw printer status, EEPROM read, EEPROM write, and declared
  counter-reset operations through an abstract `ControlChannel`.

EEPROM writes use read-back verification by default. Atomic writes read all
original values before changing printer state and restore prior values after a
failed write. These operations are covered by scripted control-channel tests;
they are not authorization to write a physical printer without the evidence
and user-confirmation requirements below.

The database keeps upstream ordering semantics: if a model occurs in more than
one group, the later group overrides an earlier one. Minimum-counter metadata
is retained, but a missing `reset` array is not silently substituted with zero
bytes. Guarded semantic-reset plans exclude those entries and write only
explicitly declared bytes.

The Python parser has an ambiguity for one-byte EEPROM addresses: it consumes
the first two bytes of a six-hex-digit reply. The Rust parser intentionally
preserves that behavior pending sanitized traffic fixtures from a real
one-byte-address printer.

### `reink-d4`

`reink-d4` consumes only `reink-platform::ByteTransport` and exposes an
`EPSON-CTRL` service channel as `ControlChannel`. It supports packet framing
across fragmented reads, transaction protocol revisions `0x10` and `0x20`,
revision fallback, service-to-socket and socket-to-service lookup, channel
opening, and credit accounting.
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
exposes identity, raw status, EEPROM, and validated write-plan operations. It also closes
the service channel and terminates D4 through `shutdown()`.

On Linux, macOS, and Windows, `probe_epson_d4_entry` is the separate safe entry-probe API used by
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
1284 device ID through the documented Printer-MIB extension OID. The CLI
composes that control channel with the core controller only for read-only
status and EEPROM inspection; it exposes no SNMP write or reset command. The
library redacts communities and USM credentials from debug output. `SnmpConfig`
can load credentials from `REINK_SNMP_*` environment variables so read-only
host applications and the CLI do not place secrets in command arguments.

`reink-discovery` browses `_ipp._tcp.local.`, `_ipps._tcp.local.`, and
`_printer._tcp.local.` using mDNS. Discovery results are network locations;
they are not proof of printer model or supported control access.

On Linux, it also enumerates `/dev/lp*` and `/dev/usb/lp*` character-device
nodes as explicit device-file selection candidates. Enumeration never opens a
device, sends traffic, or changes a driver binding.

### `reink-cli`

`reink-cli` provides inspection commands, offline binary analysis, and
explicitly confirmed EEPROM write, restore, and declared counter-reset
commands. It never performs a default or automatic write; mutations require a
selected target, exact model match, exact confirmation, and a create-new
complete backup.

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
cargo run -p reink-cli -- analyze-binary <local-capture-or-binary>
```

On Linux, list local device-file candidates without opening them:

```powershell
cargo run -p reink-cli -- local-devices
```

For a standard USB Printer Class identity read, select the exact device and
interface. The command never enters Epson D4 mode. On Linux, it automatically
detaches and reattaches an active driver only for that selected interface. On
Windows and macOS, it only claims an already libusb-accessible interface and
never changes a driver. If multiple devices share vendor/product IDs, add both
`--bus-number` and `--device-address`; ReInk refuses to choose one arbitrarily:

```powershell
cargo run -p reink-cli -- usb-id --vendor-id 0x04b8 --product-id <product-id> --interface <number>
```

Use your platform's USB listing tool or ReInk's descriptor-only candidate
listing to obtain the product, interface, and required location values. Do not
guess them. On Windows and macOS, an access or claim failure is a safe stop;
do not install, detach, rebind, or change a driver to work around it. On Linux,
failure to detach, claim, or reattach the selected interface reports reconnect,
power-cycle, and reboot remediation.

`usb-d4-probe` is a separate, opt-in capture-only command. It sends the
source-compatible Epson entry sequence and stops before D4 Init, service
opening, EEPROM access, writes, or resets. It reports only a recognized reply
or a bounded byte count:

```powershell
cargo run -p reink-cli -- usb-d4-probe --vendor-id 0x04b8 --product-id <product-id> --interface <number>
```

`usb-status` opens a normal selected D4 session, reads the D4 identity, and
requires its model to exactly match `--model` before sending the read-only Epson
`st` request. It always follows the same orderly D4 shutdown and USB close
path as EEPROM inspection. It does not invoke `usb-d4-probe`, write EEPROM,
reset a counter, or issue any other mutation:

```powershell
cargo run -p reink-cli -- usb-status --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model>
```

On Linux, this uses the existing selected-interface driver handoff and cleanup
path; it detaches and reattaches only a driver ReInk detached. The safe Linux
inspection surfaces are `local-devices`, `usb-id`, `usb-status`,
`usb-eeprom-dump`, `snmp-id`, `snmp-status`, `snmp-eeprom-read`, and
`snmp-eeprom-dump`. The opt-in `usb-d4-probe` remains separate and is not part
of those normal inspection commands.

`usb-eeprom-dump` saves a complete model-bounded binary image. It reads the
D4 identity in the same session and rejects a model mismatch before reading
EEPROM. The output path must be new and have an existing parent directory.

On Linux and macOS, `usb-eeprom-write`, `usb-eeprom-restore`, and
`usb-eeprom-reset` are confirmed CLI mutation commands. They require an exact
D4 model match, an exact confirmation, and a new backup path before USB is
opened. `usb-eeprom-reset` accepts either `waste` or `platen-pad`, then derives
updates only from explicitly declared `reset` arrays for that model; entries
with only `min` metadata are never zero-filled. Each command saves and syncs a
complete image before applying its plan, verifies every write by read-back,
rolls back every attempted byte after a failure, and always attempts orderly D4
shutdown and USB close. Cleanup failures are included in the command error. On
all supported USB platforms, use the separate
`d4-eeprom-write-evidence` command below when the goal is to write and restore
one byte as auditable physical evidence.

```powershell
cargo run -p reink-cli -- usb-eeprom-dump --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model> --output-file <new-image.bin>
cargo run -p reink-cli -- usb-eeprom-write --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model> --update 0x000c=0x00 --backup-file <new-backup.bin> --confirmation I_CONFIRM_THIS_WILL_WRITE_EEPROM
cargo run -p reink-cli -- usb-eeprom-restore --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model> --input-file <complete-image.bin> --rollback-backup-file <new-rollback.bin> --confirmation I_CONFIRM_THIS_WILL_RESTORE_EEPROM
cargo run -p reink-cli -- usb-eeprom-reset --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model> --target waste --backup-file <new-reset-backup.bin> --confirmation I_CONFIRM_THIS_WILL_RESET_DECLARED_COUNTERS
```

Write updates must be unique and within the selected model range. Before any
write, ReInk calls `prepare_eeprom_write`, saves and syncs the complete
create-new backup, then calls `apply_eeprom_write`, which enables read-back
verification and rollback. Restore rejects a missing or wrongly sized image
before USB access and maps every image byte in order onto the selected declared
range; it saves a complete create-new rollback backup before applying that
plan. EEPROM images and backups are private device-specific data: retain them
securely and never commit them.

### `reink-gui`

The optional GUI starts in descriptor-only mode. Selecting a candidate never
opens USB or starts an operation. On Linux and Windows (and macOS where the
existing libusb interface claim is accessible), explicit selected-printer
buttons run status, complete EEPROM dump, generic byte write, full-image
restore, and model-aware waste/platen-pad resets on a worker thread. Every
connected operation requires an operator-selected expected bundled model; any
exact VID/PID association is only a hint, and the D4 identity must exactly
match before access.

Persistent GUI operations require a user-selected create-new, synchronized full
backup and action-specific typed confirmation. Writes use the existing
read-back and rollback plan. Restore additionally requires a user-selected
complete image with the exact selected-model length. The result pane reports
preflight/current values, read-back or rollback outcome, D4 shutdown, and USB
cleanup. Debug transfer recording remains session-only and is captured only
when its existing opt-in was enabled before the operation began. EEPROM images,
backup paths, and transport traffic are private and are never written to
default logs.

`analyze-binary` is an offline, local-only port of ReInkPy's `search_bin`
helper. It recognizes bounded Epson factory-read/write signatures and, except
for `.pcapng` by default, printable eight-character runs. It opens no device,
does not send traffic, refuses non-regular files and files larger than 64 MiB,
and may display potential write-key bytes from the input; keep its input and
output private.

Before selecting a device for a hardware-test command, Linux, macOS, and
Windows can
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
confirms the model.

For a complete Linux evidence run, the companion `reink-results` repository
provides `run-linux-read-evidence.sh`. With the repositories checked out as
siblings, run it from `reink-results`:

```bash
./run-linux-read-evidence.sh
```

The script selects a candidate only when exactly one descriptor candidate and
one exact bundled model hint exist; otherwise it requires
`--candidate-alias` and/or `--model`. It derives the model range, preserves
preflight, identity, selected-read, dump, and boundary-probe results in an
ignored timestamped private directory, and never performs a write or reset.
The identity result remains the authoritative model confirmation.

For native Windows, use the same repository's
`run-windows-read-evidence.ps1` with explicit vendor/product/interface/
alternate-setting/bus/device/model parameters. It first confirms that the full
explicit selector identifies exactly one descriptor candidate, keeps its raw
output in ignored private evidence, skips the standalone D4 probe during
preflight, and invokes the durable `reink-cli usb-eeprom-dump` only after every
hardware-test evidence stage succeeds. It never installs, detaches, rebinds, or
changes a driver.

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

All successful reports use schema version 3 with `mode: "read_only"` and
ordered step objects (`name`, `status`, and `result`). Preserve those reports as
hardware evidence. Read-only commands never write. Physical writes and semantic resets are supported only by the explicit, gated
write-evidence command or confirmed CLI commands; no default workflow,
read-evidence runner, or GUI action writes printer state. Read-only failure reports are also schema version 3, include
reconnect/power-cycle/reboot remediation, and record the failing stage without
raw trace bytes or invented successful EEPROM values.

Normal `d4-eeprom-read` rejects addresses outside the selected model's
`mem_low..=mem_high` range before opening USB. The separate
`d4-eeprom-boundary-probe` performs exactly one explicitly acknowledged
out-of-range read; it has no default address and rejects in-range addresses.
Its result is observed behavior only, never proof that an out-of-range read is
safe.

```powershell
cargo run -p reink-hardware-test -- d4-eeprom-boundary-probe --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model> --address 0xffff --confirm-out-of-range-read I_CONFIRM_THIS_IS_A_READ_ONLY_BOUNDARY_PROBE --report-file <outside-repository-path>
```

For an explicitly selected Linux interface, `read-sequence`, `d4-identity`,
`d4-eeprom-read`, `d4-eeprom-dump`, `d4-eeprom-boundary-probe`, and
`d4-eeprom-write-evidence`
automatically detach, claim, release, and reattach only the active driver for
each operation. Schema-version-3 reports include
`linux_driver_handoff` with automatic, detached, and reattached outcomes; they
never contain raw traffic. If detach, claim, release, or reattachment fails,
reconnect or power-cycle the printer, then reboot the host if needed before
retrying. On Windows and macOS, these commands only claim and release an
already libusb-accessible selected interface. The same report field remains
present for schema compatibility, with `detached: false` and
`reattached: null`; no driver is installed, detached, rebound, changed, or
restored.

Concrete commands return nonzero for operational failures. When `--report-file`
is supplied after a D4 operation begins, they preserve a structured failure
report before returning nonzero. In particular, a failed EEPROM dump records
only completed-address count and failed address, never a partial values list.
If its explicit trace capture succeeds, the process still fails and labels that
file as incomplete private evidence.

`d4-eeprom-write-evidence` is a separate executable physical-test command. It
requires the complete selector (including bus and device address), exact D4
model identity, one in-range address and distinct test value, two exact
confirmations, and different create-new backup and report paths before USB is
opened. It pre-reads the original byte, creates and syncs a complete backup,
writes with core read-back verification, independently reads the test value,
then always attempts to restore the original byte whenever it was read. It
independently verifies restoration and writes its private structured report
only after D4 and USB cleanup have been attempted.

```powershell
cargo run -p reink-hardware-test -- d4-eeprom-write-evidence --vendor-id 0x04b8 --product-id <product-id> --interface <number> --alternate-setting <number> --bus-number <bus> --device-address <device> --model <model> --address <in-range-address> --value <different-test-byte> --backup-file <new-private-complete-backup.bin> --report-file <new-private-write-evidence-report.json> --confirm-write I_CONFIRM_THIS_WILL_WRITE_EEPROM --confirm-restoration-evidence I_CONFIRM_THIS_WILL_RESTORE_EEPROM_AND_RETAIN_PRIVATE_EVIDENCE
```

If the test write or its read-back fails, the command still attempts
restoration when the original byte is available and reports test-write,
restoration, read-back, and cleanup outcomes separately. A restoration or
cleanup failure is a nonzero result with explicit remediation; do not retry or
issue another write until the private report has been reviewed and the original
byte has been separately verified.

`snmp-id` reads and parses an IEEE 1284 device ID through SNMP. `snmp-status`
first reads that identity and refuses the vendor `st` request unless it resolves
to a built-in Epson model. `snmp-eeprom-read` and `snmp-eeprom-dump` require an
explicit built-in `--model`, verify it exactly against the SNMP identity, and
limit every read to that model's declared range. Dumps create a new binary file
only after the complete read succeeds. These commands use SNMP GET operations
only; they do not write EEPROM, reset counters, or probe USB.

All SNMP commands read credentials only from the process environment:

```powershell
$env:REINK_SNMP_HOST = "printer.example"
$env:REINK_SNMP_VERSION = "2c"
$env:REINK_SNMP_COMMUNITY = "<set outside shell history>"
cargo run -p reink-cli -- snmp-id
cargo run -p reink-cli -- snmp-status
cargo run -p reink-cli -- snmp-eeprom-read --model <model> --address 0x000c
cargo run -p reink-cli -- snmp-eeprom-dump --model <model> --output-file <new-image.bin>
```

`REINK_SNMP_PORT` and `REINK_SNMP_TIMEOUT_SECONDS` are optional and default to
`161` and `2`. For SNMPv3, set `REINK_SNMP_USERNAME`; optionally set both
`REINK_SNMP_AUTH_PROTOCOL` and `REINK_SNMP_AUTH_PASSWORD`, and, when privacy
is needed, both `REINK_SNMP_PRIVACY_PROTOCOL` and
`REINK_SNMP_PRIVACY_PASSWORD`. Supported authentication algorithms are `md5`,
`sha1`, `sha224`, `sha256`, `sha384`, and `sha512`; privacy algorithms are
`des`, `aes128`, `aes192`, and `aes256`.

The CLI never accepts credentials as arguments and emits no credentials in JSON
or text output. Status text is terminal-escaped for text output and always
accompanied by lossless hexadecimal bytes. Its EEPROM write and restore commands
remain non-default: they require the explicit confirmations and backup workflow
documented above.

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
`reink-tui`. On Linux, macOS, and Windows it asynchronously lists
printer-class USB **descriptor-only candidates** with
`reink_usb::list_printer_candidates()`. This scan reads descriptors only: it
does not open or claim a device, detach or hand off a driver, send control, D4,
or EEPROM traffic, or enable writes. Candidates use a session-only alias and
show only VID/PID, bus/address, interface/alternate setting, and exact bundled
VID/PID model hints; hints are not identity confirmation. The GUI has no
driver-handoff control. Its explicit read-only EEPROM operation automatically
hands off only a selected Linux interface driver when necessary; on Windows
and macOS it only claims an already libusb-accessible selected interface and
never changes a driver. Default mode never opens or claims a device,
hands off a driver, or sends traffic.

Raw EEPROM files remain available above persistent `Status`, `EEPROM`, and
`Tools` tabs. Bundled fixtures are hidden unless explicitly enabled with
`--fixtures`; only that opt-in mode resolves fixture identity and runs
deterministic fixture validation. Local raw EEPROM images are inspected
read-only after an explicit model selection. The GUI's editing, reset, backup,
and restore controls are guarded
operations: they require a selected target, exact model identity, a
create-new synchronized backup, and the action-specific confirmation. On macOS
and Windows they are available only when the selected interface is already
libusb-accessible; a claim failure is a safe stop.

Its persistent shell and tab-specific sub-pane rules are documented in
[UI design](docs/UI_DESIGN.md).

The GUI also has a bottom **Debug traffic** panel: a live, bounded in-memory,
session-only, protocol-aware view of opt-in recorded TX/RX traffic. It
reassembles Epson D4 entry exchanges and IEEE 1284.4 packets, and summarizes
each logical request or response as `field=value` details. Expand a row to
inspect its bytes. Copying all traffic or an individual row always includes
both the decoded line and its hexadecimal byte line, regardless of whether the
row is expanded. Capture is disabled until explicitly enabled; selecting a
descriptor candidate alone produces no traffic, and the GUI exports captured
traffic only through an explicit copy action.

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
diagnostics and repair automatically hand off only an explicitly selected Linux
printer interface; ordinary development does not modify USB drivers.

### Windows: build, test, and read-only USB evidence

Windows supports the workspace's pure crates, CLI, terminal UI, descriptor/real
GUI (with explicit fixture opt-in), mDNS, SNMP, selected read-only evidence,
and explicitly gated write-evidence libusb USB sessions. Windows USB access claims only an already libusb-accessible
selected interface. It never installs, detaches, rebinds, changes, or restores
a driver; an access failure is a safe stop.

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

`local-devices` remains Linux-only because it enumerates Linux device files.
`usb-id`, `usb-d4-probe`, and the read-only `usb-eeprom-dump` workflow are
available on Windows for an explicitly selected libusb-accessible interface.
`usb-eeprom-write`, `usb-eeprom-restore`, and `usb-eeprom-reset` remain
unavailable on Windows.
The cross-platform `d4-eeprom-write-evidence` command is available only as its
separate gated, reversible workflow; it is never invoked by the Windows
read-evidence runner.
For the complete hardware-test evidence sequence, use the sibling
`reink-results\run-windows-read-evidence.ps1` runner with its explicit
selector and model parameters. It keeps raw paths, reports, traces, and EEPROM
values in ignored private evidence.

### macOS: guarded GUI USB operations

The optional GUI can list descriptor-only USB printer candidates on macOS
without opening or configuring a printer. After explicit selection, it supports
guarded status, dump, write, restore, and reset operations only if that
interface is already libusb-accessible. It never installs, detaches, rebinds,
or changes a driver; a claim failure is a safe stop. Install the current stable
Xcode command-line tools and Rust toolchain, then run:

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
The opt-in hardware-test runner performs the supported read-only USB
sequence, automatically handing off only an explicitly selected Linux driver
for each operation. It reports detach, claim, release, and reattach failures
clearly; reconnect or power-cycle the printer, then reboot the host if needed.

### Instructions for coding agents and automation

1. Run the build, format, Clippy, and test commands above after source changes.
2. Treat all printer access as opt-in. Never run `usb-id`, D4, EEPROM, or reset
   commands against a device unless the user explicitly selects it.
3. Never install a USB/printer driver. An explicitly selected maintenance
   operation may use an already-installed driver association and must restore
   the prior association after the operation. Report detach, claim, release, or
   reattach failure with reconnect, power-cycle, and reboot remediation rather
   than hiding it or requiring an external handoff session.
4. Never commit raw captures, serial numbers, USB paths, IP addresses, SNMP
   credentials, or other device-specific data. Use sanitized transcripts only.
5. Do not add an automatic or generic write/reset command. Preserve the
   protocol-provenance, explicit-target, confirmation, backup, read-back, and
   restoration requirements of the dedicated write-evidence workflow.

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
There is no automated CI workflow in this repository. The
`reink-hardware-test` commands and sibling evidence runners are manual,
opt-in hardware validation tools, not automated CI smoke tests; each requires
an explicitly selected device. Any future hardware smoke test must preserve
that opt-in requirement.

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

`transport` must come from an explicitly selected adapter such as the libusb
USB backend. The example is read-only; validated write plans are exposed only
to the dedicated write-evidence workflow and confirmed CLI commands, never as
an automatic application action.

## Compatibility and safety

The behavioral target is the current ReInkPy source, including Epson model
metadata and the D4/SNMP control paths. The Python repository has no unit-test
suite, so each ported behavior is documented through Rust tests and small,
sanitized fixtures.

Do not commit captured traffic containing printer serial numbers, IP addresses,
or other device-specific information. Do not add USB driver installation as
part of ordinary development setup. Selected maintenance operations may use
existing driver associations only through a lifecycle that restores the prior
association and surfaces recovery failure. EEPROM writes and reset operations
must require explicit user confirmation, use read-back verification by default,
and report any rollback failure clearly.

## License

This port is licensed under the AGPL-3.0-or-later, consistent with ReInkPy.
