#![forbid(unsafe_code)]
//! USB printer interface selection and Linux libusb transport.
//!
//! This crate intentionally does not install, replace, detach, rebind, or
//! restore Windows drivers. Its concrete libusb transport is Linux-only.

mod descriptor;

#[cfg(target_os = "linux")]
mod linux;

pub use descriptor::{
    EndpointDescriptor, SelectedUsbInterface, USB_CLASS_PRINTER, UsbInterfaceDescriptor,
    select_printer_interface,
};

#[cfg(target_os = "linux")]
pub use linux::{
    D4EntryProbeResult, LinuxUsbTransport, UsbOpenError, probe_d4_entry, read_printer_device_id,
};
