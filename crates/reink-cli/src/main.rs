#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use reink_core::{ModelDatabase, PrinterIdentity};
#[cfg(target_os = "linux")]
use reink_discovery::LinuxDeviceFileDiscovery;
use reink_discovery::MdnsDiscovery;
use reink_platform::{DeviceDiscovery, DeviceLocation, DiscoveryRequest};
use reink_snmp::{SnmpConfig, SnmpControlChannel};
#[cfg(target_os = "linux")]
use reink_usb::{D4EntryProbeResult, probe_d4_entry, read_printer_device_id};
use serde_json::json;

#[derive(Parser, Debug)]
#[command(name = "reink", version, about = "Read-only ReInk printer inspection")]
struct Cli {
    /// Emit structured JSON instead of human-readable text.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// List models in the built-in Epson database.
    Models,
    /// Display the configured EEPROM metadata for one model.
    Model { name: String },
    /// Parse and display an IEEE 1284 device ID.
    ParseId { identifier: String },
    /// Discover IPP, IPPS, and printer services over mDNS.
    Discover {
        #[arg(long, default_value_t = 3)]
        timeout_seconds: u64,
    },
    /// List Linux printer device-file candidates without opening them.
    LocalDevices,
    /// Read an IEEE 1284 device ID via SNMP credentials from the environment.
    SnmpId,
    /// Read a standard USB Printer Class device ID without entering Epson D4 mode.
    UsbId {
        /// USB vendor ID in decimal or 0x-prefixed hexadecimal.
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        /// USB product ID in decimal or 0x-prefixed hexadecimal.
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        /// Explicit USB printer interface number.
        #[arg(long)]
        interface: u8,
        /// Explicit alternate setting for the printer interface.
        #[arg(long, default_value_t = 0)]
        alternate_setting: u8,
    },
    /// Probe only the Epson D4 entry reply; does not initialize D4 or open a service.
    UsbD4Probe {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        #[arg(long)]
        interface: u8,
        #[arg(long, default_value_t = 0)]
        alternate_setting: u8,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let output = match cli.command {
        Command::Models => {
            let database = ModelDatabase::builtin().map_err(|error| error.to_string())?;
            let models = database.models().collect::<Vec<_>>();
            render_models(&models, cli.json)
        }
        Command::Model { name } => {
            let database = ModelDatabase::builtin().map_err(|error| error.to_string())?;
            let spec = database
                .get(&name)
                .ok_or_else(|| format!("unknown model: {name}"))?;
            render_model(
                &spec.model,
                spec.read_key,
                spec.read_address_width.byte_len(),
                spec.write_address_width.byte_len(),
                spec.memory_low,
                spec.memory_high,
                spec.memory_operations
                    .iter()
                    .map(|operation| operation.description.as_str())
                    .collect::<Vec<_>>()
                    .as_slice(),
                cli.json,
            )
        }
        Command::ParseId { identifier } => {
            let identity =
                PrinterIdentity::parse(&identifier).map_err(|error| error.to_string())?;
            let database = ModelDatabase::builtin().map_err(|error| error.to_string())?;
            render_identity(&identity, &database, cli.json)
        }
        Command::Discover { timeout_seconds } => {
            let discovery = MdnsDiscovery;
            let devices = discovery
                .discover(DiscoveryRequest::new(Duration::from_secs(timeout_seconds)))
                .map_err(|error| error.to_string())?;
            render_discovery(devices, cli.json)
        }
        Command::LocalDevices => local_devices_output(cli.json)?,
        Command::SnmpId => {
            let config = SnmpConfig::from_environment().map_err(|error| error.to_string())?;
            let mut channel =
                SnmpControlChannel::connect(config).map_err(|error| error.to_string())?;
            let identity = channel
                .printer_identity()
                .map_err(|error| error.to_string())?;
            let database = ModelDatabase::builtin().map_err(|error| error.to_string())?;
            render_identity(&identity, &database, cli.json)
        }
        Command::UsbId {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
        } => usb_identity_output(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            cli.json,
        )?,
        Command::UsbD4Probe {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
        } => usb_d4_probe_output(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            cli.json,
        )?,
    };
    write_stdout(&output)
}

#[cfg(target_os = "linux")]
fn usb_d4_probe_output(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    as_json: bool,
) -> Result<String, String> {
    let result = probe_d4_entry(
        vendor_id,
        product_id,
        reink_platform::UsbInterfaceSelector {
            number: interface,
            alternate_setting,
        },
    )
    .map_err(|error| error.to_string())?;
    let (status, received_bytes) = match result {
        D4EntryProbeResult::Recognized => ("recognized", None),
        D4EntryProbeResult::Unrecognized { received_bytes } => {
            ("unrecognized", Some(received_bytes))
        }
    };
    Ok(if as_json {
        json!({ "d4_entry": status, "received_bytes": received_bytes }).to_string()
    } else {
        match received_bytes {
            Some(count) => format!("D4 entry reply: {status} ({count} bytes received)"),
            None => format!("D4 entry reply: {status}"),
        }
    })
}

#[cfg(not(target_os = "linux"))]
fn usb_d4_probe_output(
    _vendor_id: u16,
    _product_id: u16,
    _interface: u8,
    _alternate_setting: u8,
    _as_json: bool,
) -> Result<String, String> {
    Err("USB D4 probing is currently supported only on Linux".to_owned())
}

fn parse_u16(value: &str) -> Result<u16, String> {
    let (value, radix) = match value.strip_prefix("0x") {
        Some(value) => (value, 16),
        None => (value, 10),
    };
    u16::from_str_radix(value, radix)
        .map_err(|_| format!("expected a decimal or 0x-prefixed 16-bit integer, got {value:?}"))
}

#[cfg(target_os = "linux")]
fn usb_identity_output(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    as_json: bool,
) -> Result<String, String> {
    let bytes = read_printer_device_id(
        vendor_id,
        product_id,
        reink_platform::UsbInterfaceSelector {
            number: interface,
            alternate_setting,
        },
    )
    .map_err(|error| error.to_string())?;
    let identifier =
        std::str::from_utf8(&bytes).map_err(|_| "USB printer device ID is not UTF-8".to_owned())?;
    let identity = PrinterIdentity::parse(identifier).map_err(|error| error.to_string())?;
    let database = ModelDatabase::builtin().map_err(|error| error.to_string())?;
    Ok(render_identity(&identity, &database, as_json))
}

#[cfg(not(target_os = "linux"))]
fn usb_identity_output(
    _vendor_id: u16,
    _product_id: u16,
    _interface: u8,
    _alternate_setting: u8,
    _as_json: bool,
) -> Result<String, String> {
    Err("USB identity inspection is currently supported only on Linux".to_owned())
}

#[cfg(target_os = "linux")]
fn local_devices_output(as_json: bool) -> Result<String, String> {
    let devices = LinuxDeviceFileDiscovery
        .discover(DiscoveryRequest::new(Duration::ZERO))
        .map_err(|error| error.to_string())?;
    Ok(render_discovery(devices, as_json))
}

#[cfg(not(target_os = "linux"))]
fn local_devices_output(_as_json: bool) -> Result<String, String> {
    Err("Linux device-file discovery is unavailable on this platform".to_owned())
}

fn render_models(models: &[&str], as_json: bool) -> String {
    if as_json {
        json!({ "models": models }).to_string()
    } else {
        models.join("\n")
    }
}

#[allow(clippy::too_many_arguments)]
fn render_model(
    name: &str,
    read_key: u16,
    read_address_width: usize,
    write_address_width: usize,
    memory_low: u16,
    memory_high: u16,
    operations: &[&str],
    as_json: bool,
) -> String {
    if as_json {
        json!({
            "model": name,
            "read_key": format!("{read_key:04X}"),
            "address_widths": {
                "read": read_address_width,
                "write": write_address_width,
            },
            "memory_range": {
                "low": format!("{memory_low:04X}"),
                "high": format!("{memory_high:04X}"),
            },
            "operations": operations,
        })
        .to_string()
    } else {
        let mut output = format!(
            "model: {name}\nread-key: {read_key:04X}\naddress-widths: read={read_address_width} write={write_address_width}\nmemory-range: {memory_low:04X}-{memory_high:04X}"
        );
        for operation in operations {
            output.push_str("\noperation: ");
            output.push_str(operation);
        }
        output
    }
}

fn render_identity(identity: &PrinterIdentity, database: &ModelDatabase, as_json: bool) -> String {
    let detected_model = identity.detected_model();
    let resolved_model = database
        .resolve_identity(identity)
        .map(|spec| spec.model.as_str());
    if as_json {
        json!({
            "identity": identity.fields(),
            "detected_model": detected_model,
            "resolved_model": resolved_model,
        })
        .to_string()
    } else {
        let mut output = identity
            .fields()
            .iter()
            .map(|(key, value)| format!("{key}: {value}"))
            .collect::<Vec<_>>()
            .join("\n");
        output.push_str(&format!(
            "\ndetected-model: {}",
            detected_model.unwrap_or("unavailable")
        ));
        output.push_str(&format!(
            "\nresolved-model: {}",
            resolved_model.unwrap_or("no built-in match")
        ));
        output
    }
}

fn render_discovery(devices: Vec<reink_platform::DiscoveredDevice>, as_json: bool) -> String {
    let devices = devices
        .into_iter()
        .filter_map(|device| {
            let location = match device.location {
                DeviceLocation::Network { address } => address.to_string(),
                DeviceLocation::DeviceFile(path) => path.display().to_string(),
                DeviceLocation::Usb(_) => return None,
            };
            Some((device.display_name, location))
        })
        .collect::<Vec<_>>();
    if as_json {
        json!({
            "devices": devices
                .iter()
                .map(|(name, address)| json!({ "name": name, "address": address }))
                .collect::<Vec<_>>(),
        })
        .to_string()
    } else {
        devices
            .iter()
            .map(|(name, address)| format!("{name}\t{address}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn write_stdout(output: &str) -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    match writeln!(stdout, "{output}") {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(format!("write output failed: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use reink_core::ModelDatabase;

    use super::{Cli, Command, render_identity, render_models};

    #[test]
    fn parses_only_read_only_commands() {
        let cli = Cli::try_parse_from(["reink", "models"]).unwrap();
        assert!(matches!(cli.command, Command::Models));
        let cli = Cli::try_parse_from(["reink", "discover", "--timeout-seconds", "1"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Discover { timeout_seconds: 1 }
        ));
        let cli = Cli::try_parse_from(["reink", "--json", "snmp-id"]).unwrap();
        assert!(cli.json);
        assert!(matches!(cli.command, Command::SnmpId));
        let cli = Cli::try_parse_from(["reink", "local-devices"]).unwrap();
        assert!(matches!(cli.command, Command::LocalDevices));
        let cli = Cli::try_parse_from([
            "reink",
            "usb-id",
            "--vendor-id",
            "0x04b8",
            "--product-id",
            "1234",
            "--interface",
            "0",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::UsbId {
                vendor_id: 0x04b8,
                product_id: 1234,
                interface: 0,
                alternate_setting: 0,
            }
        ));
    }

    #[test]
    fn renders_json_without_credentials() {
        assert_eq!(render_models(&["C90"], true), r#"{"models":["C90"]}"#);
        let identity = reink_core::PrinterIdentity::parse("MFG:EPSON;MDL:C90;").unwrap();
        let database = ModelDatabase::builtin().unwrap();
        assert_eq!(
            render_identity(&identity, &database, true),
            r#"{"detected_model":"C90","identity":{"MDL":"C90","MFG":"EPSON"},"resolved_model":"C90"}"#
        );
    }

    #[test]
    fn reports_an_unmatched_identity_without_enabling_device_actions() {
        let identity = reink_core::PrinterIdentity::parse("MFG:EPSON;MDL:Unknown;").unwrap();
        let database = ModelDatabase::builtin().unwrap();

        assert_eq!(
            render_identity(&identity, &database, false),
            "MDL: Unknown\nMFG: EPSON\ndetected-model: Unknown\nresolved-model: no built-in match"
        );
    }

    #[test]
    fn parses_decimal_and_hexadecimal_usb_identifiers() {
        assert_eq!(super::parse_u16("0x04b8").unwrap(), 0x04b8);
        assert_eq!(super::parse_u16("1208").unwrap(), 1208);
        assert!(super::parse_u16("0xnope").is_err());
    }
}
