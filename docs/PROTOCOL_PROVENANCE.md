# Protocol provenance and conformance plan

This document records the evidence behind protocol behavior in the Rust port.
It prevents reverse-engineered or source-compatible behavior from being
mistaken for standards conformance.

## Evidence levels

| Level | Meaning | Allowed use |
| --- | --- | --- |
| A | Public authoritative standard or specification | Implement and cite the exact edition and section |
| B | Official vendor documentation | Implement only within documented model/protocol scope |
| C | ReInkPy source behavior or reviewed, sanitized provenance evidence | Preserve as observed behavior; label it unverified |
| D | Third-party reverse-engineering reference | Use as a research lead; validate against a capture before enabling writes |

Do not copy restricted standard text into this repository. For a licensed
standard, record its edition and section number, summarize the requirement in
original words, and keep the licensed document outside the repository.

## Private hardware observations and provenance evidence

Raw traces, reports, EEPROM images, and console transcripts from an observed
hardware run are private operational evidence, not repository provenance
evidence. Keep them untracked and outside committed documentation. They may
inform a conclusion, but do not raise an evidence level by themselves.

Only a deliberately reviewed and sanitized transcript, fixture, or summary can
be committed as level-C provenance evidence. It must remove device and host
identifiers, describe its scope and limitations, and be reviewed under the
hardware capture and fixture guide. A private observed run may therefore be
reported as an observation while the relevant protocol claim remains pending
reviewed sanitized provenance evidence or an authoritative source.

## Source inventory

| Protocol surface | Current Rust location | Current evidence | Required authoritative source | Status |
| --- | --- | --- | --- | --- |
| IEEE 1284.4 packet header | `reink-d4/src/packet.rs` | A: licensed IEEE Std 1284.4-2000 review; C: `reinkpy/d4.py` | IEEE Std 1284.4-2000 | Reviewed; covered by unit tests |
| Packet length and fragmented-read reassembly | `reink-d4/src/packet.rs` | A: licensed IEEE Std 1284.4-2000 review; C: ReInkPy `retreive()` | IEEE 1284.4 framing rules | Reviewed; covered by unit tests |
| Transaction messages, revision `0x20` | `reink-d4/src/transaction.rs` | A: licensed IEEE Std 1284.4-2000 review; C: ReInkPy `protocol_0x20.cmd_by_name` | IEEE 1284.4 transaction-channel definition | Reviewed; covered by unit tests |
| Transaction messages, revision `0x10` | `reink-d4/src/transaction.rs` | C: ReInkPy says this layout is “undocumented ? taken from d4lib.c” | IEEE 1284.4 revision-1 material and/or validated capture | Source-compatible layouts covered by unit tests; authoritative review pending |
| D4 init, service lookup, channel open, credits | `reink-d4/src/link.rs` | A: licensed IEEE Std 1284.4-2000 review; C: ReInkPy `D4Link`, `TXChannel`, and `Channel` | IEEE 1284.4 state-machine rules | Reviewed; service-to-socket and socket-to-service lookups, peer transactions, open/close/Exit covered by unit tests |
| Epson D4 entry sequence, reply recognition, and `EPSON-CTRL` service | `reink-app/src/lib.rs` | C: `reinkpy/epson.py` (`EpsonD4._init_link`); reviewed sanitized L1300 transcript summarized in `reink-results` commit `6459092` | Epson documentation or broader reviewed sanitized captures | Reviewed level-C L1300 evidence validates the entry request/reply and control-service setup; other model and authoritative conformance remain pending |
| IEEE 1284 device ID | `reink-core/src/identity.rs` | C: `reinkpy/__init__.py` parser | IEEE 1284 device-ID definition | Pending review |
| Epson printer status (`st`) | `reink-core/src/controller.rs`, `reink-app/src/lib.rs`, `reink-cli/src/main.rs` | C: `reinkpy/epson.py` `do_status()` | Epson documentation or sanitized capture | Scripted core, D4-session, and SNMP-control composition tests; hardware evidence required |
| USB printer interface selection, standard device ID, and generic bounded bulk exchange | `reink-usb/src/descriptor.rs`, `adapter.rs` | C: `reinkpy/usb.py`; private observed Linux preflight run (not committed provenance evidence) | USB-IF Printer Device Class 1.1 | Linux interface selection and a no-protocol-session device-ID read succeeded on one selected printer; selected Linux operations now automatically hand off and reattach an active driver; reviewed sanitized provenance evidence and clause-level review remain pending |
| Windows stock-driver USBPRINT enumeration and direct D4 byte transport | `reink-usb/src/native.rs`, `windows_native.rs` | A: Microsoft SetupAPI, file-I/O, and overlapped-I/O API contracts; C: reviewed sanitized vendor-utility L1300 API-boundary summary | Microsoft SetupAPI and file-I/O documentation; vendor confirmation of USBPRINT data-plane support | SetupAPI discovery, opaque tokens, bounded cancellation, typed read-only sessions, and separately gated experimental mutation are implemented. Observed `CreateFileW`/`WriteFile`/`ReadFile` D4 is validated for the reviewed read workflow; Microsoft USBPRINT-specific documentation does not explicitly bless that data plane or mutation |
| SNMP printer identification and read-only Epson control composition | `reink-snmp/src/lib.rs`, `reink-cli/src/main.rs` | C: `reinkpy/snmp.py`; RFC 3805 for standard MIB context | RFC 3805 where applicable; Epson enterprise MIB for private OIDs | Identity-validated status and model-bounded EEPROM CLI surfaces use GET only; deterministic composition tests implemented; private OID evidence still required |
| Epson EEPROM factory commands and semantic counter plans | `reink-core/src/command.rs`, `controller.rs`, `epson.rs` | C: `reinkpy/epson.py`, model TOML; reviewed sanitized L1300 results in `reink-results` commit `6459092` and `EEPROM_MAP_STATIC_ANALYSIS.md` | Epson documentation or broader reviewed sanitized captures | Reviewed level-C L1300 evidence validates the USB endpoints and factory-read key/command/reply form. It does not validate resets, writes, write order, checksums, or other models |
| Offline factory-request binary analysis | `reink-cli/src/main.rs` | C: ReInkPy `epson.py` `search_bin()` | No device protocol authority required; input remains local | Deterministic bounded parser tests implemented |

## Authoritative references

- [IEEE Std 1284.4-2000 landing page](https://standards.ieee.org/standard/1284_4-2000.html)
  — authoritative publication record. A properly licensed copy is required
  for packet-field and state-machine conformance review.
- [USB-IF Printer Device Class Document 1.1](https://www.usb.org/document-library/printer-device-class-document-11)
  — required reference for the current `reink-usb` adapter.
- [RFC 3805: Printer MIB v2](https://datatracker.ietf.org/doc/html/rfc3805)
  — standard SNMP printer-management reference. It does not make Epson private
  enterprise OIDs standard.
- [Epson technical reference portal](https://support.epson.net/publist/reference_en/)
  — official public documentation entry point; scope must be checked per model.
- [Gutenprint remote-mode reference](https://gimp-print.sourceforge.io/reference-html/x952.html)
  — level-D research material only; it is not authority for EEPROM factory
  commands.

## Experimental native Windows mutation boundary

The native USBPRINT transport opens with `GENERIC_READ | GENERIC_WRITE` and
uses bounded overlapped `WriteFile`/`ReadFile`. This permits only the explicitly
named experimental application composition; it does not validate physical
mutation parity. The evidence remains observed D4 read traffic, not a verified
native EEPROM write. Exact candidate selection, exact D4 identity, complete
model-bounded backup, read-back, rollback, and a second exact experimental
acknowledgement are required. Physical validation is pending.

## Reviewed sanitized L1300 Windows finding

The current reviewed technical summary corrects the earlier ProcMon-only
interpretation. For the observed harmless vendor maintenance utility reads, the
D4 boundary was: a vendor maintenance utility opening an opaque private direct
USB device-interface with
`CreateFileW`, writing D4 with `WriteFile`, and receiving D4 with `ReadFile`.
The observed frames did not traverse `WritePrinter`, `ReadPrinter`,
`ExtEscape`, a named pipe, or an Epson helper DLL at that boundary. Surrounding
spooler/driver initialization was context, not the D4 byte API.

The captured interface path remains private and is neither portable nor a
selector. ReInk therefore enumerates present `GUID_DEVINTERFACE_USBPRINT`
interfaces through SetupAPI, correlates generic VID/PID identity from documented
devnode properties/hardware IDs, treats each path as an opaque process-local
token, and never emits it. Microsoft documents SetupAPI, `CreateFileW`, and
overlapped file I/O, but USBPRINT-specific documentation does not explicitly
guarantee this direct data plane. The evidence observed reads only. Native
EEPROM mutation is therefore implemented only as an explicitly acknowledged,
experimental path whose physical validation remains pending.

The L1300 read-only map represents `0x0026..0x0027` as a little-endian
counter. The reviewed result's observed vendor-utility denominator of 11,809 is retained
as an evidence note only; ReInk does not apply or validate display scaling.
`0x0058` is retained only as related with an unknown role.

## Initial cross-reference findings

The Rust D4 implementation currently has source parity for the following
ReInkPy structures:

| Rust behavior | ReInkPy source behavior | Evidence level |
| --- | --- | --- |
| Six-byte `peer socket`, `source socket`, big-endian total length, credit, control header | `d4.py` `>BBHBB` | C |
| Total length includes header | `payload_length = length - hLen` | C |
| Transaction channel uses socket pair `(0, 0)` | `TXChannel.cid` | C |
| Init starts at revision `0x20` and can retry `0x10` | `D4Link._send_init` | C |
| Service channel mirrors a returned socket ID as `(id, id)` | `D4Link.get_channel` | C |
| Service name can be resolved from a socket ID | `D4Link.get_channel(cid=...)` / `GetServiceName` | C |
| Credit is received in packet headers and consumed before data sends | `D4Link._on_received` and `send` | C |

The licensed IEEE 1284.4-2000 copy was privately reviewed against the packet
header, control bits, length handling, transaction-channel bounds, credit
accounting, and revision-`0x20` state transitions. The repository contains
only independently written implementation and tests. Revision `0x10` remains
source-compatible behavior pending separate authoritative evidence.

The decoder rejects trailing bytes on fixed-layout transaction messages and
rejects empty or invalid received service names. Deterministic packet
fragmentation and transaction-codec matrices protect this boundary without
requiring a device capture. This strictness is malformed-input hardening, not
additional evidence that a revision-`0x10` layout is authoritative.

## D4 review checklist

Before a hardware adapter uses `reink-d4`, complete and record review for the
following remaining items:

1. Exact transaction-message layouts for revision `0x10`.
2. Randomized backoff behavior when both peers initiate Init simultaneously.
3. Hardware behavior for the D4 entry sequence and Epson control service beyond
   the reviewed sanitized L1300 evidence.

Convert each verified item into a fixture and a unit test named after the
behavior, not the standard text. Use
`reink_platform_test::SanitizedTranscript` for ordered byte replay, and follow
the [hardware capture and fixture guide](HARDWARE_CAPTURE_GUIDE.md) before
committing any hardware-derived data. Add the source edition and section number
in the test comment only when it is available under the project's license.

## Vendor-command safety gate

The EEPROM factory **write** encoder is exposed only through confirmed CLI
commands and the dedicated `d4-eeprom-write-evidence` physical-test command.
The latter is not a generic reset command and never runs as part of a read-only
workflow. `usb-eeprom-reset` is a confirmed CLI maintenance command, not a
generic address writer: it selects waste or platen-pad semantics and emits only
explicitly declared model-TOML reset bytes. Missing `reset` values and `min`
metadata are never zero-substituted. Before executing a physical write or reset
operation, require all of
the following:

1. Vendor documentation or repeatable capture evidence for the exact model
   family and command.
2. A test proving the request byte sequence and successful reply parsing.
3. A test proving read-back verification and failure handling.
4. Application-layer explicit confirmation and device identity display.
5. No driver installation or silent backend switching. Windows libusb mutation
   may use only an already accessible explicit selector. The separately named
   native USBPRINT mutation path uses the installed stock driver, is explicitly
   experimental, and requires its additional acknowledgement before the handle
   can be opened.
6. Explicit authorization for the selected target operation; a prior
   read-only report is useful evidence but is not itself permission.
7. The exact command acknowledgement: the dedicated evidence command needs its
   two evidence acknowledgements; native mutation also needs
   `I_ACKNOWLEDGE_WINDOWS_NATIVE_MUTATION_IS_EXPERIMENTAL`; reset needs
   `I_CONFIRM_THIS_WILL_RESET_DECLARED_COUNTERS`.
8. A complete create-new backup persisted before mutation, per-byte read-back
   verification, and rollback of every attempted byte after a failure.

`reink-hardware-test d4-eeprom-write-evidence` requires an exact
vendor/product/interface/alternate-setting/bus/device selector, exact D4 model
identity, one in-range address, and an explicit test byte. It creates its
private structured report only after cleanup. If a test write fails after the
original byte is known, it still attempts restoration and records the test
write, restoration, verification, and cleanup outcomes separately. No default
command or read-evidence runner is permitted to execute a physical write or
semantic reset. A guarded GUI action is permitted only for an
explicitly selected target when it satisfies the same identity, authorization,
backup, confirmation, read-back, and rollback gates. On macOS and Windows, it
also requires an already libusb-accessible selected interface; a claim failure
is a safe stop.
