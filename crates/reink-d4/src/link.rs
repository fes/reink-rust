use std::collections::{BTreeMap, VecDeque};
use std::error::Error;
use std::fmt;

use reink_platform::{ByteTransport, ControlChannel, ControlError, TransportError};

use crate::{
    Packet, PacketDecoder, PacketError, TransactionMessage, TransactionParseError,
    packet::HEADER_LENGTH,
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ChannelId {
    pub peer_socket: u8,
    pub source_socket: u8,
}

impl ChannelId {
    pub const TRANSACTION: Self = Self {
        peer_socket: 0,
        source_socket: 0,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolRevision {
    V10,
    V20,
}

impl ProtocolRevision {
    pub fn as_byte(self) -> u8 {
        match self {
            Self::V10 => 0x10,
            Self::V20 => 0x20,
        }
    }

    pub fn from_byte(revision: u8) -> Result<Self, TransactionParseError> {
        match revision {
            0x10 => Ok(Self::V10),
            0x20 => Ok(Self::V20),
            _ => Err(TransactionParseError::UnsupportedRevision { revision }),
        }
    }
}

#[derive(Debug)]
struct ChannelState {
    name: String,
    credit: u16,
    max_packet_size: u16,
    max_service_size: u16,
    max_credit: u16,
    open: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConversationState {
    Inactive,
    Active,
    Exiting,
    Terminated,
}

/// A blocking IEEE 1284.4 link over any platform byte transport.
#[derive(Debug)]
pub struct D4Link<T> {
    target: T,
    revision: ProtocolRevision,
    decoder: PacketDecoder,
    received: VecDeque<Packet>,
    channels: BTreeMap<ChannelId, ChannelState>,
    conversation: ConversationState,
    exit_requested: bool,
}

impl<T: ByteTransport> D4Link<T> {
    pub fn new(target: T) -> Self {
        let mut channels = BTreeMap::new();
        channels.insert(
            ChannelId::TRANSACTION,
            ChannelState {
                name: "(transaction channel)".to_owned(),
                credit: 0,
                max_packet_size: 64,
                max_service_size: 64,
                max_credit: 1,
                open: true,
            },
        );
        Self {
            target,
            revision: ProtocolRevision::V20,
            decoder: PacketDecoder::default(),
            received: VecDeque::new(),
            channels,
            conversation: ConversationState::Inactive,
            exit_requested: false,
        }
    }

    pub fn revision(&self) -> ProtocolRevision {
        self.revision
    }

    pub fn initialize(&mut self) -> Result<(), D4Error> {
        if self.conversation == ConversationState::Active {
            return Err(D4Error::ConversationAlreadyActive);
        }
        if self.conversation == ConversationState::Exiting {
            return Err(D4Error::ConversationExiting);
        }
        for revision in [ProtocolRevision::V20, ProtocolRevision::V10] {
            let reply = self.transaction(TransactionMessage::Init { revision }, false)?;
            let TransactionMessage::InitReply {
                result,
                revision: negotiated,
            } = reply
            else {
                return Err(D4Error::UnexpectedTransactionReply);
            };
            if result == 0 {
                self.revision = negotiated;
                self.transaction_channel_mut().credit = 1;
                self.conversation = ConversationState::Active;
                return Ok(());
            }
            if result != 2 {
                return Err(D4Error::DeviceRejected { result });
            }
        }
        Err(D4Error::NegotiationFailed)
    }

    pub fn open_service(&mut self, service_name: &str) -> Result<ChannelId, D4Error> {
        let reply = self.transaction(
            TransactionMessage::GetSocketId {
                service_name: service_name.to_owned(),
            },
            true,
        )?;
        let TransactionMessage::GetSocketIdReply {
            result,
            socket_id,
            service_name: returned_name,
        } = reply
        else {
            return Err(D4Error::UnexpectedTransactionReply);
        };
        if result != 0 || returned_name != service_name {
            return Err(D4Error::DeviceRejected { result });
        }
        let channel = ChannelId {
            peer_socket: socket_id,
            source_socket: socket_id,
        };
        self.channels.insert(
            channel,
            ChannelState {
                name: service_name.to_owned(),
                credit: 0,
                max_packet_size: 0x100,
                max_service_size: 0x100,
                max_credit: 0,
                open: false,
            },
        );
        let reply = self.transaction(
            TransactionMessage::OpenChannel {
                peer_socket: channel.peer_socket,
                source_socket: channel.source_socket,
                max_packet_size: 0x100,
                max_service_size: 0x100,
                max_credit: 0,
                initial_credit: Some(0),
            },
            true,
        )?;
        let TransactionMessage::OpenChannelReply {
            result,
            max_packet_size,
            max_service_size,
            max_credit,
            granted_credit,
            ..
        } = reply
        else {
            return Err(D4Error::UnexpectedTransactionReply);
        };
        if result != 0 {
            return Err(D4Error::DeviceRejected { result });
        }
        let state = self
            .channels
            .get_mut(&channel)
            .expect("pending channel is inserted before OpenChannel");
        state.credit = granted_credit;
        state.max_packet_size = max_packet_size;
        state.max_service_size = max_service_size;
        state.max_credit = max_credit;
        state.open = true;
        Ok(channel)
    }

    pub fn control_channel(
        &mut self,
        channel: ChannelId,
    ) -> Result<D4ControlChannel<'_, T>, D4Error> {
        if !self.channels.get(&channel).is_some_and(|state| state.open) {
            return Err(D4Error::UnknownChannel { channel });
        }
        Ok(D4ControlChannel {
            link: self,
            channel,
        })
    }

    pub fn target(self) -> T {
        self.target
    }

    /// Closes an open service channel. The transaction channel cannot be closed.
    pub fn close_channel(&mut self, channel: ChannelId) -> Result<(), D4Error> {
        if channel == ChannelId::TRANSACTION {
            return Err(D4Error::TransactionChannelOperation);
        }
        if !self.channels.get(&channel).is_some_and(|state| state.open) {
            return Err(D4Error::UnknownChannel { channel });
        }
        let reply = self.transaction(
            TransactionMessage::CloseChannel {
                peer_socket: channel.peer_socket,
                source_socket: channel.source_socket,
            },
            true,
        )?;
        let TransactionMessage::CloseChannelReply {
            result,
            peer_socket,
            source_socket,
        } = reply
        else {
            return Err(D4Error::UnexpectedTransactionReply);
        };
        if result != 0
            || (ChannelId {
                peer_socket,
                source_socket,
            }) != channel
        {
            return Err(D4Error::DeviceRejected { result });
        }
        self.channels.remove(&channel);
        Ok(())
    }

    /// Terminates the active IEEE 1284.4 conversation.
    pub fn exit(&mut self) -> Result<(), D4Error> {
        self.require_active_conversation()?;
        self.exit_requested = true;
        let reply = self.transaction(TransactionMessage::Exit, true)?;
        if !matches!(reply, TransactionMessage::ExitReply { result: 0 }) {
            return Err(D4Error::UnexpectedTransactionReply);
        }
        self.terminate_conversation();
        Ok(())
    }

    fn transaction(
        &mut self,
        message: TransactionMessage,
        consume_credit: bool,
    ) -> Result<TransactionMessage, D4Error> {
        let expected_reply = message.reply_code();
        self.send_transaction(&message, consume_credit)?;
        loop {
            let packet = self.next_packet()?;
            if packet.header.channel_id() != (0, 0) {
                if self.conversation == ConversationState::Terminated {
                    continue;
                }
                self.apply_credit(&packet)?;
                self.received.push_back(packet);
                continue;
            }
            self.apply_credit(&packet)?;
            let received = TransactionMessage::decode(&packet.payload, self.revision)?;
            if received.is_command() {
                self.handle_peer_transaction(received)?;
                continue;
            }
            if received.code() == expected_reply {
                return Ok(received);
            }
            return Err(D4Error::UnexpectedTransactionReply);
        }
    }

    fn request(&mut self, channel: ChannelId, request: &[u8]) -> Result<Vec<u8>, D4Error> {
        self.require_active_conversation()?;
        self.send(channel, request.to_vec(), true)?;
        loop {
            let packet = self.next_packet()?;
            if packet.header.channel_id() == (0, 0) {
                self.apply_credit(&packet)?;
                let message = TransactionMessage::decode(&packet.payload, self.revision)?;
                if message.is_command() {
                    self.handle_peer_transaction(message)?;
                    if self.conversation == ConversationState::Terminated {
                        return Err(D4Error::ConversationTerminated);
                    }
                    continue;
                }
                return Err(D4Error::UnexpectedTransactionReply);
            }
            if self.conversation == ConversationState::Terminated {
                continue;
            }
            self.apply_credit(&packet)?;
            let packet_channel = ChannelId {
                peer_socket: packet.header.peer_socket,
                source_socket: packet.header.source_socket,
            };
            if packet_channel == channel {
                return Ok(packet.payload);
            }
            self.received.push_back(packet);
        }
    }

    fn send(
        &mut self,
        channel: ChannelId,
        payload: Vec<u8>,
        consume_credit: bool,
    ) -> Result<(), D4Error> {
        if self.conversation != ConversationState::Inactive {
            self.require_active_conversation()?;
        }
        let state = self
            .channels
            .get(&channel)
            .ok_or(D4Error::UnknownChannel { channel })?;
        if channel != ChannelId::TRANSACTION && !state.open {
            return Err(D4Error::UnknownChannel { channel });
        }
        if consume_credit && state.credit == 0 {
            return Err(D4Error::InsufficientCredit {
                channel,
                name: state.name.clone(),
            });
        }
        if HEADER_LENGTH + payload.len() > usize::from(state.max_packet_size) {
            return Err(D4Error::PacketExceedsNegotiatedSize {
                channel,
                actual: HEADER_LENGTH + payload.len(),
                maximum: state.max_packet_size,
            });
        }
        let packet = Packet::new(channel.peer_socket, channel.source_socket, payload, 1, 0)?;
        self.target.write_all(&packet.encode())?;
        if consume_credit {
            self.channels
                .get_mut(&channel)
                .expect("channel existence was checked before writing")
                .credit -= 1;
        }
        Ok(())
    }

    fn next_packet(&mut self) -> Result<Packet, D4Error> {
        if let Some(packet) = self.received.pop_front() {
            return Ok(packet);
        }
        let mut buffer = [0; 4096];
        loop {
            let read = self.target.read(&mut buffer)?;
            if read == 0 {
                return Err(D4Error::UnexpectedEof);
            }
            self.received.extend(self.decoder.push(&buffer[..read])?);
            if let Some(packet) = self.received.pop_front() {
                return Ok(packet);
            }
        }
    }

    fn apply_credit(&mut self, packet: &Packet) -> Result<(), D4Error> {
        let channel = ChannelId {
            peer_socket: packet.header.peer_socket,
            source_socket: packet.header.source_socket,
        };
        let state = self
            .channels
            .get(&channel)
            .ok_or(D4Error::UnknownChannel { channel })?;
        if packet.header.length > state.max_service_size {
            return Err(D4Error::PacketExceedsNegotiatedSize {
                channel,
                actual: usize::from(packet.header.length),
                maximum: state.max_service_size,
            });
        }
        if state
            .credit
            .checked_add(u16::from(packet.header.credit))
            .is_none()
        {
            self.send_error(channel, 0x86)?;
            return Err(D4Error::CreditOverflow { channel });
        }
        self.channels
            .get_mut(&channel)
            .expect("channel existence was checked before applying credit")
            .credit += u16::from(packet.header.credit);
        Ok(())
    }

    fn send_error(&mut self, channel: ChannelId, error_code: u8) -> Result<(), D4Error> {
        let payload = TransactionMessage::Error {
            peer_socket: channel.peer_socket,
            source_socket: channel.source_socket,
            error_code,
        }
        .encode(self.revision)?;
        let packet = Packet::new(0, 0, payload, 0, 0)?;
        self.target.write_all(&packet.encode())?;
        Ok(())
    }

    fn send_transaction(
        &mut self,
        message: &TransactionMessage,
        consume_credit: bool,
    ) -> Result<(), D4Error> {
        let payload = message.encode(self.revision)?;
        if payload.len() > 58 {
            return Err(D4Error::TransactionPayloadTooLarge {
                actual: payload.len(),
            });
        }
        self.send(ChannelId::TRANSACTION, payload, consume_credit)
    }

    fn handle_peer_transaction(&mut self, message: TransactionMessage) -> Result<(), D4Error> {
        match message {
            TransactionMessage::Init { revision } => {
                if self.conversation == ConversationState::Inactive {
                    self.revision = revision;
                    self.transaction_channel_mut().credit = 1;
                    self.conversation = ConversationState::Active;
                    self.send_transaction(
                        &TransactionMessage::InitReply {
                            result: 0,
                            revision,
                        },
                        false,
                    )
                } else {
                    self.send_transaction(
                        &TransactionMessage::InitReply {
                            result: 0x0b,
                            revision: self.revision,
                        },
                        false,
                    )
                }
            }
            TransactionMessage::OpenChannel {
                peer_socket,
                source_socket,
                max_packet_size,
                max_service_size,
                ..
            } => self.handle_peer_open(
                ChannelId {
                    peer_socket,
                    source_socket,
                },
                max_packet_size,
                max_service_size,
            ),
            TransactionMessage::CloseChannel {
                peer_socket,
                source_socket,
            } => {
                let channel = ChannelId {
                    peer_socket,
                    source_socket,
                };
                let result = if channel == ChannelId::TRANSACTION {
                    0x03
                } else if self.channels.contains_key(&channel) {
                    0
                } else {
                    0x08
                };
                self.send_transaction(
                    &TransactionMessage::CloseChannelReply {
                        result,
                        peer_socket,
                        source_socket,
                    },
                    true,
                )?;
                if result == 0 {
                    self.channels
                        .get_mut(&channel)
                        .expect("channel existence was checked before closing")
                        .open = false;
                }
                Ok(())
            }
            TransactionMessage::Credit {
                peer_socket,
                source_socket,
                added_credit,
            } => {
                let channel = ChannelId {
                    peer_socket,
                    source_socket,
                };
                let credit = self
                    .channels
                    .get(&channel)
                    .ok_or(D4Error::UnknownChannel { channel })?
                    .credit
                    .checked_add(added_credit)
                    .ok_or(D4Error::CreditOverflow { channel })?;
                self.channels
                    .get_mut(&channel)
                    .expect("channel existence was checked before granting credit")
                    .credit = credit;
                self.send_transaction(
                    &TransactionMessage::CreditReply {
                        result: 0,
                        peer_socket,
                        source_socket,
                    },
                    true,
                )
            }
            TransactionMessage::CreditRequest {
                peer_socket,
                source_socket,
                ..
            } => self.send_transaction(
                &TransactionMessage::CreditRequestReply {
                    result: 0,
                    peer_socket,
                    source_socket,
                    added_credit: 0,
                },
                true,
            ),
            TransactionMessage::Exit => {
                self.send_transaction(&TransactionMessage::ExitReply { result: 0 }, true)?;
                if self.exit_requested {
                    self.conversation = ConversationState::Exiting;
                } else {
                    self.terminate_conversation();
                }
                Ok(())
            }
            TransactionMessage::GetSocketId { service_name } => self.send_transaction(
                &TransactionMessage::GetSocketIdReply {
                    result: 0x0a,
                    socket_id: 0,
                    service_name,
                },
                true,
            ),
            TransactionMessage::GetServiceName { socket_id } => self.send_transaction(
                &TransactionMessage::GetServiceNameReply {
                    result: 0x0a,
                    socket_id,
                    service_name: String::new(),
                },
                true,
            ),
            TransactionMessage::Error { .. } => Ok(()),
            _ => Err(D4Error::UnexpectedTransactionReply),
        }
    }

    fn handle_peer_open(
        &mut self,
        channel: ChannelId,
        peer_max_packet_size: u16,
        peer_max_service_size: u16,
    ) -> Result<(), D4Error> {
        let Some(state) = self.channels.get(&channel) else {
            return self.send_transaction(
                &TransactionMessage::OpenChannelReply {
                    result: 0x0d,
                    peer_socket: channel.peer_socket,
                    source_socket: channel.source_socket,
                    max_packet_size: 0,
                    max_service_size: 0,
                    max_credit: 0,
                    granted_credit: 0,
                },
                true,
            );
        };
        let reply = TransactionMessage::OpenChannelReply {
            result: 0,
            peer_socket: channel.peer_socket,
            source_socket: channel.source_socket,
            max_packet_size: state.max_packet_size.min(peer_max_packet_size),
            max_service_size: state.max_service_size.min(peer_max_service_size),
            max_credit: state.max_credit,
            granted_credit: 0,
        };
        self.send_transaction(&reply, true)
    }

    fn require_active_conversation(&self) -> Result<(), D4Error> {
        match self.conversation {
            ConversationState::Active => Ok(()),
            ConversationState::Inactive => Err(D4Error::ConversationInactive),
            ConversationState::Exiting => Err(D4Error::ConversationExiting),
            ConversationState::Terminated => Err(D4Error::ConversationTerminated),
        }
    }

    fn terminate_conversation(&mut self) {
        self.channels
            .retain(|channel, _| *channel == ChannelId::TRANSACTION);
        self.transaction_channel_mut().credit = 0;
        self.conversation = ConversationState::Terminated;
    }

    fn transaction_channel_mut(&mut self) -> &mut ChannelState {
        self.channels
            .get_mut(&ChannelId::TRANSACTION)
            .expect("transaction channel is created with D4Link")
    }
}

pub struct D4ControlChannel<'a, T> {
    link: &'a mut D4Link<T>,
    channel: ChannelId,
}

impl<T: ByteTransport> ControlChannel for D4ControlChannel<'_, T> {
    fn request(&mut self, request: &[u8]) -> Result<Vec<u8>, ControlError> {
        self.link
            .request(self.channel, request)
            .map_err(|error| ControlError::Protocol {
                message: error.to_string(),
            })
    }
}

#[derive(Debug)]
pub enum D4Error {
    Transport(TransportError),
    Packet(PacketError),
    Transaction(TransactionParseError),
    UnknownChannel {
        channel: ChannelId,
    },
    InsufficientCredit {
        channel: ChannelId,
        name: String,
    },
    UnexpectedEof,
    UnexpectedTransactionReply,
    ConversationInactive,
    ConversationAlreadyActive,
    ConversationExiting,
    ConversationTerminated,
    TransactionChannelOperation,
    DeviceRejected {
        result: u8,
    },
    NegotiationFailed,
    CreditOverflow {
        channel: ChannelId,
    },
    TransactionPayloadTooLarge {
        actual: usize,
    },
    PacketExceedsNegotiatedSize {
        channel: ChannelId,
        actual: usize,
        maximum: u16,
    },
}

impl fmt::Display for D4Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(f, "transport error: {error}"),
            Self::Packet(error) => write!(f, "packet error: {error}"),
            Self::Transaction(error) => write!(f, "transaction error: {error}"),
            Self::UnknownChannel { channel } => write!(f, "unknown channel {channel:?}"),
            Self::InsufficientCredit { name, .. } => write!(f, "insufficient credit for {name}"),
            Self::UnexpectedEof => f.write_str("unexpected end of transport stream"),
            Self::UnexpectedTransactionReply => f.write_str("unexpected transaction reply"),
            Self::ConversationInactive => f.write_str("IEEE 1284.4 conversation is not active"),
            Self::ConversationAlreadyActive => {
                f.write_str("IEEE 1284.4 conversation is already active")
            }
            Self::ConversationExiting => f.write_str("IEEE 1284.4 conversation is exiting"),
            Self::ConversationTerminated => f.write_str("IEEE 1284.4 conversation is terminated"),
            Self::TransactionChannelOperation => {
                f.write_str("the IEEE 1284.4 transaction channel cannot be closed")
            }
            Self::DeviceRejected { result } => {
                write!(f, "device rejected transaction: {result:#04x}")
            }
            Self::NegotiationFailed => f.write_str("IEEE 1284.4 revision negotiation failed"),
            Self::CreditOverflow { channel } => {
                write!(f, "credit overflow for channel {channel:?}")
            }
            Self::TransactionPayloadTooLarge { actual } => {
                write!(f, "transaction payload exceeds 58 bytes: {actual}")
            }
            Self::PacketExceedsNegotiatedSize {
                channel,
                actual,
                maximum,
            } => write!(
                f,
                "packet on channel {channel:?} is {actual} bytes, exceeding negotiated maximum {maximum}"
            ),
        }
    }
}

impl Error for D4Error {}
impl From<TransportError> for D4Error {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}
impl From<PacketError> for D4Error {
    fn from(error: PacketError) -> Self {
        Self::Packet(error)
    }
}
impl From<TransactionParseError> for D4Error {
    fn from(error: TransactionParseError) -> Self {
        Self::Transaction(error)
    }
}

#[cfg(test)]
mod tests {
    use reink_platform::ControlChannel;
    use reink_platform_test::ScriptedTransport;

    use crate::{Packet, ProtocolRevision, TransactionMessage};

    use super::{ChannelId, ChannelState, ConversationState, D4Link};

    fn transaction_packet(message: TransactionMessage, credit: u8) -> Vec<u8> {
        Packet::new(
            0,
            0,
            message.encode(ProtocolRevision::V20).unwrap(),
            credit,
            0,
        )
        .unwrap()
        .encode()
    }

    fn service_state(open: bool) -> ChannelState {
        ChannelState {
            name: "SERVICE".to_owned(),
            credit: 0,
            max_packet_size: 0x100,
            max_service_size: 0x100,
            max_credit: 4,
            open,
        }
    }

    fn active_link(target: ScriptedTransport) -> D4Link<ScriptedTransport> {
        let mut link = D4Link::new(target);
        link.conversation = ConversationState::Active;
        link.transaction_channel_mut().credit = 1;
        link
    }

    #[test]
    fn initializes_then_opens_and_uses_a_service_channel() {
        let mut target = ScriptedTransport::new("scripted");
        target.expect_write(Packet::new(0, 0, [0, 0x20], 1, 0).unwrap().encode());
        target.push_read_data(transaction_packet(
            TransactionMessage::InitReply {
                result: 0,
                revision: ProtocolRevision::V20,
            },
            2,
        ));
        target.expect_write(
            Packet::new(0, 0, b"\x09EPSON-CTRL".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        target.push_read_data(transaction_packet(
            TransactionMessage::GetSocketIdReply {
                result: 0,
                socket_id: 2,
                service_name: "EPSON-CTRL".to_owned(),
            },
            1,
        ));
        target.expect_write(
            Packet::new(0, 0, b"\x01\x02\x02\x01\x00\x01\x00\x00\x00".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        target.push_read_data(transaction_packet(
            TransactionMessage::OpenChannelReply {
                result: 0,
                peer_socket: 2,
                source_socket: 2,
                max_packet_size: 0x100,
                max_service_size: 0x100,
                max_credit: 0,
                granted_credit: 1,
            },
            1,
        ));
        target.expect_write(
            Packet::new(2, 2, b"request".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        target.push_read_data(Packet::new(2, 2, b"reply".to_vec(), 1, 0).unwrap().encode());

        let mut link = D4Link::new(target);
        link.initialize().unwrap();
        let service = link.open_service("EPSON-CTRL").unwrap();
        assert_eq!(
            service,
            ChannelId {
                peer_socket: 2,
                source_socket: 2
            }
        );
        assert_eq!(
            link.control_channel(service)
                .unwrap()
                .request(b"request")
                .unwrap(),
            b"reply"
        );
        link.target().assert_finished();
    }

    #[test]
    fn falls_back_to_revision_10() {
        let mut target = ScriptedTransport::new("scripted");
        target.expect_write(Packet::new(0, 0, [0, 0x20], 1, 0).unwrap().encode());
        target.push_read_data(transaction_packet(
            TransactionMessage::InitReply {
                result: 2,
                revision: ProtocolRevision::V10,
            },
            1,
        ));
        target.expect_write(Packet::new(0, 0, [0, 0x10], 1, 0).unwrap().encode());
        target.push_read_data(
            Packet::new(
                0,
                0,
                TransactionMessage::InitReply {
                    result: 0,
                    revision: ProtocolRevision::V10,
                }
                .encode(ProtocolRevision::V10)
                .unwrap(),
                1,
                0,
            )
            .unwrap()
            .encode(),
        );
        let mut link = D4Link::new(target);
        link.initialize().unwrap();
        assert_eq!(link.revision(), ProtocolRevision::V10);
        link.target().assert_finished();
    }

    #[test]
    fn initialize_resets_transaction_credit_to_one() {
        let mut target = ScriptedTransport::new("scripted");
        target.expect_write(Packet::new(0, 0, [0, 0x20], 1, 0).unwrap().encode());
        target.push_read_data(transaction_packet(
            TransactionMessage::InitReply {
                result: 0,
                revision: ProtocolRevision::V20,
            },
            0,
        ));

        let mut link = D4Link::new(target);
        link.initialize().unwrap();
        assert_eq!(link.channels[&ChannelId::TRANSACTION].credit, 1);
        link.target().assert_finished();
    }

    #[test]
    fn reports_credit_overflow_without_piggyback_credit() {
        let mut target = ScriptedTransport::new("scripted");
        target.expect_write(
            Packet::new(0, 0, [0x7f, 2, 2, 0x86], 0, 0)
                .unwrap()
                .encode(),
        );
        let mut link = D4Link::new(target);
        link.channels.insert(
            ChannelId {
                peer_socket: 2,
                source_socket: 2,
            },
            ChannelState {
                name: "SERVICE".to_owned(),
                credit: u16::MAX,
                max_packet_size: 0x100,
                max_service_size: 0x100,
                max_credit: 0,
                open: true,
            },
        );
        let packet = Packet::new(2, 2, [], 1, 0).unwrap();

        assert!(matches!(
            link.apply_credit(&packet),
            Err(super::D4Error::CreditOverflow { .. })
        ));
        link.target().assert_finished();
    }

    #[test]
    fn completes_a_peer_open_while_local_open_is_pending() {
        let channel = ChannelId {
            peer_socket: 2,
            source_socket: 2,
        };
        let mut target = ScriptedTransport::new("scripted");
        target.expect_write(transaction_packet(
            TransactionMessage::OpenChannelReply {
                result: 0,
                peer_socket: 2,
                source_socket: 2,
                max_packet_size: 0x80,
                max_service_size: 0x40,
                max_credit: 4,
                granted_credit: 0,
            },
            1,
        ));
        let mut link = active_link(target);
        link.channels.insert(channel, service_state(false));

        link.handle_peer_transaction(TransactionMessage::OpenChannel {
            peer_socket: 2,
            source_socket: 2,
            max_packet_size: 0x80,
            max_service_size: 0x40,
            max_credit: 9,
            initial_credit: None,
        })
        .unwrap();

        assert!(!link.channels[&channel].open);
        link.target().assert_finished();
    }

    #[test]
    fn closes_a_channel_when_the_peer_initiates_close() {
        let channel = ChannelId {
            peer_socket: 2,
            source_socket: 2,
        };
        let mut target = ScriptedTransport::new("scripted");
        target.expect_write(transaction_packet(
            TransactionMessage::CloseChannelReply {
                result: 0,
                peer_socket: 2,
                source_socket: 2,
            },
            1,
        ));
        let mut link = active_link(target);
        link.channels.insert(channel, service_state(true));

        link.handle_peer_transaction(TransactionMessage::CloseChannel {
            peer_socket: 2,
            source_socket: 2,
        })
        .unwrap();

        assert!(!link.channels[&channel].open);
        link.target().assert_finished();
    }

    #[test]
    fn closes_a_channel_when_the_local_peer_initiates_close() {
        let channel = ChannelId {
            peer_socket: 2,
            source_socket: 2,
        };
        let mut target = ScriptedTransport::new("scripted");
        target.expect_write(transaction_packet(
            TransactionMessage::CloseChannel {
                peer_socket: 2,
                source_socket: 2,
            },
            1,
        ));
        target.push_read_data(transaction_packet(
            TransactionMessage::CloseChannelReply {
                result: 0,
                peer_socket: 2,
                source_socket: 2,
            },
            1,
        ));
        let mut link = active_link(target);
        link.channels.insert(channel, service_state(true));

        link.close_channel(channel).unwrap();

        assert!(!link.channels.contains_key(&channel));
        link.target().assert_finished();
    }

    #[test]
    fn exits_when_the_peer_initiates_exit() {
        let channel = ChannelId {
            peer_socket: 2,
            source_socket: 2,
        };
        let mut target = ScriptedTransport::new("scripted");
        target.expect_write(transaction_packet(
            TransactionMessage::ExitReply { result: 0 },
            1,
        ));
        let mut link = active_link(target);
        link.channels.insert(channel, service_state(true));

        link.handle_peer_transaction(TransactionMessage::Exit)
            .unwrap();

        assert_eq!(link.conversation, ConversationState::Terminated);
        assert_eq!(link.channels.len(), 1);
        assert!(link.channels.contains_key(&ChannelId::TRANSACTION));
        link.target().assert_finished();
    }

    #[test]
    fn exits_when_the_local_peer_initiates_exit() {
        let mut target = ScriptedTransport::new("scripted");
        target.expect_write(transaction_packet(TransactionMessage::Exit, 1));
        target.push_read_data(transaction_packet(
            TransactionMessage::ExitReply { result: 0 },
            1,
        ));
        let mut link = active_link(target);
        link.channels.insert(
            ChannelId {
                peer_socket: 2,
                source_socket: 2,
            },
            service_state(true),
        );

        link.exit().unwrap();

        assert_eq!(link.conversation, ConversationState::Terminated);
        assert_eq!(link.channels.len(), 1);
        link.target().assert_finished();
    }

    #[test]
    fn services_peer_close_while_delivering_an_outstanding_data_reply() {
        let channel = ChannelId {
            peer_socket: 2,
            source_socket: 2,
        };
        let mut target = ScriptedTransport::new("scripted");
        target.expect_write(
            Packet::new(2, 2, b"request".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        target.expect_write(transaction_packet(
            TransactionMessage::CloseChannelReply {
                result: 0,
                peer_socket: 2,
                source_socket: 2,
            },
            1,
        ));
        let mut inbound = transaction_packet(
            TransactionMessage::CloseChannel {
                peer_socket: 2,
                source_socket: 2,
            },
            1,
        );
        inbound.extend(Packet::new(2, 2, b"reply".to_vec(), 0, 0).unwrap().encode());
        target.push_read_data(inbound);

        let mut link = active_link(target);
        let mut state = service_state(true);
        state.credit = 1;
        link.channels.insert(channel, state);

        assert_eq!(link.request(channel, b"request").unwrap(), b"reply");
        assert!(!link.channels[&channel].open);
        link.target().assert_finished();
    }
}
