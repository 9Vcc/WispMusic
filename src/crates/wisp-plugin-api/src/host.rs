use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{Header, PluginError};

/// The HTTP methods a provider op may ask the host to perform.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpMethod {
    Get,
    Post,
}

/// A request the plugin hands the host. The host validates the URL against the manifest's
/// `network.hosts`, applies its own TLS/DNS stack and enforces the body limit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: HttpMethod,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<Header>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: Vec<Header>,
    #[serde(default)]
    pub body: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// Host services exposed to a plugin. The native bridge implements it with the host's reqwest
/// client; the wasmtime runtime implements the same trait through its import table.
pub trait Host: Send + Sync {
    fn http(&self, request: HttpRequest) -> Result<HttpResponse, PluginError>;

    /// Performs several requests together. The default keeps them in order for hosts without a
    /// native batch path; a host that can overlap sockets should override it.
    fn http_many(&self, requests: Vec<HttpRequest>) -> Vec<Result<HttpResponse, PluginError>> {
        requests
            .into_iter()
            .map(|request| self.http(request))
            .collect()
    }

    /// Reads a secret from this plugin's namespace. The host never reveals where it is stored.
    fn secret_get(&self, key: &str) -> Option<String>;

    /// Writes a secret in this plugin's namespace. A host without secret storage rejects the
    /// write rather than silently keeping it in memory.
    fn secret_set(&self, _key: &str, _value: &str) -> Result<(), PluginError> {
        Err(PluginError::unsupported("secret_set"))
    }

    /// Reads one JSON-valued provider setting. `None` means the key was never set.
    fn config_get(&self, _key: &str) -> Option<String> {
        None
    }

    fn config_set(&self, _key: &str, _value: &str) -> Result<(), PluginError> {
        Err(PluginError::unsupported("config_set"))
    }

    /// Reads a file from the plugin's private data directory. `None` means it is absent.
    fn storage_read(&self, _key: &str) -> Option<Vec<u8>> {
        None
    }

    fn storage_write(&self, _key: &str, _bytes: &[u8]) -> Result<(), PluginError> {
        Err(PluginError::unsupported("storage_write"))
    }

    fn storage_delete(&self, _key: &str) -> Result<(), PluginError> {
        Err(PluginError::unsupported("storage_delete"))
    }

    /// Milliseconds since the Unix epoch. The default uses the system clock; a host may pin it
    /// for deterministic tests.
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Fills a fresh buffer with host randomness. An empty result means the host has no RNG and
    /// the plugin must fail rather than invent device ids.
    fn random_bytes(&self, _len: usize) -> Vec<u8> {
        Vec::new()
    }

    fn log(&self, _level: LogLevel, _message: &str) {}
}
