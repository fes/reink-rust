use std::error::Error;
use std::fmt;

use crate::TransportError;

/// A request/reply channel used by printer-specific command codecs.
///
/// D4 and SNMP adapters implement this trait. Epson command encoding remains
/// in the protocol crate, so this interface has no Epson- or OS-specific data.
pub trait ControlChannel: Send {
    /// Sends one complete request and returns its complete reply.
    fn request(&mut self, request: &[u8]) -> Result<Vec<u8>, ControlError>;
}

/// A failure while exchanging a request over a control channel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlError {
    Transport(TransportError),
    Protocol { message: String },
    DeviceRejected { message: String },
}

impl fmt::Display for ControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "transport error: {error}"),
            Self::Protocol { message } => write!(formatter, "protocol error: {message}"),
            Self::DeviceRejected { message } => {
                write!(formatter, "device rejected request: {message}")
            }
        }
    }
}

impl Error for ControlError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Protocol { .. } | Self::DeviceRejected { .. } => None,
        }
    }
}

impl From<TransportError> for ControlError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}
