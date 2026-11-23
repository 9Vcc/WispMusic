//! Manifest-scoped host services.
//!
//! A plugin never talks to the operating system directly: every HTTP request and every secret
//! lookup passes through this wrapper first. The manifest is the only source of truth, so a
//! plugin cannot widen its own permissions at runtime.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

use wisp_plugin_api::{
    ErrorCode, Host, HttpRequest, HttpResponse, LogLevel, Manifest, NetworkPolicy, PluginError,
};

/// Wraps a [`Host`] with one plugin's manifest policy.
pub struct PolicyHost {
    manifest: Manifest,
    inner: Arc<dyn Host>,
}

impl PolicyHost {
    pub fn new(manifest: Manifest, inner: Arc<dyn Host>) -> Self {
        Self { manifest, inner }
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn check(&self, request: HttpRequest) -> Result<HttpRequest, PluginError> {
        let url = ParsedUrl::parse(&request.url)?;
        check_network(&self.manifest.network, &url)?;
        Ok(request)
    }

    fn limit(&self, response: HttpResponse) -> Result<HttpResponse, PluginError> {
        let limit = self.manifest.network.max_body_bytes;
        if response.body.len() as u64 > limit {
            return Err(PluginError::new(
                ErrorCode::TooLarge,
                format!(
                    "the response body is {} bytes, over the manifest limit of {limit}",
                    response.body.len()
                ),
            ));
        }
        Ok(response)
    }
}

impl Host for PolicyHost {
    fn http(&self, request: HttpRequest) -> Result<HttpResponse, PluginError> {
        let request = self.check(request)?;
        let response = self.inner.http(request)?;
        self.limit(response)
    }

    fn http_many(&self, requests: Vec<HttpRequest>) -> Vec<Result<HttpResponse, PluginError>> {
        let mut results: Vec<Option<Result<HttpResponse, PluginError>>> =
            (0..requests.len()).map(|_| None).collect();
        let mut checked = Vec::new();
        let mut places = Vec::new();
        for (place, request) in requests.into_iter().enumerate() {
            match self.check(request) {
                Ok(request) => {
                    checked.push(request);
                    places.push(place);
                }
                Err(error) => results[place] = Some(Err(error)),
            }
        }
        let responses = self.inner.http_many(checked);
        for (place, response) in places.into_iter().zip(responses) {
            results[place] = Some(response.and_then(|response| self.limit(response)));
        }
        results
            .into_iter()
            .map(|result| {
                result.unwrap_or_else(|| {
                    Err(PluginError::new(
                        ErrorCode::Internal,
                        "the host returned fewer responses than requests",
                    ))
                })
            })
            .collect()
    }

    fn secret_get(&self, key: &str) -> Option<String> {
        let allowed = self.manifest.secrets.iter().any(|name| name == key);
        allowed.then(|| self.inner.secret_get(key)).flatten()
    }

    fn secret_set(&self, key: &str, value: &str) -> Result<(), PluginError> {
        if !self.manifest.secrets.iter().any(|name| name == key) {
            return Err(PluginError::new(
                ErrorCode::BadRequest,
                format!("secret `{key}` is not declared in the manifest"),
            ));
        }
        self.inner.secret_set(key, value)
    }

    fn config_get(&self, key: &str) -> Option<String> {
        self.inner.config_get(key)
    }

    fn config_set(&self, key: &str, value: &str) -> Result<(), PluginError> {
        self.inner.config_set(key, value)
    }

    fn storage_read(&self, key: &str) -> Option<Vec<u8>> {
        self.manifest
            .storage
            .private_dir
            .then(|| self.inner.storage_read(key))
            .flatten()
    }

    fn storage_write(&self, key: &str, bytes: &[u8]) -> Result<(), PluginError> {
        if !self.manifest.storage.private_dir {
            return Err(PluginError::new(
                ErrorCode::Unsupported,
                "the manifest does not enable private storage",
            ));
        }
        if bytes.len() as u64 > self.manifest.storage.max_bytes {
            return Err(PluginError::new(
                ErrorCode::TooLarge,
                format!(
                    "the write is {} bytes, over the manifest limit of {}",
                    bytes.len(),
                    self.manifest.storage.max_bytes
                ),
            ));
        }
        self.inner.storage_write(key, bytes)
    }

    fn storage_delete(&self, key: &str) -> Result<(), PluginError> {
        if !self.manifest.storage.private_dir {
            return Err(PluginError::new(
                ErrorCode::Unsupported,
                "the manifest does not enable private storage",
            ));
        }
        self.inner.storage_delete(key)
    }

    fn now_ms(&self) -> u64 {
        self.inner.now_ms()
    }

    fn random_bytes(&self, len: usize) -> Vec<u8> {
        self.inner.random_bytes(len)
    }

    fn log(&self, level: LogLevel, message: &str) {
        self.inner.log(level, message);
    }
}

fn denied(message: impl Into<String>) -> PluginError {
    PluginError::new(ErrorCode::Network, message)
}

fn check_network(policy: &NetworkPolicy, url: &ParsedUrl) -> Result<(), PluginError> {
    if !policy
        .schemes
        .iter()
        .any(|scheme| scheme.eq_ignore_ascii_case(&url.scheme))
    {
        return Err(denied(format!(
            "scheme `{}` is not declared by the manifest",
            url.scheme
        )));
    }
    let Some(default_port) = default_port(&url.scheme) else {
        return Err(denied(format!(
            "scheme `{}` is not supported by the HTTP broker",
            url.scheme
        )));
    };
    if !policy.hosts.iter().any(|host| same_host(host, &url.host)) {
        return Err(denied(format!(
            "host `{}` is not declared by the manifest",
            url.host
        )));
    }
    if let Some(ip) = url.ip_literal()
        && forbidden_ip(ip)
    {
        return Err(denied(format!(
            "host `{}` resolves to a private or special-use address",
            url.host
        )));
    }
    if url.port != default_port {
        return Err(denied(format!(
            "port {} is not allowed; `{}` may only use {default_port}",
            url.port, url.scheme
        )));
    }
    Ok(())
}

fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    }
}

fn same_host(allowed: &str, actual: &str) -> bool {
    let allowed = allowed.trim().to_ascii_lowercase();
    let allowed = allowed.strip_suffix('.').unwrap_or(&allowed);
    allowed == actual
}

fn forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => forbidden_v4(ip),
        IpAddr::V6(ip) => forbidden_v6(ip),
    }
}

fn forbidden_v4(ip: Ipv4Addr) -> bool {
    let octets = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_unspecified()
        || octets[0] == 0
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
        || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
        || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
        || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
        || octets[0] >= 240
}

fn forbidden_v6(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    let segments = ip.segments();
    if segments[0] & 0xfe00 == 0xfc00 || segments[0] & 0xffc0 == 0xfe80 {
        return true;
    }
    let embedded = |high: u16, low: u16| {
        Ipv4Addr::new((high >> 8) as u8, high as u8, (low >> 8) as u8, low as u8)
    };
    if segments[..5].iter().all(|segment| *segment == 0) {
        if let [0, 0, 0, 0, 0, 0xffff, high, low] = segments {
            return forbidden_v4(embedded(high, low));
        }
        if let [0, 0, 0, 0, 0, 0, high, low] = segments {
            return forbidden_v4(embedded(high, low));
        }
    }
    if segments[0] == 0x64 && segments[1] == 0xff9b && segments[2..6].iter().all(|s| *s == 0) {
        return forbidden_v4(embedded(segments[6], segments[7]));
    }
    false
}

/// The subset of a URL the broker needs. Parsing is deliberately strict: the native HTTP stack
/// is free to be liberal, but anything ambiguous is rejected here before it reaches that stack.
struct ParsedUrl {
    scheme: String,
    host: String,
    port: u16,
}

impl ParsedUrl {
    fn parse(input: &str) -> Result<Self, PluginError> {
        if input.is_empty() || input.len() > 8 * 1024 {
            return Err(denied("the request URL is empty or too long"));
        }
        if input
            .bytes()
            .any(|byte| byte <= b' ' || byte == 0x7f || byte == b'\\')
        {
            return Err(denied(
                "the request URL contains whitespace, control characters or a backslash",
            ));
        }
        let (scheme, rest) = input
            .split_once("://")
            .ok_or_else(|| denied("the request URL has no scheme"))?;
        let scheme = scheme.to_ascii_lowercase();
        let mut scheme_bytes = scheme.bytes();
        let Some(first) = scheme_bytes.next() else {
            return Err(denied("the request URL has an empty scheme"));
        };
        if !first.is_ascii_alphabetic()
            || !scheme_bytes
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
        {
            return Err(denied(format!("`{scheme}` is not a valid URL scheme")));
        }

        let authority = rest
            .split(['/', '?', '#'])
            .next()
            .filter(|authority| !authority.is_empty())
            .ok_or_else(|| denied("the request URL has no host"))?;
        if authority.contains('@') {
            return Err(denied("the request URL must not carry userinfo"));
        }

        let (raw_host, explicit_port) = split_authority(authority)?;
        let host = validate_host(raw_host)?;
        let port = match explicit_port {
            Some(port) => port,
            None => default_port(&scheme).unwrap_or(0),
        };
        Ok(Self { scheme, host, port })
    }

    fn ip_literal(&self) -> Option<IpAddr> {
        self.host.parse().ok()
    }
}

fn split_authority(authority: &str) -> Result<(&str, Option<u16>), PluginError> {
    if let Some(rest) = authority.strip_prefix('[') {
        let close = rest
            .find(']')
            .ok_or_else(|| denied("the IPv6 host is missing its closing bracket"))?;
        let host = &rest[..close];
        let tail = &rest[close + 1..];
        let port = if tail.is_empty() {
            None
        } else {
            let port = tail
                .strip_prefix(':')
                .ok_or_else(|| denied("unexpected characters after the IPv6 host"))?;
            Some(parse_port(port)?)
        };
        if host.parse::<Ipv6Addr>().is_err() {
            return Err(denied(format!("`{host}` is not a valid IPv6 address")));
        }
        return Ok((host, port));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) => {
            if host.contains(':') {
                return Err(denied(
                    "unbracketed IPv6 addresses are not accepted in a request URL",
                ));
            }
            Ok((host, Some(parse_port(port)?)))
        }
        None => Ok((authority, None)),
    }
}

fn parse_port(port: &str) -> Result<u16, PluginError> {
    if port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(denied("the request URL has an invalid port"));
    }
    port.parse()
        .map_err(|_| denied("the request URL port is out of range"))
}

fn validate_host(host: &str) -> Result<String, PluginError> {
    if host.is_empty() {
        return Err(denied("the request URL has no host"));
    }
    if !host.is_ascii() {
        return Err(denied(
            "the request URL host must be ASCII; use punycode for international names",
        ));
    }
    if host.contains('%') {
        return Err(denied("the request URL host must not be percent-encoded"));
    }
    let host = host.to_ascii_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host);
    if host.is_empty() {
        return Err(denied("the request URL has an empty host"));
    }
    if host.parse::<IpAddr>().is_ok() {
        return Ok(host.to_owned());
    }
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return Err(denied("the request URL host points at a local machine"));
    }
    for label in host.split('.') {
        if label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(denied(format!("`{host}` is not a valid DNS host name")));
        }
    }
    Ok(host.to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use wisp_plugin_api::{
        AbiVersion, Assets, Declares, Header, LocalizedText, Publisher, StoragePolicy,
    };

    use super::*;

    fn manifest(hosts: &[&str], schemes: &[&str], secrets: &[&str]) -> Manifest {
        Manifest {
            id: "test".to_owned(),
            slug: "test".to_owned(),
            name: LocalizedText {
                default: "Test".to_owned(),
                others: BTreeMap::new(),
            },
            version: "1.0.0".to_owned(),
            abi: AbiVersion { major: 1, minor: 0 },
            min_app_version: "0.52.0".to_owned(),
            entry: "lib.wasm".to_owned(),
            publisher: Publisher {
                id: "wisp-official".to_owned(),
                name: "WispMusic".to_owned(),
            },
            license: "MIT".to_owned(),
            homepage: None,
            declares: Declares::default(),
            network: NetworkPolicy {
                hosts: hosts.iter().map(|host| (*host).to_owned()).collect(),
                schemes: schemes.iter().map(|scheme| (*scheme).to_owned()).collect(),
                ..NetworkPolicy::default()
            },
            secrets: secrets.iter().map(|key| (*key).to_owned()).collect(),
            storage: StoragePolicy::default(),
            assets: Assets::default(),
            i18n: None,
        }
    }

    #[derive(Default)]
    struct CountingHost {
        calls: AtomicUsize,
        response: Mutex<Option<HttpResponse>>,
        secret: Mutex<Option<String>>,
    }

    impl Host for CountingHost {
        fn http(&self, _request: HttpRequest) -> Result<HttpResponse, PluginError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(self
                .response
                .lock()
                .expect("the response lock")
                .clone()
                .unwrap_or(HttpResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: Vec::new(),
                }))
        }

        fn secret_get(&self, key: &str) -> Option<String> {
            (key == "cookie")
                .then(|| self.secret.lock().expect("the secret lock").clone())
                .flatten()
        }
    }

    fn request(url: &str) -> HttpRequest {
        HttpRequest {
            method: wisp_plugin_api::HttpMethod::Get,
            url: url.to_owned(),
            headers: Vec::<Header>::new(),
            body: None,
            timeout_ms: None,
        }
    }

    fn guarded(manifest: Manifest) -> (PolicyHost, Arc<CountingHost>) {
        let inner = Arc::new(CountingHost::default());
        (PolicyHost::new(manifest, inner.clone()), inner)
    }

    #[derive(Default)]
    struct ScopedHost {
        secrets: Mutex<std::collections::HashMap<String, String>>,
        config: Mutex<std::collections::HashMap<String, String>>,
        storage: Mutex<std::collections::HashMap<String, Vec<u8>>>,
    }

    impl Host for ScopedHost {
        fn http(&self, _request: HttpRequest) -> Result<HttpResponse, PluginError> {
            Err(PluginError::new(ErrorCode::Network, "not used"))
        }

        fn secret_get(&self, key: &str) -> Option<String> {
            self.secrets
                .lock()
                .expect("the secret lock")
                .get(key)
                .cloned()
        }

        fn secret_set(&self, key: &str, value: &str) -> Result<(), PluginError> {
            self.secrets
                .lock()
                .expect("the secret lock")
                .insert(key.to_owned(), value.to_owned());
            Ok(())
        }

        fn config_get(&self, key: &str) -> Option<String> {
            self.config
                .lock()
                .expect("the config lock")
                .get(key)
                .cloned()
        }

        fn config_set(&self, key: &str, value: &str) -> Result<(), PluginError> {
            self.config
                .lock()
                .expect("the config lock")
                .insert(key.to_owned(), value.to_owned());
            Ok(())
        }

        fn storage_read(&self, key: &str) -> Option<Vec<u8>> {
            self.storage
                .lock()
                .expect("the storage lock")
                .get(key)
                .cloned()
        }

        fn storage_write(&self, key: &str, bytes: &[u8]) -> Result<(), PluginError> {
            self.storage
                .lock()
                .expect("the storage lock")
                .insert(key.to_owned(), bytes.to_vec());
            Ok(())
        }

        fn storage_delete(&self, key: &str) -> Result<(), PluginError> {
            self.storage.lock().expect("the storage lock").remove(key);
            Ok(())
        }
    }

    fn scoped(manifest: Manifest) -> (PolicyHost, Arc<ScopedHost>) {
        let inner = Arc::new(ScopedHost::default());
        (PolicyHost::new(manifest, inner.clone()), inner)
    }

    #[test]
    fn secrets_and_storage_are_manifest_scoped() {
        let manifest = manifest(&["music.163.com"], &["https"], &["cookie"]);
        let (host, inner) = scoped(manifest);

        host.secret_set("cookie", "MUSIC_U=test")
            .expect("the declared secret is writable");
        assert_eq!(host.secret_get("cookie"), Some("MUSIC_U=test".to_owned()));
        assert!(host.secret_set("other", "x").is_err());
        assert!(
            !inner
                .secrets
                .lock()
                .expect("the secret lock")
                .contains_key("other")
        );

        host.storage_write("cache.bin", b"abc")
            .expect("private storage defaults to enabled");
        assert_eq!(host.storage_read("cache.bin"), Some(b"abc".to_vec()));
        host.config_set("quality", r#""lossless""#)
            .expect("config is not manifest gated");
        assert_eq!(host.config_get("quality"), Some(r#""lossless""#.to_owned()));
        host.storage_delete("cache.bin").expect("delete");
        assert_eq!(host.storage_read("cache.bin"), None);
    }

    #[test]
    fn private_storage_and_size_limits_are_enforced() {
        let mut closed = manifest(&["music.163.com"], &["https"], &[]);
        closed.storage.private_dir = false;
        let (host, inner) = scoped(closed);
        assert!(host.storage_write("cache.bin", b"abc").is_err());
        assert_eq!(host.storage_read("cache.bin"), None);
        assert!(inner.storage.lock().expect("the storage lock").is_empty());

        let mut limited = manifest(&["music.163.com"], &["https"], &[]);
        limited.storage.max_bytes = 4;
        let (host, _) = scoped(limited);
        let error = host
            .storage_write("cache.bin", b"12345")
            .expect_err("the write is over the limit");
        assert_eq!(error.code, ErrorCode::TooLarge);
        host.storage_write("cache.bin", b"1234")
            .expect("the write fits");
        assert_eq!(host.storage_read("cache.bin"), Some(b"1234".to_vec()));
    }

    #[test]
    fn the_declared_host_is_allowed() {
        let (host, inner) = guarded(manifest(&["music.163.com"], &["https"], &[]));
        let response = host
            .http(request("https://music.163.com/api/search?q=test"))
            .expect("the host is allowed");
        assert_eq!(response.status, 200);
        assert_eq!(inner.calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn an_undeclared_host_is_rejected_before_the_inner_host() {
        let (host, inner) = guarded(manifest(&["music.163.com"], &["https"], &[]));
        let error = host
            .http(request("https://example.com/api"))
            .expect_err("the host is not declared");
        assert_eq!(error.code, ErrorCode::Network);
        assert_eq!(inner.calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_batch_keeps_its_order_and_refuses_bad_urls_before_the_inner_host() {
        let (host, inner) = guarded(manifest(&["music.163.com"], &["https"], &[]));
        let results = host.http_many(vec![
            request("https://music.163.com/a"),
            request("https://example.com/b"),
            request("https://music.163.com/c"),
        ]);

        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert_eq!(
            results[1]
                .as_ref()
                .expect_err("the host is not declared")
                .code,
            ErrorCode::Network
        );
        assert!(results[2].is_ok());
        assert_eq!(inner.calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn userinfo_cannot_smuggle_a_different_host() {
        let (host, inner) = guarded(manifest(&["music.163.com"], &["https"], &[]));
        let error = host
            .http(request("https://music.163.com@evil.example/api"))
            .expect_err("userinfo is rejected");
        assert_eq!(error.code, ErrorCode::Network);
        assert_eq!(inner.calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_loopback_literal_is_rejected_even_when_allowlisted() {
        let (host, inner) = guarded(manifest(&["127.0.0.1"], &["https"], &[]));
        let error = host
            .http(request("https://127.0.0.1/api"))
            .expect_err("loopback is always refused");
        assert_eq!(error.code, ErrorCode::Network);
        assert_eq!(inner.calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn an_ipv6_loopback_is_rejected() {
        let (host, inner) = guarded(manifest(&["::1"], &["https"], &[]));
        let error = host
            .http(request("https://[::1]/api"))
            .expect_err("IPv6 loopback is always refused");
        assert_eq!(error.code, ErrorCode::Network);
        assert_eq!(inner.calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_private_ipv4_address_is_rejected() {
        for address in ["10.0.0.1", "172.16.4.2", "192.168.1.5", "169.254.1.1"] {
            let url = format!("https://{address}/api");
            let (host, inner) = guarded(manifest(&[address], &["https"], &[]));
            assert!(
                host.http(request(&url)).is_err(),
                "{address} must be refused"
            );
            assert_eq!(inner.calls.load(Ordering::Relaxed), 0);
        }
    }

    #[test]
    fn non_default_ports_are_rejected() {
        let (host, inner) = guarded(manifest(&["music.163.com"], &["https"], &[]));
        let error = host
            .http(request("https://music.163.com:8443/api"))
            .expect_err("port 8443 is not declared");
        assert_eq!(error.code, ErrorCode::Network);
        assert_eq!(inner.calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn http_needs_an_explicit_manifest_scheme() {
        let (host, _) = guarded(manifest(&["music.163.com"], &["https"], &[]));
        assert!(host.http(request("http://music.163.com/api")).is_err());

        let (host, inner) = guarded(manifest(&["music.163.com"], &["http"], &[]));
        host.http(request("http://music.163.com/api"))
            .expect("http is declared");
        assert_eq!(inner.calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn ambient_dns_names_are_rejected() {
        for url in [
            "https://localhost/api",
            "https://service.local/api",
            "https://music%2e163.com/api",
            "https://music.163.com./api",
        ] {
            let (host, inner) = guarded(manifest(
                &[
                    "localhost",
                    "service.local",
                    "music%2e163.com",
                    "music.163.com.",
                ],
                &["https"],
                &[],
            ));
            let result = host.http(request(url));
            if url.ends_with("music.163.com./api") {
                result.expect("a trailing root dot is normalized");
                assert_eq!(inner.calls.load(Ordering::Relaxed), 1);
            } else {
                assert!(result.is_err(), "{url} must be refused");
                assert_eq!(inner.calls.load(Ordering::Relaxed), 0);
            }
        }
    }

    #[test]
    fn a_url_without_a_scheme_or_host_is_rejected() {
        for url in [
            "music.163.com/api",
            "https://",
            "https:///api",
            "https://music.163.com\\api",
            "https://music.163.com:abc/api",
        ] {
            let (host, inner) = guarded(manifest(&["music.163.com"], &["https"], &[]));
            assert!(host.http(request(url)).is_err(), "{url} must be refused");
            assert_eq!(inner.calls.load(Ordering::Relaxed), 0);
        }
    }

    #[test]
    fn the_secret_namespace_only_returns_declared_keys() {
        let (host, inner) = guarded(manifest(&["music.163.com"], &["https"], &["cookie"]));
        *inner.secret.lock().expect("the secret lock") = Some("MUSIC_U=test".to_owned());
        assert_eq!(host.secret_get("cookie"), Some("MUSIC_U=test".to_owned()));
        assert_eq!(host.secret_get("MUSIC_U"), None);
    }

    #[test]
    fn an_oversized_response_is_rejected() {
        let mut manifest = manifest(&["music.163.com"], &["https"], &[]);
        manifest.network.max_body_bytes = 4;
        let (host, inner) = guarded(manifest);
        *inner.response.lock().expect("the response lock") = Some(HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: b"12345".to_vec(),
        });
        let error = host
            .http(request("https://music.163.com/api"))
            .expect_err("the response is too large");
        assert_eq!(error.code, ErrorCode::TooLarge);
    }
}
