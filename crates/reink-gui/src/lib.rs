#![forbid(unsafe_code)]
//! Read-only UI state for the optional ReInk GUI.
//!
//! Fixtures never open a transport. Connected USB candidates are descriptor-only:
//! they do not identify a printer or permit device, EEPROM, or maintenance access.

use std::collections::VecDeque;

use reink_core::{EpsonSpec, ModelDatabase, PrinterIdentity};
use reink_platform::TransportEvent;

/// Maximum number of transport events retained for one GUI session.
pub const DEBUG_TRAFFIC_MAX_ENTRIES: usize = 1_000;

/// Launch mode controlling whether bundled fixtures can be selected.
///
/// Real mode is the default and contains no implicit printer source.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SourceMode {
    #[default]
    Real,
    Fixtures,
}

impl SourceMode {
    pub const fn fixtures_enabled(self) -> bool {
        matches!(self, Self::Fixtures)
    }
}

/// Direction of one recorded transport transfer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DebugTrafficDirection {
    Tx,
    Rx,
}

impl DebugTrafficDirection {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tx => "TX",
            Self::Rx => "RX",
        }
    }
}

/// One display-safe, session-only transport record.
///
/// The byte string is uppercase hexadecimal separated by single spaces. It has
/// no timestamp, transport description, or device identifier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugTrafficEntry {
    direction: DebugTrafficDirection,
    hex_bytes: String,
}

impl DebugTrafficEntry {
    pub const fn direction(&self) -> DebugTrafficDirection {
        self.direction
    }

    pub fn hex_bytes(&self) -> &str {
        &self.hex_bytes
    }
}

/// Bounded, in-memory debug traffic for the current GUI session.
///
/// Capture is disabled by default. A future explicit connected read-only
/// operation can pass `RecordingTransport::into_parts().1` to
/// [`Self::append_events`]; the events are ignored unless capture is enabled.
#[derive(Debug)]
pub struct DebugTrafficTrace {
    capture_enabled: bool,
    entries: VecDeque<DebugTrafficEntry>,
}

impl Default for DebugTrafficTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl DebugTrafficTrace {
    pub fn new() -> Self {
        Self {
            capture_enabled: false,
            entries: VecDeque::new(),
        }
    }

    pub const fn capture_enabled(&self) -> bool {
        self.capture_enabled
    }

    pub fn set_capture_enabled(&mut self, enabled: bool) {
        self.capture_enabled = enabled;
    }

    /// Appends one event when capture is enabled, returning whether it was kept.
    pub fn append(&mut self, event: &TransportEvent) -> bool {
        if !self.capture_enabled {
            return false;
        }

        let (direction, bytes) = match event {
            TransportEvent::Tx(bytes) => (DebugTrafficDirection::Tx, bytes),
            TransportEvent::Rx(bytes) => (DebugTrafficDirection::Rx, bytes),
        };
        if self.entries.len() == DEBUG_TRAFFIC_MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(DebugTrafficEntry {
            direction,
            hex_bytes: format_hex_bytes(bytes),
        });
        true
    }

    /// Appends ordered `RecordingTransport` events when capture is enabled.
    ///
    /// Read events are appended independently, including empty reads, so their
    /// original boundaries remain visible in the trace.
    pub fn append_events(&mut self, events: Vec<TransportEvent>) -> usize {
        events.iter().filter(|event| self.append(event)).count()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn count(&self) -> usize {
        self.entries.len()
    }

    pub fn entries(
        &self,
    ) -> impl ExactSizeIterator<Item = &DebugTrafficEntry> + DoubleEndedIterator {
        self.entries.iter()
    }
}

fn format_hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Page {
    Status,
    Eeprom,
    Tools,
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
    pub eeprom_bytes: &'static [u8],
}

/// Descriptor-only USB printer information shown for the current GUI session.
///
/// This intentionally omits USB strings and any device handle. The alias is
/// session-local and model hints are only exact VID/PID database matches.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DescriptorCandidate {
    pub alias: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub bus_number: u8,
    pub device_address: u8,
    pub interface_number: u8,
    pub alternate_setting: u8,
    pub model_hints: Vec<String>,
}

fn model_hints_for_usb_candidate(
    database: &ModelDatabase,
    vendor_id: u16,
    product_id: u16,
) -> Vec<String> {
    database
        .models()
        .filter(|model| {
            database.get(model).is_some_and(|spec| {
                spec.vendor_id == vendor_id && spec.product_id == Some(product_id)
            })
        })
        .map(str::to_owned)
        .collect()
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

const FIXTURE_EEPROM_LENGTH: usize = 256;

const fn fixture_eeprom(rows: &[EepromRow]) -> [u8; FIXTURE_EEPROM_LENGTH] {
    let mut bytes = [0; FIXTURE_EEPROM_LENGTH];
    let mut index = 0;
    while index < rows.len() {
        let row = rows[index];
        bytes[row.address as usize] = row.value;
        index += 1;
    }
    bytes
}

const XP_352_EEPROM_BYTES: [u8; FIXTURE_EEPROM_LENGTH] = fixture_eeprom(XP_352_EEPROM);
const C90_EEPROM_BYTES: [u8; FIXTURE_EEPROM_LENGTH] = fixture_eeprom(C90_EEPROM);

pub const FIXTURE_DEVICES: &[FixtureDevice] = &[
    FixtureDevice {
        label: "XP-352 fixture",
        identity: "MFG:EPSON;MDL:XP-352 Series;CMD:ESCPL2,BDC;SN:FIXTURE-0001;",
        validation_report: XP_352_REPORT,
        eeprom_rows: XP_352_EEPROM,
        eeprom_bytes: &XP_352_EEPROM_BYTES,
    },
    FixtureDevice {
        label: "C90 fixture",
        identity: "MFG:EPSON;MDL:C90;CMD:ESCPL2;SN:FIXTURE-0002;",
        validation_report: C90_REPORT,
        eeprom_rows: C90_EEPROM,
        eeprom_bytes: &C90_EEPROM_BYTES,
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
    selected_eeprom_row: usize,
    database: ModelDatabase,
}

impl GuiState {
    pub fn new() -> Result<Self, reink_core::SpecError> {
        Ok(Self {
            page: Page::Status,
            selected_fixture: 0,
            selected_eeprom_row: 0,
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
            self.selected_eeprom_row = 0;
        }
    }

    pub const fn selected_eeprom_row_index(&self) -> usize {
        self.selected_eeprom_row
    }

    pub fn selected_eeprom_row(&self) -> &'static EepromRow {
        &self.selected_fixture().eeprom_rows[self.selected_eeprom_row]
    }

    pub fn select_eeprom_row(&mut self, index: usize) {
        if index < self.selected_fixture().eeprom_rows.len() {
            self.selected_eeprom_row = index;
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

    pub fn model_names(&self) -> impl Iterator<Item = &str> {
        self.database.models()
    }

    pub fn model_spec(&self, model: &str) -> Option<&EpsonSpec> {
        self.database.get(model)
    }

    /// Returns bundled model names whose explicit VID/PID exactly matches.
    ///
    /// A match is a display hint only; USB descriptors do not confirm printer
    /// identity.
    pub fn model_hints_for_usb_candidate(&self, vendor_id: u16, product_id: u16) -> Vec<String> {
        model_hints_for_usb_candidate(&self.database, vendor_id, product_id)
    }
}

#[cfg(test)]
mod tests {
    use reink_core::ModelDatabase;
    use reink_platform::TransportEvent;

    use super::{
        DEBUG_TRAFFIC_MAX_ENTRIES, DebugTrafficDirection, DebugTrafficTrace, FIXTURE_DEVICES,
        GuiState, Page, SourceMode, ValidationStatus, model_hints_for_usb_candidate,
    };

    #[test]
    fn debug_trace_formats_tx_rx_and_preserves_read_fragments_in_order() {
        let mut trace = DebugTrafficTrace::new();
        trace.set_capture_enabled(true);

        assert_eq!(
            trace.append_events(vec![
                TransportEvent::Tx(vec![0x1b, 0x40]),
                TransportEvent::Rx(vec![0x06]),
                TransportEvent::Rx(vec![]),
            ]),
            3
        );

        let entries = trace.entries().collect::<Vec<_>>();
        assert_eq!(entries[0].direction(), DebugTrafficDirection::Tx);
        assert_eq!(entries[0].hex_bytes(), "1B 40");
        assert_eq!(entries[1].direction(), DebugTrafficDirection::Rx);
        assert_eq!(entries[1].hex_bytes(), "06");
        assert_eq!(entries[2].direction(), DebugTrafficDirection::Rx);
        assert_eq!(entries[2].hex_bytes(), "");
    }

    #[test]
    fn debug_trace_accepts_events_only_after_opt_in() {
        let mut trace = DebugTrafficTrace::new();
        let event = TransportEvent::Tx(vec![0xaa]);

        assert!(!trace.append(&event));
        assert_eq!(trace.count(), 0);

        trace.set_capture_enabled(true);
        assert!(trace.append(&event));
        assert_eq!(trace.count(), 1);
    }

    #[test]
    fn debug_trace_clears_session_entries() {
        let mut trace = DebugTrafficTrace::new();
        trace.set_capture_enabled(true);
        trace.append(&TransportEvent::Rx(vec![0x06]));

        trace.clear();

        assert_eq!(trace.count(), 0);
        assert!(trace.entries().next().is_none());
    }

    #[test]
    fn debug_trace_evicts_the_oldest_entry_at_its_fixed_bound() {
        let mut trace = DebugTrafficTrace::new();
        trace.set_capture_enabled(true);
        for value in 0..=DEBUG_TRAFFIC_MAX_ENTRIES {
            trace.append(&TransportEvent::Tx(vec![(value % 256) as u8]));
        }

        assert_eq!(trace.count(), DEBUG_TRAFFIC_MAX_ENTRIES);
        assert_eq!(trace.entries().next().unwrap().hex_bytes(), "01");
        assert_eq!(trace.entries().next_back().unwrap().hex_bytes(), "E8");
    }

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
    fn real_mode_is_the_default_and_does_not_enable_fixture_selection() {
        assert_eq!(SourceMode::default(), SourceMode::Real);
        assert!(!SourceMode::Real.fixtures_enabled());
        assert!(SourceMode::Fixtures.fixtures_enabled());
    }

    #[test]
    fn invalid_fixture_selection_preserves_the_current_fixture() {
        let mut state = GuiState::new().unwrap();
        state.select_fixture(FIXTURE_DEVICES.len());

        assert_eq!(state.selected_fixture_index(), 0);
    }

    #[test]
    fn navigation_reaches_each_fixture_only_view() {
        let mut state = GuiState::new().unwrap();
        assert_eq!(state.page(), Page::Status);

        state.navigate_to(Page::Eeprom);
        assert_eq!(state.page(), Page::Eeprom);
        state.navigate_to(Page::Tools);
        assert_eq!(state.page(), Page::Tools);
        state.navigate_to(Page::Status);
        assert_eq!(state.page(), Page::Status);
    }

    #[test]
    fn eeprom_selection_is_bounded_and_resets_with_fixture_changes() {
        let mut state = GuiState::new().unwrap();
        state.select_eeprom_row(2);

        assert_eq!(state.selected_eeprom_row().address, 0x000c);

        state.select_eeprom_row(3);
        assert_eq!(state.selected_eeprom_row_index(), 2);

        state.select_fixture(1);
        assert_eq!(state.selected_eeprom_row_index(), 0);
        assert_eq!(state.selected_eeprom_row().address, 0x0006);
    }

    #[test]
    fn fixture_eeprom_dump_contains_each_displayed_field_value() {
        for fixture in FIXTURE_DEVICES {
            assert_eq!(fixture.eeprom_bytes.len(), 256);
            for row in fixture.eeprom_rows {
                assert_eq!(fixture.eeprom_bytes[row.address as usize], row.value);
            }
        }
    }

    #[test]
    fn bundled_models_are_available_for_eeprom_file_interpretation() {
        let state = GuiState::new().unwrap();

        assert!(state.model_names().any(|model| model == "L1800"));
        assert!(
            state
                .model_spec("L1800")
                .is_some_and(|spec| !spec.memory_operations.is_empty())
        );
    }

    #[test]
    fn usb_model_hints_require_an_exact_vendor_and_product_match() {
        let database = ModelDatabase::from_toml(
            r#"
[[EPSON]]
models = ["Exact"]
idVendor = 0x04b8
idProduct = 0x1234

[[EPSON]]
models = ["Other product"]
idVendor = 0x04b8
idProduct = 0x5678
"#,
        )
        .unwrap();

        assert_eq!(
            model_hints_for_usb_candidate(&database, 0x04b8, 0x1234),
            ["Exact"]
        );
        assert_eq!(
            model_hints_for_usb_candidate(&database, 0x04b8, 0x5678),
            ["Other product"]
        );
        assert!(model_hints_for_usb_candidate(&database, 0x1234, 0x1234).is_empty());
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
