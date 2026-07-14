use std::sync::Mutex;

use reink_platform::{DeviceDiscovery, DiscoveredDevice, DiscoveryError, DiscoveryRequest};

/// A fixed discovery result that records every request it receives.
#[derive(Debug)]
pub struct StaticDiscovery {
    result: Result<Vec<DiscoveredDevice>, DiscoveryError>,
    requests: Mutex<Vec<DiscoveryRequest>>,
}

impl StaticDiscovery {
    pub fn success(devices: Vec<DiscoveredDevice>) -> Self {
        Self {
            result: Ok(devices),
            requests: Mutex::new(Vec::new()),
        }
    }

    pub fn failure(error: DiscoveryError) -> Self {
        Self {
            result: Err(error),
            requests: Mutex::new(Vec::new()),
        }
    }

    pub fn requests(&self) -> Vec<DiscoveryRequest> {
        self.requests
            .lock()
            .expect("scripted discovery request lock was poisoned")
            .clone()
    }
}

impl DeviceDiscovery for StaticDiscovery {
    fn discover(&self, request: DiscoveryRequest) -> Result<Vec<DiscoveredDevice>, DiscoveryError> {
        self.requests
            .lock()
            .expect("scripted discovery request lock was poisoned")
            .push(request);
        self.result.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use reink_platform::{DeviceDiscovery, DiscoveryError, DiscoveryRequest};

    use super::StaticDiscovery;

    #[test]
    fn failures_are_returned_and_requests_are_recorded() {
        let error = DiscoveryError::Unsupported { capability: "mDNS" };
        let discovery = StaticDiscovery::failure(error.clone());
        let request = DiscoveryRequest::new(Duration::from_secs(3));

        assert_eq!(discovery.discover(request).unwrap_err(), error);
        assert_eq!(discovery.requests(), vec![request]);
    }
}
