#![forbid(unsafe_code)]
//! USB printer interface selection and read-only libusb transport.
//!
//! This crate intentionally does not install, replace, detach, rebind, or
//! restore drivers. Its concrete libusb transport is available on Linux and
//! macOS only.

mod descriptor;
mod selection;

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod adapter;

pub use descriptor::{
    EndpointDescriptor, SelectedUsbInterface, USB_CLASS_PRINTER, UsbInterfaceDescriptor,
    select_printer_interface,
};
pub use selection::{
    UsbDeviceDescriptor, UsbDeviceLocation, UsbDeviceSelectionError, UsbDeviceSelector,
    select_usb_device,
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub use adapter::{
    BoundedExchangeProbeResult, ReadOnlyUsbTransport, UsbOpenError, probe_bounded_exchange,
    read_printer_device_id,
};

#[cfg(target_os = "linux")]
pub type LinuxUsbTransport = ReadOnlyUsbTransport;

#[cfg(target_os = "macos")]
pub type MacOsUsbTransport = ReadOnlyUsbTransport;
