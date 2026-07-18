use std::fmt;
use std::sync::Arc;

/// The concrete device-access surface used by a printer candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrinterBackend {
    LibUsb,
    /// Direct `ReadFile`/`WriteFile` access to a present USBPRINT interface.
    WindowsNativeStockDriver,
}

/// Application capabilities attached to a concrete backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackendCapabilities {
    pub d4_read: bool,
    pub usb_device_id: bool,
    pub persistent_mutation: bool,
    /// Mutation is available only through an explicitly named, unvalidated
    /// higher-level API. It must never be presented as validated parity.
    pub experimental_mutation: bool,
}

impl PrinterBackend {
    pub const fn capabilities(self) -> BackendCapabilities {
        match self {
            Self::LibUsb => BackendCapabilities {
                d4_read: true,
                usb_device_id: true,
                persistent_mutation: true,
                experimental_mutation: false,
            },
            Self::WindowsNativeStockDriver => BackendCapabilities {
                d4_read: true,
                usb_device_id: false,
                persistent_mutation: true,
                experimental_mutation: true,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ParsedUsbHardwareId {
    pub vendor_id: u16,
    pub product_id: u16,
    pub interface_number: Option<u8>,
}

/// Parses documented USB hardware-ID components without inspecting an
/// interface path. The input is never retained or included in an error.
pub fn parse_usb_hardware_id(value: &str) -> Option<(u16, u16, Option<u8>)> {
    let parsed = parse_usb_hardware_id_inner(value)?;
    Some((parsed.vendor_id, parsed.product_id, parsed.interface_number))
}

/// Replaces serial values in an ASCII IEEE 1284-style field without changing
/// byte length. This is suitable for UI/report buffers, not protocol parsing.
pub fn redact_identity_serial_fields(bytes: &mut [u8]) {
    for marker in [b"SN:".as_slice(), b"SERIALNUMBER:".as_slice()] {
        let mut offset = 0;
        while let Some(relative) = bytes[offset..]
            .windows(marker.len())
            .position(|window| window.eq_ignore_ascii_case(marker))
        {
            let value_start = offset + relative + marker.len();
            let value_end = bytes[value_start..]
                .iter()
                .position(|byte| *byte == b';')
                .map_or(bytes.len(), |relative_end| value_start + relative_end);
            bytes[value_start..value_end].fill(b'X');
            offset = value_end.saturating_add(1);
            if offset >= bytes.len() {
                break;
            }
        }
    }
}

pub(crate) fn parse_usb_hardware_id_inner(value: &str) -> Option<ParsedUsbHardwareId> {
    let mut vendor_id = None;
    let mut product_id = None;
    let mut interface_number = None;
    for component in value.split(['\\', '&']) {
        let component = component.trim();
        let Some(prefix) = component.as_bytes().get(..component.len().min(4)) else {
            continue;
        };
        if component.len() == 8 && prefix.eq_ignore_ascii_case(b"VID_") {
            vendor_id = component
                .get(4..)
                .and_then(|value| u16::from_str_radix(value, 16).ok());
        } else if component.len() == 8 && prefix.eq_ignore_ascii_case(b"PID_") {
            product_id = component
                .get(4..)
                .and_then(|value| u16::from_str_radix(value, 16).ok());
        } else if component.len() == 5
            && component
                .as_bytes()
                .get(..3)
                .is_some_and(|value| value.eq_ignore_ascii_case(b"MI_"))
        {
            interface_number = component
                .get(3..)
                .and_then(|value| u8::from_str_radix(value, 16).ok());
        }
    }
    Some(ParsedUsbHardwareId {
        vendor_id: vendor_id?,
        product_id: product_id?,
        interface_number,
    })
}

#[derive(Eq, PartialEq)]
pub(crate) struct NativeCandidateToken {
    #[cfg(target_os = "windows")]
    pub(crate) device_path: Vec<u16>,
}

/// A present Windows USBPRINT interface with a redacted, process-local open token.
///
/// Debug and display output deliberately omit the opaque interface path and all
/// device-instance data.
#[derive(Clone, Eq, PartialEq)]
pub struct WindowsNativePrinterCandidate {
    pub vendor_id: u16,
    pub product_id: u16,
    pub interface_number: Option<u8>,
    pub(crate) token: Arc<NativeCandidateToken>,
}

impl WindowsNativePrinterCandidate {
    pub const fn backend(&self) -> PrinterBackend {
        PrinterBackend::WindowsNativeStockDriver
    }

    pub const fn capabilities(&self) -> BackendCapabilities {
        self.backend().capabilities()
    }

    #[cfg(test)]
    fn fixture(vendor_id: u16, product_id: u16, interface_number: Option<u8>) -> Self {
        Self {
            vendor_id,
            product_id,
            interface_number,
            token: Arc::new(NativeCandidateToken {
                #[cfg(target_os = "windows")]
                device_path: Vec::new(),
            }),
        }
    }
}

impl fmt::Debug for WindowsNativePrinterCandidate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WindowsNativePrinterCandidate")
            .field("vendor_id", &format_args!("{:04x}", self.vendor_id))
            .field("product_id", &format_args!("{:04x}", self.product_id))
            .field("interface_number", &self.interface_number)
            .field("token", &"<redacted process-local token>")
            .finish()
    }
}

/// Generic, non-secret selector for one native candidate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativePrinterSelector {
    pub vendor_id: u16,
    pub product_id: u16,
    pub interface_number: Option<u8>,
}

impl NativePrinterSelector {
    pub const fn new(vendor_id: u16, product_id: u16) -> Self {
        Self {
            vendor_id,
            product_id,
            interface_number: None,
        }
    }

    pub const fn with_interface(vendor_id: u16, product_id: u16, interface_number: u8) -> Self {
        Self {
            vendor_id,
            product_id,
            interface_number: Some(interface_number),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCandidateSelectionError {
    NotFound {
        selector: NativePrinterSelector,
    },
    Ambiguous {
        selector: NativePrinterSelector,
        matches: usize,
    },
}

impl fmt::Display for NativeCandidateSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { selector } => write!(
                formatter,
                "no Windows stock-driver USBPRINT candidate matched {:04x}:{:04x}{}",
                selector.vendor_id,
                selector.product_id,
                selector
                    .interface_number
                    .map(|value| format!(" interface {value}"))
                    .unwrap_or_default()
            ),
            Self::Ambiguous { selector, matches } => write!(
                formatter,
                "Windows stock-driver USBPRINT selector {:04x}:{:04x}{} matched {matches} candidates; disconnect unrelated duplicates or provide an interface number",
                selector.vendor_id,
                selector.product_id,
                selector
                    .interface_number
                    .map(|value| format!(" interface {value}"))
                    .unwrap_or_default()
            ),
        }
    }
}

impl std::error::Error for NativeCandidateSelectionError {}

/// Selects exactly one native candidate and never chooses an arbitrary match.
pub fn select_native_candidate(
    candidates: &[WindowsNativePrinterCandidate],
    selector: NativePrinterSelector,
) -> Result<WindowsNativePrinterCandidate, NativeCandidateSelectionError> {
    let matches = candidates
        .iter()
        .filter(|candidate| {
            candidate.vendor_id == selector.vendor_id
                && candidate.product_id == selector.product_id
                && selector
                    .interface_number
                    .is_none_or(|number| candidate.interface_number == Some(number))
        })
        .cloned()
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(NativeCandidateSelectionError::NotFound { selector }),
        [candidate] => Ok(candidate.clone()),
        _ => Err(NativeCandidateSelectionError::Ambiguous {
            selector,
            matches: matches.len(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NativeCandidateSelectionError, NativePrinterSelector, PrinterBackend,
        WindowsNativePrinterCandidate, parse_usb_hardware_id, redact_identity_serial_fields,
        select_native_candidate,
    };

    #[test]
    fn parses_documented_usb_hardware_id_components_case_insensitively() {
        assert_eq!(
            parse_usb_hardware_id(r"USB\VID_04B8&PID_1234&MI_02"),
            Some((0x04b8, 0x1234, Some(2)))
        );
        assert_eq!(
            parse_usb_hardware_id(r"usb\vid_04b8&pid_abcd"),
            Some((0x04b8, 0xabcd, None))
        );
        assert_eq!(parse_usb_hardware_id(r"USBPRINT\GENERIC"), None);
    }

    #[test]
    fn native_candidate_debug_redacts_its_private_token() {
        let candidate = WindowsNativePrinterCandidate::fixture(0x04b8, 0x1234, Some(0));
        let debug = format!("{candidate:?}");
        assert!(debug.contains("<redacted process-local token>"));
        assert!(!debug.contains("device_path"));
    }

    #[test]
    fn redacts_identity_serial_fields_without_changing_length() {
        let mut bytes = b"MFG:EPSON;SN:PRIVATE;MDL:C90;".to_vec();
        let length = bytes.len();
        redact_identity_serial_fields(&mut bytes);
        assert_eq!(bytes, b"MFG:EPSON;SN:XXXXXXX;MDL:C90;");
        assert_eq!(bytes.len(), length);
    }

    #[test]
    fn native_selection_requires_an_unambiguous_match() {
        let candidates = [
            WindowsNativePrinterCandidate::fixture(0x04b8, 0x1234, Some(0)),
            WindowsNativePrinterCandidate::fixture(0x04b8, 0x1234, Some(1)),
        ];
        assert!(matches!(
            select_native_candidate(&candidates, NativePrinterSelector::new(0x04b8, 0x1234)),
            Err(NativeCandidateSelectionError::Ambiguous { matches: 2, .. })
        ));
        assert_eq!(
            select_native_candidate(
                &candidates,
                NativePrinterSelector::with_interface(0x04b8, 0x1234, 1)
            )
            .unwrap()
            .interface_number,
            Some(1)
        );
    }

    #[test]
    fn native_backend_capabilities_are_experimental_and_have_no_usb_id() {
        let capabilities = PrinterBackend::WindowsNativeStockDriver.capabilities();
        assert!(capabilities.d4_read);
        assert!(!capabilities.usb_device_id);
        assert!(capabilities.persistent_mutation);
        assert!(capabilities.experimental_mutation);
        assert!(PrinterBackend::LibUsb.capabilities().persistent_mutation);
        assert!(!PrinterBackend::LibUsb.capabilities().experimental_mutation);
    }
}
