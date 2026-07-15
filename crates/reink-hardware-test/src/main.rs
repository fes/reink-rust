#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::{Parser, Subcommand};
#[cfg(target_os = "linux")]
use reink_core::{ModelDatabase, PrinterIdentity};
#[cfg(target_os = "linux")]
use reink_usb::{D4EntryProbeResult, probe_d4_entry, read_printer_device_id};
#[cfg(any(target_os = "linux", test))]
use serde_json::Value;
#[cfg(any(target_os = "linux", test))]
use serde_json::json;

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
    },
    /// Explain why write validation is not available.
    WriteSequence,
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
        Command::ReadSequence { vendor_id, product_id, interface, alternate_setting } =>
            read_sequence(vendor_id, product_id, interface, alternate_setting),
        Command::WriteSequence => Err(
            "write validation is unavailable: it requires validated read fixtures, explicit device confirmation, backup/read-back/rollback evidence, and a separate safety review".to_owned()
        ),
        Command::D4Identity { vendor_id, product_id, interface, alternate_setting, model } => d4_identity(vendor_id, product_id, interface, alternate_setting, &model),
        Command::D4EepromRead { vendor_id, product_id, interface, alternate_setting, model, address } => d4_eeprom_read(vendor_id, product_id, interface, alternate_setting, &model, &address),
    }
}

#[cfg(target_os = "linux")]
fn d4_eeprom_read(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    model: &str,
    addresses: &[u16],
) -> Result<String, String> {
    let spec = ModelDatabase::builtin()
        .map_err(|e| e.to_string())?
        .get(model)
        .ok_or_else(|| format!("unknown model: {model}"))?
        .clone();
    let transport = reink_usb::LinuxUsbTransport::open(
        vendor_id,
        product_id,
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

#[cfg(not(target_os = "linux"))]
fn d4_eeprom_read(_: u16, _: u16, _: u8, _: u8, _: &str, _: &[u16]) -> Result<String, String> {
    Err("hardware USB validation is currently supported only on Linux".to_owned())
}

#[cfg(target_os = "linux")]
fn d4_identity(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    model: &str,
) -> Result<String, String> {
    let spec = ModelDatabase::builtin()
        .map_err(|e| e.to_string())?
        .get(model)
        .ok_or_else(|| format!("unknown model: {model}"))?
        .clone();
    let transport = reink_usb::LinuxUsbTransport::open(
        vendor_id,
        product_id,
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

#[cfg(not(target_os = "linux"))]
fn d4_identity(_: u16, _: u16, _: u8, _: u8, _: &str) -> Result<String, String> {
    Err("hardware USB validation is currently supported only on Linux".to_owned())
}

#[cfg(target_os = "linux")]
fn read_sequence(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
) -> Result<String, String> {
    let selector = reink_platform::UsbInterfaceSelector {
        number: interface,
        alternate_setting,
    };
    let bytes = read_printer_device_id(vendor_id, product_id, selector)
        .map_err(|error| error.to_string())?;
    let identity = PrinterIdentity::parse(
        std::str::from_utf8(&bytes).map_err(|_| "USB device ID is not UTF-8")?,
    )
    .map_err(|error| error.to_string())?;
    let database = ModelDatabase::builtin().map_err(|error| error.to_string())?;
    let resolved_model = database
        .resolve_identity(&identity)
        .map(|spec| spec.model.as_str());
    let entry =
        probe_d4_entry(vendor_id, product_id, selector).map_err(|error| error.to_string())?;
    let d4_entry = match entry {
        D4EntryProbeResult::Recognized => json!({"status": "recognized"}),
        D4EntryProbeResult::Unrecognized { received_bytes } => {
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

#[cfg(not(target_os = "linux"))]
fn read_sequence(_: u16, _: u16, _: u8, _: u8) -> Result<String, String> {
    Err("hardware USB validation is currently supported only on Linux".to_owned())
}

fn parse_u16(value: &str) -> Result<u16, String> {
    let (value, radix) = value
        .strip_prefix("0x")
        .map_or((value, 10), |value| (value, 16));
    u16::from_str_radix(value, radix)
        .map_err(|_| "expected a 16-bit decimal or 0x-prefixed hexadecimal integer".to_owned())
}

#[cfg(any(target_os = "linux", test))]
fn completed_step(name: &str, result: Value) -> Value {
    json!({"name": name, "status": "completed", "result": result})
}

#[cfg(any(target_os = "linux", test))]
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

#[cfg(any(target_os = "linux", test))]
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

#[cfg(any(target_os = "linux", test))]
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

#[cfg(any(target_os = "linux", test))]
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
        Cli, Command, d4_eeprom_read_report, d4_identity_report, parse_u16, read_sequence_report,
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
                ref model,
                ref address,
            } if model == "C90" && address == &[0x000c]
        ));
        assert_eq!(parse_u16("0x04b8").unwrap(), 0x04b8);
        assert!(parse_u16("not-a-number").is_err());
    }
}
