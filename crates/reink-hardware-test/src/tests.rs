use std::{
    collections::VecDeque,
    fs,
    path::{Path, PathBuf},
};

use clap::Parser;
use reink_app::EepromImage;
use reink_platform::TransportEvent;
use serde_json::{Value, json};

#[cfg(target_os = "windows")]
use super::WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT;
#[cfg(target_os = "windows")]
use super::native_write_evidence_cleanup;
use super::{
    Cli, Command, DriverHandoffReport, OUT_OF_RANGE_READ_CONFIRMATION, ReadOnlyFailureKind,
    TRACE_SANITIZATION_CONFIRMATION, WRITE_EVIDENCE_RESTORATION_CONFIRMATION,
    WRITE_EVIDENCE_WRITE_CONFIRMATION, WriteEvidenceCleanup, WriteEvidenceSession,
    WriteEvidenceStage, d4_eeprom_boundary_probe_report, d4_eeprom_dump_report,
    d4_eeprom_read_report, d4_failure_report, d4_identity_report, dump_progress,
    eeprom_dump_addresses, emit_report, execute_write_evidence, parse_trace_events, parse_u16,
    read_sequence_report, simulated_read_only_report, trace_json, trace_to_transcript,
    transcript_template, usb_candidates_report, usb_device_selector, usb_driver_state_report,
    validate_boundary_probe, validate_eeprom_read_addresses, validate_report_file_path,
    validate_trace_file_path, validate_write_evidence_gates, write_evidence_report,
};

fn report(output: String) -> Value {
    serde_json::from_str(&output).unwrap()
}

#[test]
fn driver_state_report_is_read_only_and_selector_exact() {
    let output = report(usb_driver_state_report(
        0x04b8,
        0x0066,
        0,
        0,
        1,
        2,
        reink_usb::UsbDriverState::Active,
    ));
    assert_eq!(output["command"], "usb-driver-state");
    assert_eq!(output["driver_state"], "active");
    assert_eq!(output["selector"]["vendor_id"], "04B8");
    assert_eq!(output["traffic_sent"], false);
    assert_eq!(output["device_wide_handoff_on_macos"], true);
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

#[cfg(target_os = "windows")]
#[test]
fn parses_native_write_evidence_with_the_third_acknowledgement() {
    let cli = Cli::try_parse_from([
        "reink-hardware-test",
        "windows-native-d4-eeprom-write-evidence",
        "--vendor-id",
        "0x04b8",
        "--product-id",
        "0x1234",
        "--model",
        "C90",
        "--address",
        "0x000c",
        "--value",
        "0x42",
        "--backup-file",
        "new.bin",
        "--report-file",
        "report.json",
        "--confirm-write",
        WRITE_EVIDENCE_WRITE_CONFIRMATION,
        "--confirm-restoration-evidence",
        WRITE_EVIDENCE_RESTORATION_CONFIRMATION,
        "--confirm-native-experimental-mutation",
        WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT,
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Command::WindowsNativeD4EepromWriteEvidence {
            confirm_native_experimental_mutation: Some(_),
            ..
        }
    ));
}

#[cfg(target_os = "windows")]
#[test]
fn native_evidence_cleanup_reports_success_as_completed() {
    let cleanup = native_write_evidence_cleanup(&reink_app::UsbSessionCleanup {
        d4_shutdown: reink_app::UsbCleanupStatus::Succeeded,
        usb_close: reink_app::UsbCleanupStatus::Succeeded,
    });
    assert_eq!(cleanup.d4_shutdown.status, "completed");
    assert_eq!(
        cleanup.d4_shutdown.detail,
        "D4 service closed and Exit completed"
    );
    assert_eq!(cleanup.usb_close.status, "completed");
    assert_eq!(
        cleanup.usb_close.detail,
        "Windows native USBPRINT interface closed"
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
    assert!(validate_boundary_probe(spec, outside, Some(OUT_OF_RANGE_READ_CONFIRMATION)).is_ok());
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

    assert!(eeprom_dump_addresses(spec, Some(spec.memory_high), Some(spec.memory_low)).is_err());
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
