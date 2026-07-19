# Security policy

## Reporting

Report vulnerabilities through GitHub's private vulnerability reporting or a
private security advisory when available. If neither is available, open a
minimal issue requesting a private contact channel without including exploit
details or sensitive device evidence.

Never publish printer serials, native device paths, EEPROM images, USB/network
captures, IP addresses, credentials, or host-specific identifiers.

## Scope

Security-relevant issues include:

- unintended or insufficiently confirmed persistent printer mutation;
- selecting the wrong or an ambiguous device;
- incomplete backup, verification, rollback, or driver restoration;
- identifier or credential disclosure;
- unsafe native handle, buffer, or cancellation behavior; and
- protocol input that causes memory-safety or unbounded-resource failures.

ReInk is experimental maintenance software. Physical validation status is
tracked in [`docs/PLATFORM_CAPABILITIES.md`](docs/PLATFORM_CAPABILITIES.md).
