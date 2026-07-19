use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use reink_platform::TransportEvent;
use serde_json::{Value, json};

use super::TRACE_SANITIZATION_CONFIRMATION;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SanitizedTraceEvent {
    is_write: bool,
    bytes: Vec<u8>,
}

pub(super) fn trace_to_transcript(
    trace_file: &Path,
    output_file: &Path,
    confirmation: Option<&str>,
    description: &str,
) -> Result<String, String> {
    if confirmation != Some(TRACE_SANITIZATION_CONFIRMATION) {
        return Err(format!(
            "refusing to convert trace: pass --confirmation {TRACE_SANITIZATION_CONFIRMATION} exactly after manually redacting and reviewing it"
        ));
    }
    if description.trim().is_empty() {
        return Err("fixture description must not be empty".to_owned());
    }
    validate_new_file_path(output_file, "transcript template")?;
    let source = std::fs::read_to_string(trace_file).map_err(|error| {
        format!(
            "could not read trace file {}: {error}",
            trace_file.display()
        )
    })?;
    let events = parse_trace_events(&source)?;
    let template = transcript_template(description, &events);
    write_new_file(output_file, &template, "transcript template")?;
    Ok(format!(
        "local transcript template written to {}; review it, add assertions, and do not commit it until it has been reviewed",
        output_file.display()
    ))
}

pub(super) fn parse_trace_events(source: &str) -> Result<Vec<SanitizedTraceEvent>, String> {
    let trace: Value =
        serde_json::from_str(source).map_err(|error| format!("invalid trace JSON: {error}"))?;
    let object = trace
        .as_object()
        .ok_or_else(|| "invalid trace schema: root must be an object".to_owned())?;
    for required in ["schema_version", "mode", "command", "events"] {
        if !object.contains_key(required) {
            return Err(format!("invalid trace schema: missing {required}"));
        }
    }
    if object.len() != 4 {
        return Err("invalid trace schema: root has unexpected fields".to_owned());
    }
    if object.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return Err("invalid trace schema: schema_version must be 1".to_owned());
    }
    if object.get("mode").and_then(Value::as_str) != Some("read_only") {
        return Err("invalid trace schema: mode must be read_only".to_owned());
    }
    if object
        .get("command")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err("invalid trace schema: command must be a non-empty string".to_owned());
    }
    let events = object
        .get("events")
        .and_then(Value::as_array)
        .ok_or_else(|| "invalid trace schema: events must be an array".to_owned())?;

    events
        .iter()
        .enumerate()
        .map(|(index, event)| parse_trace_event(index, event))
        .collect()
}

fn parse_trace_event(index: usize, event: &Value) -> Result<SanitizedTraceEvent, String> {
    let object = event
        .as_object()
        .ok_or_else(|| format!("invalid trace event {index}: must be an object"))?;
    if object.len() != 2 || !object.contains_key("direction") || !object.contains_key("bytes") {
        return Err(format!(
            "invalid trace event {index}: expected only direction and bytes"
        ));
    }
    let is_write = match object.get("direction").and_then(Value::as_str) {
        Some("tx") => true,
        Some("rx") => false,
        _ => {
            return Err(format!(
                "invalid trace event {index}: direction must be tx or rx"
            ));
        }
    };
    let hex = object
        .get("bytes")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("invalid trace event {index}: bytes must be a string"))?;
    if hex.len() % 2 != 0 {
        return Err(format!(
            "invalid trace event {index}: bytes must contain an even number of hexadecimal characters"
        ));
    }
    if !hex
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'A'..=b'F'))
    {
        return Err(format!(
            "invalid trace event {index}: bytes must be uppercase hexadecimal"
        ));
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|offset| {
            u8::from_str_radix(&hex[offset..offset + 2], 16).map_err(|_| {
                format!("invalid trace event {index}: bytes must be valid hexadecimal")
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SanitizedTraceEvent { is_write, bytes })
}

pub(super) fn transcript_template(description: &str, events: &[SanitizedTraceEvent]) -> String {
    let escaped_description = format!("{description:?}");
    let mut template = String::from(
        "// Local template only. The operator confirmed this evidence was manually sanitized.\n\
    // Review every byte, add behavior assertions, and do not commit this template without review.\n\
    let mut transcript = SanitizedTranscript::new(",
    );
    template.push_str(&escaped_description);
    template.push_str(");\n");
    for event in events {
        let bytes = event
            .bytes
            .iter()
            .map(|byte| format!("0x{byte:02X}"))
            .collect::<Vec<_>>()
            .join(", ");
        let method = if event.is_write {
            "expect_write"
        } else {
            "respond"
        };
        template.push_str(&format!("transcript.{method}(vec![{bytes}]);\n"));
    }
    template.push_str("// Add assertions for the behavior this transcript protects.\n");
    template
}

fn validate_new_file_path(path: &Path, kind: &str) -> Result<(), String> {
    if path.exists() {
        return Err(format!(
            "refusing to overwrite existing {kind} file: {}",
            path.display()
        ));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.is_dir()
    {
        return Err(format!(
            "{kind} file parent directory does not exist: {}",
            parent.display()
        ));
    }
    Ok(())
}

fn write_new_file(path: &Path, contents: &str, kind: &str) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("could not create {kind} file {}: {error}", path.display()))?;
    file.write_all(contents.as_bytes())
        .map_err(|error| format!("could not write {kind} file {}: {error}", path.display()))
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
pub(super) fn trace_json(command: &str, events: &[TransportEvent]) -> Value {
    let events = events
        .iter()
        .map(|event| match event {
            TransportEvent::Tx(bytes) => json!({
                "direction": "tx",
                "bytes": bytes.iter().map(|byte| format!("{byte:02X}")).collect::<String>(),
            }),
            TransportEvent::Rx(bytes) => json!({
                "direction": "rx",
                "bytes": bytes.iter().map(|byte| format!("{byte:02X}")).collect::<String>(),
            }),
        })
        .collect::<Vec<_>>();
    json!({
        "schema_version": 1,
        "mode": "read_only",
        "command": command,
        "events": events,
    })
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
pub(super) fn validate_trace_file_path(path: &Path) -> Result<(), String> {
    validate_private_new_file_path(path, "trace")
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
pub(super) fn validate_report_file_path(path: &Path) -> Result<(), String> {
    validate_private_new_file_path(path, "report")
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
pub(super) fn validate_private_new_file_path(path: &Path, kind: &str) -> Result<(), String> {
    if path.exists() {
        return Err(format!(
            "refusing to overwrite existing private {kind} file: {}",
            path.display(),
        ));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.is_dir()
    {
        return Err(format!(
            "private {kind} file parent directory does not exist: {}",
            parent.display(),
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
pub(super) fn write_report_file(path: &Path, report: &str) -> Result<(), String> {
    validate_report_file_path(path)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            format!(
                "could not create private report file {}: {error}",
                path.display()
            )
        })?;
    file.write_all(report.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            format!(
                "could not persist private report file {}: {error}",
                path.display()
            )
        })
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
pub(super) fn write_new_private_binary_file(
    path: &Path,
    bytes: &[u8],
    kind: &str,
) -> Result<(), String> {
    reink_app::write_new_binary_file(path, bytes, &format!("private {kind}"))
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows", test))]
pub(super) fn normalized_new_file_path(path: &Path, kind: &str) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let parent = parent.unwrap_or_else(|| Path::new("."));
    let parent = parent.canonicalize().map_err(|error| {
        format!(
            "could not resolve private {kind} parent directory {}: {error}",
            parent.display()
        )
    })?;
    let file_name = path
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("private {kind} path must name a file: {}", path.display()))?;
    Ok(parent.join(file_name))
}
