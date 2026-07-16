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
| Status | Selected source identity/model information or descriptor-only candidate metadata |
| EEPROM | EEPROM inspection and field interpretation |
| Tools | Opt-in fixture validation, maintenance workflow, and future write-safety controls |

The Debug traffic pane is global. It must not be replaced, hidden, or reset
when switching tabs.

## Source modes

The GUI starts in descriptor/real mode with no selected printer or fixture. On
Linux and macOS it may enumerate descriptor-only candidates automatically; on
Windows it states that native USB enumeration is unavailable. Fixtures are
hidden unless the GUI is launched with `--fixtures`; only that explicit opt-in
permits selecting a fixture and running fixture validation. Raw EEPROM file
inspection remains available in both modes.

Default mode never opens or claims a device, hands off a driver, sends control,
D4, or EEPROM traffic, and does not render fixture EEPROM bytes. Selecting a
candidate, raw file, or enabled fixture clears every other source choice.

## Sub-panes

Primary content may have tab-specific sub-panes. For example, the EEPROM tab
has separate Field and Hex dump sub-panes. Sub-panes should preserve the same
single-line outline and spacing vocabulary, but must not alter the global
three-pane layout.

## Descriptor-only USB candidates

On Linux and macOS, startup and **Refresh USB candidates** asynchronously call
only `reink_usb::list_printer_candidates()`. The selector shows this distinct
group before fixtures. Each candidate gets a session-only alias and shows only
VID/PID, bus/address, interface/alternate setting, and exact bundled VID/PID
model hints. It must not show USB manufacturer, product, or serial strings.
Hints are not identity confirmation.

Enumeration is strictly descriptor-only: it must never open or claim a device,
detach or hand off a driver, issue a control request, send D4 or EEPROM
traffic, or enable writes. No GUI driver-handoff control is present. Selecting
a candidate clears the raw EEPROM source; selecting a fixture or raw file
clears the candidate. A candidate has no identity or EEPROM data and makes
connected operations and fixture validation unavailable until a future explicit
read-only operation exists. That future selected-printer operation must
automatically detach and reattach only the selected Linux interface driver; do
not add an operation merely to perform handoff. On Windows, native USB
descriptor enumeration is unavailable; raw EEPROM inspection remains available, while fixtures require
the explicit `--fixtures` launch mode.

## Safety and diagnostics

The GUI remains transport-free for fixtures and descriptor candidates. Debug
traffic is a live, opt-in, bounded in-memory session-only pane for future
recorded-session TX/RX events. Selecting a descriptor candidate alone
produces no traffic; only a future explicit connected read-only operation can
append records. No current GUI operation emits events or exports them.
Persistent printer writes and counter resets remain unavailable until the
separate hardware-evidence and safety-review gates are complete.
