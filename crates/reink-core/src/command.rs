use std::error::Error;
use std::fmt;

use crate::{AddressWidth, EpsonSpec};

/// A decoded EEPROM read response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EepromReadReply {
    pub address: u16,
    pub value: u8,
}

/// Encodes a two-byte Epson control command and its little-endian payload size.
pub fn encode_command(command: [u8; 2], payload: &[u8]) -> Result<Vec<u8>, CommandError> {
    let payload_length =
        u16::try_from(payload.len()).map_err(|_| CommandError::PayloadTooLong {
            length: payload.len(),
        })?;
    let mut encoded = Vec::with_capacity(command.len() + 2 + payload.len());
    encoded.extend(command);
    encoded.extend(payload_length.to_le_bytes());
    encoded.extend(payload);
    Ok(encoded)
}

/// Encodes an Epson factory command, such as EEPROM read (`A`) or write (`B`).
pub fn encode_factory_command(
    read_key: u16,
    command: u8,
    payload: &[u8],
) -> Result<Vec<u8>, CommandError> {
    let check_byte = ((command >> 1) & 0x7f) | ((command << 7) & 0x80);
    let mut factory_payload = Vec::with_capacity(5 + payload.len());
    factory_payload.extend(read_key.to_le_bytes());
    factory_payload.extend([command, !command, check_byte]);
    factory_payload.extend(payload);
    encode_command(*b"||", &factory_payload)
}

pub fn encode_eeprom_read(spec: &EpsonSpec, address: u16) -> Result<Vec<u8>, CommandError> {
    let payload = encode_address(spec.read_address_width, address)?;
    encode_factory_command(spec.read_key, b'A', &payload)
}

pub fn encode_eeprom_write(
    spec: &EpsonSpec,
    address: u16,
    value: u8,
) -> Result<Vec<u8>, CommandError> {
    let write_key = spec
        .write_key
        .as_deref()
        .ok_or(CommandError::MissingWriteKey)?;
    let mut payload = encode_address(spec.write_address_width, address)?;
    payload.push(value);
    payload.extend(write_key);
    encode_factory_command(spec.read_key, b'B', &payload)
}

/// Parses the ASCII `EE:XXXXXX;` record in an Epson EEPROM read response.
///
/// Python's existing one-byte-address parser consumes the first two bytes of a
/// six-hex-digit response and ignores the final byte. The initial Rust port
/// preserves that behavior until captured one-byte-device traffic can resolve
/// whether the third byte is protocol padding.
pub fn parse_eeprom_read_reply(
    response: &[u8],
    address_width: AddressWidth,
) -> Result<EepromReadReply, CommandError> {
    let response = std::str::from_utf8(response).map_err(|_| CommandError::InvalidReadReply {
        reason: "response is not UTF-8 ASCII".to_owned(),
    })?;
    let marker = response
        .find("EE:")
        .ok_or_else(|| CommandError::InvalidReadReply {
            reason: "missing EE: marker".to_owned(),
        })?;
    let hex_start = marker + 3;
    let hex_end = hex_start + 6;
    let hex = response
        .get(hex_start..hex_end)
        .ok_or_else(|| CommandError::InvalidReadReply {
            reason: "EEPROM response has fewer than six hex digits".to_owned(),
        })?;
    if response
        .as_bytes()
        .get(hex_end)
        .is_none_or(|character| *character != b';')
    {
        return Err(CommandError::InvalidReadReply {
            reason: "EEPROM response is missing terminating semicolon".to_owned(),
        });
    }

    let bytes = decode_hex(hex)?;
    let (address, value) = match address_width {
        AddressWidth::One => (u16::from(bytes[0]), bytes[1]),
        AddressWidth::Two => (u16::from_be_bytes([bytes[0], bytes[1]]), bytes[2]),
    };
    Ok(EepromReadReply { address, value })
}

fn encode_address(width: AddressWidth, address: u16) -> Result<Vec<u8>, CommandError> {
    match width {
        AddressWidth::One if address > u16::from(u8::MAX) => {
            Err(CommandError::AddressOutOfRange { width, address })
        }
        AddressWidth::One => Ok(vec![address as u8]),
        AddressWidth::Two => Ok(address.to_le_bytes().to_vec()),
    }
}

fn decode_hex(hex: &str) -> Result<[u8; 3], CommandError> {
    let mut bytes = [0; 3];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let start = index * 2;
        *byte = u8::from_str_radix(&hex[start..start + 2], 16).map_err(|_| {
            CommandError::InvalidReadReply {
                reason: format!("invalid EEPROM hex: {hex:?}"),
            }
        })?;
    }
    Ok(bytes)
}

/// An invalid command input or malformed printer reply.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandError {
    PayloadTooLong { length: usize },
    AddressOutOfRange { width: AddressWidth, address: u16 },
    MissingWriteKey,
    InvalidReadReply { reason: String },
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLong { length } => {
                write!(
                    formatter,
                    "Epson payload exceeds u16 length: {length} bytes"
                )
            }
            Self::AddressOutOfRange { width, address } => {
                write!(
                    formatter,
                    "address {address:#06x} does not fit {width:?} width"
                )
            }
            Self::MissingWriteKey => formatter.write_str("model has no EEPROM write key"),
            Self::InvalidReadReply { reason } => {
                write!(formatter, "invalid EEPROM read reply: {reason}")
            }
        }
    }
}

impl Error for CommandError {}

#[cfg(test)]
mod tests {
    use crate::{AddressWidth, ModelDatabase};

    use super::{
        CommandError, EepromReadReply, encode_command, encode_eeprom_read, encode_eeprom_write,
        encode_factory_command, parse_eeprom_read_reply,
    };

    #[test]
    fn encodes_regular_commands() {
        assert_eq!(encode_command(*b"st", &[1]).unwrap(), b"st\x01\x00\x01");
    }

    #[test]
    fn encodes_factory_commands() {
        assert_eq!(
            encode_factory_command(0x1234, b'A', &[0x56]).unwrap(),
            b"||\x06\x004\x12A\xbe\xa0\x56"
        );
    }

    #[test]
    fn encodes_eeprom_requests_using_specified_endianness() {
        let database = ModelDatabase::builtin().unwrap();
        let spec = database.get("C90").unwrap();

        assert_eq!(
            encode_eeprom_read(spec, 0x0c).unwrap(),
            b"||\x06\x00\x06\x00A\xbe\xa0\x0c"
        );
        let write = encode_eeprom_write(spec, 0x0c, 0x42).unwrap();
        assert_eq!(&write[..9], b"||\x10\x00\x06\x00B\xbd!");
        assert_eq!(&write[9..11], b"\x0c\x00");
        assert_eq!(write[11], 0x42);
    }

    #[test]
    fn reviewed_sanitized_l1300_factory_read_request_and_reply_regression() {
        // Level-C reviewed evidence: reink-results commit 6459092,
        // wic_analysis2/L1300_WIC_D4_TRANSCRIPT.md. This uses a sanitized
        // fixture value, not a private EEPROM value.
        let database = ModelDatabase::builtin().unwrap();
        let spec = database.get("L1300").unwrap();

        assert_eq!(
            encode_eeprom_read(spec, 0x26).unwrap(),
            b"||\x07\x002\x08A\xbe\xa0\x26\x00"
        );
        assert_eq!(
            parse_eeprom_read_reply(b"@BDC PS EE:002600;", AddressWidth::Two).unwrap(),
            EepromReadReply {
                address: 0x26,
                value: 0,
            }
        );
    }

    #[test]
    fn rejects_one_byte_addresses_outside_range() {
        let database = ModelDatabase::builtin().unwrap();
        let spec = database.get("C90").unwrap();

        assert!(matches!(
            encode_eeprom_read(spec, 0x100),
            Err(CommandError::AddressOutOfRange {
                width: AddressWidth::One,
                ..
            })
        ));
    }

    #[test]
    fn parses_legacy_read_reply_for_both_address_widths() {
        assert_eq!(
            parse_eeprom_read_reply(b"@BDC PS EE:ED0100;", AddressWidth::One).unwrap(),
            EepromReadReply {
                address: 0xed,
                value: 0x01
            }
        );
        assert_eq!(
            parse_eeprom_read_reply(b"@BDC PS EE:ED0100;", AddressWidth::Two).unwrap(),
            EepromReadReply {
                address: 0xed01,
                value: 0
            }
        );
    }

    #[test]
    fn rejects_malformed_read_replies() {
        assert!(matches!(
            parse_eeprom_read_reply(b"EE:XYZ;", AddressWidth::Two),
            Err(CommandError::InvalidReadReply { .. })
        ));
    }
}
