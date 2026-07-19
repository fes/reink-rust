use clap::Parser;

use reink_core::{EepromReadReply, ModelDatabase};

#[cfg(target_os = "windows")]
use super::WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT;
use super::{
    BinaryFinding, Cli, Command, CounterResetSelection, EEPROM_COUNTER_RESET_CONFIRMATION,
    EEPROM_RESTORE_CONFIRMATION, EEPROM_WRITE_CONFIRMATION, analyze_binary_bytes,
    declared_counter_reset_updates, parse_eeprom_update, render_eeprom_readings, render_identity,
    render_models, render_printer_status, validate_confirmation, validate_eeprom_read_addresses,
    validate_eeprom_updates, validate_new_file_path, verify_requested_model,
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
            if model == "C90" && output_file == std::path::Path::new("new-image.bin")
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
        b'x', b'|', b'|', 0x07, 0x00, 0x34, 0x12, b'A', 0xbe, 0xa0, 0x78, 0x56, b'|', b'|', 0x10,
        0x00, 0x34, 0x12, b'B', 0xbd, 0x21, 0x34, 0x12, 0xfe, b'k', b'e', b'y', 0, 0, 0, 0, 0,
        b'p', b'r', b'i', b'n', b't', b'a', b'b', b'l', b'e', b'!', 0, b'|', b'|', 0x04, 0x00,
        0x34, 0x12, b'A', 0xbe, 0xa0,
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
    assert!(validate_eeprom_read_addresses(spec, &[spec.memory_high.saturating_add(1)]).is_err());
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
    assert!(validate_eeprom_updates(spec, &[(spec.memory_low, 1), (spec.memory_low, 2)]).is_err());
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
