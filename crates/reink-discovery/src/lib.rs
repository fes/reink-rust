#![forbid(unsafe_code)]
//! mDNS discovery for printer services.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::time::Instant;

use mdns_sd::{ServiceDaemon, ServiceEvent};
use reink_platform::{
    DeviceDiscovery, DeviceLocation, DiscoveredDevice, DiscoveryError, DiscoveryRequest,
    PrinterIdentityHint,
};

const PRINTER_SERVICE_TYPES: [&str; 3] = [
    "_ipp._tcp.local.",
    "_ipps._tcp.local.",
    "_printer._tcp.local.",
];

#[cfg(target_os = "linux")]
mod device_file;

#[cfg(target_os = "linux")]
pub use device_file::LinuxDeviceFileDiscovery;

/// Browses standard printer mDNS service types.
#[derive(Clone, Debug, Default)]
pub struct MdnsDiscovery;

impl DeviceDiscovery for MdnsDiscovery {
    fn discover(&self, request: DiscoveryRequest) -> Result<Vec<DiscoveredDevice>, DiscoveryError> {
        let daemon = ServiceDaemon::new().map_err(|error| DiscoveryError::Failed {
            operation: "start mDNS",
            message: error.to_string(),
        })?;
        let receivers = PRINTER_SERVICE_TYPES
            .iter()
            .map(|service_type| {
                daemon
                    .browse(service_type)
                    .map_err(|error| DiscoveryError::Failed {
                        operation: "browse mDNS",
                        message: error.to_string(),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let deadline = Instant::now() + request.timeout;
        let mut devices = BTreeMap::new();

        while Instant::now() < deadline {
            let mut received_event = false;
            for receiver in &receivers {
                if let Ok(ServiceEvent::ServiceResolved(info)) = receiver.try_recv() {
                    received_event = true;
                    for (address, device) in devices_for_service(
                        info.get_fullname(),
                        info.get_port(),
                        info.get_addresses()
                            .iter()
                            .map(|address| address.to_ip_addr()),
                    ) {
                        devices.insert(address, device);
                    }
                }
            }
            if !received_event {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
        daemon.shutdown().map_err(|error| DiscoveryError::Failed {
            operation: "stop mDNS",
            message: error.to_string(),
        })?;
        Ok(devices.into_values().collect())
    }
}

fn devices_for_service(
    name: &str,
    port: u16,
    addresses: impl IntoIterator<Item = IpAddr>,
) -> Vec<(SocketAddr, DiscoveredDevice)> {
    addresses
        .into_iter()
        .map(|ip_address| {
            let address = SocketAddr::new(ip_address, port);
            (
                address,
                DiscoveredDevice {
                    location: DeviceLocation::Network { address },
                    identity_hint: PrinterIdentityHint::default(),
                    display_name: name.to_owned(),
                },
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use reink_platform::DeviceLocation;

    use super::devices_for_service;

    #[test]
    fn creates_a_network_device_for_each_resolved_address() {
        let addresses = [
            IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ];

        let devices = devices_for_service("Printer._ipp._tcp.local.", 631, addresses);

        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].1.display_name, "Printer._ipp._tcp.local.");
        assert!(matches!(
            devices[0].1.location,
            DeviceLocation::Network { .. }
        ));
    }
}
