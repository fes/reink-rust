use std::collections::VecDeque;

use reink_platform::{ByteTransport, TransportError, TransportErrorKind};

/// A scripted outcome for one transport write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WriteStep {
    Expect(Vec<u8>),
    Error(TransportError),
}

/// A scripted outcome for one transport read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadStep {
    Data(Vec<u8>),
    Error(TransportError),
    Eof,
}

/// A deterministic [`ByteTransport`] for protocol tests.
///
/// A `Data` step may be longer than the caller's buffer. The remainder is
/// retained and returned from subsequent reads, making fragmentation explicit.
#[derive(Debug)]
pub struct ScriptedTransport {
    description: String,
    writes: VecDeque<WriteStep>,
    reads: VecDeque<ReadStep>,
    written: Vec<Vec<u8>>,
}

impl ScriptedTransport {
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            writes: VecDeque::new(),
            reads: VecDeque::new(),
            written: Vec::new(),
        }
    }

    pub fn expect_write(&mut self, data: impl Into<Vec<u8>>) {
        self.writes.push_back(WriteStep::Expect(data.into()));
    }

    pub fn fail_write(&mut self, error: TransportError) {
        self.writes.push_back(WriteStep::Error(error));
    }

    pub fn push_read_data(&mut self, data: impl Into<Vec<u8>>) {
        self.reads.push_back(ReadStep::Data(data.into()));
    }

    pub fn fail_read(&mut self, error: TransportError) {
        self.reads.push_back(ReadStep::Error(error));
    }

    pub fn push_eof(&mut self) {
        self.reads.push_back(ReadStep::Eof);
    }

    pub fn written(&self) -> &[Vec<u8>] {
        &self.written
    }

    /// Panics when the test failed to consume its complete script.
    pub fn assert_finished(&self) {
        assert!(
            self.writes.is_empty(),
            "{} expected write step(s) were not consumed: {:?}",
            self.writes.len(),
            self.writes
        );
        assert!(
            self.reads.is_empty(),
            "{} expected read step(s) were not consumed: {:?}",
            self.reads.len(),
            self.reads
        );
    }

    fn unexpected_write(&self, actual: &[u8], expected: Option<&WriteStep>) -> TransportError {
        TransportError::new(
            TransportErrorKind::Io,
            "scripted write",
            format!("unexpected write {actual:02X?}; expected {expected:?}"),
        )
    }
}

impl ByteTransport for ScriptedTransport {
    fn write_all(&mut self, data: &[u8]) -> Result<(), TransportError> {
        self.written.push(data.to_vec());

        match self.writes.pop_front() {
            Some(WriteStep::Expect(expected)) if expected == data => Ok(()),
            Some(step @ WriteStep::Expect(_)) => Err(self.unexpected_write(data, Some(&step))),
            Some(WriteStep::Error(error)) => Err(error),
            None => Err(self.unexpected_write(data, None)),
        }
    }

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, TransportError> {
        if buffer.is_empty() {
            return Ok(0);
        }

        match self.reads.pop_front() {
            Some(ReadStep::Data(mut data)) => {
                let read_len = data.len().min(buffer.len());
                buffer[..read_len].copy_from_slice(&data[..read_len]);
                data.drain(..read_len);
                if !data.is_empty() {
                    self.reads.push_front(ReadStep::Data(data));
                }
                Ok(read_len)
            }
            Some(ReadStep::Error(error)) => Err(error),
            Some(ReadStep::Eof) | None => Ok(0),
        }
    }

    fn description(&self) -> String {
        self.description.clone()
    }
}

#[cfg(test)]
mod tests {
    use reink_platform::{ByteTransport, TransportError, TransportErrorKind};

    use super::ScriptedTransport;

    #[test]
    fn fragmented_reads_preserve_byte_order() {
        let mut transport = ScriptedTransport::new("scripted");
        transport.push_read_data([0x10, 0x20, 0x30]);

        let mut first = [0; 2];
        let mut second = [0; 2];
        assert_eq!(transport.read(&mut first).unwrap(), 2);
        assert_eq!(transport.read(&mut second).unwrap(), 1);

        assert_eq!(first, [0x10, 0x20]);
        assert_eq!(second[0], 0x30);
        transport.assert_finished();
    }

    #[test]
    fn write_errors_are_propagated_and_recorded() {
        let error = TransportError::new(TransportErrorKind::Timeout, "write", "timed out");
        let mut transport = ScriptedTransport::new("scripted");
        transport.fail_write(error.clone());

        assert_eq!(transport.write_all(b"request").unwrap_err(), error);
        assert_eq!(transport.written(), &[b"request".to_vec()]);
        transport.assert_finished();
    }
}
