use std::error::Error;
use std::fmt;

use crate::ProtocolRevision;

const MAX_TRANSACTION_PAYLOAD_LENGTH: usize = 58;
const MAX_SERVICE_NAME_LENGTH: usize = 40;

/// An IEEE 1284.4 transaction-channel message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransactionMessage {
    Init {
        revision: ProtocolRevision,
    },
    InitReply {
        result: u8,
        revision: ProtocolRevision,
    },
    OpenChannel {
        peer_socket: u8,
        source_socket: u8,
        max_packet_size: u16,
        max_service_size: u16,
        max_credit: u16,
        initial_credit: Option<u16>,
    },
    OpenChannelReply {
        result: u8,
        peer_socket: u8,
        source_socket: u8,
        max_packet_size: u16,
        max_service_size: u16,
        max_credit: u16,
        granted_credit: u16,
    },
    CloseChannel {
        peer_socket: u8,
        source_socket: u8,
    },
    CloseChannelReply {
        result: u8,
        peer_socket: u8,
        source_socket: u8,
    },
    Credit {
        peer_socket: u8,
        source_socket: u8,
        added_credit: u16,
    },
    CreditReply {
        result: u8,
        peer_socket: u8,
        source_socket: u8,
    },
    CreditRequest {
        peer_socket: u8,
        source_socket: u8,
        max_credit: u16,
    },
    CreditRequestReply {
        result: u8,
        peer_socket: u8,
        source_socket: u8,
        added_credit: u16,
    },
    Exit,
    ExitReply {
        result: u8,
    },
    GetSocketId {
        service_name: String,
    },
    GetSocketIdReply {
        result: u8,
        socket_id: u8,
        service_name: String,
    },
    GetServiceName {
        socket_id: u8,
    },
    GetServiceNameReply {
        result: u8,
        socket_id: u8,
        service_name: String,
    },
    Error {
        peer_socket: u8,
        source_socket: u8,
        error_code: u8,
    },
}

impl TransactionMessage {
    pub(crate) fn code(&self) -> u8 {
        match self {
            Self::Init { .. } => 0x00,
            Self::InitReply { .. } => 0x80,
            Self::OpenChannel { .. } => 0x01,
            Self::OpenChannelReply { .. } => 0x81,
            Self::CloseChannel { .. } => 0x02,
            Self::CloseChannelReply { .. } => 0x82,
            Self::Credit { .. } => 0x03,
            Self::CreditReply { .. } => 0x83,
            Self::CreditRequest { .. } => 0x04,
            Self::CreditRequestReply { .. } => 0x84,
            Self::Exit => 0x08,
            Self::ExitReply { .. } => 0x88,
            Self::GetSocketId { .. } => 0x09,
            Self::GetSocketIdReply { .. } => 0x89,
            Self::GetServiceName { .. } => 0x0a,
            Self::GetServiceNameReply { .. } => 0x8a,
            Self::Error { .. } => 0x7f,
        }
    }

    pub(crate) fn reply_code(&self) -> u8 {
        self.code() | 0x80
    }

    pub(crate) fn is_command(&self) -> bool {
        self.code() < 0x80
    }

    pub fn encode(&self, revision: ProtocolRevision) -> Result<Vec<u8>, TransactionParseError> {
        let mut encoded = Vec::new();
        match self {
            Self::Init { revision } => encoded.extend([0x00, revision.as_byte()]),
            Self::InitReply { result, revision } => {
                encoded.extend([0x80, *result, revision.as_byte()])
            }
            Self::OpenChannel {
                peer_socket,
                source_socket,
                max_packet_size,
                max_service_size,
                max_credit,
                initial_credit,
            } => {
                encoded.extend([0x01, *peer_socket, *source_socket]);
                push_u16(&mut encoded, *max_packet_size);
                push_u16(&mut encoded, *max_service_size);
                push_u16(&mut encoded, *max_credit);
                if revision == ProtocolRevision::V10 {
                    push_u16(&mut encoded, initial_credit.unwrap_or(0));
                }
            }
            Self::OpenChannelReply {
                result,
                peer_socket,
                source_socket,
                max_packet_size,
                max_service_size,
                max_credit,
                granted_credit,
            } => {
                encoded.extend([0x81, *result, *peer_socket, *source_socket]);
                push_u16(&mut encoded, *max_packet_size);
                push_u16(&mut encoded, *max_service_size);
                push_u16(&mut encoded, *max_credit);
                push_u16(&mut encoded, *granted_credit);
            }
            Self::CloseChannel {
                peer_socket,
                source_socket,
            } => {
                encoded.extend([0x02, *peer_socket, *source_socket]);
                if revision == ProtocolRevision::V10 {
                    encoded.push(0);
                }
            }
            Self::CloseChannelReply {
                result,
                peer_socket,
                source_socket,
            } => encoded.extend([0x82, *result, *peer_socket, *source_socket]),
            Self::Credit {
                peer_socket,
                source_socket,
                added_credit,
            } => {
                encoded.extend([0x03, *peer_socket, *source_socket]);
                push_u16(&mut encoded, *added_credit);
            }
            Self::CreditReply {
                result,
                peer_socket,
                source_socket,
            } => encoded.extend([0x83, *result, *peer_socket, *source_socket]),
            Self::CreditRequest {
                peer_socket,
                source_socket,
                max_credit,
            } => {
                encoded.extend([0x04, *peer_socket, *source_socket]);
                if revision == ProtocolRevision::V10 {
                    push_u16(&mut encoded, 0x0080);
                    push_u16(&mut encoded, 0xffff);
                } else {
                    push_u16(&mut encoded, *max_credit);
                }
            }
            Self::CreditRequestReply {
                result,
                peer_socket,
                source_socket,
                added_credit,
            } => {
                encoded.extend([0x84, *result, *peer_socket, *source_socket]);
                push_u16(&mut encoded, *added_credit);
            }
            Self::Exit => encoded.push(0x08),
            Self::ExitReply { result } => encoded.extend([0x88, *result]),
            Self::GetSocketId { service_name } => {
                encoded.push(0x09);
                encoded.extend(ascii(service_name)?);
            }
            Self::GetSocketIdReply {
                result,
                socket_id,
                service_name,
            } => {
                encoded.extend([0x89, *result, *socket_id]);
                if *result == 0 {
                    encoded.extend(ascii(service_name)?);
                }
            }
            Self::GetServiceName { socket_id } => encoded.extend([0x0a, *socket_id]),
            Self::GetServiceNameReply {
                result,
                socket_id,
                service_name,
            } => {
                encoded.extend([0x8a, *result, *socket_id]);
                if *result == 0 {
                    encoded.extend(ascii(service_name)?);
                }
            }
            Self::Error {
                peer_socket,
                source_socket,
                error_code,
            } => encoded.extend([0x7f, *peer_socket, *source_socket, *error_code]),
        }
        if encoded.len() > MAX_TRANSACTION_PAYLOAD_LENGTH {
            return Err(TransactionParseError::MessageTooLarge {
                actual: encoded.len(),
            });
        }
        Ok(encoded)
    }

    pub fn decode(bytes: &[u8], revision: ProtocolRevision) -> Result<Self, TransactionParseError> {
        let (&code, body) = bytes
            .split_first()
            .ok_or(TransactionParseError::Truncated {
                message: "transaction code",
            })?;
        match code {
            0x00 => Ok(Self::Init {
                revision: ProtocolRevision::from_byte(byte(body, 0)?)?,
            }),
            0x80 => Ok(Self::InitReply {
                result: byte(body, 0)?,
                revision: ProtocolRevision::from_byte(byte(body, 1)?)?,
            }),
            0x01 => {
                let initial_credit = if revision == ProtocolRevision::V10 {
                    Some(u16_at(body, 8)?)
                } else {
                    None
                };
                Ok(Self::OpenChannel {
                    peer_socket: byte(body, 0)?,
                    source_socket: byte(body, 1)?,
                    max_packet_size: u16_at(body, 2)?,
                    max_service_size: u16_at(body, 4)?,
                    max_credit: u16_at(body, 6)?,
                    initial_credit,
                })
            }
            0x81 => Ok(Self::OpenChannelReply {
                result: byte(body, 0)?,
                peer_socket: byte(body, 1)?,
                source_socket: byte(body, 2)?,
                max_packet_size: u16_at(body, 3)?,
                max_service_size: u16_at(body, 5)?,
                max_credit: u16_at(body, 7)?,
                granted_credit: u16_at(body, 9)?,
            }),
            0x02 => Ok(Self::CloseChannel {
                peer_socket: byte(body, 0)?,
                source_socket: byte(body, 1)?,
            }),
            0x82 => Ok(Self::CloseChannelReply {
                result: byte(body, 0)?,
                peer_socket: byte(body, 1)?,
                source_socket: byte(body, 2)?,
            }),
            0x03 => Ok(Self::Credit {
                peer_socket: byte(body, 0)?,
                source_socket: byte(body, 1)?,
                added_credit: u16_at(body, 2)?,
            }),
            0x83 => Ok(Self::CreditReply {
                result: byte(body, 0)?,
                peer_socket: byte(body, 1)?,
                source_socket: byte(body, 2)?,
            }),
            0x04 => Ok(Self::CreditRequest {
                peer_socket: byte(body, 0)?,
                source_socket: byte(body, 1)?,
                max_credit: if revision == ProtocolRevision::V10 {
                    0
                } else {
                    u16_at(body, 2)?
                },
            }),
            0x84 => Ok(Self::CreditRequestReply {
                result: byte(body, 0)?,
                peer_socket: byte(body, 1)?,
                source_socket: byte(body, 2)?,
                added_credit: u16_at(body, 3)?,
            }),
            0x08 => Ok(Self::Exit),
            0x88 => Ok(Self::ExitReply {
                result: byte(body, 0)?,
            }),
            0x09 => Ok(Self::GetSocketId {
                service_name: ascii_from(body)?,
            }),
            0x89 => Ok(Self::GetSocketIdReply {
                result: byte(body, 0)?,
                socket_id: byte(body, 1)?,
                service_name: ascii_from(&body[2..])?,
            }),
            0x0a => Ok(Self::GetServiceName {
                socket_id: byte(body, 0)?,
            }),
            0x8a => Ok(Self::GetServiceNameReply {
                result: byte(body, 0)?,
                socket_id: byte(body, 1)?,
                service_name: ascii_from(&body[2..])?,
            }),
            0x7f => Ok(Self::Error {
                peer_socket: byte(body, 0)?,
                source_socket: byte(body, 1)?,
                error_code: byte(body, 2)?,
            }),
            _ => Err(TransactionParseError::UnknownCode { code }),
        }
    }
}

fn push_u16(encoded: &mut Vec<u8>, value: u16) {
    encoded.extend(value.to_be_bytes());
}

fn byte(bytes: &[u8], index: usize) -> Result<u8, TransactionParseError> {
    bytes
        .get(index)
        .copied()
        .ok_or(TransactionParseError::Truncated {
            message: "transaction field",
        })
}

fn u16_at(bytes: &[u8], index: usize) -> Result<u16, TransactionParseError> {
    Ok(u16::from_be_bytes([
        byte(bytes, index)?,
        byte(bytes, index + 1)?,
    ]))
}

fn ascii(value: &str) -> Result<&[u8], TransactionParseError> {
    if !value.is_ascii() {
        return Err(TransactionParseError::NonAsciiServiceName);
    }
    if value.is_empty() || value.len() > MAX_SERVICE_NAME_LENGTH {
        return Err(TransactionParseError::InvalidServiceName);
    }
    if !value.bytes().enumerate().all(|(index, byte)| match byte {
        b'A'..=b'Z' | b'0'..=b'9' | b'-' => {
            !(index == 0 && !byte.is_ascii_uppercase())
                && !(index + 1 == value.len() && byte == b'-')
        }
        _ => false,
    }) {
        return Err(TransactionParseError::InvalidServiceName);
    }
    Ok(value.as_bytes())
}

fn ascii_from(value: &[u8]) -> Result<String, TransactionParseError> {
    if !value.is_ascii() {
        return Err(TransactionParseError::NonAsciiServiceName);
    }
    Ok(String::from_utf8(value.to_vec()).expect("ASCII is valid UTF-8"))
}

/// A malformed transaction-channel message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransactionParseError {
    Truncated { message: &'static str },
    UnknownCode { code: u8 },
    UnsupportedRevision { revision: u8 },
    NonAsciiServiceName,
    InvalidServiceName,
    MessageTooLarge { actual: usize },
}

impl fmt::Display for TransactionParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { message } => write!(formatter, "truncated {message}"),
            Self::UnknownCode { code } => write!(formatter, "unknown transaction code {code:#04x}"),
            Self::UnsupportedRevision { revision } => {
                write!(
                    formatter,
                    "unsupported IEEE 1284.4 revision {revision:#04x}"
                )
            }
            Self::NonAsciiServiceName => formatter.write_str("service name is not ASCII"),
            Self::InvalidServiceName => formatter.write_str("invalid service name"),
            Self::MessageTooLarge { actual } => {
                write!(formatter, "transaction payload exceeds 58 bytes: {actual}")
            }
        }
    }
}

impl Error for TransactionParseError {}

#[cfg(test)]
mod tests {
    use crate::ProtocolRevision;

    use super::TransactionMessage;

    #[test]
    fn codecs_init_and_service_lookup() {
        let init = TransactionMessage::Init {
            revision: ProtocolRevision::V20,
        };
        assert_eq!(init.encode(ProtocolRevision::V20).unwrap(), [0, 0x20]);
        assert_eq!(
            TransactionMessage::decode(&[0x89, 0, 2, b'E', b'P'], ProtocolRevision::V20).unwrap(),
            TransactionMessage::GetSocketIdReply {
                result: 0,
                socket_id: 2,
                service_name: "EP".to_owned()
            }
        );
    }

    #[test]
    fn codecs_revision_specific_credit_requests() {
        let request = TransactionMessage::CreditRequest {
            peer_socket: 2,
            source_socket: 2,
            max_credit: 4,
        };
        assert_eq!(
            request.encode(ProtocolRevision::V20).unwrap(),
            [4, 2, 2, 0, 4]
        );
        assert_eq!(
            request.encode(ProtocolRevision::V10).unwrap(),
            [4, 2, 2, 0, 0x80, 0xff, 0xff]
        );
    }

    #[test]
    fn preserves_revision_10_open_and_close_layouts() {
        let open = TransactionMessage::OpenChannel {
            peer_socket: 2,
            source_socket: 3,
            max_packet_size: 0x100,
            max_service_size: 0x200,
            max_credit: 4,
            initial_credit: Some(5),
        };
        assert_eq!(
            open.encode(ProtocolRevision::V10).unwrap(),
            [1, 2, 3, 1, 0, 2, 0, 0, 4, 0, 5]
        );
        assert_eq!(
            TransactionMessage::decode(&[2, 2, 3, 0], ProtocolRevision::V10).unwrap(),
            TransactionMessage::CloseChannel {
                peer_socket: 2,
                source_socket: 3
            }
        );
    }

    #[test]
    fn rejects_invalid_or_oversized_service_names() {
        let oversized = "A".repeat(41);
        for service_name in ["", "lowercase", "-LEADING", "TRAILING-", &oversized] {
            assert!(
                TransactionMessage::GetSocketId {
                    service_name: service_name.to_owned(),
                }
                .encode(ProtocolRevision::V20)
                .is_err()
            );
        }
    }
}
