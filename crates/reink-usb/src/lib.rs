#![deny(unsafe_code)]
//! USB printer interface selection, libusb transport, and Windows stock-driver
//! transport.
//!
//! On Linux, operations temporarily hand off only the explicitly selected
//! interface. On macOS, libusb capture temporarily re-enumerates the explicitly
//! selected device. Both restore only a driver they detached.

mod descriptor;
mod native;
mod selection;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod adapter;
// Raw SetupAPI and overlapped-I/O calls are isolated here. Every unsafe block
// documents the Windows ownership or buffer invariant it relies on.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod windows_native;

/// Controls whether an active Linux or macOS kernel driver may be temporarily
/// handed off.
///
/// The default automatically detaches and restores the driver for an explicitly
/// selected target. macOS handoff is device-wide and may require root or an
/// Apple-granted entitlement. This has no driver-management effect on Windows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UsbDriverHandoff {
    /// Retain a restrictive policy for internal callers that must not detach.
    Refuse,
    /// Temporarily hand off an active Linux interface or macOS device and
    /// reattach it on close.
    #[default]
    TemporarilyDetach,
}

/// Observed lifecycle of an optional USB driver handoff.
///
/// `reattached` is `None` when this transport did not detach a driver. A
/// `Some(false)` value means reattachment was attempted and failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsbDriverHandoffOutcome {
    pub requested: bool,
    pub detached: bool,
    pub reattached: Option<bool>,
}

impl UsbDriverHandoffOutcome {
    pub const fn requested(policy: UsbDriverHandoff) -> Self {
        Self {
            requested: matches!(policy, UsbDriverHandoff::TemporarilyDetach),
            detached: false,
            reattached: None,
        }
    }

    #[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
    pub(crate) const fn with_detached(mut self, detached: bool) -> Self {
        self.detached = detached;
        self
    }

    #[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
    pub(crate) fn record_reattach(&mut self, succeeded: bool) {
        if self.detached {
            self.reattached = Some(succeeded);
        }
    }
}

pub use descriptor::{
    EndpointDescriptor, SelectedUsbInterface, USB_CLASS_PRINTER, UsbCandidateDeviceDescriptor,
    UsbInterfaceDescriptor, UsbPrinterCandidate, select_printer_candidates,
    select_printer_interface,
};
pub use native::{
    BackendCapabilities, NativeCandidateSelectionError, NativePrinterSelector, PrinterBackend,
    WindowsNativePrinterCandidate, parse_usb_hardware_id, redact_identity_serial_fields,
    select_native_candidate,
};
pub use selection::{
    UsbDeviceDescriptor, UsbDeviceLocation, UsbDeviceSelectionError, UsbDeviceSelector,
    select_usb_device,
};

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub use adapter::{
    BoundedExchangeProbeResult, ReadOnlyUsbTransport, UsbDriverState, UsbOpenError,
    inspect_usb_driver_state, list_printer_candidates, probe_bounded_exchange,
    probe_bounded_exchange_with_policy, read_printer_device_id, read_printer_device_id_with_policy,
};

#[cfg(target_os = "linux")]
pub type LinuxUsbTransport = ReadOnlyUsbTransport;

#[cfg(target_os = "macos")]
pub type MacOsUsbTransport = ReadOnlyUsbTransport;

#[cfg(target_os = "windows")]
pub use windows_native::{WindowsNativeTransport, list_windows_native_printer_candidates};

/// Compatibility alias for callers that expose only the type-restricted
/// read-only application session.
#[cfg(target_os = "windows")]
pub type WindowsNativeReadOnlyTransport = WindowsNativeTransport;

#[cfg(target_os = "windows")]
pub type WindowsUsbTransport = ReadOnlyUsbTransport;

#[cfg(test)]
mod tests {
    use super::{UsbDriverHandoff, UsbDriverHandoffOutcome};

    #[test]
    fn handoff_outcome_records_the_selected_policy() {
        assert_eq!(
            UsbDriverHandoffOutcome::requested(UsbDriverHandoff::Refuse),
            UsbDriverHandoffOutcome {
                requested: false,
                detached: false,
                reattached: None,
            }
        );
        assert_eq!(
            UsbDriverHandoffOutcome::requested(UsbDriverHandoff::TemporarilyDetach),
            UsbDriverHandoffOutcome {
                requested: true,
                detached: false,
                reattached: None,
            }
        );
    }

    #[test]
    fn default_handoff_is_automatic() {
        assert_eq!(
            UsbDriverHandoff::default(),
            UsbDriverHandoff::TemporarilyDetach
        );
    }

    #[test]
    fn handoff_outcome_records_only_a_driver_this_transport_detached() {
        let mut outcome = UsbDriverHandoffOutcome::requested(UsbDriverHandoff::TemporarilyDetach)
            .with_detached(true);
        outcome.record_reattach(true);
        assert_eq!(outcome.reattached, Some(true));

        let mut not_detached =
            UsbDriverHandoffOutcome::requested(UsbDriverHandoff::TemporarilyDetach);
        not_detached.record_reattach(false);
        assert_eq!(not_detached.reattached, None);
    }
}
