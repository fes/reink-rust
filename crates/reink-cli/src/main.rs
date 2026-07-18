#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use reink_app::{EpsonD4EntryProbeResult, EpsonD4Session, probe_epson_d4_entry};
#[cfg(target_os = "windows")]
use reink_app::{
    ReadOnlyEpsonD4Session, SelectedUsbSessionOutcome, with_selected_windows_native_epson_session,
    with_selected_windows_native_experimental_mutation_session,
};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
use reink_core::CounterResetTarget;
use reink_core::{EepromReadReply, EpsonController, EpsonSpec, ModelDatabase, PrinterIdentity};
#[cfg(target_os = "linux")]
use reink_discovery::LinuxDeviceFileDiscovery;
use reink_discovery::MdnsDiscovery;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use reink_platform::RecordingTransport;
use reink_platform::{DeviceDiscovery, DeviceLocation, DiscoveryRequest};
use reink_snmp::{SnmpConfig, SnmpControlChannel};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use reink_usb::read_printer_device_id;
use serde_json::json;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
const EEPROM_WRITE_CONFIRMATION: &str = "I_CONFIRM_THIS_WILL_WRITE_EEPROM";
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
const EEPROM_RESTORE_CONFIRMATION: &str = "I_CONFIRM_THIS_WILL_RESTORE_EEPROM";
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
const EEPROM_COUNTER_RESET_CONFIRMATION: &str = "I_CONFIRM_THIS_WILL_RESET_DECLARED_COUNTERS";
#[cfg(target_os = "windows")]
const WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT: &str =
    "I_ACKNOWLEDGE_WINDOWS_NATIVE_MUTATION_IS_EXPERIMENTAL";
const MAX_OFFLINE_BINARY_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum CounterResetSelection {
    Waste,
    PlatenPad,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
impl CounterResetSelection {
    const fn target(self) -> CounterResetTarget {
        match self {
            Self::Waste => CounterResetTarget::Waste,
            Self::PlatenPad => CounterResetTarget::PlatenPad,
        }
    }
}

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
    /// Analyze a local binary or capture for Epson EEPROM factory requests without opening a device.
    AnalyzeBinary {
        /// Existing local file to inspect. Files above 64 MiB are refused.
        input_file: PathBuf,
        /// Include printable eight-character runs for .pcapng input too.
        #[arg(long)]
        include_ascii: bool,
    },
    /// Discover IPP, IPPS, and printer services over mDNS.
    Discover {
        #[arg(long, default_value_t = 3)]
        timeout_seconds: u64,
    },
    /// List Linux printer device-file candidates without opening them.
    LocalDevices,
    /// Read an IEEE 1284 device ID via SNMP credentials from the environment.
    SnmpId,
    /// Read Epson printer status through SNMP after validating the identified model.
    SnmpStatus,
    /// Read selected in-range EEPROM addresses through SNMP after an exact model check.
    SnmpEepromRead {
        /// Model that must exactly match the SNMP identity read from the printer.
        #[arg(long)]
        model: String,
        /// EEPROM address in decimal or 0x-prefixed hexadecimal; repeat for multiple addresses.
        #[arg(long, required = true, value_parser = parse_u16)]
        address: Vec<u16>,
    },
    /// Save a complete model-bounded EEPROM image read through SNMP as a new binary file.
    SnmpEepromDump {
        /// Model that must exactly match the SNMP identity read from the printer.
        #[arg(long)]
        model: String,
        /// New private binary image path. Existing files are never overwritten.
        #[arg(long)]
        output_file: std::path::PathBuf,
    },
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
    /// List read-only USBPRINT candidates available through the Windows stock driver.
    #[cfg(target_os = "windows")]
    WindowsNativeCandidates,
    /// Read and validate D4 identity through the read-only Windows stock-driver backend.
    #[cfg(target_os = "windows")]
    WindowsNativeIdentity {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        /// Optional USB interface number from a documented hardware ID.
        #[arg(long)]
        interface: Option<u8>,
        /// Expected model used for D4 framing and exact identity validation.
        #[arg(long)]
        model: String,
    },
    /// Read Epson status through the read-only Windows stock-driver backend.
    #[cfg(target_os = "windows")]
    WindowsNativeStatus {
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
    WindowsNativeEepromRead {
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
    /// Save a model-bounded EEPROM dump read through Windows USBPRINT.
    #[cfg(target_os = "windows")]
    WindowsNativeEepromDump {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        #[arg(long)]
        interface: Option<u8>,
        #[arg(long)]
        model: String,
        /// New private binary image path. Existing files are never overwritten.
        #[arg(long)]
        output_file: PathBuf,
    },
    /// Experimental/unvalidated Windows USBPRINT EEPROM byte write.
    #[cfg(target_os = "windows")]
    WindowsNativeExperimentalEepromWrite {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        #[arg(long)]
        interface: Option<u8>,
        #[arg(long)]
        model: String,
        #[arg(long, required = true, value_parser = parse_eeprom_update)]
        update: Vec<(u16, u8)>,
        #[arg(long)]
        backup_file: PathBuf,
        #[arg(long)]
        confirmation: Option<String>,
        #[arg(long)]
        native_experimental_acknowledgement: Option<String>,
    },
    /// Experimental/unvalidated Windows USBPRINT complete EEPROM restore.
    #[cfg(target_os = "windows")]
    WindowsNativeExperimentalEepromRestore {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        #[arg(long)]
        interface: Option<u8>,
        #[arg(long)]
        model: String,
        #[arg(long)]
        input_file: PathBuf,
        #[arg(long)]
        rollback_backup_file: PathBuf,
        #[arg(long)]
        confirmation: Option<String>,
        #[arg(long)]
        native_experimental_acknowledgement: Option<String>,
    },
    /// Experimental/unvalidated Windows USBPRINT declared counter reset.
    #[cfg(target_os = "windows")]
    WindowsNativeExperimentalEepromReset {
        #[arg(long, value_parser = parse_u16)]
        vendor_id: u16,
        #[arg(long, value_parser = parse_u16)]
        product_id: u16,
        #[arg(long)]
        interface: Option<u8>,
        #[arg(long)]
        model: String,
        #[arg(long, value_enum)]
        target: CounterResetSelection,
        #[arg(long)]
        backup_file: PathBuf,
        #[arg(long)]
        confirmation: Option<String>,
        #[arg(long)]
        native_experimental_acknowledgement: Option<String>,
    },
    /// Read Epson printer status over an explicitly selected USB D4 session.
    UsbStatus {
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
        /// Model that must exactly match the D4 identity read from the selected printer.
        #[arg(long)]
        model: String,
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
    /// Reset only explicitly declared waste or platen-pad counter bytes after saving a complete new backup.
    UsbEepromReset {
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
        /// Declared counter family to reset; values without explicit reset bytes are excluded.
        #[arg(long, value_enum)]
        target: CounterResetSelection,
        /// New complete EEPROM backup path. Existing files are never overwritten.
        #[arg(long)]
        backup_file: PathBuf,
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
        Command::AnalyzeBinary {
            input_file,
            include_ascii,
        } => analyze_binary_output(&input_file, include_ascii, cli.json)?,
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
        Command::SnmpStatus => snmp_status_output(cli.json)?,
        Command::SnmpEepromRead { model, address } => {
            snmp_eeprom_read_output(&model, &address, cli.json)?
        }
        Command::SnmpEepromDump { model, output_file } => {
            snmp_eeprom_dump_output(&model, &output_file, cli.json)?
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
        #[cfg(target_os = "windows")]
        Command::WindowsNativeCandidates => windows_native_candidates_output(cli.json)?,
        #[cfg(target_os = "windows")]
        Command::WindowsNativeIdentity {
            vendor_id,
            product_id,
            interface,
            model,
        } => windows_native_identity_output(vendor_id, product_id, interface, &model, cli.json)?,
        #[cfg(target_os = "windows")]
        Command::WindowsNativeStatus {
            vendor_id,
            product_id,
            interface,
            model,
        } => windows_native_status_output(vendor_id, product_id, interface, &model, cli.json)?,
        #[cfg(target_os = "windows")]
        Command::WindowsNativeEepromRead {
            vendor_id,
            product_id,
            interface,
            model,
            address,
        } => windows_native_eeprom_read_output(
            vendor_id, product_id, interface, &model, &address, cli.json,
        )?,
        #[cfg(target_os = "windows")]
        Command::WindowsNativeEepromDump {
            vendor_id,
            product_id,
            interface,
            model,
            output_file,
        } => windows_native_eeprom_dump_output(
            vendor_id,
            product_id,
            interface,
            &model,
            &output_file,
            cli.json,
        )?,
        #[cfg(target_os = "windows")]
        Command::WindowsNativeExperimentalEepromWrite {
            vendor_id,
            product_id,
            interface,
            model,
            update,
            backup_file,
            confirmation,
            native_experimental_acknowledgement,
        } => windows_native_experimental_mutation_output(
            vendor_id,
            product_id,
            interface,
            &model,
            update,
            &backup_file,
            confirmation.as_deref(),
            native_experimental_acknowledgement.as_deref(),
            EEPROM_WRITE_CONFIRMATION,
            "write",
            "EEPROM backup",
            cli.json,
        )?,
        #[cfg(target_os = "windows")]
        Command::WindowsNativeExperimentalEepromRestore {
            vendor_id,
            product_id,
            interface,
            model,
            input_file,
            rollback_backup_file,
            confirmation,
            native_experimental_acknowledgement,
        } => {
            let spec = selected_model(&model)?;
            let updates = read_restore_image(&input_file, &spec)?;
            windows_native_experimental_mutation_output(
                vendor_id,
                product_id,
                interface,
                &model,
                updates,
                &rollback_backup_file,
                confirmation.as_deref(),
                native_experimental_acknowledgement.as_deref(),
                EEPROM_RESTORE_CONFIRMATION,
                "restore",
                "EEPROM rollback backup",
                cli.json,
            )?
        }
        #[cfg(target_os = "windows")]
        Command::WindowsNativeExperimentalEepromReset {
            vendor_id,
            product_id,
            interface,
            model,
            target,
            backup_file,
            confirmation,
            native_experimental_acknowledgement,
        } => {
            let spec = selected_model(&model)?;
            let updates = reink_app::declared_counter_reset_updates(&spec, target.target())?;
            windows_native_experimental_mutation_output(
                vendor_id,
                product_id,
                interface,
                &model,
                updates,
                &backup_file,
                confirmation.as_deref(),
                native_experimental_acknowledgement.as_deref(),
                EEPROM_COUNTER_RESET_CONFIRMATION,
                "declared counter reset",
                "EEPROM reset backup",
                cli.json,
            )?
        }
        Command::UsbStatus {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            model,
        } => usb_status_output(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            &model,
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
        Command::UsbEepromReset {
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            model,
            target,
            backup_file,
            confirmation,
        } => usb_eeprom_reset_output(
            vendor_id,
            product_id,
            interface,
            alternate_setting,
            bus_number,
            device_address,
            &model,
            target,
            &backup_file,
            confirmation.as_deref(),
            cli.json,
        )?,
    };
    write_stdout(&output)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
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

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn usb_d4_probe_output(
    _vendor_id: u16,
    _product_id: u16,
    _interface: u8,
    _alternate_setting: u8,
    _bus_number: Option<u8>,
    _device_address: Option<u8>,
    _as_json: bool,
) -> Result<String, String> {
    Err("USB D4 probing is currently supported only on Linux, macOS, or Windows".to_owned())
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

#[derive(Clone, Debug, Eq, PartialEq)]
enum BinaryFinding {
    EepromRead {
        offset: usize,
        read_key: u16,
        address: u16,
    },
    EepromWrite {
        offset: usize,
        read_key: u16,
        address: u16,
        value: u8,
        write_key: Vec<u8>,
    },
    InvalidFactoryRequest {
        offset: usize,
        reason: &'static str,
    },
    PrintableAscii {
        offset: usize,
        value: String,
    },
}

fn analyze_binary_output(
    input_file: &Path,
    include_ascii: bool,
    as_json: bool,
) -> Result<String, String> {
    let metadata = std::fs::metadata(input_file)
        .map_err(|error| format!("could not inspect {}: {error}", input_file.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "offline binary analysis requires a regular file: {}",
            input_file.display()
        ));
    }
    if metadata.len() > MAX_OFFLINE_BINARY_BYTES {
        return Err(format!(
            "refusing to analyze {}: {} bytes exceeds the {}-byte offline limit",
            input_file.display(),
            metadata.len(),
            MAX_OFFLINE_BINARY_BYTES
        ));
    }
    let bytes = std::fs::read(input_file)
        .map_err(|error| format!("could not read {}: {error}", input_file.display()))?;
    let is_pcapng = input_file
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pcapng"));
    let findings = analyze_binary_bytes(&bytes, include_ascii || !is_pcapng);
    Ok(render_binary_findings(input_file, &findings, as_json))
}

/// Safely recognizes the `search_bin` Epson factory-request signatures in
/// already captured local bytes. It never decodes or executes arbitrary input.
fn analyze_binary_bytes(bytes: &[u8], include_ascii: bool) -> Vec<BinaryFinding> {
    let mut findings = Vec::new();
    let mut offset: usize = 0;
    while offset.saturating_add(9) <= bytes.len() {
        if bytes[offset..].starts_with(b"||") {
            let command = &bytes[offset + 6..offset + 9];
            let is_read = command == [b'A', !b'A', 0xa0];
            let is_write = command == [b'B', !b'B', 0x21];
            if is_read || is_write {
                let declared_length =
                    usize::from(u16::from_le_bytes([bytes[offset + 2], bytes[offset + 3]]));
                let Some(payload_length) = declared_length.checked_sub(5) else {
                    findings.push(BinaryFinding::InvalidFactoryRequest {
                        offset,
                        reason: "declared factory payload is shorter than its five-byte header",
                    });
                    offset += 9;
                    continue;
                };
                let payload_start = offset + 9;
                let Some(payload_end) = payload_start.checked_add(payload_length) else {
                    findings.push(BinaryFinding::InvalidFactoryRequest {
                        offset,
                        reason: "declared factory payload length overflows",
                    });
                    offset += 9;
                    continue;
                };
                if payload_end > bytes.len() {
                    findings.push(BinaryFinding::InvalidFactoryRequest {
                        offset,
                        reason: "factory request payload is truncated",
                    });
                } else if is_read && payload_length < 2 {
                    findings.push(BinaryFinding::InvalidFactoryRequest {
                        offset,
                        reason: "EEPROM read request has fewer than two address bytes",
                    });
                } else if is_write && payload_length < 3 {
                    findings.push(BinaryFinding::InvalidFactoryRequest {
                        offset,
                        reason: "EEPROM write request has fewer than address and value bytes",
                    });
                } else {
                    let read_key = u16::from_le_bytes([bytes[offset + 4], bytes[offset + 5]]);
                    let address =
                        u16::from_le_bytes([bytes[payload_start], bytes[payload_start + 1]]);
                    if is_read {
                        findings.push(BinaryFinding::EepromRead {
                            offset,
                            read_key,
                            address,
                        });
                    } else {
                        findings.push(BinaryFinding::EepromWrite {
                            offset,
                            read_key,
                            address,
                            value: bytes[payload_start + 2],
                            write_key: bytes[payload_start + 3..payload_end].to_vec(),
                        });
                    }
                }
                offset += 9;
                continue;
            }
        }
        offset += 1;
    }

    if include_ascii {
        let mut offset = 0;
        while offset < bytes.len() {
            if !(0x20..=0x7e).contains(&bytes[offset]) {
                offset += 1;
                continue;
            }
            let start = offset;
            while offset < bytes.len() && (0x20..=0x7e).contains(&bytes[offset]) {
                offset += 1;
            }
            if offset - start >= 8 {
                findings.push(BinaryFinding::PrintableAscii {
                    offset: start,
                    value: String::from_utf8(bytes[start..offset].to_vec())
                        .expect("ASCII byte range is valid UTF-8"),
                });
            }
        }
    }
    findings
}

fn render_binary_findings(input_file: &Path, findings: &[BinaryFinding], as_json: bool) -> String {
    if as_json {
        let findings = findings
            .iter()
            .map(|finding| match finding {
                BinaryFinding::EepromRead {
                    offset,
                    read_key,
                    address,
                } => json!({
                    "kind": "eeprom_read",
                    "offset": offset,
                    "read_key": format!("{read_key:04X}"),
                    "address": format!("{address:04X}"),
                }),
                BinaryFinding::EepromWrite {
                    offset,
                    read_key,
                    address,
                    value,
                    write_key,
                } => json!({
                    "kind": "eeprom_write",
                    "offset": offset,
                    "read_key": format!("{read_key:04X}"),
                    "address": format!("{address:04X}"),
                    "value": format!("{value:02X}"),
                    "write_key_hex": hex_encode(write_key),
                }),
                BinaryFinding::InvalidFactoryRequest { offset, reason } => json!({
                    "kind": "invalid_factory_request",
                    "offset": offset,
                    "reason": reason,
                }),
                BinaryFinding::PrintableAscii { offset, value } => json!({
                    "kind": "printable_ascii",
                    "offset": offset,
                    "value": value,
                }),
            })
            .collect::<Vec<_>>();
        return json!({
            "mode": "offline",
            "input_file": input_file,
            "findings": findings,
        })
        .to_string();
    }
    findings
        .iter()
        .map(|finding| match finding {
            BinaryFinding::EepromRead {
                offset,
                read_key,
                address,
            } => format!("offset:{offset:08X} rkey:{read_key:04x} READ addr:{address:04x}"),
            BinaryFinding::EepromWrite {
                offset,
                read_key,
                address,
                value,
                write_key,
            } => format!(
                "offset:{offset:08X} rkey:{read_key:04x} WRITE addr:{address:04x} val:{value:02x} wkey:{}",
                hex_encode(write_key)
            ),
            BinaryFinding::InvalidFactoryRequest { offset, reason } => {
                format!("offset:{offset:08X} INVALID {reason}")
            }
            BinaryFinding::PrintableAscii { offset, value } => {
                format!("offset:{offset:08X} ASCII {value}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn snmp_status_output(as_json: bool) -> Result<String, String> {
    let config = SnmpConfig::from_environment().map_err(|error| error.to_string())?;
    let mut channel = SnmpControlChannel::connect(config).map_err(|error| error.to_string())?;
    let identity = channel
        .printer_identity()
        .map_err(|error| error.to_string())?;
    let database = ModelDatabase::builtin().map_err(|error| error.to_string())?;
    let spec = database.resolve_identity(&identity).ok_or_else(|| {
        "SNMP printer identity does not resolve to a built-in Epson model; refusing Epson status request"
            .to_owned()
    })?;
    let status = EpsonController::new(&mut channel, spec)
        .read_status()
        .map_err(|error| error.to_string())?;
    Ok(render_printer_status(&spec.model, &status, as_json))
}

fn snmp_eeprom_read_output(
    model: &str,
    addresses: &[u16],
    as_json: bool,
) -> Result<String, String> {
    let spec = selected_model(model)?;
    validate_eeprom_read_addresses(&spec, addresses)?;
    let replies = with_validated_snmp_channel(spec, model, |channel, spec| {
        EpsonController::new(channel, spec)
            .read_eeprom(addresses)
            .map_err(|error| error.to_string())
    })?;
    Ok(render_eeprom_readings(model, &replies, as_json))
}

fn snmp_eeprom_dump_output(
    model: &str,
    output_file: &Path,
    as_json: bool,
) -> Result<String, String> {
    validate_new_file_path(output_file, "EEPROM image")?;
    let spec = selected_model(model)?;
    let start_address = spec.memory_low;
    let bytes = with_validated_snmp_channel(spec, model, |channel, spec| {
        let addresses = (spec.memory_low..=spec.memory_high).collect::<Vec<_>>();
        EpsonController::new(channel, spec)
            .read_eeprom(&addresses)
            .map(|replies| {
                replies
                    .into_iter()
                    .map(|reply| reply.value)
                    .collect::<Vec<_>>()
            })
            .map_err(|error| error.to_string())
    })?;
    write_new_binary_file(output_file, &bytes, "EEPROM image")?;
    Ok(render_eeprom_dump(
        model,
        start_address,
        bytes.len(),
        output_file,
        as_json,
    ))
}

fn with_validated_snmp_channel<R>(
    spec: EpsonSpec,
    expected_model: &str,
    operation: impl FnOnce(&mut SnmpControlChannel, &EpsonSpec) -> Result<R, String>,
) -> Result<R, String> {
    let config = SnmpConfig::from_environment().map_err(|error| error.to_string())?;
    let mut channel = SnmpControlChannel::connect(config).map_err(|error| error.to_string())?;
    let identity = channel
        .printer_identity()
        .map_err(|error| error.to_string())?;
    verify_requested_model(&identity, expected_model)?;
    operation(&mut channel, &spec)
}

fn render_printer_status(model: &str, status: &[u8], as_json: bool) -> String {
    let status_text = std::str::from_utf8(status).ok();
    let status_hex = hex_encode(status);
    if as_json {
        json!({
            "mode": "read_only",
            "model": model,
            "status_text": status_text,
            "status_hex": status_hex,
        })
        .to_string()
    } else {
        let text = status_text
            .map(str::escape_default)
            .map(|escaped| escaped.to_string())
            .unwrap_or_else(|| "unavailable (non-UTF-8 response)".to_owned());
        format!("model: {model}\nstatus: {text}\nstatus-hex: {status_hex}")
    }
}

fn render_eeprom_readings(model: &str, replies: &[EepromReadReply], as_json: bool) -> String {
    if as_json {
        json!({
            "mode": "read_only",
            "model": model,
            "eeprom": replies.iter().map(|reply| json!({
                "address": format!("{:04X}", reply.address),
                "value": format!("{:02X}", reply.value),
            })).collect::<Vec<_>>(),
        })
        .to_string()
    } else {
        let readings = replies
            .iter()
            .map(|reply| format!("{:#06x}: {:#04x}", reply.address, reply.value))
            .collect::<Vec<_>>()
            .join("\n");
        format!("model: {model}\n{readings}")
    }
}

fn render_eeprom_dump(
    model: &str,
    start_address: u16,
    byte_count: usize,
    output_file: &Path,
    as_json: bool,
) -> String {
    let end_address = start_address.saturating_add(byte_count.saturating_sub(1) as u16);
    if as_json {
        json!({
            "mode": "read_only",
            "model": model,
            "start_address": format!("{start_address:04X}"),
            "end_address": format!("{end_address:04X}"),
            "byte_count": byte_count,
            "output_file": output_file,
        })
        .to_string()
    } else {
        format!(
            "Saved {byte_count}-byte EEPROM image for {model} ({start_address:#06x}..={end_address:#06x}) to {}",
            output_file.display()
        )
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn validate_eeprom_read_addresses(spec: &EpsonSpec, addresses: &[u16]) -> Result<(), String> {
    if addresses.is_empty() {
        return Err("at least one --address is required".to_owned());
    }
    let mut seen = std::collections::BTreeSet::new();
    for &address in addresses {
        if address < spec.memory_low || address > spec.memory_high {
            return Err(format!(
                "EEPROM read address {address:#06x} is outside model range {:#06x}..={:#06x}",
                spec.memory_low, spec.memory_high
            ));
        }
        if !seen.insert(address) {
            return Err(format!("EEPROM read address {address:#06x} is duplicated"));
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
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

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
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

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn read_restore_image(path: &Path, spec: &EpsonSpec) -> Result<Vec<(u16, u8)>, String> {
    let bytes = std::fs::read(path).map_err(|error| {
        format!(
            "could not read EEPROM restore image {}: {error}",
            path.display()
        )
    })?;
    reink_app::restore_eeprom_updates(spec, &bytes)
}

#[cfg(target_os = "windows")]
fn windows_native_candidate(
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
fn finish_windows_native_operation<T>(outcome: SelectedUsbSessionOutcome<T>) -> Result<T, String> {
    let mut errors = Vec::new();
    let value = match outcome.operation {
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
        Ok(value.expect("successful operation retains its value"))
    } else {
        Err(errors.join("; "))
    }
}

#[cfg(target_os = "windows")]
fn with_windows_native_read_session<T>(
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
    let candidate = windows_native_candidate(vendor_id, product_id, interface)?;
    let spec = selected_model(model)?;
    finish_windows_native_operation(with_selected_windows_native_epson_session(
        &candidate, spec, false, operation,
    ))
}

#[cfg(target_os = "windows")]
fn with_windows_native_experimental_mutation_session<T>(
    vendor_id: u16,
    product_id: u16,
    interface: Option<u8>,
    model: &str,
    operation: impl FnOnce(
        &mut EpsonD4Session<RecordingTransport<reink_usb::WindowsNativeReadOnlyTransport>>,
    ) -> Result<T, String>,
) -> Result<T, String> {
    let candidate = windows_native_candidate(vendor_id, product_id, interface)?;
    let spec = selected_model(model)?;
    finish_windows_native_operation(with_selected_windows_native_experimental_mutation_session(
        &candidate, spec, operation,
    ))
}

#[cfg(target_os = "windows")]
#[allow(clippy::too_many_arguments)]
fn windows_native_experimental_mutation_output(
    vendor_id: u16,
    product_id: u16,
    interface: Option<u8>,
    model: &str,
    updates: Vec<(u16, u8)>,
    backup_file: &Path,
    confirmation: Option<&str>,
    native_acknowledgement: Option<&str>,
    expected_confirmation: &str,
    operation: &str,
    backup_kind: &str,
    as_json: bool,
) -> Result<String, String> {
    validate_confirmation(
        confirmation,
        expected_confirmation,
        &format!("windows-native-experimental-eeprom-{operation}"),
    )?;
    if native_acknowledgement != Some(WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT) {
        return Err(format!(
            "experimental Windows native USBPRINT mutation requires --native-experimental-acknowledgement {WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT} exactly"
        ));
    }
    validate_new_file_path(backup_file, backup_kind)?;
    let spec = selected_model(model)?;
    validate_eeprom_updates(&spec, &updates)?;
    let update_count = updates.len();
    with_windows_native_experimental_mutation_session(
        vendor_id,
        product_id,
        interface,
        model,
        |session| {
            reink_app::verify_exact_model(
                &session.read_identity().map_err(|error| error.to_string())?,
                model,
            )?;
            let plan = session.prepare_eeprom_write(&updates).map_err(|error| {
                format!("could not prepare experimental Windows native EEPROM {operation}: {error}")
            })?;
            write_new_binary_file(backup_file, &plan.backup.bytes, backup_kind)?;
            session.apply_eeprom_write(&plan).map_err(|error| {
                format!("experimental Windows native USBPRINT EEPROM {operation} failed: {error}")
            })
        },
    )?;
    if as_json {
        Ok(json!({
            "backend": "windows_native_usbprint",
            "experimental_unvalidated": true,
            "operation": operation,
            "model": model,
            "byte_count": update_count,
            "backup_file": backup_file,
        })
        .to_string())
    } else {
        Ok(format!(
            "Experimental/unvalidated Windows native USBPRINT EEPROM {operation} completed for {model}: {update_count} byte(s); backup_file: {}",
            backup_file.display()
        ))
    }
}

#[cfg(target_os = "windows")]
fn windows_native_candidates_output(as_json: bool) -> Result<String, String> {
    let database = ModelDatabase::builtin().map_err(|error| error.to_string())?;
    let candidates =
        reink_usb::list_windows_native_printer_candidates().map_err(|error| error.to_string())?;
    let rendered = candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let capabilities = candidate.capabilities();
            let model_hints = database
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
                "vendor_id": format!("{:04x}", candidate.vendor_id),
                "product_id": format!("{:04x}", candidate.product_id),
                "interface": candidate.interface_number,
                "model_hints": model_hints,
                "capabilities": {
                    "d4_read": capabilities.d4_read,
                    "usb_device_id": capabilities.usb_device_id,
                    "persistent_mutation": capabilities.persistent_mutation,
                    "experimental_mutation": capabilities.experimental_mutation,
                },
            })
        })
        .collect::<Vec<_>>();
    Ok(if as_json {
        json!({"mode": "read_only", "candidates": rendered}).to_string()
    } else if rendered.is_empty() {
        "No present Windows stock-driver USBPRINT candidates.".to_owned()
    } else {
        rendered
            .iter()
            .map(|candidate| {
                format!(
                    "{} — {}:{}, interface {}, D4 reads plus explicitly gated experimental mutation (no USB device-ID)",
                    candidate["alias"].as_str().unwrap_or("windows-native"),
                    candidate["vendor_id"].as_str().unwrap_or("unknown"),
                    candidate["product_id"].as_str().unwrap_or("unknown"),
                    candidate["interface"]
                        .as_u64()
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "not reported".to_owned()),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

#[cfg(target_os = "windows")]
fn windows_native_identity_output(
    vendor_id: u16,
    product_id: u16,
    interface: Option<u8>,
    model: &str,
    as_json: bool,
) -> Result<String, String> {
    let identity =
        with_windows_native_read_session(vendor_id, product_id, interface, model, |session| {
            let identity = session.read_identity().map_err(|error| error.to_string())?;
            verify_requested_model(&identity, model)?;
            Ok(identity)
        })?;
    Ok(if as_json {
        json!({
            "backend": "windows_native_usbprint",
            "manufacturer": identity.manufacturer(),
            "model": identity.model(),
            "detected_model": identity.detected_model(),
            "command_set": identity.command_set(),
        })
        .to_string()
    } else {
        format!(
            "Backend: Windows native USBPRINT (read-only)\nManufacturer: {}\nModel: {}\nCommand set: {}",
            identity.manufacturer().unwrap_or("unknown"),
            identity.model().unwrap_or("unknown"),
            identity.command_set().join(", "),
        )
    })
}

#[cfg(target_os = "windows")]
fn windows_native_status_output(
    vendor_id: u16,
    product_id: u16,
    interface: Option<u8>,
    model: &str,
    as_json: bool,
) -> Result<String, String> {
    let mut status =
        with_windows_native_read_session(vendor_id, product_id, interface, model, |session| {
            let identity = session.read_identity().map_err(|error| error.to_string())?;
            verify_requested_model(&identity, model)?;
            session.read_status().map_err(|error| error.to_string())
        })?;
    reink_usb::redact_identity_serial_fields(&mut status);
    Ok(render_printer_status(model, &status, as_json))
}

#[cfg(target_os = "windows")]
fn windows_native_eeprom_read_output(
    vendor_id: u16,
    product_id: u16,
    interface: Option<u8>,
    model: &str,
    addresses: &[u16],
    as_json: bool,
) -> Result<String, String> {
    let spec = selected_model(model)?;
    validate_eeprom_read_addresses(&spec, addresses)?;
    validate_native_sensitive_addresses(&spec, addresses)?;
    let readings =
        with_windows_native_read_session(vendor_id, product_id, interface, model, |session| {
            let identity = session.read_identity().map_err(|error| error.to_string())?;
            verify_requested_model(&identity, model)?;
            session
                .read_eeprom(addresses)
                .map_err(|error| error.to_string())
        })?;
    Ok(render_eeprom_readings(model, &readings, as_json))
}

#[cfg(target_os = "windows")]
fn validate_native_sensitive_addresses(spec: &EpsonSpec, addresses: &[u16]) -> Result<(), String> {
    if let Some(address) = addresses.iter().copied().find(|address| {
        spec.read_only_fields
            .iter()
            .any(|field| field.sensitive && (field.address..=field.end_address).contains(address))
    }) {
        return Err(format!(
            "EEPROM address {address:#06x} is part of a sensitive identity field and cannot be displayed by the Windows native command; use a private binary dump if authorized"
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_native_eeprom_dump_output(
    vendor_id: u16,
    product_id: u16,
    interface: Option<u8>,
    model: &str,
    output_file: &Path,
    as_json: bool,
) -> Result<String, String> {
    validate_new_file_path(output_file, "EEPROM image")?;
    let image =
        with_windows_native_read_session(vendor_id, product_id, interface, model, |session| {
            let identity = session.read_identity().map_err(|error| error.to_string())?;
            verify_requested_model(&identity, model)?;
            session.dump_eeprom().map_err(|error| error.to_string())
        })?;
    write_new_binary_file(output_file, &image.bytes, "EEPROM image")?;
    Ok(if as_json {
        json!({
            "mode": "read_only",
            "backend": "windows_native_usbprint",
            "model": image.model,
            "start_address": format!("{:04X}", image.start_address),
            "end_address": format!("{:04X}", image.end_address()),
            "byte_count": image.bytes.len(),
            "output_file": output_file,
        })
        .to_string()
    } else {
        format!(
            "Saved {}-byte read-only Windows stock-driver EEPROM image for {} to {}",
            image.bytes.len(),
            image.model,
            output_file.display()
        )
    })
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

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[allow(clippy::too_many_arguments)]
fn usb_status_output(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: Option<u8>,
    device_address: Option<u8>,
    model: &str,
    as_json: bool,
) -> Result<String, String> {
    let spec = selected_model(model)?;
    let status = with_usb_epson_session(
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
            session.read_status().map_err(|error| error.to_string())
        },
    )?;
    Ok(render_printer_status(model, &status, as_json))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[allow(clippy::too_many_arguments)]
fn usb_status_output(
    _vendor_id: u16,
    _product_id: u16,
    _interface: u8,
    _alternate_setting: u8,
    _bus_number: Option<u8>,
    _device_address: Option<u8>,
    _model: &str,
    _as_json: bool,
) -> Result<String, String> {
    Err("USB printer status is currently supported only on Linux, macOS, or Windows".to_owned())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
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

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
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
    let image = with_usb_epson_session(
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

    with_usb_epson_session(
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
fn usb_eeprom_reset_output(
    vendor_id: u16,
    product_id: u16,
    interface: u8,
    alternate_setting: u8,
    bus_number: Option<u8>,
    device_address: Option<u8>,
    model: &str,
    target: CounterResetSelection,
    backup_file: &Path,
    confirmation: Option<&str>,
    as_json: bool,
) -> Result<String, String> {
    validate_confirmation(
        confirmation,
        EEPROM_COUNTER_RESET_CONFIRMATION,
        "usb-eeprom-reset",
    )?;
    validate_new_file_path(backup_file, "EEPROM reset backup")?;
    let spec = selected_model(model)?;
    let updates = declared_counter_reset_updates(&spec, target)?;

    with_usb_epson_session(
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
                .map_err(|error| format!("could not prepare declared counter reset: {error}"))?;
            write_new_binary_file(backup_file, &plan.backup.bytes, "EEPROM reset backup")?;
            session
                .apply_eeprom_write(&plan)
                .map_err(|error| format!("declared counter reset failed: {error}"))
        },
    )?;

    Ok(counter_reset_output(
        model,
        target,
        updates.len(),
        backup_file,
        as_json,
    ))
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
fn declared_counter_reset_updates(
    spec: &EpsonSpec,
    target: CounterResetSelection,
) -> Result<Vec<(u16, u8)>, String> {
    let operation = spec.counter_reset(target.target()).ok_or_else(|| {
        format!(
            "model {} has no explicitly declared {} reset bytes",
            spec.model,
            target.target().display_name()
        )
    })?;
    if !operation.has_declared_reset_values() {
        return Err(format!(
            "model {} has no explicitly declared {} reset bytes",
            spec.model,
            target.target().display_name()
        ));
    }
    let updates = operation
        .addresses
        .into_iter()
        .zip(operation.reset_values)
        .collect::<Vec<_>>();
    validate_eeprom_updates(spec, &updates)?;
    Ok(updates)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn counter_reset_output(
    model: &str,
    target: CounterResetSelection,
    byte_count: usize,
    backup_file: &Path,
    as_json: bool,
) -> String {
    let target = target.target().display_name();
    if as_json {
        json!({
            "operation": "declared_counter_reset",
            "model": model,
            "target": target,
            "byte_count": byte_count,
            "backup_file": backup_file,
        })
        .to_string()
    } else {
        format!(
            "Declared {target} reset completed for {model}: {byte_count} byte(s); backup_file: {}",
            backup_file.display()
        )
    }
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

    with_usb_epson_session(
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

fn selected_model(model: &str) -> Result<EpsonSpec, String> {
    ModelDatabase::builtin()
        .map_err(|error| error.to_string())?
        .get(model)
        .cloned()
        .ok_or_else(|| format!("unknown model: {model}"))
}

fn verify_requested_model(identity: &PrinterIdentity, model: &str) -> Result<(), String> {
    match identity.detected_model() {
        Some(detected) if detected == model => Ok(()),
        Some(detected) => Err(format!(
            "printer identity model {detected:?} does not match requested model {model:?}"
        )),
        None => Err(format!(
            "printer identity does not contain a model; requested model is {model:?}"
        )),
    }
}

fn write_new_binary_file(path: &Path, bytes: &[u8], kind: &str) -> Result<(), String> {
    reink_app::write_new_binary_file(path, bytes, kind)
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

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[allow(clippy::too_many_arguments)]
fn with_usb_epson_session<R>(
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

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
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
    Err("USB EEPROM dumps are currently supported only on Linux, macOS, or Windows".to_owned())
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
#[allow(clippy::too_many_arguments)]
fn usb_eeprom_reset_output(
    _: u16,
    _: u16,
    _: u8,
    _: u8,
    _: Option<u8>,
    _: Option<u8>,
    _: &str,
    _: CounterResetSelection,
    _: &Path,
    _: Option<&str>,
    _: bool,
) -> Result<String, String> {
    Err("USB EEPROM resets are currently supported only on Linux or macOS".to_owned())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn usb_identity_output(
    _vendor_id: u16,
    _product_id: u16,
    _interface: u8,
    _alternate_setting: u8,
    _bus_number: Option<u8>,
    _device_address: Option<u8>,
    _as_json: bool,
) -> Result<String, String> {
    Err(
        "USB identity inspection is currently supported only on Linux, macOS, or Windows"
            .to_owned(),
    )
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

    use reink_core::{EepromReadReply, ModelDatabase};

    #[cfg(target_os = "windows")]
    use super::WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT;
    use super::{
        BinaryFinding, Cli, Command, CounterResetSelection, EEPROM_COUNTER_RESET_CONFIRMATION,
        EEPROM_RESTORE_CONFIRMATION, EEPROM_WRITE_CONFIRMATION, analyze_binary_bytes,
        declared_counter_reset_updates, parse_eeprom_update, render_eeprom_readings,
        render_identity, render_models, render_printer_status, validate_confirmation,
        validate_eeprom_read_addresses, validate_eeprom_updates, validate_new_file_path,
        verify_requested_model,
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
        let cli = Cli::try_parse_from(["reink", "analyze-binary", "capture.bin"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::AnalyzeBinary {
                input_file,
                include_ascii: false,
            } if input_file == std::path::Path::new("capture.bin")
        ));
        let cli = Cli::try_parse_from(["reink", "snmp-status"]).unwrap();
        assert!(matches!(cli.command, Command::SnmpStatus));
        let cli = Cli::try_parse_from([
            "reink",
            "snmp-eeprom-read",
            "--model",
            "C90",
            "--address",
            "0x000c",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::SnmpEepromRead { model, address }
                if model == "C90" && address == vec![0x000c]
        ));
        let cli = Cli::try_parse_from([
            "reink",
            "snmp-eeprom-dump",
            "--model",
            "C90",
            "--output-file",
            "new-image.bin",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::SnmpEepromDump { model, output_file }
                if model == "C90" && output_file == std::path::PathBuf::from("new-image.bin")
        ));
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
            "usb-status",
            "--vendor-id",
            "0x04b8",
            "--product-id",
            "1234",
            "--interface",
            "0",
            "--model",
            "C90",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::UsbStatus {
                vendor_id: 0x04b8,
                product_id: 1234,
                interface: 0,
                alternate_setting: 0,
                bus_number: None,
                device_address: None,
                model,
            } if model == "C90"
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
        let cli = Cli::try_parse_from([
            "reink",
            "usb-eeprom-reset",
            "--vendor-id",
            "0x04b8",
            "--product-id",
            "1234",
            "--interface",
            "0",
            "--model",
            "C90",
            "--target",
            "waste",
            "--backup-file",
            "new-reset-backup.bin",
            "--confirmation",
            EEPROM_COUNTER_RESET_CONFIRMATION,
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::UsbEepromReset {
                target: CounterResetSelection::Waste,
                confirmation: Some(_),
                ..
            }
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_read_only_windows_native_commands_without_mutation_selectors() {
        let cli = Cli::try_parse_from([
            "reink",
            "windows-native-eeprom-read",
            "--vendor-id",
            "0x04b8",
            "--product-id",
            "0x1234",
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
            Command::WindowsNativeEepromRead {
                vendor_id: 0x04b8,
                product_id: 0x1234,
                interface: Some(0),
                model,
                address,
            } if model == "C90" && address == [0x000c]
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parses_experimental_native_write_with_distinct_acknowledgement() {
        let cli = Cli::try_parse_from([
            "reink",
            "windows-native-experimental-eeprom-write",
            "--vendor-id",
            "0x04b8",
            "--product-id",
            "0x1234",
            "--model",
            "C90",
            "--update",
            "0x000c=0xff",
            "--backup-file",
            "new.bin",
            "--confirmation",
            EEPROM_WRITE_CONFIRMATION,
            "--native-experimental-acknowledgement",
            WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT,
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::WindowsNativeExperimentalEepromWrite {
                native_experimental_acknowledgement: Some(_),
                ..
            }
        ));
        assert!(
            validate_confirmation(
                Some(EEPROM_WRITE_CONFIRMATION),
                EEPROM_WRITE_CONFIRMATION,
                "test"
            )
            .is_ok()
        );
        assert_ne!(
            EEPROM_WRITE_CONFIRMATION,
            WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT
        );
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
    fn offline_binary_analysis_finds_factory_requests_and_never_reads_past_input() {
        let bytes = [
            b'x', b'|', b'|', 0x07, 0x00, 0x34, 0x12, b'A', 0xbe, 0xa0, 0x78, 0x56, b'|', b'|',
            0x10, 0x00, 0x34, 0x12, b'B', 0xbd, 0x21, 0x34, 0x12, 0xfe, b'k', b'e', b'y', 0, 0, 0,
            0, 0, b'p', b'r', b'i', b'n', b't', b'a', b'b', b'l', b'e', b'!', 0, b'|', b'|', 0x04,
            0x00, 0x34, 0x12, b'A', 0xbe, 0xa0,
        ];
        let findings = analyze_binary_bytes(&bytes, true);

        assert_eq!(
            findings,
            vec![
                BinaryFinding::EepromRead {
                    offset: 1,
                    read_key: 0x1234,
                    address: 0x5678,
                },
                BinaryFinding::EepromWrite {
                    offset: 12,
                    read_key: 0x1234,
                    address: 0x1234,
                    value: 0xfe,
                    write_key: b"key\0\0\0\0\0".to_vec(),
                },
                BinaryFinding::InvalidFactoryRequest {
                    offset: 43,
                    reason: "declared factory payload is shorter than its five-byte header",
                },
                BinaryFinding::PrintableAscii {
                    offset: 32,
                    value: "printable!".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn offline_binary_analysis_can_exclude_printable_ascii() {
        assert!(analyze_binary_bytes(b"ABCDEFGH", false).is_empty());
    }

    #[test]
    fn renders_status_without_emitting_terminal_control_characters() {
        assert_eq!(
            render_printer_status("C90", b"@BDC ST2\r\nREADY\r\n", false),
            "model: C90\nstatus: @BDC ST2\\r\\nREADY\\r\\n\nstatus-hex: 40424443205354320D0A52454144590D0A"
        );
    }

    #[test]
    fn renders_read_only_eeprom_values_and_rejects_unsafe_addresses() {
        let database = ModelDatabase::builtin().unwrap();
        let spec = database.get("C90").unwrap();
        assert!(validate_eeprom_read_addresses(spec, &[spec.memory_low, spec.memory_high]).is_ok());
        assert!(validate_eeprom_read_addresses(spec, &[]).is_err());
        assert!(validate_eeprom_read_addresses(spec, &[spec.memory_low, spec.memory_low]).is_err());
        assert!(
            validate_eeprom_read_addresses(spec, &[spec.memory_high.saturating_add(1)]).is_err()
        );
        assert_eq!(
            render_eeprom_readings(
                "C90",
                &[EepromReadReply {
                    address: 0x0c,
                    value: 0x42,
                }],
                false,
            ),
            "model: C90\n0x000c: 0x42"
        );
    }

    #[test]
    fn rejects_a_model_that_does_not_match_the_printer_identity() {
        let matching = reink_core::PrinterIdentity::parse("MFG:EPSON;MDL:C90;").unwrap();
        let mismatched = reink_core::PrinterIdentity::parse("MFG:EPSON;MDL:XP-352;").unwrap();

        assert!(verify_requested_model(&matching, "C90").is_ok());
        assert!(verify_requested_model(&mismatched, "C90").is_err());
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
        assert!(
            validate_confirmation(
                Some(EEPROM_COUNTER_RESET_CONFIRMATION),
                EEPROM_COUNTER_RESET_CONFIRMATION,
                "usb-eeprom-reset"
            )
            .is_ok()
        );
        assert!(
            validate_confirmation(
                Some("I_CONFIRM_THIS_WILL_RESET_COUNTERS"),
                EEPROM_COUNTER_RESET_CONFIRMATION,
                "usb-eeprom-reset"
            )
            .is_err()
        );
    }

    #[test]
    fn declared_counter_resets_select_only_the_requested_explicit_operations() {
        let database = ModelDatabase::builtin().unwrap();
        let c90 = database.get("C90").unwrap();
        assert_eq!(
            declared_counter_reset_updates(c90, CounterResetSelection::Waste).unwrap(),
            vec![
                (0x06, 0),
                (0x07, 0),
                (0x0a, 0),
                (0x0b, 0),
                (0x16, 0),
                (0x17, 0),
                (0x34, 4),
                (0x35, 0x57),
                (0x0c, 1),
                (0x0d, 0xf4),
            ]
        );
        assert!(declared_counter_reset_updates(c90, CounterResetSelection::PlatenPad).is_err());

        let xp = database.get("XP-15000").unwrap();
        assert_eq!(
            declared_counter_reset_updates(xp, CounterResetSelection::PlatenPad).unwrap(),
            vec![(0x40, 0), (0x43, 0), (0x44, 0), (0x48, 0x5e), (0x1ed, 0)]
        );
        assert!(declared_counter_reset_updates(xp, CounterResetSelection::Waste).is_err());
    }

    #[test]
    fn refuses_existing_and_parentless_backup_paths() {
        let existing = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(validate_new_file_path(&existing, "EEPROM backup").is_err());
        assert!(validate_new_file_path(&existing, "EEPROM reset backup").is_err());
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
