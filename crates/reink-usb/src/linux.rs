use std::error::Error;
use std::fmt;
use std::time::Duration;

use reink_platform::{ByteTransport, TransportError, TransportErrorKind, UsbInterfaceSelector};
use rusb::{
    Context, Device, DeviceHandle, Direction, Recipient, RequestType, UsbContext, request_type,
};

use crate::{
    EndpointDescriptor, SelectedUsbInterface, UsbInterfaceDescriptor, select_printer_interface,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const GET_DEVICE_ID_REQUEST: u8 = 0;
const DEVICE_ID_BUFFER_CAPACITY: usize = 1024;

/// A claimed Linux USB printer interface backed by libusb.
pub struct LinuxUsbTransport {
    _context: Context,
    handle: DeviceHandle<Context>,
    interface: u8,
    input_endpoint: u8,
    output_endpoint: u8,
    input_packet_size: usize,
    timeout: Duration,
}

impl LinuxUsbTransport {
    /// Opens a selected inactive printer interface without modifying its driver.
    pub fn open(
        vendor_id: u16,
        product_id: u16,
        interface: UsbInterfaceSelector,
    ) -> Result<Self, UsbOpenError> {
        let context = Context::new().map_err(UsbOpenError::Context)?;
        let device = find_device(&context, vendor_id, product_id)?;
        Self::open_device(context, device, interface)
    }

    fn open_device(
        context: Context,
        device: Device<Context>,
        interface: UsbInterfaceSelector,
    ) -> Result<Self, UsbOpenError> {
        let selected = selected_interface(&device, interface)?;
        let mut handle = device.open().map_err(UsbOpenError::Open)?;
        ensure_kernel_driver_inactive(&mut handle, selected.number)?;
        handle
            .claim_interface(selected.number)
            .map_err(UsbOpenError::Claim)?;

        Ok(Self {
            _context: context,
            handle,
            interface: selected.number,
            input_endpoint: selected.input.address,
            output_endpoint: selected.output.address,
            input_packet_size: usize::from(selected.input.max_packet_size),
            timeout: DEFAULT_TIMEOUT,
        })
    }
}

/// Reads the standard USB Printer Class device identifier without a protocol session.
///
/// The selected interface must not have an active kernel driver. This function
/// never detaches, rebinds, or otherwise modifies a driver.
pub fn read_printer_device_id(
    vendor_id: u16,
    product_id: u16,
    interface: UsbInterfaceSelector,
) -> Result<Vec<u8>, UsbOpenError> {
    let context = Context::new().map_err(UsbOpenError::Context)?;
    let device = find_device(&context, vendor_id, product_id)?;
    let selected = selected_interface(&device, interface)?;
    let mut handle = device.open().map_err(UsbOpenError::Open)?;
    ensure_kernel_driver_inactive(&mut handle, selected.number)?;
    handle
        .claim_interface(selected.number)
        .map_err(UsbOpenError::Claim)?;

    let result = read_device_id_from_claimed_handle(&mut handle, selected.number);
    let release = handle
        .release_interface(selected.number)
        .map_err(UsbOpenError::Release);
    match result {
        Err(error) => Err(error),
        Ok(device_id) => release.map(|()| device_id),
    }
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
/// The selected interface must not have an active kernel driver. This function
/// never detaches, rebinds, or otherwise modifies a driver. It never returns
/// the collected reply bytes.
pub fn probe_bounded_exchange(
    vendor_id: u16,
    product_id: u16,
    interface: UsbInterfaceSelector,
    request: &[u8],
    expected_reply: &[u8],
    max_reads: usize,
) -> Result<BoundedExchangeProbeResult, UsbOpenError> {
    if expected_reply.is_empty() {
        return Err(UsbOpenError::EmptyExpectedReply);
    }
    let context = Context::new().map_err(UsbOpenError::Context)?;
    let device = find_device(&context, vendor_id, product_id)?;
    let selected = selected_interface(&device, interface)?;
    let mut handle = device.open().map_err(UsbOpenError::Open)?;
    ensure_kernel_driver_inactive(&mut handle, selected.number)?;
    handle
        .claim_interface(selected.number)
        .map_err(UsbOpenError::Claim)?;

    let result = probe_bounded_exchange_with_claimed_handle(
        &mut handle,
        &selected,
        request,
        expected_reply,
        max_reads,
    );
    let release = handle
        .release_interface(selected.number)
        .map_err(UsbOpenError::Release);
    match result {
        Err(error) => Err(error),
        Ok(outcome) => release.map(|()| outcome),
    }
}

fn probe_bounded_exchange_with_claimed_handle(
    handle: &mut DeviceHandle<Context>,
    selected: &SelectedUsbInterface,
    request: &[u8],
    expected_reply: &[u8],
    max_reads: usize,
) -> Result<BoundedExchangeProbeResult, UsbOpenError> {
    handle
        .write_bulk(selected.output.address, request, DEFAULT_TIMEOUT)
        .map_err(UsbOpenError::WriteExchange)?;
    read_bounded_reply(expected_reply, max_reads, |buffer| {
        handle.read_bulk(selected.input.address, buffer, DEFAULT_TIMEOUT)
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

impl ByteTransport for LinuxUsbTransport {
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

impl Drop for LinuxUsbTransport {
    fn drop(&mut self) {
        let _ = self.handle.release_interface(self.interface);
    }
}

fn find_device(
    context: &Context,
    vendor_id: u16,
    product_id: u16,
) -> Result<Device<Context>, UsbOpenError> {
    context
        .devices()
        .map_err(UsbOpenError::Context)?
        .iter()
        .find(|device| {
            device.device_descriptor().is_ok_and(|descriptor| {
                descriptor.vendor_id() == vendor_id && descriptor.product_id() == product_id
            })
        })
        .ok_or(UsbOpenError::DeviceNotFound {
            vendor_id,
            product_id,
        })
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
        .map(|interface| UsbInterfaceDescriptor {
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
        })
        .collect::<Vec<_>>();
    select_printer_interface(descriptor.class_code(), &interfaces)
        .filter(|selected| {
            selected.number == selector.number
                && selected.alternate_setting == selector.alternate_setting
        })
        .ok_or(UsbOpenError::InterfaceNotFound { selector })
}

trait KernelDriver {
    fn kernel_driver_active(&mut self, interface: u8) -> Result<bool, rusb::Error>;
}

impl KernelDriver for DeviceHandle<Context> {
    fn kernel_driver_active(&mut self, interface: u8) -> Result<bool, rusb::Error> {
        DeviceHandle::kernel_driver_active(self, interface)
    }
}

fn ensure_kernel_driver_inactive<H: KernelDriver>(
    handle: &mut H,
    interface: u8,
) -> Result<(), UsbOpenError> {
    match handle.kernel_driver_active(interface) {
        Ok(true) => Err(UsbOpenError::KernelDriverActive { interface }),
        Ok(false) | Err(rusb::Error::NotSupported) => Ok(()),
        Err(error) => Err(UsbOpenError::KernelDriverQuery(error)),
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
    DeviceNotFound {
        vendor_id: u16,
        product_id: u16,
    },
    Descriptor(rusb::Error),
    InterfaceNotFound {
        selector: UsbInterfaceSelector,
    },
    Open(rusb::Error),
    KernelDriverQuery(rusb::Error),
    KernelDriverActive {
        interface: u8,
    },
    Claim(rusb::Error),
    ReadDeviceId(rusb::Error),
    WriteExchange(rusb::Error),
    ReadExchange(rusb::Error),
    Release(rusb::Error),
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
            Self::DeviceNotFound {
                vendor_id,
                product_id,
            } => write!(
                formatter,
                "USB device {vendor_id:04x}:{product_id:04x} was not found"
            ),
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
                "USB interface {interface} is owned by an active kernel driver; ReInk will not detach it"
            ),
            Self::Claim(error) => write!(formatter, "claiming USB interface failed: {error}"),
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
            Self::Release(error) => write!(formatter, "releasing USB interface failed: {error}"),
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
        BoundedExchangeProbeResult, KernelDriver, UsbOpenError, ensure_kernel_driver_inactive,
        parse_device_id_response, read_bounded_reply,
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

    struct ActiveKernelDriver;

    impl KernelDriver for ActiveKernelDriver {
        fn kernel_driver_active(&mut self, _: u8) -> Result<bool, rusb::Error> {
            Ok(true)
        }
    }

    #[test]
    fn refuses_to_detach_an_active_kernel_driver() {
        let error = ensure_kernel_driver_inactive(&mut ActiveKernelDriver, 3).unwrap_err();

        assert!(matches!(
            error,
            UsbOpenError::KernelDriverActive { interface: 3 }
        ));
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
