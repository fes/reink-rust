# Hardware capture and fixture guide

This guide prepares evidence collection without enabling printer writes. It
applies when a device is available and supplements
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
5. A capture is evidence of observed behavior, not permission to enable a
   write/reset command.
6. The `--trace-file` output is an original/raw byte trace. Store it outside
   this repository, never commit it, and share only a separately redacted
   derivative when authorized.
7. `--report-file` is also private evidence. Store it outside this repository:
   EEPROM values and handoff outcomes can be device-specific. Never treat a
   report or trace as authorization for an EEPROM write, restore, reset, or any
   other state change.

## Read-only D4 capture sequence

Capture each complete request and response while retaining boundaries when the
tool exposes them:

1. Epson D4 entry command and reply.
2. D4 Init request/reply, including the negotiated revision.
3. `EPSON-CTRL` service lookup and open request/replies.
4. IEEE 1284 identity request/reply.
5. EEPROM read request/reply for one address in each supported address-width
   family.
6. Service close and D4 Exit request/replies.

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
it contains the exact schema-version-2 JSON printed on stdout. If an operation
has begun and fails, it contains a schema-version-2 read-only failure report
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

## USB identity preflight

On Linux or macOS, first list candidates without opening a device:

```powershell
cargo run -p reink-hardware-test -- usb-candidates
```

`usb-candidates` reads only libusb descriptors. It neither opens nor claims a
device, detaches or hands off a driver, issues a USB control request, nor sends
D4 traffic. Its `usb-1`-style aliases are session/report-only: select a later
operation using the complete displayed selector. `model_hints` are only
bundled-database vendor/product label/filter hints; they are not identity,
cannot select a device automatically, and may be empty even for Epson devices.
A later IEEE 1284 identity read is required to confirm the model. Windows has
no fallback enumeration and returns the established unsupported USB error.

Before any D4 interaction, use the Linux or macOS `usb-id` command with an
exact vendor/product/interface selection to request the standard USB Printer
Class device ID. If vendor/product IDs match more than one attached device,
also provide the matching `--bus-number` and `--device-address`; ReInk refuses
to select one arbitrarily. It must remain separate from Epson D4 traffic. If
an active Linux kernel driver owns the interface or macOS rejects the libusb
claim, stop by default.

For the read-only hardware-test commands (`read-sequence`, `d4-identity`,
`d4-eeprom-read`, and `d4-eeprom-dump`) only, an explicit
`--allow-driver-handoff` maintenance acknowledgement temporarily detaches an
active Linux driver, then releases and reattaches only that driver. D4 reports
retain compatibility `driver_handoff_enabled` and record actual
`driver_handoff.requested`, `.detached`, and `.reattached` outcomes without
recording raw traffic. Reattachment
failure may require manual driver recovery or a reboot. The flag has no
kernel-handoff effect on macOS and never enables writes or resets.

### Automated Linux run

For the standard complete evidence sequence, use the companion
`reink-results/run-linux-read-evidence.sh` runner with both repositories
checked out as siblings:

```bash
cd ../reink-results
./run-linux-read-evidence.sh --allow-driver-handoff
```

It captures the descriptor report, preflight, identity, selected reads,
model-bounded dump, and one derived out-of-range boundary probe in a
timestamped ignored `private-evidence/` directory. The runner automatically
selects only an unambiguous single candidate with a single exact bundled model
hint; provide `--candidate-alias` and/or `--model` for any ambiguity. A model
hint is still not identity confirmation: review the identity report before
using the capture as model-specific evidence. Raw output remains private and
must not be committed.

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
3. The Epson control OID request encoding for an identity read, if supported.
4. Timeout, authentication-failure, and unsupported-OID behavior as observed.

Never retain packet contents that contain SNMP authentication material.

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
3. Keep write/reset fixtures out of the suite until the vendor-command safety
   gate in `PROTOCOL_PROVENANCE.md` is met.
