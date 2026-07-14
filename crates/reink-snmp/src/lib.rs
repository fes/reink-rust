#![forbid(unsafe_code)]
//! A synchronous SNMP control channel for Epson printers.
//!
//! Epson control requests are exposed through enterprise OIDs. This crate maps
//! each request byte to one OID arc and returns the octet string from the SNMP
//! GET response.

use std::error::Error;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use reink_core::{IdentityParseError, PrinterIdentity};
use reink_platform::{ControlChannel, ControlError, TransportError, TransportErrorKind};
use snmp2::{Oid, SyncSession, Value, v3};

/// The prefix of Epson's SNMP control OIDs.
pub const EPSON_CONTROL_OID_PREFIX: &[u64] = &[1, 3, 6, 1, 4, 1, 1248, 1, 2, 2, 44, 1, 1, 2, 1];

/// The OID that contains the printer's IEEE 1284 device ID.
pub const IEEE_1284_DEVICE_ID_OID: &[u64] = &[1, 3, 6, 1, 4, 1, 2699, 1, 2, 1, 2, 1, 1, 3, 1];

/// Builds the Epson enterprise OID for an encoded Epson control request.
pub fn epson_control_oid(request: &[u8]) -> Vec<u64> {
    let mut oid = Vec::with_capacity(EPSON_CONTROL_OID_PREFIX.len() + request.len());
    oid.extend_from_slice(EPSON_CONTROL_OID_PREFIX);
    oid.extend(request.iter().map(|&byte| u64::from(byte)));
    oid
}

/// An SNMP server address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnmpEndpoint {
    host: String,
    port: u16,
}

impl SnmpEndpoint {
    /// Creates an endpoint after validating its host name and UDP port.
    pub fn new(host: impl Into<String>, port: u16) -> Result<Self, ConfigError> {
        let host = host.into();
        if host.trim().is_empty() {
            return Err(ConfigError::EmptyHost);
        }
        if port == 0 {
            return Err(ConfigError::ZeroPort);
        }
        Ok(Self { host, port })
    }

    /// Returns the configured host name or IP address.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Returns the configured UDP port.
    pub fn port(&self) -> u16 {
        self.port
    }

    fn socket_address(&self) -> String {
        if self.host.starts_with('[') && self.host.ends_with(']') {
            format!("{}:{}", self.host, self.port)
        } else if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// An SNMP protocol version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnmpVersion {
    V1,
    V2c,
    V3,
}

impl FromStr for SnmpVersion {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "1" | "v1" => Ok(Self::V1),
            "2c" | "v2c" => Ok(Self::V2c),
            "3" | "v3" => Ok(Self::V3),
            _ => Err(ConfigError::InvalidEnvironmentValue {
                variable: "REINK_SNMP_VERSION",
                expected: "1, 2c, or 3",
            }),
        }
    }
}

/// A community string, deliberately redacted from debug output.
#[derive(Clone, Eq, PartialEq)]
pub struct Community(Vec<u8>);

impl Community {
    /// Creates a community string from arbitrary bytes.
    pub fn new(community: impl AsRef<[u8]>) -> Self {
        Self(community.as_ref().to_vec())
    }
}

impl fmt::Debug for Community {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Community(REDACTED)")
    }
}

/// A supported SNMPv3 authentication algorithm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnmpV3AuthProtocol {
    Md5,
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
}

impl SnmpV3AuthProtocol {
    fn into_snmp2(self) -> v3::AuthProtocol {
        match self {
            Self::Md5 => v3::AuthProtocol::Md5,
            Self::Sha1 => v3::AuthProtocol::Sha1,
            Self::Sha224 => v3::AuthProtocol::Sha224,
            Self::Sha256 => v3::AuthProtocol::Sha256,
            Self::Sha384 => v3::AuthProtocol::Sha384,
            Self::Sha512 => v3::AuthProtocol::Sha512,
        }
    }
}

impl FromStr for SnmpV3AuthProtocol {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "md5" => Ok(Self::Md5),
            "sha1" => Ok(Self::Sha1),
            "sha224" => Ok(Self::Sha224),
            "sha256" => Ok(Self::Sha256),
            "sha384" => Ok(Self::Sha384),
            "sha512" => Ok(Self::Sha512),
            _ => Err(ConfigError::InvalidEnvironmentValue {
                variable: "REINK_SNMP_AUTH_PROTOCOL",
                expected: "md5, sha1, sha224, sha256, sha384, or sha512",
            }),
        }
    }
}

/// A supported SNMPv3 privacy algorithm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnmpV3PrivacyProtocol {
    Des,
    Aes128,
    Aes192,
    Aes256,
}

impl SnmpV3PrivacyProtocol {
    fn into_snmp2(self) -> v3::Cipher {
        match self {
            Self::Des => v3::Cipher::Des,
            Self::Aes128 => v3::Cipher::Aes128,
            Self::Aes192 => v3::Cipher::Aes192,
            Self::Aes256 => v3::Cipher::Aes256,
        }
    }
}

impl FromStr for SnmpV3PrivacyProtocol {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "des" => Ok(Self::Des),
            "aes128" => Ok(Self::Aes128),
            "aes192" => Ok(Self::Aes192),
            "aes256" => Ok(Self::Aes256),
            _ => Err(ConfigError::InvalidEnvironmentValue {
                variable: "REINK_SNMP_PRIVACY_PROTOCOL",
                expected: "des, aes128, aes192, or aes256",
            }),
        }
    }
}

/// SNMPv3 USM credentials, deliberately redacted from debug output.
#[derive(Clone, Eq, PartialEq)]
pub struct UsmCredentials {
    username: Vec<u8>,
    security: UsmSecurity,
}

#[derive(Clone, Eq, PartialEq)]
enum UsmSecurity {
    NoAuthentication,
    Authentication {
        protocol: SnmpV3AuthProtocol,
        authentication_password: Vec<u8>,
    },
    AuthenticationAndPrivacy {
        protocol: SnmpV3AuthProtocol,
        authentication_password: Vec<u8>,
        privacy_protocol: SnmpV3PrivacyProtocol,
        privacy_password: Vec<u8>,
    },
}

impl UsmCredentials {
    /// Creates SNMPv3 credentials without authentication or privacy.
    pub fn no_auth_no_priv(username: impl AsRef<[u8]>) -> Self {
        Self {
            username: username.as_ref().to_vec(),
            security: UsmSecurity::NoAuthentication,
        }
    }

    /// Creates authenticated SNMPv3 credentials without privacy.
    pub fn auth_no_priv(
        username: impl AsRef<[u8]>,
        protocol: SnmpV3AuthProtocol,
        authentication_password: impl AsRef<[u8]>,
    ) -> Self {
        Self {
            username: username.as_ref().to_vec(),
            security: UsmSecurity::Authentication {
                protocol,
                authentication_password: authentication_password.as_ref().to_vec(),
            },
        }
    }

    /// Creates authenticated and private SNMPv3 credentials.
    pub fn auth_priv(
        username: impl AsRef<[u8]>,
        protocol: SnmpV3AuthProtocol,
        authentication_password: impl AsRef<[u8]>,
        privacy_protocol: SnmpV3PrivacyProtocol,
        privacy_password: impl AsRef<[u8]>,
    ) -> Self {
        Self {
            username: username.as_ref().to_vec(),
            security: UsmSecurity::AuthenticationAndPrivacy {
                protocol,
                authentication_password: authentication_password.as_ref().to_vec(),
                privacy_protocol,
                privacy_password: privacy_password.as_ref().to_vec(),
            },
        }
    }

    fn into_security(self) -> v3::Security {
        match self.security {
            UsmSecurity::NoAuthentication => {
                v3::Security::new(&self.username, &[]).with_auth(v3::Auth::NoAuthNoPriv)
            }
            UsmSecurity::Authentication {
                protocol,
                authentication_password,
            } => v3::Security::new(&self.username, &authentication_password)
                .with_auth_protocol(protocol.into_snmp2())
                .with_auth(v3::Auth::AuthNoPriv),
            UsmSecurity::AuthenticationAndPrivacy {
                protocol,
                authentication_password,
                privacy_protocol,
                privacy_password,
            } => v3::Security::new(&self.username, &authentication_password)
                .with_auth_protocol(protocol.into_snmp2())
                .with_auth(v3::Auth::AuthPriv {
                    cipher: privacy_protocol.into_snmp2(),
                    privacy_password,
                }),
        }
    }
}

impl fmt::Debug for UsmCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("UsmCredentials(REDACTED)")
    }
}

/// Authentication material appropriate for an SNMP version.
#[derive(Clone, Eq, PartialEq)]
pub enum SnmpAuth {
    Community(Community),
    Usm(UsmCredentials),
}

impl SnmpAuth {
    /// Creates community-based authentication for SNMPv1 or SNMPv2c.
    pub fn community(community: impl AsRef<[u8]>) -> Self {
        Self::Community(Community::new(community))
    }

    /// Creates USM authentication for SNMPv3.
    pub fn usm(credentials: UsmCredentials) -> Self {
        Self::Usm(credentials)
    }
}

impl fmt::Debug for SnmpAuth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SnmpAuth(REDACTED)")
    }
}

/// Typed configuration for a synchronous SNMP session.
#[derive(Clone, Eq, PartialEq)]
pub struct SnmpConfig {
    endpoint: SnmpEndpoint,
    version: SnmpVersion,
    authentication: SnmpAuth,
    timeout: Duration,
    starting_request_id: i32,
}

impl SnmpConfig {
    /// Creates a configuration and rejects version/authentication mismatches.
    pub fn new(
        endpoint: SnmpEndpoint,
        version: SnmpVersion,
        authentication: SnmpAuth,
    ) -> Result<Self, ConfigError> {
        let compatible = matches!(
            (version, &authentication),
            (SnmpVersion::V1 | SnmpVersion::V2c, SnmpAuth::Community(_))
                | (SnmpVersion::V3, SnmpAuth::Usm(_))
        );
        if !compatible {
            return Err(ConfigError::AuthenticationVersionMismatch);
        }
        Ok(Self {
            endpoint,
            version,
            authentication,
            timeout: Duration::from_secs(2),
            starting_request_id: 0,
        })
    }

    /// Configures SNMPv1 community authentication.
    pub fn v1(endpoint: SnmpEndpoint, community: impl AsRef<[u8]>) -> Self {
        Self::new(endpoint, SnmpVersion::V1, SnmpAuth::community(community))
            .expect("SNMPv1 is compatible with community authentication")
    }

    /// Configures SNMPv2c community authentication.
    pub fn v2c(endpoint: SnmpEndpoint, community: impl AsRef<[u8]>) -> Self {
        Self::new(endpoint, SnmpVersion::V2c, SnmpAuth::community(community))
            .expect("SNMPv2c is compatible with community authentication")
    }

    /// Configures SNMPv3 USM authentication.
    pub fn v3(endpoint: SnmpEndpoint, credentials: UsmCredentials) -> Self {
        Self::new(endpoint, SnmpVersion::V3, SnmpAuth::usm(credentials))
            .expect("SNMPv3 is compatible with USM authentication")
    }

    /// Loads a configuration from `REINK_SNMP_*` environment variables.
    ///
    /// Required variables are `REINK_SNMP_HOST` and `REINK_SNMP_VERSION`.
    /// Community versions require `REINK_SNMP_COMMUNITY`; version 3 requires
    /// `REINK_SNMP_USERNAME`. Credentials are never included in errors.
    pub fn from_environment() -> Result<Self, ConfigError> {
        Self::from_environment_with(|name| std::env::var(name).ok())
    }

    fn from_environment_with(
        get: impl Fn(&'static str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let host = required_environment(&get, "REINK_SNMP_HOST")?;
        let port = optional_environment_u16(&get, "REINK_SNMP_PORT", 161)?;
        let version = required_environment(&get, "REINK_SNMP_VERSION")?.parse()?;
        let endpoint = SnmpEndpoint::new(host, port)?;
        let timeout_seconds = optional_environment_u64(&get, "REINK_SNMP_TIMEOUT_SECONDS", 2)?;

        let config = match version {
            SnmpVersion::V1 => Self::v1(
                endpoint,
                required_environment(&get, "REINK_SNMP_COMMUNITY")?,
            ),
            SnmpVersion::V2c => Self::v2c(
                endpoint,
                required_environment(&get, "REINK_SNMP_COMMUNITY")?,
            ),
            SnmpVersion::V3 => {
                let username = required_environment(&get, "REINK_SNMP_USERNAME")?;
                let credentials = match (
                    get("REINK_SNMP_AUTH_PROTOCOL"),
                    get("REINK_SNMP_AUTH_PASSWORD"),
                    get("REINK_SNMP_PRIVACY_PROTOCOL"),
                    get("REINK_SNMP_PRIVACY_PASSWORD"),
                ) {
                    (None, None, None, None) => UsmCredentials::no_auth_no_priv(username),
                    (Some(protocol), Some(password), None, None) => {
                        UsmCredentials::auth_no_priv(username, protocol.parse()?, password)
                    }
                    (
                        Some(auth_protocol),
                        Some(auth_password),
                        Some(privacy_protocol),
                        Some(privacy_password),
                    ) => UsmCredentials::auth_priv(
                        username,
                        auth_protocol.parse()?,
                        auth_password,
                        privacy_protocol.parse()?,
                        privacy_password,
                    ),
                    _ => return Err(ConfigError::IncompleteV3SecurityConfiguration),
                };
                Self::v3(endpoint, credentials)
            }
        };

        Ok(config.with_timeout(Duration::from_secs(timeout_seconds)))
    }

    /// Sets the read and write timeout used by the session.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the initial SNMP request ID.
    pub fn with_starting_request_id(mut self, starting_request_id: i32) -> Self {
        self.starting_request_id = starting_request_id;
        self
    }

    /// Returns the non-secret endpoint.
    pub fn endpoint(&self) -> &SnmpEndpoint {
        &self.endpoint
    }

    /// Returns the configured protocol version.
    pub fn version(&self) -> SnmpVersion {
        self.version
    }
}

impl fmt::Debug for SnmpConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SnmpConfig")
            .field("endpoint", &self.endpoint)
            .field("version", &self.version)
            .field("authentication", &"REDACTED")
            .field("timeout", &self.timeout)
            .field("starting_request_id", &self.starting_request_id)
            .finish()
    }
}

/// Invalid SNMP connection configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigError {
    EmptyHost,
    ZeroPort,
    AuthenticationVersionMismatch,
    MissingEnvironmentVariable {
        variable: &'static str,
    },
    InvalidEnvironmentValue {
        variable: &'static str,
        expected: &'static str,
    },
    IncompleteV3SecurityConfiguration,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyHost => formatter.write_str("SNMP host must not be empty"),
            Self::ZeroPort => formatter.write_str("SNMP UDP port must not be zero"),
            Self::AuthenticationVersionMismatch => {
                formatter.write_str("SNMP version and authentication type do not match")
            }
            Self::MissingEnvironmentVariable { variable } => {
                write!(formatter, "required environment variable {variable} is not set")
            }
            Self::InvalidEnvironmentValue { variable, expected } => {
                write!(formatter, "{variable} must be {expected}")
            }
            Self::IncompleteV3SecurityConfiguration => formatter.write_str(
                "SNMPv3 authentication and privacy protocol/password variables must be configured in matching pairs",
            ),
        }
    }
}

impl Error for ConfigError {}

fn required_environment(
    get: &impl Fn(&'static str) -> Option<String>,
    variable: &'static str,
) -> Result<String, ConfigError> {
    get(variable)
        .filter(|value| !value.is_empty())
        .ok_or(ConfigError::MissingEnvironmentVariable { variable })
}

fn optional_environment_u16(
    get: &impl Fn(&'static str) -> Option<String>,
    variable: &'static str,
    default: u16,
) -> Result<u16, ConfigError> {
    get(variable).map_or(Ok(default), |value| {
        value
            .parse()
            .map_err(|_| ConfigError::InvalidEnvironmentValue {
                variable,
                expected: "a valid unsigned 16-bit integer",
            })
    })
}

fn optional_environment_u64(
    get: &impl Fn(&'static str) -> Option<String>,
    variable: &'static str,
    default: u64,
) -> Result<u64, ConfigError> {
    get(variable).map_or(Ok(default), |value| {
        value
            .parse()
            .map_err(|_| ConfigError::InvalidEnvironmentValue {
                variable,
                expected: "a valid unsigned integer",
            })
    })
}

/// An error returned while opening an SNMP session or performing an SNMP GET.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SnmpError {
    Transport {
        operation: &'static str,
        message: String,
    },
    Response {
        message: String,
    },
}

impl SnmpError {
    /// Creates a transport failure, suitable for scripted test backends.
    pub fn transport(operation: &'static str, message: impl Into<String>) -> Self {
        Self::Transport {
            operation,
            message: message.into(),
        }
    }

    /// Creates a malformed or rejected-response failure for scripted backends.
    pub fn response(message: impl Into<String>) -> Self {
        Self::Response {
            message: message.into(),
        }
    }

    fn into_control_error(self) -> ControlError {
        match self {
            Self::Transport { operation, message } => ControlError::Transport(TransportError::new(
                TransportErrorKind::Io,
                operation,
                message,
            )),
            Self::Response { message } => ControlError::Protocol { message },
        }
    }
}

impl fmt::Display for SnmpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport { operation, message } => {
                write!(formatter, "{operation} failed: {message}")
            }
            Self::Response { message } => write!(formatter, "invalid SNMP response: {message}"),
        }
    }
}

impl Error for SnmpError {}

/// A minimal synchronous SNMP GET backend.
///
/// This trait isolates the network client so control and identity behavior can
/// be tested with deterministic responses rather than a printer on the network.
pub trait SnmpGetBackend: Send {
    /// Returns the OCTET STRING value of a GET request for `oid`.
    fn get_octets(&mut self, oid: &[u64]) -> Result<Vec<u8>, SnmpError>;
}

/// The real `snmp2` synchronous GET backend.
pub struct Snmp2Backend {
    session: SyncSession,
}

impl Snmp2Backend {
    /// Connects to an SNMP endpoint and initializes SNMPv3 USM when selected.
    pub fn connect(config: SnmpConfig) -> Result<Self, SnmpError> {
        let destination = config.endpoint.socket_address();
        let mut session = match (config.version, config.authentication) {
            (SnmpVersion::V1, SnmpAuth::Community(community)) => SyncSession::new_v1(
                destination.as_str(),
                &community.0,
                Some(config.timeout),
                config.starting_request_id,
            ),
            (SnmpVersion::V2c, SnmpAuth::Community(community)) => SyncSession::new_v2c(
                destination.as_str(),
                &community.0,
                Some(config.timeout),
                config.starting_request_id,
            ),
            (SnmpVersion::V3, SnmpAuth::Usm(credentials)) => SyncSession::new_v3(
                destination.as_str(),
                Some(config.timeout),
                config.starting_request_id,
                credentials.into_security(),
            ),
            _ => {
                return Err(SnmpError::response(
                    "SNMP version and authentication type do not match",
                ));
            }
        }
        .map_err(|error| SnmpError::transport("open SNMP session", error.to_string()))?;

        session
            .init()
            .map_err(|error| SnmpError::transport("initialize SNMP session", error.to_string()))?;
        Ok(Self { session })
    }

    fn get_response_octets(&mut self, oid: &[u64]) -> Result<Vec<u8>, SnmpError> {
        let oid = Oid::from(oid)
            .map_err(|error| SnmpError::response(format!("invalid requested OID: {error:?}")))?;
        let mut response = match self.session.get(&oid) {
            Err(snmp2::Error::AuthUpdated) => self.session.get(&oid),
            response => response,
        }
        .map_err(|error| SnmpError::transport("SNMP GET", error.to_string()))?;

        if response.error_status != 0 {
            return Err(SnmpError::response(format!(
                "agent returned status {} at index {}",
                response.error_status, response.error_index
            )));
        }

        match response.varbinds.next() {
            Some((_oid, Value::OctetString(octets))) => Ok(octets.to_vec()),
            Some((_oid, _)) => Err(SnmpError::response(
                "GET response value is not an OCTET STRING",
            )),
            None => Err(SnmpError::response(
                "GET response contains no variable bindings",
            )),
        }
    }
}

impl SnmpGetBackend for Snmp2Backend {
    fn get_octets(&mut self, oid: &[u64]) -> Result<Vec<u8>, SnmpError> {
        self.get_response_octets(oid)
    }
}

/// An SNMP implementation of the platform control-channel contract.
pub struct SnmpControlChannel<B = Snmp2Backend> {
    backend: B,
}

impl<B> SnmpControlChannel<B> {
    /// Wraps a GET backend, primarily for deterministic tests and custom clients.
    pub fn with_backend(backend: B) -> Self {
        Self { backend }
    }

    /// Returns the wrapped GET backend.
    pub fn into_backend(self) -> B {
        self.backend
    }
}

impl SnmpControlChannel<Snmp2Backend> {
    /// Opens a real synchronous SNMP control channel.
    pub fn connect(config: SnmpConfig) -> Result<Self, SnmpError> {
        Snmp2Backend::connect(config).map(Self::with_backend)
    }
}

impl<B: SnmpGetBackend> SnmpControlChannel<B> {
    /// Reads and parses the printer's IEEE 1284 device identifier.
    pub fn printer_identity(&mut self) -> Result<PrinterIdentity, DeviceIdentityError> {
        read_printer_identity(&mut self.backend)
    }
}

impl<B: SnmpGetBackend> ControlChannel for SnmpControlChannel<B> {
    fn request(&mut self, request: &[u8]) -> Result<Vec<u8>, ControlError> {
        self.backend
            .get_octets(&epson_control_oid(request))
            .map_err(SnmpError::into_control_error)
    }
}

/// Reads and parses an IEEE 1284 device ID through an SNMP GET backend.
pub fn read_printer_identity<B: SnmpGetBackend>(
    backend: &mut B,
) -> Result<PrinterIdentity, DeviceIdentityError> {
    let bytes = backend
        .get_octets(IEEE_1284_DEVICE_ID_OID)
        .map_err(DeviceIdentityError::Get)?;
    let identifier = std::str::from_utf8(&bytes).map_err(|_| DeviceIdentityError::NonUtf8)?;
    PrinterIdentity::parse(identifier).map_err(DeviceIdentityError::Parse)
}

/// A failure while obtaining or parsing a printer's IEEE 1284 device ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceIdentityError {
    Get(SnmpError),
    NonUtf8,
    Parse(IdentityParseError),
}

impl fmt::Display for DeviceIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Get(error) => write!(formatter, "could not read IEEE 1284 device ID: {error}"),
            Self::NonUtf8 => formatter.write_str("IEEE 1284 device ID is not UTF-8"),
            Self::Parse(error) => write!(formatter, "invalid IEEE 1284 device ID: {error}"),
        }
    }
}

impl Error for DeviceIdentityError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Get(error) => Some(error),
            Self::NonUtf8 => None,
            Self::Parse(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use reink_platform::{ControlChannel, ControlError, TransportErrorKind};

    use super::{
        ConfigError, DeviceIdentityError, EPSON_CONTROL_OID_PREFIX, IEEE_1284_DEVICE_ID_OID,
        SnmpAuth, SnmpConfig, SnmpControlChannel, SnmpEndpoint, SnmpError, SnmpGetBackend,
        SnmpVersion, epson_control_oid, read_printer_identity,
    };

    #[derive(Default)]
    struct ScriptedBackend {
        steps: VecDeque<ScriptedGet>,
    }

    type ScriptedGet = (Vec<u64>, Result<Vec<u8>, SnmpError>);

    impl ScriptedBackend {
        fn expect(&mut self, oid: impl Into<Vec<u64>>, response: Result<Vec<u8>, SnmpError>) {
            self.steps.push_back((oid.into(), response));
        }

        fn assert_finished(&self) {
            assert!(
                self.steps.is_empty(),
                "{} expected GET(s) were not performed",
                self.steps.len()
            );
        }
    }

    impl SnmpGetBackend for ScriptedBackend {
        fn get_octets(&mut self, oid: &[u64]) -> Result<Vec<u8>, SnmpError> {
            match self.steps.pop_front() {
                Some((expected, result)) if expected == oid => result,
                Some((expected, _)) => Err(SnmpError::response(format!(
                    "unexpected OID {oid:?}; expected {expected:?}"
                ))),
                None => Err(SnmpError::response(format!(
                    "unexpected OID {oid:?}; no GET was expected"
                ))),
            }
        }
    }

    #[test]
    fn epson_control_oid_appends_each_request_byte_as_an_arc() {
        let oid = epson_control_oid(&[0x00, 0x2a, 0xff]);

        assert_eq!(
            &oid[..EPSON_CONTROL_OID_PREFIX.len()],
            EPSON_CONTROL_OID_PREFIX
        );
        assert_eq!(&oid[EPSON_CONTROL_OID_PREFIX.len()..], &[0, 42, 255]);
    }

    #[test]
    fn control_channel_forwards_get_octets() {
        let request = [0x10, 0x20];
        let mut backend = ScriptedBackend::default();
        backend.expect(epson_control_oid(&request), Ok(vec![0xaa, 0xbb]));
        let mut channel = SnmpControlChannel::with_backend(backend);

        assert_eq!(channel.request(&request).unwrap(), vec![0xaa, 0xbb]);
        channel.into_backend().assert_finished();
    }

    #[test]
    fn environment_configuration_uses_community_without_exposing_it() {
        let config = SnmpConfig::from_environment_with(|name| match name {
            "REINK_SNMP_HOST" => Some("printer.local".to_owned()),
            "REINK_SNMP_VERSION" => Some("2c".to_owned()),
            "REINK_SNMP_COMMUNITY" => Some("private-community".to_owned()),
            _ => None,
        })
        .unwrap();

        assert_eq!(config.endpoint().host(), "printer.local");
        assert_eq!(config.endpoint().port(), 161);
        assert_eq!(config.version(), SnmpVersion::V2c);
        assert!(!format!("{config:?}").contains("private-community"));
    }

    #[test]
    fn environment_configuration_requires_complete_v3_security_pairs() {
        let error = SnmpConfig::from_environment_with(|name| match name {
            "REINK_SNMP_HOST" => Some("printer.local".to_owned()),
            "REINK_SNMP_VERSION" => Some("3".to_owned()),
            "REINK_SNMP_USERNAME" => Some("operator".to_owned()),
            "REINK_SNMP_AUTH_PROTOCOL" => Some("sha256".to_owned()),
            _ => None,
        })
        .unwrap_err();

        assert_eq!(error, ConfigError::IncompleteV3SecurityConfiguration);
    }

    #[test]
    fn control_channel_preserves_transport_and_response_errors() {
        let mut transport_backend = ScriptedBackend::default();
        transport_backend.expect(
            epson_control_oid(b"one"),
            Err(SnmpError::transport("SNMP GET", "timed out")),
        );
        let mut transport_channel = SnmpControlChannel::with_backend(transport_backend);
        let error = transport_channel.request(b"one").unwrap_err();
        assert!(matches!(
            error,
            ControlError::Transport(ref error) if error.kind == TransportErrorKind::Io
        ));

        let mut response_backend = ScriptedBackend::default();
        response_backend.expect(
            epson_control_oid(b"two"),
            Err(SnmpError::response("unexpected ASN.1 type")),
        );
        let mut response_channel = SnmpControlChannel::with_backend(response_backend);
        assert!(matches!(
            response_channel.request(b"two"),
            Err(ControlError::Protocol { .. })
        ));
    }

    #[test]
    fn reads_and_parses_ieee_1284_identity() {
        let mut backend = ScriptedBackend::default();
        backend.expect(
            IEEE_1284_DEVICE_ID_OID.to_vec(),
            Ok(b"MFG:EPSON;MDL:XP-4100;CMD:ESCPL2,BDC;SN:123;".to_vec()),
        );

        let identity = read_printer_identity(&mut backend).unwrap();
        assert_eq!(identity.manufacturer(), Some("EPSON"));
        assert_eq!(identity.model(), Some("XP-4100"));
        assert_eq!(identity.serial_number(), Some("123"));
        assert_eq!(identity.command_set(), ["ESCPL2", "BDC"]);
        backend.assert_finished();
    }

    #[test]
    fn identity_read_reports_get_and_parse_errors() {
        let mut get_error = ScriptedBackend::default();
        get_error.expect(
            IEEE_1284_DEVICE_ID_OID.to_vec(),
            Err(SnmpError::transport("SNMP GET", "timed out")),
        );
        assert!(matches!(
            read_printer_identity(&mut get_error),
            Err(DeviceIdentityError::Get(SnmpError::Transport { .. }))
        ));

        let mut non_utf8 = ScriptedBackend::default();
        non_utf8.expect(IEEE_1284_DEVICE_ID_OID.to_vec(), Ok(vec![0xff]));
        assert_eq!(
            read_printer_identity(&mut non_utf8).unwrap_err(),
            DeviceIdentityError::NonUtf8
        );

        let mut malformed = ScriptedBackend::default();
        malformed.expect(
            IEEE_1284_DEVICE_ID_OID.to_vec(),
            Ok(b"MFG:EPSON;bad;".to_vec()),
        );
        assert!(matches!(
            read_printer_identity(&mut malformed),
            Err(DeviceIdentityError::Parse(_))
        ));
    }

    #[test]
    fn config_is_typed_and_redacts_authentication() {
        let endpoint = SnmpEndpoint::new("192.0.2.10", 161).unwrap();
        let config = SnmpConfig::v2c(endpoint.clone(), b"top-secret");
        assert_eq!(config.endpoint(), &endpoint);
        assert_eq!(config.version(), SnmpVersion::V2c);
        assert!(!format!("{config:?}").contains("top-secret"));

        assert_eq!(
            SnmpConfig::new(endpoint, SnmpVersion::V3, SnmpAuth::community(b"public")).unwrap_err(),
            ConfigError::AuthenticationVersionMismatch
        );
    }
}
