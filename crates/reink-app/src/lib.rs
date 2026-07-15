#![forbid(unsafe_code)]
//! Application services that compose a selected transport with ReInk protocols.

use std::error::Error;
use std::fmt;

use reink_core::{EepromReadReply, EpsonController, EpsonError, EpsonSpec, PrinterIdentity};
use reink_d4::{ChannelId, D4Error, D4Link};
#[cfg(target_os = "linux")]
use reink_platform::UsbInterfaceSelector;
use reink_platform::{ByteTransport, TransportError};

const EPSON_D4_ENTRY_COMMAND: &[u8] = b"\x00\x00\x00\x1b\x01@EJL 1284.4\n@EJL\n@EJL\n";
const EPSON_D4_ENTRY_REPLY: &[u8] = b"\x00\x00\x00\x08\x01\x00\xc5\x00";
const ENTRY_REPLY_READ_LIMIT: usize = 5;

/// Outcome of the Epson D4 entry probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EpsonD4EntryProbeResult {
    /// The source-compatible Epson entry reply was recognized.
    Recognized,
    /// The device replied, but its bytes did not match the Epson entry reply.
    Unrecognized { received_bytes: usize },
}

/// Probes Epson D4 entry on a selected Linux USB interface without initializing D4.
///
/// The probe sends only the source-compatible Epson entry exchange. It does
/// not initialize D4, open a service, access EEPROM, write printer state, or
/// reset counters.
#[cfg(target_os = "linux")]
pub fn probe_epson_d4_entry(
    vendor_id: u16,
    product_id: u16,
    interface: UsbInterfaceSelector,
) -> Result<EpsonD4EntryProbeResult, ApplicationError> {
    let result = reink_usb::probe_bounded_exchange(
        vendor_id,
        product_id,
        interface,
        EPSON_D4_ENTRY_COMMAND,
        EPSON_D4_ENTRY_REPLY,
        ENTRY_REPLY_READ_LIMIT,
    )?;
    Ok(match result {
        reink_usb::BoundedExchangeProbeResult::Recognized => EpsonD4EntryProbeResult::Recognized,
        reink_usb::BoundedExchangeProbeResult::Unrecognized { received_bytes } => {
            EpsonD4EntryProbeResult::Unrecognized { received_bytes }
        }
    })
}

/// A read-only Epson control session over an initialized IEEE 1284.4 link.
pub struct EpsonD4Session<T> {
    link: D4Link<T>,
    control_channel: ChannelId,
    spec: EpsonSpec,
}

impl<T: ByteTransport> EpsonD4Session<T> {
    /// Enters Epson D4 mode, initializes the link, and opens `EPSON-CTRL`.
    ///
    /// The Epson entry exchange is source-compatible with ReInkPy and is
    /// intentionally exercised only with scripted transports until hardware
    /// evidence is available for a selected printer family.
    pub fn connect(mut target: T, spec: EpsonSpec) -> Result<Self, ApplicationError> {
        target.write_all(EPSON_D4_ENTRY_COMMAND)?;
        wait_for_entry_reply(&mut target)?;

        let mut link = D4Link::new(target);
        link.initialize()?;
        let control_channel = link.open_service("EPSON-CTRL")?;
        Ok(Self {
            link,
            control_channel,
            spec,
        })
    }

    pub fn spec(&self) -> &EpsonSpec {
        &self.spec
    }

    pub fn read_identity(&mut self) -> Result<PrinterIdentity, ApplicationError> {
        let mut channel = self.link.control_channel(self.control_channel)?;
        Ok(EpsonController::new(&mut channel, &self.spec).read_identity()?)
    }

    pub fn read_eeprom(
        &mut self,
        addresses: &[u16],
    ) -> Result<Vec<EepromReadReply>, ApplicationError> {
        let mut channel = self.link.control_channel(self.control_channel)?;
        Ok(EpsonController::new(&mut channel, &self.spec).read_eeprom(addresses)?)
    }

    /// Closes the control channel and terminates the D4 conversation.
    pub fn shutdown(&mut self) -> Result<(), ApplicationError> {
        self.link.close_channel(self.control_channel)?;
        self.link.exit()?;
        Ok(())
    }

    pub fn into_transport(self) -> T {
        self.link.target()
    }
}

fn wait_for_entry_reply<T: ByteTransport>(target: &mut T) -> Result<(), ApplicationError> {
    let mut reply = Vec::new();
    let mut buffer = [0; 256];
    for _ in 0..ENTRY_REPLY_READ_LIMIT {
        let read = target.read(&mut buffer)?;
        if read == 0 {
            return Err(ApplicationError::EntryReplyMissing);
        }
        reply.extend_from_slice(&buffer[..read]);
        if reply
            .windows(EPSON_D4_ENTRY_REPLY.len())
            .any(|window| window == EPSON_D4_ENTRY_REPLY)
        {
            return Ok(());
        }
    }
    Err(ApplicationError::EntryReplyInvalid)
}

/// Failure while composing the transport, D4, and Epson layers.
#[derive(Debug)]
pub enum ApplicationError {
    Transport(TransportError),
    D4(D4Error),
    Epson(EpsonError),
    #[cfg(target_os = "linux")]
    Usb(reink_usb::UsbOpenError),
    EntryReplyMissing,
    EntryReplyInvalid,
}

impl fmt::Display for ApplicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "transport error: {error}"),
            Self::D4(error) => write!(formatter, "D4 error: {error}"),
            Self::Epson(error) => write!(formatter, "Epson error: {error}"),
            #[cfg(target_os = "linux")]
            Self::Usb(error) => write!(formatter, "USB error: {error}"),
            Self::EntryReplyMissing => formatter.write_str("Epson D4 entry reply was not received"),
            Self::EntryReplyInvalid => {
                formatter.write_str("Epson D4 entry reply was not recognized")
            }
        }
    }
}

impl Error for ApplicationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::D4(error) => Some(error),
            Self::Epson(error) => Some(error),
            #[cfg(target_os = "linux")]
            Self::Usb(error) => Some(error),
            Self::EntryReplyMissing | Self::EntryReplyInvalid => None,
        }
    }
}

impl From<TransportError> for ApplicationError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

impl From<D4Error> for ApplicationError {
    fn from(error: D4Error) -> Self {
        Self::D4(error)
    }
}

impl From<EpsonError> for ApplicationError {
    fn from(error: EpsonError) -> Self {
        Self::Epson(error)
    }
}

#[cfg(target_os = "linux")]
impl From<reink_usb::UsbOpenError> for ApplicationError {
    fn from(error: reink_usb::UsbOpenError) -> Self {
        Self::Usb(error)
    }
}

#[cfg(test)]
mod tests {
    use reink_core::{ModelDatabase, encode_command, encode_eeprom_read};
    use reink_d4::{Packet, ProtocolRevision, TransactionMessage};
    use reink_platform_test::{SanitizedTranscript, TranscriptTransport};

    use super::{ApplicationError, EPSON_D4_ENTRY_COMMAND, EPSON_D4_ENTRY_REPLY, EpsonD4Session};

    fn spec() -> reink_core::EpsonSpec {
        ModelDatabase::builtin()
            .unwrap()
            .get("C90")
            .unwrap()
            .clone()
    }

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

    fn respond_fragmented_packet(target: &mut SanitizedTranscript, packet: Vec<u8>) {
        let split = packet.len().min(3);
        target.respond_fragmented([packet[..split].to_vec(), packet[split..].to_vec()]);
    }

    fn read_only_d4_transcript(spec: &reink_core::EpsonSpec) -> TranscriptTransport {
        let mut target = SanitizedTranscript::new("synthetic Epson D4 read-only lifecycle");
        target.expect_write(EPSON_D4_ENTRY_COMMAND);
        target.respond_fragmented([
            EPSON_D4_ENTRY_REPLY[..4].to_vec(),
            EPSON_D4_ENTRY_REPLY[4..].to_vec(),
        ]);
        target.expect_write(Packet::new(0, 0, [0, 0x20], 1, 0).unwrap().encode());
        respond_fragmented_packet(
            &mut target,
            transaction_packet(
                TransactionMessage::InitReply {
                    result: 0,
                    revision: ProtocolRevision::V20,
                },
                1,
            ),
        );
        target.expect_write(
            Packet::new(0, 0, b"\x09EPSON-CTRL".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            transaction_packet(
                TransactionMessage::GetSocketIdReply {
                    result: 0,
                    socket_id: 2,
                    service_name: "EPSON-CTRL".to_owned(),
                },
                1,
            ),
        );
        target.expect_write(
            Packet::new(0, 0, b"\x01\x02\x02\x01\x00\x01\x00\x00\x00".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            transaction_packet(
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
            ),
        );
        target.expect_write(
            Packet::new(2, 2, encode_command(*b"di", &[1]).unwrap(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            Packet::new(2, 2, b"@EJL ID MFG:EPSON;MDL:C90;".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        target.expect_write(
            Packet::new(2, 2, encode_eeprom_read(spec, 0x0c).unwrap(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            Packet::new(2, 2, b"@BDC PS EE:0C4200;".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        target.expect_write(
            Packet::new(0, 0, b"\x02\x02\x02".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            transaction_packet(
                TransactionMessage::CloseChannelReply {
                    result: 0,
                    peer_socket: 2,
                    source_socket: 2,
                },
                1,
            ),
        );
        target.expect_write(Packet::new(0, 0, [0x08], 1, 0).unwrap().encode());
        respond_fragmented_packet(
            &mut target,
            transaction_packet(TransactionMessage::ExitReply { result: 0 }, 1),
        );
        target.into_transport()
    }

    #[test]
    fn opens_a_read_only_session_and_reads_identity_and_eeprom() {
        let spec = spec();
        let target = read_only_d4_transcript(&spec);

        let mut session = EpsonD4Session::connect(target, spec).unwrap();

        assert_eq!(session.read_identity().unwrap().model(), Some("C90"));
        assert_eq!(session.read_eeprom(&[0x0c]).unwrap()[0].value, 0x42);
        session.shutdown().unwrap();
        session.into_transport().assert_finished();
    }

    #[test]
    fn rejects_an_unrecognized_epson_entry_reply() {
        let mut target = SanitizedTranscript::new("unrecognized Epson D4 entry reply");
        target.expect_write(EPSON_D4_ENTRY_COMMAND);
        target.respond_fragmented([
            b"\x00".to_vec(),
            b"\x00".to_vec(),
            b"\x00".to_vec(),
            b"\x08".to_vec(),
            b"\x01".to_vec(),
        ]);

        let error = match EpsonD4Session::connect(target.into_transport(), spec()) {
            Ok(_) => panic!("unrecognized Epson entry reply unexpectedly opened a D4 session"),
            Err(error) => error,
        };

        assert!(matches!(error, ApplicationError::EntryReplyInvalid));
    }
}
