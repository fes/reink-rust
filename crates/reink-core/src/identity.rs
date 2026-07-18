use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

/// An IEEE 1284 printer device identifier.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PrinterIdentity {
    fields: BTreeMap<String, String>,
    command_set: Vec<String>,
}

impl PrinterIdentity {
    /// Parses the semicolon-delimited IEEE 1284 identifier returned by a printer.
    pub fn parse(identifier: &str) -> Result<Self, IdentityParseError> {
        if !identifier.is_ascii() {
            return Err(IdentityParseError::NonAscii);
        }

        let mut fields = BTreeMap::new();
        for entry in identifier.split(';').filter(|entry| !entry.is_empty()) {
            let (key, value) =
                entry
                    .split_once(':')
                    .ok_or_else(|| IdentityParseError::MalformedField {
                        field: entry.to_owned(),
                    })?;
            if key.is_empty() {
                return Err(IdentityParseError::MalformedField {
                    field: entry.to_owned(),
                });
            }
            fields.insert(key.to_owned(), value.to_owned());
        }

        for (long_name, short_name) in [
            ("MANUFACTURER", "MFG"),
            ("MODEL", "MDL"),
            ("COMMAND SET", "CMD"),
        ] {
            if let Some(value) = fields.get(long_name).cloned() {
                fields.insert(short_name.to_owned(), value);
            }
        }

        let command_set = fields
            .get("CMD")
            .map(|commands| commands.split(',').map(str::to_owned).collect())
            .unwrap_or_default();

        Ok(Self {
            fields,
            command_set,
        })
    }

    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }

    pub fn manufacturer(&self) -> Option<&str> {
        self.field("MFG")
    }

    pub fn model(&self) -> Option<&str> {
        self.field("MDL")
    }

    /// Returns the source-compatible Epson database model candidate.
    ///
    /// Epson IEEE 1284 IDs commonly append ` Series`, while the bundled model
    /// database stores the base model name.
    pub fn detected_model(&self) -> Option<&str> {
        self.model()
            .map(|model| model.strip_suffix(" Series").unwrap_or(model))
    }

    pub fn serial_number(&self) -> Option<&str> {
        self.field("SN")
    }

    pub fn command_set(&self) -> &[String] {
        &self.command_set
    }

    pub fn fields(&self) -> &BTreeMap<String, String> {
        &self.fields
    }
}

/// A malformed printer device identifier.
#[derive(Clone, Eq, PartialEq)]
pub enum IdentityParseError {
    NonAscii,
    MalformedField { field: String },
}

impl fmt::Debug for IdentityParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonAscii => formatter.write_str("NonAscii"),
            Self::MalformedField { .. } => formatter.write_str("MalformedField(<redacted>)"),
        }
    }
}

impl fmt::Display for IdentityParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonAscii => formatter.write_str("IEEE 1284 device ID is not ASCII"),
            Self::MalformedField { .. } => {
                formatter.write_str("malformed IEEE 1284 device ID field")
            }
        }
    }
}

impl Error for IdentityParseError {}

#[cfg(test)]
mod tests {
    use super::{IdentityParseError, PrinterIdentity};

    #[test]
    fn parses_standard_fields_and_aliases() {
        let identity = PrinterIdentity::parse(
            "MANUFACTURER:EPSON;MODEL:XP-352 Series;COMMAND SET:ESCPL2,BDC;SN:123;",
        )
        .unwrap();

        assert_eq!(identity.manufacturer(), Some("EPSON"));
        assert_eq!(identity.model(), Some("XP-352 Series"));
        assert_eq!(identity.detected_model(), Some("XP-352"));
        assert_eq!(identity.serial_number(), Some("123"));
        assert_eq!(identity.command_set(), ["ESCPL2", "BDC"]);
    }

    #[test]
    fn rejects_non_ascii_identifiers() {
        assert_eq!(
            PrinterIdentity::parse("MFG:EPSON;MDL:é;").unwrap_err(),
            IdentityParseError::NonAscii
        );
    }

    #[test]
    fn rejects_fields_without_a_separator() {
        assert!(matches!(
            PrinterIdentity::parse("MFG:EPSON;invalid;"),
            Err(IdentityParseError::MalformedField { .. })
        ));
    }
}
