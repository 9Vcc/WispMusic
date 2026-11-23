//! Host-side plugin runtime.
//!
//! S2 slice: wasmtime load, the JSON memory protocol, the first host import table and the
//! manifest-scoped HTTP/secret broker.

#![forbid(unsafe_code)]

mod policy;

pub use policy::PolicyHost;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::Result;
use arc_swap::ArcSwap;
use base64::Engine as _;
use wasmtime::{
    Caller, Config, Engine, Instance, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, TypedFunc,
};
use wisp_plugin_api::{
    ErrorCode, Host, HttpRequest, HttpResponse, LogLevel, MAX_REPLY_BYTES, PluginError,
    REPLY_HEADER_BYTES,
};

const BAD_POINTER: i32 = -1;
const MISSING: i32 = -2;
const TOO_SMALL: i32 = -3;
const HOST_ERROR: i32 = -4;

/// Per-call and per-instance limits. The defaults mirror the plan's §9.3 table.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub fuel_per_call: u64,
    pub epoch_ticks_per_call: u64,
    pub max_memory_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // About two billion instructions; a run-of-the-mill op uses far less.
            fuel_per_call: 2_000_000_000,
            // The ticker fires every 10 ms, so 3000 ticks is about 30 seconds.
            epoch_ticks_per_call: 3_000,
            max_memory_bytes: 128 * 1024 * 1024,
        }
    }
}

pub struct Runtime {
    engine: Engine,
    limits: Limits,
    stop: Arc<AtomicBool>,
    ticker: Option<JoinHandle<()>>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(ticker) = self.ticker.take() {
            let _ = ticker.join();
        }
    }
}

impl Runtime {
    pub fn new() -> Self {
        Self::with_limits(Limits::default())
    }

    pub fn with_limits(limits: Limits) -> Self {
        let mut config = Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        let engine = Engine::new(&config).expect("the wasmtime engine builds");
        let stop = Arc::new(AtomicBool::new(false));
        let ticker_engine = engine.clone();
        let ticker_stop = Arc::clone(&stop);
        let ticker = std::thread::Builder::new()
            .name("wisp-plugin-epoch".to_owned())
            .spawn(move || {
                while !ticker_stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(10));
                    ticker_engine.increment_epoch();
                }
            })
            .expect("the epoch ticker starts");
        Self {
            engine,
            limits,
            stop,
            ticker: Some(ticker),
        }
    }

    pub fn compile(&self, wasm: &[u8]) -> Result<PluginModule> {
        let module = Module::new(&self.engine, wasm)
            .map_err(|error| anyhow::anyhow!("cannot compile the plugin module: {error}"))?;
        Ok(PluginModule { module })
    }

    /// Compiles and instantiates a plugin with no host services attached.
    pub fn load(&self, wasm: &[u8]) -> Result<LoadedPlugin> {
        self.load_with_host(wasm, Arc::new(NullHost))
    }

    /// Compiles and instantiates a plugin that exports the JSON memory ABI and imports the
    /// `env` host table.
    pub fn load_with_host(&self, wasm: &[u8], host: Arc<dyn Host>) -> Result<LoadedPlugin> {
        let module = self.compile(wasm)?;
        let mut linker = Linker::new(&self.engine);
        register_imports(&mut linker)?;
        let limits = StoreLimitsBuilder::new()
            .memory_size(self.limits.max_memory_bytes)
            .build();
        let mut store = Store::new(
            &self.engine,
            StoreData {
                host,
                limits,
                traps: 0,
            },
        );
        store.limiter(|data| &mut data.limits);
        store
            .set_fuel(self.limits.fuel_per_call)
            .map_err(|error| anyhow::anyhow!("cannot set the plugin fuel budget: {error}"))?;
        store.set_epoch_deadline(self.limits.epoch_ticks_per_call);
        let instance = linker
            .instantiate(&mut store, &module.module)
            .map_err(|error| anyhow::anyhow!("cannot instantiate the plugin module: {error}"))?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| anyhow::anyhow!("the plugin does not export `memory`"))?;
        let alloc: TypedFunc<u32, u32> = instance
            .get_typed_func(&mut store, "wisp_alloc")
            .map_err(|error| anyhow::anyhow!("the plugin has no wisp_alloc: {error}"))?;
        let free: TypedFunc<u32, ()> = instance
            .get_typed_func(&mut store, "wisp_free")
            .map_err(|error| anyhow::anyhow!("the plugin has no wisp_free: {error}"))?;
        let describe: TypedFunc<(), u32> = instance
            .get_typed_func(&mut store, "wisp_describe")
            .map_err(|error| anyhow::anyhow!("the plugin has no wisp_describe: {error}"))?;
        let call: TypedFunc<(u32, u32, u32), u32> = instance
            .get_typed_func(&mut store, "wisp_call")
            .map_err(|error| anyhow::anyhow!("the plugin has no wisp_call: {error}"))?;
        // Sign-in is optional in the first ABI: read-only plugins and the echo fixture do not
        // export it, and only a caller that actually signs in needs the error.
        let sign_in_step = instance
            .get_typed_func(&mut store, "wisp_sign_in_step")
            .ok();
        Ok(LoadedPlugin {
            store,
            instance,
            memory,
            alloc,
            free,
            describe,
            call,
            sign_in_step,
            fuel_per_call: self.limits.fuel_per_call,
            epoch_ticks_per_call: self.limits.epoch_ticks_per_call,
        })
    }

    /// Calls an exported `i32 -> i32` function. This is the runtime smoke path; the JSON ABI
    /// dispatcher is [`LoadedPlugin::call`].
    pub fn call_i32(&self, plugin: &PluginModule, name: &str, arg: i32) -> Result<i32> {
        let mut store = Store::new(&self.engine, ());
        store
            .set_fuel(self.limits.fuel_per_call)
            .map_err(|error| anyhow::anyhow!("cannot set the plugin fuel budget: {error}"))?;
        store.set_epoch_deadline(self.limits.epoch_ticks_per_call);
        let instance = Instance::new(&mut store, &plugin.module, &[])
            .map_err(|error| anyhow::anyhow!("cannot instantiate the plugin module: {error}"))?;
        let function: TypedFunc<i32, i32> =
            instance.get_typed_func(&mut store, name).map_err(|error| {
                anyhow::anyhow!("the plugin does not export {name} as i32 -> i32: {error}")
            })?;
        function
            .call(&mut store, arg)
            .map_err(|error| anyhow::anyhow!("the plugin call trapped: {error}"))
    }
}

struct StoreData {
    host: Arc<dyn Host>,
    limits: StoreLimits,
    traps: u32,
}

struct NullHost;

impl Host for NullHost {
    fn http(&self, _request: HttpRequest) -> Result<HttpResponse, PluginError> {
        Err(PluginError::new(ErrorCode::Network, "no host attached"))
    }

    fn secret_get(&self, _key: &str) -> Option<String> {
        None
    }
}

/// The `env` imports a plugin may call. Host calls are synchronous; the caller runs on a
/// dedicated plugin thread.
fn register_imports(linker: &mut Linker<StoreData>) -> Result<()> {
    linker
        .func_wrap(
            "env",
            "wisp_log",
            |mut caller: Caller<'_, StoreData>, level: u32, ptr: u32, len: u32| {
                if let Some(bytes) = read(&mut caller, ptr, len)
                    && let Ok(text) = String::from_utf8(bytes)
                {
                    let level = match level {
                        0 => LogLevel::Error,
                        1 => LogLevel::Warn,
                        2 => LogLevel::Info,
                        3 => LogLevel::Debug,
                        _ => LogLevel::Trace,
                    };
                    caller.data().host.log(level, &text);
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register wisp_log: {error}"))?;

    linker
        .func_wrap(
            "env",
            "wisp_now_ms",
            |caller: Caller<'_, StoreData>| -> i64 { caller.data().host.now_ms() as i64 },
        )
        .map_err(|error| anyhow::anyhow!("cannot register wisp_now_ms: {error}"))?;

    linker
        .func_wrap(
            "env",
            "wisp_random_bytes",
            |mut caller: Caller<'_, StoreData>, ptr: u32, len: u32| -> i32 {
                if len == 0 {
                    return 0;
                }
                let bytes = caller.data().host.random_bytes(len as usize);
                if bytes.len() != len as usize {
                    return HOST_ERROR;
                }
                match write(&mut caller, ptr, len, &bytes) {
                    Ok(_) => 0,
                    Err(code) => code,
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register wisp_random_bytes: {error}"))?;

    // `wasm32-wasip1` imports this for the plugin's RNG (fastrand). The ABI stays on the
    // host-provided random source, but a WASI-built module still expects the import to exist.
    linker
        .func_wrap(
            "wasi_snapshot_preview1",
            "random_get",
            |mut caller: Caller<'_, StoreData>, ptr: u32, len: u32| -> i32 {
                if len == 0 {
                    return 0;
                }
                let bytes = caller.data().host.random_bytes(len as usize);
                if bytes.len() != len as usize {
                    return HOST_ERROR;
                }
                match write(&mut caller, ptr, len, &bytes) {
                    Ok(_) => 0,
                    Err(code) => code,
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register wasi random_get: {error}"))?;

    linker
        .func_wrap(
            "env",
            "wisp_secret_get",
            |mut caller: Caller<'_, StoreData>,
             key_ptr: u32,
             key_len: u32,
             out_ptr: u32,
             out_cap: u32|
             -> i32 {
                let Some(key_bytes) = read(&mut caller, key_ptr, key_len) else {
                    return BAD_POINTER;
                };
                let Ok(key) = String::from_utf8(key_bytes) else {
                    return BAD_POINTER;
                };
                let Some(secret) = caller.data().host.secret_get(&key) else {
                    return MISSING;
                };
                match write(&mut caller, out_ptr, out_cap, secret.as_bytes()) {
                    Ok(len) => len as i32,
                    Err(code) => code,
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register wisp_secret_get: {error}"))?;

    linker
        .func_wrap(
            "env",
            "wisp_secret_set",
            |mut caller: Caller<'_, StoreData>,
             key_ptr: u32,
             key_len: u32,
             value_ptr: u32,
             value_len: u32|
             -> i32 {
                let Some(key) = read_string(&mut caller, key_ptr, key_len) else {
                    return BAD_POINTER;
                };
                let Some(value) = read_string(&mut caller, value_ptr, value_len) else {
                    return BAD_POINTER;
                };
                match caller.data().host.secret_set(&key, &value) {
                    Ok(()) => 0,
                    Err(_) => HOST_ERROR,
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register wisp_secret_set: {error}"))?;

    linker
        .func_wrap(
            "env",
            "wisp_config_get",
            |mut caller: Caller<'_, StoreData>,
             key_ptr: u32,
             key_len: u32,
             out_ptr: u32,
             out_cap: u32|
             -> i32 {
                let Some(key) = read_string(&mut caller, key_ptr, key_len) else {
                    return BAD_POINTER;
                };
                let Some(value) = caller.data().host.config_get(&key) else {
                    return MISSING;
                };
                match write(&mut caller, out_ptr, out_cap, value.as_bytes()) {
                    Ok(len) => len as i32,
                    Err(code) => code,
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register wisp_config_get: {error}"))?;

    linker
        .func_wrap(
            "env",
            "wisp_config_set",
            |mut caller: Caller<'_, StoreData>,
             key_ptr: u32,
             key_len: u32,
             value_ptr: u32,
             value_len: u32|
             -> i32 {
                let Some(key) = read_string(&mut caller, key_ptr, key_len) else {
                    return BAD_POINTER;
                };
                let Some(value) = read_string(&mut caller, value_ptr, value_len) else {
                    return BAD_POINTER;
                };
                match caller.data().host.config_set(&key, &value) {
                    Ok(()) => 0,
                    Err(_) => HOST_ERROR,
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register wisp_config_set: {error}"))?;

    linker
        .func_wrap(
            "env",
            "wisp_storage_get",
            |mut caller: Caller<'_, StoreData>,
             key_ptr: u32,
             key_len: u32,
             out_ptr: u32,
             out_cap: u32|
             -> i32 {
                let Some(key) = read_string(&mut caller, key_ptr, key_len) else {
                    return BAD_POINTER;
                };
                let Some(value) = caller.data().host.storage_read(&key) else {
                    return MISSING;
                };
                match write(&mut caller, out_ptr, out_cap, &value) {
                    Ok(len) => len as i32,
                    Err(code) => code,
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register wisp_storage_get: {error}"))?;

    linker
        .func_wrap(
            "env",
            "wisp_storage_set",
            |mut caller: Caller<'_, StoreData>,
             key_ptr: u32,
             key_len: u32,
             value_ptr: u32,
             value_len: u32|
             -> i32 {
                let Some(key) = read_string(&mut caller, key_ptr, key_len) else {
                    return BAD_POINTER;
                };
                let Some(value) = read(&mut caller, value_ptr, value_len) else {
                    return BAD_POINTER;
                };
                match caller.data().host.storage_write(&key, &value) {
                    Ok(()) => 0,
                    Err(_) => HOST_ERROR,
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register wisp_storage_set: {error}"))?;

    linker
        .func_wrap(
            "env",
            "wisp_storage_delete",
            |mut caller: Caller<'_, StoreData>, key_ptr: u32, key_len: u32| -> i32 {
                let Some(key) = read_string(&mut caller, key_ptr, key_len) else {
                    return BAD_POINTER;
                };
                match caller.data().host.storage_delete(&key) {
                    Ok(()) => 0,
                    Err(_) => HOST_ERROR,
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register wisp_storage_delete: {error}"))?;

    linker
        .func_wrap(
            "env",
            "wisp_http",
            |mut caller: Caller<'_, StoreData>,
             req_ptr: u32,
             req_len: u32,
             out_ptr: u32,
             out_cap: u32|
             -> i64 {
                let Some(req_bytes) = read(&mut caller, req_ptr, req_len) else {
                    return BAD_POINTER as i64;
                };
                let Ok(request) = serde_json::from_slice::<HttpRequest>(&req_bytes) else {
                    return BAD_POINTER as i64;
                };
                let host = caller.data().host.clone();
                let Ok(response) = host.http(request) else {
                    return HOST_ERROR as i64;
                };
                let Ok(bytes) = serde_json::to_vec(&response) else {
                    return HOST_ERROR as i64;
                };
                match write(&mut caller, out_ptr, out_cap, &bytes) {
                    Ok(len) => len as i64,
                    Err(code) => code as i64,
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register wisp_http: {error}"))?;

    // Same broker, compact body: the guest parses a base64 string instead of a JSON number
    // array, which keeps large song/playlist pages out of the WASM parser's stack.
    linker
        .func_wrap(
            "env",
            "wisp_http_b64",
            |mut caller: Caller<'_, StoreData>,
             req_ptr: u32,
             req_len: u32,
             out_ptr: u32,
             out_cap: u32|
             -> i64 {
                let Some(req_bytes) = read(&mut caller, req_ptr, req_len) else {
                    return BAD_POINTER as i64;
                };
                let Ok(request) = serde_json::from_slice::<HttpRequest>(&req_bytes) else {
                    return BAD_POINTER as i64;
                };
                let host = caller.data().host.clone();
                let Ok(response) = host.http(request) else {
                    return HOST_ERROR as i64;
                };
                let value = serde_json::json!({
                    "status": response.status,
                    "headers": response.headers,
                    "body": base64::engine::general_purpose::STANDARD.encode(&response.body),
                });
                let Ok(bytes) = serde_json::to_vec(&value) else {
                    return HOST_ERROR as i64;
                };
                match write(&mut caller, out_ptr, out_cap, &bytes) {
                    Ok(len) => len as i64,
                    Err(code) => code as i64,
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register wisp_http_b64: {error}"))?;

    linker
        .func_wrap(
            "env",
            "wisp_http_many",
            |mut caller: Caller<'_, StoreData>,
             req_ptr: u32,
             req_len: u32,
             out_ptr: u32,
             out_cap: u32|
             -> i64 {
                let Some(req_bytes) = read(&mut caller, req_ptr, req_len) else {
                    return BAD_POINTER as i64;
                };
                let Ok(requests) = serde_json::from_slice::<Vec<HttpRequest>>(&req_bytes) else {
                    return BAD_POINTER as i64;
                };
                let host = caller.data().host.clone();
                let responses = host.http_many(requests);
                let Ok(bytes) = serde_json::to_vec(&responses) else {
                    return HOST_ERROR as i64;
                };
                match write(&mut caller, out_ptr, out_cap, &bytes) {
                    Ok(len) => len as i64,
                    Err(code) => code as i64,
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register wisp_http_many: {error}"))?;

    // Rust std on wasm32-wasip1 imports these even when the plugin never touches the
    // filesystem or the clock. They are intentionally tiny: no files, no environment, a clock
    // from the host and stderr forwarded to the plugin log.
    linker
        .func_wrap(
            "wasi_snapshot_preview1",
            "environ_sizes_get",
            |mut caller: Caller<'_, StoreData>, count_ptr: u32, size_ptr: u32| -> i32 {
                if write(&mut caller, count_ptr, 4, &0u32.to_le_bytes()).is_err()
                    || write(&mut caller, size_ptr, 4, &0u32.to_le_bytes()).is_err()
                {
                    return 21;
                }
                0
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register environ_sizes_get: {error}"))?;

    linker
        .func_wrap(
            "wasi_snapshot_preview1",
            "environ_get",
            |_caller: Caller<'_, StoreData>, _environ_ptr: u32, _buf_ptr: u32| -> i32 { 0 },
        )
        .map_err(|error| anyhow::anyhow!("cannot register environ_get: {error}"))?;

    linker
        .func_wrap(
            "wasi_snapshot_preview1",
            "clock_time_get",
            |mut caller: Caller<'_, StoreData>,
             _clock_id: i32,
             _precision: i64,
             out_ptr: u32|
             -> i32 {
                let nanos = caller.data().host.now_ms().saturating_mul(1_000_000);
                match write(&mut caller, out_ptr, 8, &nanos.to_le_bytes()) {
                    Ok(_) => 0,
                    Err(code) => code,
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register clock_time_get: {error}"))?;

    linker
        .func_wrap(
            "wasi_snapshot_preview1",
            "fd_write",
            |mut caller: Caller<'_, StoreData>,
             _fd: i32,
             iovs_ptr: u32,
             iovs_len: u32,
             written_ptr: u32|
             -> i32 {
                let mut written = 0usize;
                let mut text = String::new();
                for index in 0..iovs_len.min(16) {
                    let Some(base) = iovs_ptr.checked_add(index * 8) else {
                        break;
                    };
                    let Some(ptr) = read_u32(&mut caller, base) else {
                        break;
                    };
                    let Some(len) = read_u32(&mut caller, base + 4) else {
                        break;
                    };
                    if let Some(bytes) = read(&mut caller, ptr, len) {
                        written = written.saturating_add(bytes.len());
                        text.push_str(&String::from_utf8_lossy(&bytes));
                    }
                }
                if !text.is_empty() {
                    caller.data().host.log(LogLevel::Debug, &text);
                }
                match write(&mut caller, written_ptr, 4, &(written as u32).to_le_bytes()) {
                    Ok(_) => 0,
                    Err(code) => code,
                }
            },
        )
        .map_err(|error| anyhow::anyhow!("cannot register fd_write: {error}"))?;

    linker
        .func_wrap(
            "wasi_snapshot_preview1",
            "proc_exit",
            |_caller: Caller<'_, StoreData>, _code: i32| {},
        )
        .map_err(|error| anyhow::anyhow!("cannot register proc_exit: {error}"))?;

    Ok(())
}

fn read_u32<T>(caller: &mut Caller<'_, T>, ptr: u32) -> Option<u32> {
    let bytes = read(caller, ptr, 4)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn memory<T>(caller: &mut Caller<'_, T>) -> Option<Memory> {
    caller
        .get_export("memory")
        .and_then(|export| export.into_memory())
}

fn read<T>(caller: &mut Caller<'_, T>, ptr: u32, len: u32) -> Option<Vec<u8>> {
    let memory = memory(caller)?;
    let start = ptr as usize;
    let end = start.checked_add(len as usize)?;
    if start == 0 || end > memory.data(&*caller).len() {
        return None;
    }
    Some(memory.data(&*caller)[start..end].to_vec())
}

fn read_string<T>(caller: &mut Caller<'_, T>, ptr: u32, len: u32) -> Option<String> {
    String::from_utf8(read(caller, ptr, len)?).ok()
}

fn write<T>(
    caller: &mut Caller<'_, T>,
    ptr: u32,
    cap: u32,
    bytes: &[u8],
) -> std::result::Result<usize, i32> {
    if bytes.len() > cap as usize {
        return Err(TOO_SMALL);
    }
    if bytes.is_empty() {
        return Ok(0);
    }
    let memory = memory(caller).ok_or(BAD_POINTER)?;
    let start = ptr as usize;
    let end = start.checked_add(bytes.len()).ok_or(BAD_POINTER)?;
    if start == 0 || end > memory.data(&*caller).len() {
        return Err(BAD_POINTER);
    }
    memory.data_mut(&mut *caller)[start..end].copy_from_slice(bytes);
    Ok(bytes.len())
}

pub struct PluginModule {
    module: Module,
}

/// A live plugin instance. Replies are `[u32 little-endian length][UTF-8 JSON]` buffers in the
/// plugin's linear memory; the host copies them out and then calls `wisp_free`.
pub struct LoadedPlugin {
    store: Store<StoreData>,
    #[cfg_attr(not(test), allow(dead_code))]
    instance: Instance,
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    free: TypedFunc<u32, ()>,
    describe: TypedFunc<(), u32>,
    call: TypedFunc<(u32, u32, u32), u32>,
    sign_in_step: Option<TypedFunc<(u32, u32, u32), u32>>,
    fuel_per_call: u64,
    epoch_ticks_per_call: u64,
}

impl LoadedPlugin {
    pub fn describe(&mut self) -> Result<String> {
        self.reset_watchdog()?;
        let pointer = match self.describe.call(&mut self.store, ()) {
            Ok(pointer) => pointer,
            Err(error) => {
                self.note_trap();
                return Err(anyhow::anyhow!("wisp_describe trapped: {error}"));
            }
        };
        let reply = self.read_reply(pointer);
        if reply.is_err() {
            self.note_trap();
        }
        reply
    }

    /// Calls `wisp_call(op, arg_ptr, arg_len)`. The host owns the argument buffer and frees it
    /// after the call; the reply is length-prefixed and freed after it is copied out.
    pub fn call(&mut self, op: u32, arg: &str) -> Result<String> {
        self.reset_watchdog()?;
        let bytes = arg.as_bytes();
        let len = u32::try_from(bytes.len())
            .map_err(|_| anyhow::anyhow!("the argument exceeds u32 bytes"))?;
        let pointer = if bytes.is_empty() {
            0
        } else {
            let pointer = match self.alloc.call(&mut self.store, len) {
                Ok(pointer) => pointer,
                Err(error) => {
                    self.note_trap();
                    return Err(anyhow::anyhow!("wisp_alloc trapped: {error}"));
                }
            };
            self.write(pointer, bytes)?;
            pointer
        };

        let reply = match self.call.call(&mut self.store, (op, pointer, len)) {
            Ok(pointer) => pointer,
            Err(error) => {
                self.note_trap();
                return Err(anyhow::anyhow!("wisp_call trapped: {error}"));
            }
        };
        if pointer != 0
            && let Err(error) = self.free.call(&mut self.store, pointer)
        {
            self.note_trap();
            return Err(anyhow::anyhow!("wisp_free trapped: {error}"));
        }
        let reply = self.read_reply(reply);
        if reply.is_err() {
            self.note_trap();
        }
        reply
    }

    /// Calls `wisp_sign_in_step(flow, input_ptr, input_len)`. The input is the host's opaque
    /// sign-in input (an empty string for the first call); the reply is the same length-prefixed
    /// JSON used by `wisp_call`.
    pub fn sign_in_step(&mut self, flow: u32, input: &str) -> Result<String> {
        self.reset_watchdog()?;
        let bytes = input.as_bytes();
        let len = u32::try_from(bytes.len())
            .map_err(|_| anyhow::anyhow!("the sign-in input exceeds u32 bytes"))?;
        let pointer = if bytes.is_empty() {
            0
        } else {
            let pointer = match self.alloc.call(&mut self.store, len) {
                Ok(pointer) => pointer,
                Err(error) => {
                    self.note_trap();
                    return Err(anyhow::anyhow!("wisp_alloc trapped: {error}"));
                }
            };
            self.write(pointer, bytes)?;
            pointer
        };

        let result = match &self.sign_in_step {
            Some(step) => step.call(&mut self.store, (flow, pointer, len)),
            None => {
                if pointer != 0 {
                    let _ = self.free.call(&mut self.store, pointer);
                }
                return Err(anyhow::anyhow!(
                    "the plugin does not export wisp_sign_in_step"
                ));
            }
        };
        if pointer != 0
            && let Err(error) = self.free.call(&mut self.store, pointer)
        {
            self.note_trap();
            return Err(anyhow::anyhow!("wisp_free trapped: {error}"));
        }
        let reply = match result {
            Ok(pointer) => self.read_reply(pointer),
            Err(error) => {
                self.note_trap();
                Err(anyhow::anyhow!("wisp_sign_in_step trapped: {error}"))
            }
        };
        if reply.is_err() {
            self.note_trap();
        }
        reply
    }

    /// How many plugin calls have trapped or returned a malformed reply.
    pub fn trap_count(&self) -> u32 {
        self.store.data().traps
    }

    fn note_trap(&mut self) {
        let data = self.store.data_mut();
        data.traps = data.traps.saturating_add(1);
    }

    fn reset_watchdog(&mut self) -> Result<()> {
        self.store
            .set_fuel(self.fuel_per_call)
            .map_err(|error| anyhow::anyhow!("cannot refuel the plugin: {error}"))?;
        self.store.set_epoch_deadline(self.epoch_ticks_per_call);
        Ok(())
    }

    fn write(&mut self, pointer: u32, bytes: &[u8]) -> Result<()> {
        let start = self.checked(pointer, bytes.len())?;
        self.memory.data_mut(&mut self.store)[start..start + bytes.len()].copy_from_slice(bytes);
        Ok(())
    }

    /// Validates a `[len][payload]` buffer and returns its payload.
    fn read_reply(&mut self, pointer: u32) -> Result<String> {
        let header = self.checked(pointer, REPLY_HEADER_BYTES as usize)?;
        let data = self.memory.data(&self.store);
        let len = u32::from_le_bytes(
            data[header..header + REPLY_HEADER_BYTES as usize]
                .try_into()
                .expect("the header is four bytes"),
        ) as usize;
        if len as u64 > MAX_REPLY_BYTES {
            let _ = self.free.call(&mut self.store, pointer);
            anyhow::bail!("the plugin reply exceeds the host limit ({len} bytes)");
        }
        let payload_pointer = pointer
            .checked_add(REPLY_HEADER_BYTES)
            .ok_or_else(|| anyhow::anyhow!("the plugin pointer overflows"))?;
        let payload = self.checked(payload_pointer, len)?;
        let bytes = self.memory.data(&self.store)[payload..payload + len].to_vec();
        self.free
            .call(&mut self.store, pointer)
            .map_err(|error| anyhow::anyhow!("wisp_free trapped: {error}"))?;
        String::from_utf8(bytes).map_err(|error| anyhow::anyhow!("the reply is not UTF-8: {error}"))
    }

    /// Returns the start offset and checks that `[start, start + len)` is inside memory.
    fn checked(&self, pointer: u32, len: usize) -> Result<usize> {
        let start = pointer as usize;
        let end = start
            .checked_add(len)
            .ok_or_else(|| anyhow::anyhow!("the plugin pointer overflows"))?;
        if start == 0 || end > self.memory.data(&self.store).len() {
            anyhow::bail!(
                "the plugin pointer {pointer} (+{len}) is outside linear memory ({} bytes)",
                self.memory.data(&self.store).len()
            );
        }
        Ok(start)
    }

    #[cfg(test)]
    fn call_export_i32(&mut self, name: &str) -> Result<i32> {
        let function: TypedFunc<(), i32> = self
            .instance
            .get_typed_func(&mut self.store, name)
            .map_err(|error| anyhow::anyhow!("the plugin has no {name}: {error}"))?;
        function
            .call(&mut self.store, ())
            .map_err(|error| anyhow::anyhow!("{name} trapped: {error}"))
    }

    #[cfg(test)]
    fn call_export_i64(&mut self, name: &str) -> Result<i64> {
        let function: TypedFunc<(), i64> = self
            .instance
            .get_typed_func(&mut self.store, name)
            .map_err(|error| anyhow::anyhow!("the plugin has no {name}: {error}"))?;
        function
            .call(&mut self.store, ())
            .map_err(|error| anyhow::anyhow!("{name} trapped: {error}"))
    }
}

/// The current instance behind a stable provider handle. A hot swap replaces the value; callers
/// already holding an `Arc` finish against the old instance.
pub struct PluginSlot<T> {
    current: ArcSwap<T>,
}

impl<T> PluginSlot<T> {
    pub fn new(value: T) -> Self {
        Self {
            current: ArcSwap::from_pointee(value),
        }
    }

    pub fn load(&self) -> Arc<T> {
        self.current.load_full()
    }

    pub fn swap(&self, value: T) -> Arc<T> {
        self.current.swap(Arc::new(value))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use wisp_plugin_api::Header;

    use super::*;

    #[test]
    fn a_wat_module_compiles_and_runs() {
        let wasm = wat::parse_str(
            r#"(module
                (func (export "echo") (param i32) (result i32)
                    local.get 0))"#,
        )
        .expect("the test module parses");

        let runtime = Runtime::new();
        let plugin = runtime.compile(&wasm).expect("the module compiles");
        assert_eq!(
            runtime
                .call_i32(&plugin, "echo", 42)
                .expect("the call runs"),
            42
        );
    }

    #[test]
    fn a_slot_swaps_and_old_handles_finish_on_the_old_value() {
        let slot = PluginSlot::new(1);
        let old = slot.load();
        assert_eq!(*old, 1);
        let previous = slot.swap(2);
        assert_eq!(*previous, 1);
        assert_eq!(*slot.load(), 2);
        assert_eq!(*old, 1);
    }

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

    #[test]
    fn the_json_memory_protocol_round_trips() {
        let wasm = wat::parse_str(ABI_WAT).expect("the ABI module parses");
        let runtime = Runtime::new();
        let mut plugin = runtime.load(&wasm).expect("the ABI plugin loads");

        assert_eq!(plugin.describe().expect("the manifest reads"), "{}");
        assert_eq!(
            plugin.call(1, r#"{"x":1}"#).expect("the call round trips"),
            r#"{"x":1}"#
        );
        assert_eq!(
            plugin.call(2, "hello").expect("the call round trips"),
            "hello"
        );
    }

    struct TestHost {
        logs: Mutex<Vec<(LogLevel, String)>>,
        secret: Option<String>,
        response: HttpResponse,
    }

    impl Host for TestHost {
        fn http(&self, _request: HttpRequest) -> Result<HttpResponse, PluginError> {
            Ok(self.response.clone())
        }

        fn secret_get(&self, key: &str) -> Option<String> {
            (key == "MUSIC_U").then(|| self.secret.clone()).flatten()
        }

        fn log(&self, level: LogLevel, message: &str) {
            self.logs
                .lock()
                .expect("the log lock")
                .push((level, message.to_owned()));
        }

        fn now_ms(&self) -> u64 {
            123
        }

        fn random_bytes(&self, len: usize) -> Vec<u8> {
            vec![7; len]
        }
    }

    fn import_wat() -> String {
        const REQUEST: &str = r#"{"method":"get","url":"https://example.com"}"#;
        format!(
            r#"(module
                (import "env" "wisp_now_ms" (func $now (result i64)))
                (import "env" "wisp_log" (func $log (param i32 i32 i32)))
                (import "env" "wisp_random_bytes" (func $rand (param i32 i32) (result i32)))
                (import "env" "wisp_secret_get" (func $secret (param i32 i32 i32 i32) (result i32)))
                (import "env" "wisp_http" (func $http (param i32 i32 i32 i32) (result i64)))
                (memory (export "memory") 1)
                (data (i32.const 64) "MUSIC_U")
                (data (i32.const 80) "{}")
                (data (i32.const 4096) "hello")
                (func (export "wisp_alloc") (param i32) (result i32) i32.const 1024)
                (func (export "wisp_free") (param i32))
                (func (export "wisp_describe") (result i32) i32.const 64)
                (func (export "wisp_call") (param i32 i32 i32) (result i32) i32.const 64)
                (func (export "call_now") (result i64) call $now)
                (func (export "call_log") (result i32)
                    i32.const 0 i32.const 4096 i32.const 5 call $log i32.const 0)
                (func (export "call_random") (result i32)
                    i32.const 2048 i32.const 8 call $rand)
                (func (export "call_secret") (result i32)
                    i32.const 64 i32.const 7 i32.const 1024 i32.const 64 call $secret)
                (func (export "call_http") (result i64)
                    i32.const 80 i32.const {} i32.const 8192 i32.const 8192 call $http))"#,
            REQUEST.replace('"', "\\\""),
            REQUEST.len()
        )
    }

    #[test]
    fn the_host_import_table_answers() {
        let host = Arc::new(TestHost {
            logs: Mutex::new(Vec::new()),
            secret: Some("MUSIC_U=test".to_owned()),
            response: HttpResponse {
                status: 200,
                headers: vec![Header {
                    name: "x-test".to_owned(),
                    value: "1".to_owned(),
                }],
                body: b"ok".to_vec(),
            },
        });
        let wasm = wat::parse_str(import_wat()).expect("the import module parses");
        let runtime = Runtime::new();
        let mut plugin = runtime
            .load_with_host(&wasm, host.clone())
            .expect("the plugin loads");

        assert_eq!(plugin.call_export_i64("call_now").expect("now"), 123);
        assert_eq!(plugin.call_export_i32("call_log").expect("log"), 0);
        assert_eq!(
            host.logs.lock().expect("the log lock").as_slice(),
            [(LogLevel::Error, "hello".to_owned())]
        );

        assert_eq!(plugin.call_export_i32("call_random").expect("random"), 0);
        assert_eq!(&plugin.memory.data(&plugin.store)[2048..2056], &[7; 8]);

        let secret_len = plugin.call_export_i32("call_secret").expect("secret");
        assert_eq!(secret_len, 12);
        assert_eq!(
            &plugin.memory.data(&plugin.store)[1024..1036],
            b"MUSIC_U=test"
        );

        let http_len = plugin.call_export_i64("call_http").expect("http");
        let response: HttpResponse = serde_json::from_slice(
            &plugin.memory.data(&plugin.store)[8192..8192 + http_len as usize],
        )
        .expect("the response parses");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"ok");
        assert_eq!(response.headers[0].name, "x-test");
    }

    #[derive(Default)]
    struct StateHost {
        secrets: Mutex<std::collections::HashMap<String, String>>,
        config: Mutex<std::collections::HashMap<String, String>>,
        storage: Mutex<std::collections::HashMap<String, Vec<u8>>>,
    }

    impl Host for StateHost {
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

    const STATE_WAT: &str = r#"(module
        (import "env" "wisp_secret_get" (func $secret_get (param i32 i32 i32 i32) (result i32)))
        (import "env" "wisp_secret_set" (func $secret_set (param i32 i32 i32 i32) (result i32)))
        (import "env" "wisp_config_get" (func $config_get (param i32 i32 i32 i32) (result i32)))
        (import "env" "wisp_config_set" (func $config_set (param i32 i32 i32 i32) (result i32)))
        (import "env" "wisp_storage_get" (func $storage_get (param i32 i32 i32 i32) (result i32)))
        (import "env" "wisp_storage_set" (func $storage_set (param i32 i32 i32 i32) (result i32)))
        (import "env" "wisp_storage_delete" (func $storage_delete (param i32 i32) (result i32)))
        (memory (export "memory") 1)
        (data (i32.const 64) "token")
        (data (i32.const 80) "abc")
        (data (i32.const 96) "missing")
        (func (export "wisp_alloc") (param i32) (result i32) i32.const 512)
        (func (export "wisp_free") (param i32))
        (func (export "wisp_describe") (result i32) i32.const 64)
        (func (export "wisp_call") (param i32 i32 i32) (result i32) i32.const 64)
        (func (export "set_secret") (result i32)
            i32.const 64 i32.const 5 i32.const 80 i32.const 3 call $secret_set)
        (func (export "get_secret") (result i32)
            i32.const 64 i32.const 5 i32.const 1024 i32.const 64 call $secret_get)
        (func (export "get_missing") (result i32)
            i32.const 96 i32.const 7 i32.const 1024 i32.const 64 call $secret_get)
        (func (export "set_config") (result i32)
            i32.const 64 i32.const 5 i32.const 80 i32.const 3 call $config_set)
        (func (export "get_config") (result i32)
            i32.const 64 i32.const 5 i32.const 1024 i32.const 64 call $config_get)
        (func (export "set_storage") (result i32)
            i32.const 64 i32.const 5 i32.const 80 i32.const 3 call $storage_set)
        (func (export "get_storage") (result i32)
            i32.const 64 i32.const 5 i32.const 1024 i32.const 64 call $storage_get)
        (func (export "delete_storage") (result i32)
            i32.const 64 i32.const 5 call $storage_delete))"#;

    #[test]
    fn the_state_imports_round_trip() {
        let host = Arc::new(StateHost::default());
        let wasm = wat::parse_str(STATE_WAT).expect("the state module parses");
        let mut plugin = Runtime::new()
            .load_with_host(&wasm, host.clone())
            .expect("the plugin loads");

        assert_eq!(
            plugin.call_export_i32("get_missing").expect("missing"),
            MISSING
        );
        assert_eq!(plugin.call_export_i32("set_secret").expect("set secret"), 0);
        assert_eq!(plugin.call_export_i32("get_secret").expect("get secret"), 3);
        assert_eq!(&plugin.memory.data(&plugin.store)[1024..1027], b"abc");
        assert_eq!(
            host.secrets
                .lock()
                .expect("the secret lock")
                .get("token")
                .map(String::as_str),
            Some("abc")
        );

        assert_eq!(plugin.call_export_i32("set_config").expect("set config"), 0);
        assert_eq!(plugin.call_export_i32("get_config").expect("get config"), 3);
        assert_eq!(&plugin.memory.data(&plugin.store)[1024..1027], b"abc");

        assert_eq!(
            plugin.call_export_i32("set_storage").expect("set storage"),
            0
        );
        assert_eq!(
            plugin.call_export_i32("get_storage").expect("get storage"),
            3
        );
        assert_eq!(&plugin.memory.data(&plugin.store)[1024..1027], b"abc");
        assert_eq!(plugin.call_export_i32("delete_storage").expect("delete"), 0);
        assert!(host.storage.lock().expect("the storage lock").is_empty());
    }

    const TRAP_WAT: &str = r#"(module
        (memory (export "memory") 1)
        (func (export "wisp_alloc") (param i32) (result i32) i32.const 1024)
        (func (export "wisp_free") (param i32))
        (data (i32.const 64) "\02\00\00\00{}")
        (func (export "wisp_describe") (result i32) i32.const 64)
        (func (export "wisp_call") (param i32 i32 i32) (result i32)
            (loop $l (br $l)) unreachable))"#;

    #[test]
    fn a_runaway_call_traps_and_is_counted() {
        let wasm = wat::parse_str(TRAP_WAT).expect("the trap module parses");
        let runtime = Runtime::with_limits(Limits {
            fuel_per_call: 1_000_000,
            epoch_ticks_per_call: 1_000_000_000,
            max_memory_bytes: 128 * 1024 * 1024,
        });
        let mut plugin = runtime.load(&wasm).expect("the trap plugin loads");
        assert_eq!(plugin.trap_count(), 0);
        assert!(plugin.call(1, "{}").is_err());
        assert_eq!(plugin.trap_count(), 1);
    }

    #[test]
    fn the_memory_limit_rejects_an_oversized_module() {
        let wasm = wat::parse_str(r#"(module (memory (export "memory") 2049))"#)
            .expect("the oversized module parses");
        assert!(Runtime::new().load(&wasm).is_err());
    }
}
