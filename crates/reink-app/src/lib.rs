#![forbid(unsafe_code)]
//! Application services that compose a selected transport with ReInk protocols.

use std::error::Error;
use std::fmt;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use reink_core::{
    CounterResetTarget, EepromReadReply, EepromWriteOptions, EpsonController, EpsonError,
    EpsonSpec, PrinterIdentity,
};
use reink_d4::{ChannelId, D4Error, D4Link};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use reink_platform::UsbInterfaceSelector;
use reink_platform::{ByteTransport, TransportError};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use reink_platform::{RecordingTransport, TransportEvent};

const EPSON_D4_ENTRY_COMMAND: &[u8] = b"\x00\x00\x00\x1b\x01@EJL 1284.4\n@EJL\n@EJL\n";
const EPSON_D4_ENTRY_REPLY: &[u8] = b"\x00\x00\x00\x08\x01\x00\xc5\x00";
const ENTRY_REPLY_READ_LIMIT: usize = 5;

/// Session-scoped guard for the first persistent write to a connected printer.
///
/// A UI must obtain a backup choice before it can dispatch its first write.
/// Selecting a backup is not enough by itself: callers only record it after
/// the EEPROM image has been saved successfully.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FirstWriteBackupGate {
    resolved: bool,
}

impl Default for FirstWriteBackupGate {
    fn default() -> Self {
        Self { resolved: false }
    }
}

impl FirstWriteBackupGate {
    /// Returns whether the caller must show the EEPROM-backup choice.
    pub const fn requires_backup_choice(self) -> bool {
        !self.resolved
    }

    /// Records that the user successfully saved an EEPROM backup.
    pub fn record_backup_saved(&mut self) {
        self.resolved = true;
    }

    /// Records that the user explicitly chose to continue without a backup.
    ///
    /// The UI must make this an intentional, separate acknowledgement rather
    /// than treating a canceled save dialog as a decline.
    pub fn record_backup_declined(&mut self) {
        self.resolved = true;
    }
}

/// Outcome of the Epson D4 entry probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EpsonD4EntryProbeResult {
    /// The source-compatible Epson entry reply was recognized.
    Recognized,
    /// The device replied, but its bytes did not match the Epson entry reply.
    Unrecognized { received_bytes: usize },
}

/// Probes Epson D4 entry on a selected Linux, macOS, or Windows USB interface without initializing D4.
///
/// The probe sends only the source-compatible Epson entry exchange. It does
/// not initialize D4, open a service, access EEPROM, write printer state, or
/// reset counters.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub fn probe_epson_d4_entry(
    device: reink_usb::UsbDeviceSelector,
    interface: UsbInterfaceSelector,
) -> Result<EpsonD4EntryProbeResult, ApplicationError> {
    let result = reink_usb::probe_bounded_exchange(
        device,
        interface,
        EPSON_D4_ENTRY_COMMAND,
        EPSON_D4_ENTRY_REPLY,
        ENTRY_REPLY_READ_LIMIT,
    )?;
    Ok(match result {
        reink_usb::BoundedExchangeProbeResult::Recognized => EpsonD4EntryProbeResult::Recognized,
        reink_usb::BoundedExchangeProbeResult::Unrecognized { received_bytes } => {
            EpsonD4EntryProbeResult::Unrecognized { received_bytes }
        }
    })
}

/// Probes Epson D4 entry with an explicit USB driver-handoff policy.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub fn probe_epson_d4_entry_with_policy(
    device: reink_usb::UsbDeviceSelector,
    interface: UsbInterfaceSelector,
    handoff: reink_usb::UsbDriverHandoff,
) -> Result<EpsonD4EntryProbeResult, ApplicationError> {
    let result = reink_usb::probe_bounded_exchange_with_policy(
        device,
        interface,
        EPSON_D4_ENTRY_COMMAND,
        EPSON_D4_ENTRY_REPLY,
        ENTRY_REPLY_READ_LIMIT,
        handoff,
    )?;
    Ok(match result {
        reink_usb::BoundedExchangeProbeResult::Recognized => EpsonD4EntryProbeResult::Recognized,
        reink_usb::BoundedExchangeProbeResult::Unrecognized { received_bytes } => {
            EpsonD4EntryProbeResult::Unrecognized { received_bytes }
        }
    })
}

/// A read-only Epson control session over an initialized IEEE 1284.4 link.
pub struct EpsonD4Session<T> {
    link: D4Link<T>,
    control_channel: ChannelId,
    spec: EpsonSpec,
}

/// Read-only application view of an Epson D4 session.
///
/// This view exposes only identity, status, and EEPROM reads. In particular it
/// has no write-plan preparation or application API, which is the capability
/// boundary used by the Windows stock-driver backend.
pub struct ReadOnlyEpsonD4Session<'a, T> {
    session: &'a mut EpsonD4Session<T>,
}

impl<T: ByteTransport> ReadOnlyEpsonD4Session<'_, T> {
    pub fn spec(&self) -> &EpsonSpec {
        self.session.spec()
    }

    pub fn read_identity(&mut self) -> Result<PrinterIdentity, ApplicationError> {
        self.session.read_identity()
    }

    pub fn read_status(&mut self) -> Result<Vec<u8>, ApplicationError> {
        self.session.read_status()
    }

    pub fn read_eeprom(
        &mut self,
        addresses: &[u16],
    ) -> Result<Vec<EepromReadReply>, ApplicationError> {
        self.session.read_eeprom(addresses)
    }

    pub fn dump_eeprom(&mut self) -> Result<EepromImage, ApplicationError> {
        self.session.dump_eeprom()
    }
}

/// A complete, read-only EEPROM image for one resolved model range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EepromImage {
    pub model: String,
    pub start_address: u16,
    pub bytes: Vec<u8>,
}

impl EepromImage {
    pub fn end_address(&self) -> u16 {
        self.start_address
            .checked_add(self.bytes.len().saturating_sub(1) as u16)
            .unwrap_or(u16::MAX)
    }

    pub fn value_at(&self, address: u16) -> Option<u8> {
        address
            .checked_sub(self.start_address)
            .and_then(|offset| self.bytes.get(usize::from(offset)))
            .copied()
    }
}

/// A model-bounded EEPROM write prepared from a complete read-only backup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EepromWritePlan {
    pub backup: EepromImage,
    pub updates: Vec<(u16, u8)>,
}

/// Verifies that a connected printer's normalized model exactly matches the
/// model selected before a D4 session was opened.
pub fn verify_exact_model(identity: &PrinterIdentity, expected_model: &str) -> Result<(), String> {
    match identity.detected_model() {
        Some(model) if model == expected_model => Ok(()),
        Some(model) => Err(format!(
            "printer identity model {model:?} does not match selected model {expected_model:?}"
        )),
        None => Err(format!(
            "printer identity does not contain a model; selected model is {expected_model:?}"
        )),
    }
}

/// Expands a complete restore image into model-bounded EEPROM updates.
pub fn restore_eeprom_updates(spec: &EpsonSpec, bytes: &[u8]) -> Result<Vec<(u16, u8)>, String> {
    let expected_length =
        usize::from(spec.memory_high).saturating_sub(usize::from(spec.memory_low)) + 1;
    if bytes.len() != expected_length {
        return Err(format!(
            "EEPROM restore image has {} bytes; model {} requires exactly {} bytes for {:#06x}..={:#06x}",
            bytes.len(),
            spec.model,
            expected_length,
            spec.memory_low,
            spec.memory_high
        ));
    }
    Ok(bytes
        .iter()
        .copied()
        .enumerate()
        .map(|(offset, value)| (spec.memory_low + offset as u16, value))
        .collect())
}

/// Returns only model bytes with explicitly declared reset values for one
/// semantic counter family.
pub fn declared_counter_reset_updates(
    spec: &EpsonSpec,
    target: CounterResetTarget,
) -> Result<Vec<(u16, u8)>, String> {
    let operation = spec.counter_reset(target).ok_or_else(|| {
        format!(
            "model {} has no explicitly declared {} reset bytes",
            spec.model,
            target.display_name()
        )
    })?;
    if !operation.has_declared_reset_values() {
        return Err(format!(
            "model {} has no explicitly declared {} reset bytes",
            spec.model,
            target.display_name()
        ));
    }
    let updates = operation
        .addresses
        .into_iter()
        .zip(operation.reset_values)
        .collect::<Vec<_>>();
    validate_eeprom_updates(spec, &updates).map_err(|error| error.to_string())?;
    Ok(updates)
}

/// Creates and durably synchronizes a new private binary file without
/// overwriting an existing path.
pub fn write_new_binary_file(path: &Path, bytes: &[u8], kind: &str) -> Result<(), String> {
    write_new_binary_file_with_parent_sync(path, bytes, kind, sync_parent_directory)
}

fn write_new_binary_file_with_parent_sync(
    path: &Path,
    bytes: &[u8],
    kind: &str,
    sync_parent: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("could not create {kind} {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("could not persist {kind} {}: {error}", path.display()))?;
    sync_parent(path).map_err(|error| {
        format!(
            "could not durably persist {kind} {}: {error}",
            path.display()
        )
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    File::open(parent)?.sync_all()
}

#[cfg(target_os = "windows")]
fn sync_parent_directory(path: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    OpenOptions::new()
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(parent)?
        .sync_all()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn sync_parent_directory(_: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "parent-directory synchronization is not supported on this platform",
    ))
}

/// Status of a cleanup stage after a selected USB operation.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UsbCleanupStatus {
    NotAttempted,
    Succeeded,
    Failed(String),
}

/// D4 and USB cleanup outcomes retained even when the requested operation fails.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsbSessionCleanup {
    pub d4_shutdown: UsbCleanupStatus,
    pub usb_close: UsbCleanupStatus,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl UsbSessionCleanup {
    pub const fn not_attempted() -> Self {
        Self {
            d4_shutdown: UsbCleanupStatus::NotAttempted,
            usb_close: UsbCleanupStatus::NotAttempted,
        }
    }
}

/// Result and cleanup record for one explicitly selected USB D4 operation.
///
/// `events` is empty unless the caller explicitly enables recording.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[derive(Debug)]
pub struct SelectedUsbSessionOutcome<T> {
    pub operation: Result<T, String>,
    pub cleanup: UsbSessionCleanup,
    pub events: Vec<TransportEvent>,
}

/// Opens one selected USB interface, runs a caller-authorized D4 operation, and
/// always attempts orderly D4 and USB cleanup.
///
/// This function does not choose a device, model, or operation. Callers must
/// verify the D4 identity in `operation` before reading model-specific state or
/// applying a write plan. Transport events are retained only when
/// `record_traffic` is true.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub fn with_selected_usb_epson_session<T>(
    device: reink_usb::UsbDeviceSelector,
    interface: UsbInterfaceSelector,
    spec: EpsonSpec,
    record_traffic: bool,
    operation: impl FnOnce(
        &mut EpsonD4Session<RecordingTransport<reink_usb::ReadOnlyUsbTransport>>,
    ) -> Result<T, String>,
) -> SelectedUsbSessionOutcome<T> {
    let transport = match reink_usb::ReadOnlyUsbTransport::open(device, interface) {
        Ok(transport) => transport,
        Err(error) => {
            return SelectedUsbSessionOutcome {
                operation: Err(format!("could not open selected USB interface: {error}")),
                cleanup: UsbSessionCleanup::not_attempted(),
                events: Vec::new(),
            };
        }
    };
    let recording = RecordingTransport::new_with_recording(transport, record_traffic);
    let mut session = match EpsonD4Session::connect_recoverable(recording, spec) {
        Ok(session) => session,
        Err((error, recording)) => {
            let (mut transport, events) = recording.into_parts();
            let d4_shutdown = match &error {
                ApplicationError::SetupRecovered { .. } => UsbCleanupStatus::Succeeded,
                ApplicationError::SetupRecovery { recovery, .. } => {
                    UsbCleanupStatus::Failed(recovery.to_string())
                }
                _ => UsbCleanupStatus::NotAttempted,
            };
            let usb_close = match transport.close() {
                Ok(()) => UsbCleanupStatus::Succeeded,
                Err(close) => UsbCleanupStatus::Failed(close.to_string()),
            };
            return SelectedUsbSessionOutcome {
                operation: Err(format!("Epson D4 session setup failed: {error}")),
                cleanup: UsbSessionCleanup {
                    d4_shutdown,
                    usb_close,
                },
                events,
            };
        }
    };

    let operation = operation(&mut session);
    let d4_shutdown = match session.shutdown() {
        Ok(()) => UsbCleanupStatus::Succeeded,
        Err(error) => UsbCleanupStatus::Failed(error.to_string()),
    };
    let recording = session.into_transport();
    let (mut transport, events) = recording.into_parts();
    let usb_close = match transport.close() {
        Ok(()) => UsbCleanupStatus::Succeeded,
        Err(error) => UsbCleanupStatus::Failed(error.to_string()),
    };
    SelectedUsbSessionOutcome {
        operation,
        cleanup: UsbSessionCleanup {
            d4_shutdown,
            usb_close,
        },
        events,
    }
}

/// Runs one read-only D4 operation through a selected Windows USBPRINT token.
///
/// The callback receives only [`ReadOnlyEpsonD4Session`], so generic EEPROM
/// write, restore, and reset APIs are unavailable through this composition.
#[cfg(target_os = "windows")]
pub fn with_selected_windows_native_epson_session<T>(
    candidate: &reink_usb::WindowsNativePrinterCandidate,
    spec: EpsonSpec,
    record_traffic: bool,
    operation: impl FnOnce(
        &mut ReadOnlyEpsonD4Session<
            '_,
            RecordingTransport<reink_usb::WindowsNativeReadOnlyTransport>,
        >,
    ) -> Result<T, String>,
) -> SelectedUsbSessionOutcome<T> {
    with_opened_windows_native_epson_session(candidate, spec, record_traffic, |session| {
        operation(&mut ReadOnlyEpsonD4Session { session })
    })
}

/// Runs an explicitly authorized experimental mutation operation through a
/// selected Windows USBPRINT token. `WriteFile` D4 mutation remains unvalidated
/// on physical hardware; callers must require a separate acknowledgement.
#[cfg(target_os = "windows")]
pub fn with_selected_windows_native_experimental_mutation_session<T>(
    candidate: &reink_usb::WindowsNativePrinterCandidate,
    spec: EpsonSpec,
    operation: impl FnOnce(
        &mut EpsonD4Session<RecordingTransport<reink_usb::WindowsNativeReadOnlyTransport>>,
    ) -> Result<T, String>,
) -> SelectedUsbSessionOutcome<T> {
    with_opened_windows_native_epson_session(candidate, spec, false, operation)
}

#[cfg(target_os = "windows")]
fn with_opened_windows_native_epson_session<T>(
    candidate: &reink_usb::WindowsNativePrinterCandidate,
    spec: EpsonSpec,
    record_traffic: bool,
    operation: impl FnOnce(
        &mut EpsonD4Session<RecordingTransport<reink_usb::WindowsNativeReadOnlyTransport>>,
    ) -> Result<T, String>,
) -> SelectedUsbSessionOutcome<T> {
    let transport = match reink_usb::WindowsNativeReadOnlyTransport::open(candidate) {
        Ok(transport) => transport,
        Err(error) => {
            return SelectedUsbSessionOutcome {
                operation: Err(format!(
                    "could not open selected Windows stock-driver interface: {error}"
                )),
                cleanup: UsbSessionCleanup::not_attempted(),
                events: Vec::new(),
            };
        }
    };
    let recording = RecordingTransport::new_with_recording(transport, record_traffic);
    let mut session = match EpsonD4Session::connect_recoverable(recording, spec) {
        Ok(session) => session,
        Err((error, recording)) => {
            let (mut transport, events) = recording.into_parts();
            let d4_shutdown = match &error {
                ApplicationError::SetupRecovered { .. } => UsbCleanupStatus::Succeeded,
                ApplicationError::SetupRecovery { recovery, .. } => {
                    UsbCleanupStatus::Failed(recovery.to_string())
                }
                _ => UsbCleanupStatus::NotAttempted,
            };
            let usb_close = match transport.close() {
                Ok(()) => UsbCleanupStatus::Succeeded,
                Err(close) => UsbCleanupStatus::Failed(close.to_string()),
            };
            return SelectedUsbSessionOutcome {
                operation: Err(format!("Epson D4 session setup failed: {error}")),
                cleanup: UsbSessionCleanup {
                    d4_shutdown,
                    usb_close,
                },
                events,
            };
        }
    };
    let operation = operation(&mut session);
    let d4_shutdown = match session.shutdown() {
        Ok(()) => UsbCleanupStatus::Succeeded,
        Err(error) => UsbCleanupStatus::Failed(error.to_string()),
    };
    let recording = session.into_transport();
    let (mut transport, events) = recording.into_parts();
    let usb_close = match transport.close() {
        Ok(()) => UsbCleanupStatus::Succeeded,
        Err(error) => UsbCleanupStatus::Failed(error.to_string()),
    };
    SelectedUsbSessionOutcome {
        operation,
        cleanup: UsbSessionCleanup {
            d4_shutdown,
            usb_close,
        },
        events,
    }
}
impl<T: ByteTransport> EpsonD4Session<T> {
    /// Enters Epson D4 mode, initializes the link, and opens `EPSON-CTRL`.
    pub fn connect(target: T, spec: EpsonSpec) -> Result<Self, ApplicationError> {
        Self::connect_recoverable(target, spec).map_err(|(error, _)| error)
    }

    /// Connects while returning the transport if D4 setup fails.
    ///
    /// Callers with explicit transport cleanup can use the returned transport
    /// to report cleanup failures. The Epson entry exchange is
    /// source-compatible with ReInkPy. Hardware use is limited to explicitly
    /// selected read sessions and the gated reversible write-evidence workflow;
    /// no caller may treat this API as authorization for an automatic write.
    pub fn connect_recoverable(
        mut target: T,
        spec: EpsonSpec,
    ) -> Result<Self, (ApplicationError, T)> {
        if let Err(error) = target.write_all(EPSON_D4_ENTRY_COMMAND) {
            return Err((error.into(), target));
        }
        if let Err(error) = wait_for_entry_reply(&mut target) {
            return Err((error, target));
        }

        let mut link = D4Link::new(target);
        if let Err(error) = link.initialize() {
            return Err((error.into(), link.target()));
        }
        let control_channel = match link.open_service("EPSON-CTRL") {
            Ok(control_channel) => control_channel,
            Err(setup) => {
                let recovery = link.exit().err();
                let target = link.target();
                let error = match recovery {
                    Some(recovery) => ApplicationError::SetupRecovery { setup, recovery },
                    None => ApplicationError::SetupRecovered { setup },
                };
                return Err((error, target));
            }
        };
        Ok(Self {
            link,
            control_channel,
            spec,
        })
    }

    pub fn spec(&self) -> &EpsonSpec {
        &self.spec
    }

    pub fn read_identity(&mut self) -> Result<PrinterIdentity, ApplicationError> {
        let mut channel = self.link.control_channel(self.control_channel)?;
        Ok(EpsonController::new(&mut channel, &self.spec).read_identity()?)
    }

    /// Reads the printer's raw Epson status response without changing printer state.
    pub fn read_status(&mut self) -> Result<Vec<u8>, ApplicationError> {
        let mut channel = self.link.control_channel(self.control_channel)?;
        Ok(EpsonController::new(&mut channel, &self.spec).read_status()?)
    }

    pub fn read_eeprom(
        &mut self,
        addresses: &[u16],
    ) -> Result<Vec<EepromReadReply>, ApplicationError> {
        let mut channel = self.link.control_channel(self.control_channel)?;
        Ok(EpsonController::new(&mut channel, &self.spec).read_eeprom(addresses)?)
    }

    /// Reads every address in the selected model's bounded EEPROM range.
    pub fn dump_eeprom(&mut self) -> Result<EepromImage, ApplicationError> {
        let start_address = self.spec.memory_low;
        let mut bytes = Vec::with_capacity(
            usize::from(self.spec.memory_high).saturating_sub(usize::from(start_address)) + 1,
        );
        for address in start_address..=self.spec.memory_high {
            let mut reply = self.read_eeprom(&[address])?;
            bytes.push(
                reply
                    .pop()
                    .expect("one requested EEPROM address produces one reply")
                    .value,
            );
        }
        Ok(EepromImage {
            model: self.spec.model.clone(),
            start_address,
            bytes,
        })
    }

    /// Reads a complete backup and validates every requested update before any
    /// EEPROM write is sent.
    pub fn prepare_eeprom_write(
        &mut self,
        updates: &[(u16, u8)],
    ) -> Result<EepromWritePlan, ApplicationError> {
        validate_eeprom_updates(&self.spec, updates)?;
        Ok(EepromWritePlan {
            backup: self.dump_eeprom()?,
            updates: updates.to_vec(),
        })
    }

    /// Applies a previously prepared plan with read-back verification and
    /// rollback of completed updates if a write fails.
    pub fn apply_eeprom_write(&mut self, plan: &EepromWritePlan) -> Result<(), ApplicationError> {
        if plan.backup.model != self.spec.model
            || plan.backup.start_address != self.spec.memory_low
            || plan.backup.bytes.len()
                != usize::from(self.spec.memory_high)
                    .saturating_sub(usize::from(self.spec.memory_low))
                    + 1
        {
            return Err(ApplicationError::WritePlan(
                "backup does not match the active model range".to_owned(),
            ));
        }
        validate_eeprom_updates(&self.spec, &plan.updates)?;
        let originals = plan
            .updates
            .iter()
            .map(|&(address, _)| {
                plan.backup
                    .value_at(address)
                    .map(|value| (address, value))
                    .ok_or_else(|| {
                        ApplicationError::WritePlan(format!(
                            "backup does not contain EEPROM address {address:#06x}"
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut channel = self.link.control_channel(self.control_channel)?;
        EpsonController::new(&mut channel, &self.spec).write_eeprom_with_originals(
            &plan.updates,
            Some(&originals),
            EepromWriteOptions {
                verify_read_back: true,
                atomic: true,
            },
        )?;
        Ok(())
    }

    /// Closes the control channel and terminates the D4 conversation.
    pub fn shutdown(&mut self) -> Result<(), ApplicationError> {
        let close = self.link.close_channel(self.control_channel).err();
        let exit = self.link.exit().err();
        match (close, exit) {
            (None, None) => Ok(()),
            (close, exit) => Err(ApplicationError::Shutdown { close, exit }),
        }
    }

    pub fn into_transport(self) -> T {
        self.link.target()
    }
}

fn validate_eeprom_updates(
    spec: &EpsonSpec,
    updates: &[(u16, u8)],
) -> Result<(), ApplicationError> {
    if updates.is_empty() {
        return Err(ApplicationError::WritePlan(
            "at least one EEPROM update is required".to_owned(),
        ));
    }
    let mut addresses = std::collections::BTreeSet::new();
    for &(address, _) in updates {
        if address < spec.memory_low || address > spec.memory_high {
            return Err(ApplicationError::WritePlan(format!(
                "EEPROM update address {address:#06x} is outside model range {:#06x}..={:#06x}",
                spec.memory_low, spec.memory_high
            )));
        }
        if !addresses.insert(address) {
            return Err(ApplicationError::WritePlan(format!(
                "EEPROM update address {address:#06x} is duplicated"
            )));
        }
    }
    Ok(())
}

fn wait_for_entry_reply<T: ByteTransport>(target: &mut T) -> Result<(), ApplicationError> {
    let mut reply = Vec::new();
    let mut buffer = [0; 256];
    let mut received_data = false;
    for _ in 0..ENTRY_REPLY_READ_LIMIT {
        let read = target.read(&mut buffer)?;
        if read == 0 {
            continue;
        }
        received_data = true;
        reply.extend_from_slice(&buffer[..read]);
        if reply
            .windows(EPSON_D4_ENTRY_REPLY.len())
            .any(|window| window == EPSON_D4_ENTRY_REPLY)
        {
            return Ok(());
        }
    }
    if received_data {
        Err(ApplicationError::EntryReplyInvalid)
    } else {
        Err(ApplicationError::EntryReplyMissing)
    }
}

/// Failure while composing the transport, D4, and Epson layers.
#[derive(Debug)]
pub enum ApplicationError {
    Transport(TransportError),
    D4(D4Error),
    SetupRecovered {
        setup: D4Error,
    },
    SetupRecovery {
        setup: D4Error,
        recovery: D4Error,
    },
    Shutdown {
        close: Option<D4Error>,
        exit: Option<D4Error>,
    },
    Epson(EpsonError),
    WritePlan(String),
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    Usb(reink_usb::UsbOpenError),
    EntryReplyMissing,
    EntryReplyInvalid,
}

impl fmt::Display for ApplicationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "transport error: {error}"),
            Self::D4(error) => write!(formatter, "D4 error: {error}"),
            Self::SetupRecovered { setup } => write!(
                formatter,
                "D4 service setup failed: {setup}; orderly D4 exit succeeded"
            ),
            Self::SetupRecovery { setup, recovery } => write!(
                formatter,
                "D4 service setup failed: {setup}; orderly D4 exit also failed: {recovery}"
            ),
            Self::Shutdown {
                close: Some(close),
                exit: Some(exit),
            } => write!(
                formatter,
                "D4 control channel close failed: {close}; D4 conversation exit also failed: {exit}"
            ),
            Self::Shutdown {
                close: Some(close),
                exit: None,
            } => write!(
                formatter,
                "D4 control channel close failed: {close}; D4 conversation exit succeeded"
            ),
            Self::Shutdown {
                close: None,
                exit: Some(exit),
            } => write!(
                formatter,
                "D4 control channel close succeeded; D4 conversation exit failed: {exit}"
            ),
            Self::Shutdown {
                close: None,
                exit: None,
            } => unreachable!("shutdown errors contain at least one failure"),
            Self::Epson(error) => write!(formatter, "Epson error: {error}"),
            Self::WritePlan(error) => write!(formatter, "invalid EEPROM write plan: {error}"),
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            Self::Usb(error) => write!(formatter, "USB error: {error}"),
            Self::EntryReplyMissing => formatter.write_str("Epson D4 entry reply was not received"),
            Self::EntryReplyInvalid => {
                formatter.write_str("Epson D4 entry reply was not recognized")
            }
        }
    }
}

impl Error for ApplicationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::D4(error) => Some(error),
            Self::SetupRecovered { setup } | Self::SetupRecovery { setup, .. } => Some(setup),
            Self::Shutdown {
                close: Some(error), ..
            }
            | Self::Shutdown {
                close: None,
                exit: Some(error),
            } => Some(error),
            Self::Shutdown {
                close: None,
                exit: None,
            } => None,
            Self::Epson(error) => Some(error),
            Self::WritePlan(_) => None,
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            Self::Usb(error) => Some(error),
            Self::EntryReplyMissing | Self::EntryReplyInvalid => None,
        }
    }
}

impl From<TransportError> for ApplicationError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

impl From<D4Error> for ApplicationError {
    fn from(error: D4Error) -> Self {
        Self::D4(error)
    }
}

impl From<EpsonError> for ApplicationError {
    fn from(error: EpsonError) -> Self {
        Self::Epson(error)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl From<reink_usb::UsbOpenError> for ApplicationError {
    fn from(error: reink_usb::UsbOpenError) -> Self {
        Self::Usb(error)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        fs, io,
        path::PathBuf,
        process,
        time::{SystemTime, UNIX_EPOCH},
    };

    use reink_core::{
        CounterResetTarget, ModelDatabase, PrinterIdentity, encode_command, encode_eeprom_read,
    };
    use reink_d4::{D4Error, Packet, ProtocolRevision, TransactionMessage};
    use reink_platform_test::{SanitizedTranscript, ScriptedTransport, TranscriptTransport};

    use super::{
        ApplicationError, EPSON_D4_ENTRY_COMMAND, EPSON_D4_ENTRY_REPLY, EpsonD4Session,
        FirstWriteBackupGate, declared_counter_reset_updates, restore_eeprom_updates,
        validate_eeprom_updates, verify_exact_model, write_new_binary_file,
        write_new_binary_file_with_parent_sync,
    };

    #[test]
    fn first_write_requires_an_explicit_backup_decision() {
        let mut gate = FirstWriteBackupGate::default();
        assert!(gate.requires_backup_choice());

        gate.record_backup_saved();

        assert!(!gate.requires_backup_choice());
    }

    #[test]
    fn explicit_backup_decline_resolves_the_first_write_prompt() {
        let mut gate = FirstWriteBackupGate::default();

        gate.record_backup_declined();

        assert!(!gate.requires_backup_choice());
    }

    #[test]
    fn binary_file_write_is_gated_when_parent_sync_fails() {
        let path = PathBuf::from(format!(
            ".reink-parent-sync-gate-{}-{}.bin",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time is after the Unix epoch")
                .as_nanos()
        ));
        let sync_attempted = Cell::new(false);

        let result = write_new_binary_file_with_parent_sync(&path, b"test", "test file", |_| {
            sync_attempted.set(true);
            Err(io::Error::new(
                io::ErrorKind::Other,
                "parent directory sync failed",
            ))
        });

        assert!(sync_attempted.get());
        fs::remove_file(&path).expect("test file is cleaned up");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("could not durably persist test file")
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[test]
    fn binary_file_write_uses_the_platform_parent_sync() {
        let directory = std::env::temp_dir().join(format!(
            "reink-parent-sync-{}-{}",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time is after the Unix epoch")
                .as_nanos()
        ));
        fs::create_dir(&directory).expect("test directory is created");
        let path = directory.join("backup.bin");

        write_new_binary_file(&path, b"durable", "test backup")
            .expect("the platform can synchronize a new file and its parent directory");
        assert_eq!(fs::read(&path).unwrap(), b"durable");
        assert!(write_new_binary_file(&path, b"replacement", "test backup").is_err());

        fs::remove_file(path).expect("test file is cleaned up");
        fs::remove_dir(directory).expect("test directory is cleaned up");
    }

    #[test]
    fn entry_probe_retries_an_empty_read_before_the_reply() {
        let mut target = ScriptedTransport::new("scripted");
        target.push_read_data(Vec::new());
        target.push_read_data(EPSON_D4_ENTRY_REPLY);

        super::wait_for_entry_reply(&mut target).unwrap();
        target.assert_finished();
    }

    #[test]
    fn write_plan_requires_unique_model_bounded_addresses() {
        let spec = spec();
        assert!(validate_eeprom_updates(&spec, &[]).is_err());
        assert!(
            validate_eeprom_updates(&spec, &[(spec.memory_low, 1), (spec.memory_low, 2)]).is_err()
        );
        assert!(
            validate_eeprom_updates(&spec, &[(spec.memory_high.saturating_add(1), 1)]).is_err()
        );
        assert!(validate_eeprom_updates(&spec, &[(spec.memory_low, 1)]).is_ok());
    }

    #[test]
    fn selected_model_must_exactly_match_the_connected_identity() {
        let matching = PrinterIdentity::parse("MFG:EPSON;MDL:C90;SN:private;").unwrap();
        let mismatched = PrinterIdentity::parse("MFG:EPSON;MDL:XP-352 Series;").unwrap();

        assert!(verify_exact_model(&matching, "C90").is_ok());
        assert!(verify_exact_model(&mismatched, "C90").is_err());
    }

    #[test]
    fn restore_updates_cover_the_exact_model_range() {
        let spec = spec();
        let length = usize::from(spec.memory_high).saturating_sub(usize::from(spec.memory_low)) + 1;
        let image = vec![0x5a; length];

        let updates = restore_eeprom_updates(&spec, &image).unwrap();

        assert_eq!(updates.len(), length);
        assert_eq!(updates.first(), Some(&(spec.memory_low, 0x5a)));
        assert_eq!(updates.last(), Some(&(spec.memory_high, 0x5a)));
        assert!(restore_eeprom_updates(&spec, &image[..length - 1]).is_err());
    }

    #[test]
    fn declared_counter_reset_excludes_undeclared_counter_families() {
        let c90 = spec();

        assert!(
            !declared_counter_reset_updates(&c90, CounterResetTarget::Waste)
                .unwrap()
                .is_empty()
        );
        assert!(declared_counter_reset_updates(&c90, CounterResetTarget::PlatenPad).is_err());
    }

    fn spec() -> reink_core::EpsonSpec {
        ModelDatabase::builtin()
            .unwrap()
            .get("C90")
            .unwrap()
            .clone()
    }

    fn transaction_packet(message: TransactionMessage, credit: u8) -> Vec<u8> {
        Packet::new(
            0,
            0,
            message.encode(ProtocolRevision::V20).unwrap(),
            credit,
            0,
        )
        .unwrap()
        .encode()
    }

    fn respond_fragmented_packet(target: &mut SanitizedTranscript, packet: Vec<u8>) {
        let split = packet.len().min(3);
        target.respond_fragmented([packet[..split].to_vec(), packet[split..].to_vec()]);
    }

    fn read_only_d4_transcript(spec: &reink_core::EpsonSpec) -> TranscriptTransport {
        let mut target = SanitizedTranscript::new("synthetic Epson D4 read-only lifecycle");
        target.expect_write(EPSON_D4_ENTRY_COMMAND);
        target.respond_fragmented([
            EPSON_D4_ENTRY_REPLY[..4].to_vec(),
            EPSON_D4_ENTRY_REPLY[4..].to_vec(),
        ]);
        target.expect_write(Packet::new(0, 0, [0, 0x20], 1, 0).unwrap().encode());
        respond_fragmented_packet(
            &mut target,
            transaction_packet(
                TransactionMessage::InitReply {
                    result: 0,
                    revision: ProtocolRevision::V20,
                },
                1,
            ),
        );
        target.expect_write(
            Packet::new(0, 0, b"\x09EPSON-CTRL".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            transaction_packet(
                TransactionMessage::GetSocketIdReply {
                    result: 0,
                    socket_id: 2,
                    service_name: "EPSON-CTRL".to_owned(),
                },
                1,
            ),
        );
        target.expect_write(
            Packet::new(0, 0, b"\x01\x02\x02\x01\x00\x01\x00\x00\x00".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            transaction_packet(
                TransactionMessage::OpenChannelReply {
                    result: 0,
                    peer_socket: 2,
                    source_socket: 2,
                    max_packet_size: 0x100,
                    max_service_size: 0x100,
                    max_credit: 0,
                    granted_credit: 1,
                },
                1,
            ),
        );
        target.expect_write(
            Packet::new(2, 2, encode_command(*b"st", &[1]).unwrap(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            Packet::new(2, 2, b"@BDC ST2\r\nREADY\r\n".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        target.expect_write(
            Packet::new(2, 2, encode_command(*b"di", &[1]).unwrap(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            Packet::new(2, 2, b"@EJL ID MFG:EPSON;MDL:C90;".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        target.expect_write(
            Packet::new(2, 2, encode_eeprom_read(spec, 0x0c).unwrap(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            Packet::new(2, 2, b"@BDC PS EE:0C4200;".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        target.expect_write(
            Packet::new(0, 0, b"\x02\x02\x02".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            transaction_packet(
                TransactionMessage::CloseChannelReply {
                    result: 0,
                    peer_socket: 2,
                    source_socket: 2,
                },
                1,
            ),
        );
        target.expect_write(Packet::new(0, 0, [0x08], 1, 0).unwrap().encode());
        respond_fragmented_packet(
            &mut target,
            transaction_packet(TransactionMessage::ExitReply { result: 0 }, 1),
        );
        target.into_transport()
    }

    #[test]
    fn opens_a_read_only_session_and_reads_status_identity_and_eeprom() {
        let spec = spec();
        let target = read_only_d4_transcript(&spec);

        let mut session = EpsonD4Session::connect(target, spec).unwrap();

        assert_eq!(session.read_status().unwrap(), b"@BDC ST2\r\nREADY\r\n");
        assert_eq!(session.read_identity().unwrap().model(), Some("C90"));
        assert_eq!(session.read_eeprom(&[0x0c]).unwrap()[0].value, 0x42);
        session.shutdown().unwrap();
        session.into_transport().assert_finished();
    }

    #[test]
    fn shutdown_exits_after_close_failure_and_reports_both_failures() {
        let mut target = SanitizedTranscript::new("D4 shutdown attempts exit after close failure");
        target.expect_write(EPSON_D4_ENTRY_COMMAND);
        target.respond_fragmented([
            EPSON_D4_ENTRY_REPLY[..4].to_vec(),
            EPSON_D4_ENTRY_REPLY[4..].to_vec(),
        ]);
        target.expect_write(Packet::new(0, 0, [0, 0x20], 1, 0).unwrap().encode());
        respond_fragmented_packet(
            &mut target,
            transaction_packet(
                TransactionMessage::InitReply {
                    result: 0,
                    revision: ProtocolRevision::V20,
                },
                1,
            ),
        );
        target.expect_write(
            Packet::new(0, 0, b"\x09EPSON-CTRL".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            transaction_packet(
                TransactionMessage::GetSocketIdReply {
                    result: 0,
                    socket_id: 2,
                    service_name: "EPSON-CTRL".to_owned(),
                },
                1,
            ),
        );
        target.expect_write(
            Packet::new(0, 0, b"\x01\x02\x02\x01\x00\x01\x00\x00\x00".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            transaction_packet(
                TransactionMessage::OpenChannelReply {
                    result: 0,
                    peer_socket: 2,
                    source_socket: 2,
                    max_packet_size: 0x100,
                    max_service_size: 0x100,
                    max_credit: 0,
                    granted_credit: 1,
                },
                1,
            ),
        );
        target.expect_write(
            Packet::new(0, 0, b"\x02\x02\x02".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            transaction_packet(
                TransactionMessage::CloseChannelReply {
                    result: 1,
                    peer_socket: 2,
                    source_socket: 2,
                },
                1,
            ),
        );
        target.expect_write(Packet::new(0, 0, [0x08], 1, 0).unwrap().encode());
        respond_fragmented_packet(
            &mut target,
            transaction_packet(TransactionMessage::ExitReply { result: 1 }, 1),
        );

        let mut session = EpsonD4Session::connect(target.into_transport(), spec()).unwrap();
        let error = session.shutdown().unwrap_err();

        assert!(matches!(
            &error,
            ApplicationError::Shutdown {
                close: Some(D4Error::DeviceRejected { result: 1 }),
                exit: Some(D4Error::UnexpectedTransactionReply),
            }
        ));
        assert!(
            error
                .to_string()
                .contains("D4 conversation exit also failed")
        );
        session.into_transport().assert_finished();
    }

    #[test]
    fn rejects_an_unrecognized_epson_entry_reply() {
        let mut target = SanitizedTranscript::new("unrecognized Epson D4 entry reply");
        target.expect_write(EPSON_D4_ENTRY_COMMAND);
        target.respond_fragmented([
            b"\x00".to_vec(),
            b"\x00".to_vec(),
            b"\x00".to_vec(),
            b"\x08".to_vec(),
            b"\x01".to_vec(),
        ]);

        let error = match EpsonD4Session::connect(target.into_transport(), spec()) {
            Ok(_) => panic!("unrecognized Epson entry reply unexpectedly opened a D4 session"),
            Err(error) => error,
        };

        assert!(matches!(error, ApplicationError::EntryReplyInvalid));
    }

    #[test]
    fn recoverable_connect_returns_the_transport_after_setup_failure() {
        let mut target = SanitizedTranscript::new("recoverable Epson D4 setup failure");
        target.expect_write(EPSON_D4_ENTRY_COMMAND);
        target.respond_fragmented([
            b"\x00".to_vec(),
            b"\x00".to_vec(),
            b"\x00".to_vec(),
            b"\x08".to_vec(),
            b"\x01".to_vec(),
        ]);

        let (error, target) =
            match EpsonD4Session::connect_recoverable(target.into_transport(), spec()) {
                Ok(_) => panic!("unrecognized Epson entry reply unexpectedly opened a D4 session"),
                Err(recovery) => recovery,
            };

        assert!(matches!(error, ApplicationError::EntryReplyInvalid));
        target.assert_finished();
    }

    #[test]
    fn failed_service_setup_attempts_an_orderly_d4_exit() {
        let mut target = SanitizedTranscript::new("failed D4 service setup exits cleanly");
        target.expect_write(EPSON_D4_ENTRY_COMMAND);
        target.respond_fragmented([
            EPSON_D4_ENTRY_REPLY[..4].to_vec(),
            EPSON_D4_ENTRY_REPLY[4..].to_vec(),
        ]);
        target.expect_write(Packet::new(0, 0, [0, 0x20], 1, 0).unwrap().encode());
        respond_fragmented_packet(
            &mut target,
            transaction_packet(
                TransactionMessage::InitReply {
                    result: 0,
                    revision: ProtocolRevision::V20,
                },
                1,
            ),
        );
        target.expect_write(
            Packet::new(0, 0, b"\x09EPSON-CTRL".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            transaction_packet(
                TransactionMessage::GetSocketIdReply {
                    result: 0,
                    socket_id: 2,
                    service_name: "EPSON-CTRL".to_owned(),
                },
                1,
            ),
        );
        target.expect_write(
            Packet::new(0, 0, b"\x01\x02\x02\x01\x00\x01\x00\x00\x00".to_vec(), 1, 0)
                .unwrap()
                .encode(),
        );
        respond_fragmented_packet(
            &mut target,
            Packet::new(0, 0, [0x81], 1, 0).unwrap().encode(),
        );
        target.expect_write(Packet::new(0, 0, [0x08], 1, 0).unwrap().encode());
        respond_fragmented_packet(
            &mut target,
            transaction_packet(TransactionMessage::ExitReply { result: 0 }, 1),
        );

        let (error, target) =
            match EpsonD4Session::connect_recoverable(target.into_transport(), spec()) {
                Ok(_) => panic!("malformed service reply unexpectedly opened a D4 session"),
                Err(recovery) => recovery,
            };

        assert!(matches!(error, ApplicationError::SetupRecovered { .. }));
        target.assert_finished();
    }
}
