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

/// One successful byte transfer observed by a [`RecordingTransport`].
///
/// Events retain the order and read boundaries exposed by the wrapped
/// transport. Failed I/O is deliberately not represented as a byte event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransportEvent {
    Tx(Vec<u8>),
    Rx(Vec<u8>),
}

/// A transparent [`ByteTransport`] wrapper that records successful transfers.
///
/// This wrapper is platform-neutral and records only what the transport
/// contract exposes: complete successful writes and the bytes returned from
/// every successful read call.
#[derive(Debug)]
pub struct RecordingTransport<T> {
    inner: T,
    events: Vec<TransportEvent>,
}

impl<T> RecordingTransport<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            events: Vec::new(),
        }
    }

    /// Returns the wrapped transport and the ordered transfer record.
    pub fn into_parts(self) -> (T, Vec<TransportEvent>) {
        (self.inner, self.events)
    }
}

impl<T: ByteTransport> ByteTransport for RecordingTransport<T> {
    fn write_all(&mut self, data: &[u8]) -> Result<(), TransportError> {
        self.inner.write_all(data)?;
        self.events.push(TransportEvent::Tx(data.to_vec()));
        Ok(())
    }

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, TransportError> {
        let count = self.inner.read(buffer)?;
        self.events
            .push(TransportEvent::Rx(buffer[..count].to_vec()));
        Ok(count)
    }

    fn description(&self) -> String {
        self.inner.description()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use crate::TransportErrorKind;

    use super::{ByteTransport, RecordingTransport, TransportError, TransportEvent};

    struct ScriptedTransport {
        writes: VecDeque<Result<(), TransportError>>,
        reads: VecDeque<Result<Vec<u8>, TransportError>>,
    }

    impl ScriptedTransport {
        fn new(
            writes: impl IntoIterator<Item = Result<(), TransportError>>,
            reads: impl IntoIterator<Item = Result<Vec<u8>, TransportError>>,
        ) -> Self {
            Self {
                writes: writes.into_iter().collect(),
                reads: reads.into_iter().collect(),
            }
        }
    }

    impl ByteTransport for ScriptedTransport {
        fn write_all(&mut self, _: &[u8]) -> Result<(), TransportError> {
            self.writes.pop_front().expect("scripted write outcome")
        }

        fn read(&mut self, buffer: &mut [u8]) -> Result<usize, TransportError> {
            let data = self.reads.pop_front().expect("scripted read outcome")?;
            assert!(data.len() <= buffer.len());
            buffer[..data.len()].copy_from_slice(&data);
            Ok(data.len())
        }

        fn description(&self) -> String {
            "scripted transport".to_owned()
        }
    }

    #[test]
    fn records_writes_and_fragmented_reads_in_transport_order() {
        let mut transport = RecordingTransport::new(ScriptedTransport::new(
            [Ok(()), Ok(())],
            [Ok(vec![0x10]), Ok(vec![0x20, 0x30])],
        ));
        let mut first = [0; 1];
        let mut second = [0; 2];

        transport.write_all(&[0xaa]).unwrap();
        assert_eq!(transport.read(&mut first).unwrap(), 1);
        transport.write_all(&[0xbb, 0xcc]).unwrap();
        assert_eq!(transport.read(&mut second).unwrap(), 2);

        let (_, events) = transport.into_parts();
        assert_eq!(
            events,
            vec![
                TransportEvent::Tx(vec![0xaa]),
                TransportEvent::Rx(vec![0x10]),
                TransportEvent::Tx(vec![0xbb, 0xcc]),
                TransportEvent::Rx(vec![0x20, 0x30]),
            ]
        );
    }

    #[test]
    fn excludes_failed_io_from_the_record() {
        let write_error = TransportError::new(TransportErrorKind::Timeout, "write", "timed out");
        let read_error = TransportError::new(TransportErrorKind::Timeout, "read", "timed out");
        let mut transport = RecordingTransport::new(ScriptedTransport::new(
            [Err(write_error.clone())],
            [Err(read_error.clone())],
        ));
        let mut buffer = [0; 1];

        assert_eq!(transport.write_all(&[0xaa]).unwrap_err(), write_error);
        assert_eq!(transport.read(&mut buffer).unwrap_err(), read_error);

        let (_, events) = transport.into_parts();
        assert!(events.is_empty());
    }
}
