#![forbid(unsafe_code)]

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use reink_app::{EpsonD4EntryProbeResult, EpsonD4Session, probe_epson_d4_entry};
use reink_core::{EpsonSpec, ModelDatabase, PrinterIdentity};
#[cfg(target_os = "linux")]
use reink_discovery::LinuxDeviceFileDiscovery;
use reink_discovery::MdnsDiscovery;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use reink_platform::RecordingTransport;
use reink_platform::{DeviceDiscovery, DeviceLocation, DiscoveryRequest};
use reink_snmp::{SnmpConfig, SnmpControlChannel};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use reink_usb::read_printer_device_id;
use serde_json::json;

#[cfg(any(target_os = "linux", target_os = "macos", test))]
const EEPROM_WRITE_CONFIRMATION: &str = "I_CONFIRM_THIS_WILL_WRITE_EEPROM";
#[cfg(any(target_os = "linux", target_os = "macos", test))]
const EEPROM_RESTORE_CONFIRMATION: &str = "I_CONFIRM_THIS_WILL_RESTORE_EEPROM";

#[derive(Parser, Debug)]
#[command(
    name = "reink",
    version,
    about = "ReInk printer inspection and explicit EEPROM operations"
)]
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
        /// Optional USB bus number; requires --device-address.
        #[arg(long)]
        bus_number: Option<u8>,
        /// Optional USB device address; requires --bus-number.
        #[arg(long)]
        device_address: Option<u8>,
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
        /// Optional USB bus number; requires --device-address.
        #[arg(long)]
        bus_number: Option<u8>,
        /// Optional USB device address; requires --bus-number.
        #[arg(long)]
        device_address: Option<u8>,
    },
    /// Read a complete model-bounded EEPROM image and save it as a new binary file.
    UsbEepromDump {
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
        /// Model that must exactly match the D4 identity read from the selected printer.
        #[arg(long)]
        model: String,
        /// New private binary image path. Existing files are never overwritten.
        #[arg(long)]
        output_file: std::path::PathBuf,
    },
    /// Apply explicit EEPROM byte updates after saving a complete new backup.
    UsbEepromWrite {
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
        /// Model that must exactly match the D4 identity read from the selected printer.
        #[arg(long)]
        model: String,
        /// EEPROM update as ADDRESS=VALUE; both values accept decimal or 0x-prefixed hexadecimal.
        #[arg(long, required = true, value_parser = parse_eeprom_update)]
        update: Vec<(u16, u8)>,
        /// New complete EEPROM backup path. Existing files are never overwritten.
        #[arg(long)]
        backup_file: PathBuf,
        /// Exact acknowledgement required before this command opens USB.
        #[arg(long)]
        confirmation: Option<String>,
    },
    /// Restore a complete EEPROM image after saving a new rollback backup.
    UsbEepromRestore {
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
        /// Model that must exactly match the D4 identity read from the selected printer.
        #[arg(long)]
        model: String,
        /// Existing complete binary EEPROM image for the selected model range.
        #[arg(long)]
        input_file: PathBuf,
        /// New complete pre-restore EEPROM image path. Existing files are never overwritten.
        #[arg(long)]
        rollback_backup_file: PathBuf,
        /// Exact acknowledgement required before this command opens USB.
        #[arg(long)]
        confirmation: Option<String>,
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
            bus_number,
            device_address,
        } => usb_identity_output(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            cli.json,
        )?,
        Command::UsbD4Probe {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
        } => usb_d4_probe_output(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            cli.json,
        )?,
        Command::UsbEepromDump {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            model,
            output_file,
        } => usb_eeprom_dump_output(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            &model,
            &output_file,
            cli.json,
        )?,
        Command::UsbEepromWrite {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            model,
            update,
            backup_file,
            confirmation,
        } => usb_eeprom_write_output(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            &model,
            &update,
            &backup_file,
            confirmation.as_deref(),
            cli.json,
        )?,
        Command::UsbEepromRestore {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            model,
            input_file,
            rollback_backup_file,
            confirmation,
        } => usb_eeprom_restore_output(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            &model,
            &input_file,
            &rollback_backup_file,
            confirmation.as_deref(),
            cli.json,
        )?,
    };
    write_stdout(&output)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn usb_d4_probe_output(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: Option<u8>,
    device_address: Option<u8>,
    as_json: bool,
) -> Result<String, String> {
    let result = probe_epson_d4_entry(
        usb_device_selector(vendor_id, product_id, bus_number, device_address)?,
        reink_platform::UsbInterfaceSelector {
            number: interface,
            alternate_setting,
        },
    )
    .map_err(|error| error.to_string())?;
    let (status, received_bytes) = match result {
        EpsonD4EntryProbeResult::Recognized => ("recognized", None),
        EpsonD4EntryProbeResult::Unrecognized { received_bytes } => {
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

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn usb_d4_probe_output(
    _vendor_id: u16,
    _product_id: u16,
    _interface: u8,
    _alternate_setting: u8,
    _bus_number: Option<u8>,
    _device_address: Option<u8>,
    _as_json: bool,
) -> Result<String, String> {
    Err("USB D4 probing is currently supported only on Linux or macOS".to_owned())
}

fn parse_u16(value: &str) -> Result<u16, String> {
    let (value, radix) = match value.strip_prefix("0x") {
        Some(value) => (value, 16),
        None => (value, 10),
    };
    u16::from_str_radix(value, radix)
        .map_err(|_| format!("expected a decimal or 0x-prefixed 16-bit integer, got {value:?}"))
}

fn parse_u8(value: &str) -> Result<u8, String> {
    let (value, radix) = match value.strip_prefix("0x") {
        Some(value) => (value, 16),
        None => (value, 10),
    };
    u8::from_str_radix(value, radix)
        .map_err(|_| format!("expected a decimal or 0x-prefixed byte, got {value:?}"))
}

fn parse_eeprom_update(value: &str) -> Result<(u16, u8), String> {
    let (address, byte) = value
        .split_once('=')
        .ok_or_else(|| format!("expected EEPROM update in ADDRESS=VALUE form, got {value:?}"))?;
    Ok((parse_u16(address)?, parse_u8(byte)?))
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn validate_eeprom_updates(spec: &EpsonSpec, updates: &[(u16, u8)]) -> Result<(), String> {
    if updates.is_empty() {
        return Err("at least one --update ADDRESS=VALUE is required".to_owned());
    }
    let mut addresses = std::collections::BTreeSet::new();
    for &(address, _) in updates {
        if address < spec.memory_low || address > spec.memory_high {
            return Err(format!(
                "EEPROM update address {address:#06x} is outside model range {:#06x}..={:#06x}",
                spec.memory_low, spec.memory_high
            ));
        }
        if !addresses.insert(address) {
            return Err(format!(
                "EEPROM update address {address:#06x} is duplicated"
            ));
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn validate_confirmation(
    confirmation: Option<&str>,
    expected: &str,
    command: &str,
) -> Result<(), String> {
    if confirmation == Some(expected) {
        Ok(())
    } else {
        Err(format!(
            "{command} requires --confirmation {expected} exactly"
        ))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn validate_new_file_path(path: &Path, kind: &str) -> Result<(), String> {
    if path.exists() {
        return Err(format!(
            "refusing to overwrite existing {kind}: {}",
            path.display()
        ));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.is_dir()
    {
        return Err(format!(
            "{kind} parent directory does not exist: {}",
            parent.display()
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn read_restore_image(path: &Path, spec: &EpsonSpec) -> Result<Vec<(u16, u8)>, String> {
    let bytes = std::fs::read(path).map_err(|error| {
        format!(
            "could not read EEPROM restore image {}: {error}",
            path.display()
        )
    })?;
    let expected_len =
        usize::from(spec.memory_high).saturating_sub(usize::from(spec.memory_low)) + 1;
    if bytes.len() != expected_len {
        return Err(format!(
            "EEPROM restore image {} has {} bytes; model {} requires exactly {} bytes for {:#06x}..={:#06x}",
            path.display(),
            bytes.len(),
            spec.model,
            expected_len,
            spec.memory_low,
            spec.memory_high
        ));
    }
    Ok(bytes
        .into_iter()
        .enumerate()
        .map(|(offset, value)| (spec.memory_low + offset as u16, value))
        .collect())
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn usb_identity_output(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: Option<u8>,
    device_address: Option<u8>,
    as_json: bool,
) -> Result<String, String> {
    let bytes = read_printer_device_id(
        usb_device_selector(vendor_id, product_id, bus_number, device_address)?,
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn usb_eeprom_dump_output(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: Option<u8>,
    device_address: Option<u8>,
    model: &str,
    output_file: &Path,
    as_json: bool,
) -> Result<String, String> {
    validate_new_file_path(output_file, "EEPROM image")?;
    let spec = selected_model(model)?;
    let image = with_usb_eeprom_session(
        vendor_id,
        product_id,
        interface,
        alternate_setting,
        bus_number,
        device_address,
        spec,
        |session| {
            verify_requested_model(
                &session.read_identity().map_err(|error| error.to_string())?,
                model,
            )?;
            session.dump_eeprom().map_err(|error| error.to_string())
        },
    )?;
    write_new_binary_file(output_file, &image.bytes, "EEPROM image")?;

    Ok(if as_json {
        json!({
            "mode": "read_only",
            "model": image.model,
            "start_address": format!("{:04X}", image.start_address),
            "end_address": format!("{:04X}", image.end_address()),
            "byte_count": image.bytes.len(),
            "output_file": output_file,
        })
        .to_string()
    } else {
        format!(
            "Saved {}-byte EEPROM image for {} ({:#06x}..={:#06x}) to {}",
            image.bytes.len(),
            image.model,
            image.start_address,
            image.end_address(),
            output_file.display()
        )
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn usb_eeprom_write_output(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: Option<u8>,
    device_address: Option<u8>,
    model: &str,
    updates: &[(u16, u8)],
    backup_file: &Path,
    confirmation: Option<&str>,
    as_json: bool,
) -> Result<String, String> {
    validate_confirmation(confirmation, EEPROM_WRITE_CONFIRMATION, "usb-eeprom-write")?;
    validate_new_file_path(backup_file, "EEPROM backup")?;
    let spec = selected_model(model)?;
    validate_eeprom_updates(&spec, updates)?;

    with_usb_eeprom_session(
        vendor_id,
        product_id,
        interface,
        alternate_setting,
        bus_number,
        device_address,
        spec,
        |session| {
            verify_requested_model(
                &session.read_identity().map_err(|error| error.to_string())?,
                model,
            )?;
            let plan = session
                .prepare_eeprom_write(updates)
                .map_err(|error| format!("could not prepare EEPROM write: {error}"))?;
            write_new_binary_file(backup_file, &plan.backup.bytes, "EEPROM backup")?;
            session
                .apply_eeprom_write(&plan)
                .map_err(|error| format!("EEPROM write failed: {error}"))
        },
    )?;

    Ok(eeprom_mutation_output(
        "write",
        model,
        updates.len(),
        "backup_file",
        backup_file,
        as_json,
    ))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn usb_eeprom_restore_output(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: Option<u8>,
    device_address: Option<u8>,
    model: &str,
    input_file: &Path,
    rollback_backup_file: &Path,
    confirmation: Option<&str>,
    as_json: bool,
) -> Result<String, String> {
    validate_confirmation(
        confirmation,
        EEPROM_RESTORE_CONFIRMATION,
        "usb-eeprom-restore",
    )?;
    validate_new_file_path(rollback_backup_file, "EEPROM rollback backup")?;
    let spec = selected_model(model)?;
    let updates = read_restore_image(input_file, &spec)?;

    with_usb_eeprom_session(
        vendor_id,
        product_id,
        interface,
        alternate_setting,
        bus_number,
        device_address,
        spec,
        |session| {
            verify_requested_model(
                &session.read_identity().map_err(|error| error.to_string())?,
                model,
            )?;
            let plan = session
                .prepare_eeprom_write(&updates)
                .map_err(|error| format!("could not prepare EEPROM restore: {error}"))?;
            write_new_binary_file(
                rollback_backup_file,
                &plan.backup.bytes,
                "EEPROM rollback backup",
            )?;
            session
                .apply_eeprom_write(&plan)
                .map_err(|error| format!("EEPROM restore failed: {error}"))
        },
    )?;

    Ok(eeprom_mutation_output(
        "restore",
        model,
        updates.len(),
        "rollback_backup_file",
        rollback_backup_file,
        as_json,
    ))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn selected_model(model: &str) -> Result<EpsonSpec, String> {
    ModelDatabase::builtin()
        .map_err(|error| error.to_string())?
        .get(model)
        .cloned()
        .ok_or_else(|| format!("unknown model: {model}"))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn verify_requested_model(identity: &PrinterIdentity, model: &str) -> Result<(), String> {
    match identity.detected_model() {
        Some(detected) if detected == model => Ok(()),
        Some(detected) => Err(format!(
            "D4 printer identity model {detected:?} does not match requested model {model:?}"
        )),
        None => Err(format!(
            "D4 printer identity does not contain a model; requested model is {model:?}"
        )),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn write_new_binary_file(path: &Path, bytes: &[u8], kind: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("could not create {kind} {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("could not persist {kind} {}: {error}", path.display()))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn eeprom_mutation_output(
    operation: &str,
    model: &str,
    byte_count: usize,
    backup_key: &str,
    backup_file: &Path,
    as_json: bool,
) -> String {
    if as_json {
        json!({
            "operation": operation,
            "model": model,
            "byte_count": byte_count,
            (backup_key): backup_file,
        })
        .to_string()
    } else {
        format!(
            "EEPROM {operation} completed for {model}: {byte_count} byte(s); {backup_key}: {}",
            backup_file.display()
        )
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[allow(clippy::too_many_arguments)]
fn with_usb_eeprom_session<R>(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: Option<u8>,
    device_address: Option<u8>,
    spec: EpsonSpec,
    operation: impl FnOnce(
        &mut EpsonD4Session<RecordingTransport<reink_usb::ReadOnlyUsbTransport>>,
    ) -> Result<R, String>,
) -> Result<R, String> {
    let transport = reink_usb::ReadOnlyUsbTransport::open(
        usb_device_selector(vendor_id, product_id, bus_number, device_address)?,
        reink_platform::UsbInterfaceSelector {
            number: interface,
            alternate_setting,
        },
    )
    .map_err(|error| error.to_string())?;
    let mut session =
        match EpsonD4Session::connect_recoverable(RecordingTransport::new(transport), spec) {
            Ok(session) => session,
            Err((setup, transport)) => {
                let (mut transport, _) = transport.into_parts();
                return match transport.close() {
                    Ok(()) => Err(format!("D4 session setup failed: {setup}")),
                    Err(close) => Err(format!(
                        "D4 session setup failed: {setup}; USB transport close failed: {close}"
                    )),
                };
            }
        };
    let result = operation(&mut session);
    let shutdown = session.shutdown().map_err(|error| error.to_string());
    let (mut transport, _) = session.into_transport().into_parts();
    let close = transport.close().map_err(|error| error.to_string());
    match (result, shutdown, close) {
        (Ok(value), Ok(()), Ok(())) => Ok(value),
        (result, shutdown, close) => {
            let mut errors = Vec::new();
            if let Err(error) = result {
                errors.push(error);
            }
            if let Err(error) = shutdown {
                errors.push(format!("D4 shutdown failed: {error}"));
            }
            if let Err(error) = close {
                errors.push(format!("USB transport close failed: {error}"));
            }
            Err(errors.join("; "))
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[allow(clippy::too_many_arguments)]
fn usb_eeprom_dump_output(
    _: u16,
    _: u16,
    _: u8,
    _: u8,
    _: Option<u8>,
    _: Option<u8>,
    _: &str,
    _: &Path,
    _: bool,
) -> Result<String, String> {
    Err("USB EEPROM dumps are currently supported only on Linux or macOS".to_owned())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[allow(clippy::too_many_arguments)]
fn usb_eeprom_write_output(
    _: u16,
    _: u16,
    _: u8,
    _: u8,
    _: Option<u8>,
    _: Option<u8>,
    _: &str,
    _: &[(u16, u8)],
    _: &Path,
    _: Option<&str>,
    _: bool,
) -> Result<String, String> {
    Err("USB EEPROM writes are currently supported only on Linux or macOS".to_owned())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[allow(clippy::too_many_arguments)]
fn usb_eeprom_restore_output(
    _: u16,
    _: u16,
    _: u8,
    _: u8,
    _: Option<u8>,
    _: Option<u8>,
    _: &str,
    _: &Path,
    _: &Path,
    _: Option<&str>,
    _: bool,
) -> Result<String, String> {
    Err("USB EEPROM restores are currently supported only on Linux or macOS".to_owned())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn usb_identity_output(
    _vendor_id: u16,
    _product_id: u16,
    _interface: u8,
    _alternate_setting: u8,
    _bus_number: Option<u8>,
    _device_address: Option<u8>,
    _as_json: bool,
) -> Result<String, String> {
    Err("USB identity inspection is currently supported only on Linux or macOS".to_owned())
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

    use super::{
        Cli, Command, EEPROM_RESTORE_CONFIRMATION, EEPROM_WRITE_CONFIRMATION, parse_eeprom_update,
        render_identity, render_models, validate_confirmation, validate_eeprom_updates,
        validate_new_file_path,
    };

    #[test]
    fn parses_cli_commands() {
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
                bus_number: None,
                device_address: None,
            }
        ));
        let cli = Cli::try_parse_from([
            "reink",
            "usb-eeprom-write",
            "--vendor-id",
            "0x04b8",
            "--product-id",
            "1234",
            "--interface",
            "0",
            "--model",
            "C90",
            "--update",
            "0x000c=0xff",
            "--backup-file",
            "new-backup.bin",
            "--confirmation",
            EEPROM_WRITE_CONFIRMATION,
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::UsbEepromWrite {
                update,
                confirmation: Some(_),
                ..
            } if update == vec![(0x000c, 0xff)]
        ));
        let cli = Cli::try_parse_from([
            "reink",
            "usb-eeprom-restore",
            "--vendor-id",
            "0x04b8",
            "--product-id",
            "1234",
            "--interface",
            "0",
            "--model",
            "C90",
            "--input-file",
            "image.bin",
            "--rollback-backup-file",
            "new-rollback.bin",
            "--confirmation",
            EEPROM_RESTORE_CONFIRMATION,
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::UsbEepromRestore {
                confirmation: Some(_),
                ..
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

    #[test]
    fn parses_eeprom_updates_and_rejects_malformed_values() {
        assert_eq!(parse_eeprom_update("0x000c=0xff").unwrap(), (0x000c, 0xff));
        assert_eq!(parse_eeprom_update("12=34").unwrap(), (12, 34));
        assert!(parse_eeprom_update("0x000c").is_err());
        assert!(parse_eeprom_update("0x000c=0x100").is_err());
    }

    #[test]
    fn validates_write_gates_before_usb_access() {
        let database = ModelDatabase::builtin().unwrap();
        let spec = database.get("C90").unwrap();
        assert!(
            validate_confirmation(
                Some(EEPROM_WRITE_CONFIRMATION),
                EEPROM_WRITE_CONFIRMATION,
                "usb-eeprom-write"
            )
            .is_ok()
        );
        assert!(
            validate_confirmation(
                Some("I_CONFIRM_WRITES"),
                EEPROM_WRITE_CONFIRMATION,
                "usb-eeprom-write"
            )
            .is_err()
        );
        assert!(validate_eeprom_updates(spec, &[(spec.memory_low, 1)]).is_ok());
        assert!(validate_eeprom_updates(spec, &[]).is_err());
        assert!(
            validate_eeprom_updates(spec, &[(spec.memory_low, 1), (spec.memory_low, 2)]).is_err()
        );
        assert!(validate_eeprom_updates(spec, &[(spec.memory_high.saturating_add(1), 1)]).is_err());
    }

    #[test]
    fn refuses_existing_and_parentless_backup_paths() {
        let existing = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(validate_new_file_path(&existing, "EEPROM backup").is_err());
        assert!(
            validate_new_file_path(
                std::path::Path::new("missing-parent-for-reink-cli-test\\backup.bin"),
                "EEPROM backup"
            )
            .is_err()
        );
    }

    #[test]
    fn requires_complete_optional_usb_location() {
        assert!(super::usb_device_selector(0x04b8, 0x1234, Some(1), None).is_err());
        assert_eq!(
            super::usb_device_selector(0x04b8, 0x1234, Some(1), Some(2)).unwrap(),
            reink_usb::UsbDeviceSelector::at_location(0x04b8, 0x1234, 1, 2)
        );
    }
}
