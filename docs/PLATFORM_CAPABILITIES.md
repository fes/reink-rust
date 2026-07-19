# Platform capabilities

This is the canonical implementation and validation matrix. Other documents
should link here rather than restating platform status.

| Backend | Platforms | Read identity/status | EEPROM dump | Write/restore/reset | Driver handling | Physical validation |
| --- | --- | --- | --- | --- | --- | --- |
| libusb | Linux | Yes | Yes | Yes, explicitly confirmed | Interface-scoped detach/reattach of an installed driver | L1300/L1800 reads and reversible evidence validated |
| libusb | macOS | Yes | Yes | Yes, explicitly confirmed | Device-wide capture/re-enumeration; normally requires `sudo`; restores only when detached | Implementation and progressive runners complete; printer validation pending |
| libusb | Windows | Yes when the interface is already accessible | Yes | Available through guarded libusb surfaces | Claim/release only; no Windows driver change | Hardware validation depends on an existing libusb-accessible interface |
| USBPRINT | Windows | Yes through installed stock driver | Yes | Experimental; second acknowledgement required | Uses installed driver without rebinding | Observed read transport supports the design; complete physical parity validation pending |
| SNMP | Cross-platform network | Identity and supported status reads | Selected model-bounded reads | No mutation surface | None | Deterministic coverage; private Epson OID evidence remains limited |
| Offline image | Cross-platform | Model metadata only | Open and analyze a private image | No device mutation | None | Hardware-independent |

## User surfaces

| Surface | Purpose | Mutation policy |
| --- | --- | --- |
| `reink-cli` | Inspection, durable dumps, and explicit maintenance operations | Exact selection, backup, acknowledgement, and verification |
| `reink-tui` | Read-only model and workflow browser | No mutation |
| `reink-gui` | Guarded selected-printer workflows and trace inspection | Per-operation acknowledgement; no startup or selection side effects |
| `reink-hardware-test` | Progressive physical evidence | Separate read and reversible-write commands; never automatic reset |

## Known evidence gaps

- physical macOS driver capture, restoration, read, and reversible-write runs;
- physical Windows USBPRINT mutation, restoration, and reset;
- authoritative revision-`0x10` D4 documentation;
- trustworthy EEPROM checksum/CRC algorithm and storage metadata; and
- a complete authoritative EEPROM map.

These are evidence gaps, not invitations to add permissive fallback behavior.
