# UI design

## Persistent shell

The optional `reink-gui` has three persistent top-level panes:

1. **Tab strip** provides navigation and source selection. It is always
   visible and selecting a tab does not replace the surrounding shell.
2. **Primary content** is the only top-level pane whose contents change with
   the selected tab.
3. **Debug traffic** is always visible, bottom-anchored, and independent of
   tab selection. Its top resize strip controls the split between it and the
   primary content pane.

The shell uses native top and bottom panel allocation so Debug traffic reserves
its space structurally and primary content cannot extend underneath it. Each
pane has one outlined frame and a consistent outer margin. Debug traffic's
height is adjusted with a compact centered grip in the buffer above its frame,
rather than a full-width bar; do not add competing separator lines.

The primary-content and Debug-traffic panes always render their contents inside
independent vertical scroll viewports. Reducing either pane must reveal a
scrollbar rather than clip its content.
Primary content and Debug traffic each use an internal vertical scroll area, so
overflowing content remains within its pane.

## Tab behavior

Tabs render only inside the primary content pane:

| Tab | Primary content |
| --- | --- |
| Status | Selected source identity/model information, status, and durable dump controls |
| EEPROM | EEPROM inspection, field interpretation, and guarded selected-byte write entry |
| Tools | Opt-in fixture validation plus guarded backup, restore, and semantic reset controls |

The Debug traffic pane is global. It must not be replaced, hidden, or reset
when switching tabs.

## Source modes

The GUI starts in descriptor/real mode with no selected printer or fixture. On
Linux, macOS, and Windows it may enumerate descriptor-only candidates
automatically. Fixtures are hidden unless the GUI is launched with `--fixtures`;
only that explicit opt-in permits selecting a fixture and running fixture
validation. Raw EEPROM file inspection remains available in both modes.

Default startup and candidate selection never open or claim a device, hand off
a driver, or send control, D4, or EEPROM traffic. A user must explicitly start
a connected operation after selecting a candidate and expected model. Selecting
a candidate, raw file, or enabled fixture clears every other source choice.

## Sub-panes

Primary content may have tab-specific sub-panes. For example, the EEPROM tab
has separate Field and Hex dump sub-panes. Sub-panes should preserve the same
single-line outline and spacing vocabulary, but must not alter the global
three-pane layout.

For a loaded model-bounded image, the Fields sub-pane uses only read-only field
metadata. Multi-byte fields show their decoded little-endian value and expose
encoding, confidence, and reviewed-evidence notes on hover. Sensitive fields
hide their decoded value by default, while the existing raw private hex dump
remains unchanged. Selecting a field selects and highlights its start byte, so
guarded byte editing continues to operate on one explicit byte.

## Descriptor-only USB candidates

On Linux, macOS, and Windows, startup and **Refresh USB candidates**
asynchronously call only `reink_usb::list_printer_candidates()`. The selector shows this distinct
group before fixtures. Each candidate gets a session-only alias and shows only
VID/PID, bus/address, interface/alternate setting, and exact bundled VID/PID
model hints. It must not show USB manufacturer, product, or serial strings.
Hints are not identity confirmation.

Enumeration is strictly descriptor-only: it must never open or claim a device,
detach or hand off a driver, issue a control request, send D4 or EEPROM
traffic, or enable writes. No GUI driver-handoff control is present. Selecting
a candidate clears the raw EEPROM source; selecting a fixture or raw file
clears the candidate. A candidate has no identity or EEPROM data and makes fixture validation
unavailable until an explicit connected operation is selected. The user chooses
one expected bundled model; an exact VID/PID association, when available, is
only a hint. Every operation then confirms the D4 identity exactly matches that
model. **Read printer status** and **Save complete EEPROM dump** run on a worker
thread. The dump uses a user-selected create-new file and keeps the saved image
available for local inspection.

On Linux and Windows (and macOS where the existing libusb claim is accessible),
the Tools and EEPROM panes also expose guarded generic byte write, full-image
restore, and waste/platen-pad reset dialogs. They require an explicitly
selected candidate/model, a user-selected create-new synchronized full backup,
and the action-specific exact typed acknowledgement. Restore also requires a
user-selected complete model-length image. The reset dialog derives only
explicitly declared reset bytes. Every mutation uses `EepromWritePlan` read-back
verification and rollback-on-failure. Result details include preflight/current
values and D4/USB cleanup; selecting a candidate never starts any of them.
Linux restores only a driver the selected transport detached. Windows and macOS
only claim and release an already libusb-accessible interface; they never
install, detach, rebind, or change a driver.

Reviewed level-C L1300 results markdown (`reink-results` commit `6459092`,
`wic_analysis/L1300_WIC_ANALYSIS.md`) records WIC traffic routed through
Winspool, `spoolsv`, and the Epson driver to USB D4. The exact public versus
proprietary API boundary remains unknown; do not guess a `WritePrinter`,
`ReadPrinter`, `ExtEscape`, IOCTL, or other stock-driver backend. This finding
does not add a Windows stock backend.

## Safety and diagnostics

Fixtures remain transport-free. Debug traffic is a live, opt-in, bounded
in-memory session-only pane for `RecordingTransport` TX/RX events. It
reassembles Epson D4 entry exchanges and IEEE 1284.4 packets across USB reads,
then presents one concise `field=value` request or response per collapsible
row. Collapsed rows use a horizontal chevron; expanding a row reveals the
indented hexadecimal bytes. Copying the pane or an individual row always
places both the `field=value` line and its hexadecimal `bytes=` line on the
clipboard, independent of chevron state. Empty bulk-IN observations remain
visible and do not reset packet reassembly. Selecting a descriptor candidate
alone produces no traffic. An operation records events only when the user
enabled capture before starting it; only an explicit copy action exports
captured traffic to the clipboard. Default result panes do not retain raw
identity fields, and no private image, path, or traffic data is
emitted to default logs.

Physical GUI writes and resets are available only through the explicit guarded
dialogs above. They never run by default, on candidate selection, after file
selection, or after entering a confirmation; the user must press the final
operation-specific confirmation button.
