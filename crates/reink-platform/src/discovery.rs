use std::error::Error;
use std::fmt;
use std::time::Duration;

use crate::DiscoveredDevice;

/// Discovery options shared by USB, mDNS, and device-file adapters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiscoveryRequest {
    pub timeout: Duration,
}

impl DiscoveryRequest {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

/// Lists devices using one platform or network discovery mechanism.
pub trait DeviceDiscovery: Send + Sync {
    fn discover(&self, request: DiscoveryRequest) -> Result<Vec<DiscoveredDevice>, DiscoveryError>;
}

/// A discovery operation failed before producing a complete device list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiscoveryError {
    Unsupported {
        capability: &'static str,
    },
    Failed {
        operation: &'static str,
        message: String,
    },
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { capability } => {
                write!(formatter, "unsupported capability: {capability}")
            }
            Self::Failed { operation, message } => {
                write!(formatter, "discovery {operation} failed: {message}")
            }
        }
    }
}

impl Error for DiscoveryError {}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::DiscoveryRequest;

    #[test]
    fn request_retains_the_callers_timeout() {
        let request = DiscoveryRequest::new(Duration::from_secs(5));

        assert_eq!(request.timeout, Duration::from_secs(5));
    }
}
