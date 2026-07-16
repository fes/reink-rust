use std::error::Error;
use std::fmt;
use std::time::Duration;

use reink_platform::{ByteTransport, TransportError, TransportErrorKind, UsbInterfaceSelector};
use rusb::{
    Context, Device, DeviceHandle, Direction, Recipient, RequestType, UsbContext, request_type,
};

use crate::{
    EndpointDescriptor, SelectedUsbInterface, UsbCandidateDeviceDescriptor, UsbDeviceDescriptor,
    UsbDeviceLocation, UsbDeviceSelectionError, UsbDeviceSelector, UsbDriverHandoff,
    UsbDriverHandoffOutcome, UsbInterfaceDescriptor, UsbPrinterCandidate,
    select_printer_candidates, select_printer_interface, select_usb_device,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const GET_DEVICE_ID_REQUEST: u8 = 0;
const DEVICE_ID_BUFFER_CAPACITY: usize = 1024;

/// Lists USB printer candidates using libusb descriptors only.
///
/// This function does not open or claim a device, detach a driver, issue a USB
/// control request, or perform any printer protocol traffic.
pub fn list_printer_candidates() -> Result<Vec<UsbPrinterCandidate>, UsbOpenError> {
    let context = Context::new().map_err(UsbOpenError::Context)?;
    let devices = context.devices().map_err(UsbOpenError::Context)?;
    let descriptors = devices
        .iter()
        .map(candidate_device_descriptor)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(select_printer_candidates(&descriptors))
}

fn candidate_device_descriptor(
    device: Device<Context>,
) -> Result<UsbCandidateDeviceDescriptor, UsbOpenError> {
    let descriptor = device
        .device_descriptor()
        .map_err(UsbOpenError::Descriptor)?;
    let configuration = device
        .config_descriptor(0)
        .map_err(UsbOpenError::Descriptor)?;
    Ok(UsbCandidateDeviceDescriptor {
        vendor_id: descriptor.vendor_id(),
        product_id: descriptor.product_id(),
        bus_number: device.bus_number(),
        device_address: device.address(),
        device_class: descriptor.class_code(),
        interfaces: configuration
            .interfaces()
            .flat_map(|interface| interface.descriptors())
            .map(usb_interface_descriptor)
            .collect(),
    })
}

fn usb_interface_descriptor(interface: rusb::InterfaceDescriptor<'_>) -> UsbInterfaceDescriptor {
    UsbInterfaceDescriptor {
        number: interface.interface_number(),
        alternate_setting: interface.setting_number(),
        class_code: interface.class_code(),
        endpoints: interface
            .endpoint_descriptors()
            .map(|endpoint| EndpointDescriptor {
                address: endpoint.address(),
                attributes: match endpoint.transfer_type() {
                    rusb::TransferType::Control => 0,
                    rusb::TransferType::Isochronous => 1,
                    rusb::TransferType::Bulk => 2,
                    rusb::TransferType::Interrupt => 3,
                },
                max_packet_size: endpoint.max_packet_size(),
            })
            .collect(),
    }
}

/// A claimed read-only USB printer interface backed by libusb.
pub struct ReadOnlyUsbTransport {
    _context: Context,
    handle: DeviceHandle<Context>,
    interface: u8,
    input_endpoint: u8,
    output_endpoint: u8,
    input_packet_size: usize,
    timeout: Duration,
    claimed: bool,
    detached_kernel_driver: bool,
    driver_handoff: UsbDriverHandoffOutcome,
}

impl ReadOnlyUsbTransport {
    /// Opens a selected printer interface.
    ///
    /// On Linux, this automatically detaches an active driver for the selected
    /// interface and reattaches it on close. On macOS, libusb claim failure is
    /// returned without a driver workaround.
    pub fn open(
        device: UsbDeviceSelector,
        interface: UsbInterfaceSelector,
    ) -> Result<Self, UsbOpenError> {
        Self::open_with_policy(device, interface, UsbDriverHandoff::default())
    }

    /// Opens a selected printer interface with an explicit driver-handoff policy.
    ///
    /// On Linux, `TemporarilyDetach` detaches an active kernel driver only for
    /// this interface. Call [`Self::close`] to report release or reattach
    /// failures. On macOS the policy does not change normal claim behavior.
    pub fn open_with_policy(
        device: UsbDeviceSelector,
        interface: UsbInterfaceSelector,
        handoff: UsbDriverHandoff,
    ) -> Result<Self, UsbOpenError> {
        let context = Context::new().map_err(UsbOpenError::Context)?;
        let usb_device = find_device(&context, device)?;
        Self::open_device_with_policy(context, usb_device, interface, handoff)
    }

    fn open_device_with_policy(
        context: Context,
        device: Device<Context>,
        interface: UsbInterfaceSelector,
        handoff: UsbDriverHandoff,
    ) -> Result<Self, UsbOpenError> {
        let selected = selected_interface(&device, interface)?;
        let mut handle = device.open().map_err(UsbOpenError::Open)?;
        #[cfg(target_os = "linux")]
        let detached_kernel_driver =
            claim_interface_with_policy(&mut handle, selected.number, handoff, |handle| {
                handle.claim_interface(selected.number)
            })?;
        #[cfg(target_os = "macos")]
        let detached_kernel_driver = {
            let _ = handoff;
            handle
                .claim_interface(selected.number)
                .map_err(UsbOpenError::Claim)?;
            false
        };

        Ok(Self {
            _context: context,
            handle,
            interface: selected.number,
            input_endpoint: selected.input.address,
            output_endpoint: selected.output.address,
            input_packet_size: usize::from(selected.input.max_packet_size),
            timeout: DEFAULT_TIMEOUT,
            claimed: true,
            detached_kernel_driver,
            driver_handoff: UsbDriverHandoffOutcome::requested(handoff)
                .with_detached(detached_kernel_driver),
        })
    }

    /// Releases the claimed interface and reattaches a driver this transport detached.
    ///
    /// Both cleanup operations are attempted. This is the only cleanup API
    /// that reports failures; `Drop` is best effort only.
    pub fn close(&mut self) -> Result<(), UsbOpenError> {
        let release = if self.claimed {
            self.claimed = false;
            self.handle
                .release_interface(self.interface)
                .map_err(UsbOpenError::Release)
                .err()
        } else {
            None
        };
        #[cfg(target_os = "linux")]
        let reattach = if self.detached_kernel_driver {
            match self.handle.attach_kernel_driver(self.interface) {
                Ok(()) => {
                    self.detached_kernel_driver = false;
                    self.driver_handoff.record_reattach(true);
                    None
                }
                Err(error) => {
                    self.driver_handoff.record_reattach(false);
                    Some(UsbOpenError::ReattachKernelDriver(error))
                }
            }
        } else {
            None
        };
        #[cfg(target_os = "macos")]
        let reattach: Option<UsbOpenError> = None;

        match (release, reattach) {
            (None, None) => Ok(()),
            (Some(error), None) | (None, Some(error)) => Err(error),
            (Some(release), Some(reattach)) => Err(UsbOpenError::ReleaseAndReattach {
                release: Box::new(release),
                reattach: Box::new(reattach),
            }),
        }
    }

    /// Returns the actual driver-handoff lifecycle observed by this transport.
    pub const fn driver_handoff_outcome(&self) -> UsbDriverHandoffOutcome {
        self.driver_handoff
    }
}

/// Reads the standard USB Printer Class device identifier without a protocol session.
///
/// On Linux, an active driver for the explicitly selected interface is
/// temporarily detached and reattached before this operation returns.
pub fn read_printer_device_id(
    device: UsbDeviceSelector,
    interface: UsbInterfaceSelector,
) -> Result<Vec<u8>, UsbOpenError> {
    read_printer_device_id_with_policy(device, interface, UsbDriverHandoff::default())
}

/// Reads a printer device identifier with an explicit driver-handoff policy.
pub fn read_printer_device_id_with_policy(
    device: UsbDeviceSelector,
    interface: UsbInterfaceSelector,
    handoff: UsbDriverHandoff,
) -> Result<Vec<u8>, UsbOpenError> {
    let mut transport = ReadOnlyUsbTransport::open_with_policy(device, interface, handoff)?;
    let result = read_device_id_from_claimed_handle(&mut transport.handle, transport.interface);
    finish_operation(result, &mut transport)
}

/// Outcome of a bounded request/reply probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoundedExchangeProbeResult {
    /// The expected reply bytes were recognized.
    Recognized,
    /// The device replied, but its bytes did not contain the expected reply.
    Unrecognized { received_bytes: usize },
}

/// Sends a request and looks for an expected reply across at most `max_reads`.
///
/// On Linux, an active driver for the explicitly selected interface is
/// temporarily detached and reattached before this operation returns. This
/// function never returns collected reply bytes.
pub fn probe_bounded_exchange(
    device: UsbDeviceSelector,
    interface: UsbInterfaceSelector,
    request: &[u8],
    expected_reply: &[u8],
    max_reads: usize,
) -> Result<BoundedExchangeProbeResult, UsbOpenError> {
    probe_bounded_exchange_with_policy(
        device,
        interface,
        request,
        expected_reply,
        max_reads,
        UsbDriverHandoff::default(),
    )
}

/// Runs a bounded exchange with an explicit driver-handoff policy.
pub fn probe_bounded_exchange_with_policy(
    device: UsbDeviceSelector,
    interface: UsbInterfaceSelector,
    request: &[u8],
    expected_reply: &[u8],
    max_reads: usize,
    handoff: UsbDriverHandoff,
) -> Result<BoundedExchangeProbeResult, UsbOpenError> {
    if expected_reply.is_empty() {
        return Err(UsbOpenError::EmptyExpectedReply);
    }
    let mut transport = ReadOnlyUsbTransport::open_with_policy(device, interface, handoff)?;
    let result = probe_bounded_exchange_with_claimed_transport(
        &mut transport,
        request,
        expected_reply,
        max_reads,
    );
    finish_operation(result, &mut transport)
}

fn finish_operation<T>(
    result: Result<T, UsbOpenError>,
    transport: &mut ReadOnlyUsbTransport,
) -> Result<T, UsbOpenError> {
    match (result, transport.close()) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(operation), Ok(())) => Err(operation),
        (Ok(_), Err(close)) => Err(close),
        (Err(operation), Err(close)) => Err(UsbOpenError::OperationAndClose {
            operation: Box::new(operation),
            close: Box::new(close),
        }),
    }
}

fn probe_bounded_exchange_with_claimed_transport(
    transport: &mut ReadOnlyUsbTransport,
    request: &[u8],
    expected_reply: &[u8],
    max_reads: usize,
) -> Result<BoundedExchangeProbeResult, UsbOpenError> {
    transport
        .handle
        .write_bulk(transport.output_endpoint, request, DEFAULT_TIMEOUT)
        .map_err(UsbOpenError::WriteExchange)?;
    read_bounded_reply(expected_reply, max_reads, |buffer| {
        transport
            .handle
            .read_bulk(transport.input_endpoint, buffer, DEFAULT_TIMEOUT)
    })
    .map_err(UsbOpenError::ReadExchange)
}

fn read_bounded_reply(
    expected_reply: &[u8],
    max_reads: usize,
    mut read: impl FnMut(&mut [u8]) -> Result<usize, rusb::Error>,
) -> Result<BoundedExchangeProbeResult, rusb::Error> {
    let mut reply = Vec::new();
    let mut buffer = [0; 256];
    for _ in 0..max_reads {
        match read(&mut buffer) {
            Ok(count) => {
                if append_and_recognize(&mut reply, &buffer[..count], expected_reply) {
                    return Ok(BoundedExchangeProbeResult::Recognized);
                }
            }
            Err(rusb::Error::Timeout) => break,
            Err(error) => return Err(error),
        }
    }
    Ok(BoundedExchangeProbeResult::Unrecognized {
        received_bytes: reply.len(),
    })
}

fn append_and_recognize(reply: &mut Vec<u8>, fragment: &[u8], expected_reply: &[u8]) -> bool {
    reply.extend_from_slice(fragment);
    !expected_reply.is_empty()
        && reply
            .windows(expected_reply.len())
            .any(|window| window == expected_reply)
}

impl ByteTransport for ReadOnlyUsbTransport {
    fn write_all(&mut self, data: &[u8]) -> Result<(), TransportError> {
        self.handle
            .write_bulk(self.output_endpoint, data, self.timeout)
            .map_err(|error| transport_error("write USB bulk endpoint", error))?;
        Ok(())
    }

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, TransportError> {
        let buffer = if buffer.is_empty() {
            return Ok(0);
        } else {
            buffer
        };
        self.handle
            .read_bulk(self.input_endpoint, buffer, self.timeout)
            .map_err(|error| transport_error("read USB bulk endpoint", error))
    }

    fn description(&self) -> String {
        format!(
            "usb:interface={}:in={:#04x}:out={:#04x}:packet={}",
            self.interface, self.input_endpoint, self.output_endpoint, self.input_packet_size
        )
    }
}

impl Drop for ReadOnlyUsbTransport {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn find_device(
    context: &Context,
    selector: UsbDeviceSelector,
) -> Result<Device<Context>, UsbOpenError> {
    let devices = context.devices().map_err(UsbOpenError::Context)?;
    let descriptors = devices
        .iter()
        .filter_map(|device| {
            device
                .device_descriptor()
                .ok()
                .map(|descriptor| UsbDeviceDescriptor {
                    vendor_id: descriptor.vendor_id(),
                    product_id: descriptor.product_id(),
                    location: UsbDeviceLocation {
                        bus_number: device.bus_number(),
                        address: device.address(),
                    },
                })
        })
        .collect::<Vec<_>>();
    let selected =
        select_usb_device(&descriptors, selector).map_err(UsbOpenError::DeviceSelection)?;
    devices
        .iter()
        .find(|device| {
            device.bus_number() == selected.location.bus_number
                && device.address() == selected.location.address
        })
        .ok_or(UsbOpenError::DeviceSelection(
            UsbDeviceSelectionError::NotFound { selector },
        ))
}

fn selected_interface(
    device: &Device<Context>,
    selector: UsbInterfaceSelector,
) -> Result<SelectedUsbInterface, UsbOpenError> {
    let descriptor = device
        .device_descriptor()
        .map_err(UsbOpenError::Descriptor)?;
    let configuration = device
        .config_descriptor(0)
        .map_err(UsbOpenError::Descriptor)?;
    let interfaces = configuration
        .interfaces()
        .flat_map(|interface| interface.descriptors())
        .map(usb_interface_descriptor)
        .collect::<Vec<_>>();
    select_printer_interface(descriptor.class_code(), &interfaces)
        .filter(|selected| {
            selected.number == selector.number
                && selected.alternate_setting == selector.alternate_setting
        })
        .ok_or(UsbOpenError::InterfaceNotFound { selector })
}

#[cfg(target_os = "linux")]
trait KernelDriver {
    fn kernel_driver_active(&mut self, interface: u8) -> Result<bool, rusb::Error>;
    fn detach_kernel_driver(&mut self, interface: u8) -> Result<(), rusb::Error>;
    fn attach_kernel_driver(&mut self, interface: u8) -> Result<(), rusb::Error>;
}

#[cfg(target_os = "linux")]
impl KernelDriver for DeviceHandle<Context> {
    fn kernel_driver_active(&mut self, interface: u8) -> Result<bool, rusb::Error> {
        DeviceHandle::kernel_driver_active(self, interface)
    }

    fn detach_kernel_driver(&mut self, interface: u8) -> Result<(), rusb::Error> {
        DeviceHandle::detach_kernel_driver(self, interface)
    }

    fn attach_kernel_driver(&mut self, interface: u8) -> Result<(), rusb::Error> {
        DeviceHandle::attach_kernel_driver(self, interface)
    }
}

#[cfg(target_os = "linux")]
fn claim_interface_with_policy<H, F>(
    handle: &mut H,
    interface: u8,
    handoff: UsbDriverHandoff,
    claim: F,
) -> Result<bool, UsbOpenError>
where
    H: KernelDriver,
    F: FnOnce(&mut H) -> Result<(), rusb::Error>,
{
    let detached_kernel_driver = match handle.kernel_driver_active(interface) {
        Ok(true) if handoff == UsbDriverHandoff::Refuse => {
            return Err(UsbOpenError::KernelDriverActive { interface });
        }
        Ok(true) => {
            handle
                .detach_kernel_driver(interface)
                .map_err(UsbOpenError::DetachKernelDriver)?;
            true
        }
        Ok(false) | Err(rusb::Error::NotSupported) => false,
        Err(error) => return Err(UsbOpenError::KernelDriverQuery(error)),
    };
    match claim(handle) {
        Ok(()) => Ok(detached_kernel_driver),
        Err(claim) if detached_kernel_driver => match handle.attach_kernel_driver(interface) {
            Ok(()) => Err(UsbOpenError::Claim(claim)),
            Err(reattach) => Err(UsbOpenError::ClaimAndReattach { claim, reattach }),
        },
        Err(claim) => Err(UsbOpenError::Claim(claim)),
    }
}

fn read_device_id_from_claimed_handle(
    handle: &mut DeviceHandle<Context>,
    interface: u8,
) -> Result<Vec<u8>, UsbOpenError> {
    let mut response = [0; DEVICE_ID_BUFFER_CAPACITY];
    let response_length = handle
        .read_control(
            request_type(Direction::In, RequestType::Class, Recipient::Interface),
            GET_DEVICE_ID_REQUEST,
            0,
            u16::from(interface),
            &mut response,
            DEFAULT_TIMEOUT,
        )
        .map_err(UsbOpenError::ReadDeviceId)?;
    parse_device_id_response(&response[..response_length])
}

fn parse_device_id_response(response: &[u8]) -> Result<Vec<u8>, UsbOpenError> {
    let Some(length_bytes) = response.get(..2) else {
        return Err(UsbOpenError::InvalidDeviceIdLength {
            received: response.len(),
            declared: None,
        });
    };
    let declared = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
    if !(2..=response.len()).contains(&declared) {
        return Err(UsbOpenError::InvalidDeviceIdLength {
            received: response.len(),
            declared: Some(declared),
        });
    }
    Ok(response[2..declared].to_vec())
}

fn transport_error(operation: &'static str, error: rusb::Error) -> TransportError {
    let kind = match error {
        rusb::Error::Access => TransportErrorKind::PermissionDenied,
        rusb::Error::NoDevice => TransportErrorKind::DeviceUnavailable,
        rusb::Error::Timeout => TransportErrorKind::Timeout,
        rusb::Error::NotSupported => TransportErrorKind::Unsupported,
        _ => TransportErrorKind::Io,
    };
    TransportError::new(kind, operation, error.to_string())
}

#[derive(Debug)]
pub enum UsbOpenError {
    Context(rusb::Error),
    DeviceSelection(UsbDeviceSelectionError),
    Descriptor(rusb::Error),
    InterfaceNotFound {
        selector: UsbInterfaceSelector,
    },
    Open(rusb::Error),
    KernelDriverQuery(rusb::Error),
    KernelDriverActive {
        interface: u8,
    },
    DetachKernelDriver(rusb::Error),
    Claim(rusb::Error),
    ClaimAndReattach {
        claim: rusb::Error,
        reattach: rusb::Error,
    },
    ReadDeviceId(rusb::Error),
    WriteExchange(rusb::Error),
    ReadExchange(rusb::Error),
    Release(rusb::Error),
    ReattachKernelDriver(rusb::Error),
    ReleaseAndReattach {
        release: Box<UsbOpenError>,
        reattach: Box<UsbOpenError>,
    },
    OperationAndClose {
        operation: Box<UsbOpenError>,
        close: Box<UsbOpenError>,
    },
    EmptyExpectedReply,
    InvalidDeviceIdLength {
        received: usize,
        declared: Option<usize>,
    },
}

impl fmt::Display for UsbOpenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Context(error) => write!(formatter, "libusb context failed: {error}"),
            Self::DeviceSelection(UsbDeviceSelectionError::NotFound { selector }) => write!(
                formatter,
                "USB device {:04x}:{:04x} was not found",
                selector.vendor_id, selector.product_id
            ),
            Self::DeviceSelection(UsbDeviceSelectionError::Ambiguous { selector, matches }) => {
                write!(
                    formatter,
                    "USB device {:04x}:{:04x} matched {matches} devices; provide --bus-number and --device-address",
                    selector.vendor_id, selector.product_id
                )
            }
            Self::Descriptor(error) => write!(formatter, "USB descriptor read failed: {error}"),
            Self::InterfaceNotFound { selector } => write!(
                formatter,
                "USB printer interface was not found: {selector:?}"
            ),
            Self::Open(error) => write!(formatter, "opening USB device failed: {error}"),
            Self::KernelDriverQuery(error) => {
                write!(formatter, "querying USB kernel driver failed: {error}")
            }
            Self::KernelDriverActive { interface } => write!(
                formatter,
                "USB interface {interface} is owned by an active kernel driver and the selected restrictive policy will not detach it"
            ),
            Self::DetachKernelDriver(error) => {
                write!(
                    formatter,
                    "detaching USB kernel driver failed: {error}; reconnect the printer, power-cycle it if needed, then reboot the host before retrying"
                )
            }
            Self::Claim(error) => write!(
                formatter,
                "claiming USB interface failed: {error}; reconnect the printer, power-cycle it if needed, then reboot the host before retrying"
            ),
            Self::ClaimAndReattach { claim, reattach } => write!(
                formatter,
                "claiming USB interface failed: {claim}; reattaching the detached kernel driver also failed: {reattach}; reconnect the printer, power-cycle it if needed, then reboot the host before retrying"
            ),
            Self::ReadDeviceId(error) => {
                write!(formatter, "reading USB printer device ID failed: {error}")
            }
            Self::WriteExchange(error) => {
                write!(
                    formatter,
                    "writing bounded USB exchange request failed: {error}"
                )
            }
            Self::ReadExchange(error) => {
                write!(
                    formatter,
                    "reading bounded USB exchange reply failed: {error}"
                )
            }
            Self::Release(error) => write!(
                formatter,
                "releasing USB interface failed: {error}; reconnect the printer, power-cycle it if needed, then reboot the host before retrying"
            ),
            Self::ReattachKernelDriver(error) => {
                write!(
                    formatter,
                    "reattaching the detached USB kernel driver failed: {error}; reconnect the printer, power-cycle it if needed, then reboot the host before retrying"
                )
            }
            Self::ReleaseAndReattach { release, reattach } => {
                write!(formatter, "{release}; {reattach}")
            }
            Self::OperationAndClose { operation, close } => {
                write!(formatter, "{operation}; cleanup also failed: {close}")
            }
            Self::EmptyExpectedReply => {
                formatter.write_str("bounded USB exchange expected reply must not be empty")
            }
            Self::InvalidDeviceIdLength { received, declared } => write!(
                formatter,
                "invalid USB printer device-ID length: received {received} bytes, declared {declared:?}"
            ),
        }
    }
}

impl Error for UsbOpenError {}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::{
        BoundedExchangeProbeResult, UsbOpenError, parse_device_id_response, read_bounded_reply,
    };

    #[test]
    fn parses_a_standard_length_prefixed_device_id() {
        assert_eq!(
            parse_device_id_response(b"\x00\x12MFG:EPSON;MDL:X;").unwrap(),
            b"MFG:EPSON;MDL:X;"
        );
    }

    #[test]
    fn rejects_truncated_or_invalid_device_id_lengths() {
        assert!(matches!(
            parse_device_id_response(b"\x00"),
            Err(UsbOpenError::InvalidDeviceIdLength { declared: None, .. })
        ));
        assert!(matches!(
            parse_device_id_response(b"\x00\x10short"),
            Err(UsbOpenError::InvalidDeviceIdLength {
                declared: Some(16),
                ..
            })
        ));
    }

    #[cfg(target_os = "linux")]
    use super::{KernelDriver, UsbDriverHandoff, claim_interface_with_policy};

    #[cfg(target_os = "linux")]
    struct MockKernelDriver {
        detach_fails: bool,
        attach_fails: bool,
        events: Vec<&'static str>,
    }

    #[cfg(target_os = "linux")]
    impl MockKernelDriver {
        fn active() -> Self {
            Self {
                detach_fails: false,
                attach_fails: false,
                events: Vec::new(),
            }
        }
    }

    #[cfg(target_os = "linux")]
    impl KernelDriver for MockKernelDriver {
        fn kernel_driver_active(&mut self, _: u8) -> Result<bool, rusb::Error> {
            self.events.push("active");
            Ok(true)
        }

        fn detach_kernel_driver(&mut self, _: u8) -> Result<(), rusb::Error> {
            self.events.push("detach");
            if self.detach_fails {
                Err(rusb::Error::Access)
            } else {
                Ok(())
            }
        }

        fn attach_kernel_driver(&mut self, _: u8) -> Result<(), rusb::Error> {
            self.events.push("attach");
            if self.attach_fails {
                Err(rusb::Error::Access)
            } else {
                Ok(())
            }
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn restrictive_policy_refuses_an_active_kernel_driver() {
        let mut driver = MockKernelDriver::active();
        let error =
            claim_interface_with_policy(&mut driver, 3, UsbDriverHandoff::Refuse, |_| Ok(()))
                .unwrap_err();

        assert!(matches!(
            error,
            UsbOpenError::KernelDriverActive { interface: 3 }
        ));
        assert_eq!(driver.events, ["active"]);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn default_policy_detaches_before_claiming() {
        let mut driver = MockKernelDriver::active();
        let detached =
            claim_interface_with_policy(&mut driver, 3, UsbDriverHandoff::default(), |driver| {
                driver.events.push("claim");
                Ok(())
            })
            .unwrap();

        assert!(detached);
        assert_eq!(driver.events, ["active", "detach", "claim"]);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn claim_failure_reattaches_a_driver_detached_by_the_transport() {
        let mut driver = MockKernelDriver::active();
        let error = claim_interface_with_policy(
            &mut driver,
            3,
            UsbDriverHandoff::TemporarilyDetach,
            |driver| {
                driver.events.push("claim");
                Err(rusb::Error::Busy)
            },
        )
        .unwrap_err();

        assert!(matches!(error, UsbOpenError::Claim(rusb::Error::Busy)));
        assert_eq!(driver.events, ["active", "detach", "claim", "attach"]);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn claim_and_reattach_failures_are_both_reported() {
        let mut driver = MockKernelDriver {
            attach_fails: true,
            ..MockKernelDriver::active()
        };
        let error = claim_interface_with_policy(
            &mut driver,
            3,
            UsbDriverHandoff::TemporarilyDetach,
            |_| Err(rusb::Error::Busy),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            UsbOpenError::ClaimAndReattach {
                claim: rusb::Error::Busy,
                reattach: rusb::Error::Access,
            }
        ));
        assert_eq!(driver.events, ["active", "detach", "attach"]);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn detach_failure_does_not_attempt_claim_or_reattach() {
        let mut driver = MockKernelDriver {
            detach_fails: true,
            ..MockKernelDriver::active()
        };
        let error = claim_interface_with_policy(
            &mut driver,
            3,
            UsbDriverHandoff::TemporarilyDetach,
            |driver| {
                driver.events.push("claim");
                Ok(())
            },
        )
        .unwrap_err();

        assert!(matches!(
            error,
            UsbOpenError::DetachKernelDriver(rusb::Error::Access)
        ));
        assert_eq!(driver.events, ["active", "detach"]);
    }

    #[test]
    fn recognizes_an_expected_reply_across_fragments() {
        let mut fragments =
            VecDeque::from([b"prefix-\x01\x02".to_vec(), b"\x03\x04-suffix".to_vec()]);

        let result = read_bounded_reply(b"\x01\x02\x03\x04", 2, |buffer| {
            let fragment = fragments.pop_front().unwrap();
            buffer[..fragment.len()].copy_from_slice(&fragment);
            Ok(fragment.len())
        })
        .unwrap();

        assert_eq!(result, BoundedExchangeProbeResult::Recognized);
    }

    #[test]
    fn stops_after_the_configured_read_limit() {
        let mut fragments =
            VecDeque::from([b"no".to_vec(), b" match".to_vec(), b" expected".to_vec()]);

        let result = read_bounded_reply(b"expected", 2, |buffer| {
            let fragment = fragments.pop_front().unwrap();
            buffer[..fragment.len()].copy_from_slice(&fragment);
            Ok(fragment.len())
        })
        .unwrap();

        assert_eq!(
            result,
            BoundedExchangeProbeResult::Unrecognized { received_bytes: 8 }
        );
        assert_eq!(fragments.len(), 1);
    }
}
