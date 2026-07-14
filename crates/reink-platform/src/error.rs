use std::error::Error;
use std::fmt;

/// The class of failure reported by a concrete byte transport.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportErrorKind {
    Io,
    PermissionDenied,
    DeviceUnavailable,
    Timeout,
    Unsupported,
}

/// Contextual transport failure safe to display in a CLI or UI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransportError {
    pub kind: TransportErrorKind,
    pub operation: &'static str,
    pub message: String,
}

impl TransportError {
    pub fn new(
        kind: TransportErrorKind,
        operation: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            operation,
            message: message.into(),
        }
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} failed ({:?}): {}",
            self.operation, self.kind, self.message
        )
    }
}

impl Error for TransportError {}

#[cfg(test)]
mod tests {
    use super::{TransportError, TransportErrorKind};

    #[test]
    fn error_keeps_operation_and_category() {
        let error = TransportError::new(
            TransportErrorKind::PermissionDenied,
            "claim USB interface",
            "access denied",
        );

        assert_eq!(error.kind, TransportErrorKind::PermissionDenied);
        assert_eq!(error.operation, "claim USB interface");
        assert!(error.to_string().contains("access denied"));
    }
}
