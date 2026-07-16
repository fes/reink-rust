#![forbid(unsafe_code)]
//! USB printer interface selection and read-only libusb transport.
//!
//! By default this crate refuses active Linux kernel drivers. Explicit
//! maintenance APIs can temporarily detach and restore only the driver they
//! detached. Its concrete libusb transport is available on Linux and macOS.

mod descriptor;
mod selection;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod adapter;

/// Controls whether an active Linux kernel driver may be temporarily detached.
///
/// This policy is an explicit maintenance acknowledgement. It has no
/// driver-management effect on macOS or Windows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UsbDriverHandoff {
    /// Refuse to use an interface owned by an active Linux kernel driver.
    #[default]
    Refuse,
    /// Temporarily detach an active Linux kernel driver and reattach it on close.
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
pub use selection::{
    UsbDeviceDescriptor, UsbDeviceLocation, UsbDeviceSelectionError, UsbDeviceSelector,
    select_usb_device,
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use adapter::{
    BoundedExchangeProbeResult, ReadOnlyUsbTransport, UsbOpenError, list_printer_candidates,
    probe_bounded_exchange, probe_bounded_exchange_with_policy, read_printer_device_id,
    read_printer_device_id_with_policy,
};

#[cfg(target_os = "linux")]
pub type LinuxUsbTransport = ReadOnlyUsbTransport;

#[cfg(target_os = "macos")]
pub type MacOsUsbTransport = ReadOnlyUsbTransport;

#[cfg(test)]
mod tests {
    use super::{UsbDriverHandoff, UsbDriverHandoffOutcome};

    #[test]
    fn handoff_outcome_does_not_claim_a_detach_without_one() {
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
