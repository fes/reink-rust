use eframe::egui::{self, Color32, RichText};
use reink_core::EepromFieldEncoding;
use reink_gui::ValidationStatus;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use reink_app::{UsbCleanupStatus, UsbSessionCleanup};

use super::EepromFileField;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(super) fn cleanup_is_successful(cleanup: &UsbSessionCleanup) -> bool {
    matches!(&cleanup.d4_shutdown, UsbCleanupStatus::Succeeded)
        && matches!(&cleanup.usb_close, UsbCleanupStatus::Succeeded)
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(super) fn cleanup_report_lines(cleanup: &UsbSessionCleanup) -> Vec<String> {
    vec![
        format!(
            "D4 shutdown: {}.",
            cleanup_status_label(&cleanup.d4_shutdown)
        ),
        format!("USB cleanup: {}.", cleanup_status_label(&cleanup.usb_close)),
    ]
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub(super) fn cleanup_status_label(status: &UsbCleanupStatus) -> String {
    match status {
        UsbCleanupStatus::NotAttempted => "not attempted".to_owned(),
        UsbCleanupStatus::Succeeded => "succeeded".to_owned(),
        UsbCleanupStatus::Failed(error) => format!("failed ({error})"),
    }
}

pub(super) fn eeprom_image_offset(
    start_address: usize,
    byte_count: usize,
    address: usize,
) -> Option<usize> {
    address
        .checked_sub(start_address)
        .filter(|offset| *offset < byte_count)
}

pub(super) fn eeprom_field_address_label(field: &EepromFileField) -> String {
    if field.address == field.end_address {
        format!("0x{:04X}", field.address)
    } else {
        format!("0x{:04X}..0x{:04X}", field.address, field.end_address)
    }
}

pub(super) fn eeprom_field_value(
    field: &EepromFileField,
    image_start_address: usize,
    bytes: &[u8],
) -> String {
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

pub(super) fn eeprom_field_bytes<'a>(
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

pub(super) fn eeprom_field_tooltip(field: &EepromFileField) -> String {
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

#[allow(clippy::too_many_arguments)]
pub(super) fn eeprom_table_row(
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

pub(super) fn paint_eeprom_cell(
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

pub(super) fn eeprom_field_column_width(
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

pub(super) fn wrap_eeprom_cell_text(
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

pub(super) fn paint_dashed_vertical_line(
    ui: &egui::Ui,
    x: f32,
    rect: egui::Rect,
    stroke: egui::Stroke,
) {
    let mut y = rect.top() + 2.0;
    while y < rect.bottom() - 2.0 {
        let end = (y + 3.0).min(rect.bottom() - 2.0);
        ui.painter()
            .line_segment([egui::pos2(x, y), egui::pos2(x, end)], stroke);
        y += 6.0;
    }
}

pub(super) fn end_truncate(value: &str, maximum_characters: usize) -> String {
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

pub(super) fn dump_cell(value: String, highlighted: bool) -> RichText {
    let text = RichText::new(value).monospace();
    if highlighted {
        text.background_color(Color32::from_rgb(239, 220, 130))
    } else {
        text
    }
}

pub(super) fn ascii_cell(byte: u8) -> char {
    match byte {
        b' '..=b'~' => byte as char,
        _ => '.',
    }
}

pub(super) fn ascii_dump_line(
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
                background: if highlighted {
                    Color32::from_rgb(239, 220, 130)
                } else {
                    Color32::default()
                },
                ..Default::default()
            },
        );
    }
    line
}

pub(super) fn status_color(status: ValidationStatus) -> Color32 {
    match status {
        ValidationStatus::Success => Color32::from_rgb(36, 130, 76),
        ValidationStatus::Blocked => Color32::from_rgb(174, 112, 0),
        ValidationStatus::Failure => Color32::from_rgb(181, 47, 47),
    }
}
