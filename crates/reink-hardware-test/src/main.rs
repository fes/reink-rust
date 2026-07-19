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
    with_selected_windows_native_experimental_mutation_session,
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

mod evidence_files;
use evidence_files::{
    normalized_new_file_path, trace_json, trace_to_transcript, validate_private_new_file_path,
    validate_report_file_path, validate_trace_file_path, write_new_private_binary_file,
    write_report_file,
};
#[cfg(test)]
use evidence_files::{parse_trace_events, transcript_template};

const TRACE_SANITIZATION_CONFIRMATION: &str = "I_CONFIRM_TRACE_IS_SANITIZED";
const WRITE_EVIDENCE_WRITE_CONFIRMATION: &str = "I_CONFIRM_THIS_WILL_WRITE_EEPROM";
const WRITE_EVIDENCE_RESTORATION_CONFIRMATION: &str =
    "I_CONFIRM_THIS_WILL_RESTORE_EEPROM_AND_RETAIN_PRIVATE_EVIDENCE";
#[cfg(target_os = "windows")]
const WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT: &str =
    "I_ACKNOWLEDGE_WINDOWS_NATIVE_MUTATION_IS_EXPERIMENTAL";
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
    /// Inspect kernel-driver ownership without claiming or detaching the interface.
    UsbDriverState {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        #[arg(long)]
        interface: u8,
        #[arg(long, default_value_t = 0)]
        alternate_setting: u8,
        #[arg(long)]
        bus_number: u8,
        #[arg(long)]
        device_address: u8,
    },
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
    /// Experimental Windows USBPRINT single-byte reversible mutation evidence.
    #[cfg(target_os = "windows")]
    WindowsNativeD4EepromWriteEvidence {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        /// Optional documented USB interface number; VID/PID-only ambiguity is rejected.
        #[arg(long)]
        interface: Option<u8>,
        #[arg(long)]
        model: String,
        #[arg(long, value_parser = parse_u16)]
        address: u16,
        #[arg(long, value_parser = parse_u8)]
        value: u8,
        #[arg(long)]
        backup_file: PathBuf,
        #[arg(long)]
        report_file: PathBuf,
        #[arg(long)]
        confirm_write: Option<String>,
        #[arg(long)]
        confirm_restoration_evidence: Option<String>,
        #[arg(long)]
        confirm_native_experimental_mutation: Option<String>,
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
        Command::UsbDriverState {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
        } => usb_driver_state(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
        ),
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
        #[cfg(target_os = "windows")]
        Command::WindowsNativeD4EepromWriteEvidence {
            vendor_id,
            product_id,
            interface,
            model,
            address,
            value,
            backup_file,
            report_file,
            confirm_write,
            confirm_restoration_evidence,
            confirm_native_experimental_mutation,
        } => windows_native_d4_eeprom_write_evidence(
            vendor_id,
            product_id,
            interface,
            &model,
            address,
            value,
            &backup_file,
            &report_file,
            confirm_write.as_deref(),
            confirm_restoration_evidence.as_deref(),
            confirm_native_experimental_mutation.as_deref(),
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

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn usb_driver_state(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: u8,
    device_address: u8,
) -> Result<String, String> {
    let state = reink_usb::inspect_usb_driver_state(
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
    )
    .map_err(|error| error.to_string())?;
    Ok(usb_driver_state_report(
        vendor_id,
        product_id,
        interface,
        alternate_setting,
        bus_number,
        device_address,
        state,
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn usb_driver_state(_: u16, _: u16, _: u8, _: u8, _: u8, _: u8) -> Result<String, String> {
    Err(
        "USB driver-state inspection is currently supported only on Linux, macOS, or Windows"
            .to_owned(),
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
#[allow(clippy::too_many_arguments)]
fn usb_driver_state_report(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: u8,
    device_address: u8,
    state: reink_usb::UsbDriverState,
) -> String {
    let state = match state {
        reink_usb::UsbDriverState::Active => "active",
        reink_usb::UsbDriverState::Inactive => "inactive",
        reink_usb::UsbDriverState::Unsupported => "unsupported",
    };
    json!({
        "schema_version": 1,
        "mode": "read_only",
        "command": "usb-driver-state",
        "selector": {
            "vendor_id": format!("{vendor_id:04X}"),
            "product_id": format!("{product_id:04X}"),
            "interface": interface,
            "alternate_setting": alternate_setting,
            "bus_number": bus_number,
            "device_address": device_address,
        },
        "driver_state": state,
        "device_wide_handoff_on_macos": true,
        "traffic_sent": false,
    })
    .to_string()
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
            let capabilities = candidate.capabilities();
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
                    "d4_read": capabilities.d4_read,
                    "usb_device_id": capabilities.usb_device_id,
                    "persistent_mutation": capabilities.persistent_mutation,
                    "experimental_mutation": capabilities.experimental_mutation,
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
        "driver_handoff": driver_handoff.platform_json(),
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

#[cfg(target_os = "windows")]
fn native_write_evidence_cleanup(cleanup: &reink_app::UsbSessionCleanup) -> WriteEvidenceCleanup {
    let stage =
        |succeeded: &str, not_attempted: &str, status: &reink_app::UsbCleanupStatus| match status {
            reink_app::UsbCleanupStatus::Succeeded => {
                WriteEvidenceStage::completed(succeeded, None)
            }
            reink_app::UsbCleanupStatus::Failed(error) => {
                WriteEvidenceStage::failed(format!("{succeeded}: {error}"))
            }
            reink_app::UsbCleanupStatus::NotAttempted => WriteEvidenceStage::skipped(not_attempted),
        };
    WriteEvidenceCleanup {
        d4_shutdown: stage(
            "D4 service closed and Exit completed",
            "D4 shutdown was not attempted",
            &cleanup.d4_shutdown,
        ),
        usb_close: stage(
            "Windows native USBPRINT interface closed",
            "Windows native USBPRINT close was not attempted",
            &cleanup.usb_close,
        ),
    }
}

#[cfg(target_os = "windows")]
#[allow(clippy::too_many_arguments)]
fn windows_native_write_evidence_report(
    vendor_id: u16,
    product_id: u16,
    interface: Option<u8>,
    model: &str,
    address: u16,
    requested_value: u8,
    backup_file: &Path,
    connection: &WriteEvidenceStage,
    outcome: Option<&WriteEvidenceOutcome>,
    cleanup: &WriteEvidenceCleanup,
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
        write_evidence_stage_json("windows-native-usbprint-close", &cleanup.usb_close),
    ]);
    let completed_safely = connection.succeeded()
        && outcome.is_some_and(WriteEvidenceOutcome::completed_safely)
        && cleanup.succeeded();
    json!({
        "schema_version": 1,
        "mode": "write_evidence",
        "command": "windows-native-d4-eeprom-write-evidence",
        "backend": "windows_native_usbprint",
        "experimental_unvalidated": true,
        "status": if completed_safely { "completed" } else { "failed" },
        "selector": {
            "vendor_id": format!("{vendor_id:04X}"),
            "product_id": format!("{product_id:04X}"),
            "interface": interface,
        },
        "model": model,
        "test": {
            "address": format!("{address:04X}"),
            "requested_value": format!("{requested_value:02X}"),
            "original_value": outcome.and_then(|outcome| outcome.original_value).map(|value| format!("{value:02X}")),
        },
        "backup_file": backup_file,
        "stages": stages,
        "remediation": (!completed_safely).then_some(WRITE_EVIDENCE_REMEDIATION),
        "next_step": "The test byte must be restored and independently verified; native USBPRINT mutation remains experimental.",
    })
    .to_string()
}

#[cfg(target_os = "windows")]
#[allow(clippy::too_many_arguments)]
fn windows_native_d4_eeprom_write_evidence(
    vendor_id: u16,
    product_id: u16,
    interface: Option<u8>,
    model: &str,
    address: u16,
    requested_value: u8,
    backup_file: &Path,
    report_file: &Path,
    write_confirmation: Option<&str>,
    restoration_confirmation: Option<&str>,
    native_acknowledgement: Option<&str>,
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
    if native_acknowledgement != Some(WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT) {
        return Err(format!(
            "windows-native-d4-eeprom-write-evidence requires --confirm-native-experimental-mutation {WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT} exactly"
        ));
    }
    let candidate = match selected_windows_native_candidate(vendor_id, product_id, interface) {
        Ok(candidate) => candidate,
        Err(error) => {
            let connection = WriteEvidenceStage::failed(format!(
                "Windows native USBPRINT candidate selection failed: {error}"
            ));
            let cleanup = WriteEvidenceCleanup::not_started();
            let outcome = WriteEvidenceOutcome::new();
            let report = windows_native_write_evidence_report(
                vendor_id,
                product_id,
                interface,
                model,
                address,
                requested_value,
                backup_file,
                &connection,
                Some(&outcome),
                &cleanup,
            );
            return finish_write_evidence_report(report_file, report, false);
        }
    };
    let outcome =
        with_selected_windows_native_experimental_mutation_session(&candidate, spec, |session| {
            Ok(execute_write_evidence(
                session,
                model,
                address,
                requested_value,
                |bytes| write_new_private_binary_file(backup_file, bytes, "complete EEPROM backup"),
            ))
        });
    let cleanup = native_write_evidence_cleanup(&outcome.cleanup);
    let connection = if outcome.operation.is_ok() {
        WriteEvidenceStage::completed("experimental Windows native USBPRINT D4 initialized", None)
    } else {
        WriteEvidenceStage::failed(
            outcome
                .operation
                .as_ref()
                .err()
                .cloned()
                .unwrap_or_else(|| "Windows native D4 operation failed".to_owned()),
        )
    };
    let evidence = outcome.operation.ok();
    let completed_safely = evidence
        .as_ref()
        .is_some_and(WriteEvidenceOutcome::completed_safely)
        && cleanup.succeeded()
        && connection.succeeded();
    let report = windows_native_write_evidence_report(
        vendor_id,
        product_id,
        interface,
        model,
        address,
        requested_value,
        backup_file,
        &connection,
        evidence.as_ref(),
        &cleanup,
    );
    finish_write_evidence_report(report_file, report, completed_safely)
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
#[allow(clippy::too_many_arguments)]
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

    fn platform_json(self) -> Value {
        json!({
            "platform": std::env::consts::OS,
            "scope": if cfg!(target_os = "macos") {
                "device"
            } else if cfg!(target_os = "linux") {
                "interface"
            } else {
                "none"
            },
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
        "driver_handoff": driver_handoff.platform_json(),
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
        "driver_handoff": driver_handoff.platform_json(),
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
mod tests;
