#![forbid(unsafe_code)]

use eframe::egui::{self, Color32, RichText};
use regex::RegexBuilder;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use reink_app::{
    EepromImage, UsbCleanupStatus, UsbSessionCleanup, declared_counter_reset_updates,
    restore_eeprom_updates, verify_exact_model, with_selected_usb_epson_session,
    write_new_binary_file,
};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use reink_core::{CounterResetTarget, EpsonSpec, ModelDatabase};
use reink_core::{EepromFieldConfidence, EepromFieldEncoding};
use reink_gui::{
    DebugTrafficTrace, DescriptorCandidate, GuiState, Page, SourceMode, ValidationStatus,
};
use reink_platform::TransportEvent;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::path::PathBuf;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::sync::mpsc::{Receiver, TryRecvError};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use std::{fs::File, io::Read};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1080.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ReInk",
        options,
        Box::new(|_| {
            Ok(Box::new(ReinkGui::new(source_mode_from_args(
                std::env::args(),
            ))))
        }),
    )
}

pub struct ReinkGui {
    state: GuiState,
    source_mode: SourceMode,
    selected_fixture: Option<usize>,
    show_validation_report: bool,
    loaded_eeprom: Option<LoadedEeprom>,
    usb_candidates: Vec<DescriptorCandidate>,
    selected_usb_candidate: Option<usize>,
    selected_usb_model: Option<String>,
    usb_scan_status: UsbScanStatus,
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    usb_scan_receiver: Option<Receiver<Result<Vec<reink_usb::UsbPrinterCandidate>, String>>>,
    file_error: Option<String>,
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    usb_operation_receiver: Option<Receiver<UsbOperationOutcome>>,
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    usb_operation_result: Option<UsbOperationResultReport>,
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    pending_usb_operation: Option<PendingUsbOperation>,
    model_filter: String,
    usb_model_filter: String,
    debug_traffic: DebugTrafficTrace,
    debug_height: f32,
}

struct LoadedEeprom {
    path: String,
    bytes: Vec<u8>,
    start_address: usize,
    selected_offset: usize,
    model: Option<String>,
}

struct EepromFileField {
    address: usize,
    end_address: usize,
    label: String,
    encoding: EepromFieldEncoding,
    confidence: Option<EepromFieldConfidence>,
    evidence_note: Option<String>,
    sensitive: bool,
}

enum UsbScanStatus {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    Scanning,
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    Ready,
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    Failed(String),
    Unavailable,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
struct UsbOperationOutcome {
    operation: Result<UsbOperationSuccess, String>,
    cleanup: UsbSessionCleanup,
    events: Vec<TransportEvent>,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
enum UsbOperationSuccess {
    Status {
        model: String,
        response_bytes: usize,
        display: String,
    },
    Dump {
        image: EepromImage,
        output_file: PathBuf,
    },
    Mutation {
        action: &'static str,
        model: String,
        backup_file: PathBuf,
        current_values: Vec<(u16, u8)>,
        update_count: usize,
    },
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
enum UsbOperationRequest {
    Status,
    Dump {
        output_file: PathBuf,
    },
    Write {
        updates: Vec<(u16, u8)>,
        backup_file: PathBuf,
    },
    Restore {
        image: Vec<u8>,
        backup_file: PathBuf,
    },
    Reset {
        target: CounterResetTarget,
        backup_file: PathBuf,
    },
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
enum PendingUsbOperation {
    Write {
        address: String,
        value: String,
        backup_file: Option<PathBuf>,
        confirmation: String,
    },
    Restore {
        restore_image: Option<RestoreImagePreflight>,
        backup_file: Option<PathBuf>,
        confirmation: String,
    },
    Reset {
        target: CounterResetTarget,
        backup_file: Option<PathBuf>,
        confirmation: String,
    },
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
struct RestoreImagePreflight {
    path: PathBuf,
    result: Result<ValidatedRestoreImage, String>,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
struct ValidatedRestoreImage {
    bytes: Vec<u8>,
    update_count: usize,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
struct UsbOperationResultReport {
    success: bool,
    headline: String,
    lines: Vec<String>,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
const EEPROM_WRITE_CONFIRMATION: &str = "I_CONFIRM_THIS_WILL_WRITE_EEPROM";
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
const EEPROM_RESTORE_CONFIRMATION: &str = "I_CONFIRM_THIS_WILL_RESTORE_EEPROM";
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
const EEPROM_COUNTER_RESET_CONFIRMATION: &str = "I_CONFIRM_THIS_WILL_RESET_DECLARED_COUNTERS";

fn source_mode_from_args(arguments: impl IntoIterator<Item = String>) -> SourceMode {
    if arguments
        .into_iter()
        .any(|argument| argument == "--fixtures")
    {
        SourceMode::Fixtures
    } else {
        SourceMode::Real
    }
}

impl ReinkGui {
    fn new(source_mode: SourceMode) -> Self {
        let mut gui = Self {
            state: GuiState::new().expect("the built-in model database is valid"),
            source_mode,
            selected_fixture: source_mode.fixtures_enabled().then_some(0),
            show_validation_report: false,
            loaded_eeprom: None,
            usb_candidates: Vec::new(),
            selected_usb_candidate: None,
            selected_usb_model: None,
            usb_scan_status: UsbScanStatus::Unavailable,
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            usb_scan_receiver: None,
            file_error: None,
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            usb_operation_receiver: None,
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            usb_operation_result: None,
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            pending_usb_operation: None,
            model_filter: String::new(),
            usb_model_filter: String::new(),
            debug_traffic: DebugTrafficTrace::new(),
            debug_height: 180.0,
        };
        gui.refresh_usb_candidates();
        gui
    }

    /// Adds events captured under the explicit operation's sampled debug opt-in.
    ///
    /// This is intentionally a session-only integration seam: the trace model
    /// retains these events only because the selected worker operation started
    /// after the user explicitly enabled capture.
    pub fn append_recorded_transport_events(&mut self, events: Vec<TransportEvent>) -> usize {
        self.debug_traffic.append_captured_events(events)
    }

    fn selected_usb_candidate(&self) -> Option<&DescriptorCandidate> {
        self.selected_usb_candidate
            .and_then(|index| self.usb_candidates.get(index))
    }

    fn selected_usb_model(&self) -> Option<&str> {
        selected_model_for_usb_candidate(
            &self.state,
            self.selected_usb_candidate(),
            self.selected_usb_model.as_deref(),
        )
    }

    fn source_label(&self) -> String {
        if let Some(candidate) = self.selected_usb_candidate() {
            candidate.alias.clone()
        } else if let Some(file) = &self.loaded_eeprom {
            file.path.clone()
        } else if self.selected_fixture.is_some() {
            self.state.selected_fixture().label.to_owned()
        } else {
            "No printer selected".to_owned()
        }
    }

    fn fixture_selected(&self) -> bool {
        self.source_mode.fixtures_enabled() && self.selected_fixture.is_some()
    }

    fn usb_scan_status_label(&self) -> String {
        match &self.usb_scan_status {
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            UsbScanStatus::Scanning => "Scanning USB printer descriptors…".to_owned(),
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            UsbScanStatus::Ready => format!(
                "{} USB descriptor candidate(s) found",
                self.usb_candidates.len()
            ),
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            UsbScanStatus::Failed(error) => format!("USB descriptor scan failed: {error}"),
            UsbScanStatus::Unavailable => {
                "USB descriptor enumeration is unavailable on Windows.".to_owned()
            }
        }
    }

    fn selected_address(&self) -> usize {
        self.loaded_eeprom
            .as_ref()
            .map(|file| file.start_address + file.selected_offset)
            .unwrap_or(self.state.selected_eeprom_row().address as usize)
    }

    fn selected_value(&self) -> u8 {
        self.loaded_eeprom
            .as_ref()
            .map(|file| file.bytes[file.selected_offset])
            .unwrap_or(self.state.selected_eeprom_row().value)
    }

    fn selected_field_label(&self) -> String {
        if let Some(field) = self
            .file_fields()
            .into_iter()
            .find(|field| field.address == self.selected_address())
        {
            field.label
        } else if self.loaded_eeprom.is_some() {
            "Raw EEPROM byte".to_owned()
        } else {
            self.state.selected_eeprom_row().label.to_owned()
        }
    }

    fn eeprom_bytes(&self) -> &[u8] {
        self.loaded_eeprom
            .as_ref()
            .map(|file| file.bytes.as_slice())
            .unwrap_or(self.state.selected_fixture().eeprom_bytes)
    }

    fn file_fields(&self) -> Vec<EepromFileField> {
        let Some(model) = self
            .loaded_eeprom
            .as_ref()
            .and_then(|file| file.model.as_deref())
        else {
            return Vec::new();
        };
        let Some(spec) = self.state.model_spec(model) else {
            return Vec::new();
        };

        if spec.read_only_fields.is_empty() {
            spec.memory_operations
                .iter()
                .flat_map(|operation| {
                    operation.addresses.iter().map(|address| EepromFileField {
                        address: *address as usize,
                        end_address: *address as usize,
                        label: operation.description.clone(),
                        encoding: EepromFieldEncoding::RawBytes,
                        confidence: None,
                        evidence_note: None,
                        sensitive: false,
                    })
                })
                .collect()
        } else {
            spec.read_only_fields
                .iter()
                .map(|field| EepromFileField {
                    address: usize::from(field.address),
                    end_address: usize::from(field.end_address),
                    label: field.label.clone(),
                    encoding: field.encoding,
                    confidence: Some(field.confidence),
                    evidence_note: Some(field.evidence_note.clone()),
                    sensitive: field.sensitive,
                })
                .collect()
        }
    }

    fn select_eeprom_address(&mut self, address: usize) {
        if let Some(file) = &mut self.loaded_eeprom {
            if address >= file.start_address {
                let offset = address - file.start_address;
                if offset < file.bytes.len() {
                    file.selected_offset = offset;
                }
            }
        } else if let Some(index) = self
            .state
            .selected_fixture()
            .eeprom_rows
            .iter()
            .position(|row| row.address as usize == address)
        {
            self.state.select_eeprom_row(index);
        }
    }

    fn open_eeprom_file(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("EEPROM image", &["bin", "eeprom", "eep", "rom"])
            .pick_file()
        else {
            return;
        };

        match std::fs::read(&path) {
            Ok(bytes) if bytes.is_empty() => {
                self.file_error = Some("The selected EEPROM file is empty.".to_owned());
            }
            Ok(bytes) => {
                self.loaded_eeprom = Some(LoadedEeprom {
                    path: path.display().to_string(),
                    bytes,
                    start_address: 0,
                    selected_offset: 0,
                    model: None,
                });
                self.selected_usb_candidate = None;
                self.selected_usb_model = None;
                self.selected_fixture = None;
                self.file_error = None;
                #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
                {
                    self.usb_operation_result = None;
                    self.pending_usb_operation = None;
                }
                self.show_validation_report = false;
                self.state.navigate_to(Page::Eeprom);
            }
            Err(error) => {
                self.file_error = Some(format!("Unable to open EEPROM file: {error}"));
            }
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn refresh_usb_candidates(&mut self) {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        self.usb_scan_receiver = Some(receiver);
        self.usb_scan_status = UsbScanStatus::Scanning;
        std::thread::spawn(move || {
            let result = reink_usb::list_printer_candidates().map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    fn refresh_usb_candidates(&mut self) {
        self.usb_scan_status = UsbScanStatus::Unavailable;
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn poll_usb_candidates(&mut self) {
        let result =
            self.usb_scan_receiver
                .as_ref()
                .and_then(|receiver| match receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => Some(Err(
                        "The USB descriptor scan stopped unexpectedly.".to_owned(),
                    )),
                });
        let Some(result) = result else {
            return;
        };

        self.usb_scan_receiver = None;
        match result {
            Ok(candidates) => {
                self.usb_candidates = candidates
                    .into_iter()
                    .enumerate()
                    .map(|(index, candidate)| DescriptorCandidate {
                        alias: format!("usb-{}", index + 1),
                        vendor_id: candidate.vendor_id,
                        product_id: candidate.product_id,
                        bus_number: candidate.bus_number,
                        device_address: candidate.device_address,
                        interface_number: candidate.interface_number,
                        alternate_setting: candidate.alternate_setting,
                        model_hints: self.state.model_hints_for_usb_candidate(
                            candidate.vendor_id,
                            candidate.product_id,
                        ),
                    })
                    .collect();
                self.selected_usb_candidate = None;
                self.selected_usb_model = None;
                self.usb_scan_status = UsbScanStatus::Ready;
            }
            Err(error) => self.usb_scan_status = UsbScanStatus::Failed(error),
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    fn poll_usb_candidates(&mut self) {}

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn usb_operation_in_progress(&self) -> bool {
        self.usb_operation_receiver.is_some()
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn poll_usb_operation(&mut self) {
        let result =
            self.usb_operation_receiver
                .as_ref()
                .and_then(|receiver| match receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => Some(UsbOperationOutcome {
                        operation: Err(
                            "The selected-printer operation stopped unexpectedly.".to_owned()
                        ),
                        cleanup: UsbSessionCleanup::not_attempted(),
                        events: Vec::new(),
                    }),
                });
        let Some(result) = result else {
            return;
        };

        self.usb_operation_receiver = None;
        let UsbOperationOutcome {
            operation,
            cleanup,
            events,
        } = result;
        if !events.is_empty() {
            self.append_recorded_transport_events(events);
        }
        let cleanup_lines = cleanup_report_lines(&cleanup);
        match operation {
            Ok(UsbOperationSuccess::Status {
                model,
                response_bytes,
                display,
            }) => {
                self.usb_operation_result = Some(UsbOperationResultReport {
                    success: cleanup_is_successful(&cleanup),
                    headline: if cleanup_is_successful(&cleanup) {
                        "Printer status completed".to_owned()
                    } else {
                        "Printer status read completed; cleanup needs attention".to_owned()
                    },
                    lines: [
                        vec![
                            format!("Exact D4 identity/model match confirmed for {model}."),
                            format!("Status response ({response_bytes} byte(s)): {display}"),
                        ],
                        cleanup_lines,
                    ]
                    .concat(),
                });
            }
            Ok(UsbOperationSuccess::Dump { image, output_file }) => {
                self.loaded_eeprom = Some(LoadedEeprom {
                    path: output_file.display().to_string(),
                    bytes: image.bytes,
                    start_address: usize::from(image.start_address),
                    selected_offset: 0,
                    model: Some(image.model),
                });
                self.selected_fixture = None;
                self.file_error = None;
                self.show_validation_report = false;
                self.state.navigate_to(Page::Eeprom);
                self.usb_operation_result = Some(UsbOperationResultReport {
                    success: cleanup_is_successful(&cleanup),
                    headline: if cleanup_is_successful(&cleanup) {
                        "EEPROM dump saved".to_owned()
                    } else {
                        "EEPROM dump saved; cleanup needs attention".to_owned()
                    },
                    lines: [
                        vec![
                            "Exact D4 identity/model match confirmed before EEPROM access."
                                .to_owned(),
                            format!(
                                "Saved a complete {}-byte model-bounded image (0x{:04X}..=0x{:04X}) to {}.",
                                self.loaded_eeprom.as_ref().map_or(0, |file| file.bytes.len()),
                                self.loaded_eeprom.as_ref().map_or(0, |file| file.start_address),
                                self.loaded_eeprom.as_ref().map_or(0, |file| {
                                    file.start_address + file.bytes.len().saturating_sub(1)
                                }),
                                output_file.display(),
                            ),
                        ],
                        cleanup_lines,
                    ]
                    .concat(),
                });
            }
            Ok(UsbOperationSuccess::Mutation {
                action,
                model,
                backup_file,
                current_values,
                update_count,
            }) => {
                self.usb_operation_result = Some(UsbOperationResultReport {
                    success: cleanup_is_successful(&cleanup),
                    headline: if cleanup_is_successful(&cleanup) {
                        format!("EEPROM {action} completed")
                    } else {
                        format!("EEPROM {action} completed; cleanup needs attention")
                    },
                    lines: [
                        vec![
                            format!("Exact D4 identity/model match confirmed for {model}."),
                            format!(
                                "Created and synchronized complete pre-operation backup: {}.",
                                backup_file.display()
                            ),
                            format!(
                                "Current values captured before the write: {}.",
                                format_current_values(&current_values)
                            ),
                            format!(
                                "Read-back verification succeeded for all {update_count} written byte(s); rollback was not needed."
                            ),
                        ],
                        cleanup_lines,
                    ]
                    .concat(),
                });
            }
            Err(error) => {
                self.usb_operation_result = Some(UsbOperationResultReport {
                    success: false,
                    headline: "Selected-printer operation failed".to_owned(),
                    lines: [
                        vec![
                            error,
                            "If a write reached EEPROM, the guarded write plan attempted read-back verification and rollback; the operation error reports any rollback failure."
                                .to_owned(),
                        ],
                        cleanup_lines,
                    ]
                    .concat(),
                });
            }
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    fn poll_usb_operation(&mut self) {}

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn start_selected_usb_operation(&mut self, request: UsbOperationRequest) {
        if self.usb_operation_receiver.is_some() {
            return;
        }
        if matches!(&self.usb_scan_status, UsbScanStatus::Scanning) {
            self.usb_operation_result = Some(UsbOperationResultReport {
                success: false,
                headline: "Selected-printer operation blocked".to_owned(),
                lines: vec![
                    "Wait for USB candidate enumeration to finish before starting an operation."
                        .to_owned(),
                ],
            });
            return;
        }
        let Some(candidate) = self.selected_usb_candidate().cloned() else {
            return;
        };
        let Some(expected_model) = self.selected_usb_model().map(str::to_owned) else {
            self.usb_operation_result = Some(UsbOperationResultReport {
                success: false,
                headline: "Selected-printer operation blocked".to_owned(),
                lines: vec![
                    "Select one expected bundled model for the selected candidate before opening USB."
                        .to_owned(),
                ],
            });
            return;
        };
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        self.usb_operation_receiver = Some(receiver);
        self.usb_operation_result = None;
        let record_traffic = self.debug_traffic.capture_enabled();
        if record_traffic {
            self.debug_traffic.begin_operation();
        }
        std::thread::spawn(move || {
            let outcome =
                run_selected_usb_operation(candidate, expected_model, request, record_traffic);
            let _ = sender.send(outcome);
        });
    }

    fn tab_strip_pane(&mut self, ui: &mut egui::Ui) {
        ui.allocate_ui_with_layout(
            ui.available_size(),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                for (page, label) in [
                    (Page::Status, "Status"),
                    (Page::Eeprom, "EEPROM"),
                    (Page::Tools, "Tools"),
                ] {
                    if ui
                        .selectable_label(self.state.page() == page, label)
                        .clicked()
                    {
                        self.state.navigate_to(page);
                    }
                }

                let selector_width = ui.available_width();
                ui.allocate_ui_with_layout(
                    egui::vec2(selector_width, ui.spacing().interact_size.y),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let selected_fixture = self.selected_fixture;
                        let selected_usb_candidate = self.selected_usb_candidate;
                        let source_label = self.source_label();
                        let source_display = end_truncate(&source_label, 42);
                        let mut open_file = false;
                        let source_response = egui::ComboBox::from_id_salt("fixture-device")
                            .selected_text(source_display)
                            .width(320.0)
                            .show_ui(ui, |ui| {
                                #[cfg(any(
                                    target_os = "linux",
                                    target_os = "macos",
                                    target_os = "windows"
                                ))]
                                let source_changes_allowed = !self.usb_operation_in_progress();
                                #[cfg(not(any(
                                    target_os = "linux",
                                    target_os = "macos",
                                    target_os = "windows"
                                )))]
                                let source_changes_allowed = true;
                                ui.strong("USB descriptor candidates");
                                if !source_changes_allowed {
                                    ui.label(
                                        "Source selection is locked until the selected-printer operation finishes.",
                                    );
                                }
                                ui.add_enabled_ui(source_changes_allowed, |ui| {
                                if self.usb_candidates.is_empty() {
                                    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
                                    ui.label(match &self.usb_scan_status {
                                        UsbScanStatus::Scanning => "Scanning…",
                                        _ => "No USB descriptor candidates",
                                    });
                                    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
                                    ui.label("No USB descriptor candidates");
                                }
                                for (index, candidate) in self.usb_candidates.iter().enumerate() {
                                    let label = format!(
                                        "{} — {:04X}:{:04X}, bus {}, address {}, interface {} alt {}",
                                        candidate.alias,
                                        candidate.vendor_id,
                                        candidate.product_id,
                                        candidate.bus_number,
                                        candidate.device_address,
                                        candidate.interface_number,
                                        candidate.alternate_setting,
                                    );
                                    if ui
                                        .selectable_label(selected_usb_candidate == Some(index), label)
                                        .clicked()
                                    {
                                        self.selected_usb_candidate = Some(index);
                                        self.selected_usb_model = (candidate.model_hints.len() == 1)
                                            .then(|| candidate.model_hints[0].clone());
                                        self.loaded_eeprom = None;
                                        self.selected_fixture = None;
                                        self.show_validation_report = false;
                                        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
                                        {
                                            self.usb_operation_result = None;
                                            self.pending_usb_operation = None;
                                        }
                                    }
                                }
                                if self.source_mode.fixtures_enabled() {
                                    ui.separator();
                                    ui.strong("Fixtures");
                                    for (index, fixture) in
                                        reink_gui::FIXTURE_DEVICES.iter().enumerate()
                                    {
                                        if ui
                                            .selectable_label(
                                                self.loaded_eeprom.is_none()
                                                    && self.selected_usb_candidate.is_none()
                                                    && selected_fixture == Some(index),
                                                fixture.label,
                                            )
                                            .clicked()
                                        {
                                            self.loaded_eeprom = None;
                                            self.selected_usb_candidate = None;
                                            self.selected_usb_model = None;
                                            self.selected_fixture = Some(index);
                                            self.state.select_fixture(index);
                                            self.show_validation_report = false;
                                            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
                                            {
                                                self.usb_operation_result = None;
                                                self.pending_usb_operation = None;
                                            }
                                        }
                                    }
                                }
                                ui.separator();
                                if ui.button("Open EEPROM file...").clicked() {
                                    open_file = true;
                                }
                                });
                            });
                        source_response.response.on_hover_text(source_label);
                        if open_file {
                            self.open_eeprom_file();
                        }
                        ui.label("Printer");
                        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
                        if ui
                            .add_enabled(
                                !matches!(&self.usb_scan_status, UsbScanStatus::Scanning)
                                    && !self.usb_operation_in_progress(),
                                egui::Button::new("Refresh USB candidates"),
                            )
                            .clicked()
                        {
                            self.refresh_usb_candidates();
                        }
                        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
                        ui.add_enabled(false, egui::Button::new("Refresh USB candidates"));
                        ui.label(self.usb_scan_status_label());

                        if self.loaded_eeprom.is_some() {
                            if self.selected_usb_candidate().is_some() {
                                ui.label(
                                    self.loaded_eeprom
                                        .as_ref()
                                        .and_then(|file| file.model.as_deref())
                                        .unwrap_or("D4-confirmed model unavailable"),
                                );
                                ui.label("D4-confirmed model");
                            } else {
                                let current_model = self
                                    .loaded_eeprom
                                    .as_ref()
                                    .and_then(|file| file.model.clone());
                                let mut selected_model = current_model.clone();
                                let model_names = self
                                    .state
                                    .model_names()
                                    .map(str::to_owned)
                                    .collect::<Vec<_>>();
                                let filter = RegexBuilder::new(&self.model_filter)
                                    .case_insensitive(true)
                                    .build()
                                    .ok();
                                let combo_id = ui.make_persistent_id("eeprom-model");
                                let focus_filter = selected_model.is_none()
                                    && !egui::ComboBox::is_open(ui.ctx(), combo_id);
                                let model_popup_height =
                                    72.0 + 5.0 * ui.spacing().interact_size.y;
                                egui::ComboBox::from_id_salt("eeprom-model")
                                    .selected_text(
                                        selected_model.as_deref().unwrap_or("Select model..."),
                                    )
                                    .width(180.0)
                                    .height(model_popup_height)
                                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                                    .show_ui(ui, |ui| {
                                        ui.label("Filter (case-insensitive regex)");
                                        let filter_response = ui.add(
                                            egui::TextEdit::singleline(&mut self.model_filter)
                                                .id_salt("eeprom-model-filter"),
                                        );
                                        if focus_filter {
                                            filter_response.request_focus();
                                        }
                                        if !self.model_filter.is_empty() && filter.is_none() {
                                            ui.colored_label(
                                                Color32::RED,
                                                "Invalid regular expression",
                                            );
                                        }
                                        ui.separator();
                                        for model in &model_names {
                                            if !self.model_filter.is_empty()
                                                && !filter
                                                    .as_ref()
                                                    .is_some_and(|regex| regex.is_match(model))
                                            {
                                                continue;
                                            }
                                            if ui
                                                .selectable_label(
                                                    selected_model.as_deref()
                                                        == Some(model.as_str()),
                                                    model,
                                                )
                                                .clicked()
                                            {
                                                selected_model = Some(model.clone());
                                                ui.close();
                                            }
                                        }
                                    });
                                if selected_model != current_model {
                                    if let Some(file) = &mut self.loaded_eeprom {
                                        file.model = selected_model;
                                    }
                                }
                            }
                        }
                    },
                );
            },
        );
    }

    fn status(&mut self, ui: &mut egui::Ui) {
        ui.heading("Printer status");
        if let Some(candidate) = self.selected_usb_candidate().cloned() {
            egui::Grid::new("usb-descriptor-candidate")
                .num_columns(2)
                .spacing([18.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Source");
                    ui.label("Descriptor-only candidate");
                    ui.end_row();
                    ui.label("Selected alias");
                    ui.monospace(&candidate.alias);
                    ui.end_row();
                    ui.label("VID/PID");
                    ui.monospace(format!(
                        "{:04X}:{:04X}",
                        candidate.vendor_id, candidate.product_id
                    ));
                    ui.end_row();
                    ui.label("Bus/address");
                    ui.label(format!(
                        "{} / {}",
                        candidate.bus_number, candidate.device_address
                    ));
                    ui.end_row();
                    ui.label("Interface/alternate setting");
                    ui.label(format!(
                        "{} / {}",
                        candidate.interface_number, candidate.alternate_setting
                    ));
                    ui.end_row();
                    ui.label("Bundled model hints");
                    ui.label(if candidate.model_hints.is_empty() {
                        "No exact VID/PID associations in the bundled model database".to_owned()
                    } else {
                        candidate.model_hints.join(", ")
                    });
                    ui.end_row();
                });
            ui.add_space(8.0);
            ui.label(
                "Select an expected model as an operator-supplied guard. VID/PID associations are hints, not printer identity confirmation; every explicit operation reads the D4 identity and requires an exact model match before access.",
            );
            let selected_model = self.selected_usb_model.clone();
            let model_names = self
                .state
                .model_names()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let filter = RegexBuilder::new(&self.usb_model_filter)
                .case_insensitive(true)
                .build()
                .ok();
            let combo_id = ui.make_persistent_id("selected-usb-model");
            let focus_filter =
                selected_model.is_none() && !egui::ComboBox::is_open(ui.ctx(), combo_id);
            let model_popup_height = 72.0 + 5.0 * ui.spacing().interact_size.y;
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            let model_selection_enabled = !self.usb_operation_in_progress();
            #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
            let model_selection_enabled = true;
            ui.add_enabled_ui(model_selection_enabled, |ui| {
                egui::ComboBox::from_id_salt("selected-usb-model")
                    .selected_text(
                        selected_model
                            .as_deref()
                            .unwrap_or("Choose expected model..."),
                    )
                    .width(220.0)
                    .height(model_popup_height)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show_ui(ui, |ui| {
                        ui.label("Filter (case-insensitive regex)");
                        let filter_response = ui.add(
                            egui::TextEdit::singleline(&mut self.usb_model_filter)
                                .id_salt("selected-usb-model-filter"),
                        );
                        if focus_filter {
                            filter_response.request_focus();
                        }
                        if !self.usb_model_filter.is_empty() && filter.is_none() {
                            ui.colored_label(Color32::RED, "Invalid regular expression");
                        }
                        ui.separator();
                        for model in &model_names {
                            if !self.usb_model_filter.is_empty()
                                && !filter.as_ref().is_some_and(|regex| regex.is_match(model))
                            {
                                continue;
                            }
                            if ui
                                .selectable_label(selected_model.as_deref() == Some(model), model)
                                .clicked()
                            {
                                self.selected_usb_model = Some(model.clone());
                                #[cfg(any(
                                    target_os = "linux",
                                    target_os = "macos",
                                    target_os = "windows"
                                ))]
                                {
                                    self.usb_operation_result = None;
                                    self.pending_usb_operation = None;
                                }
                                ui.close();
                            }
                        }
                    });
            });
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            {
                let enabled =
                    self.selected_usb_model().is_some() && !self.usb_operation_in_progress();
                if self.usb_operation_in_progress() {
                    ui.strong("A selected-printer operation is running on a worker thread.");
                } else {
                    ui.label("No printer connection has been opened until one of these buttons is pressed.");
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(enabled, egui::Button::new("Read printer status"))
                        .clicked()
                    {
                        self.start_selected_usb_operation(UsbOperationRequest::Status);
                    }
                    if ui
                        .add_enabled(enabled, egui::Button::new("Save complete EEPROM dump..."))
                        .clicked()
                    {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_file_name("eeprom-image.bin")
                            .add_filter("EEPROM image", &["bin", "eeprom", "eep", "rom"])
                            .save_file()
                        {
                            self.start_selected_usb_operation(UsbOperationRequest::Dump {
                                output_file: path,
                            });
                        }
                    }
                });
                ui.label(
                    "Raw identity fields are not retained in the GUI result. The explicitly requested status response is shown in the result pane; EEPROM images are private device-specific files.",
                );
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
            {
                ui.strong("Selected-printer operations are unavailable on this platform.");
                ui.add_enabled(false, egui::Button::new("Read printer status"));
                ui.add_enabled(false, egui::Button::new("Save complete EEPROM dump..."));
            }
            if self.selected_usb_model().is_none() {
                ui.colored_label(
                    Color32::from_rgb(174, 112, 0),
                    "Choose one expected model before opening the selected USB interface. It is confirmed only by the exact D4 identity read for the operation.",
                );
            }
            if let Some(file) = &self.loaded_eeprom {
                ui.add_space(8.0);
                ui.label(format!(
                    "Most recent connected image: {} bytes, 0x{:04X}..=0x{:04X}.",
                    file.bytes.len(),
                    file.start_address,
                    file.start_address + file.bytes.len().saturating_sub(1)
                ));
            }
            return;
        }

        if let Some(file) = &self.loaded_eeprom {
            egui::Grid::new("file-identity")
                .num_columns(2)
                .spacing([18.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Source");
                    ui.label("EEPROM file");
                    ui.end_row();
                    ui.label("Path");
                    ui.monospace(&file.path);
                    ui.end_row();
                    ui.label("Size");
                    ui.label(format!("{} bytes", file.bytes.len()));
                    ui.end_row();
                    ui.label("Model");
                    ui.label(
                        file.model
                            .as_deref()
                            .unwrap_or("Select a model in the tab bar"),
                    );
                    ui.end_row();
                });
            return;
        }

        if !self.fixture_selected() {
            ui.strong("No printer selected");
            ui.label(
                "Default real mode has no fixture source. Select a descriptor-only candidate or open a raw EEPROM file.",
            );
            ui.label(
                "No printer connection, claim, driver handoff, control request, or protocol traffic has occurred.",
            );
            return;
        }

        let fixture = self.state.selected_fixture();
        let resolution = self.state.identity_resolution();
        egui::Grid::new("identity-resolution")
            .num_columns(2)
            .spacing([18.0, 8.0])
            .show(ui, |ui| {
                ui.label("Fixture IEEE 1284 ID");
                ui.monospace(fixture.identity);
                ui.end_row();
                ui.label("Manufacturer");
                ui.label(resolution.manufacturer.as_deref().unwrap_or("Unavailable"));
                ui.end_row();
                ui.label("Advertised model");
                ui.label(
                    resolution
                        .advertised_model
                        .as_deref()
                        .unwrap_or("Unavailable"),
                );
                ui.end_row();
                ui.label("Bundled model match");
                ui.label(
                    resolution
                        .resolved_model
                        .as_deref()
                        .unwrap_or("No built-in match"),
                );
                ui.end_row();
            });
    }

    fn eeprom_viewer(&mut self, ui: &mut egui::Ui) {
        ui.heading("EEPROM");
        if self.selected_usb_candidate().is_some() && self.loaded_eeprom.is_none() {
            ui.label("No EEPROM data is available for the selected descriptor-only candidate.");
            ui.label(
                "No printer connection has been opened; use the Status tab to explicitly save a complete EEPROM dump first.",
            );
            return;
        }
        if self.loaded_eeprom.is_none() && !self.fixture_selected() {
            ui.label("No EEPROM data is available because no printer source is selected.");
            ui.label(
                "Select a descriptor-only candidate or open a raw EEPROM file. Fixtures require launching with --fixtures.",
            );
            return;
        }
        if self.loaded_eeprom.is_some() {
            if self.selected_usb_candidate().is_some() {
                ui.label(
                    "The most recently saved connected EEPROM image is shown below. It is a private device-specific file; displayed values are not a substitute for the pre-write backup taken by a persistent operation.",
                );
            } else {
                ui.label("The selected raw EEPROM image is shown below. Choose a model in the tab bar to label known model-specific fields.");
            }
        } else {
            ui.label(
                "The full 256-byte fixture EEPROM is shown below; no physical EEPROM is read, written, backed up, restored, or reset.",
            );
        }
        ui.add_space(8.0);
        let rows = self.state.selected_fixture().eeprom_rows;
        let bytes = self.eeprom_bytes().to_vec();
        let selected_address = self.selected_address();
        let selected_value = self.selected_value();
        let selected_label = self.selected_field_label();
        let selected_is_file = self.loaded_eeprom.is_some();
        let file_start_address = self
            .loaded_eeprom
            .as_ref()
            .map_or(0, |file| file.start_address);
        let file_fields = self.file_fields();
        let mut selected_address_from_dump = None;
        let field_column_width = eeprom_field_column_width(
            ui,
            &rows,
            &file_fields,
            selected_is_file,
            selected_label.as_str(),
        );
        let address_width = 84.0;
        let value_width = 160.0;
        let field_panel_width = address_width + field_column_width + value_width + 32.0;
        let dump_panel_width = 600.0;
        let dump_panel_height = 48.0
            + bytes.chunks(16).count() as f32
                * (ui.text_style_height(&egui::TextStyle::Body) + 4.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(field_panel_width, dump_panel_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.heading("Fields");
                    egui::Frame::default()
                        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                        .inner_margin(egui::Margin::same(4))
                        .show(ui, |ui| {
                            eeprom_table_row(
                                ui,
                                address_width,
                                field_column_width,
                                value_width,
                                "Address",
                                "Field",
                                "Value",
                                false,
                                true,
                                24.0,
                            );
                            if selected_is_file && file_fields.is_empty() {
                                ui.separator();
                                eeprom_table_row(
                                    ui,
                                    address_width,
                                    field_column_width,
                                    value_width,
                                    &format!("0x{selected_address:04X}"),
                                    "Raw EEPROM byte",
                                    &format!("0x{selected_value:02X}"),
                                    true,
                                    false,
                                    42.0,
                                );
                            } else if selected_is_file {
                                for field in &file_fields {
                                    if eeprom_image_offset(
                                        file_start_address,
                                        bytes.len(),
                                        field.address,
                                    )
                                    .is_none()
                                    {
                                        continue;
                                    }
                                    ui.separator();
                                    let value = eeprom_field_value(
                                        field,
                                        file_start_address,
                                        &bytes,
                                    );
                                    let response = eeprom_table_row(
                                        ui,
                                        address_width,
                                        field_column_width,
                                        value_width,
                                        &eeprom_field_address_label(field),
                                        &field.label,
                                        &value,
                                        field.address == selected_address,
                                        false,
                                        42.0,
                                    )
                                    .on_hover_text(eeprom_field_tooltip(field));
                                    if response.clicked() {
                                        selected_address_from_dump = Some(field.address);
                                    }
                                }
                            } else {
                                for (index, row) in rows.iter().enumerate() {
                                    ui.separator();
                                    let is_selected =
                                        self.state.selected_eeprom_row_index() == index;
                                    let response = eeprom_table_row(
                                        ui,
                                        address_width,
                                        field_column_width,
                                        value_width,
                                        &format!("0x{:04X}", row.address),
                                        row.label,
                                        &format!("0x{:02X}", row.value),
                                        is_selected,
                                        false,
                                        42.0,
                                    );
                                    if response.clicked() {
                                        self.state.select_eeprom_row(index);
                                    }
                                }
                            }
                        });
                    ui.add_space(16.0);
                    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
                    if self.selected_usb_candidate().is_some() {
                        let enabled = self.selected_usb_model().is_some()
                            && !self.usb_operation_in_progress();
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::new("Prepare guarded write of selected byte..."),
                            )
                            .clicked()
                        {
                            self.pending_usb_operation = Some(PendingUsbOperation::Write {
                                address: format!("0x{selected_address:04X}"),
                                value: format!("0x{selected_value:02X}"),
                                backup_file: None,
                                confirmation: String::new(),
                            });
                        }
                        ui.label(format!(
                            "Selected cached field: {selected_label} at 0x{selected_address:04X}. The worker reads a complete fresh backup and reports the current value before writing."
                        ));
                    } else {
                        ui.add_enabled(
                            false,
                            egui::Button::new("Prepare guarded write of selected byte..."),
                        );
                        ui.label(
                            "Writing requires an explicit selected USB candidate, exact identity/model match, a create-new backup, and a typed confirmation.",
                        );
                    }
                    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
                    {
                        ui.add_enabled(
                            false,
                            egui::Button::new("Prepare guarded write of selected byte..."),
                        );
                        ui.label("Selected-printer EEPROM writes are unavailable on this platform.");
                    }
                },
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(dump_panel_width, dump_panel_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::default()
                        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                        .inner_margin(egui::Margin::same(4))
                        .show(ui, |ui| {
                            ui.heading("Hex dump");
                            egui::Grid::new("fixture-eeprom-dump-grid")
                                .num_columns(3)
                                .spacing([12.0, 4.0])
                                .show(ui, |ui| {
                                    ui.strong("Address");
                                    ui.strong("Bytes");
                                    ui.strong("ASCII");
                                    ui.end_row();

                                    for (line, chunk) in bytes.chunks(16).enumerate() {
                                        let line_address = self
                                            .loaded_eeprom
                                            .as_ref()
                                            .map(|file| file.start_address + line * 16)
                                            .unwrap_or(line * 16);
                                        ui.monospace(format!("{line_address:04X}"));
                                        ui.horizontal(|ui| {
                                            for (index, byte) in chunk.iter().enumerate() {
                                                let address = line_address + index;
                                                if ui
                                                    .add(
                                                        egui::Label::new(dump_cell(
                                                            format!("{byte:02X}"),
                                                            address == selected_address,
                                                        ))
                                                        .sense(egui::Sense::click()),
                                                    )
                                                    .clicked()
                                                {
                                                    selected_address_from_dump = Some(address);
                                                }
                                            }
                                        });
                                        ui.label(ascii_dump_line(
                                            chunk,
                                            line_address,
                                            selected_address,
                                        ));
                                        ui.end_row();
                                    }
                                });
                        });
                },
            );
        });
        if let Some(address) = selected_address_from_dump {
            self.select_eeprom_address(address);
        }
    }

    fn tools(&mut self, ui: &mut egui::Ui) {
        ui.heading("Tools");
        if self.selected_usb_candidate().is_some() {
            ui.label(
                "Selected-printer operations are explicit, run on a worker thread, and never run when selecting this candidate.",
            );
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            {
                let enabled =
                    self.selected_usb_model().is_some() && !self.usb_operation_in_progress();
                if self.selected_usb_model().is_none() {
                    ui.colored_label(
                        Color32::from_rgb(174, 112, 0),
                        "Choose an expected model on the Status tab before enabling connected operations.",
                    );
                }
                if self.usb_operation_in_progress() {
                    ui.strong("A selected-printer operation is running on a worker thread.");
                }
                ui.add_space(12.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("Durable EEPROM dump");
                    ui.label(
                        "Reads a complete model-bounded image only after an exact D4 identity/model match, then creates a new user-selected private file.",
                    );
                    if ui
                        .add_enabled(enabled, egui::Button::new("Save complete EEPROM dump..."))
                        .clicked()
                    {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_file_name("eeprom-image.bin")
                            .add_filter("EEPROM image", &["bin", "eeprom", "eep", "rom"])
                            .save_file()
                        {
                            self.start_selected_usb_operation(UsbOperationRequest::Dump {
                                output_file: path,
                            });
                        }
                    }
                });
                ui.add_space(12.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("Generic EEPROM byte write");
                    ui.label(
                        "The confirmation dialog validates one model-bounded address/value, requires a create-new full backup, then uses read-back verification and rollback-on-failure.",
                    );
                    if ui
                        .add_enabled(enabled, egui::Button::new("Prepare guarded EEPROM write..."))
                        .clicked()
                    {
                        self.pending_usb_operation = Some(PendingUsbOperation::Write {
                            address: String::new(),
                            value: String::new(),
                            backup_file: None,
                            confirmation: String::new(),
                        });
                    }
                });
                ui.add_space(12.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("EEPROM restore");
                    ui.label(
                        "Select a complete image and a separate new rollback backup. The model range and image length are validated before USB opens.",
                    );
                    if ui
                        .add_enabled(enabled, egui::Button::new("Choose image and prepare restore..."))
                        .clicked()
                    {
                        self.pending_usb_operation = Some(PendingUsbOperation::Restore {
                            restore_image: None,
                            backup_file: None,
                            confirmation: String::new(),
                        });
                    }
                });
                ui.add_space(12.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.heading("Model-aware counter reset");
                    ui.label(
                        "Only explicitly declared model reset bytes are eligible; missing reset metadata is never substituted with zero.",
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(enabled, egui::Button::new("Prepare waste reset..."))
                            .clicked()
                        {
                            self.pending_usb_operation = Some(PendingUsbOperation::Reset {
                                target: CounterResetTarget::Waste,
                                backup_file: None,
                                confirmation: String::new(),
                            });
                        }
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::new("Prepare platen-pad reset..."),
                            )
                            .clicked()
                        {
                            self.pending_usb_operation = Some(PendingUsbOperation::Reset {
                                target: CounterResetTarget::PlatenPad,
                                backup_file: None,
                                confirmation: String::new(),
                            });
                        }
                    });
                });
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
            {
                ui.strong("Selected-printer operations are unavailable on this platform.");
                ui.add_enabled(false, egui::Button::new("Save complete EEPROM dump..."));
                ui.add_enabled(false, egui::Button::new("Prepare guarded EEPROM write..."));
                ui.add_enabled(
                    false,
                    egui::Button::new("Choose image and prepare restore..."),
                );
                ui.add_enabled(false, egui::Button::new("Prepare waste reset..."));
                ui.add_enabled(false, egui::Button::new("Prepare platen-pad reset..."));
            }
            return;
        }
        if self.loaded_eeprom.is_none() && !self.fixture_selected() {
            ui.label(
                "No printer selected; fixture validation is unavailable in default real mode.",
            );
            ui.label("Launch with --fixtures to select a deterministic fixture.");
            ui.add_enabled(false, egui::Button::new("Run validation"));
            return;
        }
        ui.label("Fixture-only maintenance and validation workflows.");
        ui.add_space(12.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("Run validation");
            if self.loaded_eeprom.is_some() {
                ui.label("Unavailable for a raw EEPROM file: no printer model or protocol session is present.");
                ui.add_enabled(false, egui::Button::new("Run validation"));
            } else if ui.button("Run validation").clicked() {
                self.show_validation_report = true;
            } else {
                ui.label("Runs the selected fixture's deterministic validation scenario.");
            }
            if self.loaded_eeprom.is_none() && self.show_validation_report {
                ui.add_space(8.0);
                egui::Grid::new("validation-report")
                    .striped(true)
                    .num_columns(3)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        ui.strong("Status");
                        ui.strong("Check");
                        ui.strong("Detail");
                        ui.end_row();
                        for item in self.state.selected_fixture().validation_report {
                            ui.label(
                                RichText::new(item.status.label()).color(status_color(item.status)),
                            );
                            ui.label(item.check);
                            ui.label(item.detail);
                            ui.end_row();
                        }
                    });
            }
        });
        ui.add_space(12.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("Waste-ink counter");
            ui.label("Fixture value: 1,048");
            ui.add_enabled(false, egui::Button::new("Reset to zero"));
            ui.label("Fixture resets are intentionally unavailable; selected-printer resets require the guarded controls for a real USB candidate.");
        });
        ui.add_space(12.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("EEPROM backup and restore");
            ui.horizontal(|ui| {
                ui.add_enabled(false, egui::Button::new("Back up EEPROM"));
                ui.add_enabled(false, egui::Button::new("Restore EEPROM"));
            });
            ui.label("Backup and restore remain unavailable in fixture-only mode.");
        });
        ui.add_space(12.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.heading("First-write backup");
            ui.label(
                "Persistent writes require a complete backup and command-specific confirmations.",
            );
            ui.label(
                "Fixtures cannot select a USB target or create a backup. Use a real selected candidate for the separately gated controls.",
            );
            ui.add_enabled(false, egui::Button::new("Choose EEPROM backup..."));
            ui.label("GUI backup selection is intentionally unavailable.");
        });
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn selected_operation_dialog(&mut self, context: &egui::Context) {
        let Some(mut pending) = self.pending_usb_operation.take() else {
            return;
        };
        let title = match &pending {
            PendingUsbOperation::Write { .. } => "Confirm guarded EEPROM write",
            PendingUsbOperation::Restore { .. } => "Confirm guarded EEPROM restore",
            PendingUsbOperation::Reset { target, .. } => match *target {
                CounterResetTarget::Waste => "Confirm guarded waste-counter reset",
                CounterResetTarget::PlatenPad => "Confirm guarded platen-pad reset",
            },
        };
        let expected_model = self.selected_usb_model().map(str::to_owned);
        let spec = expected_model
            .as_deref()
            .and_then(|model| self.state.model_spec(model))
            .cloned();
        let mut open = true;
        let mut close_dialog = false;
        let mut dispatch = None;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(true)
            .default_width(580.0)
            .open(&mut open)
            .show(context, |ui| {
                ui.strong("This dialog does not start an operation by itself.");
                ui.label(
                    "The worker will re-read the exact D4 identity, require it to match this expected model, create and sync a new complete backup, then perform the confirmed action.",
                );
                ui.add_space(8.0);
                ui.label(format!(
                    "Selected target: {}",
                    self.selected_usb_candidate()
                        .map_or("no selected USB candidate", |candidate| candidate.alias.as_str())
                ));
                ui.label(format!(
                    "Expected model: {}",
                    expected_model.as_deref().unwrap_or("not selected")
                ));
                let Some(spec) = spec.as_ref() else {
                    ui.colored_label(
                        Color32::from_rgb(181, 47, 47),
                        "The expected model is no longer valid for this candidate. Close this dialog and select it again on Status.",
                    );
                    return;
                };
                ui.label(format!(
                    "Model EEPROM range: 0x{:04X}..=0x{:04X}",
                    spec.memory_low, spec.memory_high
                ));
                ui.separator();

                match &mut pending {
                    PendingUsbOperation::Write {
                        address,
                        value,
                        backup_file,
                        confirmation,
                    } => {
                        ui.heading("Preflight");
                        ui.horizontal(|ui| {
                            ui.label("Address");
                            ui.add(
                                egui::TextEdit::singleline(address)
                                    .hint_text("0x000C or decimal")
                                    .desired_width(140.0),
                            );
                            ui.label("Value");
                            ui.add(
                                egui::TextEdit::singleline(value)
                                    .hint_text("0x00 or decimal")
                                    .desired_width(120.0),
                            );
                        });
                        let update = parse_u16_input(address)
                            .and_then(|address| {
                                parse_u8_input(value).map(|value| (address, value))
                            })
                            .and_then(|(address, value)| {
                                if address < spec.memory_low || address > spec.memory_high {
                                    Err(format!(
                                        "Address 0x{address:04X} is outside the selected model range."
                                    ))
                                } else {
                                    Ok((address, value))
                                }
                            });
                        match &update {
                            Ok((address, value)) => {
                                ui.label(format!(
                                    "Requested update: 0x{address:04X} = 0x{value:02X}. The fresh complete backup provides the current value immediately before writing."
                                ));
                            }
                            Err(error) => {
                                ui.colored_label(Color32::from_rgb(181, 47, 47), error);
                            }
                        }
                        choose_backup_file(ui, backup_file, "eeprom-write-backup.bin");
                        typed_confirmation(
                            ui,
                            confirmation,
                            EEPROM_WRITE_CONFIRMATION,
                            "EEPROM write",
                        );
                        let run = update.is_ok()
                            && backup_file.is_some()
                            && confirmation_matches(confirmation, EEPROM_WRITE_CONFIRMATION);
                        dialog_actions(
                            ui,
                            &mut close_dialog,
                            run,
                            "Run confirmed EEPROM write",
                            || {
                            let (address, value) =
                                update.expect("enabled only with a valid EEPROM update");
                            dispatch = Some(UsbOperationRequest::Write {
                                updates: vec![(address, value)],
                                backup_file: backup_file
                                    .clone()
                                    .expect("enabled only after backup selection"),
                            });
                            },
                        );
                    }
                    PendingUsbOperation::Restore {
                        restore_image,
                        backup_file,
                        confirmation,
                    } => {
                        ui.heading("Preflight");
                        if ui.button("Choose complete EEPROM image...").clicked() {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("EEPROM image", &["bin", "eeprom", "eep", "rom"])
                                .pick_file()
                            {
                                *restore_image = Some(preflight_restore_image(&path, spec));
                            }
                        }
                        if let Some(preflight) = restore_image.as_ref() {
                            ui.monospace(preflight.path.display().to_string());
                        } else {
                            ui.label("No restore image selected.");
                        }
                        match restore_image.as_ref().map(|preflight| &preflight.result) {
                            Some(Ok(image)) => {
                                ui.label(format!(
                                    "Image validation succeeded: {} model-bounded byte(s) will be restored. A separate fresh current-value backup is still required.",
                                    image.update_count
                                ));
                            }
                            Some(Err(error)) => {
                                ui.colored_label(Color32::from_rgb(181, 47, 47), error);
                            }
                            None => {}
                        }
                        choose_backup_file(ui, backup_file, "eeprom-restore-rollback.bin");
                        typed_confirmation(
                            ui,
                            confirmation,
                            EEPROM_RESTORE_CONFIRMATION,
                            "EEPROM restore",
                        );
                        let run = restore_image
                            .as_ref()
                            .is_some_and(|preflight| preflight.result.is_ok())
                            && backup_file.is_some()
                            && confirmation_matches(confirmation, EEPROM_RESTORE_CONFIRMATION);
                        dialog_actions(
                            ui,
                            &mut close_dialog,
                            run,
                            "Run confirmed EEPROM restore",
                            || {
                            let image = restore_image
                                .take()
                                .expect("enabled only with a selected restore image")
                                .result
                                .expect("enabled only with a valid restore image")
                                .bytes;
                            dispatch = Some(UsbOperationRequest::Restore {
                                image,
                                backup_file: backup_file
                                    .clone()
                                    .expect("enabled only after backup selection"),
                            });
                            },
                        );
                    }
                    PendingUsbOperation::Reset {
                        target,
                        backup_file,
                        confirmation,
                    } => {
                        ui.heading("Preflight");
                        let updates = declared_counter_reset_updates(spec, *target);
                        match &updates {
                            Ok(updates) => {
                                ui.label(format!(
                                    "Declared {} bytes only: {}.",
                                    target.display_name(),
                                    format_updates(updates)
                                ));
                            }
                            Err(error) => {
                                ui.colored_label(Color32::from_rgb(181, 47, 47), error);
                            }
                        }
                        ui.label(
                            "The worker captures the current values for these bytes from a fresh full backup before applying this semantic reset.",
                        );
                        choose_backup_file(ui, backup_file, "eeprom-reset-backup.bin");
                        typed_confirmation(
                            ui,
                            confirmation,
                            EEPROM_COUNTER_RESET_CONFIRMATION,
                            "declared counter reset",
                        );
                        let run = updates.is_ok()
                            && backup_file.is_some()
                            && confirmation_matches(
                                confirmation,
                                EEPROM_COUNTER_RESET_CONFIRMATION,
                            );
                        let action_label = target.display_name();
                        dialog_actions(
                            ui,
                            &mut close_dialog,
                            run,
                            &format!("Run confirmed {action_label}"),
                            || {
                                dispatch = Some(UsbOperationRequest::Reset {
                                    target: *target,
                                    backup_file: backup_file
                                        .clone()
                                        .expect("enabled only after backup selection"),
                                });
                            },
                        );
                    }
                }
            });
        if close_dialog {
            open = false;
        }
        if let Some(request) = dispatch {
            self.start_selected_usb_operation(request);
        } else if open {
            self.pending_usb_operation = Some(pending);
        }
    }

    fn debug_traffic_pane(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("debug-traffic-scroll")
            .max_height(ui.available_height())
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Debug traffic");
                    if self.debug_traffic.count() > 0 && ui.button("Clear").clicked() {
                        self.debug_traffic.clear();
                    }
                });
                let mut capture_enabled = self.debug_traffic.capture_enabled();
                if ui
                    .checkbox(
                        &mut capture_enabled,
                        "Capture traffic for this session (opt-in)",
                    )
                    .changed()
                {
                    self.debug_traffic.set_capture_enabled(capture_enabled);
                }
                ui.label(
                    "Selecting a candidate alone produces no traffic. Transfers are recorded only for an explicit selected-printer operation that starts while this opt-in is enabled; the GUI never exports them.",
                );
                ui.add_space(6.0);
                if self.debug_traffic.count() == 0 {
                    ui.label("No traffic captured in this session.");
                } else {
                    ui.strong("Recorded requests and responses");
                    for entry in self.debug_traffic.entries() {
                        egui::CollapsingHeader::new(entry.summary())
                            .id_salt(("debug-traffic-entry", entry.id()))
                            .default_open(false)
                            .show(ui, |ui| {
                                ui.indent(("debug-traffic-entry-bytes", entry.id()), |ui| {
                                    let bytes = if entry.hex_bytes().is_empty() {
                                        "<empty>"
                                    } else {
                                        entry.hex_bytes()
                                    };
                                    ui.monospace(format!("bytes={bytes}"));
                                });
                            });
                    }
                }
            });
    }

    fn primary_content_pane(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("primary-content-scroll")
            .max_height(ui.available_height())
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(error) = &self.file_error {
                    ui.colored_label(Color32::from_rgb(181, 47, 47), error);
                    ui.add_space(6.0);
                }
                #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
                if let Some(report) = &self.usb_operation_result {
                    let color = if report.success {
                        Color32::from_rgb(36, 130, 76)
                    } else {
                        Color32::from_rgb(181, 47, 47)
                    };
                    ui.colored_label(color, &report.headline);
                    for line in &report.lines {
                        ui.label(line);
                    }
                    ui.add_space(6.0);
                }
                match self.state.page() {
                    Page::Status => self.status(ui),
                    Page::Eeprom => self.eeprom_viewer(ui),
                    Page::Tools => self.tools(ui),
                }
            });
    }

    fn render_three_pane_layout(&mut self, ui: &mut egui::Ui) {
        let outline = egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
        let pane_frame = |vertical_margin| {
            egui::Frame::default()
                .stroke(outline)
                .inner_margin(egui::Margin::same(8))
                .outer_margin(egui::Margin::symmetric(8, vertical_margin))
        };

        egui::Panel::top("tab-strip")
            .exact_size(44.0)
            .show_separator_line(false)
            .frame(
                egui::Frame::default()
                    .stroke(outline)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .outer_margin(egui::Margin::symmetric(8, 4)),
            )
            .show(ui, |ui| self.tab_strip_pane(ui));
        let debug_response = egui::Panel::bottom("debug-traffic")
            .exact_size(self.debug_height)
            .resizable(false)
            .show_separator_line(false)
            .frame(pane_frame(8))
            .show(ui, |ui| self.debug_traffic_pane(ui));
        let grip = egui::Rect::from_center_size(
            egui::pos2(
                debug_response.response.rect.center().x,
                debug_response.response.rect.top() + 4.0,
            ),
            egui::vec2(48.0, 3.0),
        );
        let response = ui
            .interact(
                grip,
                ui.id().with("debug-resize-handle"),
                egui::Sense::drag(),
            )
            .on_hover_cursor(egui::CursorIcon::ResizeVertical);
        if response.dragged() {
            let pointer_delta = ui.ctx().input(|input| input.pointer.delta().y);
            self.debug_height = (self.debug_height - pointer_delta).clamp(100.0, 420.0);
        }
        let grip_color = if response.hovered() || response.dragged() {
            ui.visuals().widgets.active.bg_stroke.color
        } else {
            ui.visuals().widgets.noninteractive.bg_stroke.color
        };
        ui.painter().rect_filled(grip, 1.0, grip_color);

        pane_frame(4).show(ui, |ui| self.primary_content_pane(ui));
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn selected_model_for_usb_candidate<'a>(
    state: &GuiState,
    candidate: Option<&DescriptorCandidate>,
    selected_model: Option<&'a str>,
) -> Option<&'a str> {
    candidate?;
    selected_model.filter(|model| state.model_spec(model).is_some())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn expected_restore_image_length(spec: &EpsonSpec) -> usize {
    usize::from(spec.memory_high).saturating_sub(usize::from(spec.memory_low)) + 1
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn validate_restore_image_size(spec: &EpsonSpec, actual_size: u64) -> Result<usize, String> {
    let expected_length = expected_restore_image_length(spec);
    if actual_size != expected_length as u64 {
        return Err(format!(
            "EEPROM restore image has {actual_size} bytes; model {} requires exactly {expected_length} bytes for {:#06x}..={:#06x}",
            spec.model, spec.memory_low, spec.memory_high
        ));
    }
    Ok(expected_length)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn preflight_restore_image(path: &std::path::Path, spec: &EpsonSpec) -> RestoreImagePreflight {
    let result = (|| {
        let metadata = std::fs::metadata(path).map_err(|error| {
            format!(
                "Could not stat selected restore image {}: {error}",
                path.display()
            )
        })?;
        if !metadata.is_file() {
            return Err(format!(
                "Selected restore image {} is not a regular file",
                path.display()
            ));
        }
        let expected_length = validate_restore_image_size(spec, metadata.len())?;
        let file = File::open(path).map_err(|error| {
            format!(
                "Could not open selected restore image {}: {error}",
                path.display()
            )
        })?;
        let opened_metadata = file.metadata().map_err(|error| {
            format!(
                "Could not stat selected restore image {} after opening it: {error}",
                path.display()
            )
        })?;
        if !opened_metadata.is_file() {
            return Err(format!(
                "Selected restore image {} changed to a non-regular file",
                path.display()
            ));
        }
        validate_restore_image_size(spec, opened_metadata.len())?;

        let mut bytes = Vec::with_capacity(expected_length);
        let read_limit = (expected_length as u64)
            .checked_add(1)
            .expect("EEPROM image length is bounded by u16 addresses");
        file.take(read_limit)
            .read_to_end(&mut bytes)
            .map_err(|error| {
                format!(
                    "Could not read selected restore image {}: {error}",
                    path.display()
                )
            })?;
        let update_count = restore_eeprom_updates(spec, &bytes)?.len();
        Ok(ValidatedRestoreImage {
            bytes,
            update_count,
        })
    })();
    RestoreImagePreflight {
        path: path.to_path_buf(),
        result,
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn run_selected_usb_operation(
    candidate: DescriptorCandidate,
    expected_model: String,
    request: UsbOperationRequest,
    record_traffic: bool,
) -> UsbOperationOutcome {
    let database = match ModelDatabase::builtin() {
        Ok(database) => database,
        Err(error) => {
            return UsbOperationOutcome {
                operation: Err(format!("model database load failed: {error}")),
                cleanup: UsbSessionCleanup::not_attempted(),
                events: Vec::new(),
            };
        }
    };
    let spec = match database.get(&expected_model).cloned() {
        Some(spec) => spec,
        None => {
            return UsbOperationOutcome {
                operation: Err(format!("unknown selected expected model: {expected_model}")),
                cleanup: UsbSessionCleanup::not_attempted(),
                events: Vec::new(),
            };
        }
    };
    enum PreparedRequest {
        Status,
        Dump(PathBuf),
        Mutation {
            action: &'static str,
            updates: Vec<(u16, u8)>,
            backup_file: PathBuf,
        },
    }
    let request = match request {
        UsbOperationRequest::Status => PreparedRequest::Status,
        UsbOperationRequest::Dump { output_file } => PreparedRequest::Dump(output_file),
        UsbOperationRequest::Write {
            updates,
            backup_file,
        } => PreparedRequest::Mutation {
            action: "write",
            updates,
            backup_file,
        },
        UsbOperationRequest::Restore { image, backup_file } => {
            match restore_eeprom_updates(&spec, &image) {
                Ok(updates) => PreparedRequest::Mutation {
                    action: "restore",
                    updates,
                    backup_file,
                },
                Err(error) => {
                    return UsbOperationOutcome {
                        operation: Err(error),
                        cleanup: UsbSessionCleanup::not_attempted(),
                        events: Vec::new(),
                    };
                }
            }
        }
        UsbOperationRequest::Reset {
            target,
            backup_file,
        } => match declared_counter_reset_updates(&spec, target) {
            Ok(updates) => PreparedRequest::Mutation {
                action: target.display_name(),
                updates,
                backup_file,
            },
            Err(error) => {
                return UsbOperationOutcome {
                    operation: Err(error),
                    cleanup: UsbSessionCleanup::not_attempted(),
                    events: Vec::new(),
                };
            }
        },
    };
    let device = reink_usb::UsbDeviceSelector::at_location(
        candidate.vendor_id,
        candidate.product_id,
        candidate.bus_number,
        candidate.device_address,
    );
    let interface = reink_platform::UsbInterfaceSelector {
        number: candidate.interface_number,
        alternate_setting: candidate.alternate_setting,
    };
    let outcome = with_selected_usb_epson_session(
        device,
        interface,
        spec,
        record_traffic,
        move |session| {
            let identity = session.read_identity().map_err(|error| error.to_string())?;
            verify_exact_model(&identity, &expected_model)?;
            match request {
                PreparedRequest::Status => {
                    let response = session.read_status().map_err(|error| error.to_string())?;
                    Ok(UsbOperationSuccess::Status {
                        model: expected_model,
                        response_bytes: response.len(),
                        display: display_status_response(&response),
                    })
                }
                PreparedRequest::Dump(output_file) => {
                    let image = session.dump_eeprom().map_err(|error| error.to_string())?;
                    write_new_binary_file(&output_file, &image.bytes, "EEPROM image")?;
                    Ok(UsbOperationSuccess::Dump { image, output_file })
                }
                PreparedRequest::Mutation {
                    action,
                    updates,
                    backup_file,
                } => {
                    let plan = session
                        .prepare_eeprom_write(&updates)
                        .map_err(|error| format!("could not prepare EEPROM {action}: {error}"))?;
                    let current_values = plan
                        .updates
                        .iter()
                        .map(|&(address, _)| {
                            plan.backup
                                .value_at(address)
                                .map(|value| (address, value))
                                .ok_or_else(|| {
                                    format!("complete backup did not contain {address:#06x}")
                                })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let current_description = format_current_values(&current_values);
                    write_new_binary_file(&backup_file, &plan.backup.bytes, "EEPROM backup")
                        .map_err(|error| {
                            format!(
                                "{error}; pre-write current values captured from the complete backup: {current_description}"
                            )
                        })?;
                    session.apply_eeprom_write(&plan).map_err(|error| {
                        format!(
                            "EEPROM {action} failed after capturing pre-write current values ({current_description}): {error}"
                        )
                    })?;
                    Ok(UsbOperationSuccess::Mutation {
                        action,
                        model: expected_model,
                        backup_file,
                        current_values,
                        update_count: plan.updates.len(),
                    })
                }
            }
        },
    );
    UsbOperationOutcome {
        operation: outcome.operation,
        cleanup: outcome.cleanup,
        events: outcome.events,
    }
}

impl eframe::App for ReinkGui {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        self.poll_usb_candidates();
        self.poll_usb_operation();
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        if matches!(&self.usb_scan_status, UsbScanStatus::Scanning)
            || self.usb_operation_in_progress()
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.render_three_pane_layout(ui);
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        self.selected_operation_dialog(ui.ctx());
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn choose_backup_file(ui: &mut egui::Ui, backup_file: &mut Option<PathBuf>, suggested_name: &str) {
    if ui.button("Choose new complete backup...").clicked() {
        if let Some(path) = rfd::FileDialog::new()
            .set_file_name(suggested_name)
            .add_filter("EEPROM image", &["bin", "eeprom", "eep", "rom"])
            .save_file()
        {
            *backup_file = Some(path);
        }
    }
    match backup_file {
        Some(path) => {
            ui.monospace(format!("New backup path: {}", path.display()));
            ui.label("The worker uses create-new semantics and rejects an existing path.");
        }
        None => {
            ui.colored_label(
                Color32::from_rgb(174, 112, 0),
                "Choose a new complete backup path before this persistent operation can run.",
            );
        }
    };
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn typed_confirmation(ui: &mut egui::Ui, confirmation: &mut String, required: &str, action: &str) {
    ui.label(format!("Type this exact confirmation for the {action}:"));
    ui.monospace(required);
    ui.add(egui::TextEdit::singleline(confirmation).desired_width(430.0));
    if !confirmation.is_empty() && !confirmation_matches(confirmation, required) {
        ui.colored_label(
            Color32::from_rgb(181, 47, 47),
            "Confirmation does not exactly match.",
        );
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn confirmation_matches(provided: &str, required: &str) -> bool {
    provided == required
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn dialog_actions(
    ui: &mut egui::Ui,
    open: &mut bool,
    enabled: bool,
    label: &str,
    run: impl FnOnce(),
) {
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Cancel").clicked() {
            *open = false;
        }
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            run();
            *open = false;
        }
    });
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn parse_u16_input(value: &str) -> Result<u16, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Enter an EEPROM address.".to_owned());
    }
    if let Some(value) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u16::from_str_radix(value, 16).map_err(|_| "Invalid hexadecimal EEPROM address.".to_owned())
    } else {
        value
            .parse::<u16>()
            .map_err(|_| "Invalid decimal EEPROM address.".to_owned())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn parse_u8_input(value: &str) -> Result<u8, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Enter an EEPROM byte value.".to_owned());
    }
    if let Some(value) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u8::from_str_radix(value, 16)
            .map_err(|_| "Invalid hexadecimal EEPROM byte value.".to_owned())
    } else {
        value
            .parse::<u8>()
            .map_err(|_| "Invalid decimal EEPROM byte value.".to_owned())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn display_status_response(bytes: &[u8]) -> String {
    const MAX_STATUS_CHARACTERS: usize = 1_024;
    match std::str::from_utf8(bytes) {
        Ok(status) => {
            let trimmed = status.trim();
            let mut display = trimmed
                .chars()
                .take(MAX_STATUS_CHARACTERS)
                .collect::<String>();
            if trimmed.chars().count() > MAX_STATUS_CHARACTERS {
                display.push_str("… (truncated)");
            }
            display
        }
        Err(_) => "Binary status response is not shown.".to_owned(),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn format_updates(updates: &[(u16, u8)]) -> String {
    format_current_values(updates)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn format_current_values(values: &[(u16, u8)]) -> String {
    const DISPLAY_LIMIT: usize = 16;
    let visible = values
        .iter()
        .take(DISPLAY_LIMIT)
        .map(|(address, value)| format!("0x{address:04X}=0x{value:02X}"))
        .collect::<Vec<_>>()
        .join(", ");
    if values.len() > DISPLAY_LIMIT {
        format!("{visible}, … ({} byte(s) total)", values.len())
    } else if values.is_empty() {
        "none".to_owned()
    } else {
        visible
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn cleanup_is_successful(cleanup: &UsbSessionCleanup) -> bool {
    matches!(&cleanup.d4_shutdown, UsbCleanupStatus::Succeeded)
        && matches!(&cleanup.usb_close, UsbCleanupStatus::Succeeded)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn cleanup_report_lines(cleanup: &UsbSessionCleanup) -> Vec<String> {
    vec![
        format!(
            "D4 shutdown: {}.",
            cleanup_status_label(&cleanup.d4_shutdown)
        ),
        format!("USB cleanup: {}.", cleanup_status_label(&cleanup.usb_close)),
    ]
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn cleanup_status_label(status: &UsbCleanupStatus) -> String {
    match status {
        UsbCleanupStatus::NotAttempted => "not attempted".to_owned(),
        UsbCleanupStatus::Succeeded => "succeeded".to_owned(),
        UsbCleanupStatus::Failed(error) => format!("failed ({error})"),
    }
}

fn eeprom_image_offset(start_address: usize, byte_count: usize, address: usize) -> Option<usize> {
    address
        .checked_sub(start_address)
        .filter(|offset| *offset < byte_count)
}

fn eeprom_field_address_label(field: &EepromFileField) -> String {
    if field.address == field.end_address {
        format!("0x{:04X}", field.address)
    } else {
        format!("0x{:04X}..0x{:04X}", field.address, field.end_address)
    }
}

fn eeprom_field_value(field: &EepromFileField, image_start_address: usize, bytes: &[u8]) -> String {
    if field.sensitive {
        return "Hidden (sensitive)".to_owned();
    }
    let Some(field_bytes) = eeprom_field_bytes(field, image_start_address, bytes) else {
        return "Unavailable".to_owned();
    };

    match field.encoding {
        EepromFieldEncoding::U8 => {
            let value = field_bytes[0];
            format!("{value} (0x{value:02X})")
        }
        EepromFieldEncoding::U16Le => {
            let value = u16::from_le_bytes([field_bytes[0], field_bytes[1]]);
            format!("{value} (0x{value:04X})")
        }
        EepromFieldEncoding::U32Le => {
            let value = u32::from_le_bytes([
                field_bytes[0],
                field_bytes[1],
                field_bytes[2],
                field_bytes[3],
            ]);
            format!("{value} (0x{value:08X})")
        }
        EepromFieldEncoding::Ascii => {
            let value = field_bytes
                .iter()
                .map(|byte| match byte {
                    b' '..=b'~' => char::from(*byte),
                    _ => '.',
                })
                .collect::<String>();
            format!("{value:?}")
        }
        EepromFieldEncoding::RawBytes => field_bytes
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn eeprom_field_bytes<'a>(
    field: &EepromFileField,
    image_start_address: usize,
    bytes: &'a [u8],
) -> Option<&'a [u8]> {
    let offset = eeprom_image_offset(image_start_address, bytes.len(), field.address)?;
    let byte_len = field
        .end_address
        .checked_sub(field.address)?
        .checked_add(1)?;
    bytes.get(offset..offset.checked_add(byte_len)?)
}

fn eeprom_field_tooltip(field: &EepromFileField) -> String {
    let mut lines = vec![
        format!("Read-only field: {}", eeprom_field_address_label(field)),
        format!("Encoding: {}", field.encoding.label()),
    ];
    if let Some(confidence) = field.confidence {
        lines.push(format!("Confidence: {}", confidence.label()));
    }
    if let Some(note) = &field.evidence_note {
        lines.push(format!("Evidence: {note}"));
    }
    if field.sensitive {
        lines.push("Decoded value is hidden by default because it is sensitive.".to_owned());
    }
    lines.join("\n")
}

fn eeprom_table_row(
    ui: &mut egui::Ui,
    address_width: f32,
    field_width: f32,
    value_width: f32,
    address: &str,
    field: &str,
    value: &str,
    selected: bool,
    header: bool,
    height: f32,
) -> egui::Response {
    let (address_alignment, field_alignment, value_alignment) = if header {
        (
            egui::Align::Center,
            egui::Align::Center,
            egui::Align::Center,
        )
    } else {
        (egui::Align::Min, egui::Align::Min, egui::Align::Max)
    };
    let separator_width = 8.0;
    let row_width = address_width + field_width + value_width + 2.0 * separator_width;
    let sense = if header {
        egui::Sense::hover()
    } else {
        egui::Sense::click()
    };
    let (row_rect, response) = ui.allocate_exact_size(egui::vec2(row_width, height), sense);
    let address_rect =
        egui::Rect::from_min_size(row_rect.min, egui::vec2(address_width, row_rect.height()));
    let first_separator_x = address_rect.right() + separator_width / 2.0;
    let field_rect = egui::Rect::from_min_size(
        egui::pos2(address_rect.right() + separator_width, row_rect.top()),
        egui::vec2(field_width, row_rect.height()),
    );
    let second_separator_x = field_rect.right() + separator_width / 2.0;
    let value_rect = egui::Rect::from_min_size(
        egui::pos2(field_rect.right() + separator_width, row_rect.top()),
        egui::vec2(value_width, row_rect.height()),
    );
    let stroke = egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
    paint_dashed_vertical_line(ui, first_separator_x, row_rect, stroke);
    paint_dashed_vertical_line(ui, second_separator_x, row_rect, stroke);
    paint_eeprom_cell(
        ui,
        address_rect,
        address,
        selected,
        address_alignment,
        header,
    );
    paint_eeprom_cell(ui, field_rect, field, selected, field_alignment, header);
    paint_eeprom_cell(ui, value_rect, value, selected, value_alignment, header);
    response
}

fn paint_eeprom_cell(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    value: &str,
    selected: bool,
    alignment: egui::Align,
    header: bool,
) {
    let font = egui::FontId::monospace(14.0);
    let color = if selected {
        Color32::WHITE
    } else {
        ui.visuals().text_color()
    };
    let horizontal_padding = 6.0;
    let lines = if header {
        vec![value.to_owned()]
    } else {
        wrap_eeprom_cell_text(ui, value, &font, rect.width() - 2.0 * horizontal_padding)
    };
    let line_height = ui.fonts_mut(|fonts| fonts.row_height(&font));
    let text_height = line_height * lines.len() as f32;
    let mut y = rect.center().y - text_height / 2.0;

    if selected {
        ui.painter()
            .rect_filled(rect, 0.0, Color32::from_rgb(52, 92, 118));
    }
    for line in lines {
        let galley = ui.painter().layout_no_wrap(line, font.clone(), color);
        let x = match alignment {
            egui::Align::Min => rect.left() + horizontal_padding,
            egui::Align::Center => rect.center().x - galley.size().x / 2.0,
            egui::Align::Max => rect.right() - horizontal_padding - galley.size().x,
        };
        let position = egui::pos2(x, y);
        ui.painter().galley(position, galley, color);
        y += line_height;
    }
}

fn eeprom_field_column_width(
    ui: &egui::Ui,
    rows: &[reink_gui::EepromRow],
    file_fields: &[EepromFileField],
    selected_is_file: bool,
    selected_label: &str,
) -> f32 {
    let labels = if selected_is_file && file_fields.is_empty() {
        vec![selected_label]
    } else if selected_is_file {
        file_fields
            .iter()
            .map(|field| field.label.as_str())
            .collect()
    } else {
        rows.iter().map(|row| row.label).collect()
    };
    let font = egui::FontId::monospace(14.0);
    labels
        .into_iter()
        .map(|label| {
            ui.painter()
                .layout_no_wrap(label.to_owned(), font.clone(), ui.visuals().text_color())
                .size()
                .x
        })
        .fold(160.0, f32::max)
        + 12.0
}

fn wrap_eeprom_cell_text(
    ui: &egui::Ui,
    value: &str,
    font: &egui::FontId,
    max_width: f32,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in value.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_owned()
        } else {
            format!("{line} {word}")
        };
        if !line.is_empty()
            && ui
                .painter()
                .layout_no_wrap(candidate.clone(), font.clone(), ui.visuals().text_color())
                .size()
                .x
                > max_width
        {
            lines.push(std::mem::take(&mut line));
            line.push_str(word);
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn paint_dashed_vertical_line(ui: &egui::Ui, x: f32, rect: egui::Rect, stroke: egui::Stroke) {
    let mut y = rect.top() + 2.0;
    while y < rect.bottom() - 2.0 {
        let end = (y + 3.0).min(rect.bottom() - 2.0);
        ui.painter()
            .line_segment([egui::pos2(x, y), egui::pos2(x, end)], stroke);
        y += 6.0;
    }
}

fn end_truncate(value: &str, maximum_characters: usize) -> String {
    if value.chars().count() <= maximum_characters {
        return value.to_owned();
    }
    let suffix = value
        .chars()
        .rev()
        .take(maximum_characters.saturating_sub(1))
        .collect::<Vec<_>>();
    format!("…{}", suffix.into_iter().rev().collect::<String>())
}

fn dump_cell(value: String, highlighted: bool) -> RichText {
    let text = RichText::new(value).monospace();
    if highlighted {
        text.background_color(Color32::from_rgb(239, 220, 130))
    } else {
        text
    }
}

fn ascii_cell(byte: u8) -> char {
    match byte {
        b' '..=b'~' => byte as char,
        _ => '.',
    }
}

fn ascii_dump_line(
    bytes: &[u8],
    line_address: usize,
    selected_address: usize,
) -> egui::text::LayoutJob {
    let mut line = egui::text::LayoutJob::default();
    for (index, byte) in bytes.iter().enumerate() {
        let highlighted = line_address + index == selected_address;
        line.append(
            &ascii_cell(*byte).to_string(),
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::monospace(14.0),
                background: highlighted
                    .then_some(Color32::from_rgb(239, 220, 130))
                    .unwrap_or_default(),
                ..Default::default()
            },
        );
    }
    line
}

fn status_color(status: ValidationStatus) -> Color32 {
    match status {
        ValidationStatus::Success => Color32::from_rgb(36, 130, 76),
        ValidationStatus::Blocked => Color32::from_rgb(174, 112, 0),
        ValidationStatus::Failure => Color32::from_rgb(181, 47, 47),
    }
}

#[cfg(test)]
mod tests {
    use reink_core::{EepromFieldConfidence, EepromFieldEncoding};
    use reink_gui::SourceMode;

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
    use reink_gui::{DescriptorCandidate, GuiState};

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

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[test]
    fn unassociated_usb_candidate_accepts_only_a_known_expected_model() {
        let state = GuiState::new().unwrap();
        let candidate = DescriptorCandidate {
            alias: "usb-1".to_owned(),
            vendor_id: 0x04b8,
            product_id: 0x0001,
            bus_number: 1,
            device_address: 1,
            interface_number: 0,
            alternate_setting: 0,
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
}
