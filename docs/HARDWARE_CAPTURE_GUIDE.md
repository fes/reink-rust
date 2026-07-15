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

## USB identity preflight

Before any D4 interaction, use the Linux `usb-id` command with an exact
vendor/product/interface selection to request the standard USB Printer Class
device ID. It must remain separate from Epson D4 traffic. If an active kernel
driver owns the interface, stop: ReInk deliberately refuses to detach the
driver, and no workaround, driver rebinding, or manual detachment belongs in
this project's workflow.

## Read-only validation matrix

For each accessible printer, collect two complete structured runs of device-ID,
D4 entry, D4 identity, and orderly shutdown. After identity validation, collect
small EEPROM read sets covering low, middle, known waste-counter, and
near-upper-bound addresses. Record invalid-model and invalid-address failures
only through the validation driver; do not deliberately provoke USB or printer
faults.

## SNMP capture sequence

For an authorized read-only session, retain sanitized evidence of:

1. Protocol version and authentication mode, without secrets.
2. The IEEE 1284 device-ID OID request and response type.
3. The Epson control OID request encoding for an identity read, if supported.
4. Timeout, authentication-failure, and unsupported-OID behavior as observed.

Never retain packet contents that contain SNMP authentication material.

## Turning evidence into a test

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
