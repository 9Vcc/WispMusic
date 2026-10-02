use base64::Engine;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, SET_COOKIE};
use reqwest::Client;
use serde::Serialize;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::OnceLock;

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

/// 总超时。这条通道的调用方是 JS 侧 `await invoke("ncm_fetch")`，而 Tauri IPC
/// 不可取消（渲染进程的 AbortController 根本接不进来），所以超时**只能**设在这里：
/// 没有它，一个半开连接（对方 accept 之后不再回数据——第三方解灰 API 的常态）
/// 就让那个 await 永远不返回，`playSong` 卡在音源解析上，加载转圈永不落幕。
/// 内置 API 的 JSON 请求正常都在 1~3s，20s 足够容下慢成功又能保证「最坏有结论」。
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
/// 建连超时：对端 IP 不通/被墙时快速失败，不必等满总超时
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NcmFetchResponse {
    status: u16,
    headers: HashMap<String, String>,
    set_cookies: Vec<String>,
    body: String,
    body_is_base64: bool,
}

fn follow_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .user_agent(DEFAULT_USER_AGENT)
            .redirect(reqwest::redirect::Policy::limited(10))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("failed to build reqwest follow client")
    })
}

fn no_follow_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .user_agent(DEFAULT_USER_AGENT)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("failed to build reqwest no-follow client")
    })
}

// 渲染进程专用 HTTP 通道：绕过 WebView 的 CORS 与 Set-Cookie 过滤，
// 供内置网易云 API（weapi/eapi/linuxapi）与解灰音源共用
#[tauri::command]
pub async fn ncm_fetch(
    method: String,
    url: String,
    headers: HashMap<String, String>,
    body: Option<String>,
    body_is_base64: Option<bool>,
    follow_redirects: Option<bool>,
) -> Result<NcmFetchResponse, String> {
    let client = if follow_redirects.unwrap_or(true) {
        follow_client()
    } else {
        no_follow_client()
    };

    let method = reqwest::Method::from_str(&method.to_uppercase())
        .map_err(|e| format!("invalid method '{}': {}", method, e))?;
    let mut request = client.request(method, &url);

    let mut header_map = HeaderMap::new();
    for (key, value) in headers {
        if key.is_empty() {
            continue;
        }
        let name = HeaderName::from_bytes(key.to_lowercase().as_bytes())
            .map_err(|e| format!("invalid header name '{}': {}", key, e))?;
        let val = HeaderValue::from_str(&value)
            .map_err(|e| format!("invalid header value for '{}': {}", key, e))?;
        header_map.insert(name, val);
    }
    if !header_map.is_empty() {
        request = request.headers(header_map);
    }

    if let Some(body) = body {
        if body_is_base64.unwrap_or(false) {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(body.as_bytes())
                .map_err(|e| format!("invalid base64 body: {}", e))?;
            request = request.body(bytes);
        } else {
            request = request.body(body);
        }
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("request failed: {}", e))?;

    let status = response.status().as_u16();

    let mut resp_headers: HashMap<String, String> = HashMap::new();
    let mut set_cookies: Vec<String> = Vec::new();
    for (name, value) in response.headers() {
        let value_str = value.to_str().unwrap_or("").to_string();
        if name == SET_COOKIE {
            set_cookies.push(value_str);
        } else {
            resp_headers
                .entry(name.as_str().to_string())
                .and_modify(|existing| {
                    existing.push_str(", ");
                    existing.push_str(&value_str);
                })
                .or_insert(value_str);
        }
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("failed to read response body: {}", e))?;

    let (body, body_is_base64) = match String::from_utf8(bytes.to_vec()) {
        Ok(text) => (text, false),
        Err(e) => {
            let raw = e.into_bytes();
            (base64::engine::general_purpose::STANDARD.encode(&raw), true)
        }
    };

    Ok(NcmFetchResponse {
        status,
        headers: resp_headers,
        set_cookies,
        body,
        body_is_base64,
    })
}
