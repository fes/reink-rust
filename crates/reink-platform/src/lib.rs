#![forbid(unsafe_code)]
//! OS-neutral contracts for ReInk transports and printer discovery.
//!
//! Concrete adapters own OS and third-party library details. Protocol and UI
//! crates use only the types and traits defined here.

mod control;
mod device;
mod discovery;
mod error;
mod transport;

pub use control::{ControlChannel, ControlError};
pub use device::{
    DeviceLocation, DiscoveredDevice, PrinterIdentityHint, UsbInterfaceSelector, UsbSelector,
};
pub use discovery::{DeviceDiscovery, DiscoveryError, DiscoveryRequest};
pub use error::{TransportError, TransportErrorKind};
pub use transport::ByteTransport;
