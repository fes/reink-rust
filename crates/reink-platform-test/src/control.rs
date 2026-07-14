use std::collections::VecDeque;

use reink_platform::{ControlChannel, ControlError};

#[derive(Clone, Debug, Eq, PartialEq)]
enum ControlStep {
    Reply(Vec<u8>),
    Error(ControlError),
}

/// A strict request/reply script for a [`ControlChannel`].
#[derive(Debug, Default)]
pub struct ScriptedControlChannel {
    steps: VecDeque<(Vec<u8>, ControlStep)>,
    requests: Vec<Vec<u8>>,
}

impl ScriptedControlChannel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn expect_reply(&mut self, request: impl Into<Vec<u8>>, reply: impl Into<Vec<u8>>) {
        self.steps
            .push_back((request.into(), ControlStep::Reply(reply.into())));
    }

    pub fn expect_error(&mut self, request: impl Into<Vec<u8>>, error: ControlError) {
        self.steps
            .push_back((request.into(), ControlStep::Error(error)));
    }

    pub fn requests(&self) -> &[Vec<u8>] {
        &self.requests
    }

    /// Panics when the test failed to consume its complete script.
    pub fn assert_finished(&self) {
        assert!(
            self.steps.is_empty(),
            "{} expected request step(s) were not consumed",
            self.steps.len()
        );
    }
}

impl ControlChannel for ScriptedControlChannel {
    fn request(&mut self, request: &[u8]) -> Result<Vec<u8>, ControlError> {
        self.requests.push(request.to_vec());

        match self.steps.pop_front() {
            Some((expected, ControlStep::Reply(reply))) if expected == request => Ok(reply),
            Some((expected, ControlStep::Error(error))) if expected == request => Err(error),
            Some((expected, _)) => Err(ControlError::Protocol {
                message: format!("unexpected request {request:02X?}; expected {expected:02X?}"),
            }),
            None => Err(ControlError::Protocol {
                message: format!("unexpected request {request:02X?}; no request was expected"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use reink_platform::{ControlChannel, ControlError};

    use super::ScriptedControlChannel;

    #[test]
    fn replies_are_matched_in_request_order() {
        let mut channel = ScriptedControlChannel::new();
        channel.expect_reply(b"first", b"one");
        channel.expect_reply(b"second", b"two");

        assert_eq!(channel.request(b"first").unwrap(), b"one");
        assert_eq!(channel.request(b"second").unwrap(), b"two");
        assert_eq!(channel.requests(), &[b"first".to_vec(), b"second".to_vec()]);
        channel.assert_finished();
    }

    #[test]
    fn unexpected_requests_return_protocol_errors() {
        let mut channel = ScriptedControlChannel::new();
        channel.expect_reply(b"expected", b"reply");

        assert!(matches!(
            channel.request(b"actual"),
            Err(ControlError::Protocol { .. })
        ));
    }
}
