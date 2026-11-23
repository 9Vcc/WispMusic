//! End-to-end smoke test for the real `netease-plugin.wasm`.
//!
//! Build the module first, then run:
//! `WISP_NETEASE_WASM=target/wasm32-wasip1/debug/netease_plugin_wasm.wasm \
//!  cargo test -p wisp-plugin-host --test netease_wasm -- --ignored --nocapture`

use std::sync::Arc;

use wisp_plugin_api::{ErrorCode, Host, HttpRequest, HttpResponse, LogLevel, PluginError, ops};
use wisp_plugin_host::Runtime;

struct CannedHost;

const SEARCH: &str = r#"{
  "result": {
    "songCount": 1,
    "songs": [
      {
        "id": 186016,
        "name": "晴天",
        "ar": [{ "id": 6452, "name": "周杰伦" }],
        "al": { "id": 18905, "name": "叶惠美", "picUrl": "http://p1.music.126.net/a.jpg" },
        "dt": 269000,
        "no": 3,
        "cd": "1",
        "pop": 100,
        "st": 0
      }
    ]
  }
}"#;

impl Host for CannedHost {
    fn http(&self, request: HttpRequest) -> Result<HttpResponse, PluginError> {
        if request.url.contains("/api/cloudsearch/pc") {
            Ok(HttpResponse {
                status: 200,
                headers: Vec::new(),
                body: SEARCH.as_bytes().to_vec(),
            })
        } else {
            Err(PluginError::new(
                ErrorCode::NotFound,
                format!("unexpected host request: {}", request.url),
            ))
        }
    }

    fn secret_get(&self, key: &str) -> Option<String> {
        (key == "cookie").then(|| "MUSIC_U=test; os=pc".to_owned())
    }

    fn log(&self, _level: LogLevel, message: &str) {
        println!("PLUGIN: {message}");
    }

    fn now_ms(&self) -> u64 {
        123
    }

    fn random_bytes(&self, len: usize) -> Vec<u8> {
        vec![7; len]
    }
}

#[test]
#[ignore]
fn the_real_netease_wasm_describes_and_searches() {
    let path = std::env::var("WISP_NETEASE_WASM")
        .expect("set WISP_NETEASE_WASM to the built netease-plugin-wasm artifact");
    let wasm = std::fs::read(path).expect("the wasm artifact reads");

    let runtime = Runtime::new();
    let mut plugin = runtime
        .load_with_host(&wasm, Arc::new(CannedHost))
        .expect("the netease wasm loads");

    let manifest = plugin.describe().expect("the manifest reads");
    assert!(manifest.contains("\"slug\":\"netease\""), "{manifest}");

    let reply = plugin
        .call(ops::music_api::SEARCH, r#"{"query":"jay"}"#)
        .expect("search runs");
    assert!(reply.contains("\"tracks\""), "{reply}");
    assert!(reply.contains("晴天"), "{reply}");
    assert_eq!(plugin.trap_count(), 0);
}
