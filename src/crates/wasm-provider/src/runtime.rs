//! Host-side loading of a flat `.wisp` package through the wasmtime runtime.
//!
//! Loading is lazy: the package is validated when the provider is built, while the wasm module
//! is compiled and instantiated on the first op call. A plugin that cannot load is remembered as
//! failed, so the caller can fall back without paying the compile cost on every request.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use wisp_plugin_api::{
    ErrorCode, Header, Host, HttpMethod, HttpRequest, HttpResponse, LogLevel, Manifest, PluginError,
};
use wisp_plugin_host::{LoadedPlugin as HostPlugin, PolicyHost, Runtime};
use wisp_plugin_pack::{VerifyOptions, load as load_package};

pub(crate) fn verify_options() -> VerifyOptions {
    VerifyOptions {
        public_key: None,
        allow_unsigned: true,
        allow_zip: false,
        skip_signature: true,
    }
}

/// A freshly activated package is watched for this long. More than two traps, or a p95 call
/// latency over twice the previous package's baseline, rolls it back to N-1.
const OBSERVATION_WINDOW: Duration = Duration::from_secs(60);
const TRAP_LIMIT: u32 = 3;
const LATENCY_SAMPLES: usize = 20;
const SAMPLE_CAPACITY: usize = 128;

#[derive(Debug)]
struct Observation {
    started: Instant,
    traps: u32,
    baseline: Option<Duration>,
    samples: VecDeque<Duration>,
}

impl Observation {
    fn new(baseline: Option<Duration>, now: Instant) -> Self {
        Self {
            started: now,
            traps: 0,
            baseline,
            samples: VecDeque::new(),
        }
    }

    fn roll(&mut self, now: Instant) {
        if now.duration_since(self.started) >= OBSERVATION_WINDOW {
            self.started = now;
            self.traps = 0;
            self.samples.clear();
        }
    }

    /// Records a trap and reports whether the package crossed the observation threshold.
    fn trapped(&mut self, now: Instant) -> bool {
        self.roll(now);
        self.traps = self.traps.saturating_add(1);
        self.traps >= TRAP_LIMIT
    }

    /// Records a successful call and reports whether its p95 latency doubled the baseline.
    fn sampled(&mut self, now: Instant, elapsed: Duration) -> bool {
        self.roll(now);
        if self.samples.len() == SAMPLE_CAPACITY {
            self.samples.pop_front();
        }
        self.samples.push_back(elapsed);
        let Some(baseline) = self.baseline else {
            return false;
        };
        if baseline.is_zero() || self.samples.len() < LATENCY_SAMPLES {
            return false;
        }
        p95(&self.samples).is_some_and(|p95| p95 > baseline.saturating_mul(2))
    }
}

fn p95(samples: &VecDeque<Duration>) -> Option<Duration> {
    let mut values: Vec<Duration> = samples.iter().copied().collect();
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let rank = (values.len() * 95).div_ceil(100);
    values.get(rank.saturating_sub(1)).copied()
}

/// Host services shared by every client of one provider slot. The cookie cell is the only
/// mutable part: signing in updates it in place, so a hot-swapped plugin sees the new session.
pub struct SessionHost {
    http: reqwest::Client,
    handle: tokio::runtime::Handle,
    cookie: Mutex<String>,
    secrets: Mutex<HashMap<String, String>>,
    config: Mutex<HashMap<String, String>>,
    storage_dir: Mutex<Option<PathBuf>>,
    storage_quota: Mutex<u64>,
    allowed_hosts: RwLock<Vec<String>>,
}

impl SessionHost {
    pub fn new(
        http: reqwest::Client,
        handle: tokio::runtime::Handle,
        allowed_hosts: Vec<String>,
    ) -> Self {
        Self {
            http,
            handle,
            cookie: Mutex::new(String::new()),
            secrets: Mutex::new(HashMap::new()),
            config: Mutex::new(HashMap::new()),
            storage_dir: Mutex::new(None),
            storage_quota: Mutex::new(16 * 1024 * 1024),
            allowed_hosts: RwLock::new(allowed_hosts),
        }
    }

    pub fn set_cookie(&self, cookie: &str) {
        if let Ok(mut current) = self.cookie.lock() {
            current.clear();
            current.push_str(cookie);
        }
    }

    pub fn set_storage_quota(&self, bytes: u64) {
        if let Ok(mut quota) = self.storage_quota.lock() {
            *quota = bytes;
        }
    }

    pub fn set_allowed_hosts(&self, hosts: Vec<String>) {
        if let Ok(mut allowed) = self.allowed_hosts.write() {
            *allowed = hosts;
        }
    }

    pub fn set_storage_dir(&self, dir: PathBuf) {
        if let Ok(mut storage) = self.storage_dir.lock() {
            *storage = Some(dir.clone());
        }
        if let Ok(text) = fs::read_to_string(dir.join("config.json"))
            && let Ok(values) = serde_json::from_str::<HashMap<String, String>>(&text)
            && let Ok(mut config) = self.config.lock()
        {
            *config = values;
        }
        if let Ok(text) = fs::read_to_string(dir.join("secrets.json"))
            && let Ok(values) = serde_json::from_str::<HashMap<String, String>>(&text)
            && let Ok(mut secrets) = self.secrets.lock()
        {
            *secrets = values;
        }
    }

    fn storage_path(&self, key: &str) -> Option<PathBuf> {
        if key.is_empty()
            || key.len() > 128
            || key.contains(['/', '\\', ':', '\0'])
            || key.contains("..")
            || key == "config.json"
            || key == "secrets.json"
        {
            return None;
        }
        let dir = self.storage_dir.lock().ok()?.clone()?;
        Some(dir.join(key))
    }

    fn persist_json(&self, name: &str, bytes: Vec<u8>, what: &str) -> Result<(), PluginError> {
        let Some(dir) = self.storage_dir.lock().ok().and_then(|dir| dir.clone()) else {
            return Ok(());
        };
        fs::create_dir_all(&dir)
            .and_then(|()| fs::write(dir.join(name), bytes))
            .map_err(|error| PluginError::new(ErrorCode::Internal, format!("{what}: {error}")))
    }

    fn persist_config(&self) -> Result<(), PluginError> {
        let values = self
            .config
            .lock()
            .map_err(|_| PluginError::new(ErrorCode::Internal, "config lock poisoned"))?;
        let bytes = serde_json::to_vec(&*values)
            .map_err(|error| PluginError::new(ErrorCode::Internal, error.to_string()))?;
        drop(values);
        self.persist_json("config.json", bytes, "cannot persist config")
    }

    fn persist_secrets(&self) -> Result<(), PluginError> {
        let values = self
            .secrets
            .lock()
            .map_err(|_| PluginError::new(ErrorCode::Internal, "secret lock poisoned"))?;
        let bytes = serde_json::to_vec(&*values)
            .map_err(|error| PluginError::new(ErrorCode::Internal, error.to_string()))?;
        drop(values);
        self.persist_json("secrets.json", bytes, "cannot persist secrets")
    }
}

impl Host for SessionHost {
    fn http(&self, request: HttpRequest) -> std::result::Result<HttpResponse, PluginError> {
        let url = reqwest::Url::parse(&request.url).map_err(|error| {
            PluginError::new(ErrorCode::BadRequest, format!("bad plugin url: {error}"))
        })?;
        if url.scheme() != "https" {
            return Err(PluginError::new(
                ErrorCode::BadRequest,
                "plugins may only reach https hosts",
            ));
        }
        let host = url
            .host_str()
            .ok_or_else(|| PluginError::new(ErrorCode::BadRequest, "the plugin url has no host"))?;
        let allowed = self
            .allowed_hosts
            .read()
            .map(|hosts| hosts.iter().any(|allowed| allowed == host))
            .unwrap_or(false);
        if !allowed {
            return Err(PluginError::new(
                ErrorCode::BadRequest,
                format!("host {host} is not declared in the manifest"),
            ));
        }

        let http = self.http.clone();
        self.handle.block_on(async move {
            let method = match request.method {
                HttpMethod::Get => reqwest::Method::GET,
                HttpMethod::Post => reqwest::Method::POST,
            };
            let mut builder = http.request(method, url);
            for header in &request.headers {
                builder = builder.header(header.name.as_str(), header.value.as_str());
            }
            if let Some(body) = request.body {
                builder = builder.body(body);
            }
            if let Some(ms) = request.timeout_ms {
                builder = builder.timeout(Duration::from_millis(ms));
            }
            let response = builder.send().await.map_err(|error| {
                PluginError::new(
                    ErrorCode::Network,
                    format!("the plugin request failed: {error}"),
                )
            })?;
            let status = response.status().as_u16();
            let headers: Vec<Header> = response
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    Some(Header {
                        name: name.as_str().to_owned(),
                        value: value.to_str().ok()?.to_owned(),
                    })
                })
                .collect();
            let body = response.bytes().await.map_err(|error| {
                PluginError::new(
                    ErrorCode::Network,
                    format!("the plugin response failed: {error}"),
                )
            })?;
            if body.len() > 8 * 1024 * 1024 {
                return Err(PluginError::new(
                    ErrorCode::TooLarge,
                    "the plugin response exceeds the host limit",
                ));
            }
            Ok(HttpResponse {
                status,
                headers,
                body: body.to_vec(),
            })
        })
    }

    fn http_many(
        &self,
        requests: Vec<HttpRequest>,
    ) -> Vec<std::result::Result<HttpResponse, PluginError>> {
        std::thread::scope(|scope| {
            requests
                .into_iter()
                .map(|request| {
                    let started = std::time::Instant::now();
                    let path = reqwest::Url::parse(&request.url)
                        .map(|url| url.path().to_owned())
                        .unwrap_or_else(|_| request.url.clone());
                    scope.spawn(move || {
                        let result = self.http(request);
                        log::debug!("plugins: batched {path} in {:?}", started.elapsed());
                        result
                    })
                })
                .map(|handle| {
                    handle.join().unwrap_or_else(|_| {
                        Err(PluginError::new(
                            ErrorCode::Internal,
                            "a plugin request thread panicked",
                        ))
                    })
                })
                .collect()
        })
    }

    fn secret_get(&self, key: &str) -> Option<String> {
        if key == "cookie" {
            return self
                .cookie
                .lock()
                .ok()
                .map(|cookie| cookie.clone())
                .filter(|cookie| !cookie.is_empty());
        }
        self.secrets
            .lock()
            .ok()
            .and_then(|secrets| secrets.get(key).cloned())
    }

    fn secret_set(&self, key: &str, value: &str) -> std::result::Result<(), PluginError> {
        if key == "cookie" {
            self.set_cookie(value);
            return Ok(());
        }
        if key.is_empty() || key.contains(['/', '\\']) || key.contains("..") {
            return Err(PluginError::new(ErrorCode::BadRequest, "bad secret key"));
        }
        self.secrets
            .lock()
            .map_err(|_| PluginError::new(ErrorCode::Internal, "secret lock poisoned"))?
            .insert(key.to_owned(), value.to_owned());
        self.persist_secrets()
    }

    fn config_get(&self, key: &str) -> Option<String> {
        self.config
            .lock()
            .ok()
            .and_then(|config| config.get(key).cloned())
    }

    fn config_set(&self, key: &str, value: &str) -> std::result::Result<(), PluginError> {
        if key.is_empty() {
            return Err(PluginError::new(ErrorCode::BadRequest, "empty config key"));
        }
        self.config
            .lock()
            .map_err(|_| PluginError::new(ErrorCode::Internal, "config lock poisoned"))?
            .insert(key.to_owned(), value.to_owned());
        self.persist_config()
    }

    fn storage_read(&self, key: &str) -> Option<Vec<u8>> {
        let path = self.storage_path(key)?;
        fs::read(path).ok()
    }

    fn storage_write(&self, key: &str, bytes: &[u8]) -> std::result::Result<(), PluginError> {
        let path = self
            .storage_path(key)
            .ok_or_else(|| PluginError::new(ErrorCode::BadRequest, "bad storage key"))?;
        let dir = self
            .storage_dir
            .lock()
            .ok()
            .and_then(|dir| dir.clone())
            .ok_or_else(|| {
                PluginError::new(ErrorCode::Unsupported, "no private storage attached")
            })?;
        let existing = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        let used = directory_size(&dir);
        let quota = self.storage_quota.lock().map(|quota| *quota).unwrap_or(0);
        let next = used.saturating_sub(existing) + bytes.len() as u64;
        if next > quota {
            return Err(PluginError::new(
                ErrorCode::TooLarge,
                format!("the plugin data directory would grow to {next} bytes, over {quota}"),
            ));
        }
        fs::create_dir_all(&dir)
            .map_err(|error| PluginError::new(ErrorCode::Internal, error.to_string()))?;
        fs::write(path, bytes)
            .map_err(|error| PluginError::new(ErrorCode::Internal, error.to_string()))
    }

    fn storage_delete(&self, key: &str) -> std::result::Result<(), PluginError> {
        let path = self
            .storage_path(key)
            .ok_or_else(|| PluginError::new(ErrorCode::BadRequest, "bad storage key"))?;
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(PluginError::new(ErrorCode::Internal, error.to_string())),
        }
    }

    fn random_bytes(&self, len: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; len];
        for byte in &mut bytes {
            *byte = fastrand::u8(..);
        }
        bytes
    }

    fn log(&self, level: LogLevel, message: &str) {
        match level {
            LogLevel::Error => log::error!("netease-plugin: {message}"),
            LogLevel::Warn => log::warn!("netease-plugin: {message}"),
            LogLevel::Info => log::info!("netease-plugin: {message}"),
            LogLevel::Debug => log::debug!("netease-plugin: {message}"),
            LogLevel::Trace => log::trace!("netease-plugin: {message}"),
        }
    }
}

fn directory_size(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|entry| entry.metadata().ok())
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .sum()
}

static SHARED: OnceLock<Mutex<HashMap<String, Weak<WasmPlugin>>>> = OnceLock::new();

/// The runtime health of one flat plugin slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginHealth {
    Active,
    RolledBack,
    Quarantined(String),
}

pub fn health_for(slug: &str) -> Option<PluginHealth> {
    let slots = SHARED.get()?;
    let slots = slots.lock().ok()?;
    slots.get(slug)?.upgrade().map(|plugin| plugin.health())
}

/// Whether the provider's stored account needs the user to sign in again. A runtime failure
/// does not set this; only a stored session the provider refused does.
pub fn sign_in_required_for(slug: &str) -> Option<bool> {
    let slots = SHARED.get()?;
    let slots = slots.lock().ok()?;
    slots
        .get(slug)?
        .upgrade()
        .map(|plugin| plugin.needs_sign_in())
}

/// The same package slot for every provider incarnation of a slug. A later catalog scan swaps
/// the active package in place, so existing clients keep using the slot and see the new wasm.
///
/// Only the map lookup runs under the registry lock. `swap` and the slot build verify and
/// compile the package, which can take seconds, and the UI reads health through `health_for`
/// on the main thread — holding the registry lock across that work would freeze it.
pub fn shared(package: &Path, host: Option<Arc<SessionHost>>) -> Result<Arc<WasmPlugin>> {
    let package = fs::canonicalize(package)
        .with_context(|| format!("cannot resolve {}", package.display()))?;
    let manifest = load_package(&package, &verify_options())?.manifest;
    let slots = SHARED.get_or_init(|| Mutex::new(HashMap::new()));

    let existing = {
        let slots = slots
            .lock()
            .map_err(|_| anyhow::anyhow!("the plugin slot registry is poisoned"))?;
        slots.get(&manifest.slug).and_then(Weak::upgrade)
    };
    if let Some(existing) = existing {
        if let Err(error) = existing.swap(&package) {
            log::warn!(
                "plugins: cannot hot-switch {} to {}: {error:#}",
                manifest.slug,
                manifest.version
            );
        }
        return Ok(existing);
    }

    let plugin = Arc::new(match host {
        Some(host) => WasmPlugin::with_session_host(&package, host)?,
        None => WasmPlugin::new(&package)?,
    });
    {
        let mut slots = slots
            .lock()
            .map_err(|_| anyhow::anyhow!("the plugin slot registry is poisoned"))?;
        // Another thread may have won the race while this one was building. The earlier slot
        // keeps the slot identity; the package this one loaded is swapped into it.
        if let Some(existing) = slots.get(&manifest.slug).and_then(Weak::upgrade) {
            if let Err(error) = existing.swap(&package) {
                log::warn!(
                    "plugins: cannot hot-switch {} to {}: {error:#}",
                    manifest.slug,
                    manifest.version
                );
            }
            return Ok(existing);
        }
        slots.insert(manifest.slug, Arc::downgrade(&plugin));
    }
    Ok(plugin)
}

/// A flat package that can call wasm ops on demand. One instance belongs to one installed
/// provider, so its runtime and linear memory are kept for the life of that provider.
pub struct WasmPlugin {
    manifest: Manifest,
    host: Option<Arc<dyn Host>>,
    session_host: Option<Arc<SessionHost>>,
    state: Mutex<State>,
    /// A compiled, described package waiting for the active call (if any) to finish. It lives
    /// outside `state` because the state lock is held for the whole wasm call.
    pending: Mutex<Option<Staged>>,
    /// The health the UI reads, outside `state` for the same reason: wasm ops hold the state
    /// lock across host network round trips, and the UI asks for health every second. It must
    /// never wait on that, so writers mirror the health transitions here under this short lock.
    reported: Mutex<PluginHealth>,
    /// Account-level state: a stored cookie that no longer opens should ask for a sign-in
    /// without touching the package or rolling it back.
    sign_in_required: AtomicBool,
    /// Interactive calls wait in front of queued background reads, so opening a page does not
    /// sit behind the library's startup sweep.
    high_waiting: AtomicUsize,
}

struct HighPriority<'a>(&'a AtomicUsize);

impl Drop for HighPriority<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

struct State {
    package: PathBuf,
    previous: Option<PathBuf>,
    loaded: Option<Loaded>,
    failed: Option<String>,
    description: Option<String>,
    observation: Option<Observation>,
    samples: VecDeque<Duration>,
}

struct Loaded {
    // The runtime owns the epoch ticker; it must outlive the instantiated store.
    _runtime: Runtime,
    plugin: HostPlugin,
    version: String,
    hosts: Vec<String>,
    storage_max: u64,
}

struct Staged {
    package: PathBuf,
    loaded: Loaded,
    description: String,
}

impl WasmPlugin {
    pub fn new(package: &Path) -> Result<Self> {
        Self::with_host(package, None)
    }

    pub fn with_host(package: &Path, host: Option<Arc<dyn Host>>) -> Result<Self> {
        Self::build(package, host, None)
    }

    pub fn with_session_host(package: &Path, host: Arc<SessionHost>) -> Result<Self> {
        Self::build(package, None, Some(host))
    }

    fn build(
        package: &Path,
        host: Option<Arc<dyn Host>>,
        session_host: Option<Arc<SessionHost>>,
    ) -> Result<Self> {
        let package = fs::canonicalize(package)
            .with_context(|| format!("cannot resolve {}", package.display()))?;
        let loaded = load_package(&package, &verify_options())?;
        Ok(Self {
            manifest: loaded.manifest,
            host,
            session_host,
            state: Mutex::new(State {
                package,
                previous: None,
                loaded: None,
                failed: None,
                description: None,
                observation: None,
                samples: VecDeque::new(),
            }),
            pending: Mutex::new(None),
            reported: Mutex::new(PluginHealth::Active),
            sign_in_required: AtomicBool::new(false),
            high_waiting: AtomicUsize::new(0),
        })
    }

    /// Replaces the active package after a hot install. The new module is compiled and
    /// described as a shadow instance first; if an op is running, it waits in `pending` and is
    /// activated at the first idle window. A bad shadow leaves the active instance untouched.
    pub fn swap(&self, package: &Path) -> Result<()> {
        let package = fs::canonicalize(package)
            .with_context(|| format!("cannot resolve {}", package.display()))?;
        if let Ok(state) = self.state.try_lock()
            && state.package == package
            && state.loaded.is_some()
            && state.failed.is_none()
        {
            return Ok(());
        }

        let (loaded, description) = self.instantiate(&package)?;
        let version = loaded.version.clone();
        let staged = Staged {
            package,
            loaded,
            description,
        };
        match self.state.try_lock() {
            Ok(mut state) => self.activate(&mut state, staged),
            Err(_) => {
                let mut pending = self
                    .pending
                    .lock()
                    .map_err(|_| anyhow::anyhow!("the plugin staging lock is poisoned"))?;
                *pending = Some(staged);
                drop(pending);
                log::info!(
                    "plugins: staged wasm {version} for {} until the active op finishes",
                    self.manifest.slug
                );
                self.apply_staged();
            }
        }
        Ok(())
    }

    pub fn health(&self) -> PluginHealth {
        // Bring a staged swap in when the slot is idle, but never wait on `state`: wasm ops
        // hold it across host network calls, and the UI reads this every second.
        self.apply_staged();
        self.reported
            .lock()
            .map(|health| health.clone())
            .unwrap_or_else(|_| PluginHealth::Quarantined("runtime lock poisoned".to_owned()))
    }

    fn report(&self, health: PluginHealth) {
        if let Ok(mut reported) = self.reported.lock() {
            *reported = health;
        }
    }

    fn activate(&self, state: &mut State, staged: Staged) {
        let version = staged.loaded.version.clone();
        self.apply_session_policy(&staged.loaded);
        if state.package != staged.package {
            state.previous = Some(state.package.clone());
        }
        let baseline = (state.samples.len() >= LATENCY_SAMPLES)
            .then(|| p95(&state.samples))
            .flatten();
        state.package = staged.package;
        state.failed = None;
        state.description = Some(staged.description);
        state.samples.clear();
        state.observation = Some(Observation::new(baseline, Instant::now()));
        self.report(PluginHealth::Active);
        state.loaded = Some(staged.loaded);
        log::info!(
            "plugins: hot-switched {} to wasm {version} at an idle window",
            self.manifest.slug
        );
    }

    fn apply_session_policy(&self, loaded: &Loaded) {
        if let Some(session) = &self.session_host {
            session.set_storage_quota(loaded.storage_max);
            session.set_allowed_hosts(loaded.hosts.clone());
        }
    }

    fn apply_staged(&self) {
        let Ok(mut state) = self.state.try_lock() else {
            return;
        };
        let staged = self
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.take());
        if let Some(staged) = staged {
            self.activate(&mut state, staged);
        }
    }

    #[cfg(test)]
    fn pending_package(&self) -> Option<PathBuf> {
        self.pending
            .lock()
            .ok()?
            .as_ref()
            .map(|staged| staged.package.clone())
    }

    fn instantiate(&self, package: &Path) -> Result<(Loaded, String)> {
        let package_manifest = load_package(package, &verify_options())
            .context("cannot read the package for wasm execution")?;
        if package_manifest.manifest.id != self.manifest.id
            || package_manifest.manifest.slug != self.manifest.slug
        {
            bail!(
                "{} describes {} ({}) but this slot is {} ({})",
                package.display(),
                package_manifest.manifest.slug,
                package_manifest.manifest.id,
                self.manifest.slug,
                self.manifest.id
            );
        }
        let loaded = package_manifest;
        let entry = loaded
            .entry(&loaded.manifest.entry)
            .with_context(|| {
                format!(
                    "the package has no `{}` entry for wasm execution",
                    loaded.manifest.entry
                )
            })?
            .raw
            .clone();
        let runtime = Runtime::new();
        let host = match &self.session_host {
            Some(session) => {
                let guarded = PolicyHost::new(loaded.manifest.clone(), session.clone());
                Some(Arc::new(guarded) as Arc<dyn Host>)
            }
            None => self.host.clone(),
        };
        let mut plugin = match host {
            Some(host) => runtime.load_with_host(&entry, host),
            None => runtime.load(&entry),
        }
        .with_context(|| format!("cannot instantiate {}", package.display()))?;
        let description = plugin
            .describe()
            .context("the plugin did not answer wisp_describe")?;
        // The package manifest is authoritative; a guest that names a different plugin must not
        // enter the slot. Fixtures that answer non-JSON are left to the package manifest.
        if let Ok(serde_json::Value::Object(described)) =
            serde_json::from_str::<serde_json::Value>(&description)
        {
            for (key, expected) in [
                ("id", loaded.manifest.id.as_str()),
                ("slug", loaded.manifest.slug.as_str()),
            ] {
                if let Some(actual) = described.get(key).and_then(serde_json::Value::as_str)
                    && actual != expected
                {
                    bail!("the wasm describes {key} `{actual}` but its package says `{expected}`");
                }
            }
        }
        Ok((
            Loaded {
                _runtime: runtime,
                plugin,
                version: loaded.manifest.version,
                hosts: loaded.manifest.network.hosts,
                storage_max: loaded.manifest.storage.max_bytes,
            },
            description,
        ))
    }

    fn fail_active(&self, state: &mut State, error: &anyhow::Error, reason: &str) {
        state.observation = None;
        state.samples.clear();
        let Some(previous) = state.previous.take() else {
            self.quarantine(state, error, "no previous package is available");
            return;
        };
        match self.instantiate(&previous) {
            Ok((loaded, description)) => {
                self.apply_session_policy(&loaded);
                state.package = previous;
                state.loaded = Some(loaded);
                state.description = Some(description);
                state.failed = None;
                self.report(PluginHealth::RolledBack);
                log::warn!(
                    "plugins: {} rolled back to {} after {reason}",
                    self.manifest.slug,
                    state.package.display()
                );
            }
            Err(rollback) => {
                self.quarantine(state, error, &format!("rollback failed: {rollback:#}"));
            }
        }
    }

    fn quarantine(&self, state: &mut State, error: &anyhow::Error, detail: &str) {
        let reason = format!("{error:#}; {detail}");
        state.observation = None;
        state.samples.clear();
        state.loaded = None;
        state.failed = Some(reason.clone());
        self.report(PluginHealth::Quarantined(reason));
        log::error!("plugins: {} quarantined: {detail}", self.manifest.slug);
    }

    pub fn set_cookie(&self, cookie: &str) {
        if let Some(host) = &self.session_host {
            host.set_cookie(cookie);
        }
    }

    pub fn set_sign_in_required(&self, required: bool) {
        self.sign_in_required.store(required, Ordering::Relaxed);
    }

    pub fn needs_sign_in(&self) -> bool {
        self.sign_in_required.load(Ordering::Relaxed)
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn describe(&self) -> Result<String> {
        self.apply_staged();
        let result = (|| {
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("the plugin runtime lock is poisoned"))?;
            self.ensure_loaded(&mut state)?;
            Ok(state.description.clone().unwrap_or_else(|| "{}".to_owned()))
        })();
        self.apply_staged();
        result
    }

    pub fn call(&self, op: u32, arg: &str) -> Result<String> {
        self.call_with_priority(op, arg, false)
    }

    /// Runs an interactive op ahead of background reads waiting for the same slot. A call
    /// already in flight still finishes first; there is no preemption.
    pub fn call_high(&self, op: u32, arg: &str) -> Result<String> {
        self.call_with_priority(op, arg, true)
    }

    fn call_with_priority(&self, op: u32, arg: &str, high: bool) -> Result<String> {
        let _high = high.then(|| {
            self.high_waiting.fetch_add(1, Ordering::SeqCst);
            HighPriority(&self.high_waiting)
        });
        if !high {
            while self.high_waiting.load(Ordering::SeqCst) > 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        self.apply_staged();
        let result = (|| {
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("the plugin runtime lock is poisoned"))?;
            self.ensure_loaded(&mut state)?;
            let sample = op != wisp_plugin_api::ops::music_api::TRACK_RADIO;
            self.tracked(&mut state, sample, |plugin| plugin.call(op, arg))
        })();
        self.apply_staged();
        result
    }

    pub fn sign_in_step(&self, flow: u32, input: &str) -> Result<String> {
        self.apply_staged();
        let result = (|| {
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("the plugin runtime lock is poisoned"))?;
            self.ensure_loaded(&mut state)?;
            self.tracked(&mut state, true, |plugin| plugin.sign_in_step(flow, input))
        })();
        self.apply_staged();
        result
    }

    fn tracked(
        &self,
        state: &mut State,
        sample: bool,
        call: impl FnOnce(&mut HostPlugin) -> Result<String>,
    ) -> Result<String> {
        let started = Instant::now();
        let loaded = state
            .loaded
            .as_mut()
            .expect("ensure_loaded filled the plugin");
        let before = loaded.plugin.trap_count();
        let result = call(&mut loaded.plugin);
        let trapped = loaded.plugin.trap_count() > before;
        let elapsed = started.elapsed();
        match &result {
            Ok(_) if sample => {
                if state.samples.len() == SAMPLE_CAPACITY {
                    state.samples.pop_front();
                }
                state.samples.push_back(elapsed);
                let exceeded = state
                    .observation
                    .as_mut()
                    .is_some_and(|observation| observation.sampled(Instant::now(), elapsed));
                if exceeded {
                    let error = anyhow::anyhow!(
                        "the p95 call latency doubled during the 60 s observation window"
                    );
                    self.fail_active(state, &error, "latency");
                }
            }
            Ok(_) => {}
            Err(error) if trapped => {
                let exceeded = state
                    .observation
                    .as_mut()
                    .is_some_and(|observation| observation.trapped(Instant::now()));
                if exceeded {
                    self.fail_active(state, error, "three traps");
                }
            }
            Err(_) => {}
        }
        result
    }

    fn ensure_loaded(&self, state: &mut State) -> Result<()> {
        if let Some(error) = &state.failed {
            bail!("the plugin failed to load earlier: {error}");
        }
        if state.loaded.is_some() {
            return Ok(());
        }

        match self.instantiate(&state.package) {
            Ok((loaded, description)) => {
                log::info!(
                    "plugins: wasm execution ready for {} {}",
                    self.manifest.slug,
                    loaded.version
                );
                self.apply_session_policy(&loaded);
                state.description = Some(description);
                state.samples.clear();
                state.observation = Some(Observation::new(None, Instant::now()));
                state.loaded = Some(loaded);
                Ok(())
            }
            Err(error) => {
                let message = format!("{error:#}");
                log::warn!(
                    "plugins: wasm execution failed for {} {}: {message}",
                    self.manifest.slug,
                    self.manifest.version
                );
                state.failed = Some(message);
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABI_WAT: &str = r#"(module
        (memory (export "memory") 1)
        (global $next (mut i32) (i32.const 1024))
        (func $alloc (export "wisp_alloc") (param $len i32) (result i32)
            (local $ptr i32)
            (global.set $next
                (i32.and (i32.add (global.get $next) (i32.const 7)) (i32.const -8)))
            (local.set $ptr (global.get $next))
            (global.set $next (i32.add (local.get $ptr) (local.get $len)))
            (local.get $ptr))
        (func (export "wisp_free") (param $ptr i32))
        (data (i32.const 64) "\02\00\00\00{}")
        (func (export "wisp_describe") (result i32) i32.const 64)
        (func (export "wisp_call") (param $op i32) (param $arg i32) (param $len i32) (result i32)
            (local $ptr i32)
            (local.set $ptr (call $alloc (i32.add (local.get $len) (i32.const 4))))
            (i32.store (local.get $ptr) (local.get $len))
            (memory.copy (i32.add (local.get $ptr) (i32.const 4)) (local.get $arg) (local.get $len))
            (local.get $ptr)))"#;

    fn fixture(dir: &Path) -> PathBuf {
        let source = dir.join("source");
        fs::create_dir_all(source.join("icons")).expect("the source tree");
        let mut manifest = crate::test_support::manifest("netease");
        manifest.entry = "lib.wasm".to_owned();
        manifest.version = "1.0.0".to_owned();
        fs::write(
            source.join("wisp.json"),
            serde_json::to_vec(&manifest).expect("the manifest"),
        )
        .expect("wisp.json");
        fs::write(
            source.join("lib.wasm"),
            wat::parse_str(ABI_WAT).expect("the test wasm"),
        )
        .expect("lib.wasm");
        fs::write(source.join("icons/netease.svg"), b"<svg/>").expect("the icon");
        let package = wisp_plugin_pack::pack_dir(&source, None).expect("the package");
        let path = dir.join("netease-1.0.0.wisp");
        wisp_plugin_pack::write_wisp(&package, &path).expect("the flat package");
        path
    }

    fn wat_data(value: &str) -> String {
        (value.len() as u32)
            .to_le_bytes()
            .iter()
            .chain(value.as_bytes())
            .map(|byte| format!("\\{byte:02x}"))
            .collect()
    }

    fn marked_package(dir: &Path, version: &str, marker: &str) -> PathBuf {
        let source = dir.join(format!("source-{version}"));
        fs::create_dir_all(source.join("icons")).expect("the source tree");
        let mut manifest = crate::test_support::manifest("netease");
        manifest.entry = "lib.wasm".to_owned();
        manifest.version = version.to_owned();
        fs::write(
            source.join("wisp.json"),
            serde_json::to_vec(&manifest).expect("the manifest"),
        )
        .expect("wisp.json");
        let wat = ABI_WAT.replace("\\02\\00\\00\\00{}", &wat_data(marker));
        fs::write(
            source.join("lib.wasm"),
            wat::parse_str(&wat).expect("the marked wasm"),
        )
        .expect("lib.wasm");
        fs::write(source.join("icons/netease.svg"), b"<svg/>").expect("the icon");
        let package = wisp_plugin_pack::pack_dir(&source, None).expect("the package");
        let path = dir.join(format!("netease-{version}.wisp"));
        wisp_plugin_pack::write_wisp(&package, &path).expect("the flat package");
        path
    }

    fn trap_package(dir: &Path, version: &str) -> PathBuf {
        let source = dir.join(format!("source-{version}"));
        fs::create_dir_all(source.join("icons")).expect("the source tree");
        let mut manifest = crate::test_support::manifest("netease");
        manifest.entry = "lib.wasm".to_owned();
        manifest.version = version.to_owned();
        fs::write(
            source.join("wisp.json"),
            serde_json::to_vec(&manifest).expect("the manifest"),
        )
        .expect("wisp.json");
        let trap = ABI_WAT.replace(
            r#"(func (export "wisp_call") (param $op i32) (param $arg i32) (param $len i32) (result i32)
            (local $ptr i32)"#,
            r#"(func (export "wisp_call") (param $op i32) (param $arg i32) (param $len i32) (result i32)
            (local $ptr i32)
            unreachable"#,
        );
        fs::write(
            source.join("lib.wasm"),
            wat::parse_str(&trap).expect("the trap wasm"),
        )
        .expect("lib.wasm");
        fs::write(source.join("icons/netease.svg"), b"<svg/>").expect("the icon");
        let package = wisp_plugin_pack::pack_dir(&source, None).expect("the package");
        let path = dir.join(format!("netease-{version}.wisp"));
        wisp_plugin_pack::write_wisp(&package, &path).expect("the flat package");
        path
    }

    #[test]
    fn a_flat_package_instantiates_and_calls_ops() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let package = fixture(dir.path());
        let plugin = WasmPlugin::new(&package).expect("the plugin");
        assert_eq!(plugin.manifest().slug, "netease");
        assert_eq!(plugin.describe().expect("describe"), "{}");
        assert_eq!(plugin.call(0x0400, "{}").expect("call"), "{}");
    }

    #[test]
    fn a_slot_swaps_its_package_in_place() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let one = marked_package(dir.path(), "1.0.0", "one");
        let two = marked_package(dir.path(), "1.1.0", "two");
        let plugin = WasmPlugin::new(&one).expect("the first plugin");
        assert_eq!(plugin.describe().expect("describe"), "one");
        plugin.swap(&two).expect("the hot swap");
        assert_eq!(plugin.describe().expect("describe"), "two");
    }

    #[test]
    fn a_shadow_swap_waits_for_the_active_call() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let one = marked_package(dir.path(), "5.0.0", "one");
        let two = marked_package(dir.path(), "5.1.0", "two");
        let plugin = WasmPlugin::new(&one).expect("the first plugin");
        assert_eq!(plugin.describe().expect("describe"), "one");

        // Hold the state lock as an in-flight wasm call would.
        let guard = plugin.state.lock().expect("the state lock");
        plugin.swap(&two).expect("the shadow stages");
        assert!(plugin.pending_package().is_some());
        assert_eq!(guard.description.as_deref(), Some("one"));

        drop(guard);
        assert_eq!(plugin.describe().expect("describe"), "two");
        assert!(plugin.pending_package().is_none());
    }

    #[test]
    fn a_shadow_with_the_wrong_identity_is_rejected() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let good = marked_package(dir.path(), "6.0.0", "one");
        let wrong = marked_package(dir.path(), "6.1.0", r#"{"id":"other","slug":"netease"}"#);
        let plugin = WasmPlugin::new(&good).expect("the first plugin");
        assert_eq!(plugin.describe().expect("describe"), "one");
        let error = plugin.swap(&wrong).expect_err("the identity must match");
        assert!(
            format!("{error:#}").contains("describes id `other`"),
            "{error:#}"
        );
        assert_eq!(plugin.describe().expect("describe"), "one");
    }

    #[test]
    fn concurrent_calls_survive_a_hot_swap() {
        use std::sync::Barrier;

        let dir = tempfile::tempdir().expect("a temp dir");
        let one = marked_package(dir.path(), "7.0.0", "one");
        let two = marked_package(dir.path(), "7.1.0", "two");
        let plugin = Arc::new(WasmPlugin::new(&one).expect("the first plugin"));
        assert_eq!(plugin.describe().expect("describe"), "one");

        let barrier = Arc::new(Barrier::new(11));
        let mut handles = Vec::new();
        for _ in 0..10 {
            let plugin = Arc::clone(&plugin);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..10 {
                    plugin.call(0, "{}").expect("a concurrent call");
                }
            }));
        }
        barrier.wait();
        plugin.swap(&two).expect("the hot swap");
        for handle in handles {
            handle.join().expect("the caller thread");
        }
        assert_eq!(plugin.describe().expect("describe"), "two");
    }

    #[test]
    fn shared_slots_keep_the_same_handle_across_packages() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let one = marked_package(dir.path(), "2.0.0", "one");
        let two = marked_package(dir.path(), "2.1.0", "two");
        let first = shared(&one, None).expect("the first slot");
        let second = shared(&two, None).expect("the swapped slot");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(second.describe().expect("describe"), "two");
    }

    #[test]
    fn a_slot_rolls_back_after_three_traps() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let good = marked_package(dir.path(), "3.0.0", "good");
        let bad = trap_package(dir.path(), "3.1.0");
        let plugin = WasmPlugin::new(&good).expect("the good plugin");
        plugin.swap(&bad).expect("the bad package loads");
        assert_eq!(plugin.health(), PluginHealth::Active);
        assert!(plugin.call(0, "{}").is_err());
        assert_eq!(plugin.health(), PluginHealth::Active);
        assert!(plugin.call(0, "{}").is_err());
        assert_eq!(plugin.health(), PluginHealth::Active);
        assert!(plugin.call(0, "{}").is_err());
        assert_eq!(plugin.health(), PluginHealth::RolledBack);
        assert_eq!(plugin.describe().expect("describe"), "good");
    }

    #[test]
    fn a_slot_without_a_previous_package_is_quarantined() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let bad = trap_package(dir.path(), "4.0.0");
        let plugin = WasmPlugin::new(&bad).expect("the bad plugin");
        assert!(plugin.call(0, "{}").is_err());
        assert!(plugin.call(0, "{}").is_err());
        assert!(plugin.call(0, "{}").is_err());
        assert!(matches!(plugin.health(), PluginHealth::Quarantined(_)));
        assert!(plugin.call(0, "{}").is_err());
    }

    #[test]
    fn the_trap_window_resets_after_sixty_seconds() {
        let now = Instant::now();
        let mut observation = Observation::new(None, now);
        assert!(!observation.trapped(now));
        assert!(!observation.trapped(now));
        assert!(observation.trapped(now));

        let mut observation = Observation::new(None, now);
        assert!(!observation.trapped(now));
        assert!(!observation.trapped(now));
        let later = now + OBSERVATION_WINDOW + Duration::from_secs(1);
        assert!(!observation.trapped(later));
        assert!(!observation.trapped(later));
        assert!(observation.trapped(later));
    }

    #[test]
    fn latency_samples_roll_back_after_doubling_the_baseline() {
        let now = Instant::now();
        let mut observation = Observation::new(Some(Duration::from_millis(10)), now);
        for _ in 0..18 {
            assert!(!observation.sampled(now, Duration::from_millis(10)));
        }
        assert!(!observation.sampled(now, Duration::from_millis(30)));
        assert!(observation.sampled(now, Duration::from_millis(30)));
    }

    #[test]
    fn the_sign_in_marker_is_independent_of_slot_health() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let package = fixture(dir.path());
        let plugin = WasmPlugin::new(&package).expect("the plugin");
        assert!(!plugin.needs_sign_in());
        plugin.set_sign_in_required(true);
        assert!(plugin.needs_sign_in());
        assert_eq!(plugin.health(), PluginHealth::Active);
        plugin.set_sign_in_required(false);
        assert!(!plugin.needs_sign_in());
    }

    #[test]
    fn a_real_package_runs_pure_ops_when_requested() {
        let Some(path) = std::env::var_os("WISP_NETEASE_WISP") else {
            return;
        };
        let plugin = WasmPlugin::new(Path::new(&path)).expect("the real plugin");
        let args = wisp_plugin_api::CoverAtArgs {
            cover: "https://p1.music.126.net/a.jpg".to_owned(),
            size: wisp_plugin_api::CoverSizeDto::Full,
        };
        let reply = plugin
            .call(
                wisp_plugin_api::ops::music_api::COVER_AT,
                &serde_json::to_string(&args).expect("the args"),
            )
            .expect("the wasm cover_at");
        let reply: wisp_plugin_api::CoverAtReply = serde_json::from_str(&reply).expect("the reply");
        assert!(reply.cover.is_some(), "the plugin answered cover_at");
    }

    struct MockHost;

    impl wisp_plugin_api::Host for MockHost {
        fn http(
            &self,
            request: wisp_plugin_api::HttpRequest,
        ) -> Result<wisp_plugin_api::HttpResponse, wisp_plugin_api::PluginError> {
            assert!(
                request.url.starts_with("https://music.163.com/"),
                "unexpected plugin URL: {}",
                request.url
            );
            let body = if request.url.contains("/api/w/nuser/account/get") {
                r#"{"account":{"id":1},"profile":{"userId":1,"nickname":"Tester","avatarUrl":null}}"#
            } else if request.url.contains("/api/login/qrcode/unikey") {
                r#"{"unikey":"abc","code":200}"#
            } else if request.url.contains("/api/song/enhance/player/url") {
                r#"{"data":[{"url":"https://p1.music.126.net/a.mp3","br":320000,"size":1234,"code":200,"level":"exhigh","expi":3600}]}"#
            } else if request.url.contains("/weapi/") {
                r#"{"code":200}"#
            } else {
                r#"{"result":{"songs":[{"id":1,"name":"晴天","dt":1000,"st":0}]}}"#
            };
            Ok(wisp_plugin_api::HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: body.as_bytes().to_vec(),
            })
        }

        fn secret_get(&self, _key: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn a_real_package_calls_search_through_the_host() {
        let Some(path) = std::env::var_os("WISP_NETEASE_WISP") else {
            return;
        };
        let plugin = WasmPlugin::with_host(Path::new(&path), Some(std::sync::Arc::new(MockHost)))
            .expect("the real plugin");
        let reply = plugin
            .call(
                wisp_plugin_api::ops::music_api::SEARCH,
                r#"{"query":"jay"}"#,
            )
            .expect("the wasm search");
        let reply: wisp_plugin_api::SearchReply = serde_json::from_str(&reply).expect("the reply");
        assert_eq!(reply.tracks.len(), 1);
        assert_eq!(reply.tracks[0].name, "晴天");
    }

    fn sign_in_step(plugin: &WasmPlugin, flow: u32, input: &str) -> wisp_plugin_api::SignInStep {
        let reply = plugin
            .sign_in_step(flow, input)
            .expect("the wasm sign-in step");
        serde_json::from_str(&reply).expect("the parsed step")
    }

    #[test]
    fn a_real_package_runs_sign_in_steps_through_the_host() {
        let Some(path) = std::env::var_os("WISP_NETEASE_WISP") else {
            return;
        };
        let plugin = WasmPlugin::with_host(Path::new(&path), Some(std::sync::Arc::new(MockHost)))
            .expect("the real plugin");

        match sign_in_step(&plugin, wisp_plugin_api::FLOW_QR, "start") {
            wisp_plugin_api::SignInStep::Prompt(wisp_plugin_api::Prompt::Qr { url, .. }) => {
                assert!(url.contains("codekey=abc"), "unexpected qr url: {url}");
            }
            other => panic!("expected a qr prompt, got {other:?}"),
        }
        assert!(matches!(
            sign_in_step(&plugin, wisp_plugin_api::FLOW_PHONE, "start"),
            wisp_plugin_api::SignInStep::Prompt(wisp_plugin_api::Prompt::Phone { .. })
        ));
        assert!(matches!(
            sign_in_step(&plugin, wisp_plugin_api::FLOW_SECRET, "start"),
            wisp_plugin_api::SignInStep::Failed { .. }
        ));
    }

    #[test]
    fn a_real_package_runs_a_write_op_through_the_host() {
        let Some(path) = std::env::var_os("WISP_NETEASE_WISP") else {
            return;
        };
        let plugin = WasmPlugin::with_host(Path::new(&path), Some(std::sync::Arc::new(MockHost)))
            .expect("the real plugin");
        let reply = plugin
            .call(
                wisp_plugin_api::ops::music_api::SET_TRACK_SAVED,
                r#"{"track_id":"1","saved":true}"#,
            )
            .expect("the wasm write");
        let reply: wisp_plugin_api::AckReply = serde_json::from_str(&reply).expect("the ack");
        assert_eq!(reply, wisp_plugin_api::AckReply {});
    }

    #[test]
    fn a_real_package_returns_a_stream_plan() {
        let Some(path) = std::env::var_os("WISP_NETEASE_WISP") else {
            return;
        };
        let plugin = WasmPlugin::with_host(Path::new(&path), Some(std::sync::Arc::new(MockHost)))
            .expect("the real plugin");
        let reply = plugin
            .call(
                wisp_plugin_api::ops::ext::STREAM_RESOLVE,
                r#"{"track_id":"1","quality":"exhigh"}"#,
            )
            .expect("the wasm stream plan");
        let plan: wisp_plugin_api::StreamPlan = serde_json::from_str(&reply).expect("the plan");
        assert_eq!(plan.url, "https://p1.music.126.net/a.mp3");
        assert_eq!(plan.variant, "exhigh");
        assert_eq!(plan.content_length, Some(1234));
        assert!(
            plan.headers
                .iter()
                .any(|header| header.name.eq_ignore_ascii_case("user-agent")),
            "the plan should carry the stream headers"
        );
    }

    #[tokio::test]
    async fn session_host_persists_state_and_guards_keys() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();
        let dir = tempfile::tempdir().expect("a temp dir");
        let data = dir.path().join("data");
        let host = SessionHost::new(
            reqwest::Client::new(),
            tokio::runtime::Handle::current(),
            Vec::new(),
        );
        host.set_storage_dir(data.clone());
        host.set_storage_quota(64);

        host.secret_set("token", "abc").expect("the secret");
        host.config_set("quality", r#""lossless""#)
            .expect("the config");
        host.storage_write("cache.bin", b"xyz")
            .expect("the private file");
        assert!(data.join("secrets.json").is_file());
        assert!(data.join("config.json").is_file());
        assert!(data.join("cache.bin").is_file());

        let fresh = SessionHost::new(
            reqwest::Client::new(),
            tokio::runtime::Handle::current(),
            Vec::new(),
        );
        fresh.set_storage_dir(data.clone());
        assert_eq!(fresh.secret_get("token"), Some("abc".to_owned()));
        assert_eq!(
            fresh.config_get("quality"),
            Some(r#""lossless""#.to_owned())
        );
        assert_eq!(fresh.storage_read("cache.bin"), Some(b"xyz".to_vec()));
        fresh.storage_delete("cache.bin").expect("delete");
        assert_eq!(fresh.storage_read("cache.bin"), None);

        for key in ["", "../escape", "a/b", "a\\b", "C:evil", "config.json"] {
            assert!(
                fresh.storage_write(key, b"x").is_err(),
                "{key:?} must be refused"
            );
        }
        fresh.set_storage_quota(directory_size(&data) + 4);
        let error = fresh
            .storage_write("big.bin", b"12345")
            .expect_err("the quota is enforced");
        assert_eq!(error.code, ErrorCode::TooLarge);
        fresh
            .storage_write("big.bin", b"1234")
            .expect("the file fits");

        fresh
            .secret_set("cookie", "MUSIC_U=1")
            .expect("the session");
        assert_eq!(fresh.secret_get("cookie"), Some("MUSIC_U=1".to_owned()));
        let reloaded = SessionHost::new(
            reqwest::Client::new(),
            tokio::runtime::Handle::current(),
            Vec::new(),
        );
        reloaded.set_storage_dir(data);
        assert_eq!(reloaded.secret_get("cookie"), None);
    }
}
