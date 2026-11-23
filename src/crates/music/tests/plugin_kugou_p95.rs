//! K1 boundary latency: the real Kugou package with a canned search host.
//!
//! Run manually with `WISP_KUGOU_WISP=<package> cargo test -p music --test plugin_kugou_p95 -- --nocapture`.
//! The test skips when no package is supplied, like the other real-wasm tests.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use music::plugins::runtime::WasmPlugin;
use serde_json::json;
use wisp_plugin_api::ops;
use wisp_plugin_api::{ErrorCode, Host, HttpRequest, HttpResponse, PluginError};

const SEARCH_ROUNDS: usize = 200;
const SEARCH_P95: Duration = Duration::from_millis(8);

struct P95Host {
    search: String,
    storage: Mutex<HashMap<String, Vec<u8>>>,
}

impl Host for P95Host {
    fn http(&self, request: HttpRequest) -> Result<HttpResponse, PluginError> {
        if request.url.contains("/v2/search/song") {
            return Ok(HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: self.search.as_bytes().to_vec(),
            });
        }
        Err(PluginError::new(
            ErrorCode::NotFound,
            format!("unexpected request {}", request.url),
        ))
    }

    fn secret_get(&self, _key: &str) -> Option<String> {
        None
    }

    fn storage_read(&self, key: &str) -> Option<Vec<u8>> {
        self.storage.lock().expect("storage").get(key).cloned()
    }

    fn storage_write(&self, key: &str, bytes: &[u8]) -> Result<(), PluginError> {
        self.storage
            .lock()
            .expect("storage")
            .insert(key.to_owned(), bytes.to_vec());
        Ok(())
    }

    fn now_ms(&self) -> u64 {
        1_700_000_000_000
    }

    fn random_bytes(&self, len: usize) -> Vec<u8> {
        vec![7; len]
    }
}

fn tracks(count: usize) -> serde_json::Value {
    let lists: Vec<_> = (0..count)
        .map(|index| {
            json!({
                "FileHash": format!("{:032X}", 0x1000 + index),
                "SongName": format!("Track {index}"),
                "SingerName": "Artist",
                "Singers": [{ "name": "Artist", "id": 6452 }],
                "AlbumID": "18905",
                "AlbumName": "Album",
                "MixSongID": (100_000 + index).to_string(),
                "Duration": 200,
                "Image": "http://imge.kugou.com/stdmusic/{size}/x.jpg",
                "Privilege": 8,
                "PayType": 0
            })
        })
        .collect();
    json!({
        "status": 1,
        "error_code": 0,
        "data": { "total": count, "lists": lists }
    })
}

fn p95(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[(samples.len() * 95).div_ceil(100).saturating_sub(1)]
}

fn measure(plugin: &WasmPlugin, op: u32, args: &str, rounds: usize) -> Duration {
    for _ in 0..10 {
        plugin.call(op, args).expect("the warmup op");
    }
    let mut samples = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let started = Instant::now();
        plugin.call(op, args).expect("the measured op");
        samples.push(started.elapsed());
    }
    p95(samples)
}

#[test]
fn kugou_search_stays_inside_the_boundary_budget() {
    let Some(path) = std::env::var_os("WISP_KUGOU_WISP") else {
        return;
    };
    let host = Arc::new(P95Host {
        search: tracks(50).to_string(),
        storage: Mutex::new(HashMap::from([(
            "identity.json".to_owned(),
            br#"{"guid":"550e8400-e29b-41d4-a716-446655440000","mid":"308741372901437977228425563242293240765","webgl":"1","dfid":"dfid-test"}"#.to_vec(),
        )])),
    });
    let plugin = WasmPlugin::with_host(Path::new(&path), Some(host)).expect("the real plugin");
    plugin.describe().expect("describe");

    let search_p95 = measure(
        &plugin,
        ops::music_api::SEARCH,
        r#"{"query":"soak"}"#,
        SEARCH_ROUNDS,
    );
    println!("kugou p95: search {search_p95:?} (limit {SEARCH_P95:?})");
    assert!(search_p95 <= SEARCH_P95, "kugou search p95 {search_p95:?}");
}
