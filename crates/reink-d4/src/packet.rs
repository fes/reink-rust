use std::collections::VecDeque;
use std::error::Error;
use std::fmt;

pub const HEADER_LENGTH: usize = 6;
const CONTROL_MASK: u8 = 0x03;

/// The fixed six-byte IEEE 1284.4 packet header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacketHeader {
    pub peer_socket: u8,
    pub source_socket: u8,
    pub length: u16,
    pub credit: u8,
    pub control: u8,
}

impl PacketHeader {
    pub fn new(
        peer_socket: u8,
        source_socket: u8,
        payload_length: usize,
        credit: u8,
        control: u8,
    ) -> Result<Self, PacketError> {
        if control & !CONTROL_MASK != 0 {
            return Err(PacketError::ReservedControlBits { control });
        }
        let length = HEADER_LENGTH
            .checked_add(payload_length)
            .ok_or(PacketError::PayloadTooLarge { payload_length })?;
        let length =
            u16::try_from(length).map_err(|_| PacketError::PayloadTooLarge { payload_length })?;
        Ok(Self {
            peer_socket,
            source_socket,
            length,
            credit,
            control,
        })
    }

    pub fn payload_length(self) -> usize {
        usize::from(self.length) - HEADER_LENGTH
    }

    pub fn channel_id(self) -> (u8, u8) {
        (self.peer_socket, self.source_socket)
    }

    pub fn encode(self) -> [u8; HEADER_LENGTH] {
        let mut encoded = [0; HEADER_LENGTH];
        encoded[0] = self.peer_socket;
        encoded[1] = self.source_socket;
        encoded[2..4].copy_from_slice(&self.length.to_be_bytes());
        encoded[4] = self.credit;
        encoded[5] = self.control;
        encoded
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PacketError> {
        if bytes.len() < HEADER_LENGTH {
            return Err(PacketError::TruncatedHeader {
                actual: bytes.len(),
            });
        }
        let length = u16::from_be_bytes([bytes[2], bytes[3]]);
        if usize::from(length) < HEADER_LENGTH {
            return Err(PacketError::InvalidLength { length });
        }
        if bytes[5] & !CONTROL_MASK != 0 {
            return Err(PacketError::ReservedControlBits { control: bytes[5] });
        }
        Ok(Self {
            peer_socket: bytes[0],
            source_socket: bytes[1],
            length,
            credit: bytes[4],
            control: bytes[5],
        })
    }
}

/// A complete IEEE 1284.4 packet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Packet {
    pub header: PacketHeader,
    pub payload: Vec<u8>,
}

impl Packet {
    pub fn new(
        peer_socket: u8,
        source_socket: u8,
        payload: impl Into<Vec<u8>>,
        credit: u8,
        control: u8,
    ) -> Result<Self, PacketError> {
        let payload = payload.into();
        let header = PacketHeader::new(peer_socket, source_socket, payload.len(), credit, control)?;
        Ok(Self { header, payload })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(HEADER_LENGTH + self.payload.len());
        encoded.extend(self.header.encode());
        encoded.extend(&self.payload);
        encoded
    }
}

/// Incrementally reassembles packets across arbitrary transport reads.
#[derive(Debug, Default)]
pub struct PacketDecoder {
    buffer: VecDeque<u8>,
}

impl PacketDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Packet>, PacketError> {
        self.buffer.extend(bytes);
        let mut packets = Vec::new();

        while self.buffer.len() >= HEADER_LENGTH {
            let header_bytes: Vec<_> = self.buffer.iter().take(HEADER_LENGTH).copied().collect();
            let header = PacketHeader::decode(&header_bytes)?;
            let packet_length = usize::from(header.length);
            if self.buffer.len() < packet_length {
                break;
            }

            self.buffer.drain(..HEADER_LENGTH);
            let payload = self.buffer.drain(..header.payload_length()).collect();
            packets.push(Packet { header, payload });
        }

        Ok(packets)
    }

    pub fn buffered_len(&self) -> usize {
        self.buffer.len()
    }
}

/// Invalid IEEE 1284.4 packet framing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PacketError {
    TruncatedHeader { actual: usize },
    InvalidLength { length: u16 },
    ReservedControlBits { control: u8 },
    PayloadTooLarge { payload_length: usize },
}

impl fmt::Display for PacketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedHeader { actual } => {
                write!(formatter, "packet header requires 6 bytes, got {actual}")
            }
            Self::InvalidLength { length } => {
                write!(
                    formatter,
                    "packet length {length} is shorter than its header"
                )
            }
            Self::ReservedControlBits { control } => {
                write!(
                    formatter,
                    "reserved packet control bits are set: {control:#04x}"
                )
            }
            Self::PayloadTooLarge { payload_length } => {
                write!(
                    formatter,
                    "packet payload is too large: {payload_length} bytes"
                )
            }
        }
    }
}

impl Error for PacketError {}

#[cfg(test)]
mod tests {
    use super::{Packet, PacketDecoder, PacketError, PacketHeader};

    #[test]
    fn encodes_and_decodes_a_packet() {
        let packet = Packet::new(2, 2, [1, 2], 1, 0).unwrap();
        assert_eq!(packet.encode(), [2, 2, 0, 8, 1, 0, 1, 2]);

        let header = PacketHeader::decode(&packet.encode()).unwrap();
        assert_eq!(header, packet.header);
        assert_eq!(header.payload_length(), 2);
    }

    #[test]
    fn reassembles_fragmented_and_back_to_back_packets() {
        let first = Packet::new(0, 0, [0x80, 0, 0x20], 1, 0).unwrap().encode();
        let second = Packet::new(2, 2, [0xaa], 1, 0).unwrap().encode();
        let mut decoder = PacketDecoder::default();

        assert!(decoder.push(&first[..4]).unwrap().is_empty());
        let mut tail = first[4..].to_vec();
        tail.extend(second);
        let packets = decoder.push(&tail).unwrap();

        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].payload, [0x80, 0, 0x20]);
        assert_eq!(packets[1].payload, [0xaa]);
        assert_eq!(decoder.buffered_len(), 0);
    }

    #[test]
    fn rejects_header_lengths_smaller_than_the_header() {
        assert_eq!(
            PacketHeader::decode(&[0, 0, 0, 5, 0, 0]).unwrap_err(),
            PacketError::InvalidLength { length: 5 }
        );
    }

    #[test]
    fn rejects_reserved_control_bits() {
        assert_eq!(
            Packet::new(2, 2, [], 0, 0x04).unwrap_err(),
            PacketError::ReservedControlBits { control: 0x04 }
        );
        assert_eq!(
            PacketHeader::decode(&[2, 2, 0, 6, 0, 0x80]).unwrap_err(),
            PacketError::ReservedControlBits { control: 0x80 }
        );
    }
}
