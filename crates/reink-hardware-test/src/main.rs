#![forbid(unsafe_code)]

use std::process::ExitCode;
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use clap::{Parser, Subcommand};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
use reink_app::EepromWritePlan;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use reink_app::{EpsonD4EntryProbeResult, probe_epson_d4_entry};
#[cfg(target_os = "windows")]
use reink_app::{
    ReadOnlyEpsonD4Session, SelectedUsbSessionOutcome, with_selected_windows_native_epson_session,
};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use reink_core::PrinterIdentity;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
use reink_core::{EpsonSpec, ModelDatabase};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use reink_platform::RecordingTransport;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
use reink_platform::TransportEvent;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use reink_usb::read_printer_device_id;
use serde_json::{Value, json};

const TRACE_SANITIZATION_CONFIRMATION: &str = "I_CONFIRM_TRACE_IS_SANITIZED";
const WRITE_EVIDENCE_WRITE_CONFIRMATION: &str = "I_CONFIRM_THIS_WILL_WRITE_EEPROM";
const WRITE_EVIDENCE_RESTORATION_CONFIRMATION: &str =
    "I_CONFIRM_THIS_WILL_RESTORE_EEPROM_AND_RETAIN_PRIVATE_EVIDENCE";
const WRITE_EVIDENCE_REMEDIATION: &str = "Do not repeat this test or issue another write. Treat restoration as unverified, retain the private report and durable backup, reconnect or power-cycle the printer if needed, then verify the original byte with a separately confirmed read before further action.";
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", target_os = "windows", test)),
    allow(dead_code)
)]
const DRIVER_RECOVERY_REMEDIATION: &str = "Reconnect the printer, power-cycle it if needed, then reboot the host before retrying. This does not authorize a write, restore, or reset.";
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
const OUT_OF_RANGE_READ_CONFIRMATION: &str = "I_CONFIRM_THIS_IS_A_READ_ONLY_BOUNDARY_PROBE";

#[derive(Parser)]
#[command(
    name = "reink-hardware-test",
    about = "Opt-in ReInk hardware validation"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Convert an operator-reviewed trace into a local sanitized transcript template.
    TraceToTranscript {
        /// Existing local trace JSON produced by --trace-file.
        #[arg(long)]
        trace_file: PathBuf,
        /// New local Rust template path; this command refuses to overwrite.
        #[arg(long)]
        output_file: PathBuf,
        /// Exact acknowledgement that the trace was manually redacted and reviewed.
        #[arg(long)]
        confirmation: Option<String>,
        /// Generic fixture description placed in the generated template.
        #[arg(long, default_value = "sanitized fixture")]
        description: String,
    },
    /// List descriptor-only USB printer candidates without opening a device.
    UsbCandidates,
    /// List present read-only USBPRINT interfaces through the Windows stock driver.
    #[cfg(target_os = "windows")]
    WindowsNativeCandidates,
    /// Read D4 identity through the read-only Windows stock-driver backend.
    #[cfg(target_os = "windows")]
    WindowsNativeD4Identity {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        #[arg(long)]
        interface: Option<u8>,
        #[arg(long)]
        model: String,
    },
    /// Read status through the read-only Windows stock-driver backend.
    #[cfg(target_os = "windows")]
    WindowsNativeD4Status {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        #[arg(long)]
        interface: Option<u8>,
        #[arg(long)]
        model: String,
    },
    /// Read selected model-bounded EEPROM addresses through Windows USBPRINT.
    #[cfg(target_os = "windows")]
    WindowsNativeD4EepromRead {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        #[arg(long)]
        interface: Option<u8>,
        #[arg(long)]
        model: String,
        #[arg(long, required = true, value_parser = parse_u16)]
        address: Vec<u16>,
    },
    /// Read the complete model-bounded EEPROM range through Windows USBPRINT.
    #[cfg(target_os = "windows")]
    WindowsNativeD4EepromDump {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        #[arg(long)]
        interface: Option<u8>,
        #[arg(long)]
        model: String,
    },
    /// Run the currently supported read-only USB preflight sequence.
    ReadSequence {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        #[arg(long)]
        interface: u8,
        #[arg(long, default_value_t = 0)]
        alternate_setting: u8,
        #[arg(long)]
        bus_number: Option<u8>,
        #[arg(long)]
        device_address: Option<u8>,
        /// Omit the one-way D4 entry probe so a later D4 command starts a fresh session.
        #[arg(long)]
        skip_d4_entry_probe: bool,
    },
    /// Write and restore one explicitly selected EEPROM byte as private physical evidence.
    D4EepromWriteEvidence {
        /// USB vendor ID in decimal or 0x-prefixed hexadecimal.
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        /// USB product ID in decimal or 0x-prefixed hexadecimal.
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        /// Explicit USB printer interface number.
        #[arg(long)]
        interface: u8,
        /// Explicit alternate setting for the selected printer interface.
        #[arg(long, default_value_t = 0)]
        alternate_setting: u8,
        /// Explicit libusb bus number; this command never selects by VID/PID alone.
        #[arg(long)]
        bus_number: u8,
        /// Explicit libusb device address; this command never selects by VID/PID alone.
        #[arg(long)]
        device_address: u8,
        /// Model that must exactly match the D4 identity read from the selected printer.
        #[arg(long)]
        model: String,
        /// In-range EEPROM address in decimal or 0x-prefixed hexadecimal.
        #[arg(long, value_parser = parse_u16)]
        address: u16,
        /// Test byte in decimal or 0x-prefixed hexadecimal. It must differ from the pre-read byte.
        #[arg(long, value_parser = parse_u8)]
        value: u8,
        /// New complete private EEPROM backup path. Existing files are never overwritten.
        #[arg(long)]
        backup_file: PathBuf,
        /// New private structured report path. It is written only after cleanup.
        #[arg(long)]
        report_file: PathBuf,
        /// Exact acknowledgement required before this command opens USB.
        #[arg(long)]
        confirm_write: Option<String>,
        /// Exact acknowledgement that restoration and private evidence are required.
        #[arg(long)]
        confirm_restoration_evidence: Option<String>,
    },
    /// Run D4 Init, EPSON-CTRL identity read, orderly close, and Exit.
    D4Identity {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        #[arg(long)]
        interface: u8,
        #[arg(long, default_value_t = 0)]
        alternate_setting: u8,
        #[arg(long)]
        bus_number: Option<u8>,
        #[arg(long)]
        device_address: Option<u8>,
        #[arg(long)]
        model: String,
        /// Save a private, ordered D4 byte trace after cleanup; refuses to overwrite.
        #[arg(long)]
        trace_file: Option<PathBuf>,
        /// Save the structured report after cleanup; refuses to overwrite.
        #[arg(long)]
        report_file: Option<PathBuf>,
    },
    /// Read explicitly selected EEPROM addresses over a read-only D4 session.
    D4EepromRead {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        #[arg(long)]
        interface: u8,
        #[arg(long, default_value_t = 0)]
        alternate_setting: u8,
        #[arg(long)]
        bus_number: Option<u8>,
        #[arg(long)]
        device_address: Option<u8>,
        #[arg(long)]
        model: String,
        #[arg(long, required = true, value_parser = parse_u16)]
        address: Vec<u16>,
        /// Save a private, ordered D4 byte trace after cleanup; refuses to overwrite.
        #[arg(long)]
        trace_file: Option<PathBuf>,
        /// Save the structured report after cleanup; refuses to overwrite.
        #[arg(long)]
        report_file: Option<PathBuf>,
    },
    /// Read every EEPROM address in a model-bounded range without writing.
    D4EepromDump {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        #[arg(long)]
        interface: u8,
        #[arg(long, default_value_t = 0)]
        alternate_setting: u8,
        #[arg(long)]
        bus_number: Option<u8>,
        #[arg(long)]
        device_address: Option<u8>,
        #[arg(long)]
        model: String,
        /// First address to read; defaults to the model's declared lower bound.
        #[arg(long, value_parser = parse_u16)]
        start_address: Option<u16>,
        /// Last address to read; defaults to the model's declared upper bound.
        #[arg(long, value_parser = parse_u16)]
        end_address: Option<u16>,
        /// Save a private, ordered D4 byte trace after cleanup; refuses to overwrite.
        #[arg(long)]
        trace_file: Option<PathBuf>,
        /// Save the structured report after cleanup; refuses to overwrite.
        #[arg(long)]
        report_file: Option<PathBuf>,
    },
    /// Make one explicitly acknowledged, out-of-model-range EEPROM read.
    D4EepromBoundaryProbe {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        #[arg(long)]
        interface: u8,
        #[arg(long, default_value_t = 0)]
        alternate_setting: u8,
        #[arg(long)]
        bus_number: Option<u8>,
        #[arg(long)]
        device_address: Option<u8>,
        #[arg(long)]
        model: String,
        #[arg(long, value_parser = parse_u16)]
        address: u16,
        /// Exact acknowledgement required before this single read-only boundary probe.
        #[arg(long)]
        confirm_out_of_range_read: Option<String>,
        /// Save a private, ordered D4 byte trace after cleanup; refuses to overwrite.
        #[arg(long)]
        trace_file: Option<PathBuf>,
        /// Save the structured report after cleanup; refuses to overwrite.
        #[arg(long)]
        report_file: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<String, String> {
    match cli.command {
        Command::TraceToTranscript {
            trace_file,
            output_file,
            confirmation,
            description,
        } => trace_to_transcript(
            &trace_file,
            &output_file,
            confirmation.as_deref(),
            &description,
        ),
        Command::UsbCandidates => usb_candidates(),
        #[cfg(target_os = "windows")]
        Command::WindowsNativeCandidates => windows_native_candidates(),
        #[cfg(target_os = "windows")]
        Command::WindowsNativeD4Identity {
            vendor_id,
            product_id,
            interface,
            model,
        } => windows_native_d4_identity(vendor_id, product_id, interface, &model),
        #[cfg(target_os = "windows")]
        Command::WindowsNativeD4Status {
            vendor_id,
            product_id,
            interface,
            model,
        } => windows_native_d4_status(vendor_id, product_id, interface, &model),
        #[cfg(target_os = "windows")]
        Command::WindowsNativeD4EepromRead {
            vendor_id,
            product_id,
            interface,
            model,
            address,
        } => windows_native_d4_eeprom_read(vendor_id, product_id, interface, &model, &address),
        #[cfg(target_os = "windows")]
        Command::WindowsNativeD4EepromDump {
            vendor_id,
            product_id,
            interface,
            model,
        } => windows_native_d4_eeprom_dump(vendor_id, product_id, interface, &model),
        Command::ReadSequence {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            skip_d4_entry_probe,
        } => read_sequence(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            skip_d4_entry_probe,
        ),
        Command::D4EepromWriteEvidence {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            model,
            address,
            value,
            backup_file,
            report_file,
            confirm_write,
            confirm_restoration_evidence,
        } => d4_eeprom_write_evidence(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            &model,
            address,
            value,
            &backup_file,
            &report_file,
            confirm_write.as_deref(),
            confirm_restoration_evidence.as_deref(),
        ),
        Command::D4Identity {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            model,
            trace_file,
            report_file,
        } => d4_identity(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            &model,
            trace_file.as_deref(),
            report_file.as_deref(),
        ),
        Command::D4EepromRead {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            model,
            address,
            trace_file,
            report_file,
        } => d4_eeprom_read(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            &model,
            &address,
            trace_file.as_deref(),
            report_file.as_deref(),
        ),
        Command::D4EepromDump {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            model,
            start_address,
            end_address,
            trace_file,
            report_file,
        } => d4_eeprom_dump(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            &model,
            start_address,
            end_address,
            trace_file.as_deref(),
            report_file.as_deref(),
        ),
        Command::D4EepromBoundaryProbe {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            model,
            address,
            confirm_out_of_range_read,
            trace_file,
            report_file,
        } => d4_eeprom_boundary_probe(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            &model,
            address,
            confirm_out_of_range_read.as_deref(),
            trace_file.as_deref(),
            report_file.as_deref(),
        ),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
#[derive(Clone, Debug, Eq, PartialEq)]
struct WriteEvidenceStage {
    status: &'static str,
    detail: String,
    value: Option<u8>,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
impl WriteEvidenceStage {
    fn completed(detail: impl Into<String>, value: Option<u8>) -> Self {
        Self {
            status: "completed",
            detail: detail.into(),
            value,
        }
    }

    fn failed(detail: impl Into<String>) -> Self {
        Self {
            status: "failed",
            detail: detail.into(),
            value: None,
        }
    }

    fn skipped(detail: impl Into<String>) -> Self {
        Self {
            status: "skipped",
            detail: detail.into(),
            value: None,
        }
    }

    fn succeeded(&self) -> bool {
        self.status == "completed"
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
#[derive(Clone, Debug, Eq, PartialEq)]
struct WriteEvidenceOutcome {
    identity: WriteEvidenceStage,
    pre_read: WriteEvidenceStage,
    backup: WriteEvidenceStage,
    test_write: WriteEvidenceStage,
    test_readback: WriteEvidenceStage,
    restoration: WriteEvidenceStage,
    restoration_readback: WriteEvidenceStage,
    original_value: Option<u8>,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
impl WriteEvidenceOutcome {
    fn new() -> Self {
        Self {
            identity: WriteEvidenceStage::skipped("D4 identity has not been read"),
            pre_read: WriteEvidenceStage::skipped("original byte was not read"),
            backup: WriteEvidenceStage::skipped("complete backup was not created"),
            test_write: WriteEvidenceStage::skipped("test write was not attempted"),
            test_readback: WriteEvidenceStage::skipped("test write read-back was not attempted"),
            restoration: WriteEvidenceStage::skipped("restoration was not attempted"),
            restoration_readback: WriteEvidenceStage::skipped(
                "restoration read-back was not attempted",
            ),
            original_value: None,
        }
    }

    fn completed_safely(&self) -> bool {
        self.identity.succeeded()
            && self.pre_read.succeeded()
            && self.backup.succeeded()
            && self.test_write.succeeded()
            && self.test_readback.succeeded()
            && self.restoration.succeeded()
            && self.restoration_readback.succeeded()
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
trait WriteEvidenceSession {
    fn identity_model(&mut self) -> Result<Option<String>, String>;
    fn read_byte(&mut self, address: u16) -> Result<u8, String>;
    fn prepare_write_plan(&mut self, updates: &[(u16, u8)]) -> Result<EepromWritePlan, String>;
    fn apply_write_plan(&mut self, plan: &EepromWritePlan) -> Result<(), String>;
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl<T: reink_platform::ByteTransport> WriteEvidenceSession for reink_app::EpsonD4Session<T> {
    fn identity_model(&mut self) -> Result<Option<String>, String> {
        Ok(self
            .read_identity()
            .map_err(|error| error.to_string())?
            .detected_model()
            .map(str::to_owned))
    }

    fn read_byte(&mut self, address: u16) -> Result<u8, String> {
        self.read_eeprom(&[address])
            .map_err(|error| error.to_string())?
            .into_iter()
            .next()
            .map(|reply| reply.value)
            .ok_or_else(|| "single-byte EEPROM read returned no reply".to_owned())
    }

    fn prepare_write_plan(&mut self, updates: &[(u16, u8)]) -> Result<EepromWritePlan, String> {
        self.prepare_eeprom_write(updates)
            .map_err(|error| error.to_string())
    }

    fn apply_write_plan(&mut self, plan: &EepromWritePlan) -> Result<(), String> {
        self.apply_eeprom_write(plan)
            .map_err(|error| error.to_string())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn execute_write_evidence<S, F>(
    session: &mut S,
    model: &str,
    address: u16,
    requested_value: u8,
    persist_backup: F,
) -> WriteEvidenceOutcome
where
    S: WriteEvidenceSession,
    F: FnOnce(&[u8]) -> Result<(), String>,
{
    let mut outcome = WriteEvidenceOutcome::new();
    match session.identity_model() {
        Ok(Some(detected)) if detected == model => {
            outcome.identity = WriteEvidenceStage::completed(
                "D4 identity exactly matched the requested model",
                None,
            );
        }
        Ok(Some(detected)) => {
            outcome.identity = WriteEvidenceStage::failed(format!(
                "D4 identity model {detected:?} does not match requested model {model:?}"
            ));
            return outcome;
        }
        Ok(None) => {
            outcome.identity = WriteEvidenceStage::failed(format!(
                "D4 identity does not contain a model; requested model is {model:?}"
            ));
            return outcome;
        }
        Err(error) => {
            outcome.identity =
                WriteEvidenceStage::failed(format!("D4 identity read failed: {error}"));
            return outcome;
        }
    }

    let original_value = match session.read_byte(address) {
        Ok(value) => {
            outcome.pre_read =
                WriteEvidenceStage::completed("original byte read before any write", Some(value));
            outcome.original_value = Some(value);
            value
        }
        Err(error) => {
            outcome.pre_read =
                WriteEvidenceStage::failed(format!("original-byte pre-read failed: {error}"));
            return outcome;
        }
    };
    if original_value == requested_value {
        outcome.test_write = WriteEvidenceStage::failed(
            "requested test byte equals the pre-read original byte; refusing a non-evidentiary write",
        );
        return outcome;
    }

    let test_plan = match session.prepare_write_plan(&[(address, requested_value)]) {
        Ok(plan) => plan,
        Err(error) => {
            outcome.backup =
                WriteEvidenceStage::failed(format!("complete backup preparation failed: {error}"));
            return outcome;
        }
    };
    if test_plan.backup.value_at(address) != Some(original_value) {
        outcome.backup = WriteEvidenceStage::failed(
            "complete backup does not match the original-byte pre-read; refusing to write",
        );
        return outcome;
    }
    if let Err(error) = persist_backup(&test_plan.backup.bytes) {
        outcome.backup = WriteEvidenceStage::failed(format!(
            "durable complete backup persistence failed: {error}"
        ));
        return outcome;
    }
    outcome.backup = WriteEvidenceStage::completed(
        "complete backup was created with create-new semantics and synced before the test write",
        None,
    );

    match session.apply_write_plan(&test_plan) {
        Ok(()) => {
            outcome.test_write = WriteEvidenceStage::completed(
                "test write completed with core read-back verification",
                Some(requested_value),
            );
            match session.read_byte(address) {
                Ok(actual) if actual == requested_value => {
                    outcome.test_readback = WriteEvidenceStage::completed(
                        "independent read-back matched the requested test byte",
                        Some(actual),
                    );
                }
                Ok(actual) => {
                    outcome.test_readback = WriteEvidenceStage::failed(format!(
                        "independent test write read-back mismatch: expected {requested_value:#04x}, got {actual:#04x}"
                    ));
                }
                Err(error) => {
                    outcome.test_readback = WriteEvidenceStage::failed(format!(
                        "independent test write read-back failed: {error}"
                    ));
                }
            }
        }
        Err(error) => {
            outcome.test_write = WriteEvidenceStage::failed(format!(
                "test write or its core read-back verification failed: {error}"
            ));
        }
    }

    let restoration_plan = EepromWritePlan {
        backup: test_plan.backup.clone(),
        updates: vec![(address, original_value)],
    };
    match session.apply_write_plan(&restoration_plan) {
        Ok(()) => {
            outcome.restoration = WriteEvidenceStage::completed(
                "restoration write completed with core read-back verification",
                Some(original_value),
            );
            match session.read_byte(address) {
                Ok(actual) if actual == original_value => {
                    outcome.restoration_readback = WriteEvidenceStage::completed(
                        "independent restoration read-back matched the original byte",
                        Some(actual),
                    );
                }
                Ok(actual) => {
                    outcome.restoration_readback = WriteEvidenceStage::failed(format!(
                        "independent restoration read-back mismatch: expected {original_value:#04x}, got {actual:#04x}"
                    ));
                }
                Err(error) => {
                    outcome.restoration_readback = WriteEvidenceStage::failed(format!(
                        "independent restoration read-back failed: {error}"
                    ));
                }
            }
        }
        Err(error) => {
            outcome.restoration = WriteEvidenceStage::failed(format!(
                "restoration write or its core read-back verification failed: {error}"
            ));
        }
    }

    outcome
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SanitizedTraceEvent {
    is_write: bool,
    bytes: Vec<u8>,
}

fn trace_to_transcript(
    trace_file: &Path,
    output_file: &Path,
    confirmation: Option<&str>,
    description: &str,
) -> Result<String, String> {
    if confirmation != Some(TRACE_SANITIZATION_CONFIRMATION) {
        return Err(format!(
            "refusing to convert trace: pass --confirmation {TRACE_SANITIZATION_CONFIRMATION} exactly after manually redacting and reviewing it"
        ));
    }
    if description.trim().is_empty() {
        return Err("fixture description must not be empty".to_owned());
    }
    validate_new_file_path(output_file, "transcript template")?;
    let source = std::fs::read_to_string(trace_file).map_err(|error| {
        format!(
            "could not read trace file {}: {error}",
            trace_file.display()
        )
    })?;
    let events = parse_trace_events(&source)?;
    let template = transcript_template(description, &events);
    write_new_file(output_file, &template, "transcript template")?;
    Ok(format!(
        "local transcript template written to {}; review it, add assertions, and do not commit it until it has been reviewed",
        output_file.display()
    ))
}

fn parse_trace_events(source: &str) -> Result<Vec<SanitizedTraceEvent>, String> {
    let trace: Value =
        serde_json::from_str(source).map_err(|error| format!("invalid trace JSON: {error}"))?;
    let object = trace
        .as_object()
        .ok_or_else(|| "invalid trace schema: root must be an object".to_owned())?;
    for required in ["schema_version", "mode", "command", "events"] {
        if !object.contains_key(required) {
            return Err(format!("invalid trace schema: missing {required}"));
        }
    }
    if object.len() != 4 {
        return Err("invalid trace schema: root has unexpected fields".to_owned());
    }
    if object.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err("invalid trace schema: schema_version must be 1".to_owned());
    }
    if object.get("mode").and_then(Value::as_str) != Some("read_only") {
        return Err("invalid trace schema: mode must be read_only".to_owned());
    }
    if object
        .get("command")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err("invalid trace schema: command must be a non-empty string".to_owned());
    }
    let events = object
        .get("events")
        .and_then(Value::as_array)
        .ok_or_else(|| "invalid trace schema: events must be an array".to_owned())?;

    events
        .iter()
        .enumerate()
        .map(|(index, event)| parse_trace_event(index, event))
        .collect()
}

fn parse_trace_event(index: usize, event: &Value) -> Result<SanitizedTraceEvent, String> {
    let object = event
        .as_object()
        .ok_or_else(|| format!("invalid trace event {index}: must be an object"))?;
    if object.len() != 2 || !object.contains_key("direction") || !object.contains_key("bytes") {
        return Err(format!(
            "invalid trace event {index}: expected only direction and bytes"
        ));
    }
    let is_write = match object.get("direction").and_then(Value::as_str) {
        Some("tx") => true,
        Some("rx") => false,
        _ => {
            return Err(format!(
                "invalid trace event {index}: direction must be tx or rx"
            ));
        }
    };
    let hex = object
        .get("bytes")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("invalid trace event {index}: bytes must be a string"))?;
    if hex.len() % 2 != 0 {
        return Err(format!(
            "invalid trace event {index}: bytes must contain an even number of hexadecimal characters"
        ));
    }
    if !hex
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'A'..=b'F'))
    {
        return Err(format!(
            "invalid trace event {index}: bytes must be uppercase hexadecimal"
        ));
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|offset| {
            u8::from_str_radix(&hex[offset..offset + 2], 16).map_err(|_| {
                format!("invalid trace event {index}: bytes must be valid hexadecimal")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SanitizedTraceEvent { is_write, bytes })
}

fn transcript_template(description: &str, events: &[SanitizedTraceEvent]) -> String {
    let escaped_description = format!("{description:?}");
    let mut template = String::from(
        "// Local template only. The operator confirmed this evidence was manually sanitized.\n\
    // Review every byte, add behavior assertions, and do not commit this template without review.\n\
    let mut transcript = SanitizedTranscript::new(",
    );
    template.push_str(&escaped_description);
    template.push_str(");\n");
    for event in events {
        let bytes = event
            .bytes
            .iter()
            .map(|byte| format!("0x{byte:02X}"))
            .collect::<Vec<_>>()
            .join(", ");
        let method = if event.is_write {
            "expect_write"
        } else {
            "respond"
        };
        template.push_str(&format!("transcript.{method}(vec![{bytes}]);\n"));
    }
    template.push_str("// Add assertions for the behavior this transcript protects.\n");
    template
}

fn validate_new_file_path(path: &Path, kind: &str) -> Result<(), String> {
    if path.exists() {
        return Err(format!(
            "refusing to overwrite existing {kind} file: {}",
            path.display()
        ));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.is_dir()
    {
        return Err(format!(
            "{kind} file parent directory does not exist: {}",
            parent.display()
        ));
    }
    Ok(())
}

fn write_new_file(path: &Path, contents: &str, kind: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("could not create {kind} file {}: {error}", path.display()))?;
    file.write_all(contents.as_bytes())
        .map_err(|error| format!("could not write {kind} file {}: {error}", path.display()))
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn trace_json(command: &str, events: &[TransportEvent]) -> Value {
    let events = events
        .iter()
        .map(|event| match event {
            TransportEvent::Tx(bytes) => json!({
                "direction": "tx",
                "bytes": bytes.iter().map(|byte| format!("{byte:02X}")).collect::<String>(),
            }),
            TransportEvent::Rx(bytes) => json!({
                "direction": "rx",
                "bytes": bytes.iter().map(|byte| format!("{byte:02X}")).collect::<String>(),
            }),
        })
        .collect::<Vec<_>>();
    json!({
        "schema_version": 1,
        "mode": "read_only",
        "command": command,
        "events": events,
    })
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn validate_trace_file_path(path: &Path) -> Result<(), String> {
    validate_private_new_file_path(path, "trace")
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn validate_report_file_path(path: &Path) -> Result<(), String> {
    validate_private_new_file_path(path, "report")
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn validate_private_new_file_path(path: &Path, kind: &str) -> Result<(), String> {
    if path.exists() {
        return Err(format!(
            "refusing to overwrite existing private {kind} file: {}",
            path.display(),
        ));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.is_dir()
    {
        return Err(format!(
            "private {kind} file parent directory does not exist: {}",
            parent.display(),
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn write_report_file(path: &Path, report: &str) -> Result<(), String> {
    validate_report_file_path(path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            format!(
                "could not create private report file {}: {error}",
                path.display()
            )
        })?;
    file.write_all(report.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            format!(
                "could not persist private report file {}: {error}",
                path.display()
            )
        })
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn write_new_private_binary_file(path: &Path, bytes: &[u8], kind: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            format!(
                "could not create private {kind} {}: {error}",
                path.display()
            )
        })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            format!(
                "could not persist private {kind} {}: {error}",
                path.display()
            )
        })
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn normalized_new_file_path(path: &Path, kind: &str) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let parent = parent.unwrap_or_else(|| Path::new("."));
    let parent = parent.canonicalize().map_err(|error| {
        format!(
            "could not resolve private {kind} parent directory {}: {error}",
            parent.display()
        )
    })?;
    let file_name = path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("private {kind} path must name a file: {}", path.display()))?;
    Ok(parent.join(file_name))
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn validate_write_evidence_gates(
    spec: &EpsonSpec,
    address: u16,
    backup_file: &Path,
    report_file: &Path,
    write_confirmation: Option<&str>,
    restoration_confirmation: Option<&str>,
) -> Result<(), String> {
    if write_confirmation != Some(WRITE_EVIDENCE_WRITE_CONFIRMATION) {
        return Err(format!(
            "d4-eeprom-write-evidence requires --confirm-write {WRITE_EVIDENCE_WRITE_CONFIRMATION} exactly"
        ));
    }
    if restoration_confirmation != Some(WRITE_EVIDENCE_RESTORATION_CONFIRMATION) {
        return Err(format!(
            "d4-eeprom-write-evidence requires --confirm-restoration-evidence {WRITE_EVIDENCE_RESTORATION_CONFIRMATION} exactly"
        ));
    }
    validate_eeprom_read_addresses(spec, &[address])?;
    validate_private_new_file_path(backup_file, "complete EEPROM backup")?;
    validate_report_file_path(report_file)?;
    if normalized_new_file_path(backup_file, "complete EEPROM backup")?
        == normalized_new_file_path(report_file, "write-evidence report")?
    {
        return Err(
            "complete EEPROM backup and write-evidence report must use different create-new paths"
                .to_owned(),
        );
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn emit_report(report: String, report_file: Option<&Path>) -> Result<String, String> {
    if let Some(path) = report_file {
        write_report_file(path, &report)?;
    }
    Ok(report)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", target_os = "windows")),
    allow(dead_code)
)]
fn fail_with_report(
    report: String,
    primary_error: String,
    report_file: Option<&Path>,
) -> Result<String, String> {
    match report_file {
        None => Err(format!("{primary_error}; {DRIVER_RECOVERY_REMEDIATION}")),
        Some(path) => match write_report_file(path, &report) {
            Ok(()) => Err(format!("{primary_error}; {DRIVER_RECOVERY_REMEDIATION}")),
            Err(report_error) => Err(format!(
                "{primary_error}; {report_error}; {DRIVER_RECOVERY_REMEDIATION}"
            )),
        },
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn driver_recovery_error(error: impl std::fmt::Display) -> String {
    format!("{error}; {DRIVER_RECOVERY_REMEDIATION}")
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn write_trace_file(path: &Path, command: &str, events: &[TransportEvent]) -> Result<(), String> {
    validate_trace_file_path(path)?;
    let contents = serde_json::to_vec(&trace_json(command, events))
        .map_err(|error| format!("could not encode private trace: {error}"))?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            format!(
                "could not create private trace file {}: {error}",
                path.display()
            )
        })?;
    file.write_all(&contents).map_err(|error| {
        format!(
            "could not write private trace file {}: {error}",
            path.display()
        )
    })
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn finish_trace<T>(
    mut operation: D4Operation<T>,
    trace_file: Option<&Path>,
    command: &str,
) -> D4Operation<T> {
    let trace_result = trace_file.map(|path| {
        write_trace_file(path, command, &operation.events)
            .map(|()| format!("incomplete private trace saved to {}", path.display()))
    });

    match (&mut operation.result, trace_result) {
        (Ok(_), None | Some(Ok(_))) => {}
        (result @ Ok(_), Some(Err(error))) => *result = Err(error),
        (Err(error), Some(Ok(trace_message))) => {
            *error = format!("{error}; {trace_message}");
        }
        (Err(error), Some(Err(trace_error))) => {
            *error = format!("{error}; private trace could not be saved: {trace_error}");
        }
        (Err(_), None) => {}
    }
    operation
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn usb_candidates() -> Result<String, String> {
    let database = ModelDatabase::builtin().map_err(|error| error.to_string())?;
    let candidates = reink_usb::list_printer_candidates().map_err(|error| error.to_string())?;
    Ok(usb_candidates_report(&candidates, &database))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn usb_candidates() -> Result<String, String> {
    Err(
        "hardware USB validation is currently supported only on Linux, macOS, or Windows"
            .to_owned(),
    )
}

#[cfg(target_os = "windows")]
fn selected_windows_native_candidate(
    vendor_id: u16,
    product_id: u16,
    interface: Option<u8>,
) -> Result<reink_usb::WindowsNativePrinterCandidate, String> {
    let candidates =
        reink_usb::list_windows_native_printer_candidates().map_err(|error| error.to_string())?;
    let selector = interface.map_or_else(
        || reink_usb::NativePrinterSelector::new(vendor_id, product_id),
        |number| reink_usb::NativePrinterSelector::with_interface(vendor_id, product_id, number),
    );
    reink_usb::select_native_candidate(&candidates, selector).map_err(|error| error.to_string())
}

#[cfg(target_os = "windows")]
fn finish_windows_native_hardware_operation<T>(
    outcome: SelectedUsbSessionOutcome<T>,
) -> Result<T, String> {
    let mut errors = Vec::new();
    let result = match outcome.operation {
        Ok(value) => Some(value),
        Err(error) => {
            errors.push(error);
            None
        }
    };
    if let reink_app::UsbCleanupStatus::Failed(error) = outcome.cleanup.d4_shutdown {
        errors.push(format!("D4 shutdown failed: {error}"));
    }
    if let reink_app::UsbCleanupStatus::Failed(error) = outcome.cleanup.usb_close {
        errors.push(format!(
            "Windows stock-driver transport close failed: {error}"
        ));
    }
    if errors.is_empty() {
        Ok(result.expect("successful native operation retains its result"))
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(target_os = "windows")]
fn with_windows_native_hardware_session<T>(
    vendor_id: u16,
    product_id: u16,
    interface: Option<u8>,
    model: &str,
    operation: impl FnOnce(
        &mut ReadOnlyEpsonD4Session<
            '_,
            RecordingTransport<reink_usb::WindowsNativeReadOnlyTransport>,
        >,
    ) -> Result<T, String>,
) -> Result<T, String> {
    let candidate = selected_windows_native_candidate(vendor_id, product_id, interface)?;
    let spec = ModelDatabase::builtin()
        .map_err(|error| error.to_string())?
        .get(model)
        .cloned()
        .ok_or_else(|| format!("unknown model: {model}"))?;
    finish_windows_native_hardware_operation(with_selected_windows_native_epson_session(
        &candidate, spec, false, operation,
    ))
}

#[cfg(target_os = "windows")]
fn windows_native_candidates() -> Result<String, String> {
    let database = ModelDatabase::builtin().map_err(|error| error.to_string())?;
    let candidates =
        reink_usb::list_windows_native_printer_candidates().map_err(|error| error.to_string())?;
    let candidates = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let hints = database
                .models()
                .filter(|model| {
                    database.get(model).is_some_and(|spec| {
                        spec.vendor_id == candidate.vendor_id
                            && spec.product_id == Some(candidate.product_id)
                    })
                })
                .collect::<Vec<_>>();
            json!({
                "alias": format!("windows-native-{}", index + 1),
                "backend": "windows_native_usbprint",
                "selector": {
                    "vendor_id": format!("{:04x}", candidate.vendor_id),
                    "product_id": format!("{:04x}", candidate.product_id),
                    "interface": candidate.interface_number,
                },
                "model_hints": hints,
                "capabilities": {
                    "d4_read": true,
                    "usb_device_id": false,
                    "persistent_mutation": false,
                },
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "schema_version": 4,
        "mode": "read_only",
        "command": "windows-native-candidates",
        "candidates": candidates,
    })
    .to_string())
}

#[cfg(target_os = "windows")]
fn windows_native_d4_identity(
    vendor_id: u16,
    product_id: u16,
    interface: Option<u8>,
    model: &str,
) -> Result<String, String> {
    let identity =
        with_windows_native_hardware_session(vendor_id, product_id, interface, model, |session| {
            let identity = session.read_identity().map_err(|error| error.to_string())?;
            reink_app::verify_exact_model(&identity, model)?;
            Ok(identity)
        })?;
    Ok(json!({
        "schema_version": 4,
        "mode": "read_only",
        "backend": "windows_native_usbprint",
        "command": "windows-native-d4-identity",
        "identity": {
            "manufacturer": identity.manufacturer(),
            "model": identity.model(),
            "command_set": identity.command_set(),
        },
    })
    .to_string())
}

#[cfg(target_os = "windows")]
fn windows_native_d4_status(
    vendor_id: u16,
    product_id: u16,
    interface: Option<u8>,
    model: &str,
) -> Result<String, String> {
    let mut status =
        with_windows_native_hardware_session(vendor_id, product_id, interface, model, |session| {
            let identity = session.read_identity().map_err(|error| error.to_string())?;
            reink_app::verify_exact_model(&identity, model)?;
            session.read_status().map_err(|error| error.to_string())
        })?;
    reink_usb::redact_identity_serial_fields(&mut status);
    Ok(json!({
        "schema_version": 4,
        "mode": "read_only",
        "backend": "windows_native_usbprint",
        "command": "windows-native-d4-status",
        "model": model,
        "response_bytes": status.len(),
        "status_hex": status.iter().map(|byte| format!("{byte:02X}")).collect::<String>(),
    })
    .to_string())
}

#[cfg(target_os = "windows")]
fn windows_native_d4_eeprom_read(
    vendor_id: u16,
    product_id: u16,
    interface: Option<u8>,
    model: &str,
    addresses: &[u16],
) -> Result<String, String> {
    let spec = ModelDatabase::builtin()
        .map_err(|error| error.to_string())?
        .get(model)
        .cloned()
        .ok_or_else(|| format!("unknown model: {model}"))?;
    validate_eeprom_read_addresses(&spec, addresses)?;
    validate_windows_native_sensitive_addresses(&spec, addresses)?;
    let readings =
        with_windows_native_hardware_session(vendor_id, product_id, interface, model, |session| {
            let identity = session.read_identity().map_err(|error| error.to_string())?;
            reink_app::verify_exact_model(&identity, model)?;
            session
                .read_eeprom(addresses)
                .map_err(|error| error.to_string())
        })?;
    Ok(json!({
        "schema_version": 4,
        "mode": "read_only",
        "backend": "windows_native_usbprint",
        "command": "windows-native-d4-eeprom-read",
        "model": model,
        "readings": readings.iter().map(|reading| json!({
            "address": format!("{:04X}", reading.address),
            "value": reading.value,
        })).collect::<Vec<_>>(),
    })
    .to_string())
}

#[cfg(target_os = "windows")]
fn validate_windows_native_sensitive_addresses(
    spec: &EpsonSpec,
    addresses: &[u16],
) -> Result<(), String> {
    if let Some(address) = addresses.iter().copied().find(|address| {
        spec.read_only_fields
            .iter()
            .any(|field| field.sensitive && (field.address..=field.end_address).contains(address))
    }) {
        return Err(format!(
            "EEPROM address {address:#06x} is part of a sensitive identity field and cannot be included in a Windows native report"
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_native_d4_eeprom_dump(
    vendor_id: u16,
    product_id: u16,
    interface: Option<u8>,
    model: &str,
) -> Result<String, String> {
    let image =
        with_windows_native_hardware_session(vendor_id, product_id, interface, model, |session| {
            let identity = session.read_identity().map_err(|error| error.to_string())?;
            reink_app::verify_exact_model(&identity, model)?;
            session.dump_eeprom().map_err(|error| error.to_string())
        })?;
    Ok(json!({
        "schema_version": 4,
        "mode": "read_only",
        "backend": "windows_native_usbprint",
        "command": "windows-native-d4-eeprom-dump",
        "model": image.model,
        "start_address": format!("{:04X}", image.start_address),
        "end_address": format!("{:04X}", image.end_address()),
        "byte_count": image.bytes.len(),
        "data_retained": false,
        "note": "EEPROM bytes are intentionally omitted from this report; use the CLI native dump command to save a private binary image.",
    })
    .to_string())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
struct D4Operation<T> {
    result: Result<T, String>,
    events: Vec<TransportEvent>,
    driver_handoff: DriverHandoffReport,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn finish_d4_operation<T, E: std::fmt::Display>(
    mut session: reink_app::EpsonD4Session<RecordingTransport<reink_usb::ReadOnlyUsbTransport>>,
    operation: Result<T, E>,
) -> D4Operation<T> {
    let shutdown = session.shutdown();
    let (mut transport, events) = session.into_transport().into_parts();
    let close = transport.close();
    if close.is_err() {
        // Match Drop's best-effort retry before recording the final lifecycle.
        let _ = transport.close();
    }
    let driver_handoff = DriverHandoffReport::from_usb(transport.driver_handoff_outcome());

    let result = match operation {
        Ok(value) => match (shutdown, close) {
            (Ok(()), Ok(())) => Ok(value),
            (Err(error), Ok(())) => Err(format!("D4 shutdown failed: {error}")),
            (Ok(()), Err(error)) => Err(format!("USB transport close failed: {error}")),
            (Err(shutdown), Err(close)) => Err(format!(
                "D4 shutdown failed: {shutdown}; USB transport close also failed: {close}"
            )),
        },
        Err(operation) => {
            let mut errors = vec![format!("D4 operation failed: {operation}")];
            if let Err(error) = shutdown {
                errors.push(format!("D4 shutdown also failed: {error}"));
            }
            if let Err(error) = close {
                errors.push(format!("USB transport close also failed: {error}"));
            }
            Err(errors.join("; "))
        }
    };
    D4Operation {
        result,
        events,
        driver_handoff,
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
#[derive(Clone, Debug, Eq, PartialEq)]
struct WriteEvidenceCleanup {
    d4_shutdown: WriteEvidenceStage,
    usb_close: WriteEvidenceStage,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
impl WriteEvidenceCleanup {
    fn not_started() -> Self {
        Self {
            d4_shutdown: WriteEvidenceStage::skipped("D4 session was not established"),
            usb_close: WriteEvidenceStage::skipped("USB transport was not opened"),
        }
    }

    fn succeeded(&self) -> bool {
        self.d4_shutdown.succeeded() && self.usb_close.succeeded()
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn write_evidence_stage_json(name: &str, stage: &WriteEvidenceStage) -> Value {
    let mut value = json!({
        "name": name,
        "status": stage.status,
        "detail": stage.detail,
    });
    if let Some(byte) = stage.value {
        value["value"] = json!(format!("{byte:02X}"));
    }
    value
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
#[allow(clippy::too_many_arguments)]
fn write_evidence_report(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: u8,
    device_address: u8,
    model: &str,
    address: u16,
    requested_value: u8,
    backup_file: &Path,
    connection: &WriteEvidenceStage,
    outcome: Option<&WriteEvidenceOutcome>,
    cleanup: &WriteEvidenceCleanup,
    driver_handoff: DriverHandoffReport,
) -> String {
    let mut stages = vec![write_evidence_stage_json("d4-session-connect", connection)];
    if let Some(outcome) = outcome {
        stages.extend([
            write_evidence_stage_json("identity-confirmation", &outcome.identity),
            write_evidence_stage_json("original-byte-pre-read", &outcome.pre_read),
            write_evidence_stage_json("complete-backup", &outcome.backup),
            write_evidence_stage_json("test-write", &outcome.test_write),
            write_evidence_stage_json("test-write-readback", &outcome.test_readback),
            write_evidence_stage_json("restoration", &outcome.restoration),
            write_evidence_stage_json("restoration-readback", &outcome.restoration_readback),
        ]);
    }
    stages.extend([
        write_evidence_stage_json("d4-session-shutdown", &cleanup.d4_shutdown),
        write_evidence_stage_json("usb-close-and-driver-handoff", &cleanup.usb_close),
    ]);
    let completed_safely = connection.succeeded()
        && outcome.is_some_and(WriteEvidenceOutcome::completed_safely)
        && cleanup.succeeded();
    json!({
        "schema_version": 1,
        "mode": "write_evidence",
        "command": "d4-eeprom-write-evidence",
        "status": if completed_safely { "completed" } else { "failed" },
        "selector": {
            "vendor_id": format!("{vendor_id:04X}"),
            "product_id": format!("{product_id:04X}"),
            "interface": interface,
            "alternate_setting": alternate_setting,
            "bus_number": bus_number,
            "device_address": device_address,
        },
        "model": model,
        "test": {
            "address": format!("{address:04X}"),
            "requested_value": format!("{requested_value:02X}"),
            "original_value": outcome.and_then(|outcome| outcome.original_value).map(|value| format!("{value:02X}")),
        },
        "backup_file": backup_file,
        "linux_driver_handoff": driver_handoff.json(),
        "stages": stages,
        "remediation": (!completed_safely).then_some(WRITE_EVIDENCE_REMEDIATION),
        "next_step": if completed_safely {
            "The selected byte was restored and independently verified. Retain this private evidence; do not issue any automatic follow-up write."
        } else {
            "Use the private stage outcomes to remediate the failure before any further device interaction."
        },
    })
    .to_string()
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn close_write_evidence_transport(
    transport: RecordingTransport<reink_usb::ReadOnlyUsbTransport>,
) -> (WriteEvidenceStage, DriverHandoffReport) {
    let (mut transport, _) = transport.into_parts();
    let close = transport.close();
    if close.is_err() {
        // Retry only cleanup, matching the normal D4 lifecycle's best-effort Drop behavior.
        let _ = transport.close();
    }
    let driver_handoff = DriverHandoffReport::from_usb(transport.driver_handoff_outcome());
    let stage = match close {
        Ok(()) => {
            WriteEvidenceStage::completed("USB interface released after the D4 session", None)
        }
        Err(error) => WriteEvidenceStage::failed(format!(
            "USB close or Linux driver reattachment failed: {error}"
        )),
    };
    (stage, driver_handoff)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn finish_write_evidence_session(
    mut session: reink_app::EpsonD4Session<RecordingTransport<reink_usb::ReadOnlyUsbTransport>>,
) -> (WriteEvidenceCleanup, DriverHandoffReport) {
    let d4_shutdown = match session.shutdown() {
        Ok(()) => WriteEvidenceStage::completed("D4 service closed and Exit completed", None),
        Err(error) => WriteEvidenceStage::failed(format!("D4 shutdown failed: {error}")),
    };
    let (usb_close, driver_handoff) = close_write_evidence_transport(session.into_transport());
    (
        WriteEvidenceCleanup {
            d4_shutdown,
            usb_close,
        },
        driver_handoff,
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn finish_write_evidence_report(
    report_file: &Path,
    report: String,
    completed_safely: bool,
) -> Result<String, String> {
    write_report_file(report_file, &report).map_err(|error| {
        format!(
            "write-evidence cleanup completed but the private report could not be persisted: {error}; {WRITE_EVIDENCE_REMEDIATION}"
        )
    })?;
    if completed_safely {
        Ok(
            "Write-evidence completed and the private report was persisted after cleanup."
                .to_owned(),
        )
    } else {
        Err(format!(
            "Write-evidence did not complete safely; the private report was persisted after cleanup. {WRITE_EVIDENCE_REMEDIATION}"
        ))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[allow(clippy::too_many_arguments)]
fn d4_eeprom_write_evidence(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: u8,
    device_address: u8,
    model: &str,
    address: u16,
    requested_value: u8,
    backup_file: &Path,
    report_file: &Path,
    write_confirmation: Option<&str>,
    restoration_confirmation: Option<&str>,
) -> Result<String, String> {
    let spec = ModelDatabase::builtin()
        .map_err(|error| error.to_string())?
        .get(model)
        .cloned()
        .ok_or_else(|| format!("unknown model: {model}"))?;
    validate_write_evidence_gates(
        &spec,
        address,
        backup_file,
        report_file,
        write_confirmation,
        restoration_confirmation,
    )?;

    let automatic_handoff = DriverHandoffReport::automatic();
    let transport = match reink_usb::ReadOnlyUsbTransport::open(
        reink_usb::UsbDeviceSelector::at_location(
            vendor_id,
            product_id,
            bus_number,
            device_address,
        ),
        reink_platform::UsbInterfaceSelector {
            number: interface,
            alternate_setting,
        },
    ) {
        Ok(transport) => transport,
        Err(error) => {
            let connection =
                WriteEvidenceStage::failed(format!("USB transport open failed: {error}"));
            let cleanup = WriteEvidenceCleanup::not_started();
            let outcome = WriteEvidenceOutcome::new();
            let report = write_evidence_report(
                vendor_id,
                product_id,
                interface,
                alternate_setting,
                bus_number,
                device_address,
                model,
                address,
                requested_value,
                backup_file,
                &connection,
                Some(&outcome),
                &cleanup,
                automatic_handoff,
            );
            return finish_write_evidence_report(report_file, report, false);
        }
    };

    match reink_app::EpsonD4Session::connect_recoverable(RecordingTransport::new(transport), spec) {
        Ok(mut session) => {
            let connection =
                WriteEvidenceStage::completed("D4 initialized and EPSON-CTRL opened", None);
            let outcome =
                execute_write_evidence(&mut session, model, address, requested_value, |bytes| {
                    write_new_private_binary_file(backup_file, bytes, "complete EEPROM backup")
                });
            let (cleanup, driver_handoff) = finish_write_evidence_session(session);
            let completed_safely = outcome.completed_safely() && cleanup.succeeded();
            let report = write_evidence_report(
                vendor_id,
                product_id,
                interface,
                alternate_setting,
                bus_number,
                device_address,
                model,
                address,
                requested_value,
                backup_file,
                &connection,
                Some(&outcome),
                &cleanup,
                driver_handoff,
            );
            finish_write_evidence_report(report_file, report, completed_safely)
        }
        Err((error, transport)) => {
            let connection = WriteEvidenceStage::failed(format!(
                "D4 session setup failed before any EEPROM access: {error}"
            ));
            let (usb_close, driver_handoff) = close_write_evidence_transport(transport);
            let cleanup = WriteEvidenceCleanup {
                d4_shutdown: WriteEvidenceStage::skipped("D4 session setup did not complete"),
                usb_close,
            };
            let outcome = WriteEvidenceOutcome::new();
            let report = write_evidence_report(
                vendor_id,
                product_id,
                interface,
                alternate_setting,
                bus_number,
                device_address,
                model,
                address,
                requested_value,
                backup_file,
                &connection,
                Some(&outcome),
                &cleanup,
                driver_handoff,
            );
            finish_write_evidence_report(report_file, report, false)
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[allow(clippy::too_many_arguments)]
fn d4_eeprom_write_evidence(
    _: u16,
    _: u16,
    _: u8,
    _: u8,
    _: u8,
    _: u8,
    _: &str,
    _: u16,
    _: u8,
    _: &Path,
    _: &Path,
    _: Option<&str>,
    _: Option<&str>,
) -> Result<String, String> {
    Err(
        "hardware USB validation is currently supported only on Linux, macOS, or Windows"
            .to_owned(),
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn run_d4_operation<T, E: std::fmt::Display, F>(
    transport: reink_usb::ReadOnlyUsbTransport,
    spec: reink_core::EpsonSpec,
    operation: F,
) -> D4Operation<T>
where
    F: FnOnce(
        &mut reink_app::EpsonD4Session<RecordingTransport<reink_usb::ReadOnlyUsbTransport>>,
    ) -> Result<T, E>,
{
    match reink_app::EpsonD4Session::connect_recoverable(RecordingTransport::new(transport), spec) {
        Ok(mut session) => {
            let operation = operation(&mut session);
            finish_d4_operation(session, operation)
        }
        Err((error, transport)) => {
            let (mut transport, events) = transport.into_parts();
            let close = transport.close();
            if close.is_err() {
                // Match Drop's best-effort retry before recording the final lifecycle.
                let _ = transport.close();
            }
            let result = match close {
                Ok(()) => Err(format!("D4 session setup failed: {error}")),
                Err(close) => Err(format!(
                    "D4 session setup failed: {error}; USB transport close also failed: {close}"
                )),
            };
            D4Operation {
                result,
                events,
                driver_handoff: DriverHandoffReport::from_usb(transport.driver_handoff_outcome()),
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[allow(clippy::too_many_arguments)]
fn d4_eeprom_read(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: Option<u8>,
    device_address: Option<u8>,
    model: &str,
    addresses: &[u16],
    trace_file: Option<&Path>,
    report_file: Option<&Path>,
) -> Result<String, String> {
    if let Some(path) = trace_file {
        validate_trace_file_path(path)?;
    }
    if let Some(path) = report_file {
        validate_report_file_path(path)?;
    }
    let spec = ModelDatabase::builtin()
        .map_err(|e| e.to_string())?
        .get(model)
        .ok_or_else(|| format!("unknown model: {model}"))?
        .clone();
    validate_eeprom_read_addresses(&spec, addresses)?;
    let automatic_handoff = DriverHandoffReport::automatic();
    let transport = match reink_usb::ReadOnlyUsbTransport::open(
        usb_device_selector(vendor_id, product_id, bus_number, device_address)?,
        reink_platform::UsbInterfaceSelector {
            number: interface,
            alternate_setting,
        },
    ) {
        Ok(transport) => transport,
        Err(error) => {
            let primary = format!("USB transport open failed: {error}");
            return fail_with_report(
                d4_failure_report(
                    "d4-eeprom-read",
                    automatic_handoff,
                    "usb-open",
                    &primary,
                    None,
                ),
                primary,
                report_file,
            );
        }
    };
    let operation = finish_trace(
        run_d4_operation(transport, spec, |session| session.read_eeprom(addresses)),
        trace_file,
        "d4-eeprom-read",
    );
    match operation.result {
        Ok(values) => emit_report(
            d4_eeprom_read_report(
                values
            .iter()
            .map(|value| json!({"address": format!("{:04X}", value.address), "value": value.value}))
            .collect(),
                operation.driver_handoff,
            ),
            report_file,
        ),
        Err(error) => fail_with_report(
            d4_failure_report(
                "d4-eeprom-read",
                operation.driver_handoff,
                "d4-eeprom-read",
                &error,
                None,
            ),
            error,
            report_file,
        ),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[allow(clippy::too_many_arguments)]
fn d4_eeprom_dump(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: Option<u8>,
    device_address: Option<u8>,
    model: &str,
    start_address: Option<u16>,
    end_address: Option<u16>,
    trace_file: Option<&Path>,
    report_file: Option<&Path>,
) -> Result<String, String> {
    if let Some(path) = trace_file {
        validate_trace_file_path(path)?;
    }
    if let Some(path) = report_file {
        validate_report_file_path(path)?;
    }
    let spec = ModelDatabase::builtin()
        .map_err(|e| e.to_string())?
        .get(model)
        .ok_or_else(|| format!("unknown model: {model}"))?
        .clone();
    let addresses = eeprom_dump_addresses(&spec, start_address, end_address)?;
    let first_address = *addresses
        .first()
        .expect("validated EEPROM dump ranges are never empty");
    let last_address = *addresses
        .last()
        .expect("validated EEPROM dump ranges are never empty");
    let automatic_handoff = DriverHandoffReport::automatic();
    let transport = match reink_usb::ReadOnlyUsbTransport::open(
        usb_device_selector(vendor_id, product_id, bus_number, device_address)?,
        reink_platform::UsbInterfaceSelector {
            number: interface,
            alternate_setting,
        },
    ) {
        Ok(transport) => transport,
        Err(error) => {
            let primary = format!("USB transport open failed: {error}");
            return fail_with_report(
                d4_failure_report(
                    "d4-eeprom-dump",
                    automatic_handoff,
                    "usb-open",
                    &primary,
                    Some(dump_progress(0, None)),
                ),
                primary,
                report_file,
            );
        }
    };
    let mut dump_failure = None;
    let mut completed_address_count = 0;
    let operation = finish_trace(
        run_d4_operation(transport, spec, |session| {
            let mut values = Vec::with_capacity(addresses.len());
            for &address in &addresses {
                match session.read_eeprom(&[address]) {
                    Ok(mut read) => {
                        values.push(read.remove(0));
                        completed_address_count = values.len();
                    }
                    Err(read_error) => {
                        dump_failure = Some((values.len(), address));
                        return Err(format!(
                            "EEPROM dump failed after {} successful reads; failed address {address:#06x}: {read_error}",
                            values.len()
                        ));
                    }
                }
            }
            Ok(values)
        }),
        trace_file,
        "d4-eeprom-dump",
    );
    match operation.result {
        Ok(values) => emit_report(
            d4_eeprom_dump_report(
                model,
                first_address,
                last_address,
                values
            .iter()
            .map(|value| json!({"address": format!("{:04X}", value.address), "value": value.value}))
            .collect(),
                operation.driver_handoff,
            ),
            report_file,
        ),
        Err(error) => fail_with_report(
            d4_failure_report(
                "d4-eeprom-dump",
                operation.driver_handoff,
                if dump_failure.is_some() {
                    "eeprom-dump-read"
                } else {
                    "d4-eeprom-dump"
                },
                &error,
                Some(dump_progress(
                    completed_address_count,
                    dump_failure.map(|(_, failed_address)| failed_address),
                )),
            ),
            error,
            report_file,
        ),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[allow(clippy::too_many_arguments)]
fn d4_eeprom_dump(
    _: u16,
    _: u16,
    _: u8,
    _: u8,
    _: Option<u8>,
    _: Option<u8>,
    _: &str,
    _: Option<u16>,
    _: Option<u16>,
    _: Option<&Path>,
    _: Option<&Path>,
) -> Result<String, String> {
    Err(
        "hardware USB validation is currently supported only on Linux, macOS, or Windows"
            .to_owned(),
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[allow(clippy::too_many_arguments)]
fn d4_eeprom_read(
    _: u16,
    _: u16,
    _: u8,
    _: u8,
    _: Option<u8>,
    _: Option<u8>,
    _: &str,
    _: &[u16],
    _: Option<&Path>,
    _: Option<&Path>,
) -> Result<String, String> {
    Err(
        "hardware USB validation is currently supported only on Linux, macOS, or Windows"
            .to_owned(),
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn d4_identity(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: Option<u8>,
    device_address: Option<u8>,
    model: &str,
    trace_file: Option<&Path>,
    report_file: Option<&Path>,
) -> Result<String, String> {
    if let Some(path) = trace_file {
        validate_trace_file_path(path)?;
    }
    if let Some(path) = report_file {
        validate_report_file_path(path)?;
    }
    let spec = ModelDatabase::builtin()
        .map_err(|e| e.to_string())?
        .get(model)
        .ok_or_else(|| format!("unknown model: {model}"))?
        .clone();
    let automatic_handoff = DriverHandoffReport::automatic();
    let transport = match reink_usb::ReadOnlyUsbTransport::open(
        usb_device_selector(vendor_id, product_id, bus_number, device_address)?,
        reink_platform::UsbInterfaceSelector {
            number: interface,
            alternate_setting,
        },
    ) {
        Ok(transport) => transport,
        Err(error) => {
            let primary = format!("USB transport open failed: {error}");
            return fail_with_report(
                d4_failure_report("d4-identity", automatic_handoff, "usb-open", &primary, None),
                primary,
                report_file,
            );
        }
    };
    let operation = finish_trace(
        run_d4_operation(transport, spec, |session| session.read_identity()),
        trace_file,
        "d4-identity",
    );
    match operation.result {
        Ok(identity) => emit_report(
            d4_identity_report(json!(identity.fields()), operation.driver_handoff),
            report_file,
        ),
        Err(error) => fail_with_report(
            d4_failure_report(
                "d4-identity",
                operation.driver_handoff,
                "d4-identity",
                &error,
                None,
            ),
            error,
            report_file,
        ),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn d4_identity(
    _: u16,
    _: u16,
    _: u8,
    _: u8,
    _: Option<u8>,
    _: Option<u8>,
    _: &str,
    _: Option<&Path>,
    _: Option<&Path>,
) -> Result<String, String> {
    Err(
        "hardware USB validation is currently supported only on Linux, macOS, or Windows"
            .to_owned(),
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[allow(clippy::too_many_arguments)]
fn d4_eeprom_boundary_probe(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: Option<u8>,
    device_address: Option<u8>,
    model: &str,
    address: u16,
    confirmation: Option<&str>,
    trace_file: Option<&Path>,
    report_file: Option<&Path>,
) -> Result<String, String> {
    if let Some(path) = trace_file {
        validate_trace_file_path(path)?;
    }
    if let Some(path) = report_file {
        validate_report_file_path(path)?;
    }
    let spec = ModelDatabase::builtin()
        .map_err(|error| error.to_string())?
        .get(model)
        .ok_or_else(|| format!("unknown model: {model}"))?
        .clone();
    validate_boundary_probe(&spec, address, confirmation)?;

    let automatic_handoff = DriverHandoffReport::automatic();
    let transport = match reink_usb::ReadOnlyUsbTransport::open(
        usb_device_selector(vendor_id, product_id, bus_number, device_address)?,
        reink_platform::UsbInterfaceSelector {
            number: interface,
            alternate_setting,
        },
    ) {
        Ok(transport) => transport,
        Err(error) => {
            let primary = format!("USB transport open failed: {error}");
            return fail_with_report(
                d4_failure_report(
                    "d4-eeprom-boundary-probe",
                    automatic_handoff,
                    "usb-open",
                    &primary,
                    None,
                ),
                primary,
                report_file,
            );
        }
    };
    let operation = finish_trace(
        run_d4_operation(transport, spec, |session| session.read_eeprom(&[address])),
        trace_file,
        "d4-eeprom-boundary-probe",
    );
    match operation.result {
        Ok(mut values) => {
            let value = values
                .pop()
                .expect("a successful single-address EEPROM read has one reply");
            emit_report(
                d4_eeprom_boundary_probe_report(address, value.value, operation.driver_handoff),
                report_file,
            )
        }
        Err(error) => fail_with_report(
            d4_failure_report(
                "d4-eeprom-boundary-probe",
                operation.driver_handoff,
                "eeprom-boundary-probe-read",
                &error,
                None,
            ),
            error,
            report_file,
        ),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[allow(clippy::too_many_arguments)]
fn d4_eeprom_boundary_probe(
    _: u16,
    _: u16,
    _: u8,
    _: u8,
    _: Option<u8>,
    _: Option<u8>,
    _: &str,
    _: u16,
    _: Option<&str>,
    _: Option<&Path>,
    _: Option<&Path>,
) -> Result<String, String> {
    Err(
        "hardware USB validation is currently supported only on Linux, macOS, or Windows"
            .to_owned(),
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn read_sequence(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: Option<u8>,
    device_address: Option<u8>,
    skip_d4_entry_probe: bool,
) -> Result<String, String> {
    let selector = reink_platform::UsbInterfaceSelector {
        number: interface,
        alternate_setting,
    };
    let device = usb_device_selector(vendor_id, product_id, bus_number, device_address)?;
    let bytes = read_printer_device_id(device, selector).map_err(driver_recovery_error)?;
    let identity = PrinterIdentity::parse(
        std::str::from_utf8(&bytes).map_err(|_| "USB device ID is not UTF-8")?,
    )
    .map_err(|error| error.to_string())?;
    let database = ModelDatabase::builtin().map_err(|error| error.to_string())?;
    let resolved_model = database
        .resolve_identity(&identity)
        .map(|spec| spec.model.as_str());
    let d4_entry = if skip_d4_entry_probe {
        None
    } else {
        let entry = probe_epson_d4_entry(device, selector).map_err(driver_recovery_error)?;
        Some(match entry {
            EpsonD4EntryProbeResult::Recognized => json!({"status": "recognized"}),
            EpsonD4EntryProbeResult::Unrecognized { received_bytes } => {
                json!({"status": "unrecognized", "received_bytes": received_bytes})
            }
        })
    };
    Ok(read_sequence_report(
        json!({"vendor_id": format!("{vendor_id:04x}"), "product_id": format!("{product_id:04x}"), "interface": interface, "alternate_setting": alternate_setting, "bytes_received": bytes.len()}),
        json!(identity.fields()),
        json!({"detected_model": identity.detected_model(), "resolved_model": resolved_model}),
        d4_entry,
        DriverHandoffReport::automatic(),
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn read_sequence(
    _: u16,
    _: u16,
    _: u8,
    _: u8,
    _: Option<u8>,
    _: Option<u8>,
    _: bool,
) -> Result<String, String> {
    Err(
        "hardware USB validation is currently supported only on Linux, macOS, or Windows"
            .to_owned(),
    )
}

fn parse_u16(value: &str) -> Result<u16, String> {
    let (value, radix) = value
        .strip_prefix("0x")
        .map_or((value, 10), |value| (value, 16));
    u16::from_str_radix(value, radix)
        .map_err(|_| "expected a 16-bit decimal or 0x-prefixed hexadecimal integer".to_owned())
}

fn parse_u8(value: &str) -> Result<u8, String> {
    let (value, radix) = value
        .strip_prefix("0x")
        .map_or((value, 10), |value| (value, 16));
    u8::from_str_radix(value, radix)
        .map_err(|_| "expected a byte in decimal or 0x-prefixed hexadecimal form".to_owned())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn eeprom_dump_addresses(
    spec: &EpsonSpec,
    start_address: Option<u16>,
    end_address: Option<u16>,
) -> Result<Vec<u16>, String> {
    let start = start_address.unwrap_or(spec.memory_low);
    let end = end_address.unwrap_or(spec.memory_high);
    if start > end {
        return Err(format!(
            "EEPROM dump start address {start:#06x} exceeds end address {end:#06x}"
        ));
    }
    if start < spec.memory_low || end > spec.memory_high {
        return Err(format!(
            "EEPROM dump range {start:#06x}..={end:#06x} is outside the model range {:#06x}..={:#06x}",
            spec.memory_low, spec.memory_high
        ));
    }
    Ok((start..=end).collect())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn validate_eeprom_read_addresses(spec: &EpsonSpec, addresses: &[u16]) -> Result<(), String> {
    for &address in addresses {
        if !(spec.memory_low..=spec.memory_high).contains(&address) {
            return Err(format!(
                "EEPROM address {address:#06x} is outside model {} range {:#06x}..={:#06x}",
                spec.model, spec.memory_low, spec.memory_high
            ));
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn validate_boundary_probe(
    spec: &EpsonSpec,
    address: u16,
    confirmation: Option<&str>,
) -> Result<(), String> {
    if confirmation != Some(OUT_OF_RANGE_READ_CONFIRMATION) {
        return Err(format!(
            "boundary probe requires --confirm-out-of-range-read {OUT_OF_RANGE_READ_CONFIRMATION}"
        ));
    }
    if (spec.memory_low..=spec.memory_high).contains(&address) {
        return Err(format!(
            "boundary probe address {address:#06x} is within model {} range {:#06x}..={:#06x}; use d4-eeprom-read instead",
            spec.model, spec.memory_low, spec.memory_high
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn usb_device_selector(
    vendor_id: u16,
    product_id: u16,
    bus_number: Option<u8>,
    device_address: Option<u8>,
) -> Result<reink_usb::UsbDeviceSelector, String> {
    match (bus_number, device_address) {
        (None, None) => Ok(reink_usb::UsbDeviceSelector::new(vendor_id, product_id)),
        (Some(bus_number), Some(device_address)) => Ok(reink_usb::UsbDeviceSelector::at_location(
            vendor_id,
            product_id,
            bus_number,
            device_address,
        )),
        _ => Err("--bus-number and --device-address must be provided together".to_owned()),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn candidate_model_hints(
    candidate: &reink_usb::UsbPrinterCandidate,
    database: &ModelDatabase,
) -> Vec<String> {
    database
        .models()
        .filter(|model| {
            let spec = database
                .get(model)
                .expect("model database iterator returns known model names");
            spec.vendor_id == candidate.vendor_id && spec.product_id == Some(candidate.product_id)
        })
        .map(str::to_owned)
        .collect()
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn usb_candidates_report(
    candidates: &[reink_usb::UsbPrinterCandidate],
    database: &ModelDatabase,
) -> String {
    let candidates = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            json!({
                "alias": format!("usb-{}", index + 1),
                "selector": {
                    "vendor_id": format!("{:04x}", candidate.vendor_id),
                    "product_id": format!("{:04x}", candidate.product_id),
                    "bus_number": candidate.bus_number,
                    "device_address": candidate.device_address,
                    "interface": candidate.interface_number,
                    "alternate_setting": candidate.alternate_setting,
                },
                "model_hints": candidate_model_hints(candidate, database),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "schema_version": 3,
        "mode": "read_only",
        "command": "usb-candidates",
        "candidates": candidates,
        "next_step": "Select one candidate by its shown selector; its alias is session/report-only and a later IEEE 1284 identity read confirms the model.",
    })
    .to_string()
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn completed_step(name: &str, result: Value) -> Value {
    json!({"name": name, "status": "completed", "result": result})
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadOnlyFailureKind {
    Blocked,
    Timeout,
    Malformed,
}

#[cfg(test)]
impl ReadOnlyFailureKind {
    fn status(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Timeout => "timeout",
            Self::Malformed => "malformed",
        }
    }
}

#[cfg(test)]
fn failed_step(name: &str, kind: ReadOnlyFailureKind, message: &str) -> Value {
    json!({
        "name": name,
        "status": kind.status(),
        "error": {
            "kind": kind.status(),
            "message": message,
        },
    })
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DriverHandoffReport {
    automatic: bool,
    detached: bool,
    reattached: Option<bool>,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
impl DriverHandoffReport {
    #[cfg_attr(
        not(any(target_os = "linux", target_os = "macos", target_os = "windows")),
        allow(dead_code)
    )]
    const fn automatic() -> Self {
        Self {
            automatic: true,
            detached: false,
            reattached: None,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    const fn from_usb(outcome: reink_usb::UsbDriverHandoffOutcome) -> Self {
        Self {
            automatic: outcome.requested,
            detached: outcome.detached,
            reattached: outcome.reattached,
        }
    }

    fn json(self) -> Value {
        json!({
            "automatic": self.automatic,
            "detached": self.detached,
            "reattached": self.reattached,
        })
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
impl From<bool> for DriverHandoffReport {
    fn from(automatic: bool) -> Self {
        Self {
            automatic,
            detached: false,
            reattached: None,
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn read_only_report(
    command: &str,
    driver_handoff: impl Into<DriverHandoffReport>,
    steps: Vec<Value>,
    next_step: &str,
) -> String {
    let driver_handoff = driver_handoff.into();
    json!({
        "schema_version": 3,
        "mode": "read_only",
        "command": command,
        "linux_driver_handoff": driver_handoff.json(),
        "steps": steps,
        "next_step": next_step,
    })
    .to_string()
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn d4_failure_report(
    command: &str,
    driver_handoff: DriverHandoffReport,
    stage: &str,
    error: &str,
    dump_progress: Option<Value>,
) -> String {
    let mut failure = json!({
        "stage": stage,
        "error": error,
    });
    if let Some(progress) = dump_progress {
        failure["dump_progress"] = progress;
    }
    json!({
        "schema_version": 3,
        "mode": "read_only",
        "command": command,
        "status": "failed",
        "linux_driver_handoff": driver_handoff.json(),
        "failure": failure,
        "remediation": DRIVER_RECOVERY_REMEDIATION,
        "next_step": "Resolve the observed read-only failure before retrying. This report does not authorize any EEPROM write, restore, reset, or other state change.",
    })
    .to_string()
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn dump_progress(completed_address_count: usize, failed_address: Option<u16>) -> Value {
    json!({
        "completed_address_count": completed_address_count,
        "failed_address": failed_address.map(|address| format!("{address:04X}")),
    })
}

/// Produces a deterministic report for hardware-independent schema tests.
#[cfg(test)]
fn simulated_read_only_report(
    command: &str,
    completed: Vec<(&str, Value)>,
    failure: (&str, ReadOnlyFailureKind, &str),
    next_step: &str,
) -> String {
    let mut steps = completed
        .into_iter()
        .map(|(name, result)| completed_step(name, result))
        .collect::<Vec<_>>();
    steps.push(failed_step(failure.0, failure.1, failure.2));
    read_only_report(command, false, steps, next_step)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn read_sequence_report(
    usb: Value,
    identity: Value,
    model_resolution: Value,
    d4_entry: Option<Value>,
    driver_handoff: impl Into<DriverHandoffReport>,
) -> String {
    let mut steps = vec![
        completed_step("usb-device-id", usb),
        completed_step("parse-device-id", identity),
        completed_step("resolve-model", model_resolution),
    ];
    if let Some(d4_entry) = d4_entry {
        steps.push(completed_step("d4-entry-probe", d4_entry));
    }
    read_only_report(
        "read-sequence",
        driver_handoff,
        steps,
        "Review this read-only preflight evidence before using d4-identity or d4-eeprom-read. This report does not authorize a write; use only an explicitly authorized write-evidence or confirmed CLI operation.",
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn d4_identity_report(identity: Value, driver_handoff: impl Into<DriverHandoffReport>) -> String {
    read_only_report(
        "d4-identity",
        driver_handoff,
        vec![
            completed_step(
                "d4-session-connect",
                json!({"init": "completed", "service": "EPSON-CTRL"}),
            ),
            completed_step("identity-read", identity),
            completed_step("d4-session-shutdown", json!({"exit": "completed"})),
        ],
        "Review identity evidence before selecting EEPROM addresses. Physical writes require their own explicit write-evidence or confirmed CLI gates.",
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn d4_eeprom_read_report(
    values: Vec<Value>,
    driver_handoff: impl Into<DriverHandoffReport>,
) -> String {
    read_only_report(
        "d4-eeprom-read",
        driver_handoff,
        vec![
            completed_step(
                "d4-session-connect",
                json!({"init": "completed", "service": "EPSON-CTRL"}),
            ),
            completed_step("eeprom-read", json!({"values": values})),
            completed_step("d4-session-shutdown", json!({"exit": "completed"})),
        ],
        "Preserve this read-only EEPROM evidence. It does not authorize a write; use only an explicitly authorized write-evidence or confirmed CLI operation.",
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn d4_eeprom_dump_report(
    model: &str,
    start_address: u16,
    end_address: u16,
    values: Vec<Value>,
    driver_handoff: impl Into<DriverHandoffReport>,
) -> String {
    read_only_report(
        "d4-eeprom-dump",
        driver_handoff,
        vec![
            completed_step(
                "d4-session-connect",
                json!({"init": "completed", "service": "EPSON-CTRL"}),
            ),
            completed_step(
                "eeprom-dump",
                json!({
                    "model": model,
                    "range": {
                        "start_address": format!("{start_address:04X}"),
                        "end_address": format!("{end_address:04X}"),
                    },
                    "value_count": values.len(),
                    "values": values,
                }),
            ),
            completed_step("d4-session-shutdown", json!({"exit": "completed"})),
        ],
        "Retain this read-only EEPROM dump privately. It is not authorization for any EEPROM write, restore, or reset.",
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn d4_eeprom_boundary_probe_report(
    address: u16,
    value: u8,
    driver_handoff: impl Into<DriverHandoffReport>,
) -> String {
    read_only_report(
        "d4-eeprom-boundary-probe",
        driver_handoff,
        vec![
            completed_step(
                "d4-session-connect",
                json!({"init": "completed", "service": "EPSON-CTRL"}),
            ),
            completed_step(
                "out-of-range-eeprom-read",
                json!({
                    "address": format!("{address:04X}"),
                    "value": value,
                    "interpretation": "observed behavior only; this successful reply is not proof that out-of-range EEPROM reads are safe.",
                }),
            ),
            completed_step("d4-session-shutdown", json!({"exit": "completed"})),
        ],
        "Treat this as observed behavior only, not proof that out-of-range reads are safe. It does not authorize any EEPROM write, restore, reset, or other state change.",
    )
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        fs,
        path::{Path, PathBuf},
    };

    use clap::Parser;
    use reink_app::EepromImage;
    use reink_platform::TransportEvent;
    use serde_json::{Value, json};

    use super::{
        Cli, Command, DriverHandoffReport, OUT_OF_RANGE_READ_CONFIRMATION, ReadOnlyFailureKind,
        TRACE_SANITIZATION_CONFIRMATION, WRITE_EVIDENCE_RESTORATION_CONFIRMATION,
        WRITE_EVIDENCE_WRITE_CONFIRMATION, WriteEvidenceCleanup, WriteEvidenceSession,
        WriteEvidenceStage, d4_eeprom_boundary_probe_report, d4_eeprom_dump_report,
        d4_eeprom_read_report, d4_failure_report, d4_identity_report, dump_progress,
        eeprom_dump_addresses, emit_report, execute_write_evidence, parse_trace_events, parse_u16,
        read_sequence_report, simulated_read_only_report, trace_json, trace_to_transcript,
        transcript_template, usb_candidates_report, usb_device_selector, validate_boundary_probe,
        validate_eeprom_read_addresses, validate_report_file_path, validate_trace_file_path,
        validate_write_evidence_gates, write_evidence_report,
    };

    fn report(output: String) -> Value {
        serde_json::from_str(&output).unwrap()
    }

    fn assert_completed_steps(report: &Value, expected_names: &[&str]) {
        assert_eq!(report["schema_version"], 3);
        assert_eq!(report["mode"], "read_only");
        let steps = report["steps"].as_array().unwrap();
        assert_eq!(steps.len(), expected_names.len());
        for (step, expected_name) in steps.iter().zip(expected_names) {
            assert_eq!(step["name"], *expected_name);
            assert_eq!(step["status"], "completed");
            assert!(step.get("result").is_some());
        }
    }

    struct MockWriteEvidenceSession {
        identities: VecDeque<Result<Option<String>, String>>,
        reads: VecDeque<Result<u8, String>>,
        plans: VecDeque<Result<reink_app::EepromWritePlan, String>>,
        applies: VecDeque<Result<(), String>>,
        applied_updates: Vec<Vec<(u16, u8)>>,
    }

    impl WriteEvidenceSession for MockWriteEvidenceSession {
        fn identity_model(&mut self) -> Result<Option<String>, String> {
            self.identities
                .pop_front()
                .expect("test identity response was configured")
        }

        fn read_byte(&mut self, _: u16) -> Result<u8, String> {
            self.reads
                .pop_front()
                .expect("test EEPROM read response was configured")
        }

        fn prepare_write_plan(
            &mut self,
            _: &[(u16, u8)],
        ) -> Result<reink_app::EepromWritePlan, String> {
            self.plans
                .pop_front()
                .expect("test EEPROM plan was configured")
        }

        fn apply_write_plan(&mut self, plan: &reink_app::EepromWritePlan) -> Result<(), String> {
            self.applied_updates.push(plan.updates.clone());
            self.applies
                .pop_front()
                .expect("test EEPROM apply result was configured")
        }
    }

    fn test_write_plan(
        spec: &reink_core::EpsonSpec,
        address: u16,
        original_value: u8,
        requested_value: u8,
    ) -> reink_app::EepromWritePlan {
        reink_app::EepromWritePlan {
            backup: EepromImage {
                model: spec.model.clone(),
                start_address: spec.memory_low,
                bytes: vec![
                    original_value;
                    usize::from(spec.memory_high) - usize::from(spec.memory_low) + 1
                ],
            },
            updates: vec![(address, requested_value)],
        }
    }

    #[test]
    fn read_sequence_uses_versioned_per_step_results() {
        let output = report(read_sequence_report(
            json!({"vendor_id": "04b8", "bytes_received": 25}),
            json!({"MFG": "EPSON", "MDL": "C90"}),
            json!({"detected_model": "C90", "resolved_model": "C90"}),
            Some(json!({"status": "recognized"})),
            true,
        ));

        assert_eq!(output["command"], "read-sequence");
        assert_eq!(output["linux_driver_handoff"]["automatic"], true);
        assert_eq!(output["linux_driver_handoff"]["detached"], false);
        assert!(output["linux_driver_handoff"]["reattached"].is_null());
        assert_completed_steps(
            &output,
            &[
                "usb-device-id",
                "parse-device-id",
                "resolve-model",
                "d4-entry-probe",
            ],
        );
        assert_eq!(output["steps"][3]["result"]["status"], "recognized");
        assert!(
            output["next_step"]
                .as_str()
                .unwrap()
                .contains("d4-identity")
        );
    }

    #[test]
    fn read_sequence_can_omit_the_state_changing_d4_entry_probe() {
        let output = report(read_sequence_report(
            json!({"vendor_id": "04b8", "bytes_received": 25}),
            json!({"MFG": "EPSON", "MDL": "C90"}),
            json!({"detected_model": "C90", "resolved_model": "C90"}),
            None,
            false,
        ));

        assert_completed_steps(
            &output,
            &["usb-device-id", "parse-device-id", "resolve-model"],
        );
    }

    #[test]
    fn d4_reports_preserve_read_only_step_evidence() {
        let identity = report(d4_identity_report(
            json!({"MFG": "EPSON", "MDL": "C90"}),
            false,
        ));
        assert_eq!(identity["command"], "d4-identity");
        assert_completed_steps(
            &identity,
            &["d4-session-connect", "identity-read", "d4-session-shutdown"],
        );
        assert_eq!(identity["steps"][1]["result"]["MDL"], "C90");

        let eeprom = report(d4_eeprom_read_report(
            vec![json!({"address": "000C", "value": 66})],
            false,
        ));
        assert_eq!(eeprom["command"], "d4-eeprom-read");
        assert_completed_steps(
            &eeprom,
            &["d4-session-connect", "eeprom-read", "d4-session-shutdown"],
        );
        assert_eq!(eeprom["steps"][1]["result"]["values"][0]["address"], "000C");

        let dump = report(d4_eeprom_dump_report(
            "L1300",
            0x0100,
            0x0101,
            vec![
                json!({"address": "0100", "value": 1}),
                json!({"address": "0101", "value": 2}),
            ],
            false,
        ));
        assert_eq!(dump["command"], "d4-eeprom-dump");
        assert_completed_steps(
            &dump,
            &["d4-session-connect", "eeprom-dump", "d4-session-shutdown"],
        );
        assert_eq!(dump["steps"][1]["result"]["value_count"], 2);
        assert_eq!(dump["steps"][1]["result"]["range"]["start_address"], "0100");
    }

    #[test]
    fn simulated_driver_reports_preserve_completed_evidence_and_classify_failures() {
        let cases = [
            (
                ReadOnlyFailureKind::Blocked,
                "active kernel driver",
                "blocked",
            ),
            (
                ReadOnlyFailureKind::Timeout,
                "USB read timed out",
                "timeout",
            ),
            (
                ReadOnlyFailureKind::Malformed,
                "device ID has an invalid length",
                "malformed",
            ),
        ];

        for (kind, message, status) in cases {
            let output = report(simulated_read_only_report(
                "read-sequence",
                vec![("usb-device-id", json!({"bytes_received": 25}))],
                ("parse-device-id", kind, message),
                "Resolve the reported read-only condition before retrying; this read-only result does not authorize a write or reset.",
            ));
            assert_eq!(output["schema_version"], 3);
            assert_eq!(output["mode"], "read_only");
            assert_eq!(output["steps"][0]["status"], "completed");
            assert_eq!(output["steps"][1]["name"], "parse-device-id");
            assert_eq!(output["steps"][1]["status"], status);
            assert_eq!(output["steps"][1]["error"]["kind"], status);
            assert_eq!(output["steps"][1]["error"]["message"], message);
            assert!(output["steps"][1].get("result").is_none());
        }
    }

    #[test]
    fn usb_candidates_use_result_only_aliases_and_exact_product_model_hints() {
        let database = reink_core::ModelDatabase::from_toml(
            r#"
[[EPSON]]
models = ["Candidate A", "Candidate B"]
idVendor = 0x04b8
idProduct = 0x1234
"#,
        )
        .unwrap();
        let output = report(usb_candidates_report(
            &[
                reink_usb::UsbPrinterCandidate {
                    vendor_id: 0x04b8,
                    product_id: 0x1234,
                    bus_number: 1,
                    device_address: 2,
                    interface_number: 3,
                    alternate_setting: 0,
                },
                reink_usb::UsbPrinterCandidate {
                    vendor_id: 0x04b8,
                    product_id: 0xffff,
                    bus_number: 4,
                    device_address: 5,
                    interface_number: 6,
                    alternate_setting: 0,
                },
            ],
            &database,
        ));

        assert_eq!(output["schema_version"], 3);
        assert_eq!(output["mode"], "read_only");
        assert_eq!(output["command"], "usb-candidates");
        assert_eq!(output["candidates"][0]["alias"], "usb-1");
        assert_eq!(output["candidates"][1]["alias"], "usb-2");
        assert_eq!(output["candidates"][0]["selector"]["vendor_id"], "04b8");
        assert_eq!(output["candidates"][0]["selector"]["product_id"], "1234");
        assert_eq!(output["candidates"][0]["selector"]["bus_number"], 1);
        assert_eq!(output["candidates"][0]["selector"]["device_address"], 2);
        assert_eq!(output["candidates"][0]["selector"]["interface"], 3);
        assert_eq!(output["candidates"][0]["selector"]["alternate_setting"], 0);
        assert_eq!(
            output["candidates"][0]["model_hints"],
            json!(["Candidate A", "Candidate B"])
        );
        assert_eq!(output["candidates"][1]["model_hints"], json!([]));
        assert!(
            output["next_step"]
                .as_str()
                .unwrap()
                .contains("session/report-only")
        );
        assert!(output["candidates"][0].get("manufacturer").is_none());
        assert!(output["candidates"][0].get("product").is_none());
        assert!(output["candidates"][0].get("serial_number").is_none());
    }

    #[test]
    fn parses_explicit_read_only_and_write_evidence_commands() {
        let cli = Cli::try_parse_from(["reink-hardware-test", "usb-candidates"]).unwrap();
        assert!(matches!(cli.command, Command::UsbCandidates));

        let cli = Cli::try_parse_from([
            "reink-hardware-test",
            "trace-to-transcript",
            "--trace-file",
            "private/reviewed-trace.json",
            "--output-file",
            "private/template.rs",
            "--confirmation",
            TRACE_SANITIZATION_CONFIRMATION,
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::TraceToTranscript {
                trace_file,
                output_file,
                confirmation: Some(ref confirmation),
                description,
            } if trace_file == Path::new("private/reviewed-trace.json")
                && output_file == Path::new("private/template.rs")
                && confirmation == TRACE_SANITIZATION_CONFIRMATION
                && description == "sanitized fixture"
        ));

        let cli = Cli::try_parse_from([
            "reink-hardware-test",
            "d4-eeprom-read",
            "--vendor-id",
            "0x04b8",
            "--product-id",
            "1234",
            "--interface",
            "0",
            "--model",
            "C90",
            "--address",
            "0x000c",
            "--trace-file",
            "private/eeprom-read.json",
            "--report-file",
            "private/eeprom-read-report.json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::D4EepromRead {
                vendor_id: 0x04b8,
                product_id: 1234,
                interface: 0,
                alternate_setting: 0,
                bus_number: None,
                device_address: None,
                trace_file: Some(ref trace_file),
                report_file: Some(ref report_file),
                ref model,
                ref address,
                ..
            } if model == "C90" && address == &[0x000c]
                && trace_file == Path::new("private/eeprom-read.json")
                && report_file == Path::new("private/eeprom-read-report.json")
        ));
        assert_eq!(parse_u16("0x04b8").unwrap(), 0x04b8);
        assert!(parse_u16("not-a-number").is_err());

        let cli = Cli::try_parse_from([
            "reink-hardware-test",
            "d4-eeprom-dump",
            "--vendor-id",
            "0x04b8",
            "--product-id",
            "1234",
            "--interface",
            "0",
            "--model",
            "L1300",
            "--start-address",
            "0x0100",
            "--end-address",
            "0x0101",
            "--trace-file",
            "private/eeprom-dump.json",
            "--report-file",
            "private/eeprom-dump-report.json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::D4EepromDump {
                model,
                start_address: Some(0x0100),
                end_address: Some(0x0101),
                trace_file: Some(ref trace_file),
                report_file: Some(ref report_file),
                ..
            } if model == "L1300" && trace_file == Path::new("private/eeprom-dump.json")
                && report_file == Path::new("private/eeprom-dump-report.json")
        ));

        let cli = Cli::try_parse_from([
            "reink-hardware-test",
            "read-sequence",
            "--vendor-id",
            "0x04b8",
            "--product-id",
            "1234",
            "--interface",
            "0",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::ReadSequence { .. }));

        let cli = Cli::try_parse_from([
            "reink-hardware-test",
            "d4-identity",
            "--vendor-id",
            "0x04b8",
            "--product-id",
            "1234",
            "--interface",
            "0",
            "--model",
            "C90",
            "--trace-file",
            "private/identity.json",
            "--report-file",
            "private/identity-report.json",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::D4Identity {
                trace_file: Some(ref trace_file),
                report_file: Some(ref report_file),
                ..
            } if trace_file == Path::new("private/identity.json")
                && report_file == Path::new("private/identity-report.json")
        ));

        let cli = Cli::try_parse_from([
            "reink-hardware-test",
            "d4-eeprom-boundary-probe",
            "--vendor-id",
            "0x04b8",
            "--product-id",
            "1234",
            "--interface",
            "0",
            "--model",
            "C90",
            "--address",
            "0xffff",
            "--confirm-out-of-range-read",
            OUT_OF_RANGE_READ_CONFIRMATION,
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::D4EepromBoundaryProbe {
                address: 0xffff,
                confirm_out_of_range_read: Some(ref confirmation),
                ..
            } if confirmation == OUT_OF_RANGE_READ_CONFIRMATION
        ));

        let cli = Cli::try_parse_from([
            "reink-hardware-test",
            "d4-eeprom-write-evidence",
            "--vendor-id",
            "0x04b8",
            "--product-id",
            "1234",
            "--interface",
            "0",
            "--alternate-setting",
            "0",
            "--bus-number",
            "1",
            "--device-address",
            "2",
            "--model",
            "C90",
            "--address",
            "0x000c",
            "--value",
            "0x42",
            "--backup-file",
            "private/complete-backup.bin",
            "--report-file",
            "private/write-evidence-report.json",
            "--confirm-write",
            WRITE_EVIDENCE_WRITE_CONFIRMATION,
            "--confirm-restoration-evidence",
            WRITE_EVIDENCE_RESTORATION_CONFIRMATION,
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::D4EepromWriteEvidence {
                vendor_id: 0x04b8,
                product_id: 1234,
                interface: 0,
                alternate_setting: 0,
                bus_number: 1,
                device_address: 2,
                address: 0x000c,
                value: 0x42,
                backup_file,
                report_file,
                confirm_write: Some(ref confirm_write),
                confirm_restoration_evidence: Some(ref confirm_restoration_evidence),
                ref model,
            } if model == "C90"
                && backup_file == Path::new("private/complete-backup.bin")
                && report_file == Path::new("private/write-evidence-report.json")
                && confirm_write == WRITE_EVIDENCE_WRITE_CONFIRMATION
                && confirm_restoration_evidence == WRITE_EVIDENCE_RESTORATION_CONFIRMATION
        ));
    }

    #[test]
    fn trace_json_is_ordered_read_only_and_uses_uppercase_hex() {
        let trace = trace_json(
            "d4-identity",
            &[
                TransportEvent::Tx(vec![0x1b, 0x40]),
                TransportEvent::Rx(vec![0xa0]),
                TransportEvent::Rx(vec![]),
            ],
        );

        assert_eq!(trace["schema_version"], 1);
        assert_eq!(trace["mode"], "read_only");
        assert_eq!(trace["command"], "d4-identity");
        assert_eq!(
            trace["events"][0],
            json!({"direction": "tx", "bytes": "1B40"})
        );
        assert_eq!(
            trace["events"][1],
            json!({"direction": "rx", "bytes": "A0"})
        );
        assert_eq!(trace["events"][2], json!({"direction": "rx", "bytes": ""}));
        assert!(trace.get("usb_path").is_none());
        assert!(trace.get("host").is_none());
    }

    #[test]
    fn trace_file_refuses_an_existing_path_without_writing() {
        let existing = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");

        assert!(
            validate_trace_file_path(&existing)
                .unwrap_err()
                .contains("refusing to overwrite")
        );
    }

    #[test]
    fn trace_to_transcript_parses_schema_and_preserves_event_boundaries_in_order() {
        let events = parse_trace_events(
            r#"{"schema_version":1,"mode":"read_only","command":"d4-identity","events":[{"direction":"tx","bytes":"1B40"},{"direction":"rx","bytes":"06"},{"direction":"rx","bytes":""},{"direction":"tx","bytes":"AA"}]}"#,
        )
        .unwrap();

        let template = transcript_template("sanitized fixture", &events);

        assert_eq!(
            template,
            "// Local template only. The operator confirmed this evidence was manually sanitized.\n\
// Review every byte, add behavior assertions, and do not commit this template without review.\n\
let mut transcript = SanitizedTranscript::new(\"sanitized fixture\");\n\
transcript.expect_write(vec![0x1B, 0x40]);\n\
transcript.respond(vec![0x06]);\n\
transcript.respond(vec![]);\n\
transcript.expect_write(vec![0xAA]);\n\
// Add assertions for the behavior this transcript protects.\n"
        );
    }

    #[test]
    fn trace_to_transcript_rejects_invalid_schema_direction_and_hex() {
        for (trace, expected) in [
            (
                r#"{"schema_version":2,"mode":"read_only","command":"d4","events":[]}"#,
                "schema_version",
            ),
            (
                r#"{"schema_version":1,"mode":"read_only","command":"d4","events":[{"direction":"write","bytes":"AA"}]}"#,
                "direction",
            ),
            (
                r#"{"schema_version":1,"mode":"read_only","command":"d4","events":[{"direction":"rx","bytes":"0a"}]}"#,
                "uppercase hexadecimal",
            ),
            (
                r#"{"schema_version":1,"mode":"read_only","command":"d4","events":[{"direction":"rx","bytes":"ABC"}]}"#,
                "even number",
            ),
        ] {
            assert!(parse_trace_events(trace).unwrap_err().contains(expected));
        }
    }

    #[test]
    fn trace_to_transcript_requires_confirmation_and_never_overwrites() {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let input = directory.join(format!(
            "trace-to-transcript-input-{}.json",
            std::process::id()
        ));
        let output = directory.join(format!(
            "trace-to-transcript-output-{}.rs",
            std::process::id()
        ));
        let _ = fs::remove_file(&input);
        let _ = fs::remove_file(&output);
        fs::write(
            &input,
            r#"{"schema_version":1,"mode":"read_only","command":"d4","events":[{"direction":"tx","bytes":"AA"}]}"#,
        )
        .unwrap();

        let refused = trace_to_transcript(&input, &output, None, "sanitized fixture").unwrap_err();
        assert!(refused.contains(TRACE_SANITIZATION_CONFIRMATION));
        assert!(!output.exists());

        trace_to_transcript(
            &input,
            &output,
            Some(TRACE_SANITIZATION_CONFIRMATION),
            "sanitized fixture",
        )
        .unwrap();
        let generated = fs::read_to_string(&output).unwrap();
        assert!(generated.contains("transcript.expect_write(vec![0xAA]);"));

        let missing_parent = directory
            .join(format!(
                "missing-transcript-template-parent-{}",
                std::process::id()
            ))
            .join("template.rs");
        let missing_parent_error = trace_to_transcript(
            &input,
            &missing_parent,
            Some(TRACE_SANITIZATION_CONFIRMATION),
            "sanitized fixture",
        )
        .unwrap_err();
        assert!(missing_parent_error.contains("parent directory does not exist"));

        let overwrite = trace_to_transcript(
            &input,
            &output,
            Some(TRACE_SANITIZATION_CONFIRMATION),
            "sanitized fixture",
        )
        .unwrap_err();
        assert!(overwrite.contains("refusing to overwrite"));
        assert_eq!(fs::read_to_string(&output).unwrap(), generated);

        fs::remove_file(input).unwrap();
        fs::remove_file(output).unwrap();
    }

    #[test]
    fn report_file_requires_a_new_path_with_an_existing_parent_and_writes_exact_json() {
        let existing = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(
            validate_report_file_path(&existing)
                .unwrap_err()
                .contains("refusing to overwrite")
        );
        let missing_parent = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("does-not-exist-for-report-test")
            .join("report.json");
        assert!(
            validate_report_file_path(&missing_parent)
                .unwrap_err()
                .contains("parent directory does not exist")
        );

        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("report-file-test-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let report = "{\"schema_version\":3,\"mode\":\"read_only\"}".to_owned();
        assert_eq!(emit_report(report.clone(), Some(&path)).unwrap(), report);
        assert_eq!(fs::read_to_string(&path).unwrap(), report);
        assert!(emit_report("{}".to_owned(), Some(&path)).is_err());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn address_and_boundary_probe_validation_stay_local_and_explicit() {
        let database = reink_core::ModelDatabase::builtin().unwrap();
        let spec = database.get("L1300").unwrap();
        assert!(validate_eeprom_read_addresses(spec, &[spec.memory_low, spec.memory_high]).is_ok());
        let outside = spec
            .memory_high
            .checked_add(1)
            .unwrap_or_else(|| spec.memory_low - 1);
        assert!(validate_eeprom_read_addresses(spec, &[outside]).is_err());
        assert!(validate_boundary_probe(spec, outside, None).is_err());
        assert!(validate_boundary_probe(spec, outside, Some("I_CONFIRM")).is_err());
        assert!(
            validate_boundary_probe(spec, spec.memory_low, Some(OUT_OF_RANGE_READ_CONFIRMATION))
                .is_err()
        );
        assert!(
            validate_boundary_probe(spec, outside, Some(OUT_OF_RANGE_READ_CONFIRMATION)).is_ok()
        );
    }

    #[test]
    fn failure_reports_are_schema_v3_and_include_recovery_remediation() {
        let failure_report = report(d4_failure_report(
            "d4-eeprom-dump",
            DriverHandoffReport {
                automatic: true,
                detached: true,
                reattached: Some(false),
            },
            "eeprom-dump-read",
            "read timed out",
            Some(dump_progress(3, Some(0x0103))),
        ));
        assert_eq!(failure_report["schema_version"], 3);
        assert_eq!(failure_report["status"], "failed");
        assert_eq!(failure_report["linux_driver_handoff"]["detached"], true);
        assert_eq!(failure_report["linux_driver_handoff"]["reattached"], false);
        assert!(
            failure_report["remediation"]
                .as_str()
                .unwrap()
                .contains("Reconnect")
        );
        assert_eq!(
            failure_report["failure"]["dump_progress"]["completed_address_count"],
            3
        );
        assert!(failure_report.get("values").is_none());
        assert!(failure_report.get("events").is_none());
        let setup_failure = report(d4_failure_report(
            "d4-eeprom-dump",
            false.into(),
            "d4-session-connect",
            "entry reply missing",
            Some(dump_progress(0, None)),
        ));
        assert!(setup_failure["failure"]["dump_progress"]["failed_address"].is_null());
    }

    #[test]
    fn boundary_probe_report_labels_observation_without_safety_claim() {
        let report = report(d4_eeprom_boundary_probe_report(0xffff, 42, false));
        assert_eq!(report["command"], "d4-eeprom-boundary-probe");
        assert_eq!(report["steps"][1]["result"]["address"], "FFFF");
        assert!(
            report["steps"][1]["result"]["interpretation"]
                .as_str()
                .unwrap()
                .contains("not proof")
        );
    }

    #[test]
    fn dump_range_defaults_to_and_stays_within_the_model_bounds() {
        let database = reink_core::ModelDatabase::builtin().unwrap();
        let spec = database.get("L1300").unwrap();

        let default_range = eeprom_dump_addresses(spec, None, None).unwrap();
        assert_eq!(default_range.first(), Some(&spec.memory_low));
        assert_eq!(default_range.last(), Some(&spec.memory_high));

        assert!(
            eeprom_dump_addresses(spec, Some(spec.memory_high), Some(spec.memory_low)).is_err()
        );
        if let Some(before_low) = spec.memory_low.checked_sub(1) {
            assert!(eeprom_dump_addresses(spec, Some(before_low), None).is_err());
        }
        if let Some(after_high) = spec.memory_high.checked_add(1) {
            assert!(eeprom_dump_addresses(spec, None, Some(after_high)).is_err());
        }
    }

    #[test]
    fn requires_a_complete_optional_usb_location() {
        assert!(usb_device_selector(0x04b8, 0x1234, Some(1), None).is_err());
        assert_eq!(
            usb_device_selector(0x04b8, 0x1234, Some(1), Some(2)).unwrap(),
            reink_usb::UsbDeviceSelector::at_location(0x04b8, 0x1234, 1, 2)
        );
    }

    #[test]
    fn write_evidence_gates_are_exact_local_and_require_distinct_new_paths() {
        let database = reink_core::ModelDatabase::builtin().unwrap();
        let spec = database.get("C90").unwrap();
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"));
        let backup = directory.join(format!("write-evidence-backup-{}.bin", std::process::id()));
        let report = directory.join(format!("write-evidence-report-{}.json", std::process::id()));
        let _ = fs::remove_file(&backup);
        let _ = fs::remove_file(&report);

        assert!(
            validate_write_evidence_gates(
                spec,
                spec.memory_low,
                &backup,
                &report,
                None,
                Some(WRITE_EVIDENCE_RESTORATION_CONFIRMATION),
            )
            .is_err()
        );
        assert!(
            validate_write_evidence_gates(
                spec,
                spec.memory_low,
                &backup,
                &report,
                Some(WRITE_EVIDENCE_WRITE_CONFIRMATION),
                Some("I_CONFIRM_RESTORE"),
            )
            .is_err()
        );
        let out_of_range = spec
            .memory_high
            .checked_add(1)
            .unwrap_or_else(|| spec.memory_low.checked_sub(1).unwrap());
        assert!(
            validate_write_evidence_gates(
                spec,
                out_of_range,
                &backup,
                &report,
                Some(WRITE_EVIDENCE_WRITE_CONFIRMATION),
                Some(WRITE_EVIDENCE_RESTORATION_CONFIRMATION),
            )
            .is_err()
        );
        assert!(
            validate_write_evidence_gates(
                spec,
                spec.memory_low,
                &backup,
                &backup,
                Some(WRITE_EVIDENCE_WRITE_CONFIRMATION),
                Some(WRITE_EVIDENCE_RESTORATION_CONFIRMATION),
            )
            .is_err()
        );
        assert!(
            validate_write_evidence_gates(
                spec,
                spec.memory_low,
                &backup,
                &report,
                Some(WRITE_EVIDENCE_WRITE_CONFIRMATION),
                Some(WRITE_EVIDENCE_RESTORATION_CONFIRMATION),
            )
            .is_ok()
        );
    }

    #[test]
    fn write_evidence_uses_core_plans_and_restores_after_verified_test_write() {
        let spec = reink_core::ModelDatabase::builtin()
            .unwrap()
            .get("C90")
            .unwrap()
            .clone();
        let address = spec.memory_low;
        let mut session = MockWriteEvidenceSession {
            identities: VecDeque::from([Ok(Some("C90".to_owned()))]),
            reads: VecDeque::from([Ok(0x10), Ok(0x42), Ok(0x10)]),
            plans: VecDeque::from([Ok(test_write_plan(&spec, address, 0x10, 0x42))]),
            applies: VecDeque::from([Ok(()), Ok(())]),
            applied_updates: Vec::new(),
        };
        let mut persisted_backup_len = None;

        let outcome = execute_write_evidence(&mut session, "C90", address, 0x42, |bytes| {
            persisted_backup_len = Some(bytes.len());
            Ok(())
        });

        assert!(outcome.completed_safely());
        assert_eq!(
            persisted_backup_len,
            Some(
                test_write_plan(&spec, address, 0x10, 0x42)
                    .backup
                    .bytes
                    .len()
            )
        );
        assert_eq!(
            session.applied_updates,
            vec![vec![(address, 0x42)], vec![(address, 0x10)]]
        );
    }

    #[test]
    fn write_evidence_attempts_restoration_when_the_test_write_fails() {
        let spec = reink_core::ModelDatabase::builtin()
            .unwrap()
            .get("C90")
            .unwrap()
            .clone();
        let address = spec.memory_low;
        let mut session = MockWriteEvidenceSession {
            identities: VecDeque::from([Ok(Some("C90".to_owned()))]),
            reads: VecDeque::from([Ok(0x10), Ok(0x10)]),
            plans: VecDeque::from([Ok(test_write_plan(&spec, address, 0x10, 0x42))]),
            applies: VecDeque::from([Err("write rejected".to_owned()), Ok(())]),
            applied_updates: Vec::new(),
        };

        let outcome = execute_write_evidence(&mut session, "C90", address, 0x42, |_| Ok(()));

        assert_eq!(outcome.test_write.status, "failed");
        assert_eq!(outcome.restoration.status, "completed");
        assert_eq!(outcome.restoration_readback.status, "completed");
        assert_eq!(
            session.applied_updates,
            vec![vec![(address, 0x42)], vec![(address, 0x10)]]
        );
    }

    #[test]
    fn write_evidence_report_distinguishes_restoration_failure_after_cleanup() {
        let spec = reink_core::ModelDatabase::builtin()
            .unwrap()
            .get("C90")
            .unwrap()
            .clone();
        let address = spec.memory_low;
        let mut session = MockWriteEvidenceSession {
            identities: VecDeque::from([Ok(Some("C90".to_owned()))]),
            reads: VecDeque::from([Ok(0x10), Ok(0x42)]),
            plans: VecDeque::from([Ok(test_write_plan(&spec, address, 0x10, 0x42))]),
            applies: VecDeque::from([Ok(()), Err("restore rejected".to_owned())]),
            applied_updates: Vec::new(),
        };
        let outcome = execute_write_evidence(&mut session, "C90", address, 0x42, |_| Ok(()));
        let cleanup = WriteEvidenceCleanup {
            d4_shutdown: WriteEvidenceStage::completed("shutdown complete", None),
            usb_close: WriteEvidenceStage::completed("USB close complete", None),
        };
        let connection = WriteEvidenceStage::completed("connected", None);
        let output = report(write_evidence_report(
            0x04b8,
            0x1234,
            0,
            0,
            1,
            2,
            "C90",
            address,
            0x42,
            Path::new("private/backup.bin"),
            &connection,
            Some(&outcome),
            &cleanup,
            false.into(),
        ));

        assert_eq!(output["mode"], "write_evidence");
        assert_eq!(output["status"], "failed");
        assert_eq!(output["stages"][6]["name"], "restoration");
        assert_eq!(output["stages"][6]["status"], "failed");
        assert!(
            output["remediation"]
                .as_str()
                .unwrap()
                .contains("Do not repeat")
        );
        assert_eq!(
            session.applied_updates,
            vec![vec![(address, 0x42)], vec![(address, 0x10)]]
        );
    }
}
