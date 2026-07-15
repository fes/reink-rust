/// A physical USB location reported by libusb.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsbDeviceLocation {
    pub bus_number: u8,
    pub address: u8,
}

/// An explicit USB device selection.
///
/// Vendor and product IDs identify the printer model. A bus/address pair may
/// further select one physical device when several matching printers are
/// attached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsbDeviceSelector {
    pub vendor_id: u16,
    pub product_id: u16,
    pub location: Option<UsbDeviceLocation>,
}

impl UsbDeviceSelector {
    pub const fn new(vendor_id: u16, product_id: u16) -> Self {
        Self {
            vendor_id,
            product_id,
            location: None,
        }
    }

    pub const fn at_location(vendor_id: u16, product_id: u16, bus_number: u8, address: u8) -> Self {
        Self {
            vendor_id,
            product_id,
            location: Some(UsbDeviceLocation {
                bus_number,
                address,
            }),
        }
    }
}

/// Descriptor fields used to select an attached USB device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsbDeviceDescriptor {
    pub vendor_id: u16,
    pub product_id: u16,
    pub location: UsbDeviceLocation,
}

/// Failure to unambiguously select a USB device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsbDeviceSelectionError {
    NotFound {
        selector: UsbDeviceSelector,
    },
    Ambiguous {
        selector: UsbDeviceSelector,
        matches: usize,
    },
}

/// Selects one device without choosing an arbitrary matching printer.
pub fn select_usb_device(
    devices: &[UsbDeviceDescriptor],
    selector: UsbDeviceSelector,
) -> Result<UsbDeviceDescriptor, UsbDeviceSelectionError> {
    let matches = devices
        .iter()
        .copied()
        .filter(|device| {
            device.vendor_id == selector.vendor_id
                && device.product_id == selector.product_id
                && selector
                    .location
                    .is_none_or(|location| device.location == location)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(UsbDeviceSelectionError::NotFound { selector }),
        [device] => Ok(*device),
        _ => Err(UsbDeviceSelectionError::Ambiguous {
            selector,
            matches: matches.len(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        UsbDeviceDescriptor, UsbDeviceLocation, UsbDeviceSelectionError, UsbDeviceSelector,
        select_usb_device,
    };

    fn device(bus_number: u8, address: u8) -> UsbDeviceDescriptor {
        UsbDeviceDescriptor {
            vendor_id: 0x04b8,
            product_id: 0x1234,
            location: UsbDeviceLocation {
                bus_number,
                address,
            },
        }
    }

    #[test]
    fn requires_a_location_when_vendor_product_match_multiple_devices() {
        let error = select_usb_device(
            &[device(1, 2), device(1, 3)],
            UsbDeviceSelector::new(0x04b8, 0x1234),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            UsbDeviceSelectionError::Ambiguous { matches: 2, .. }
        ));
    }

    #[test]
    fn selects_the_explicit_bus_and_address() {
        assert_eq!(
            select_usb_device(
                &[device(1, 2), device(1, 3)],
                UsbDeviceSelector::at_location(0x04b8, 0x1234, 1, 3),
            )
            .unwrap(),
            device(1, 3)
        );
    }
}
