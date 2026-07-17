# Hardware capture and fixture guide

This guide prepares read-only evidence collection and the separately explicit
reversible write-evidence workflow. It never makes a printer write automatic.
It applies when a device is available and supplements
[protocol provenance](PROTOCOL_PROVENANCE.md).

## Capture rules

1. Capture only an explicitly selected device and keep the original capture
   outside this repository.
2. Do not collect or commit SNMP community strings, SNMPv3 credentials,
   serial numbers, MAC addresses, host names, IP addresses, USB paths, or user
   names.
3. Redact identifiers consistently with placeholders before sharing a trace.
   Preserve byte lengths where framing or parsing depends on them.
4. Record the transport, operating system, selected model family, firmware
   family when known, timestamp, and whether the interaction was read-only.
   Keep this metadata private if it identifies a device or person.
5. A capture is evidence of observed behavior, not permission for a generic
   write/reset action. A physical write or semantic counter reset remains
   available only through an explicit write-evidence, confirmed CLI, or
   separately guarded GUI gate.
6. The `--trace-file` output is an original/raw byte trace. Store it outside
   this repository, never commit it, and share only a separately redacted
   derivative when authorized.
7. `--report-file` is also private evidence. Store it outside this repository:
   EEPROM values and handoff outcomes can be device-specific. Never treat a
   read-only report or trace as authorization for an EEPROM write, restore,
   reset, or any other state change.

## Read-only D4 capture sequence

Capture each complete request and response while retaining boundaries when the
tool exposes them:

1. Epson D4 entry command and reply.
2. D4 Init request/reply, including the negotiated revision.
3. `EPSON-CTRL` service lookup and open request/replies.
4. IEEE 1284 identity request/reply.
5. Read-only Epson `st` status request/reply.
6. EEPROM read request/reply for one address in each supported address-width
   family.
7. Service close and D4 Exit request/replies.

When possible, also capture intentionally fragmented reads and normal
back-to-back packets. Do not induce errors or send writes solely to create a
fixture.

The read-only hardware driver can retain these D4 boundaries only with an
explicit `--trace-file <outside-repository-path>` on `d4-identity`,
`d4-eeprom-read`, `d4-eeprom-dump`, or `d4-eeprom-boundary-probe`. It writes stable JSON with
`schema_version: 1`, `mode: "read_only"`, the command name, and ordered
`tx`/`rx` events whose `bytes` are uppercase hexadecimal. It deliberately
excludes USB paths, host data, serials, credentials, model-report values, and
raw descriptor strings. The parent directory must already exist and an
existing trace path is never overwritten. The trace is created only after D4
shutdown and USB close/driver-handoff cleanup have been attempted; it is not
included in the normal stdout report.

Each D4 command also accepts `--report-file <outside-repository-path>`. It
refuses overwrite and requires an existing parent just like traces. On success,
it contains the exact schema-version-3 JSON printed on stdout. If an operation
has begun and fails, it contains a schema-version-3 read-only failure report
with command, stage, error text, and driver-handoff outcome, but never raw
trace bytes or fabricated successful values. Dump failures additionally record
the completed-address count and failed address.

```powershell
cargo run -p reink-hardware-test -- d4-identity --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model> --trace-file <outside-repository-path>
cargo run -p reink-hardware-test -- d4-eeprom-read --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model> --address 0x000c --trace-file <outside-repository-path>
cargo run -p reink-hardware-test -- d4-eeprom-dump --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model> --trace-file <outside-repository-path>
cargo run -p reink-hardware-test -- d4-eeprom-read --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model> --address 0x000c --report-file <outside-repository-path>
```

`d4-eeprom-read` rejects addresses outside the selected model's
`memory_low..=memory_high` before opening USB. The only permitted way to make
an out-of-model-range read is the dedicated one-read boundary probe with its
exact acknowledgement. It has no default address, rejects in-range addresses,
and reports an observed reply as behavior only—not proof that out-of-range
reads are safe.

```powershell
cargo run -p reink-hardware-test -- d4-eeprom-boundary-probe --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model> --address 0xffff --confirm-out-of-range-read I_CONFIRM_THIS_IS_A_READ_ONLY_BOUNDARY_PROBE --report-file <outside-repository-path>
```

## Reversible physical write evidence

`d4-eeprom-write-evidence` is the only hardware-test command that writes a
physical EEPROM byte. It is intentionally separate from all read-only commands
and automated read-evidence runners. It never chooses a candidate, model,
interface, address, or value: the full USB location, model, one in-range
address, and a test byte must be supplied explicitly.

Before USB is opened, it requires two exact confirmations, validates the model
range, and rejects existing or aliased backup/report paths. In its D4 session it
requires an exact identity-model match, pre-reads the original byte, creates
and syncs a full create-new backup, writes the selected byte with
`EepromWritePlan`/core read-back verification, and independently reads the
test byte. It then attempts restoration whenever the original byte is known,
including after a test-write failure, and independently verifies the restored
byte. The private report is created only after D4 shutdown and USB cleanup have
both been attempted; it records every stage and distinguishes test-write,
restoration, read-back, and cleanup failures.

```powershell
cargo run -p reink-hardware-test -- d4-eeprom-write-evidence --vendor-id 0x04b8 --product-id <product-id> --interface <number> --alternate-setting <number> --bus-number <bus> --device-address <device> --model <model> --address <in-range-address> --value <different-test-byte> --backup-file <new-private-complete-backup.bin> --report-file <new-private-write-evidence-report.json> --confirm-write I_CONFIRM_THIS_WILL_WRITE_EEPROM --confirm-restoration-evidence I_CONFIRM_THIS_WILL_RESTORE_EEPROM_AND_RETAIN_PRIVATE_EVIDENCE
```

Do not run it without an explicitly authorized target operation. If any write,
restoration, read-back, shutdown, or USB cleanup stage fails, do not retry and
do not issue another write. Retain the private backup/report, reconnect or
power-cycle if needed, and verify the original byte with a separately confirmed
read before further action.

## Confirmed semantic counter reset

`reink-cli usb-eeprom-reset` is a separate, explicit maintenance operation; it
is not a hardware-evidence command and no evidence runner invokes it. It
requires an exact selected model identity, a new complete private backup, and
the exact acknowledgement
`I_CONFIRM_THIS_WILL_RESET_DECLARED_COUNTERS` before USB is opened. Its
`--target waste` and `--target platen-pad` options are semantically separate.
The command merges only matching model-TOML operations with an explicitly
declared `reset` byte array. It never converts a missing `reset` field or a
`min` metadata field into zeros.

The command uses the same complete backup, per-byte read-back verification,
rollback-on-failure, orderly D4 shutdown, and USB cleanup path as confirmed
write/restore. Unlike `d4-eeprom-write-evidence`, a successful reset is not
automatically restored and does not create a hardware-test report: retain the
private backup and command result. Do not add this command to read-evidence
runners or use it solely to produce a capture.

### Guarded GUI operations

The GUI's connected controls are not evidence runners and never start from
startup, candidate selection, file selection, or a typed acknowledgement alone.
On Linux and Windows (and macOS only where the existing libusb interface claim
is accessible), a user must select one descriptor candidate and one
bundled expected model, then press an operation button. Exact VID/PID
associations are display hints only. The worker reads the D4 identity and
refuses any identity/model mismatch before status, dump, or model-specific
access.

The GUI can save a durable complete dump to a new user-selected private file.
Generic write, restore, waste reset, and platen-pad reset each require a new
user-selected complete backup path, create and sync that backup before writing,
and require their respective exact typed confirmation. Restore also requires a
user-selected complete image of the exact model range. Every mutation uses the
same guarded read-back and rollback path as the application service; its result
reports current values, verification or rollback detail, D4 shutdown, and USB
cleanup. Keep GUI images, backups, displayed values, and opt-in debug traffic
private and out of default logs and this repository.

```powershell
cargo run -p reink-cli -- usb-eeprom-reset --vendor-id 0x04b8 --product-id <product-id> --interface <number> --model <model> --target waste --backup-file <new-private-complete-backup.bin> --confirmation I_CONFIRM_THIS_WILL_RESET_DECLARED_COUNTERS
```

## USB identity preflight

On Linux, macOS, or Windows, first list candidates without opening a device:

```powershell
cargo run -p reink-hardware-test -- usb-candidates
```

`usb-candidates` reads only libusb descriptors. It neither opens nor claims a
device, detaches or hands off a driver, issues a USB control request, nor sends
D4 traffic. Its `usb-1`-style aliases are session/report-only: select a later
operation using the complete displayed selector. `model_hints` are only
bundled-database vendor/product label/filter hints; they are not identity,
cannot select a device automatically, and may be empty even for Epson devices.
A later IEEE 1284 identity read is required to confirm the model.

Before any D4 interaction, use the Linux, macOS, or Windows `usb-id` command with an
exact vendor/product/interface selection to request the standard USB Printer
Class device ID. If vendor/product IDs match more than one attached device,
also provide the matching `--bus-number` and `--device-address`; ReInk refuses
to select one arbitrarily. It must remain separate from Epson D4 traffic. If
an active Linux kernel driver owns the interface, ReInk automatically detaches
and reattaches it for each operation. On macOS and Windows, ReInk only attempts
to claim an already libusb-accessible interface. A claim failure is a safe stop:
do not install, detach, rebind, or otherwise change a driver.

For selected Linux read-only hardware-test commands (`read-sequence`,
`d4-identity`, `d4-eeprom-read`, `d4-eeprom-dump`, and
`d4-eeprom-boundary-probe`), handoff is automatic: ReInk temporarily detaches
an active driver, then releases and reattaches only that driver for each
operation. Schema-version-3 D4 reports record
`linux_driver_handoff.automatic`, `.detached`, and `.reattached` outcomes
without raw traffic. No external manual unbind is required. On failure, the
report directs the operator to reconnect or power-cycle the printer and reboot
the host if needed before retrying. The current macOS implementation does not
detach a driver. Windows also never installs, detaches, rebinds, changes, or
restores a driver. Driver handoff alone is never authorization for a write or
reset.

`read-sequence` includes an isolated D4 entry probe by default. That probe
stops before D4 Init and may leave a printer awaiting D4 traffic, so the
automated runner passes `--skip-d4-entry-probe` and lets each later D4 command
start and close its own session.

After every evidence command succeeds, the automated Linux and Windows runners
invoke the
durable `reink-cli usb-eeprom-dump` workflow and saves a new private
`eeprom-image.bin` beside the reports. It is a model-bounded read-only image,
not a hardware-test report or write authorization. These runners never invoke
write evidence; use only the separate explicit write-evidence runner when a
target operation has been authorized.

### Automated Linux run

For the standard complete evidence sequence, use the companion
`reink-results/run-linux-read-evidence.sh` runner with both repositories
checked out as siblings:

```bash
cd ../reink-results
./run-linux-read-evidence.sh
```

It captures the descriptor report, preflight, identity, selected reads,
model-bounded dump, and one derived out-of-range boundary probe in a
timestamped ignored `private-evidence/` directory. The runner automatically
selects only an unambiguous single candidate with a single exact bundled model
hint; provide `--candidate-alias` and/or `--model` for any ambiguity. A model
hint is still not identity confirmation: review the identity report before
using the capture as model-specific evidence. Raw output remains private and
must not be committed.

### Automated Windows run

For native Windows, use the companion
`reink-results/run-windows-read-evidence.ps1` runner with explicit
vendor/product/interface/alternate-setting/bus/device/model parameters. It
records descriptor candidates first and rejects a selector that does not match
exactly one candidate. It captures the same preflight (with the D4 entry probe
skipped), identity, selected reads, model-bounded dump, boundary probe, and
conditional durable image. All output remains in an ignored private evidence
directory; the runner does not print private paths or values. Windows commands
only claim an already libusb-accessible interface and never install, detach,
rebind, or change a driver.

## Read-only validation matrix

For each accessible printer, collect two complete structured runs of device-ID,
D4 entry, D4 identity, and orderly shutdown. After identity validation, collect
small EEPROM read sets covering low, middle, known waste-counter, and
near-upper-bound addresses. Record invalid-model and invalid-address failures
only through the validation driver; do not deliberately provoke USB or printer
faults.

Only after those selected-address reads succeed, `d4-eeprom-dump` may read the
model-declared `mem_low..=mem_high` range. Retain its output privately: it may
contain device-specific data. A successful dump is read-only evidence, not
authorization to restore the image or write any EEPROM address.

If a dump read fails, the driver reports the completed count and failed address
and emits no partial-success report. An explicitly requested trace may still be
saved after cleanup as incomplete private evidence, but the command remains a
failure and the trace is not a successful dump report.

## SNMP capture sequence

For an authorized read-only session, retain sanitized evidence of:

1. Protocol version and authentication mode, without secrets.
2. The IEEE 1284 device-ID OID request and response type.
3. The Epson control OID request encoding for the read-only `st` status request,
   if supported.
4. After exact identity/model validation, the Epson control OID request and
   response type for an in-range EEPROM read, if supported.
5. Timeout, authentication-failure, and unsupported-OID behavior as observed.

Never retain packet contents that contain SNMP authentication material.
SNMP status and EEPROM inspection are evidence only: they must not be extended
to a write, reset, or out-of-range probe.

## Turning evidence into a test

Before using `trace-to-transcript`, manually redact and review the private
trace. The command does not perform sanitization and must be given the exact
operator acknowledgement:

```powershell
cargo run -p reink-hardware-test -- trace-to-transcript --trace-file <reviewed-private-trace> --output-file <new-local-template.rs> --confirmation I_CONFIRM_TRACE_IS_SANITIZED --description "sanitized fixture"
```

It validates the capture schema and preserves ordered TX/RX event boundaries in
a local `SanitizedTranscript` builder template. It refuses to overwrite an
existing output file. That template is not automatically committable source:
review every byte again, add assertions for the behavior it protects, and
review the resulting test before committing it. Do not add the original trace
or any unreviewed generated template to source control.

Create a fixture named for behavior, not a printer or standard section. Use
`reink_platform_test::SanitizedTranscript` to represent each byte exchange in
strict order. It rejects writes before required reads and supports explicitly
fragmented responses. Label synthetic fixtures as synthetic and capture-derived
fixtures with their evidence level in the test comment.

For every new fixture:

1. Confirm no sensitive or device-specific data remains.
2. Add an assertion for the observable behavior the fixture protects.
3. Keep physical write/reset fixtures out of the suite. Exercise
   write-evidence branching only with mocks unless an explicitly authorized
   target operation is being run through its dedicated command.
