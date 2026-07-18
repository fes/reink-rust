use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use serde::Deserialize;

use crate::PrinterIdentity;

pub const BUILTIN_EPSON_TOML: &str = include_str!("../assets/epson.toml");

/// Number of bytes used to encode an EEPROM address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressWidth {
    One,
    Two,
}

impl AddressWidth {
    pub fn byte_len(self) -> usize {
        match self {
            Self::One => 1,
            Self::Two => 2,
        }
    }

    fn from_raw(field: &'static str, value: u8) -> Result<Self, SpecError> {
        match value {
            1 => Ok(Self::One),
            2 => Ok(Self::Two),
            _ => Err(SpecError::InvalidAddressWidth { field, value }),
        }
    }
}

/// A configured EEPROM counter or maintenance operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryOperation {
    pub description: String,
    pub addresses: Vec<u16>,
    pub reset_values: Vec<u8>,
    /// Whether `reset_values` was explicitly declared in the model metadata.
    ///
    /// A missing `reset` field is deliberately not converted to zero bytes.
    /// ReInkPy's dynamic helper can fall back to zero (and its scalar `min`
    /// metadata cannot be safely zipped as byte values), but a guarded physical
    /// reset must write only values the specification explicitly declares.
    pub reset_values_declared: bool,
    pub minimum: Option<u32>,
}

impl MemoryOperation {
    pub fn has_declared_reset_values(&self) -> bool {
        self.reset_values_declared
    }
}

/// A read-only interpretation for one contiguous EEPROM field.
///
/// This metadata is deliberately separate from [`MemoryOperation`]: it does
/// not authorize writes, define reset values, or imply a write ordering.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EepromField {
    /// First EEPROM address in the field's inclusive range.
    pub address: u16,
    /// Last EEPROM address in the field's inclusive range.
    pub end_address: u16,
    pub label: String,
    pub encoding: EepromFieldEncoding,
    pub confidence: EepromFieldConfidence,
    /// Human-readable provenance and limitations for this interpretation.
    pub evidence_note: String,
    /// Whether a UI must hide the decoded value by default.
    pub sensitive: bool,
}

impl EepromField {
    pub const fn byte_len(&self) -> usize {
        self.end_address as usize - self.address as usize + 1
    }
}

/// Encoding used only to display a read-only EEPROM field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EepromFieldEncoding {
    U8,
    U16Le,
    U32Le,
    Ascii,
    RawBytes,
}

impl EepromFieldEncoding {
    pub const fn label(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::U16Le => "little-endian u16",
            Self::U32Le => "little-endian u32",
            Self::Ascii => "ASCII",
            Self::RawBytes => "raw bytes",
        }
    }

    const fn fixed_byte_len(self) -> Option<usize> {
        match self {
            Self::U8 => Some(1),
            Self::U16Le => Some(2),
            Self::U32Le => Some(4),
            Self::Ascii | Self::RawBytes => None,
        }
    }

    fn from_raw(label: &str, value: &str) -> Result<Self, SpecError> {
        match value {
            "u8" => Ok(Self::U8),
            "u16le" => Ok(Self::U16Le),
            "u32le" => Ok(Self::U32Le),
            "ascii" => Ok(Self::Ascii),
            "raw" => Ok(Self::RawBytes),
            _ => Err(SpecError::InvalidReadOnlyFieldEncoding {
                label: label.to_owned(),
                encoding: value.to_owned(),
            }),
        }
    }
}

/// Strength of the reviewed evidence behind a read-only EEPROM interpretation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EepromFieldConfidence {
    Confirmed,
    StronglyRelated,
    RelatedUnknown,
}

impl EepromFieldConfidence {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Confirmed => "Confirmed",
            Self::StronglyRelated => "Strongly related",
            Self::RelatedUnknown => "Related; role unknown",
        }
    }

    fn from_raw(label: &str, value: &str) -> Result<Self, SpecError> {
        match value {
            "confirmed" => Ok(Self::Confirmed),
            "strongly-related" => Ok(Self::StronglyRelated),
            "related-unknown" => Ok(Self::RelatedUnknown),
            _ => Err(SpecError::InvalidReadOnlyFieldConfidence {
                label: label.to_owned(),
                confidence: value.to_owned(),
            }),
        }
    }
}

/// A counter family with separately declared Epson reset semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CounterResetTarget {
    Waste,
    PlatenPad,
}

impl CounterResetTarget {
    pub const fn description_fragment(self) -> &'static str {
        match self {
            Self::Waste => "waste counter",
            Self::PlatenPad => "platen pad counter",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Waste => "waste counter",
            Self::PlatenPad => "platen pad counter",
        }
    }
}

/// Model-specific Epson EEPROM settings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EpsonSpec {
    pub model: String,
    pub brand: String,
    pub vendor_id: u16,
    pub product_id: Option<u16>,
    pub read_key: u16,
    pub write_key: Option<Vec<u8>>,
    pub shifted_write_key: Option<String>,
    pub read_address_width: AddressWidth,
    pub write_address_width: AddressWidth,
    pub memory_low: u16,
    pub memory_high: u16,
    pub memory_operations: Vec<MemoryOperation>,
    /// Read-only EEPROM display metadata. This has no reset or write semantics.
    pub read_only_fields: Vec<EepromField>,
}

impl EpsonSpec {
    /// Merges matching operations with explicit reset bytes while retaining the
    /// first address order.
    ///
    /// Address values later in the specification replace earlier values, which
    /// matches the Python implementation's dictionary update behavior.
    pub fn merged_operation(&self, description_fragment: &str) -> Option<MemoryOperation> {
        let fragment = description_fragment.to_ascii_lowercase();
        let mut address_positions = BTreeMap::new();
        let mut addresses = Vec::new();
        let mut reset_values = Vec::new();

        for operation in &self.memory_operations {
            if !operation
                .description
                .to_ascii_lowercase()
                .contains(&fragment)
            {
                continue;
            }
            if !operation.has_declared_reset_values() {
                continue;
            }

            for (&address, &value) in operation.addresses.iter().zip(&operation.reset_values) {
                if let Some(&position) = address_positions.get(&address) {
                    reset_values[position] = value;
                } else {
                    address_positions.insert(address, addresses.len());
                    addresses.push(address);
                    reset_values.push(value);
                }
            }
        }

        (!addresses.is_empty()).then(|| MemoryOperation {
            description: format!("All {description_fragment}s"),
            addresses,
            reset_values,
            reset_values_declared: true,
            minimum: None,
        })
    }

    pub fn waste_counter_reset(&self) -> Option<MemoryOperation> {
        self.counter_reset(CounterResetTarget::Waste)
    }

    pub fn platen_pad_counter_reset(&self) -> Option<MemoryOperation> {
        self.counter_reset(CounterResetTarget::PlatenPad)
    }

    /// Returns a physical-reset operation composed only of explicitly declared
    /// byte values for the requested counter family.
    pub fn counter_reset(&self, target: CounterResetTarget) -> Option<MemoryOperation> {
        self.merged_operation(target.description_fragment())
    }
}

/// Epson specifications indexed by the exact advertised model name.
#[derive(Clone, Debug, Default)]
pub struct ModelDatabase {
    models: BTreeMap<String, EpsonSpec>,
}

impl ModelDatabase {
    pub fn builtin() -> Result<Self, SpecError> {
        Self::from_toml(BUILTIN_EPSON_TOML)
    }

    pub fn from_toml(input: &str) -> Result<Self, SpecError> {
        let raw: RawDatabase = toml::from_str(input).map_err(SpecError::Toml)?;
        let mut models = BTreeMap::new();

        for raw_spec in raw.epson {
            let read_address_width = AddressWidth::from_raw("rlen", raw_spec.read_length)?;
            let write_address_width = AddressWidth::from_raw("wlen", raw_spec.write_length)?;
            if raw_spec.memory_low > raw_spec.memory_high {
                return Err(SpecError::InvalidMemoryRange {
                    low: raw_spec.memory_low,
                    high: raw_spec.memory_high,
                });
            }

            let write_key = raw_spec
                .write_key
                .map(|key| latin1_bytes(&key))
                .transpose()?;
            if let Some(key) = &write_key
                && key.len() != 8
            {
                return Err(SpecError::InvalidWriteKeyLength { length: key.len() });
            }

            let memory_operations = raw_spec
                .mem
                .into_iter()
                .map(MemoryOperation::try_from)
                .collect::<Result<Vec<_>, _>>()?;
            let read_only_fields = raw_spec
                .read_only_fields
                .into_iter()
                .map(|field| field.into_eeprom_field(raw_spec.memory_low, raw_spec.memory_high))
                .collect::<Result<Vec<_>, _>>()?;
            validate_read_only_fields(&read_only_fields)?;

            for model in raw_spec.models {
                let spec = EpsonSpec {
                    model: model.clone(),
                    brand: raw_spec.brand.clone().unwrap_or_else(|| "EPSON".to_owned()),
                    vendor_id: raw_spec.vendor_id.unwrap_or(0x04b8),
                    product_id: raw_spec.product_id,
                    read_key: raw_spec.rkey,
                    write_key: write_key.clone(),
                    shifted_write_key: raw_spec.shifted_write_key.clone(),
                    read_address_width,
                    write_address_width,
                    memory_low: raw_spec.memory_low,
                    memory_high: raw_spec.memory_high,
                    memory_operations: memory_operations.clone(),
                    read_only_fields: read_only_fields.clone(),
                };

                // ReInkPy loads groups in order and lets a later duplicate
                // model replace an earlier one.
                models.insert(model, spec);
            }
        }

        Ok(Self { models })
    }

    pub fn get(&self, model: &str) -> Option<&EpsonSpec> {
        self.models.get(model)
    }

    /// Resolves a database specification from a normalized IEEE 1284 identity.
    pub fn resolve_identity(&self, identity: &PrinterIdentity) -> Option<&EpsonSpec> {
        identity.detected_model().and_then(|model| self.get(model))
    }

    pub fn models(&self) -> impl Iterator<Item = &str> {
        self.models.keys().map(String::as_str)
    }
}

#[derive(Deserialize)]
struct RawDatabase {
    #[serde(rename = "EPSON")]
    epson: Vec<RawSpec>,
}

#[derive(Deserialize)]
struct RawSpec {
    #[serde(default)]
    brand: Option<String>,
    #[serde(rename = "idVendor")]
    vendor_id: Option<u16>,
    #[serde(rename = "idProduct")]
    product_id: Option<u16>,
    #[serde(default)]
    rkey: u16,
    #[serde(default, rename = "wkey")]
    write_key: Option<String>,
    #[serde(default, rename = "wkey1")]
    shifted_write_key: Option<String>,
    #[serde(default = "default_address_length", rename = "rlen")]
    read_length: u8,
    #[serde(default = "default_address_length", rename = "wlen")]
    write_length: u8,
    #[serde(default, rename = "mem_low")]
    memory_low: u16,
    #[serde(default = "default_memory_high", rename = "mem_high")]
    memory_high: u16,
    #[serde(default)]
    mem: Vec<RawMemoryOperation>,
    #[serde(default)]
    read_only_fields: Vec<RawEepromField>,
    #[serde(default)]
    models: Vec<String>,
}

fn default_address_length() -> u8 {
    2
}

fn default_memory_high() -> u16 {
    0xff
}

#[derive(Deserialize)]
struct RawMemoryOperation {
    addr: Vec<u16>,
    desc: String,
    #[serde(default)]
    reset: Vec<u8>,
    #[serde(default)]
    min: Option<u32>,
}

impl TryFrom<RawMemoryOperation> for MemoryOperation {
    type Error = SpecError;

    fn try_from(raw: RawMemoryOperation) -> Result<Self, Self::Error> {
        if raw.addr.is_empty() {
            return Err(SpecError::EmptyMemoryOperation {
                description: raw.desc,
            });
        }
        if !raw.reset.is_empty() && raw.reset.len() != raw.addr.len() {
            return Err(SpecError::ResetLengthMismatch {
                description: raw.desc,
                addresses: raw.addr.len(),
                reset_values: raw.reset.len(),
            });
        }

        let reset_values_declared = !raw.reset.is_empty();
        Ok(Self {
            description: raw.desc,
            addresses: raw.addr,
            reset_values: raw.reset,
            reset_values_declared,
            minimum: raw.min,
        })
    }
}

#[derive(Deserialize)]
struct RawEepromField {
    #[serde(default)]
    address: Option<u16>,
    #[serde(default)]
    range: Option<[u16; 2]>,
    label: String,
    encoding: String,
    confidence: String,
    evidence: String,
    #[serde(default)]
    sensitive: bool,
}

impl RawEepromField {
    fn into_eeprom_field(
        self,
        memory_low: u16,
        memory_high: u16,
    ) -> Result<EepromField, SpecError> {
        let (address, end_address) = match (self.address, self.range) {
            (Some(address), None) => (address, address),
            (None, Some([address, end_address])) if address <= end_address => {
                (address, end_address)
            }
            (None, Some([address, end_address])) => {
                return Err(SpecError::InvalidReadOnlyFieldRange {
                    label: self.label,
                    address,
                    end_address,
                });
            }
            _ => {
                return Err(SpecError::InvalidReadOnlyFieldLocation { label: self.label });
            }
        };
        if address < memory_low || end_address > memory_high {
            return Err(SpecError::ReadOnlyFieldOutsideMemoryRange {
                label: self.label,
                address,
                end_address,
                memory_low,
                memory_high,
            });
        }

        let encoding = EepromFieldEncoding::from_raw(&self.label, &self.encoding)?;
        let byte_len = usize::from(end_address) - usize::from(address) + 1;
        if let Some(expected_len) = encoding.fixed_byte_len()
            && byte_len != expected_len
        {
            return Err(SpecError::ReadOnlyFieldLengthMismatch {
                label: self.label,
                encoding,
                byte_len,
            });
        }
        let confidence = EepromFieldConfidence::from_raw(&self.label, &self.confidence)?;

        Ok(EepromField {
            address,
            end_address,
            label: self.label,
            encoding,
            confidence,
            evidence_note: self.evidence,
            sensitive: self.sensitive,
        })
    }
}

fn validate_read_only_fields(fields: &[EepromField]) -> Result<(), SpecError> {
    for (index, field) in fields.iter().enumerate() {
        if let Some(overlap) = fields[..index]
            .iter()
            .find(|other| field.address <= other.end_address && other.address <= field.end_address)
        {
            return Err(SpecError::OverlappingReadOnlyFields {
                first: overlap.label.clone(),
                second: field.label.clone(),
            });
        }
    }
    Ok(())
}

fn latin1_bytes(value: &str) -> Result<Vec<u8>, SpecError> {
    value
        .chars()
        .map(|character| u8::try_from(character as u32).map_err(|_| SpecError::NonLatin1WriteKey))
        .collect()
}

/// Invalid or unsafe Epson model metadata.
#[derive(Debug)]
pub enum SpecError {
    Toml(toml::de::Error),
    InvalidAddressWidth {
        field: &'static str,
        value: u8,
    },
    InvalidMemoryRange {
        low: u16,
        high: u16,
    },
    InvalidWriteKeyLength {
        length: usize,
    },
    NonLatin1WriteKey,
    EmptyMemoryOperation {
        description: String,
    },
    ResetLengthMismatch {
        description: String,
        addresses: usize,
        reset_values: usize,
    },
    InvalidReadOnlyFieldLocation {
        label: String,
    },
    InvalidReadOnlyFieldRange {
        label: String,
        address: u16,
        end_address: u16,
    },
    ReadOnlyFieldOutsideMemoryRange {
        label: String,
        address: u16,
        end_address: u16,
        memory_low: u16,
        memory_high: u16,
    },
    InvalidReadOnlyFieldEncoding {
        label: String,
        encoding: String,
    },
    InvalidReadOnlyFieldConfidence {
        label: String,
        confidence: String,
    },
    ReadOnlyFieldLengthMismatch {
        label: String,
        encoding: EepromFieldEncoding,
        byte_len: usize,
    },
    OverlappingReadOnlyFields {
        first: String,
        second: String,
    },
}

impl fmt::Display for SpecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Toml(error) => write!(formatter, "invalid Epson TOML: {error}"),
            Self::InvalidAddressWidth { field, value } => {
                write!(formatter, "{field} must be one or two bytes, got {value}")
            }
            Self::InvalidMemoryRange { low, high } => {
                write!(formatter, "invalid EEPROM range {low:#06x}..={high:#06x}")
            }
            Self::InvalidWriteKeyLength { length } => {
                write!(formatter, "write key must be 8 bytes, got {length}")
            }
            Self::NonLatin1WriteKey => formatter.write_str("write key is not Latin-1"),
            Self::EmptyMemoryOperation { description } => {
                write!(
                    formatter,
                    "memory operation {description:?} has no addresses"
                )
            }
            Self::ResetLengthMismatch {
                description,
                addresses,
                reset_values,
            } => write!(
                formatter,
                "memory operation {description:?} has {addresses} addresses but {reset_values} reset values"
            ),
            Self::InvalidReadOnlyFieldLocation { label } => write!(
                formatter,
                "read-only EEPROM field {label:?} must declare exactly one address or range"
            ),
            Self::InvalidReadOnlyFieldRange {
                label,
                address,
                end_address,
            } => write!(
                formatter,
                "read-only EEPROM field {label:?} has invalid range {address:#06x}..={end_address:#06x}"
            ),
            Self::ReadOnlyFieldOutsideMemoryRange {
                label,
                address,
                end_address,
                memory_low,
                memory_high,
            } => write!(
                formatter,
                "read-only EEPROM field {label:?} range {address:#06x}..={end_address:#06x} is outside {memory_low:#06x}..={memory_high:#06x}"
            ),
            Self::InvalidReadOnlyFieldEncoding { label, encoding } => write!(
                formatter,
                "read-only EEPROM field {label:?} has unsupported encoding {encoding:?}"
            ),
            Self::InvalidReadOnlyFieldConfidence { label, confidence } => write!(
                formatter,
                "read-only EEPROM field {label:?} has unsupported confidence {confidence:?}"
            ),
            Self::ReadOnlyFieldLengthMismatch {
                label,
                encoding,
                byte_len,
            } => write!(
                formatter,
                "read-only EEPROM field {label:?} has {byte_len} byte(s), but {} requires {}",
                encoding.label(),
                encoding.fixed_byte_len().unwrap_or_default()
            ),
            Self::OverlappingReadOnlyFields { first, second } => write!(
                formatter,
                "read-only EEPROM fields {first:?} and {second:?} overlap"
            ),
        }
    }
}

impl Error for SpecError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Toml(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AddressWidth, CounterResetTarget, EepromFieldConfidence, EepromFieldEncoding,
        ModelDatabase, SpecError,
    };

    #[test]
    fn builtin_database_loads_known_model() {
        let database = ModelDatabase::builtin().unwrap();
        let spec = database.get("XP-352").unwrap();

        assert_eq!(spec.read_address_width, AddressWidth::Two);
        assert!(!spec.memory_operations.is_empty());
    }

    #[test]
    fn waste_counter_reset_merges_only_declared_operations() {
        let database = ModelDatabase::builtin().unwrap();
        let spec = database.get("C90").unwrap();
        let operation = spec.waste_counter_reset().unwrap();

        assert_eq!(
            operation.addresses,
            vec![0x06, 0x07, 0x0a, 0x0b, 0x16, 0x17, 0x34, 0x35, 0x0c, 0x0d]
        );
        assert_eq!(
            operation.reset_values,
            vec![0, 0, 0, 0, 0, 0, 4, 0x57, 1, 0xf4]
        );
        assert!(operation.has_declared_reset_values());
    }

    #[test]
    fn platen_resets_do_not_include_waste_operations() {
        let database = ModelDatabase::builtin().unwrap();
        let spec = database.get("XP-15000").unwrap();
        let operation = spec.counter_reset(CounterResetTarget::PlatenPad).unwrap();

        assert_eq!(operation.addresses, vec![0x40, 0x43, 0x44, 0x48, 0x1ed]);
        assert_eq!(operation.reset_values, vec![0, 0, 0, 0x5e, 0]);
        assert!(spec.counter_reset(CounterResetTarget::Waste).is_none());
    }

    #[test]
    fn missing_reset_values_remain_undeclared_and_are_not_zeroed() {
        let source = r#"
            [[EPSON]]
            models = ["Undeclared"]
            mem = [{ addr = [1, 2], desc = "Waste counter", min = 500 }]
        "#;
        let database = ModelDatabase::from_toml(source).unwrap();
        let operation = &database.get("Undeclared").unwrap().memory_operations[0];

        assert_eq!(operation.reset_values, Vec::<u8>::new());
        assert!(!operation.has_declared_reset_values());
        assert_eq!(operation.minimum, Some(500));
        assert!(
            database
                .get("Undeclared")
                .unwrap()
                .waste_counter_reset()
                .is_none()
        );
    }

    #[test]
    fn l1300_has_read_only_capture_metadata_without_changing_reset_semantics() {
        let database = ModelDatabase::builtin().unwrap();
        let l1300 = database.get("L1300").unwrap();
        let et_14000 = database.get("ET-14000").unwrap();

        assert!(et_14000.read_only_fields.is_empty());
        assert_eq!(l1300.read_only_fields.len(), 11);
        assert_eq!(
            l1300
                .read_only_fields
                .iter()
                .map(|field| {
                    (
                        field.address,
                        field.end_address,
                        field.encoding,
                        field.confidence,
                        field.sensitive,
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                (
                    0x26,
                    0x27,
                    EepromFieldEncoding::U16Le,
                    EepromFieldConfidence::Confirmed,
                    false,
                ),
                (
                    0x58,
                    0x58,
                    EepromFieldEncoding::RawBytes,
                    EepromFieldConfidence::RelatedUnknown,
                    false,
                ),
                (
                    0x16a,
                    0x16a,
                    EepromFieldEncoding::U8,
                    EepromFieldConfidence::StronglyRelated,
                    false,
                ),
                (
                    0x16e,
                    0x16e,
                    EepromFieldEncoding::U8,
                    EepromFieldConfidence::StronglyRelated,
                    false,
                ),
                (
                    0xb0,
                    0xb3,
                    EepromFieldEncoding::U32Le,
                    EepromFieldConfidence::Confirmed,
                    false,
                ),
                (
                    0xb4,
                    0xb7,
                    EepromFieldEncoding::U32Le,
                    EepromFieldConfidence::Confirmed,
                    false,
                ),
                (
                    0x4a,
                    0x4a,
                    EepromFieldEncoding::RawBytes,
                    EepromFieldConfidence::Confirmed,
                    false,
                ),
                (
                    0x5c,
                    0x5f,
                    EepromFieldEncoding::RawBytes,
                    EepromFieldConfidence::Confirmed,
                    false,
                ),
                (
                    0xc2,
                    0xcb,
                    EepromFieldEncoding::Ascii,
                    EepromFieldConfidence::Confirmed,
                    true,
                ),
                (
                    0x82,
                    0x85,
                    EepromFieldEncoding::RawBytes,
                    EepromFieldConfidence::StronglyRelated,
                    false,
                ),
                (
                    0x98,
                    0x98,
                    EepromFieldEncoding::RawBytes,
                    EepromFieldConfidence::StronglyRelated,
                    false,
                ),
            ]
        );

        let waste = l1300
            .read_only_fields
            .iter()
            .find(|field| field.address == 0x26)
            .unwrap();
        assert_eq!(waste.end_address, 0x27);
        assert_eq!(waste.byte_len(), 2);
        assert_eq!(waste.encoding, EepromFieldEncoding::U16Le);
        assert_eq!(waste.confidence, EepromFieldConfidence::Confirmed);

        let serial = l1300
            .read_only_fields
            .iter()
            .find(|field| field.address == 0xc2)
            .unwrap();
        assert_eq!(serial.end_address, 0xcb);
        assert_eq!(serial.encoding, EepromFieldEncoding::Ascii);
        assert!(serial.sensitive);

        let related_unknown = l1300
            .read_only_fields
            .iter()
            .find(|field| field.address == 0x58)
            .unwrap();
        assert_eq!(
            related_unknown.confidence,
            EepromFieldConfidence::RelatedUnknown
        );
        assert_eq!(related_unknown.encoding, EepromFieldEncoding::RawBytes);
    }

    #[test]
    fn rejects_read_only_field_with_encoding_length_mismatch() {
        let source = r#"
            [[EPSON]]
            models = ["Bad"]
            read_only_fields = [
              { range = [0x10, 0x12], label = "Bad counter", encoding = "u16le", confidence = "confirmed", evidence = "test" },
            ]
        "#;

        assert!(matches!(
            ModelDatabase::from_toml(source),
            Err(SpecError::ReadOnlyFieldLengthMismatch { .. })
        ));
    }

    #[test]
    fn rejects_reset_lengths_that_do_not_match_addresses() {
        let source = r#"
            [[EPSON]]
            models = ["Bad"]
            mem = [{ addr = [1, 2], desc = "Waste counter", reset = [0] }]
        "#;

        assert!(matches!(
            ModelDatabase::from_toml(source),
            Err(SpecError::ResetLengthMismatch { .. })
        ));
    }
}
