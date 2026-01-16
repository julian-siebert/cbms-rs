use std::{
    collections::HashMap,
    fmt::{self, Formatter},
    str,
};

use ciborium::{Value, cbor, from_reader, into_writer};
use serde::{Deserialize, Serialize};
use tokio::runtime::TryCurrentError;

#[cfg(feature = "quic")]
pub mod quic;
pub mod transport;
#[cfg(feature = "async")]
pub mod transport_async;

pub const PROTOCOL_VERSION: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub u128);

impl MessageId {
    pub fn new() -> Self {
        use rand::RngCore;
        let mut rng = rand::rng();
        MessageId(rng.next_u64() as u128 | ((rng.next_u64() as u128) << 64))
    }

    pub fn as_u128(&self) -> u128 {
        self.0
    }
}

impl Default for MessageId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", &self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageType {
    Req,
    Res,
    Push,
    Stream,
}

/// Status codes (sysexists.h inspired)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StatusCode {
    Success = 0,
    UsageError = 64,
    DataError = 65,
    NoInput = 66,
    NoUser = 67,
    NoHost = 68,
    Unavailable = 69,
    SoftwareError = 70,
    OSError = 71,
    OSFile = 72,
    IOError = 74,
    TempFail = 75,
    Protocol = 76,
    NoPerm = 77,
    ConfigError = 78,
    /// SIGINT
    Interrupt = 130,
    /// SIGQUIT
    Quit = 131,
    /// SIGILL
    Illegal = 132,
    /// SIGABRT
    Abort = 134,
    /// SIGKILL
    Kill = 137,
    /// SIGSEGV
    SegFault = 139,
    /// SIGTERM
    Terminate = 143,
}

impl StatusCode {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::TempFail | Self::Unavailable)
    }

    pub fn is_signal(&self) -> bool {
        matches!(
            self,
            Self::Interrupt
                | Self::Quit
                | Self::Illegal
                | Self::Abort
                | Self::Kill
                | Self::SegFault
                | Self::Terminate
        )
    }

    pub fn as_u8(&self) -> u8 {
        *self as u8
    }

    pub fn from_code(code: u8) -> Self {
        match code {
            0 => Self::Success,
            64 => Self::UsageError,
            65 => Self::DataError,
            66 => Self::NoInput,
            67 => Self::NoUser,
            68 => Self::NoHost,
            69 => Self::Unavailable,
            70 => Self::SoftwareError,
            71 => Self::OSError,
            72 => Self::OSFile,
            74 => Self::IOError,
            75 => Self::TempFail,
            76 => Self::Protocol,
            77 => Self::NoPerm,
            78 => Self::ConfigError,
            130 => Self::Interrupt,
            131 => Self::Quit,
            132 => Self::Illegal,
            134 => Self::Abort,
            137 => Self::Kill,
            139 => Self::SegFault,
            143 => Self::Terminate,
            _ => Self::Protocol,
        }
    }
}

impl Serialize for StatusCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8(self.as_u8())
    }
}

impl<'de> Deserialize<'de> for StatusCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let code = u8::deserialize(deserializer)?;
        Ok(StatusCode::from_code(code))
    }
}

pub type Metadata = HashMap<String, Value>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    #[serde(rename = "v")]
    pub version: u8,

    #[serde(rename = "id")]
    pub id: MessageId,

    #[serde(rename = "type")]
    pub msg_type: MessageType,

    #[serde(rename = "status", skip_serializing_if = "Option::is_none")]
    pub status: Option<StatusCode>,

    #[serde(rename = "method", skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,

    #[serde(rename = "meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Metadata>,

    #[serde(rename = "payload", skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,

    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<MessageId>,
}

impl Message {
    pub fn new_request(method: impl Into<String>, payload: Option<Value>) -> Self {
        Message {
            version: PROTOCOL_VERSION,
            id: MessageId::new(),
            msg_type: MessageType::Req,
            status: None,
            method: Some(method.into()),
            meta: None,
            payload,
            reference: None,
        }
    }

    pub fn new_response(request_id: MessageId, status: StatusCode, payload: Option<Value>) -> Self {
        Message {
            version: PROTOCOL_VERSION,
            id: MessageId::new(),
            msg_type: MessageType::Res,
            status: Some(status),
            method: None,
            meta: None,
            payload,
            reference: Some(request_id),
        }
    }

    pub fn success(request_id: MessageId, payload: Option<Value>) -> Self {
        Self::new_response(request_id, StatusCode::Success, payload)
    }

    pub fn error(request_id: MessageId, status: StatusCode, error: ErrorPayload) -> Self {
        let payload = error.to_value().ok();
        Self::new_response(request_id, status, payload)
    }

    pub fn new_push(method: impl Into<String>, payload: Option<Value>) -> Self {
        Message {
            version: PROTOCOL_VERSION,
            id: MessageId::new(),
            msg_type: MessageType::Push,
            status: None,
            method: Some(method.into()),
            meta: None,
            payload,
            reference: None,
        }
    }

    pub fn new_stream(
        stream_id: MessageId,
        payload: Option<Value>,
        status: Option<StatusCode>,
    ) -> Self {
        Message {
            version: PROTOCOL_VERSION,
            id: MessageId::new(),
            msg_type: MessageType::Stream,
            status,
            method: None,
            meta: None,
            payload,
            reference: Some(stream_id),
        }
    }

    pub fn with_meta(mut self, key: impl Into<String>, value: Value) -> Self {
        self.meta
            .get_or_insert_with(HashMap::new)
            .insert(key.into(), value);
        self
    }

    pub fn with_metadata(mut self, meta: Metadata) -> Self {
        match &mut self.meta {
            Some(existing) => existing.extend(meta),
            None => self.meta = Some(meta),
        }
        self
    }

    pub fn with_status(mut self, status: StatusCode) -> Self {
        self.status = Some(status);
        self
    }

    pub fn is_request(&self) -> bool {
        matches!(self.msg_type, MessageType::Req)
    }

    pub fn is_response(&self) -> bool {
        matches!(self.msg_type, MessageType::Res)
    }

    pub fn is_success(&self) -> bool {
        self.status
            .as_ref()
            .map(|s| s.is_success())
            .unwrap_or(false)
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.version != PROTOCOL_VERSION {
            return Err(Error::InvalidMessage(format!(
                "Unsupported protocol version: {}",
                self.version
            )));
        }

        match self.msg_type {
            MessageType::Req | MessageType::Push => {
                if self.method.is_none() {
                    return Err(Error::InvalidMessage(format!(
                        "{:?} messages require a method",
                        self.msg_type
                    )));
                }
                if self.reference.is_some() {
                    return Err(Error::InvalidMessage(format!(
                        "{:?} messages must not have a reference",
                        self.msg_type
                    )));
                }
            }
            MessageType::Res => {
                if self.status.is_none() {
                    return Err(Error::InvalidMessage(
                        "Response messages require a status".to_string(),
                    ));
                }
                if self.reference.is_none() {
                    return Err(Error::InvalidMessage(
                        "Response messages require a reference to a request".to_string(),
                    ));
                }
                if self.method.is_some() {
                    return Err(Error::InvalidMessage(
                        "Response messages must not have a method".to_string(),
                    ));
                }
            }
            MessageType::Stream => {
                if self.reference.is_none() {
                    return Err(Error::InvalidMessage(
                        "Stream messages require a reference".to_string(),
                    ));
                }
                if self.method.is_some() {
                    return Err(Error::InvalidMessage(
                        "Stream messages must not have a method".to_string(),
                    ));
                }
            }
        }

        if let Some(payload) = &self.payload {
            validate_value(payload)?;
        }

        Ok(())
    }

    pub fn to_cbor(&self) -> Result<Vec<u8>, Error> {
        let mut buffer = Vec::new();
        into_writer(self, &mut buffer)?;
        Ok(buffer)
    }

    pub fn from_cbor(bytes: &[u8]) -> Result<Self, Error> {
        Ok(from_reader(bytes)?)
    }
}

fn validate_value(value: &Value) -> Result<(), Error> {
    const MAX_DEPTH: usize = 64;
    const MAX_ENTRIES: usize = 1024;
    /// 16 MB
    const MAX_TEXT_LENGTH: usize = 16 * 1024 * 1024;
    /// 16 MB
    const MAX_BYTES_LENGTH: usize = 16 * 1024 * 1024;

    fn validate_inner(value: &Value, depth: usize, path: &str) -> Result<(), Error> {
        if depth > MAX_DEPTH {
            return Err(Error::InvalidMessage(format!(
                "CBOR nesting too deep at '{}': depth {} (max {})",
                path, depth, MAX_DEPTH
            )));
        }

        match value {
            Value::Array(items) => {
                let len = items.len();
                if len > MAX_ENTRIES {
                    return Err(Error::InvalidMessage(format!(
                        "CBOR array too large at '{}': {} entries (max {})",
                        path, len, MAX_ENTRIES
                    )));
                }

                for (i, item) in items.iter().enumerate() {
                    let item_path = format!("{}[{}]", path, i);
                    validate_inner(item, depth + 1, &item_path)?;
                }

                Ok(())
            }
            Value::Map(map) => {
                let len = map.len();
                if len > MAX_ENTRIES {
                    return Err(Error::InvalidMessage(format!(
                        "CBOR map too large at '{}': {} entries (max {})",
                        path, len, MAX_ENTRIES
                    )));
                }

                for (key, map_val) in map {
                    let key_str = match key {
                        Value::Text(text_string) => text_string.clone(),
                        Value::Integer(integer) => format!("{:?}", integer),
                        _ => "<invalid key>".into(),
                    };

                    let new_path = if path.is_empty() {
                        key_str.clone()
                    } else {
                        format!("{}.{}", path, key_str)
                    };

                    match key {
                        Value::Text(_) | Value::Integer(_) => {}
                        _ => {
                            return Err(Error::InvalidMessage(
                                "CBOR map keys must be Text or Integer".into(),
                            ));
                        }
                    }
                    validate_inner(map_val, depth + 1, &new_path)?;
                }

                Ok(())
            }
            Value::Tag(_, tag_value) => validate_inner(tag_value, depth + 1, path),
            Value::Integer(_) | Value::Float(_) | Value::Bool(_) | Value::Null => Ok(()),
            Value::Text(text_string) => {
                let len = text_string.len();
                if len > MAX_TEXT_LENGTH {
                    return Err(Error::InvalidMessage(format!(
                        "CBOR text too long at '{}': {} chars (max {})",
                        path, len, MAX_TEXT_LENGTH
                    )));
                }
                Ok(())
            }
            Value::Bytes(bytes) => {
                let len = bytes.len();
                if len > MAX_BYTES_LENGTH {
                    return Err(Error::InvalidMessage(format!(
                        "CBOR bytes too long at '{}': {} bytes (max {})",
                        path, len, MAX_BYTES_LENGTH
                    )));
                }
                Ok(())
            }
            _ => Err(Error::InvalidMessage(format!(
                "Unsupported CBOR value type at '{}'",
                path,
            ))),
        }
    }

    validate_inner(value, 0, "")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub code: String,
    pub message: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl ErrorPayload {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        ErrorPayload {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn from_status(status: StatusCode) -> Self {
        let description = match status {
            StatusCode::Success => "Success",
            StatusCode::UsageError => "Command usage error",
            StatusCode::DataError => "Data format error",
            StatusCode::NoInput => "Cannot open input",
            StatusCode::NoUser => "Addressee unknown",
            StatusCode::NoHost => "Unknown host",
            StatusCode::Unavailable => "Service unavailable",
            StatusCode::SoftwareError => "Internal software error",
            StatusCode::OSError => "System error",
            StatusCode::OSFile => "Critical OS file missing",
            StatusCode::IOError => "Input/Output error",
            StatusCode::TempFail => "Temporary failure",
            StatusCode::Protocol => "Protocol error",
            StatusCode::NoPerm => "Permission denied",
            StatusCode::ConfigError => "Configuration error",
            StatusCode::Interrupt => "Interrupted by signal",
            StatusCode::Quit => "Quit signal",
            StatusCode::Illegal => "Illegal instruction",
            StatusCode::Abort => "Aborted",
            StatusCode::Kill => "Killed",
            StatusCode::SegFault => "Segmentation fault",
            StatusCode::Terminate => "Terminated",
        }
        .to_string();
        ErrorPayload {
            code: status.as_u8().to_string(),
            message: description,
            details: None,
        }
    }

    pub fn to_value(&self) -> Result<Value, Error> {
        Ok(cbor!(&self)?)
    }
}

impl fmt::Display for ErrorPayload {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("CBOR serialization error: {0}")]
    CborSer(#[from] ciborium::ser::Error<std::io::Error>),

    #[error("CBOR deserialization error: {0}")]
    CborDe(#[from] ciborium::de::Error<std::io::Error>),

    #[error("CBOR value error: {0}")]
    Cbor(#[from] ciborium::value::Error),

    #[error("Request timed out after {0} ms")]
    Timeout(u64),

    #[error("Response channel closed unexpectedly")]
    ChannelClosed,

    #[error("Invalid message received: {0}")]
    InvalidMessage(String),

    #[error("Invalid frame: {0}")]
    InvalidFrame(String),

    #[error("Hashing failed: {0}")]
    HashError(String),

    #[error("Tokio try current error: {0}")]
    TryCurrent(#[from] TryCurrentError),

    #[error("External error: {0}")]
    External(#[from] Box<dyn std::error::Error + Send + Sync>),
}

impl std::error::Error for ErrorPayload {}
