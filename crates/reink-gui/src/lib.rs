#![forbid(unsafe_code)]
//! Fixture-backed, read-only UI state for the optional ReInk GUI.
//!
//! This crate deliberately depends only on `reink-core`. Its fixtures never
//! open a transport and its UI exposes no EEPROM write or counter-reset action.

use reink_core::{ModelDatabase, PrinterIdentity};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Page {
    Home,
    ValidationReport,
    EepromViewer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValidationStatus {
    Success,
    Blocked,
    Failure,
}

impl ValidationStatus {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Success => "Success",
            Self::Blocked => "Blocked",
            Self::Failure => "Failure",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidationReportItem {
    pub status: ValidationStatus,
    pub check: &'static str,
    pub detail: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EepromRow {
    pub address: u16,
    pub value: u8,
    pub label: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FixtureDevice {
    pub label: &'static str,
    pub identity: &'static str,
    pub validation_report: &'static [ValidationReportItem],
    pub eeprom_rows: &'static [EepromRow],
}

const XP_352_REPORT: &[ValidationReportItem] = &[
    ValidationReportItem {
        status: ValidationStatus::Success,
        check: "Fixture safety boundary",
        detail: "Fixture mode is active; no physical transport is linked.",
    },
    ValidationReportItem {
        status: ValidationStatus::Success,
        check: "Identity parsing",
        detail: "The IEEE 1284 fixture identity parsed successfully.",
    },
    ValidationReportItem {
        status: ValidationStatus::Blocked,
        check: "EEPROM read",
        detail: "Blocked intentionally: this GUI has no transport dependency.",
    },
    ValidationReportItem {
        status: ValidationStatus::Failure,
        check: "Fixture protocol replay",
        detail: "Simulated malformed reply retained to demonstrate a visible failure state.",
    },
];

const C90_REPORT: &[ValidationReportItem] = &[
    ValidationReportItem {
        status: ValidationStatus::Success,
        check: "Fixture safety boundary",
        detail: "Fixture mode is active; no physical transport is linked.",
    },
    ValidationReportItem {
        status: ValidationStatus::Success,
        check: "Model resolution",
        detail: "The bundled model database resolved the C90 fixture.",
    },
    ValidationReportItem {
        status: ValidationStatus::Blocked,
        check: "Waste-counter reset",
        detail: "Blocked intentionally: reset operations are not present in this GUI.",
    },
    ValidationReportItem {
        status: ValidationStatus::Failure,
        check: "Fixture reply validation",
        detail: "Simulated checksum mismatch retained to demonstrate a visible failure state.",
    },
];

const XP_352_EEPROM: &[EepromRow] = &[
    EepromRow {
        address: 0x0006,
        value: 0x18,
        label: "Fixture counter byte A",
    },
    EepromRow {
        address: 0x0007,
        value: 0x04,
        label: "Fixture counter byte B",
    },
    EepromRow {
        address: 0x000c,
        value: 0x57,
        label: "Fixture maintenance byte",
    },
];

const C90_EEPROM: &[EepromRow] = &[
    EepromRow {
        address: 0x0006,
        value: 0x00,
        label: "Fixture counter byte A",
    },
    EepromRow {
        address: 0x0007,
        value: 0x20,
        label: "Fixture counter byte B",
    },
    EepromRow {
        address: 0x0035,
        value: 0x57,
        label: "Fixture maintenance byte",
    },
];

pub const FIXTURE_DEVICES: &[FixtureDevice] = &[
    FixtureDevice {
        label: "XP-352 fixture",
        identity: "MFG:EPSON;MDL:XP-352 Series;CMD:ESCPL2,BDC;SN:FIXTURE-0001;",
        validation_report: XP_352_REPORT,
        eeprom_rows: XP_352_EEPROM,
    },
    FixtureDevice {
        label: "C90 fixture",
        identity: "MFG:EPSON;MDL:C90;CMD:ESCPL2;SN:FIXTURE-0002;",
        validation_report: C90_REPORT,
        eeprom_rows: C90_EEPROM,
    },
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityResolution {
    pub manufacturer: Option<String>,
    pub advertised_model: Option<String>,
    pub resolved_model: Option<String>,
}

#[derive(Debug)]
pub struct GuiState {
    page: Page,
    selected_fixture: usize,
    database: ModelDatabase,
}

impl GuiState {
    pub fn new() -> Result<Self, reink_core::SpecError> {
        Ok(Self {
            page: Page::Home,
            selected_fixture: 0,
            database: ModelDatabase::builtin()?,
        })
    }

    pub const fn page(&self) -> Page {
        self.page
    }

    pub const fn selected_fixture_index(&self) -> usize {
        self.selected_fixture
    }

    pub fn selected_fixture(&self) -> &'static FixtureDevice {
        &FIXTURE_DEVICES[self.selected_fixture]
    }

    pub fn select_fixture(&mut self, index: usize) {
        if index < FIXTURE_DEVICES.len() {
            self.selected_fixture = index;
        }
    }

    pub fn navigate_to(&mut self, page: Page) {
        self.page = page;
    }

    pub fn identity_resolution(&self) -> IdentityResolution {
        let identity = PrinterIdentity::parse(self.selected_fixture().identity).ok();
        let resolved_model = identity
            .as_ref()
            .and_then(|identity| self.database.resolve_identity(identity))
            .map(|spec| spec.model.clone());
        IdentityResolution {
            manufacturer: identity
                .as_ref()
                .and_then(PrinterIdentity::manufacturer)
                .map(str::to_owned),
            advertised_model: identity
                .as_ref()
                .and_then(PrinterIdentity::model)
                .map(str::to_owned),
            resolved_model,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FIXTURE_DEVICES, GuiState, Page, ValidationStatus};

    #[test]
    fn fixture_selection_changes_the_resolved_model() {
        let mut state = GuiState::new().unwrap();
        assert_eq!(
            state.identity_resolution().resolved_model.as_deref(),
            Some("XP-352")
        );

        state.select_fixture(1);

        assert_eq!(state.selected_fixture_index(), 1);
        assert_eq!(
            state.identity_resolution().resolved_model.as_deref(),
            Some("C90")
        );
    }

    #[test]
    fn invalid_fixture_selection_preserves_the_current_fixture() {
        let mut state = GuiState::new().unwrap();
        state.select_fixture(FIXTURE_DEVICES.len());

        assert_eq!(state.selected_fixture_index(), 0);
    }

    #[test]
    fn navigation_reaches_each_read_only_view() {
        let mut state = GuiState::new().unwrap();
        assert_eq!(state.page(), Page::Home);

        state.navigate_to(Page::ValidationReport);
        assert_eq!(state.page(), Page::ValidationReport);
        state.navigate_to(Page::EepromViewer);
        assert_eq!(state.page(), Page::EepromViewer);
        state.navigate_to(Page::Home);
        assert_eq!(state.page(), Page::Home);
    }

    #[test]
    fn fixture_report_order_contains_all_statuses() {
        let statuses = FIXTURE_DEVICES[0]
            .validation_report
            .iter()
            .map(|item| item.status)
            .collect::<Vec<_>>();

        assert_eq!(
            statuses,
            [
                ValidationStatus::Success,
                ValidationStatus::Success,
                ValidationStatus::Blocked,
                ValidationStatus::Failure,
            ]
        );
    }
}
