use crate::TransportError;

/// A blocking, bidirectional byte stream to a selected printer interface.
///
/// Implementations acquire their OS resource in their constructor and release
/// it in `Drop`. In particular, a USB implementation must reattach a kernel
/// driver only when that instance detached it.
pub trait ByteTransport: Send {
    /// Writes every byte in `data` or returns a contextual transport error.
    fn write_all(&mut self, data: &[u8]) -> Result<(), TransportError>;

    /// Reads up to `buffer.len()` bytes.
    ///
    /// A return value of `Ok(0)` is reserved for a closed stream. Timeout
    /// behavior must be reported as `TransportErrorKind::Timeout`.
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, TransportError>;

    /// Returns a non-secret description suitable for diagnostics.
    fn description(&self) -> String;
}
