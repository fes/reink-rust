use std::collections::VecDeque;

use reink_platform::{ByteTransport, TransportError, TransportErrorKind};

/// One direction of a sanitized protocol transcript.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TranscriptStep {
    /// Bytes the implementation must write next.
    Write(Vec<u8>),
    /// Bytes the simulated peer returns on the next read or reads.
    Read(Vec<u8>),
}

/// An ordered, sanitized transcript for replaying protocol traffic in tests.
///
/// Do not construct fixtures from unredacted traffic. Replace printer-specific
/// identifiers, addresses, and credentials before committing a transcript.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SanitizedTranscript {
    description: String,
    steps: Vec<TranscriptStep>,
}

impl SanitizedTranscript {
    /// Creates an empty transcript with a human-readable sanitized description.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            steps: Vec::new(),
        }
    }

    /// Records bytes the implementation must write at this point in the replay.
    pub fn expect_write(&mut self, bytes: impl Into<Vec<u8>>) {
        self.steps.push(TranscriptStep::Write(bytes.into()));
    }

    /// Records peer bytes returned at this point in the replay.
    pub fn respond(&mut self, bytes: impl Into<Vec<u8>>) {
        self.steps.push(TranscriptStep::Read(bytes.into()));
    }

    /// Records peer bytes split into explicit transport read fragments.
    pub fn respond_fragmented<I, B>(&mut self, fragments: I)
    where
        I: IntoIterator<Item = B>,
        B: Into<Vec<u8>>,
    {
        for fragment in fragments {
            self.respond(fragment);
        }
    }

    /// Builds a strict transport that replays this transcript.
    pub fn into_transport(self) -> TranscriptTransport {
        TranscriptTransport {
            description: self.description,
            steps: self.steps.into(),
        }
    }
}

/// A strict [`ByteTransport`] that replays an ordered sanitized transcript.
#[derive(Debug)]
pub struct TranscriptTransport {
    description: String,
    steps: VecDeque<TranscriptStep>,
}

impl TranscriptTransport {
    /// Panics when a test did not consume the entire transcript.
    pub fn assert_finished(&self) {
        assert!(
            self.steps.is_empty(),
            "{} transcript step(s) were not consumed: {:?}",
            self.steps.len(),
            self.steps
        );
    }

    fn unexpected_operation(&self, operation: &str, actual: Option<&[u8]>) -> TransportError {
        TransportError::new(
            TransportErrorKind::Io,
            "transcript replay",
            format!(
                "{} attempted {operation} {}; expected {:?}",
                self.description,
                actual.map_or_else(String::new, |bytes| format!("{bytes:02X?}")),
                self.steps.front()
            ),
        )
    }
}

impl ByteTransport for TranscriptTransport {
    fn write_all(&mut self, data: &[u8]) -> Result<(), TransportError> {
        match self.steps.pop_front() {
            Some(TranscriptStep::Write(expected)) if expected == data => Ok(()),
            Some(step @ TranscriptStep::Write(_)) => {
                self.steps.push_front(step);
                Err(self.unexpected_operation("write", Some(data)))
            }
            Some(step @ TranscriptStep::Read(_)) => {
                self.steps.push_front(step);
                Err(self.unexpected_operation("write before reading", Some(data)))
            }
            None => Err(self.unexpected_operation("write", Some(data))),
        }
    }

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, TransportError> {
        if buffer.is_empty() {
            return Ok(0);
        }

        match self.steps.pop_front() {
            Some(TranscriptStep::Read(mut bytes)) => {
                let count = bytes.len().min(buffer.len());
                buffer[..count].copy_from_slice(&bytes[..count]);
                bytes.drain(..count);
                if !bytes.is_empty() {
                    self.steps.push_front(TranscriptStep::Read(bytes));
                }
                Ok(count)
            }
            Some(step @ TranscriptStep::Write(_)) => {
                self.steps.push_front(step);
                Err(self.unexpected_operation("read before writing", None))
            }
            None => Ok(0),
        }
    }

    fn description(&self) -> String {
        self.description.clone()
    }
}

#[cfg(test)]
mod tests {
    use reink_platform::ByteTransport;

    use super::SanitizedTranscript;

    #[test]
    fn transcript_enforces_interleaved_order_and_fragments_reads() {
        let mut transcript = SanitizedTranscript::new("sanitized lifecycle");
        transcript.expect_write([0x01, 0x02]);
        transcript.respond_fragmented([vec![0x03], vec![0x04, 0x05]]);
        let mut transport = transcript.into_transport();

        assert!(transport.write_all(&[0x01, 0x02]).is_ok());
        let mut first = [0; 1];
        let mut second = [0; 2];
        assert_eq!(transport.read(&mut first).unwrap(), 1);
        assert_eq!(transport.read(&mut second).unwrap(), 2);
        assert_eq!(first, [0x03]);
        assert_eq!(second, [0x04, 0x05]);
        transport.assert_finished();
    }

    #[test]
    fn transcript_rejects_writes_before_required_reads() {
        let mut transcript = SanitizedTranscript::new("sanitized lifecycle");
        transcript.respond([0x03]);
        let mut transport = transcript.into_transport();

        let error = transport.write_all(&[0x01]).unwrap_err();

        assert!(error.message.contains("write before reading"));
        let mut response = [0; 1];
        assert_eq!(transport.read(&mut response).unwrap(), 1);
        transport.assert_finished();
    }
}
