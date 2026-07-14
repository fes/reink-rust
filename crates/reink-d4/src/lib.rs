#![forbid(unsafe_code)]
//! Blocking IEEE 1284.4 framing, transactions, and service channels.

mod link;
mod packet;
mod transaction;

pub use link::{ChannelId, D4ControlChannel, D4Error, D4Link, ProtocolRevision};
pub use packet::{Packet, PacketDecoder, PacketError, PacketHeader};
pub use transaction::{TransactionMessage, TransactionParseError};
