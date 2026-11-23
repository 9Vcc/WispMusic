//! S5/S6 boundary latency: a real provider package with a canned host.
//!
//! Run manually with `WISP_NETEASE_WISP=... cargo test -p music --test plugin_p95 -- --nocapture`.
//! The test skips when no package is supplied, like the other real-wasm tests.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use music::plugins::runtime::WasmPlugin;
use serde_json::json;
use wisp_plugin_api::ops;
use wisp_plugin_api::{ErrorCode, Host, HttpRequest, HttpResponse, PluginError};

const SEARCH_ROUNDS: usize = 200;
const HOME_ROUNDS: usize = 100;
const SEARCH_P95: Duration = Duration::from_millis(8);
const HOME_P95: Duration = Duration::from_millis(20);

struct P95Host {
    search: String,
    home: String,
}

impl Host for P95Host {
    fn http(&self, request: HttpRequest) -> Result<HttpResponse, PluginError> {
        let body = if request.url.contains("/api/cloudsearch/pc") {
            self.search.as_bytes().to_vec()
        } else if request.url.contains("/api/homepage/block/page") {
            self.home.as_bytes().to_vec()
        } else if request.url.contains("/api/recommend/songs") {
            br#"{"data":{"dailySongs":[]}}"#.to_vec()
        } else if request.url.contains("/api/v1/radio/get") {
            br#"{"data":[]}"#.to_vec()
        } else if request.url.contains("/api/toplist/detail") {
            br#"{"list":[]}"#.to_vec()
        } else if request.url.contains("/api/playlist/list") {
            br#"{"playlists":[]}"#.to_vec()
        } else {
            return Err(PluginError::new(
                ErrorCode::NotFound,
                format!("unexpected request {}", request.url),
            ));
        };
        Ok(HttpResponse {
            status: 200,
            headers: Vec::new(),
            body,
        })
    }

    fn secret_get(&self, key: &str) -> Option<String> {
        (key == "cookie").then(|| "MUSIC_U=test; os=pc".to_owned())
    }

    fn now_ms(&self) -> u64 {
        1_700_000_000_000
    }

    fn random_bytes(&self, len: usize) -> Vec<u8> {
        vec![7; len]
    }
}

fn tracks(count: usize) -> serde_json::Value {
    let songs: Vec<_> = (0..count)
        .map(|index| {
            json!({
                "id": 100_000 + index,
                "name": format!("Track {index}"),
                "ar": [{ "id": 6452, "name": "Artist" }],
                "al": { "id": 18905, "name": "Album", "picUrl": "http://p1.music.126.net/a.jpg" },
                "dt": 200_000 + index as u64,
                "no": index + 1,
                "cd": "1",
                "pop": 100,
                "st": 0
            })
        })
        .collect();
    json!({ "result": { "songs": songs } })
}

fn home_feed(count: usize) -> serde_json::Value {
    let resources: Vec<_> = (0..count)
        .map(|index| {
            json!({
                "resourceType": "song",
                "resourceId": 100_000 + index,
                "uiElement": {
                    "mainTitle": { "title": format!("Track {index}") },
                    "image": { "imageUrl": "http://p1.music.126.net/a.jpg" }
                },
                "resourceExtInfo": {
                    "artists": [{ "id": 6452, "name": "Artist" }],
                    "songData": { "duration": 200_000 + index as u64 }
                }
            })
        })
        .collect();
    json!({
        "data": {
            "blocks": [{
                "uiElement": { "mainTitle": { "title": "Recommended" } },
                "creatives": [{
                    "uiElement": { "mainTitle": { "title": "For you" } },
                    "resources": resources
                }]
            }]
        }
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
fn search_and_home_stay_inside_the_boundary_budget() {
    let Some(path) = std::env::var_os("WISP_NETEASE_WISP") else {
        return;
    };
    let host = Arc::new(P95Host {
        search: tracks(50).to_string(),
        home: home_feed(50).to_string(),
    });
    let plugin = WasmPlugin::with_host(Path::new(&path), Some(host)).expect("the real plugin");
    plugin.describe().expect("describe");

    let search_p95 = measure(
        &plugin,
        ops::music_api::SEARCH,
        r#"{"query":"soak"}"#,
        SEARCH_ROUNDS,
    );
    let home_p95 = measure(&plugin, ops::music_api::HOME, "{}", HOME_ROUNDS);
    println!(
        "p95: search {search_p95:?} (limit {SEARCH_P95:?}) home {home_p95:?} (limit {HOME_P95:?})"
    );
    assert!(search_p95 <= SEARCH_P95, "search p95 {search_p95:?}");
    assert!(home_p95 <= HOME_P95, "home p95 {home_p95:?}");
}
