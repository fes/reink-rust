use std::path::Path;

use serde_json::json;

use super::hex_encode;

const MAX_OFFLINE_BINARY_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum BinaryFinding {
    EepromRead {
        offset: usize,
        read_key: u16,
        address: u16,
    },
    EepromWrite {
        offset: usize,
        read_key: u16,
        address: u16,
        value: u8,
        write_key: Vec<u8>,
    },
    InvalidFactoryRequest {
        offset: usize,
        reason: &'static str,
    },
    PrintableAscii {
        offset: usize,
        value: String,
    },
}

pub(super) fn analyze_binary_output(
    input_file: &Path,
    include_ascii: bool,
    as_json: bool,
) -> Result<String, String> {
    let metadata = std::fs::metadata(input_file)
        .map_err(|error| format!("could not inspect {}: {error}", input_file.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "offline binary analysis requires a regular file: {}",
            input_file.display()
        ));
    }
    if metadata.len() > MAX_OFFLINE_BINARY_BYTES {
        return Err(format!(
            "refusing to analyze {}: {} bytes exceeds the {}-byte offline limit",
            input_file.display(),
            metadata.len(),
            MAX_OFFLINE_BINARY_BYTES
        ));
    }
    let bytes = std::fs::read(input_file)
        .map_err(|error| format!("could not read {}: {error}", input_file.display()))?;
    let is_pcapng = input_file
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pcapng"));
    let findings = analyze_binary_bytes(&bytes, include_ascii || !is_pcapng);
    Ok(render_binary_findings(input_file, &findings, as_json))
}

/// Safely recognizes the `search_bin` Epson factory-request signatures in
/// already captured local bytes. It never decodes or executes arbitrary input.
pub(super) fn analyze_binary_bytes(bytes: &[u8], include_ascii: bool) -> Vec<BinaryFinding> {
    let mut findings = Vec::new();
    let mut offset: usize = 0;
    while offset.saturating_add(9) <= bytes.len() {
        if bytes[offset..].starts_with(b"||") {
            let command = &bytes[offset + 6..offset + 9];
            let is_read = command == [b'A', !b'A', 0xa0];
            let is_write = command == [b'B', !b'B', 0x21];
            if is_read || is_write {
                let declared_length =
                    usize::from(u16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]));
                let Some(payload_length) = declared_length.checked_sub(5) else {
                    findings.push(BinaryFinding::InvalidFactoryRequest {
                        offset,
                        reason: "declared factory payload is shorter than its five-byte header",
                    });
                    offset += 9;
                    continue;
                };
                let payload_start = offset + 9;
                let Some(payload_end) = payload_start.checked_add(payload_length) else {
                    findings.push(BinaryFinding::InvalidFactoryRequest {
                        offset,
                        reason: "declared factory payload length overflows",
                    });
                    offset += 9;
                    continue;
                };
                if payload_end > bytes.len() {
                    findings.push(BinaryFinding::InvalidFactoryRequest {
                        offset,
                        reason: "factory request payload is truncated",
                    });
                } else if is_read && payload_length < 2 {
                    findings.push(BinaryFinding::InvalidFactoryRequest {
                        offset,
                        reason: "EEPROM read request has fewer than two address bytes",
                    });
                } else if is_write && payload_length < 3 {
                    findings.push(BinaryFinding::InvalidFactoryRequest {
                        offset,
                        reason: "EEPROM write request has fewer than address and value bytes",
                    });
                } else {
                    let read_key = u16::from_le_bytes([bytes[offset + 4], bytes[offset + 5]]);
                    let address =
                        u16::from_le_bytes([bytes[payload_start], bytes[payload_start + 1]]);
                    if is_read {
                        findings.push(BinaryFinding::EepromRead {
                            offset,
                            read_key,
                            address,
                        });
                    } else {
                        findings.push(BinaryFinding::EepromWrite {
                            offset,
                            read_key,
                            address,
                            value: bytes[payload_start + 2],
                            write_key: bytes[payload_start + 3..payload_end].to_vec(),
                        });
                    }
                }
                offset += 9;
                continue;
            }
        }
        offset += 1;
    }

    if include_ascii {
        let mut offset = 0;
        while offset < bytes.len() {
            if !(0x20..=0x7e).contains(&bytes[offset]) {
                offset += 1;
                continue;
            }
            let start = offset;
            while offset < bytes.len() && (0x20..=0x7e).contains(&bytes[offset]) {
                offset += 1;
            }
            if offset - start >= 8 {
                findings.push(BinaryFinding::PrintableAscii {
                    offset: start,
                    value: String::from_utf8(bytes[start..offset].to_vec())
                        .expect("ASCII byte range is valid UTF-8"),
                });
            }
        }
    }
    findings
}

fn render_binary_findings(input_file: &Path, findings: &[BinaryFinding], as_json: bool) -> String {
    if as_json {
        let findings = findings
            .iter()
            .map(|finding| match finding {
                BinaryFinding::EepromRead {
                    offset,
                    read_key,
                    address,
                } => json!({
                    "kind": "eeprom_read",
                    "offset": offset,
                    "read_key": format!("{read_key:04X}"),
                    "address": format!("{address:04X}"),
                }),
                BinaryFinding::EepromWrite {
                    offset,
                    read_key,
                    address,
                    value,
                    write_key,
                } => json!({
                    "kind": "eeprom_write",
                    "offset": offset,
                    "read_key": format!("{read_key:04X}"),
                    "address": format!("{address:04X}"),
                    "value": format!("{value:02X}"),
                    "write_key_hex": hex_encode(write_key),
                }),
                BinaryFinding::InvalidFactoryRequest { offset, reason } => json!({
                    "kind": "invalid_factory_request",
                    "offset": offset,
                    "reason": reason,
                }),
                BinaryFinding::PrintableAscii { offset, value } => json!({
                    "kind": "printable_ascii",
                    "offset": offset,
                    "value": value,
                }),
            })
            .collect::<Vec<_>>();
        return json!({
            "mode": "offline",
            "input_file": input_file,
            "findings": findings,
        })
        .to_string();
    }
    findings
        .iter()
        .map(|finding| match finding {
            BinaryFinding::EepromRead {
                offset,
                read_key,
                address,
            } => format!("offset:{offset:08X} rkey:{read_key:04x} READ addr:{address:04x}"),
            BinaryFinding::EepromWrite {
                offset,
                read_key,
                address,
                value,
                write_key,
            } => format!(
                "offset:{offset:08X} rkey:{read_key:04x} WRITE addr:{address:04x} val:{value:02x} wkey:{}",
                hex_encode(write_key)
            ),
            BinaryFinding::InvalidFactoryRequest { offset, reason } => {
                format!("offset:{offset:08X} INVALID {reason}")
            }
            BinaryFinding::PrintableAscii { offset, value } => {
                format!("offset:{offset:08X} ASCII {value}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
