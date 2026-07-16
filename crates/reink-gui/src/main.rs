#![forbid(unsafe_code)]

use eframe::egui::{self, Color32, RichText};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use reink_app::{EepromImage, EpsonD4Session};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use reink_core::ModelDatabase;
use reink_gui::{
    DebugTrafficTrace, DescriptorCandidate, GuiState, Page, SourceMode, ValidationStatus,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use reink_platform::RecordingTransport;
use reink_platform::TransportEvent;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::sync::mpsc::{Receiver, TryRecvError};

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
    usb_scan_status: UsbScanStatus,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    usb_scan_receiver: Option<Receiver<Result<Vec<reink_usb::UsbPrinterCandidate>, String>>>,
    file_error: Option<String>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    usb_dump_receiver: Option<Receiver<UsbEepromDumpOutcome>>,
    usb_dump_error: Option<String>,
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
    label: String,
}

enum UsbScanStatus {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    Scanning,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    Ready,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    Failed(String),
    Unavailable,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
struct UsbEepromDumpOutcome {
    result: Result<EepromImage, String>,
    events: Vec<TransportEvent>,
}

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
            usb_scan_status: UsbScanStatus::Unavailable,
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            usb_scan_receiver: None,
            file_error: None,
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            usb_dump_receiver: None,
            usb_dump_error: None,
            debug_traffic: DebugTrafficTrace::new(),
            debug_height: 180.0,
        };
        gui.refresh_usb_candidates();
        gui
    }

    /// Adds the ordered result of a future read-only `RecordingTransport` session.
    ///
    /// This is intentionally a session-only integration seam: the trace model
    /// discards these events until the user has explicitly enabled capture.
    pub fn append_recorded_transport_events(&mut self, events: Vec<TransportEvent>) -> usize {
        self.debug_traffic.append_events(events)
    }

    fn selected_usb_candidate(&self) -> Option<&DescriptorCandidate> {
        self.selected_usb_candidate
            .and_then(|index| self.usb_candidates.get(index))
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
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            UsbScanStatus::Scanning => "Scanning USB printer descriptors…".to_owned(),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            UsbScanStatus::Ready => format!(
                "{} USB descriptor candidate(s) found",
                self.usb_candidates.len()
            ),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
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

        spec.memory_operations
            .iter()
            .flat_map(|operation| {
                operation.addresses.iter().map(|address| EepromFileField {
                    address: *address as usize,
                    label: operation.description.clone(),
                })
            })
            .collect()
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
                self.usb_dump_error = None;
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
                self.selected_fixture = None;
                self.file_error = None;
                self.usb_dump_error = None;
                self.show_validation_report = false;
                self.state.navigate_to(Page::Eeprom);
            }
            Err(error) => {
                self.file_error = Some(format!("Unable to open EEPROM file: {error}"));
                self.usb_dump_error = None;
            }
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn refresh_usb_candidates(&mut self) {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        self.usb_scan_receiver = Some(receiver);
        self.usb_scan_status = UsbScanStatus::Scanning;
        std::thread::spawn(move || {
            let result = reink_usb::list_printer_candidates().map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn refresh_usb_candidates(&mut self) {
        self.usb_scan_status = UsbScanStatus::Unavailable;
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
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
                self.usb_scan_status = UsbScanStatus::Ready;
            }
            Err(error) => self.usb_scan_status = UsbScanStatus::Failed(error),
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn poll_usb_candidates(&mut self) {}

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn usb_dump_in_progress(&self) -> bool {
        self.usb_dump_receiver.is_some()
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn poll_usb_eeprom_dump(&mut self) {
        let result =
            self.usb_dump_receiver
                .as_ref()
                .and_then(|receiver| match receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => Some(UsbEepromDumpOutcome {
                        result: Err(
                            "The selected-printer EEPROM dump stopped unexpectedly.".to_owned()
                        ),
                        events: Vec::new(),
                    }),
                });
        let Some(result) = result else {
            return;
        };

        self.usb_dump_receiver = None;
        let UsbEepromDumpOutcome { result, events } = result;
        if !events.is_empty() {
            self.append_recorded_transport_events(events);
        }
        match result {
            Ok(image) => {
                self.loaded_eeprom = Some(LoadedEeprom {
                    path: format!("USB dump: {}", image.model),
                    bytes: image.bytes,
                    start_address: usize::from(image.start_address),
                    selected_offset: 0,
                    model: Some(image.model),
                });
                self.selected_usb_candidate = None;
                self.selected_fixture = None;
                self.file_error = None;
                self.usb_dump_error = None;
                self.show_validation_report = false;
                self.state.navigate_to(Page::Eeprom);
            }
            Err(error) => self.usb_dump_error = Some(error),
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn poll_usb_eeprom_dump(&mut self) {}

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn start_selected_usb_eeprom_dump(&mut self) {
        if self.usb_dump_receiver.is_some() {
            return;
        }
        let Some(candidate) = self.selected_usb_candidate().cloned() else {
            return;
        };
        if candidate.model_hints.len() != 1 {
            self.usb_dump_error = Some(
                "Selected printer candidate must resolve to exactly one model hint before dumping EEPROM.".to_owned(),
            );
            return;
        }
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        self.usb_dump_receiver = Some(receiver);
        self.usb_dump_error = None;
        std::thread::spawn(move || {
            let outcome = dump_selected_usb_eeprom(candidate);
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
                        let mut open_file = false;
                        egui::ComboBox::from_id_salt("fixture-device")
                            .selected_text(source_label)
                            .width(320.0)
                            .show_ui(ui, |ui| {
                                ui.strong("USB descriptor candidates");
                                if self.usb_candidates.is_empty() {
                                    #[cfg(any(target_os = "linux", target_os = "macos"))]
                                    ui.label(match &self.usb_scan_status {
                                        UsbScanStatus::Scanning => "Scanning…",
                                        _ => "No USB descriptor candidates",
                                    });
                                    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
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
                                        self.loaded_eeprom = None;
                                        self.selected_fixture = None;
                                        self.show_validation_report = false;
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
                                            self.selected_fixture = Some(index);
                                            self.state.select_fixture(index);
                                            self.show_validation_report = false;
                                        }
                                    }
                                }
                                ui.separator();
                                if ui.button("Open EEPROM file...").clicked() {
                                    open_file = true;
                                }
                            });
                        if open_file {
                            self.open_eeprom_file();
                        }
                        ui.label("Printer");
                        #[cfg(any(target_os = "linux", target_os = "macos"))]
                        if ui
                            .add_enabled(
                                !matches!(&self.usb_scan_status, UsbScanStatus::Scanning),
                                egui::Button::new("Refresh USB candidates"),
                            )
                            .clicked()
                        {
                            self.refresh_usb_candidates();
                        }
                        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
                        ui.add_enabled(false, egui::Button::new("Refresh USB candidates"));
                        ui.label(self.usb_scan_status_label());

                        if self.loaded_eeprom.is_some() {
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
                            egui::ComboBox::from_id_salt("eeprom-model")
                                .selected_text(
                                    selected_model.as_deref().unwrap_or("Select model..."),
                                )
                                .width(180.0)
                                .show_ui(ui, |ui| {
                                    for model in &model_names {
                                        if ui
                                            .selectable_label(
                                                selected_model.as_deref() == Some(model.as_str()),
                                                model,
                                            )
                                            .clicked()
                                        {
                                            selected_model = Some(model.clone());
                                        }
                                    }
                                });
                            if selected_model != current_model {
                                if let Some(file) = &mut self.loaded_eeprom {
                                    file.model = selected_model;
                                }
                            }
                            ui.label("Model");
                        }
                    },
                );
            },
        );
    }

    fn status(&mut self, ui: &mut egui::Ui) {
        ui.heading("Printer status");
        if let Some(candidate) = self.selected_usb_candidate() {
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
                        "No exact VID/PID matches in the bundled model database".to_owned()
                    } else {
                        candidate.model_hints.join(", ")
                    });
                    ui.end_row();
                });
            ui.add_space(8.0);
            ui.label("Hints are not identity confirmation.");
            ui.strong("No printer connection has been opened");
            ui.label("Identity/EEPROM reads require a future explicit read-only operation.");
            if candidate.model_hints.len() == 1 {
                ui.label(format!("Resolved model hint: {}", candidate.model_hints[0]));
                #[cfg(any(target_os = "linux", target_os = "macos"))]
                {
                    if self.usb_dump_in_progress() {
                        ui.label("Dumping selected printer EEPROM…");
                    } else if ui.button("Dump selected printer EEPROM").clicked() {
                        self.start_selected_usb_eeprom_dump();
                    }
                }
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
        if self.selected_usb_candidate().is_some() {
            ui.label("No EEPROM data is available for the selected descriptor-only candidate.");
            ui.label(
                "No printer connection has been opened; a future explicit read-only operation is required before identity or EEPROM reads.",
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
            ui.label("The selected raw EEPROM image is shown below. Choose a model in the tab bar to label known model-specific fields.");
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
        let value_width = 60.0;
        let field_panel_width = address_width + field_column_width + value_width + 32.0;
        let dump_panel_width = 600.0;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(field_panel_width, 430.0),
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
                                    if field.address >= bytes.len() {
                                        continue;
                                    }
                                    ui.separator();
                                    let response = eeprom_table_row(
                                        ui,
                                        address_width,
                                        field_column_width,
                                        value_width,
                                        &format!("0x{:04X}", field.address),
                                        &field.label,
                                        &format!("0x{:02X}", bytes[field.address]),
                                        field.address == selected_address,
                                        false,
                                        42.0,
                                    );
                                    if response {
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
                                    if response {
                                        self.state.select_eeprom_row(index);
                                    }
                                }
                            }
                        });
                },
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(dump_panel_width, 430.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::default()
                        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
                        .inner_margin(egui::Margin::same(4))
                        .show(ui, |ui| {
                            ui.heading("Hex dump");
                            egui::ScrollArea::both()
                                .id_salt("fixture-eeprom-dump")
                                .max_height(358.0)
                                .show(ui, |ui| {
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
                                                            selected_address_from_dump =
                                                                Some(address);
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
                        });
                },
            );
        });
        if let Some(address) = selected_address_from_dump {
            self.select_eeprom_address(address);
        }
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            let mut direct_editing = false;
            ui.add_enabled(
                false,
                egui::Checkbox::new(&mut direct_editing, "Enable direct EEPROM editing"),
            );
            ui.label("Unavailable until hardware evidence and safety review authorize it.");
        });
        ui.add_enabled_ui(false, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Selected field: {} at 0x{selected_address:04X}",
                    selected_label
                ));
                let mut value = format!("0x{selected_value:02X}");
                ui.add(egui::TextEdit::singleline(&mut value).desired_width(80.0));
            });
        });
    }

    fn tools(&mut self, ui: &mut egui::Ui) {
        ui.heading("Tools");
        if self.selected_usb_candidate().is_some() {
            ui.label("Connected operations are unavailable for this descriptor-only candidate.");
            ui.label(
                "No printer connection has been opened, and fixture validation cannot run against a USB candidate.",
            );
            ui.add_enabled(false, egui::Button::new("Run validation"));
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
            ui.label("Reset is unavailable: no physical write or reset path is linked.");
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
                "Before the first persistent write to a connected printer, ReInk will ask whether to save an EEPROM backup.",
            );
            ui.label(
                "A canceled save dialog will not count as declining the backup; continuing without one requires a separate explicit acknowledgement.",
            );
            ui.add_enabled(false, egui::Button::new("Choose EEPROM backup..."));
            ui.label("Unavailable until a hardware write path passes the separate safety review.");
        });
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
                    "Selecting a descriptor candidate alone produces no traffic. Only a future explicit connected read-only operation can append records.",
                );
                ui.add_space(6.0);
                if self.debug_traffic.count() == 0 {
                    ui.label("No traffic captured in this session.");
                } else {
                    ui.strong("Recorded transfers");
                    for entry in self.debug_traffic.entries() {
                        ui.monospace(format!(
                            "{}  {}",
                            entry.direction().label(),
                            entry.hex_bytes()
                        ));
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
                if let Some(error) = &self.usb_dump_error {
                    ui.colored_label(Color32::from_rgb(181, 47, 47), error);
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

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn dump_selected_usb_eeprom(candidate: DescriptorCandidate) -> UsbEepromDumpOutcome {
    let hint = candidate
        .model_hints
        .first()
        .cloned()
        .unwrap_or_else(|| "<missing model hint>".to_owned());
    let database = match ModelDatabase::builtin() {
        Ok(database) => database,
        Err(error) => {
            return UsbEepromDumpOutcome {
                result: Err(format!("model database load failed: {error}")),
                events: Vec::new(),
            };
        }
    };
    let spec = match database.get(&hint).cloned() {
        Some(spec) => spec,
        None => {
            return UsbEepromDumpOutcome {
                result: Err(format!("unknown EEPROM model hint: {hint}")),
                events: Vec::new(),
            };
        }
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
    let transport = match reink_usb::ReadOnlyUsbTransport::open(device, interface) {
        Ok(transport) => transport,
        Err(error) => {
            return UsbEepromDumpOutcome {
                result: Err(error.to_string()),
                events: Vec::new(),
            };
        }
    };

    let recording = RecordingTransport::new(transport);
    let mut session = match EpsonD4Session::connect_recoverable(recording, spec) {
        Ok(session) => session,
        Err((error, recording)) => {
            let (mut transport, events) = recording.into_parts();
            let close_error = transport.close().err().map(|error| error.to_string());
            return UsbEepromDumpOutcome {
                result: Err(match close_error {
                    Some(close) => format!(
                        "Epson D4 session setup failed: {error}; USB transport close failed: {close}"
                    ),
                    None => format!("Epson D4 session setup failed: {error}"),
                }),
                events,
            };
        }
    };

    let operation_result = (|| -> Result<EepromImage, String> {
        let identity = session.read_identity().map_err(|error| error.to_string())?;
        let identity_model = identity
            .detected_model()
            .ok_or_else(|| "printer identity did not report a model".to_owned())?;
        if identity_model != hint {
            return Err(format!(
                "selected model hint {hint:?} did not match printer identity {identity_model:?}"
            ));
        }
        session.dump_eeprom().map_err(|error| error.to_string())
    })();
    let shutdown_result = session.shutdown().map_err(|error| error.to_string());
    let recording = session.into_transport();
    let (mut transport, events) = recording.into_parts();
    let close_result = transport.close().map_err(|error| error.to_string());

    let result = match (operation_result, shutdown_result, close_result) {
        (Ok(image), Ok(()), Ok(())) => Ok(image),
        (Err(operation), Ok(()), Ok(())) => Err(operation),
        (Ok(_), Err(shutdown), Ok(())) => Err(format!("D4 shutdown failed: {shutdown}")),
        (Ok(_), Ok(()), Err(close)) => Err(format!("USB transport close failed: {close}")),
        (Err(operation), Err(shutdown), Ok(())) => {
            Err(format!("{operation}; D4 shutdown failed: {shutdown}"))
        }
        (Err(operation), Ok(()), Err(close)) => {
            Err(format!("{operation}; USB transport close failed: {close}"))
        }
        (Ok(_), Err(shutdown), Err(close)) => Err(format!(
            "D4 shutdown failed: {shutdown}; USB transport close failed: {close}"
        )),
        (Err(operation), Err(shutdown), Err(close)) => Err(format!(
            "{operation}; D4 shutdown failed: {shutdown}; USB transport close failed: {close}"
        )),
    };

    UsbEepromDumpOutcome { result, events }
}

impl eframe::App for ReinkGui {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        self.poll_usb_candidates();
        self.poll_usb_eeprom_dump();
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if matches!(&self.usb_scan_status, UsbScanStatus::Scanning) || self.usb_dump_in_progress() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.render_three_pane_layout(ui);
    }
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
) -> bool {
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
    response.clicked()
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
    use reink_gui::SourceMode;

    use super::source_mode_from_args;

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
}
