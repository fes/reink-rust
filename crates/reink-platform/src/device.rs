use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;

/// A selected printer location that a concrete adapter can open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceLocation {
    Usb(UsbSelector),
    DeviceFile(PathBuf),
    Network { address: SocketAddr },
}

/// Stable USB attributes used to select a printer interface.
///
/// The implementation may use additional backend-specific identifiers while a
/// device is open, but those identifiers must not be required by core code.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UsbSelector {
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
    pub interface: Option<UsbInterfaceSelector>,
}

/// USB interface identity needed to claim a specific printer interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsbInterfaceSelector {
    pub number: u8,
    pub alternate_setting: u8,
}

/// Metadata returned from discovery before an active transport is opened.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredDevice {
    pub location: DeviceLocation,
    pub identity_hint: PrinterIdentityHint,
    pub display_name: String,
}

/// Best-effort identity information from enumeration or service advertisements.
///
/// This is not authoritative printer identification. The protocol layer must
/// retrieve and parse the IEEE 1284 device ID before selecting a model.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PrinterIdentityHint {
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub serial_number: Option<String>,
    pub fields: BTreeMap<String, String>,
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv6Addr, SocketAddr};

    use super::{DeviceLocation, UsbInterfaceSelector, UsbSelector};

    #[test]
    fn network_locations_preserve_ipv6_addresses() {
        let address = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 161);
        let location = DeviceLocation::Network { address };

        assert_eq!(location, DeviceLocation::Network { address });
    }

    #[test]
    fn usb_selector_identifies_an_interface_without_os_paths() {
        let selector = UsbSelector {
            vendor_id: Some(0x04b8),
            interface: Some(UsbInterfaceSelector {
                number: 1,
                alternate_setting: 0,
            }),
            ..UsbSelector::default()
        };

        assert_eq!(selector.vendor_id, Some(0x04b8));
        assert_eq!(selector.interface.unwrap().alternate_setting, 0);
    }
}
