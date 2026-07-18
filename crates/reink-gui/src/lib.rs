#![forbid(unsafe_code)]
//! Guarded UI state for the optional ReInk GUI.
//!
//! Fixtures never open a transport. Connected USB candidates are descriptor-only:
//! they do not identify a printer or permit device, EEPROM, or maintenance access
//! until the executable's explicit selected-printer operation is confirmed.

use std::collections::VecDeque;

use reink_core::{EpsonSpec, ModelDatabase, PrinterIdentity};
use reink_d4::{PacketHeader, ProtocolRevision, TransactionMessage};
use reink_platform::TransportEvent;

/// Maximum number of transport events retained for one GUI session.
pub const DEBUG_TRAFFIC_MAX_ENTRIES: usize = 1_000;
const D4_HEADER_LENGTH: usize = 6;
const EPSON_D4_ENTRY_COMMAND: &[u8] = b"\x00\x00\x00\x1b\x01@EJL 1284.4\n@EJL\n@EJL\n";
const EPSON_D4_ENTRY_REPLY: &[u8] = b"\x00\x00\x00\x08\x01\x00\xc5\x00";

/// Launch mode controlling whether bundled fixtures can be selected.
///
/// Real mode is the default and contains no implicit printer source.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SourceMode {
    #[default]
    Real,
    Fixtures,
}

impl SourceMode {
    pub const fn fixtures_enabled(self) -> bool {
        matches!(self, Self::Fixtures)
    }
}

/// Direction of one recorded transport transfer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DebugTrafficDirection {
    Tx,
    Rx,
}

impl DebugTrafficDirection {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tx => "TX",
            Self::Rx => "RX",
        }
    }
}

/// One display-safe, session-only transport record.
///
/// The byte string is uppercase hexadecimal separated by single spaces. It is
/// retained only for the current opt-in session and has no timestamp or device
/// identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugTrafficEntry {
    id: u64,
    direction: DebugTrafficDirection,
    summary: String,
    hex_bytes: String,
}

impl DebugTrafficEntry {
    pub const fn id(&self) -> u64 {
        self.id
    }

    pub const fn direction(&self) -> DebugTrafficDirection {
        self.direction
    }

    pub fn summary(&self) -> &str {
        &self.summary
    }

    pub fn hex_bytes(&self) -> &str {
        &self.hex_bytes
    }

    pub fn clipboard_text(&self) -> String {
        let bytes = if self.hex_bytes.is_empty() {
            "<empty>"
        } else {
            &self.hex_bytes
        };
        format!("{}\nbytes={bytes}", self.summary)
    }
}

#[derive(Debug)]
struct ReassembledTransfer {
    direction: DebugTrafficDirection,
    bytes: Vec<u8>,
    kind: ReassembledTransferKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReassembledTransferKind {
    EmptyRead,
    Raw,
    EntryCommand,
    EntryReply,
    D4Packet,
}

/// Reassembles one unidirectional USB byte stream without assigning meaning to
/// incomplete frames until their protocol boundary is available.
#[derive(Debug, Default)]
struct DebugTrafficDecoder {
    bytes: Vec<u8>,
    d4_active: bool,
}

impl DebugTrafficDecoder {
    fn push(&mut self, direction: DebugTrafficDirection, bytes: &[u8]) -> Vec<ReassembledTransfer> {
        if bytes.is_empty() {
            return match direction {
                DebugTrafficDirection::Rx => vec![ReassembledTransfer {
                    direction,
                    bytes: Vec::new(),
                    kind: ReassembledTransferKind::EmptyRead,
                }],
                DebugTrafficDirection::Tx => vec![ReassembledTransfer {
                    direction,
                    bytes: Vec::new(),
                    kind: ReassembledTransferKind::Raw,
                }],
            };
        }

        if !self.d4_active && !self.bytes.is_empty() {
            let entry = match direction {
                DebugTrafficDirection::Tx => EPSON_D4_ENTRY_COMMAND,
                DebugTrafficDirection::Rx => EPSON_D4_ENTRY_REPLY,
            };
            let mut candidate = self.bytes.clone();
            candidate.extend_from_slice(bytes);
            if !candidate.starts_with(entry) && !is_prefix_of(&candidate, entry) {
                let pending = std::mem::take(&mut self.bytes);
                let mut transfers = vec![ReassembledTransfer {
                    direction,
                    bytes: pending,
                    kind: ReassembledTransferKind::Raw,
                }];
                transfers.extend(self.push(direction, bytes));
                return transfers;
            }
        }

        self.bytes.extend_from_slice(bytes);
        let entry = match direction {
            DebugTrafficDirection::Tx => EPSON_D4_ENTRY_COMMAND,
            DebugTrafficDirection::Rx => EPSON_D4_ENTRY_REPLY,
        };
        let mut transfers = Vec::new();

        loop {
            if !self.d4_active {
                if self.bytes.starts_with(entry) {
                    let bytes = self.bytes.drain(..entry.len()).collect();
                    self.d4_active = true;
                    transfers.push(ReassembledTransfer {
                        direction,
                        bytes,
                        kind: match direction {
                            DebugTrafficDirection::Tx => ReassembledTransferKind::EntryCommand,
                            DebugTrafficDirection::Rx => ReassembledTransferKind::EntryReply,
                        },
                    });
                    continue;
                }
                if is_prefix_of(&self.bytes, entry) {
                    break;
                }

                transfers.push(ReassembledTransfer {
                    direction,
                    bytes: std::mem::take(&mut self.bytes),
                    kind: ReassembledTransferKind::Raw,
                });
                break;
            }

            if self.bytes.len() < D4_HEADER_LENGTH {
                break;
            }

            let header = PacketHeader::decode(&self.bytes[..D4_HEADER_LENGTH]);
            let Ok(header) = header else {
                let split_at = self.next_packet_start().unwrap_or(self.bytes.len());
                transfers.push(ReassembledTransfer {
                    direction,
                    bytes: self.bytes.drain(..split_at).collect(),
                    kind: ReassembledTransferKind::Raw,
                });
                continue;
            };
            let total_length = usize::from(header.length);
            if self.bytes.len() < total_length {
                break;
            }
            transfers.push(ReassembledTransfer {
                direction,
                bytes: self.bytes.drain(..total_length).collect(),
                kind: ReassembledTransferKind::D4Packet,
            });
        }

        transfers
    }

    fn next_packet_start(&self) -> Option<usize> {
        (1..self.bytes.len().saturating_sub(D4_HEADER_LENGTH - 1)).find(|start| {
            PacketHeader::decode(&self.bytes[*start..*start + D4_HEADER_LENGTH]).is_ok()
        })
    }
}

fn is_prefix_of(bytes: &[u8], whole: &[u8]) -> bool {
    bytes.len() <= whole.len() && bytes.iter().zip(whole).all(|(left, right)| left == right)
}

/// Bounded, in-memory debug traffic for the current GUI session.
///
/// Capture is disabled by default. An explicit connected operation that sampled
/// the opt-in before it started can pass `RecordingTransport::into_parts().1`
/// to [`Self::append_captured_events`].
#[derive(Debug)]
pub struct DebugTrafficTrace {
    capture_enabled: bool,
    entries: VecDeque<DebugTrafficEntry>,
    tx_decoder: DebugTrafficDecoder,
    rx_decoder: DebugTrafficDecoder,
    transaction_revision: ProtocolRevision,
    next_entry_id: u64,
}

impl Default for DebugTrafficTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl DebugTrafficTrace {
    pub fn new() -> Self {
        Self {
            capture_enabled: false,
            entries: VecDeque::new(),
            tx_decoder: DebugTrafficDecoder::default(),
            rx_decoder: DebugTrafficDecoder::default(),
            transaction_revision: ProtocolRevision::V20,
            next_entry_id: 0,
        }
    }

    pub const fn capture_enabled(&self) -> bool {
        self.capture_enabled
    }

    pub fn set_capture_enabled(&mut self, enabled: bool) {
        self.capture_enabled = enabled;
    }

    /// Appends one event when capture is enabled, returning whether it was kept.
    pub fn append(&mut self, event: &TransportEvent) -> bool {
        if !self.capture_enabled {
            return false;
        }
        self.append_captured(event);
        true
    }

    /// Appends ordered `RecordingTransport` events when capture is enabled.
    ///
    /// Transfers are incrementally reassembled into D4 entry and packet records.
    /// Empty reads remain visible without changing reassembly state.
    pub fn append_events(&mut self, events: Vec<TransportEvent>) -> usize {
        events.iter().filter(|event| self.append(event)).count()
    }

    /// Appends events that were captured under an already sampled explicit
    /// operation opt-in.
    ///
    /// Callers must invoke this only when their operation started with capture
    /// enabled. It preserves that decision even if the user toggles the
    /// checkbox while the worker is still completing.
    pub fn append_captured_events(&mut self, events: Vec<TransportEvent>) -> usize {
        events
            .iter()
            .map(|event| self.append_captured(event))
            .count()
    }

    fn append_captured(&mut self, event: &TransportEvent) {
        let (direction, bytes) = match event {
            TransportEvent::Tx(bytes) => (DebugTrafficDirection::Tx, bytes),
            TransportEvent::Rx(bytes) => (DebugTrafficDirection::Rx, bytes),
        };
        let transfers = match direction {
            DebugTrafficDirection::Tx => self.tx_decoder.push(direction, bytes),
            DebugTrafficDirection::Rx => self.rx_decoder.push(direction, bytes),
        };
        for transfer in transfers {
            self.append_transfer(transfer);
        }
    }

    fn append_transfer(&mut self, transfer: ReassembledTransfer) {
        let summary = self.summary_for(&transfer);
        if self.entries.len() == DEBUG_TRAFFIC_MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(DebugTrafficEntry {
            id: self.next_entry_id,
            direction: transfer.direction,
            summary,
            hex_bytes: format_hex_bytes(&transfer.bytes),
        });
        self.next_entry_id = self.next_entry_id.wrapping_add(1);
    }

    fn summary_for(&mut self, transfer: &ReassembledTransfer) -> String {
        let direction = transfer.direction.label();
        match transfer.kind {
            ReassembledTransferKind::EmptyRead => {
                format!("{direction} usb_bulk_in=empty observation=timeout_like")
            }
            ReassembledTransferKind::Raw if transfer.bytes.is_empty() => {
                format!("{direction} usb_bulk_out=empty")
            }
            ReassembledTransferKind::Raw => {
                format!(
                    "{direction} transfer=raw framing=unrecognized bytes={}",
                    transfer.bytes.len()
                )
            }
            ReassembledTransferKind::EntryCommand => {
                format!(
                    "{direction} epson_d4_entry=command bytes={}",
                    transfer.bytes.len()
                )
            }
            ReassembledTransferKind::EntryReply => {
                format!(
                    "{direction} epson_d4_entry=reply result=recognized bytes={}",
                    transfer.bytes.len()
                )
            }
            ReassembledTransferKind::D4Packet => self.d4_packet_summary(direction, &transfer.bytes),
        }
    }

    fn d4_packet_summary(&mut self, direction: &str, bytes: &[u8]) -> String {
        let Ok(header) = PacketHeader::decode(bytes) else {
            return format!(
                "{direction} transfer=raw framing=unrecognized bytes={}",
                bytes.len()
            );
        };
        let payload = bytes.get(D4_HEADER_LENGTH..).unwrap_or_default();
        let request_response = if direction == "TX" {
            "request"
        } else {
            "response"
        };
        let mut fields = vec![
            format!("{direction} d4=packet"),
            format!("request_response={request_response}"),
            format!("peer_socket={}", header.peer_socket),
            format!("source_socket={}", header.source_socket),
            format!("total_length={}", header.length),
            format!("payload_length={}", header.payload_length()),
            format!("credit={}", header.credit),
            format!("control=0x{:02X}", header.control),
            format!("control_bits=0b{:02b}", header.control),
        ];
        if header.channel_id() == (0, 0) {
            fields.extend(self.transaction_fields(payload));
        } else {
            fields.extend(control_payload_fields(payload));
        }
        fields.join(" ")
    }

    fn transaction_fields(&mut self, payload: &[u8]) -> Vec<String> {
        let mut parsed = None;
        for revision in [
            self.transaction_revision,
            alternate_revision(self.transaction_revision),
        ] {
            if let Ok(message) = TransactionMessage::decode(payload, revision) {
                parsed = Some((message, revision));
                break;
            }
        }
        let Some((message, revision)) = parsed else {
            return vec![format!(
                "transaction=unknown revision_tried={}",
                revision_label(self.transaction_revision)
            )];
        };

        let mut fields = vec![format!("revision={}", revision_label(revision))];
        match &message {
            TransactionMessage::Init { revision } => {
                fields.push("transaction=init".to_owned());
                fields.push(format!("requested_revision={}", revision_label(*revision)));
                self.transaction_revision = *revision;
            }
            TransactionMessage::InitReply { result, revision } => {
                fields.push("transaction=init_reply".to_owned());
                fields.push(format!("result=0x{result:02X}"));
                fields.push(format!("negotiated_revision={}", revision_label(*revision)));
                self.transaction_revision = *revision;
            }
            TransactionMessage::OpenChannel {
                peer_socket,
                source_socket,
                max_packet_size,
                max_service_size,
                max_credit,
                initial_credit,
            } => {
                fields.push("transaction=open_channel".to_owned());
                fields.push(format!("transaction_peer_socket={peer_socket}"));
                fields.push(format!("transaction_source_socket={source_socket}"));
                fields.push(format!("max_packet_size={max_packet_size}"));
                fields.push(format!("max_service_size={max_service_size}"));
                fields.push(format!("max_credit={max_credit}"));
                if let Some(initial_credit) = initial_credit {
                    fields.push(format!("initial_credit={initial_credit}"));
                }
            }
            TransactionMessage::OpenChannelReply {
                result,
                peer_socket,
                source_socket,
                max_packet_size,
                max_service_size,
                max_credit,
                granted_credit,
            } => {
                fields.push("transaction=open_channel_reply".to_owned());
                fields.push(format!("result=0x{result:02X}"));
                fields.push(format!("transaction_peer_socket={peer_socket}"));
                fields.push(format!("transaction_source_socket={source_socket}"));
                fields.push(format!("max_packet_size={max_packet_size}"));
                fields.push(format!("max_service_size={max_service_size}"));
                fields.push(format!("max_credit={max_credit}"));
                fields.push(format!("granted_credit={granted_credit}"));
            }
            TransactionMessage::CloseChannel {
                peer_socket,
                source_socket,
            } => {
                fields.push("transaction=close_channel".to_owned());
                fields.push(format!("transaction_peer_socket={peer_socket}"));
                fields.push(format!("transaction_source_socket={source_socket}"));
            }
            TransactionMessage::CloseChannelReply {
                result,
                peer_socket,
                source_socket,
            } => {
                fields.push("transaction=close_channel_reply".to_owned());
                fields.push(format!("result=0x{result:02X}"));
                fields.push(format!("transaction_peer_socket={peer_socket}"));
                fields.push(format!("transaction_source_socket={source_socket}"));
            }
            TransactionMessage::Credit {
                peer_socket,
                source_socket,
                added_credit,
            } => {
                fields.push("transaction=credit".to_owned());
                fields.push(format!("transaction_peer_socket={peer_socket}"));
                fields.push(format!("transaction_source_socket={source_socket}"));
                fields.push(format!("added_credit={added_credit}"));
            }
            TransactionMessage::CreditReply {
                result,
                peer_socket,
                source_socket,
            } => {
                fields.push("transaction=credit_reply".to_owned());
                fields.push(format!("result=0x{result:02X}"));
                fields.push(format!("transaction_peer_socket={peer_socket}"));
                fields.push(format!("transaction_source_socket={source_socket}"));
            }
            TransactionMessage::CreditRequest {
                peer_socket,
                source_socket,
                max_credit,
            } => {
                fields.push("transaction=credit_request".to_owned());
                fields.push(format!("transaction_peer_socket={peer_socket}"));
                fields.push(format!("transaction_source_socket={source_socket}"));
                fields.push(format!("max_credit={max_credit}"));
            }
            TransactionMessage::CreditRequestReply {
                result,
                peer_socket,
                source_socket,
                added_credit,
            } => {
                fields.push("transaction=credit_request_reply".to_owned());
                fields.push(format!("result=0x{result:02X}"));
                fields.push(format!("transaction_peer_socket={peer_socket}"));
                fields.push(format!("transaction_source_socket={source_socket}"));
                fields.push(format!("added_credit={added_credit}"));
            }
            TransactionMessage::Exit => fields.push("transaction=exit".to_owned()),
            TransactionMessage::ExitReply { result } => {
                fields.push("transaction=exit_reply".to_owned());
                fields.push(format!("result=0x{result:02X}"));
            }
            TransactionMessage::GetSocketId { service_name } => {
                fields.push("transaction=get_socket_id".to_owned());
                fields.push(format!("service_name={service_name}"));
            }
            TransactionMessage::GetSocketIdReply {
                result,
                socket_id,
                service_name,
            } => {
                fields.push("transaction=get_socket_id_reply".to_owned());
                fields.push(format!("result=0x{result:02X}"));
                fields.push(format!("socket_id={socket_id}"));
                if !service_name.is_empty() {
                    fields.push(format!("service_name={service_name}"));
                }
            }
            TransactionMessage::GetServiceName { socket_id } => {
                fields.push("transaction=get_service_name".to_owned());
                fields.push(format!("socket_id={socket_id}"));
            }
            TransactionMessage::GetServiceNameReply {
                result,
                socket_id,
                service_name,
            } => {
                fields.push("transaction=get_service_name_reply".to_owned());
                fields.push(format!("result=0x{result:02X}"));
                fields.push(format!("socket_id={socket_id}"));
                if !service_name.is_empty() {
                    fields.push(format!("service_name={service_name}"));
                }
            }
            TransactionMessage::Error {
                peer_socket,
                source_socket,
                error_code,
            } => {
                fields.push("transaction=error".to_owned());
                fields.push(format!("transaction_peer_socket={peer_socket}"));
                fields.push(format!("transaction_source_socket={source_socket}"));
                fields.push(format!("error_code=0x{error_code:02X}"));
            }
        }
        fields
    }

    /// Begins a newly captured operation without removing already displayed traffic.
    ///
    /// Reassembly and transaction decoding are scoped to a single selected-printer
    /// operation, so incomplete transfers from an earlier operation cannot affect it.
    pub fn begin_operation(&mut self) {
        self.reset_decoder_state();
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.reset_decoder_state();
    }

    pub fn count(&self) -> usize {
        self.entries.len()
    }

    pub fn clipboard_text(&self) -> String {
        self.entries
            .iter()
            .map(DebugTrafficEntry::clipboard_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn entries(
        &self,
    ) -> impl ExactSizeIterator<Item = &DebugTrafficEntry> + DoubleEndedIterator {
        self.entries.iter()
    }

    fn reset_decoder_state(&mut self) {
        self.tx_decoder = DebugTrafficDecoder::default();
        self.rx_decoder = DebugTrafficDecoder::default();
        self.transaction_revision = ProtocolRevision::V20;
    }
}

fn format_hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn alternate_revision(revision: ProtocolRevision) -> ProtocolRevision {
    match revision {
        ProtocolRevision::V10 => ProtocolRevision::V20,
        ProtocolRevision::V20 => ProtocolRevision::V10,
    }
}

fn revision_label(revision: ProtocolRevision) -> &'static str {
    match revision {
        ProtocolRevision::V10 => "V10",
        ProtocolRevision::V20 => "V20",
    }
}

fn control_payload_fields(payload: &[u8]) -> Vec<String> {
    if payload.is_ascii() && (payload.starts_with(b"@") || payload.starts_with(b"EE:")) {
        return ascii_response_fields(payload);
    }
    let Some(command) = payload.get(..2) else {
        return vec![format!("payload=raw payload_bytes={}", payload.len())];
    };
    let Some(length_bytes) = payload.get(2..4) else {
        return vec![format!(
            "command={} payload=truncated payload_bytes={}",
            command_label(command),
            payload.len()
        )];
    };
    let declared_length = usize::from(u16::from_le_bytes([length_bytes[0], length_bytes[1]]));
    let body = &payload[4..];
    let mut fields = vec![
        format!("command={}", command_label(command)),
        format!("payload_length={declared_length}"),
    ];
    if body.len() != declared_length {
        fields.push(format!("payload_actual_length={}", body.len()));
    }

    if command == b"||" {
        fields.extend(factory_command_fields(
            &body[..body.len().min(declared_length)],
        ));
    } else if body.is_ascii() {
        fields.push(format!("payload_ascii={}", ascii_summary(body)));
    }
    fields
}

fn command_label(command: &[u8]) -> String {
    if command.len() == 2 && command.iter().all(u8::is_ascii_graphic) {
        String::from_utf8_lossy(command).into_owned()
    } else {
        format_hex_bytes(command)
    }
}

fn factory_command_fields(factory: &[u8]) -> Vec<String> {
    let mut fields = Vec::new();
    let Some(key) = factory.get(..2) else {
        fields.push(format!("factory=truncated factory_bytes={}", factory.len()));
        return fields;
    };
    fields.push(format!(
        "factory_key=0x{:04X}",
        u16::from_le_bytes([key[0], key[1]])
    ));
    let Some(operation) = factory.get(2).copied() else {
        fields.push(format!("factory=truncated factory_bytes={}", factory.len()));
        return fields;
    };
    let complement = factory.get(3).copied();
    let check = factory.get(4).copied();
    fields.push(format!(
        "operation={}",
        char::from(operation).escape_default()
    ));
    if let Some(complement) = complement {
        fields.push(format!("operation_complement=0x{complement:02X}"));
        fields.push(format!("complement_valid={}", complement == !operation));
    }
    if let Some(check) = check {
        let expected = ((operation >> 1) & 0x7f) | ((operation << 7) & 0x80);
        fields.push(format!("check=0x{check:02X}"));
        fields.push(format!("check_valid={}", check == expected));
    }

    let body = factory.get(5..).unwrap_or_default();
    match operation {
        b'A' => {
            fields.push(format!("address_bytes={}", format_hex_bytes(body)));
            if body.len() == 2 {
                fields.push(format!(
                    "address=0x{:04X}",
                    u16::from_le_bytes([body[0], body[1]])
                ));
            }
        }
        b'B' if body.len() >= 3 => {
            let address = &body[..2];
            fields.push(format!("address_bytes={}", format_hex_bytes(address)));
            fields.push(format!(
                "address=0x{:04X}",
                u16::from_le_bytes([address[0], address[1]])
            ));
            fields.push(format!("value=0x{:02X}", body[2]));
            fields.push(format!("write_key_length={}", body.len() - 3));
        }
        b'B' => fields.push(format!("factory_write=truncated body_bytes={}", body.len())),
        _ => fields.push(format!("factory_body_length={}", body.len())),
    }
    fields
}

fn ascii_summary(bytes: &[u8]) -> String {
    let mut value = String::new();
    for byte in bytes.iter().take(80) {
        match byte {
            b' '..=b'~' => value.push(char::from(*byte)),
            b'\r' => value.push_str("\\r"),
            b'\n' => value.push_str("\\n"),
            b'\t' => value.push_str("\\t"),
            _ => value.push('.'),
        }
    }
    if bytes.len() > 80 {
        value.push('…');
    }
    value
}

fn ascii_response_fields(payload: &[u8]) -> Vec<String> {
    let mut fields = vec![
        "response=ascii".to_owned(),
        format!("ascii={}", ascii_summary(payload)),
    ];
    let Some(marker) = payload.windows(3).position(|window| window == b"EE:") else {
        return fields;
    };
    let Some(hex) = payload.get(marker + 3..marker + 9) else {
        return fields;
    };
    if payload.get(marker + 9) != Some(&b';') || !hex.iter().all(u8::is_ascii_hexdigit) {
        return fields;
    }
    let parse_byte = |pair: &[u8]| {
        std::str::from_utf8(pair)
            .ok()
            .and_then(|value| u8::from_str_radix(value, 16).ok())
    };
    let (Some(high), Some(low), Some(value)) = (
        parse_byte(&hex[..2]),
        parse_byte(&hex[2..4]),
        parse_byte(&hex[4..]),
    ) else {
        return fields;
    };
    fields.push(format!(
        "eeprom_address=0x{:04X}",
        u16::from_be_bytes([high, low])
    ));
    fields.push(format!("eeprom_value=0x{value:02X}"));
    fields
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Page {
    Status,
    Eeprom,
    Tools,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationStatus {
    Success,
    Blocked,
    Failure,
}

impl ValidationStatus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Success => "Success",
            Self::Blocked => "Blocked",
            Self::Failure => "Failure",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidationReportItem {
    pub status: ValidationStatus,
    pub check: &'static str,
    pub detail: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EepromRow {
    pub address: u16,
    pub value: u8,
    pub label: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixtureDevice {
    pub label: &'static str,
    pub identity: &'static str,
    pub validation_report: &'static [ValidationReportItem],
    pub eeprom_rows: &'static [EepromRow],
    pub eeprom_bytes: &'static [u8],
}

/// Descriptor-only USB printer information shown for the current GUI session.
///
/// This intentionally omits USB strings and public device handles. A native
/// token, when present, is process-local and redacted by its `Debug`
/// implementation. The alias is session-local and model hints are only exact
/// VID/PID database matches.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DescriptorCandidateBackend {
    LibUsb,
    #[cfg(target_os = "windows")]
    WindowsNative(reink_usb::WindowsNativePrinterCandidate),
}

impl DescriptorCandidateBackend {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::LibUsb => "libusb",
            #[cfg(target_os = "windows")]
            Self::WindowsNative(_) => "Windows USBPRINT (experimental mutation)",
        }
    }

    pub const fn permits_persistent_mutation(&self) -> bool {
        match self {
            Self::LibUsb => true,
            #[cfg(target_os = "windows")]
            Self::WindowsNative(candidate) => candidate.capabilities().persistent_mutation,
        }
    }

    pub const fn experimental_mutation(&self) -> bool {
        match self {
            Self::LibUsb => false,
            #[cfg(target_os = "windows")]
            Self::WindowsNative(candidate) => candidate.capabilities().experimental_mutation,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DescriptorCandidate {
    pub alias: String,
    pub backend: DescriptorCandidateBackend,
    pub vendor_id: u16,
    pub product_id: u16,
    pub bus_number: Option<u8>,
    pub device_address: Option<u8>,
    pub interface_number: Option<u8>,
    pub alternate_setting: Option<u8>,
    pub model_hints: Vec<String>,
}

impl DescriptorCandidate {
    pub const fn permits_persistent_mutation(&self) -> bool {
        self.backend.permits_persistent_mutation()
    }

    pub const fn experimental_mutation(&self) -> bool {
        self.backend.experimental_mutation()
    }
}

fn model_hints_for_usb_candidate(
    database: &ModelDatabase,
    vendor_id: u16,
    product_id: u16,
) -> Vec<String> {
    database
        .models()
        .filter(|model| {
            database.get(model).is_some_and(|spec| {
                spec.vendor_id == vendor_id && spec.product_id == Some(product_id)
            })
        })
        .map(str::to_owned)
        .collect()
}

const XP_352_REPORT: &[ValidationReportItem] = &[
    ValidationReportItem {
        status: ValidationStatus::Success,
        check: "Fixture safety boundary",
        detail: "Fixture mode is active; no physical transport is linked.",
    },
    ValidationReportItem {
        status: ValidationStatus::Success,
        check: "Identity parsing",
        detail: "The IEEE 1284 fixture identity parsed successfully.",
    },
    ValidationReportItem {
        status: ValidationStatus::Blocked,
        check: "EEPROM read",
        detail: "Blocked intentionally: this GUI has no transport dependency.",
    },
    ValidationReportItem {
        status: ValidationStatus::Failure,
        check: "Fixture protocol replay",
        detail: "Simulated malformed reply retained to demonstrate a visible failure state.",
    },
];

const C90_REPORT: &[ValidationReportItem] = &[
    ValidationReportItem {
        status: ValidationStatus::Success,
        check: "Fixture safety boundary",
        detail: "Fixture mode is active; no physical transport is linked.",
    },
    ValidationReportItem {
        status: ValidationStatus::Success,
        check: "Model resolution",
        detail: "The bundled model database resolved the C90 fixture.",
    },
    ValidationReportItem {
        status: ValidationStatus::Blocked,
        check: "Waste-counter reset",
        detail: "Blocked intentionally: reset operations are not present in this GUI.",
    },
    ValidationReportItem {
        status: ValidationStatus::Failure,
        check: "Fixture reply validation",
        detail: "Simulated checksum mismatch retained to demonstrate a visible failure state.",
    },
];

const XP_352_EEPROM: &[EepromRow] = &[
    EepromRow {
        address: 0x0006,
        value: 0x18,
        label: "Fixture counter byte A",
    },
    EepromRow {
        address: 0x0007,
        value: 0x04,
        label: "Fixture counter byte B",
    },
    EepromRow {
        address: 0x000c,
        value: 0x57,
        label: "Fixture maintenance byte",
    },
];

const C90_EEPROM: &[EepromRow] = &[
    EepromRow {
        address: 0x0006,
        value: 0x00,
        label: "Fixture counter byte A",
    },
    EepromRow {
        address: 0x0007,
        value: 0x20,
        label: "Fixture counter byte B",
    },
    EepromRow {
        address: 0x0035,
        value: 0x57,
        label: "Fixture maintenance byte",
    },
];

const FIXTURE_EEPROM_LENGTH: usize = 256;

const fn fixture_eeprom(rows: &[EepromRow]) -> [u8; FIXTURE_EEPROM_LENGTH] {
    let mut bytes = [0; FIXTURE_EEPROM_LENGTH];
    let mut index = 0;
    while index < rows.len() {
        let row = rows[index];
        bytes[row.address as usize] = row.value;
        index += 1;
    }
    bytes
}

const XP_352_EEPROM_BYTES: [u8; FIXTURE_EEPROM_LENGTH] = fixture_eeprom(XP_352_EEPROM);
const C90_EEPROM_BYTES: [u8; FIXTURE_EEPROM_LENGTH] = fixture_eeprom(C90_EEPROM);

pub const FIXTURE_DEVICES: &[FixtureDevice] = &[
    FixtureDevice {
        label: "XP-352 fixture",
        identity: "MFG:EPSON;MDL:XP-352 Series;CMD:ESCPL2,BDC;SN:FIXTURE-0001;",
        validation_report: XP_352_REPORT,
        eeprom_rows: XP_352_EEPROM,
        eeprom_bytes: &XP_352_EEPROM_BYTES,
    },
    FixtureDevice {
        label: "C90 fixture",
        identity: "MFG:EPSON;MDL:C90;CMD:ESCPL2;SN:FIXTURE-0002;",
        validation_report: C90_REPORT,
        eeprom_rows: C90_EEPROM,
        eeprom_bytes: &C90_EEPROM_BYTES,
    },
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityResolution {
    pub manufacturer: Option<String>,
    pub advertised_model: Option<String>,
    pub resolved_model: Option<String>,
}

#[derive(Debug)]
pub struct GuiState {
    page: Page,
    selected_fixture: usize,
    selected_eeprom_row: usize,
    database: ModelDatabase,
}

impl GuiState {
    pub fn new() -> Result<Self, reink_core::SpecError> {
        Ok(Self {
            page: Page::Status,
            selected_fixture: 0,
            selected_eeprom_row: 0,
            database: ModelDatabase::builtin()?,
        })
    }

    pub const fn page(&self) -> Page {
        self.page
    }

    pub const fn selected_fixture_index(&self) -> usize {
        self.selected_fixture
    }

    pub fn selected_fixture(&self) -> &'static FixtureDevice {
        &FIXTURE_DEVICES[self.selected_fixture]
    }

    pub fn select_fixture(&mut self, index: usize) {
        if index < FIXTURE_DEVICES.len() {
            self.selected_fixture = index;
            self.selected_eeprom_row = 0;
        }
    }

    pub const fn selected_eeprom_row_index(&self) -> usize {
        self.selected_eeprom_row
    }

    pub fn selected_eeprom_row(&self) -> &'static EepromRow {
        &self.selected_fixture().eeprom_rows[self.selected_eeprom_row]
    }

    pub fn select_eeprom_row(&mut self, index: usize) {
        if index < self.selected_fixture().eeprom_rows.len() {
            self.selected_eeprom_row = index;
        }
    }

    pub fn navigate_to(&mut self, page: Page) {
        self.page = page;
    }

    pub fn identity_resolution(&self) -> IdentityResolution {
        let identity = PrinterIdentity::parse(self.selected_fixture().identity).ok();
        let resolved_model = identity
            .as_ref()
            .and_then(|identity| self.database.resolve_identity(identity))
            .map(|spec| spec.model.clone());
        IdentityResolution {
            manufacturer: identity
                .as_ref()
                .and_then(PrinterIdentity::manufacturer)
                .map(str::to_owned),
            advertised_model: identity
                .as_ref()
                .and_then(PrinterIdentity::model)
                .map(str::to_owned),
            resolved_model,
        }
    }

    pub fn model_names(&self) -> impl Iterator<Item = &str> {
        self.database.models()
    }

    pub fn model_spec(&self, model: &str) -> Option<&EpsonSpec> {
        self.database.get(model)
    }

    /// Returns bundled model names whose explicit VID/PID exactly matches.
    ///
    /// A match is a display hint only; USB descriptors do not confirm printer
    /// identity.
    pub fn model_hints_for_usb_candidate(&self, vendor_id: u16, product_id: u16) -> Vec<String> {
        model_hints_for_usb_candidate(&self.database, vendor_id, product_id)
    }
}

#[cfg(test)]
mod tests {
    use reink_core::ModelDatabase;
    use reink_d4::{Packet, ProtocolRevision, TransactionMessage};
    use reink_platform::TransportEvent;

    use super::{
        DEBUG_TRAFFIC_MAX_ENTRIES, DebugTrafficDirection, DebugTrafficTrace,
        EPSON_D4_ENTRY_COMMAND, EPSON_D4_ENTRY_REPLY, FIXTURE_DEVICES, GuiState, Page, SourceMode,
        ValidationStatus, format_hex_bytes, model_hints_for_usb_candidate,
    };

    #[test]
    fn debug_trace_formats_tx_rx_and_preserves_read_fragments_in_order() {
        let mut trace = DebugTrafficTrace::new();
        trace.set_capture_enabled(true);

        assert_eq!(
            trace.append_events(vec![
                TransportEvent::Tx(vec![0x1b, 0x40]),
                TransportEvent::Rx(vec![0x06]),
                TransportEvent::Rx(vec![]),
            ]),
            3
        );

        let entries = trace.entries().collect::<Vec<_>>();
        assert_eq!(entries[0].direction(), DebugTrafficDirection::Tx);
        assert_eq!(entries[0].hex_bytes(), "1B 40");
        assert_eq!(entries[1].direction(), DebugTrafficDirection::Rx);
        assert_eq!(entries[1].hex_bytes(), "06");
        assert_eq!(entries[2].direction(), DebugTrafficDirection::Rx);
        assert_eq!(entries[2].hex_bytes(), "");
        assert_eq!(
            entries[2].summary(),
            "RX usb_bulk_in=empty observation=timeout_like"
        );
        assert_eq!(
            entries[2].clipboard_text(),
            "RX usb_bulk_in=empty observation=timeout_like\nbytes=<empty>"
        );
        assert_eq!(
            trace.clipboard_text(),
            "TX transfer=raw framing=unrecognized bytes=2\nbytes=1B 40\nRX transfer=raw framing=unrecognized bytes=1\nbytes=06\nRX usb_bulk_in=empty observation=timeout_like\nbytes=<empty>"
        );
        assert!(entries[0].id() < entries[1].id());
    }

    #[test]
    fn debug_trace_reassembles_a_fragmented_epson_d4_entry_reply() {
        let mut trace = DebugTrafficTrace::new();
        trace.set_capture_enabled(true);

        trace.append(&TransportEvent::Rx(EPSON_D4_ENTRY_REPLY[..4].to_vec()));
        assert_eq!(trace.count(), 0);
        trace.append(&TransportEvent::Rx(EPSON_D4_ENTRY_REPLY[4..].to_vec()));

        let entry = trace.entries().next().unwrap();
        assert_eq!(entry.direction(), DebugTrafficDirection::Rx);
        assert_eq!(entry.hex_bytes(), "00 00 00 08 01 00 C5 00");
        assert_eq!(
            entry.summary(),
            "RX epson_d4_entry=reply result=recognized bytes=8"
        );
    }

    #[test]
    fn debug_trace_reassembles_fragmented_and_multiple_d4_packets() {
        let mut trace = DebugTrafficTrace::new();
        trace.set_capture_enabled(true);
        trace.append(&TransportEvent::Tx(EPSON_D4_ENTRY_COMMAND.to_vec()));

        let first = Packet::new(2, 2, b"di\x01\x00\x01".to_vec(), 1, 0)
            .unwrap()
            .encode();
        let second = Packet::new(2, 2, b"st\x01\x00\x01".to_vec(), 3, 1)
            .unwrap()
            .encode();
        trace.append(&TransportEvent::Tx(first[..4].to_vec()));
        assert_eq!(trace.count(), 1);
        let mut tail = first[4..].to_vec();
        tail.extend(&second);
        trace.append(&TransportEvent::Tx(tail));

        let entries = trace.entries().collect::<Vec<_>>();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[1].hex_bytes(), format_hex_bytes(&first));
        assert!(entries[1].summary().contains("command=di"));
        assert!(entries[1].summary().contains("payload_length=1"));
        assert!(entries[2].summary().contains("command=st"));
        assert!(entries[2].summary().contains("credit=3"));
        assert!(entries[2].summary().contains("control_bits=0b01"));
    }

    #[test]
    fn debug_trace_tracks_v20_and_v10_transaction_layouts() {
        let mut trace = DebugTrafficTrace::new();
        trace.set_capture_enabled(true);
        trace.append(&TransportEvent::Tx(EPSON_D4_ENTRY_COMMAND.to_vec()));

        let init_v20 = Packet::new(
            0,
            0,
            TransactionMessage::Init {
                revision: ProtocolRevision::V20,
            }
            .encode(ProtocolRevision::V20)
            .unwrap(),
            1,
            0,
        )
        .unwrap()
        .encode();
        let open_v20 = Packet::new(
            0,
            0,
            TransactionMessage::OpenChannel {
                peer_socket: 2,
                source_socket: 3,
                max_packet_size: 0x100,
                max_service_size: 0x200,
                max_credit: 4,
                initial_credit: None,
            }
            .encode(ProtocolRevision::V20)
            .unwrap(),
            1,
            0,
        )
        .unwrap()
        .encode();
        let init_v10 = Packet::new(
            0,
            0,
            TransactionMessage::Init {
                revision: ProtocolRevision::V10,
            }
            .encode(ProtocolRevision::V10)
            .unwrap(),
            1,
            0,
        )
        .unwrap()
        .encode();
        let open_v10 = Packet::new(
            0,
            0,
            TransactionMessage::OpenChannel {
                peer_socket: 4,
                source_socket: 5,
                max_packet_size: 0x40,
                max_service_size: 0x80,
                max_credit: 6,
                initial_credit: Some(7),
            }
            .encode(ProtocolRevision::V10)
            .unwrap(),
            1,
            0,
        )
        .unwrap()
        .encode();
        let mut transfer = init_v20;
        transfer.extend(open_v20);
        transfer.extend(init_v10);
        transfer.extend(open_v10);
        trace.append(&TransportEvent::Tx(transfer));

        let summaries = trace
            .entries()
            .map(|entry| entry.summary())
            .collect::<Vec<_>>();
        assert_eq!(summaries.len(), 5);
        assert!(summaries[1].contains("transaction=init"));
        assert!(summaries[1].contains("requested_revision=V20"));
        assert!(summaries[2].contains("revision=V20"));
        assert!(summaries[2].contains("max_packet_size=256"));
        assert!(summaries[3].contains("requested_revision=V10"));
        assert!(summaries[4].contains("revision=V10"));
        assert!(summaries[4].contains("initial_credit=7"));
    }

    #[test]
    fn debug_trace_decodes_l1300_factory_read_and_ascii_eeprom_reply() {
        let mut trace = DebugTrafficTrace::new();
        trace.set_capture_enabled(true);
        trace.append(&TransportEvent::Tx(EPSON_D4_ENTRY_COMMAND.to_vec()));
        let read = Packet::new(2, 2, b"||\x07\x002\x08A\xbe\xa0\x26\x00".to_vec(), 1, 0)
            .unwrap()
            .encode();
        trace.append(&TransportEvent::Tx(read));

        let read_summary = trace.entries().next_back().unwrap().summary();
        assert!(read_summary.contains("command=||"));
        assert!(read_summary.contains("factory_key=0x0832"));
        assert!(read_summary.contains("operation=A"));
        assert!(read_summary.contains("operation_complement=0xBE"));
        assert!(read_summary.contains("check=0xA0"));
        assert!(read_summary.contains("address_bytes=26 00"));
        assert!(read_summary.contains("address=0x0026"));

        let mut reply_trace = DebugTrafficTrace::new();
        reply_trace.set_capture_enabled(true);
        reply_trace.append(&TransportEvent::Rx(EPSON_D4_ENTRY_REPLY.to_vec()));
        let reply = Packet::new(2, 2, b"@BDC PS EE:002600;".to_vec(), 1, 0)
            .unwrap()
            .encode();
        reply_trace.append(&TransportEvent::Rx(reply));

        let reply_summary = reply_trace.entries().next_back().unwrap().summary();
        assert!(reply_summary.contains("request_response=response"));
        assert!(reply_summary.contains("response=ascii"));
        assert!(reply_summary.contains("eeprom_address=0x0026"));
        assert!(reply_summary.contains("eeprom_value=0x00"));
    }

    #[test]
    fn debug_trace_reports_factory_write_key_length_without_decoding_the_key() {
        let mut trace = DebugTrafficTrace::new();
        trace.set_capture_enabled(true);
        trace.append(&TransportEvent::Tx(EPSON_D4_ENTRY_COMMAND.to_vec()));
        let payload = b"||\x11\x002\x08B\xbd!\x26\x00\xAASENSITIVE";
        let packet = Packet::new(2, 2, payload.to_vec(), 1, 0).unwrap().encode();
        trace.append(&TransportEvent::Tx(packet));

        let summary = trace.entries().next_back().unwrap().summary();
        assert!(summary.contains("operation=B"));
        assert!(summary.contains("address_bytes=26 00"));
        assert!(summary.contains("value=0xAA"));
        assert!(summary.contains("write_key_length=9"));
        assert!(!summary.contains("SENSITIVE"));
    }

    #[test]
    fn debug_trace_keeps_malformed_and_unknown_transfers_as_raw() {
        let mut trace = DebugTrafficTrace::new();
        trace.set_capture_enabled(true);
        trace.append(&TransportEvent::Tx(EPSON_D4_ENTRY_COMMAND.to_vec()));

        trace.append(&TransportEvent::Tx(vec![0, 0, 0, 5, 0, 0]));
        let unknown = Packet::new(2, 2, [0xff], 1, 0).unwrap().encode();
        trace.append(&TransportEvent::Tx(unknown));

        let entries = trace.entries().collect::<Vec<_>>();
        assert_eq!(entries[1].hex_bytes(), "00 00 00 05 00 00");
        assert!(entries[1].summary().contains("transfer=raw"));
        assert!(entries[1].summary().contains("framing=unrecognized"));
        assert!(entries[2].summary().contains("payload=raw"));
    }

    #[test]
    fn empty_read_does_not_break_pending_packet_reassembly() {
        let mut trace = DebugTrafficTrace::new();
        trace.set_capture_enabled(true);
        trace.append(&TransportEvent::Rx(EPSON_D4_ENTRY_REPLY.to_vec()));
        let packet = Packet::new(2, 2, b"st\x01\x00\x01".to_vec(), 1, 0)
            .unwrap()
            .encode();

        trace.append(&TransportEvent::Rx(packet[..3].to_vec()));
        trace.append(&TransportEvent::Rx(Vec::new()));
        trace.append(&TransportEvent::Rx(packet[3..].to_vec()));

        let entries = trace.entries().collect::<Vec<_>>();
        assert_eq!(entries.len(), 3);
        assert_eq!(
            entries[1].summary(),
            "RX usb_bulk_in=empty observation=timeout_like"
        );
        assert_eq!(entries[2].hex_bytes(), format_hex_bytes(&packet));
    }

    #[test]
    fn debug_trace_accepts_events_only_after_opt_in() {
        let mut trace = DebugTrafficTrace::new();
        let event = TransportEvent::Tx(vec![0xaa]);

        assert!(!trace.append(&event));
        assert_eq!(trace.count(), 0);

        trace.set_capture_enabled(true);
        assert!(trace.append(&event));
        assert_eq!(trace.count(), 1);
    }

    #[test]
    fn sampled_operation_capture_survives_a_later_checkbox_toggle() {
        let mut trace = DebugTrafficTrace::new();
        trace.set_capture_enabled(true);
        trace.set_capture_enabled(false);

        assert_eq!(
            trace.append_captured_events(vec![TransportEvent::Tx(vec![0xaa])]),
            1
        );
        assert_eq!(trace.count(), 1);
    }

    #[test]
    fn debug_trace_begins_each_operation_with_fresh_decoder_state() {
        let mut trace = DebugTrafficTrace::new();
        trace.set_capture_enabled(true);
        trace.append(&TransportEvent::Tx(EPSON_D4_ENTRY_COMMAND.to_vec()));
        trace.append(&TransportEvent::Rx(EPSON_D4_ENTRY_REPLY[..4].to_vec()));
        trace.transaction_revision = ProtocolRevision::V10;

        trace.begin_operation();

        assert_eq!(trace.count(), 1);
        assert!(trace.tx_decoder.bytes.is_empty());
        assert!(!trace.tx_decoder.d4_active);
        assert!(trace.rx_decoder.bytes.is_empty());
        assert!(!trace.rx_decoder.d4_active);
        assert_eq!(trace.transaction_revision, ProtocolRevision::V20);

        trace.append(&TransportEvent::Tx(EPSON_D4_ENTRY_COMMAND.to_vec()));

        let entries = trace.entries().collect::<Vec<_>>();
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries[0].summary(),
            format!(
                "TX epson_d4_entry=command bytes={}",
                EPSON_D4_ENTRY_COMMAND.len()
            )
        );
        assert_eq!(
            entries[1].summary(),
            format!(
                "TX epson_d4_entry=command bytes={}",
                EPSON_D4_ENTRY_COMMAND.len()
            )
        );
    }

    #[test]
    fn debug_trace_clear_discards_partial_packets() {
        let mut trace = DebugTrafficTrace::new();
        trace.set_capture_enabled(true);
        trace.append(&TransportEvent::Tx(EPSON_D4_ENTRY_COMMAND.to_vec()));
        let packet = Packet::new(2, 2, b"st\x07\x00PRIVATE".to_vec(), 1, 0)
            .unwrap()
            .encode();
        trace.append(&TransportEvent::Tx(packet[..12].to_vec()));

        trace.clear();
        trace.append(&TransportEvent::Tx(packet[12..].to_vec()));

        let entries = trace.entries().collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].hex_bytes(), format_hex_bytes(&packet[12..]));
        assert!(entries[0].summary().contains("transfer=raw"));
        assert_ne!(entries[0].hex_bytes(), format_hex_bytes(&packet));
        assert!(
            !entries[0]
                .hex_bytes()
                .contains(&format_hex_bytes(&packet[..12]))
        );
    }

    #[test]
    fn debug_trace_evicts_the_oldest_entry_at_its_fixed_bound() {
        let mut trace = DebugTrafficTrace::new();
        trace.set_capture_enabled(true);
        for value in 0..=DEBUG_TRAFFIC_MAX_ENTRIES {
            trace.append(&TransportEvent::Tx(vec![(value % 256) as u8]));
        }

        assert_eq!(trace.count(), DEBUG_TRAFFIC_MAX_ENTRIES);
        assert_eq!(trace.entries().next().unwrap().hex_bytes(), "01");
        assert_eq!(trace.entries().next_back().unwrap().hex_bytes(), "E8");
    }

    #[test]
    fn fixture_selection_changes_the_resolved_model() {
        let mut state = GuiState::new().unwrap();
        assert_eq!(
            state.identity_resolution().resolved_model.as_deref(),
            Some("XP-352")
        );

        state.select_fixture(1);

        assert_eq!(state.selected_fixture_index(), 1);
        assert_eq!(
            state.identity_resolution().resolved_model.as_deref(),
            Some("C90")
        );
    }

    #[test]
    fn real_mode_is_the_default_and_does_not_enable_fixture_selection() {
        assert_eq!(SourceMode::default(), SourceMode::Real);
        assert!(!SourceMode::Real.fixtures_enabled());
        assert!(SourceMode::Fixtures.fixtures_enabled());
    }

    #[test]
    fn invalid_fixture_selection_preserves_the_current_fixture() {
        let mut state = GuiState::new().unwrap();
        state.select_fixture(FIXTURE_DEVICES.len());

        assert_eq!(state.selected_fixture_index(), 0);
    }

    #[test]
    fn navigation_reaches_each_fixture_only_view() {
        let mut state = GuiState::new().unwrap();
        assert_eq!(state.page(), Page::Status);

        state.navigate_to(Page::Eeprom);
        assert_eq!(state.page(), Page::Eeprom);
        state.navigate_to(Page::Tools);
        assert_eq!(state.page(), Page::Tools);
        state.navigate_to(Page::Status);
        assert_eq!(state.page(), Page::Status);
    }

    #[test]
    fn eeprom_selection_is_bounded_and_resets_with_fixture_changes() {
        let mut state = GuiState::new().unwrap();
        state.select_eeprom_row(2);

        assert_eq!(state.selected_eeprom_row().address, 0x000c);

        state.select_eeprom_row(3);
        assert_eq!(state.selected_eeprom_row_index(), 2);

        state.select_fixture(1);
        assert_eq!(state.selected_eeprom_row_index(), 0);
        assert_eq!(state.selected_eeprom_row().address, 0x0006);
    }

    #[test]
    fn fixture_eeprom_dump_contains_each_displayed_field_value() {
        for fixture in FIXTURE_DEVICES {
            assert_eq!(fixture.eeprom_bytes.len(), 256);
            for row in fixture.eeprom_rows {
                assert_eq!(fixture.eeprom_bytes[row.address as usize], row.value);
            }
        }
    }

    #[test]
    fn bundled_models_are_available_for_eeprom_file_interpretation() {
        let state = GuiState::new().unwrap();

        assert!(state.model_names().any(|model| model == "L1800"));
        assert!(
            state
                .model_spec("L1800")
                .is_some_and(|spec| !spec.memory_operations.is_empty())
        );
    }

    #[test]
    fn usb_model_hints_require_an_exact_vendor_and_product_match() {
        let database = ModelDatabase::from_toml(
            r#"
[[EPSON]]
models = ["Exact"]
idVendor = 0x04b8
idProduct = 0x1234

[[EPSON]]
models = ["Other product"]
idVendor = 0x04b8
idProduct = 0x5678
"#,
        )
        .unwrap();

        assert_eq!(
            model_hints_for_usb_candidate(&database, 0x04b8, 0x1234),
            ["Exact"]
        );
        assert_eq!(
            model_hints_for_usb_candidate(&database, 0x04b8, 0x5678),
            ["Other product"]
        );
        assert!(model_hints_for_usb_candidate(&database, 0x1234, 0x1234).is_empty());
    }

    #[test]
    fn fixture_report_order_contains_all_statuses() {
        let statuses = FIXTURE_DEVICES[0]
            .validation_report
            .iter()
            .map(|item| item.status)
            .collect::<Vec<_>>();

        assert_eq!(
            statuses,
            [
                ValidationStatus::Success,
                ValidationStatus::Success,
                ValidationStatus::Blocked,
                ValidationStatus::Failure,
            ]
        );
    }
}
