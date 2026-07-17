use std::error::Error;
use std::fmt;

use reink_platform::{ControlChannel, ControlError};

use crate::{
    CommandError, EepromReadReply, EpsonSpec, IdentityParseError, MemoryOperation, PrinterIdentity,
    encode_command, encode_eeprom_read, encode_eeprom_write, parse_eeprom_read_reply,
};

/// Safety controls for EEPROM writes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EepromWriteOptions {
    /// Read every value back after its write.
    pub verify_read_back: bool,
    /// Read all original values before writing and restore completed writes if a write fails.
    pub atomic: bool,
}

impl Default for EepromWriteOptions {
    fn default() -> Self {
        Self {
            verify_read_back: true,
            atomic: false,
        }
    }
}

/// Epson operations over an already established control channel.
pub struct EpsonController<'a, C> {
    channel: &'a mut C,
    spec: &'a EpsonSpec,
}

impl<'a, C: ControlChannel> EpsonController<'a, C> {
    pub fn new(channel: &'a mut C, spec: &'a EpsonSpec) -> Self {
        Self { channel, spec }
    }

    /// Requests and parses the printer's IEEE 1284 device ID.
    pub fn read_identity(&mut self) -> Result<PrinterIdentity, EpsonError> {
        let request = encode_command(*b"di", &[1])?;
        let response = self.channel.request(&request)?;
        let response =
            std::str::from_utf8(&response).map_err(|_| EpsonError::InvalidIdentityResponse)?;
        let identifier = response
            .strip_prefix("@EJL ID")
            .ok_or(EpsonError::InvalidIdentityResponse)?
            .trim_start();
        Ok(PrinterIdentity::parse(identifier)?)
    }

    /// Reads the printer's raw Epson status response without changing printer state.
    pub fn read_status(&mut self) -> Result<Vec<u8>, EpsonError> {
        let request = encode_command(*b"st", &[1])?;
        Ok(self.channel.request(&request)?)
    }

    /// Reads the requested EEPROM addresses in order.
    pub fn read_eeprom(&mut self, addresses: &[u16]) -> Result<Vec<EepromReadReply>, EpsonError> {
        addresses
            .iter()
            .copied()
            .map(|address| self.read_eeprom_address(address))
            .collect()
    }

    /// Writes EEPROM values, optionally verifying each value and rolling back
    /// every attempted write after a failure.
    pub fn write_eeprom(
        &mut self,
        updates: &[(u16, u8)],
        options: EepromWriteOptions,
    ) -> Result<(), EpsonError> {
        let originals = if options.atomic {
            Some(
                self.read_eeprom(
                    &updates
                        .iter()
                        .map(|(address, _)| *address)
                        .collect::<Vec<_>>(),
                )?
                .into_iter()
                .map(|reply| (reply.address, reply.value))
                .collect::<Vec<_>>(),
            )
        } else {
            None
        };
        self.write_eeprom_with_originals(updates, originals.as_deref(), options)
    }

    /// Writes values using caller-captured originals for atomic restoration.
    ///
    /// `originals` must contain every update address exactly once. This lets an
    /// application use the durable complete image it captured before any write,
    /// rather than taking a second, narrower pre-write snapshot.
    pub fn write_eeprom_with_originals(
        &mut self,
        updates: &[(u16, u8)],
        originals: Option<&[(u16, u8)]>,
        options: EepromWriteOptions,
    ) -> Result<(), EpsonError> {
        if let Some(originals) = originals {
            for &(address, _) in updates {
                if !originals
                    .iter()
                    .any(|(original_address, _)| *original_address == address)
                {
                    return Err(EpsonError::MissingAtomicOriginal { address });
                }
            }
        } else if options.atomic {
            return Err(EpsonError::MissingAtomicOriginal {
                address: updates.first().map_or(0, |(address, _)| *address),
            });
        }

        let mut attempted = Vec::new();
        for &(address, value) in updates {
            attempted.push(address);
            if let Err(error) = self.write_eeprom_address(address, value, options.verify_read_back)
            {
                let rollback_errors = originals
                    .map(|originals| {
                        attempted
                            .iter()
                            .rev()
                            .filter_map(|attempted_address| {
                                let (_, original_value) =
                                    originals.iter().find(|(original_address, _)| {
                                        original_address == attempted_address
                                    })?;
                                self.write_eeprom_address(
                                    *attempted_address,
                                    *original_value,
                                    options.verify_read_back,
                                )
                                .err()
                                .map(|rollback| (*attempted_address, rollback.to_string()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                return Err(EpsonError::AtomicWriteFailed {
                    address,
                    reason: error.to_string(),
                    rollback_errors,
                });
            }
        }
        Ok(())
    }

    /// Resets the configured waste-counter operation with explicit write options.
    pub fn reset_waste(&mut self, options: EepromWriteOptions) -> Result<(), EpsonError> {
        let operation = self
            .spec
            .waste_counter_reset()
            .ok_or(EpsonError::OperationUnavailable)?;
        self.write_operation(&operation, options)
    }

    pub fn write_operation(
        &mut self,
        operation: &MemoryOperation,
        options: EepromWriteOptions,
    ) -> Result<(), EpsonError> {
        if !operation.has_declared_reset_values() {
            return Err(EpsonError::OperationHasNoDeclaredReset {
                description: operation.description.clone(),
            });
        }
        if operation.addresses.len() != operation.reset_values.len() {
            return Err(EpsonError::OperationResetLengthMismatch {
                description: operation.description.clone(),
                addresses: operation.addresses.len(),
                reset_values: operation.reset_values.len(),
            });
        }
        let updates = operation
            .addresses
            .iter()
            .copied()
            .zip(operation.reset_values.iter().copied())
            .collect::<Vec<_>>();
        self.write_eeprom(&updates, options)
    }

    fn read_eeprom_address(&mut self, address: u16) -> Result<EepromReadReply, EpsonError> {
        let request = encode_eeprom_read(self.spec, address)?;
        let response = self.channel.request(&request)?;
        let reply = parse_eeprom_read_reply(&response, self.spec.read_address_width)?;
        if reply.address != address {
            return Err(EpsonError::AddressMismatch {
                requested: address,
                received: reply.address,
            });
        }
        Ok(reply)
    }

    fn write_eeprom_address(
        &mut self,
        address: u16,
        value: u8,
        verify_read_back: bool,
    ) -> Result<(), EpsonError> {
        let request = encode_eeprom_write(self.spec, address, value)?;
        let response = self.channel.request(&request)?;
        if !response.windows(4).any(|window| window == b":OK;") {
            return Err(EpsonError::WriteRejected { address });
        }
        if verify_read_back {
            let reply = self.read_eeprom_address(address)?;
            if reply.value != value {
                return Err(EpsonError::VerificationFailed {
                    address,
                    expected: value,
                    actual: reply.value,
                });
            }
        }
        Ok(())
    }
}

/// Failure while performing a printer-specific control operation.
#[derive(Debug)]
pub enum EpsonError {
    Control(ControlError),
    Command(CommandError),
    Identity(IdentityParseError),
    InvalidIdentityResponse,
    AddressMismatch {
        requested: u16,
        received: u16,
    },
    WriteRejected {
        address: u16,
    },
    VerificationFailed {
        address: u16,
        expected: u8,
        actual: u8,
    },
    AtomicWriteFailed {
        address: u16,
        reason: String,
        rollback_errors: Vec<(u16, String)>,
    },
    MissingAtomicOriginal {
        address: u16,
    },
    OperationHasNoDeclaredReset {
        description: String,
    },
    OperationResetLengthMismatch {
        description: String,
        addresses: usize,
        reset_values: usize,
    },
    OperationUnavailable,
}

impl fmt::Display for EpsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Control(error) => write!(formatter, "control-channel error: {error}"),
            Self::Command(error) => write!(formatter, "Epson command error: {error}"),
            Self::Identity(error) => write!(formatter, "invalid printer identity: {error}"),
            Self::InvalidIdentityResponse => formatter.write_str("invalid Epson identity response"),
            Self::AddressMismatch {
                requested,
                received,
            } => write!(
                formatter,
                "EEPROM response address {received:#06x} does not match requested {requested:#06x}"
            ),
            Self::WriteRejected { address } => {
                write!(formatter, "EEPROM write was rejected at {address:#06x}")
            }
            Self::VerificationFailed {
                address,
                expected,
                actual,
            } => write!(
                formatter,
                "EEPROM verification failed at {address:#06x}: expected {expected:#04x}, got {actual:#04x}"
            ),
            Self::AtomicWriteFailed {
                address,
                reason,
                rollback_errors,
            } => {
                write!(
                    formatter,
                    "atomic EEPROM write failed at {address:#06x}: {reason}"
                )?;
                for (rollback_address, rollback_reason) in rollback_errors {
                    write!(
                        formatter,
                        "; rollback failed at {rollback_address:#06x}: {rollback_reason}"
                    )?;
                }
                if rollback_errors.is_empty() {
                    formatter.write_str("; rollback completed for all attempted bytes")?;
                }
                Ok(())
            }
            Self::MissingAtomicOriginal { address } => write!(
                formatter,
                "atomic EEPROM write has no original value for {address:#06x}"
            ),
            Self::OperationHasNoDeclaredReset { description } => write!(
                formatter,
                "EEPROM operation {description:?} has no explicitly declared reset values"
            ),
            Self::OperationResetLengthMismatch {
                description,
                addresses,
                reset_values,
            } => write!(
                formatter,
                "EEPROM operation {description:?} has {addresses} addresses but {reset_values} reset values"
            ),
            Self::OperationUnavailable => formatter.write_str("waste-counter reset is unavailable"),
        }
    }
}

impl Error for EpsonError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Control(error) => Some(error),
            Self::Command(error) => Some(error),
            Self::Identity(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ControlError> for EpsonError {
    fn from(error: ControlError) -> Self {
        Self::Control(error)
    }
}

impl From<CommandError> for EpsonError {
    fn from(error: CommandError) -> Self {
        Self::Command(error)
    }
}

impl From<IdentityParseError> for EpsonError {
    fn from(error: IdentityParseError) -> Self {
        Self::Identity(error)
    }
}

#[cfg(test)]
mod tests {
    use reink_platform_test::ScriptedControlChannel;

    use crate::{ModelDatabase, encode_command, encode_eeprom_read, encode_eeprom_write};

    use super::{EepromWriteOptions, EpsonController, EpsonError};

    fn spec() -> crate::EpsonSpec {
        ModelDatabase::builtin()
            .unwrap()
            .get("C90")
            .unwrap()
            .clone()
    }

    #[test]
    fn reads_identity_and_eeprom_values() {
        let spec = spec();
        let mut channel = ScriptedControlChannel::new();
        channel.expect_reply(
            encode_command(*b"di", &[1]).unwrap(),
            b"@EJL ID MFG:EPSON;MDL:C90;",
        );
        channel.expect_reply(
            encode_eeprom_read(&spec, 0x0c).unwrap(),
            b"@BDC PS EE:0C4200;",
        );

        let mut controller = EpsonController::new(&mut channel, &spec);
        assert_eq!(controller.read_identity().unwrap().model(), Some("C90"));
        assert_eq!(controller.read_eeprom(&[0x0c]).unwrap()[0].value, 0x42);
        channel.assert_finished();
    }

    #[test]
    fn reads_raw_printer_status_without_writing() {
        let spec = spec();
        let mut channel = ScriptedControlChannel::new();
        channel.expect_reply(
            encode_command(*b"st", &[1]).unwrap(),
            b"@BDC ST2\r\nREADY\r\n",
        );

        let mut controller = EpsonController::new(&mut channel, &spec);
        assert_eq!(controller.read_status().unwrap(), b"@BDC ST2\r\nREADY\r\n");
        channel.assert_finished();
    }

    #[test]
    fn writes_and_verifies_eeprom_values_by_default() {
        let spec = spec();
        let mut channel = ScriptedControlChannel::new();
        channel.expect_reply(encode_eeprom_write(&spec, 0x0c, 0x42).unwrap(), b":OK;");
        channel.expect_reply(
            encode_eeprom_read(&spec, 0x0c).unwrap(),
            b"@BDC PS EE:0C4200;",
        );

        let mut controller = EpsonController::new(&mut channel, &spec);
        controller
            .write_eeprom(&[(0x0c, 0x42)], EepromWriteOptions::default())
            .unwrap();
        channel.assert_finished();
    }

    #[test]
    fn atomic_write_restores_completed_values_after_failure() {
        let spec = spec();
        let mut channel = ScriptedControlChannel::new();
        channel.expect_reply(
            encode_eeprom_read(&spec, 0x0c).unwrap(),
            b"@BDC PS EE:0C1000;",
        );
        channel.expect_reply(
            encode_eeprom_read(&spec, 0x0d).unwrap(),
            b"@BDC PS EE:0D2000;",
        );
        channel.expect_reply(encode_eeprom_write(&spec, 0x0c, 0x42).unwrap(), b":OK;");
        channel.expect_reply(encode_eeprom_write(&spec, 0x0d, 0x43).unwrap(), b":NA;");
        channel.expect_reply(encode_eeprom_write(&spec, 0x0c, 0x10).unwrap(), b":OK;");

        let mut controller = EpsonController::new(&mut channel, &spec);
        assert!(matches!(
            controller.write_eeprom(
                &[(0x0c, 0x42), (0x0d, 0x43)],
                EepromWriteOptions {
                    verify_read_back: false,
                    atomic: true,
                },
            ),
            Err(EpsonError::AtomicWriteFailed { .. })
        ));
        channel.assert_finished();
    }

    #[test]
    fn atomic_write_restores_the_failed_address_and_every_prior_address() {
        let spec = spec();
        let mut channel = ScriptedControlChannel::new();
        channel.expect_reply(encode_eeprom_write(&spec, 0x0c, 0x42).unwrap(), b":OK;");
        channel.expect_reply(encode_eeprom_write(&spec, 0x0d, 0x43).unwrap(), b":NA;");
        channel.expect_reply(encode_eeprom_write(&spec, 0x0d, 0x20).unwrap(), b":OK;");
        channel.expect_reply(encode_eeprom_write(&spec, 0x0c, 0x10).unwrap(), b":OK;");

        let mut controller = EpsonController::new(&mut channel, &spec);
        assert!(matches!(
            controller.write_eeprom_with_originals(
                &[(0x0c, 0x42), (0x0d, 0x43)],
                Some(&[(0x0c, 0x10), (0x0d, 0x20)]),
                EepromWriteOptions {
                    verify_read_back: false,
                    atomic: true,
                },
            ),
            Err(EpsonError::AtomicWriteFailed {
                rollback_errors,
                ..
            }) if rollback_errors.is_empty()
        ));
        channel.assert_finished();
    }

    #[test]
    fn atomic_rollback_readback_verifies_every_restored_value() {
        let spec = spec();
        let mut channel = ScriptedControlChannel::new();
        channel.expect_reply(encode_eeprom_write(&spec, 0x0c, 0x42).unwrap(), b":OK;");
        channel.expect_reply(
            encode_eeprom_read(&spec, 0x0c).unwrap(),
            b"@BDC PS EE:0C4200;",
        );
        channel.expect_reply(encode_eeprom_write(&spec, 0x0d, 0x43).unwrap(), b":OK;");
        channel.expect_reply(
            encode_eeprom_read(&spec, 0x0d).unwrap(),
            b"@BDC PS EE:0D0000;",
        );
        channel.expect_reply(encode_eeprom_write(&spec, 0x0d, 0x20).unwrap(), b":OK;");
        channel.expect_reply(
            encode_eeprom_read(&spec, 0x0d).unwrap(),
            b"@BDC PS EE:0D2000;",
        );
        channel.expect_reply(encode_eeprom_write(&spec, 0x0c, 0x10).unwrap(), b":OK;");
        channel.expect_reply(
            encode_eeprom_read(&spec, 0x0c).unwrap(),
            b"@BDC PS EE:0C1000;",
        );

        let mut controller = EpsonController::new(&mut channel, &spec);
        assert!(matches!(
            controller.write_eeprom_with_originals(
                &[(0x0c, 0x42), (0x0d, 0x43)],
                Some(&[(0x0c, 0x10), (0x0d, 0x20)]),
                EepromWriteOptions {
                    verify_read_back: true,
                    atomic: true,
                },
            ),
            Err(EpsonError::AtomicWriteFailed {
                rollback_errors,
                ..
            }) if rollback_errors.is_empty()
        ));
        channel.assert_finished();
    }
}
