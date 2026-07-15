#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::{Parser, Subcommand};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use reink_app::{EpsonD4EntryProbeResult, probe_epson_d4_entry};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use reink_core::{ModelDatabase, PrinterIdentity};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use reink_usb::read_printer_device_id;
use serde_json::{Value, json};

const NON_EXECUTABLE_WRITE_CONFIRMATION: &str = "I_CONFIRM_THIS_DOES_NOT_EXECUTE_WRITES";

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
    },
    /// Explain why write validation is not available.
    WriteSequence,
    /// Create a non-executable write-validation safety-gate report.
    WriteValidationPlan {
        /// SHA-256 reference for a separately retained sanitized hardware-evidence report.
        #[arg(long)]
        evidence_sha256: Option<String>,
        /// Exact acknowledgement that this command cannot execute physical writes.
        #[arg(long)]
        confirmation: Option<String>,
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
        Command::ReadSequence {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
        } => read_sequence(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
        ),
        Command::WriteSequence => Err(
            "write validation is unavailable: it requires validated read fixtures, explicit device confirmation, backup/read-back/rollback evidence, and a separate safety review".to_owned()
        ),
        Command::WriteValidationPlan {
            evidence_sha256,
            confirmation,
        } => Ok(write_validation_plan_report(
            evidence_sha256.as_deref(),
            confirmation.as_deref(),
        )),
        Command::D4Identity {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            model,
        } => d4_identity(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            &model,
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
        } => d4_eeprom_read(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            &model,
            &address,
        ),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
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
) -> Result<String, String> {
    let spec = ModelDatabase::builtin()
        .map_err(|e| e.to_string())?
        .get(model)
        .ok_or_else(|| format!("unknown model: {model}"))?
        .clone();
    let transport = reink_usb::ReadOnlyUsbTransport::open(
        usb_device_selector(vendor_id, product_id, bus_number, device_address)?,
        reink_platform::UsbInterfaceSelector {
            number: interface,
            alternate_setting,
        },
    )
    .map_err(|e| e.to_string())?;
    let mut session =
        reink_app::EpsonD4Session::connect(transport, spec).map_err(|e| e.to_string())?;
    let values = session.read_eeprom(addresses).map_err(|e| e.to_string())?;
    session.shutdown().map_err(|e| e.to_string())?;
    Ok(d4_eeprom_read_report(
        values
            .iter()
            .map(|value| json!({"address": format!("{:04X}", value.address), "value": value.value}))
            .collect(),
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
) -> Result<String, String> {
    Err("hardware USB validation is currently supported only on Linux or macOS".to_owned())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn d4_identity(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: Option<u8>,
    device_address: Option<u8>,
    model: &str,
) -> Result<String, String> {
    let spec = ModelDatabase::builtin()
        .map_err(|e| e.to_string())?
        .get(model)
        .ok_or_else(|| format!("unknown model: {model}"))?
        .clone();
    let transport = reink_usb::ReadOnlyUsbTransport::open(
        usb_device_selector(vendor_id, product_id, bus_number, device_address)?,
        reink_platform::UsbInterfaceSelector {
            number: interface,
            alternate_setting,
        },
    )
    .map_err(|e| e.to_string())?;
    let mut session =
        reink_app::EpsonD4Session::connect(transport, spec).map_err(|e| e.to_string())?;
    let identity = session.read_identity().map_err(|e| e.to_string())?;
    session.shutdown().map_err(|e| e.to_string())?;
    Ok(d4_identity_report(json!(identity.fields())))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn d4_identity(
    _: u16,
    _: u16,
    _: u8,
    _: u8,
    _: Option<u8>,
    _: Option<u8>,
    _: &str,
) -> Result<String, String> {
    Err("hardware USB validation is currently supported only on Linux or macOS".to_owned())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn read_sequence(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: Option<u8>,
    device_address: Option<u8>,
) -> Result<String, String> {
    let selector = reink_platform::UsbInterfaceSelector {
        number: interface,
        alternate_setting,
    };
    let device = usb_device_selector(vendor_id, product_id, bus_number, device_address)?;
    let bytes = read_printer_device_id(device, selector).map_err(|error| error.to_string())?;
    let identity = PrinterIdentity::parse(
        std::str::from_utf8(&bytes).map_err(|_| "USB device ID is not UTF-8")?,
    )
    .map_err(|error| error.to_string())?;
    let database = ModelDatabase::builtin().map_err(|error| error.to_string())?;
    let resolved_model = database
        .resolve_identity(&identity)
        .map(|spec| spec.model.as_str());
    let entry = probe_epson_d4_entry(device, selector).map_err(|error| error.to_string())?;
    let d4_entry = match entry {
        EpsonD4EntryProbeResult::Recognized => json!({"status": "recognized"}),
        EpsonD4EntryProbeResult::Unrecognized { received_bytes } => {
            json!({"status": "unrecognized", "received_bytes": received_bytes})
        }
    };
    Ok(read_sequence_report(
        json!({"vendor_id": format!("{vendor_id:04x}"), "product_id": format!("{product_id:04x}"), "interface": interface, "alternate_setting": alternate_setting, "bytes_received": bytes.len()}),
        json!(identity.fields()),
        json!({"detected_model": identity.detected_model(), "resolved_model": resolved_model}),
        d4_entry,
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn read_sequence(
    _: u16,
    _: u16,
    _: u8,
    _: u8,
    _: Option<u8>,
    _: Option<u8>,
) -> Result<String, String> {
    Err("hardware USB validation is currently supported only on Linux or macOS".to_owned())
}

fn parse_u16(value: &str) -> Result<u16, String> {
    let (value, radix) = value
        .strip_prefix("0x")
        .map_or((value, 10), |value| (value, 16));
    u16::from_str_radix(value, radix)
        .map_err(|_| "expected a 16-bit decimal or 0x-prefixed hexadecimal integer".to_owned())
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
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

fn is_sha256_reference(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn safety_gate(name: &str, satisfied: bool, requirement: &str) -> Value {
    json!({
        "name": name,
        "status": if satisfied { "satisfied" } else { "blocked" },
        "requirement": requirement,
    })
}

fn write_validation_plan_report(
    evidence_sha256: Option<&str>,
    confirmation: Option<&str>,
) -> String {
    let evidence_is_sanitized_reference = evidence_sha256.is_some_and(is_sha256_reference);
    let explicit_non_executable_confirmation =
        confirmation == Some(NON_EXECUTABLE_WRITE_CONFIRMATION);
    json!({
        "schema_version": 1,
        "mode": "non_executable",
        "command": "write-validation-plan",
        "execution": "disabled",
        "evidence_sha256": evidence_is_sanitized_reference.then_some(evidence_sha256),
        "gates": [
            safety_gate(
                "sanitized-hardware-evidence-reference",
                evidence_is_sanitized_reference,
                "Provide the SHA-256 reference of a separately retained sanitized read-only hardware report.",
            ),
            safety_gate(
                "explicit-non-executable-confirmation",
                explicit_non_executable_confirmation,
                "Pass --confirmation I_CONFIRM_THIS_DOES_NOT_EXECUTE_WRITES exactly.",
            ),
            safety_gate(
                "separate-write-safety-review",
                false,
                "A human safety review must approve backup, read-back, rollback, and device-specific evidence before any future implementation can be considered.",
            ),
        ],
        "next_step": "Retain sanitized evidence and obtain separate human safety review. This command cannot enable, queue, or execute a physical write or reset.",
    })
    .to_string()
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn completed_step(name: &str, result: Value) -> Value {
    json!({"name": name, "status": "completed", "result": result})
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReadOnlyFailureKind {
    Blocked,
    Timeout,
    Malformed,
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
impl ReadOnlyFailureKind {
    fn status(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Timeout => "timeout",
            Self::Malformed => "malformed",
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
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

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn read_only_report(command: &str, steps: Vec<Value>, next_step: &str) -> String {
    json!({
        "schema_version": 2,
        "mode": "read_only",
        "command": command,
        "steps": steps,
        "next_step": next_step,
    })
    .to_string()
}

/// Produces a deterministic report for a hardware-independent driver simulation.
///
/// Concrete USB operations currently return their native error to preserve a
/// nonzero process exit. This helper defines the schema used by tests and by a
/// future opt-in runner that can retain partial read-only evidence safely.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
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
    read_only_report(command, steps, next_step)
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn read_sequence_report(
    usb: Value,
    identity: Value,
    model_resolution: Value,
    d4_entry: Value,
) -> String {
    read_only_report(
        "read-sequence",
        vec![
            completed_step("usb-device-id", usb),
            completed_step("parse-device-id", identity),
            completed_step("resolve-model", model_resolution),
            completed_step("d4-entry-probe", d4_entry),
        ],
        "Review this read-only preflight evidence before using d4-identity or d4-eeprom-read; write and reset validation remain unavailable.",
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn d4_identity_report(identity: Value) -> String {
    read_only_report(
        "d4-identity",
        vec![
            completed_step(
                "d4-session-connect",
                json!({"init": "completed", "service": "EPSON-CTRL"}),
            ),
            completed_step("identity-read", identity),
            completed_step("d4-session-shutdown", json!({"exit": "completed"})),
        ],
        "Review identity evidence before selecting any EEPROM addresses; write and reset validation remain unavailable.",
    )
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn d4_eeprom_read_report(values: Vec<Value>) -> String {
    read_only_report(
        "d4-eeprom-read",
        vec![
            completed_step(
                "d4-session-connect",
                json!({"init": "completed", "service": "EPSON-CTRL"}),
            ),
            completed_step("eeprom-read", json!({"values": values})),
            completed_step("d4-session-shutdown", json!({"exit": "completed"})),
        ],
        "Preserve this read-only EEPROM evidence for future write-safety review; write and reset validation remain unavailable.",
    )
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use serde_json::{Value, json};

    use super::{
        Cli, Command, NON_EXECUTABLE_WRITE_CONFIRMATION, ReadOnlyFailureKind,
        d4_eeprom_read_report, d4_identity_report, parse_u16, read_sequence_report,
        simulated_read_only_report, usb_device_selector, write_validation_plan_report,
    };

    fn report(output: String) -> Value {
        serde_json::from_str(&output).unwrap()
    }

    fn assert_completed_steps(report: &Value, expected_names: &[&str]) {
        assert_eq!(report["schema_version"], 2);
        assert_eq!(report["mode"], "read_only");
        let steps = report["steps"].as_array().unwrap();
        assert_eq!(steps.len(), expected_names.len());
        for (step, expected_name) in steps.iter().zip(expected_names) {
            assert_eq!(step["name"], *expected_name);
            assert_eq!(step["status"], "completed");
            assert!(step.get("result").is_some());
        }
    }

    #[test]
    fn read_sequence_uses_versioned_per_step_results() {
        let output = report(read_sequence_report(
            json!({"vendor_id": "04b8", "bytes_received": 25}),
            json!({"MFG": "EPSON", "MDL": "C90"}),
            json!({"detected_model": "C90", "resolved_model": "C90"}),
            json!({"status": "recognized"}),
        ));

        assert_eq!(output["command"], "read-sequence");
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
    fn d4_reports_preserve_read_only_step_evidence() {
        let identity = report(d4_identity_report(json!({"MFG": "EPSON", "MDL": "C90"})));
        assert_eq!(identity["command"], "d4-identity");
        assert_completed_steps(
            &identity,
            &["d4-session-connect", "identity-read", "d4-session-shutdown"],
        );
        assert_eq!(identity["steps"][1]["result"]["MDL"], "C90");

        let eeprom = report(d4_eeprom_read_report(vec![
            json!({"address": "000C", "value": 66}),
        ]));
        assert_eq!(eeprom["command"], "d4-eeprom-read");
        assert_completed_steps(
            &eeprom,
            &["d4-session-connect", "eeprom-read", "d4-session-shutdown"],
        );
        assert_eq!(eeprom["steps"][1]["result"]["values"][0]["address"], "000C");
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
                "Resolve the reported read-only condition before retrying; no write or reset is available.",
            ));
            assert_eq!(output["schema_version"], 2);
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
    fn parses_only_explicit_read_only_commands() {
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
                ref model,
                ref address,
            } if model == "C90" && address == &[0x000c]
        ));
        assert_eq!(parse_u16("0x04b8").unwrap(), 0x04b8);
        assert!(parse_u16("not-a-number").is_err());
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
    fn write_validation_plan_stays_non_executable_when_gates_are_blocked() {
        let plan = report(write_validation_plan_report(None, None));

        assert_eq!(plan["mode"], "non_executable");
        assert_eq!(plan["execution"], "disabled");
        assert!(plan["evidence_sha256"].is_null());
        assert!(
            plan["gates"]
                .as_array()
                .unwrap()
                .iter()
                .all(|gate| gate["status"] == "blocked")
        );
        assert!(
            plan["next_step"]
                .as_str()
                .unwrap()
                .contains("cannot enable")
        );
    }

    #[test]
    fn write_validation_plan_requires_sanitized_reference_and_exact_confirmation() {
        let evidence = "a".repeat(64);
        let plan = report(write_validation_plan_report(
            Some(&evidence),
            Some(NON_EXECUTABLE_WRITE_CONFIRMATION),
        ));
        assert_eq!(plan["execution"], "disabled");
        assert_eq!(plan["evidence_sha256"], evidence);
        assert_eq!(plan["gates"][0]["status"], "satisfied");
        assert_eq!(plan["gates"][1]["status"], "satisfied");
        assert_eq!(plan["gates"][2]["status"], "blocked");

        let invalid = report(write_validation_plan_report(
            Some("not-a-sha256-reference"),
            Some("I_CONFIRM_WRITES"),
        ));
        assert!(invalid["evidence_sha256"].is_null());
        assert_eq!(invalid["gates"][0]["status"], "blocked");
        assert_eq!(invalid["gates"][1]["status"], "blocked");
    }
}
