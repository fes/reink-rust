pub const USB_CLASS_PRINTER: u8 = 0x07;
const USB_ENDPOINT_DIRECTION_IN: u8 = 0x80;
const USB_TRANSFER_TYPE_MASK: u8 = 0x03;
const USB_TRANSFER_TYPE_BULK: u8 = 0x02;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EndpointDescriptor {
    pub address: u8,
    pub attributes: u8,
    pub max_packet_size: u16,
}

impl EndpointDescriptor {
    pub fn is_bulk(self) -> bool {
        self.attributes & USB_TRANSFER_TYPE_MASK == USB_TRANSFER_TYPE_BULK
    }

    pub fn is_in(self) -> bool {
        self.address & USB_ENDPOINT_DIRECTION_IN != 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsbInterfaceDescriptor {
    pub number: u8,
    pub alternate_setting: u8,
    pub class_code: u8,
    pub endpoints: Vec<EndpointDescriptor>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectedUsbInterface {
    pub number: u8,
    pub alternate_setting: u8,
    pub input: EndpointDescriptor,
    pub output: EndpointDescriptor,
}

/// Selects a printer-class interface with one bulk input and one bulk output.
///
/// Interface alternate setting zero matches the ReInkPy USB discovery policy.
pub fn select_printer_interface(
    device_class: u8,
    interfaces: &[UsbInterfaceDescriptor],
) -> Option<SelectedUsbInterface> {
    interfaces.iter().find_map(|interface| {
        if interface.alternate_setting != 0
            || (device_class != USB_CLASS_PRINTER && interface.class_code != USB_CLASS_PRINTER)
        {
            return None;
        }
        let input = interface
            .endpoints
            .iter()
            .copied()
            .find(|endpoint| endpoint.is_bulk() && endpoint.is_in())?;
        let output = interface
            .endpoints
            .iter()
            .copied()
            .find(|endpoint| endpoint.is_bulk() && !endpoint.is_in())?;
        Some(SelectedUsbInterface {
            number: interface.number,
            alternate_setting: interface.alternate_setting,
            input,
            output,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::{
        EndpointDescriptor, USB_CLASS_PRINTER, UsbInterfaceDescriptor, select_printer_interface,
    };

    fn endpoint(address: u8, attributes: u8) -> EndpointDescriptor {
        EndpointDescriptor {
            address,
            attributes,
            max_packet_size: 64,
        }
    }

    #[test]
    fn selects_bulk_endpoints_on_a_printer_interface() {
        let interfaces = [UsbInterfaceDescriptor {
            number: 1,
            alternate_setting: 0,
            class_code: USB_CLASS_PRINTER,
            endpoints: vec![endpoint(0x81, 2), endpoint(0x02, 2)],
        }];

        let selected = select_printer_interface(0, &interfaces).unwrap();
        assert_eq!(selected.number, 1);
        assert_eq!(selected.input.address, 0x81);
        assert_eq!(selected.output.address, 0x02);
    }

    #[test]
    fn rejects_non_printer_interfaces_and_missing_endpoint_directions() {
        let interfaces = [
            UsbInterfaceDescriptor {
                number: 1,
                alternate_setting: 0,
                class_code: 0xff,
                endpoints: vec![endpoint(0x81, 2), endpoint(0x02, 2)],
            },
            UsbInterfaceDescriptor {
                number: 2,
                alternate_setting: 0,
                class_code: USB_CLASS_PRINTER,
                endpoints: vec![endpoint(0x81, 2)],
            },
        ];

        assert!(select_printer_interface(0, &interfaces).is_none());
    }

    #[test]
    fn accepts_a_printer_device_class_and_rejects_nonzero_alternates() {
        let interfaces = [UsbInterfaceDescriptor {
            number: 1,
            alternate_setting: 1,
            class_code: 0xff,
            endpoints: vec![endpoint(0x81, 2), endpoint(0x02, 2)],
        }];

        assert!(select_printer_interface(USB_CLASS_PRINTER, &interfaces).is_none());
    }
}
