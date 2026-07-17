#![forbid(unsafe_code)]
//! Protocol-independent ReInk domain logic.
//!
//! This crate contains no transport, OS, UI, or async-runtime dependency.

mod command;
mod controller;
mod epson;
mod identity;

pub use command::{
    CommandError, EepromReadReply, encode_command, encode_eeprom_read, encode_eeprom_write,
    encode_factory_command, parse_eeprom_read_reply,
};
pub use controller::{EepromWriteOptions, EpsonController, EpsonError};
pub use epson::{
    AddressWidth, BUILTIN_EPSON_TOML, CounterResetTarget, EpsonSpec, MemoryOperation,
    ModelDatabase, SpecError,
};
pub use identity::{IdentityParseError, PrinterIdentity};
