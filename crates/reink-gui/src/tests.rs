use reink_core::{EepromFieldConfidence, EepromFieldEncoding};
use reink_gui::SourceMode;

#[cfg(target_os = "windows")]
use super::redact_native_identity_serials;
#[cfg(target_os = "windows")]
use super::{
    EEPROM_WRITE_CONFIRMATION, WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT,
    require_native_experimental_mutation_acknowledgement,
};
use super::{
    EepromFileField, eeprom_field_address_label, eeprom_field_tooltip, eeprom_field_value,
    eeprom_image_offset, source_mode_from_args,
};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use super::{
    confirmation_matches, display_status_response, format_current_values, parse_u8_input,
    parse_u16_input, selected_model_for_usb_candidate, validate_restore_image_size,
};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use reink_gui::{DescriptorCandidate, DescriptorCandidateBackend, GuiState};
#[cfg(target_os = "windows")]
use reink_platform::TransportEvent;

#[test]
fn fixtures_require_an_explicit_flag_and_unknown_arguments_are_ignored() {
    assert_eq!(
        source_mode_from_args(["reink-gui".to_owned(), "--unknown".to_owned()]),
        SourceMode::Real
    );
    assert_eq!(
        source_mode_from_args(["reink-gui".to_owned(), "--fixtures".to_owned()]),
        SourceMode::Fixtures
    );
}

#[test]
fn model_bounded_image_fields_use_relative_offsets() {
    assert_eq!(eeprom_image_offset(0x0200, 0x20, 0x0214), Some(0x14));
    assert_eq!(eeprom_image_offset(0x0200, 0x20, 0x01ff), None);
    assert_eq!(eeprom_image_offset(0x0200, 0x20, 0x0220), None);
}

#[test]
fn loaded_field_values_decode_multi_byte_data_and_hide_sensitive_values() {
    let counter = EepromFileField {
        address: 0x26,
        end_address: 0x27,
        label: "Waste counter".to_owned(),
        encoding: EepromFieldEncoding::U16Le,
        confidence: Some(EepromFieldConfidence::Confirmed),
        evidence_note: Some("Reviewed sanitized evidence.".to_owned()),
        sensitive: false,
    };
    let pages = EepromFileField {
        address: 0xb0,
        end_address: 0xb3,
        label: "Total print-page count".to_owned(),
        encoding: EepromFieldEncoding::U32Le,
        confidence: Some(EepromFieldConfidence::Confirmed),
        evidence_note: Some("Reviewed sanitized evidence.".to_owned()),
        sensitive: false,
    };
    let serial = EepromFileField {
        address: 0xc2,
        end_address: 0xcb,
        label: "Serial number".to_owned(),
        encoding: EepromFieldEncoding::Ascii,
        confidence: Some(EepromFieldConfidence::Confirmed),
        evidence_note: Some("Sensitive device-specific field.".to_owned()),
        sensitive: true,
    };
    let mut image = vec![0; 0xcc];
    image[0x26..0x28].copy_from_slice(&[0x34, 0x12]);
    image[0xb0..0xb4].copy_from_slice(&[0x78, 0x56, 0x34, 0x12]);
    image[0xc2..0xcc].copy_from_slice(b"PRIVATE-01");

    assert_eq!(eeprom_field_address_label(&counter), "0x0026..0x0027");
    assert_eq!(eeprom_field_value(&counter, 0, &image), "4660 (0x1234)");
    assert_eq!(
        eeprom_field_value(&pages, 0, &image),
        "305419896 (0x12345678)"
    );
    assert_eq!(eeprom_field_value(&serial, 0, &image), "Hidden (sensitive)");
    let tooltip = eeprom_field_tooltip(&serial);
    assert!(tooltip.contains("Confidence: Confirmed"));
    assert!(tooltip.contains("Sensitive device-specific field."));
    assert!(!tooltip.contains("PRIVATE-01"));
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn guarded_operation_inputs_require_exact_confirmation_and_bounded_bytes() {
    assert!(confirmation_matches(
        "I_CONFIRM_THIS_WILL_WRITE_EEPROM",
        "I_CONFIRM_THIS_WILL_WRITE_EEPROM"
    ));
    assert!(!confirmation_matches(
        "i_confirm_this_will_write_eeprom",
        "I_CONFIRM_THIS_WILL_WRITE_EEPROM"
    ));
    assert_eq!(parse_u16_input("0x00FF"), Ok(0x00ff));
    assert_eq!(parse_u16_input("255"), Ok(255));
    assert!(parse_u16_input("0x10000").is_err());
    assert_eq!(parse_u8_input("0x7F"), Ok(0x7f));
    assert!(parse_u8_input("256").is_err());
}

#[cfg(target_os = "windows")]
#[test]
fn native_mutation_requires_a_distinct_exact_experimental_acknowledgement() {
    assert!(confirmation_matches(
        WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT,
        WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT
    ));
    assert!(!confirmation_matches(
        "I_ACKNOWLEDGE_WINDOWS_NATIVE_MUTATION_IS_EXPERIMENTAL ",
        WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT
    ));
    assert_ne!(
        EEPROM_WRITE_CONFIRMATION,
        WINDOWS_NATIVE_EXPERIMENTAL_MUTATION_ACKNOWLEDGEMENT
    );
    assert!(require_native_experimental_mutation_acknowledgement(true).is_ok());
    assert!(require_native_experimental_mutation_acknowledgement(false).is_err());
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn unassociated_usb_candidate_accepts_only_a_known_expected_model() {
    let state = GuiState::new().unwrap();
    let candidate = DescriptorCandidate {
        alias: "usb-1".to_owned(),
        backend: DescriptorCandidateBackend::LibUsb,
        vendor_id: 0x04b8,
        product_id: 0x0001,
        bus_number: Some(1),
        device_address: Some(1),
        interface_number: Some(0),
        alternate_setting: Some(0),
        model_hints: Vec::new(),
    };

    assert_eq!(
        selected_model_for_usb_candidate(&state, Some(&candidate), Some("C90")),
        Some("C90")
    );
    assert_eq!(
        selected_model_for_usb_candidate(&state, Some(&candidate), Some("not-a-model")),
        None
    );
}

#[cfg(target_os = "windows")]
#[test]
fn native_debug_capture_redacts_serials_across_read_boundaries() {
    let events = vec![
        TransportEvent::Tx(vec![1, 2]),
        TransportEvent::Rx(b"MFG:EPSON;SN:PRI".to_vec()),
        TransportEvent::Rx(b"VATE;MDL:C90;".to_vec()),
    ];
    let redacted = redact_native_identity_serials(events);
    let received = redacted
        .iter()
        .filter_map(|event| match event {
            TransportEvent::Rx(bytes) => Some(bytes.as_slice()),
            TransportEvent::Tx(_) => None,
        })
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(
        String::from_utf8(received).unwrap(),
        "MFG:EPSON;SN:XXXXXXX;MDL:C90;"
    );
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn restore_preflight_rejects_multi_gib_sizes_before_reading() {
    let state = GuiState::new().unwrap();
    let spec = state.model_spec("C90").unwrap();

    assert!(validate_restore_image_size(spec, u64::MAX).is_err());
    assert!(validate_restore_image_size(spec, 0).is_err());
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[test]
fn operation_results_bound_private_value_and_status_displays() {
    assert_eq!(
        format_current_values(&[(0x0006, 0x18), (0x0007, 0x04)]),
        "0x0006=0x18, 0x0007=0x04"
    );
    assert_eq!(
        display_status_response(b"@BDC ST2\r\nREADY\r\n"),
        "@BDC ST2\r\nREADY"
    );
    assert_eq!(
        display_status_response(&[0xff]),
        "Binary status response is not shown."
    );
}
