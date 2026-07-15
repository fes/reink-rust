#![forbid(unsafe_code)]

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use reink_core::{ModelDatabase, PrinterIdentity};
#[cfg(target_os = "linux")]
use reink_usb::{D4EntryProbeResult, probe_d4_entry, read_printer_device_id};
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
    }
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
    let model = identity.model().unwrap_or_default();
    let known_model = ModelDatabase::builtin()
        .map_err(|error| error.to_string())?
        .get(model)
        .is_some();
    let entry =
        probe_d4_entry(vendor_id, product_id, selector).map_err(|error| error.to_string())?;
    let d4_entry = match entry {
        D4EntryProbeResult::Recognized => json!({"status": "recognized"}),
        D4EntryProbeResult::Unrecognized { received_bytes } => {
            json!({"status": "unrecognized", "received_bytes": received_bytes})
        }
    };
    Ok(json!({
        "schema_version": 1,
        "mode": "read_only",
        "usb": {"vendor_id": format!("{vendor_id:04x}"), "product_id": format!("{product_id:04x}"), "interface": interface, "alternate_setting": alternate_setting},
        "identity": identity.fields(),
        "model_database_match": known_model,
        "d4_entry": d4_entry,
        "next_step": "D4 Init and EPSON-CTRL are intentionally not implemented in this driver yet"
    }).to_string())
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
