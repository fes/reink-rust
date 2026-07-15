#![forbid(unsafe_code)]

use eframe::egui::{self, Color32, RichText};
use reink_gui::{GuiState, Page, ValidationStatus};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([960.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "ReInk fixture GUI",
        options,
        Box::new(|_| Ok(Box::new(ReinkGui::new()))),
    )
}

struct ReinkGui {
    state: GuiState,
}

impl ReinkGui {
    fn new() -> Self {
        Self {
            state: GuiState::new().expect("the built-in model database is valid"),
        }
    }

    fn navigation(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("ReInk");
            ui.label("Fixture-backed read-only GUI");
            ui.separator();
            for (page, label) in [
                (Page::Home, "Home"),
                (Page::ValidationReport, "Validation report"),
                (Page::EepromViewer, "EEPROM viewer"),
            ] {
                if ui
                    .selectable_label(self.state.page() == page, label)
                    .clicked()
                {
                    self.state.navigate_to(page);
                }
            }
        });
    }

    fn safety_notice(ui: &mut egui::Ui) {
        egui::Frame::default()
            .fill(Color32::from_rgb(232, 245, 235))
            .stroke(egui::Stroke::new(1.0, Color32::from_rgb(63, 121, 77)))
            .corner_radius(6)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.label(RichText::new("Safety status: fixture-only and read-only").strong());
                ui.label(
                    "No USB, SNMP, D4, or other printer transport is linked. This UI has no write or reset actions.",
                );
            });
    }

    fn home(&mut self, ui: &mut egui::Ui) {
        Self::safety_notice(ui);
        ui.add_space(12.0);
        ui.heading("Fixture device");
        let selected = self.state.selected_fixture_index();
        egui::ComboBox::from_id_salt("fixture-device")
            .selected_text(self.state.selected_fixture().label)
            .show_ui(ui, |ui| {
                for (index, fixture) in reink_gui::FIXTURE_DEVICES.iter().enumerate() {
                    if ui
                        .selectable_label(selected == index, fixture.label)
                        .clicked()
                    {
                        self.state.select_fixture(index);
                    }
                }
            });

        ui.add_space(12.0);
        ui.heading("Identity and model resolution");
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

        ui.add_space(18.0);
        if ui.button("View ordered validation report").clicked() {
            self.state.navigate_to(Page::ValidationReport);
        }
        if ui.button("View fixture EEPROM rows").clicked() {
            self.state.navigate_to(Page::EepromViewer);
        }
    }

    fn validation_report(&self, ui: &mut egui::Ui) {
        ui.heading("Ordered validation report");
        ui.label(format!("Fixture: {}", self.state.selected_fixture().label));
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
                    ui.label(RichText::new(item.status.label()).color(status_color(item.status)));
                    ui.label(item.check);
                    ui.label(item.detail);
                    ui.end_row();
                }
            });
    }

    fn eeprom_viewer(&self, ui: &mut egui::Ui) {
        ui.heading("EEPROM viewer");
        ui.label("Mock rows only — no EEPROM is read, written, or reset.");
        ui.add_space(8.0);
        egui::Grid::new("eeprom-viewer")
            .striped(true)
            .num_columns(3)
            .spacing([24.0, 8.0])
            .show(ui, |ui| {
                ui.strong("Address");
                ui.strong("Value");
                ui.strong("Fixture label");
                ui.end_row();
                for row in self.state.selected_fixture().eeprom_rows {
                    ui.monospace(format!("0x{:04X}", row.address));
                    ui.monospace(format!("0x{:02X}", row.value));
                    ui.label(row.label);
                    ui.end_row();
                }
            });
    }
}

impl eframe::App for ReinkGui {
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        self.navigation(ui);
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| match self.state.page() {
            Page::Home => self.home(ui),
            Page::ValidationReport => self.validation_report(ui),
            Page::EepromViewer => self.eeprom_viewer(ui),
        });
    }
}

fn status_color(status: ValidationStatus) -> Color32 {
    match status {
        ValidationStatus::Success => Color32::from_rgb(36, 130, 76),
        ValidationStatus::Blocked => Color32::from_rgb(174, 112, 0),
        ValidationStatus::Failure => Color32::from_rgb(181, 47, 47),
    }
}
