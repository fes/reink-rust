#![forbid(unsafe_code)]
//! Deterministic test doubles for ReInk protocol and application tests.
//!
//! The scripted doubles are deliberately strict: writes and requests must
//! match their scripts in order, and each test should call `assert_finished`
//! to ensure it consumed all expected behavior.

mod control;
mod discovery;
mod transcript;
mod transport;

pub use control::ScriptedControlChannel;
pub use discovery::StaticDiscovery;
pub use transcript::{SanitizedTranscript, TranscriptStep, TranscriptTransport};
pub use transport::{ReadStep, ScriptedTransport, WriteStep};
