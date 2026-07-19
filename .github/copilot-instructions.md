# ReInk repository instructions

Read `docs/MAINTAINING.md`, then only the owning crate and linked canonical
document needed for the task.

- Preserve the dependency direction in `docs/ARCHITECTURE.md`.
- Never select an ambiguous printer or silently switch backends.
- Never install a driver. Linux handoff is interface-scoped; macOS handoff is
  device-wide; Windows libusb does not change drivers.
- Never make a persistent operation automatic. Writes, restores, and resets
  require exact model/range validation, a durable complete backup, explicit
  acknowledgement, read-back verification, rollback, and cleanup reporting.
- Windows USBPRINT mutation remains experimental and requires its additional
  acknowledgement.
- Never commit raw captures, EEPROM images, serials, native device paths,
  network addresses, credentials, or private evidence.
- Reuse `reink-app` lifecycle and plan helpers; do not duplicate safety logic in
  CLI, GUI, or evidence commands.
- Update `docs/PLATFORM_CAPABILITIES.md` for support-status changes and
  `docs/PROTOCOL_PROVENANCE.md` for evidence changes.
- Run formatting, Clippy, and workspace tests with the pinned toolchain.
