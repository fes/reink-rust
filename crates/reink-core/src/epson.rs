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
    use super::{AddressWidth, CounterResetTarget, ModelDatabase, SpecError};

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
