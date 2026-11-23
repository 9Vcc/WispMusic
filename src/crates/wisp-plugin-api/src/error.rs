use serde::{Deserialize, Serialize};

/// Frozen error codes on the plug-in boundary. The host maps them back to its own error kinds;
/// a plugin never sends a host error object across the ABI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u32)]
pub enum ErrorCode {
    Ok = 0,
    /// The op is not implemented; the host falls back to its default implementation.
    Unsupported = 1,
    Auth = 2,
    Throttled = 3,
    NotFound = 4,
    Network = 5,
    BadRequest = 6,
    TooLarge = 7,
    Timeout = 8,
    /// The platform refuses this track (copyright or region).
    Refused = 9,
    Internal = 10,
}

impl ErrorCode {
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    pub const fn from_u32(code: u32) -> Option<Self> {
        Some(match code {
            0 => Self::Ok,
            1 => Self::Unsupported,
            2 => Self::Auth,
            3 => Self::Throttled,
            4 => Self::NotFound,
            5 => Self::Network,
            6 => Self::BadRequest,
            7 => Self::TooLarge,
            8 => Self::Timeout,
            9 => Self::Refused,
            10 => Self::Internal,
            _ => return None,
        })
    }
}

/// The only error shape crossing the ABI: a frozen code plus a human-readable message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
}

impl PluginError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after_ms: None,
        }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unsupported, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_code_round_trips_through_its_number() {
        for code in [
            ErrorCode::Ok,
            ErrorCode::Unsupported,
            ErrorCode::Auth,
            ErrorCode::Throttled,
            ErrorCode::NotFound,
            ErrorCode::Network,
            ErrorCode::BadRequest,
            ErrorCode::TooLarge,
            ErrorCode::Timeout,
            ErrorCode::Refused,
            ErrorCode::Internal,
        ] {
            assert_eq!(ErrorCode::from_u32(code.as_u32()), Some(code));
        }
        assert_eq!(ErrorCode::from_u32(11), None);
    }

    #[test]
    fn an_error_round_trips_as_json() {
        let error = PluginError {
            code: ErrorCode::Throttled,
            message: "try later".to_owned(),
            retry_after_ms: Some(2_000),
        };
        let text = serde_json::to_string(&error).expect("serialize");
        let again: PluginError = serde_json::from_str(&text).expect("deserialize");
        assert_eq!(again, error);
    }
}
