//! unm-server 引擎的纯 Rust 移植（`@unblockneteasemusic/server@0.28.0`）。
//!
//! 只移植 provider 匹配层，不移植它给别的音乐软件用的 HTTP 代理 / 证书逻辑。
//! 覆盖：kugou、bodian、migu、kuwo（antiserver 明文链路）、bilibili、qq / joox
//! （后两者需要用户 Cookie，设置页接入前自动跳过）。
//! 不移植：yt-dlp / youtube / pyncmd（依赖外部二进制，与网易云灰歌无关）、
//! kwDES（585 行自定义 DES，只用在高音质酷我链路；这里退到 mp3 的 antiserver）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Duration;

use crate::Quality;

const AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const KUWO_UA: &str = "okhttp/3.10.0";
const DART_UA: &str = "Dart/2.19 (dart:io)";
const TIMEOUT: Duration = Duration::from_secs(5);

/// 解灰一次尝试的总预算边界（毫秒）。
const BUDGET_MIN_MS: u32 = 5_000;
const BUDGET_MAX_MS: u32 = 30_000;

/// 解灰配置：由 state 的设置页推送进来（`AppSettings::unblock_config()`），
/// 默认值与 Wisp `settingStore.network` 一致：开启解灰、自动音源（全部引擎）、
/// 优先无损、顺序优先、15 秒预算、两个 Cookie 留空。
#[derive(Clone, Debug)]
pub struct UnblockConfig {
    pub enabled: bool,
    /// 对应 `unblockEnableFlac`：引擎永远先试无损，与当前播放档位无关。
    pub enable_flac: bool,
    /// 对应 `unblockOrderFirst`：true 按固定顺序逐个试引擎；false 全部引擎并发抢最快。
    pub order_first: bool,
    /// 对应 `unblockBudget`：整个解灰搜索的总预算，到点即放弃。
    pub budget_ms: u32,
    /// 对应 `unblockSource`：空 = 自动/全部引擎；否则逗号分隔的引擎名。
    pub source: String,
    pub qq_cookie: Option<Arc<String>>,
    pub joox_cookie: Option<Arc<String>>,
}

impl UnblockConfig {
    pub fn new(
        enabled: bool,
        enable_flac: bool,
        order_first: bool,
        budget_ms: u32,
        source: impl Into<String>,
        qq_cookie: Option<String>,
        joox_cookie: Option<String>,
    ) -> Self {
        Self {
            enabled,
            enable_flac,
            order_first,
            budget_ms: budget_ms.clamp(BUDGET_MIN_MS, BUDGET_MAX_MS),
            source: source.into(),
            qq_cookie: qq_cookie.map(Arc::new),
            joox_cookie: joox_cookie.map(Arc::new),
        }
    }
}

impl Default for UnblockConfig {
    fn default() -> Self {
        Self::new(true, true, false, 15_000, "", None, None)
    }
}

/// The last tracks the player opened, by NetEase id, so an unblock does not have to wait for
/// the provider's track-detail op after a provider switch.
static RECENT: OnceLock<Mutex<HashMap<i64, TrackInfo>>> = OnceLock::new();

fn recent() -> &'static Mutex<HashMap<i64, TrackInfo>> {
    RECENT.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Remembers the metadata the player already has, keyed by the id the unblock engines search
/// by. Providers whose ids are not numeric are ignored.
pub(crate) fn remember(track: &crate::Track) {
    let Some(info) = TrackInfo::from_track(track) else {
        return;
    };
    let Ok(mut recent) = recent().lock() else {
        return;
    };
    if recent.len() >= 64 && !recent.contains_key(&info.id) {
        recent.clear();
    }
    if let Some(existing) = recent.get_mut(&info.id) {
        if info.duration_ms > 0 {
            existing.duration_ms = info.duration_ms;
        }
        if !info.artists.is_empty() || existing.artists.is_empty() {
            existing.name = info.name;
            existing.artists = info.artists;
            existing.keyword = info.keyword;
        }
    } else {
        recent.insert(info.id, info);
    }
}

pub(crate) fn remembered(id: &str) -> Option<TrackInfo> {
    let id = id.parse::<i64>().ok()?;
    recent().lock().ok()?.get(&id).cloned()
}

/// 设置页推送进来的当前配置；music crate 不依赖 state，走这个共享槽。
fn shared() -> &'static RwLock<UnblockConfig> {
    static SHARED: OnceLock<RwLock<UnblockConfig>> = OnceLock::new();
    SHARED.get_or_init(|| RwLock::new(UnblockConfig::default()))
}

/// 设置层把用户配置推送进来；启动时与每次设置变化时调用。
pub fn set_unblock_config(config: UnblockConfig) {
    if let Ok(mut guard) = shared().write() {
        *guard = config;
    }
}

/// 当前解灰配置的快照；锁被毒化时回落默认（开启解灰、无 Cookie）。
fn current() -> UnblockConfig {
    shared().read().map(|guard| guard.clone()).unwrap_or_default()
}

impl UnblockConfig {
    fn offers(&self, name: &str) -> bool {
        self.source.is_empty()
            || self
                .source
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case(name))
    }
}

/// 供搜索匹配用的曲目信息（来自网易云的歌曲详情）。
#[derive(Clone, Debug)]
pub(crate) struct TrackInfo {
    pub id: i64,
    pub name: String,
    pub artists: Vec<String>,
    pub duration_ms: i64,
    /// 上游 `search/song` 的 keyword：`歌名 - 歌手1 / 歌手2`
    pub keyword: String,
}

impl TrackInfo {
    pub(crate) fn from_track(track: &crate::Track) -> Option<Self> {
        let id = track.id.as_ref()?.parse::<i64>().ok()?;
        let name = strip_annotations(&track.name);
        let artists: Vec<String> = track.artist_refs.iter().map(|artist| artist.name.clone()).collect();
        let mut shown = String::new();
        for artist in &artists {
            if shown.len() > 15 {
                break;
            }
            if !shown.is_empty() {
                shown.push_str(" / ");
            }
            shown.push_str(artist);
        }
        Some(Self {
            id,
            keyword: if shown.is_empty() {
                name.clone()
            } else {
                format!("{name} - {shown}")
            },
            name,
            artists,
            duration_ms: track.duration.as_millis() as i64,
        })
    }
}

/// 去掉「(cover: …)」「（翻自：…）」一类尾巴（上游 find.js 的规则）。
fn strip_annotations(name: &str) -> String {
    let mut out = name.to_owned();
    for open in ['（', '('] {
        let close = if open == '（' { '）' } else { ')' };
        while let Some(start) = out.find(open) {
            let Some(relative) = out[start..].find(close) else {
                break;
            };
            let end = start + relative + close.len_utf8();
            let inner = &out[start + open.len_utf8()..end - close.len_utf8()];
            let lower = inner.to_ascii_lowercase();
            if lower.trim_start().starts_with("cover") || inner.trim_start().starts_with("翻自") {
                out.replace_range(start..end, "");
            } else {
                // 这一段不是标注，接着从后面找
                let rest = out[end..].to_owned();
                return format!("{}{}", &out[..end], strip_annotations(&rest));
            }
        }
    }
    out
}

/// 前 5 个里时长差 5 秒内的第一个；没有就第一条（上游 select.js）。
fn select<T>(list: Vec<T>, info: &TrackInfo, duration: impl Fn(&T) -> i64) -> Option<T> {
    // 用索引取，避免要求 T: Clone
    if let Some(index) = list.iter().take(5).position(|item| {
        let duration = duration(item);
        duration > 0 && (duration - info.duration_ms).abs() < 5_000
    }) {
        return list.into_iter().nth(index);
    }
    list.into_iter().next()
}

/// One HTTP client per user agent, kept for the process. The unblock engines are the only
/// callers, and a fresh client per engine per search would pay a connector and TLS setup for
/// every gray song; the first unblock of a session felt that the most.
pub(super) fn client(ua: &'static str) -> Option<reqwest::Client> {
    static CLIENTS: OnceLock<Mutex<HashMap<&'static str, reqwest::Client>>> = OnceLock::new();
    let mut clients = CLIENTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()?;
    if let Some(client) = clients.get(ua) {
        return Some(client.clone());
    }
    let built = reqwest::Client::builder()
        .dns_resolver(crate::dns::dns())
        .user_agent(ua)
        .timeout(TIMEOUT)
        .build()
        .ok()?;
    clients.insert(ua, built.clone());
    Some(built)
}

fn md5_hex(text: &str) -> String {
    use md5::{Digest as _, Md5};
    let mut hasher = Md5::new();
    hasher.update(text.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// URL 编码（encodeURIComponent 口径）。
pub(crate) fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(*byte, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')');
        if keep {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// 从响应体里找第一个 `http...` 直链（酷我 antiserver 返回一段文本）。
fn first_url(body: &str) -> Option<String> {
    let start = body.find("http")?;
    let rest = &body[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '"' || c == '$')
        .unwrap_or(rest.len());
    Some(rest[..end].to_owned())
}

/// 依次试各引擎，返回第一个直链。配置来自设置页推送的共享槽。
pub(crate) async fn match_url(info: &TrackInfo, quality: Quality) -> Option<(String, &'static str)> {
    match_url_with(info, quality, &current()).await
}

pub(crate) async fn match_url_with(
    info: &TrackInfo,
    quality: Quality,
    config: &UnblockConfig,
) -> Option<(String, &'static str)> {
    if !config.enabled {
        return None;
    }
    let budget = Duration::from_millis(u64::from(
        config.budget_ms.clamp(BUDGET_MIN_MS, BUDGET_MAX_MS),
    ));
    match tokio::time::timeout(budget, search(info, quality, config)).await {
        Ok(found) => found,
        Err(_) => {
            log::info!(
                "unm: gave up after {} ms on {}",
                budget.as_millis(),
                info.name
            );
            None
        }
    }
}

/// 顺序对齐上游 DEFAULT_SOURCE = kugou, bodian, migu（ytdlp 已跳过）。
fn engines(config: &UnblockConfig) -> [(&'static str, bool); 7] {
    [
        ("kugou", config.offers("kugou")),
        ("bodian", config.offers("bodian")),
        ("migu", config.offers("migu")),
        ("kuwo", config.offers("kuwo")),
        ("bilibili", config.offers("bilibili")),
        ("qq", config.offers("qq")),
        ("joox", config.offers("joox")),
    ]
}

async fn attempt(
    name: &str,
    info: &TrackInfo,
    quality: Quality,
    config: &UnblockConfig,
) -> Option<String> {
    match name {
        "kugou" => kugou(info, config.enable_flac).await,
        "bodian" => bodian(info, config.enable_flac).await,
        "migu" => migu(info, config.enable_flac).await,
        "kuwo" => kuwo(info).await,
        "bilibili" => bilibili(info).await,
        "qq" => qq(info, quality, config.enable_flac, config.qq_cookie.clone()).await,
        "joox" => joox(info, config.joox_cookie.clone()).await,
        _ => None,
    }
}

async fn usable(name: &str, url: &str, info: &TrackInfo) -> bool {
    if url.starts_with("http") && super::unblock::valid_audio(url, info.duration_ms).await {
        return true;
    }
    log::warn!("unm: {name} returned an unusable stream for {}", info.name);
    false
}

async fn search(
    info: &TrackInfo,
    quality: Quality,
    config: &UnblockConfig,
) -> Option<(String, &'static str)> {
    let engines = engines(config);
    if config.order_first {
        for (name, offered) in engines {
            if !offered {
                continue;
            }
            if let Some(url) = attempt(name, info, quality, config).await
                && usable(name, &url, info).await
            {
                return Some((url, name));
            }
            log::debug!("unm: {name} missed {}", info.name);
        }
        return None;
    }

    let mut running = futures::stream::FuturesUnordered::new();
    for (name, offered) in engines {
        if !offered {
            continue;
        }
        running.push(async move { (name, attempt(name, info, quality, config).await) });
    }
    while let Some((name, matched)) = futures::StreamExt::next(&mut running).await {
        match matched {
            Some(url) => {
                if usable(name, &url, info).await {
                    return Some((url, name));
                }
            }
            None => log::debug!("unm: {name} missed {}", info.name),
        }
    }
    None
}

// ---------------- kugou ----------------

async fn kugou(info: &TrackInfo, enable_flac: bool) -> Option<String> {
    let http = client(AGENT)?;
    let url = format!(
        "http://mobilecdn.kugou.com/api/v3/search/song?keyword={}&page=1&pagesize=10",
        encode(&info.keyword)
    );
    let body: serde_json::Value = http.get(url).send().await.ok()?.json().await.ok()?;
    let list = body
        .pointer("/data/info")
        .and_then(serde_json::Value::as_array)?
        .clone();
    let picked = select(list.clone(), info, |song| {
        song.get("duration").and_then(serde_json::Value::as_i64).unwrap_or(0) * 1000
    })?;
    let album_id = picked
        .get("album_id")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);

    // 优先无损/高码率，逐个尝试（ENABLE_FLAC 由音质档位决定）
    let formats: &[&str] = if enable_flac {
        &["sqhash", "hqhash", "hash"]
    } else {
        &["hqhash", "hash"]
    };
    for format in formats {
        let Some(hash) = picked.get(format).and_then(serde_json::Value::as_str) else {
            continue;
        };
        let key = md5_hex(&format!("{hash}kgcloudv2"));
        let url = format!(
            "http://trackercdn.kugou.com/i/v2/?key={key}&hash={hash}&appid=1005&pid=2&cmd=25&behavior=play&album_id={album_id}"
        );
        let answer: serde_json::Value = http.get(url).send().await.ok()?.json().await.ok()?;
        if let Some(found) = answer.pointer("/url/0").and_then(serde_json::Value::as_str) {
            return Some(found.to_owned());
        }
        log::debug!(
            "unm: kugou trackercdn refused {} (status {:?})",
            info.name,
            answer.get("status")
        );
    }
    None
}

// ---------------- bodian（酷我波点）----------------

async fn bodian(info: &TrackInfo, enable_flac: bool) -> Option<String> {
    let http = client(DART_UA)?;
    let id = kuwo_search_async(info).await?;
    log::debug!("unm: bodian searched {} -> kuwo id {}", info.name, id);
    // 上游的 deviceId 在模块加载时生成一次，免广告解锁与取链必须用同一个
    let devid = fastrand::u64(..100_000_000_000).to_string();
    // 上游先打一次免广告接口（失败也无所谓），这里保留请求以维持行为一致
    let ad = "http://bd-api.kuwo.cn/api/service/advert/watch?uid=-1&token=&timestamp=1724306124436&sign=15a676d66285117ad714e8c8371691da";
    let body = serde_json::json!({"type": 5, "subType": 5, "musicId": 0, "adToken": ""});
    // 上游明确带 charset=utf-8；reqwest 的 .json() 会把 Content-Type 覆盖掉，
    // 服务端据此判 sign invalid，所以这里手工序列化 body。
    let answer = http
        .post(ad)
        .header("plat", "ar")
        .header("channel", "aliopen")
        .header("devid", devid.clone())
        .header("ver", "3.9.0")
        .header("host", "bd-api.kuwo.cn")
        .header("qimei36", "1e9970cbcdc20a031dee9f37100017e1840e")
        .header("content-type", "application/json; charset=utf-8")
        .body(serde_json::to_string(&body).unwrap_or_default())
        .send()
        .await;
    let answer = match answer {
        Ok(response) => response.text().await.unwrap_or_default(),
        Err(_) => String::new(),
    };
    log::debug!("unm: bodian ad-free response for {} = {}", info.name, answer);

    let br = if enable_flac { "2000kflac" } else { "320kmp3" };
    let path = "/api/play/music/v2/audioUrl";
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let query = format!("&br={br}&musicId={id}&timestamp={now}");
    // sign = md5("kuwotest" + query 中的字母数字排序 + path)
    let mut chars: Vec<char> = query.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    chars.sort_unstable();
    let plain = format!("kuwotest{}{path}", chars.into_iter().collect::<String>());
    let signed = format!(
        "http://bd-api.kuwo.cn{path}?{query}&sign={sign}",
        sign = md5_hex(&plain)
    );
    let answer: serde_json::Value = http
        .get(signed)
        .header("plat", "ar")
        .header("channel", "aliopen")
        .header("devid", devid.clone())
        .header("ver", "3.9.0")
        .header("host", "bd-api.kuwo.cn")
        .header("qimei36", "1e9970cbcdc20a031dee9f37100017e1840e")
        .header("X-Forwarded-For", "1.0.1.114")
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    if answer.get("code").and_then(serde_json::Value::as_i64) != Some(200) {
        log::debug!(
            "unm: bodian audioUrl for {} refused: {}",
            info.name,
            answer.to_string().chars().take(180).collect::<String>()
        );
        return None;
    }
    answer
        .pointer("/data/audioUrl")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

// ---------------- migu ----------------

async fn migu(info: &TrackInfo, enable_flac: bool) -> Option<String> {
    let http = client(AGENT)?;
    let url = format!(
        "https://m.music.migu.cn/migu/remoting/scr_search_tag?keyword={}&type=2&rows=20&pgc=1",
        encode(&info.keyword)
    );
    let body: serde_json::Value = http
        .get(url)
        .header("origin", "http://music.migu.cn/")
        .header("referer", "http://m.music.migu.cn/v3/")
        .header("channel", "0146921")
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    let list = body.get("musics").and_then(serde_json::Value::as_array)?.clone();
    let picked = select(list, info, |song| {
        song.get("duration")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
    })?;
    let id = picked.get("id").and_then(serde_json::Value::as_str)?;

    let formats: &[&str] = if enable_flac {
        &["ZQ24", "SQ", "HQ", "PQ"]
    } else {
        &["HQ", "PQ"]
    };
    for format in formats {
        let url = format!(
            "https://app.c.nf.migu.cn/MIGUM2.0/strategy/listen-url/v2.4?netType=01&resourceType=2&songId={id}&toneFlag={format}"
        );
        let answer: serde_json::Value = http
            .get(url)
            .header("origin", "http://music.migu.cn/")
            .header("referer", "http://m.music.migu.cn/v3/")
            .header("channel", "0146921")
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        let data = answer.get("data")?;
        if data.get("audioFormatType").and_then(serde_json::Value::as_str) == Some(*format) {
            if let Some(found) = data.get("url").and_then(serde_json::Value::as_str) {
                return Some(found.to_owned());
            }
        }
    }
    None
}

// ---------------- kuwo（antiserver 明文链路，仅 mp3）----------------

/// 酷我 / 波点共用的搜索：返回第一个匹配的 MUSICRID 数字 id。
async fn kuwo_search_async(info: &TrackInfo) -> Option<String> {
    let http = client(AGENT)?;
    let url = format!(
        "http://search.kuwo.cn/r.s?&correct=1&vipver=1&stype=comprehensive&encoding=utf8&rformat=json&mobi=1&show_copyright_off=1&searchapi=6&all={}",
        encode(&info.keyword.replace(" - ", " "))
    );
    let body: serde_json::Value = http.get(url).send().await.ok()?.json().await.ok()?;
    if body.pointer("/content/1/musicpage/abslist").is_none() {
        log::debug!("unm: kuwo search returned no abslist for {}", info.name);
    }
    let list = body
        .pointer("/content/1/musicpage/abslist")
        .and_then(serde_json::Value::as_array)?
        .clone();
    let picked = select(list, info, |song| {
        let raw = song.get("DURATION");
        let seconds = raw
            .and_then(serde_json::Value::as_i64)
            .or_else(|| raw.and_then(serde_json::Value::as_str).and_then(|s| s.parse().ok()))
            .unwrap_or(0);
        seconds * 1000
    })?;
    picked
        .get("MUSICRID")
        .and_then(serde_json::Value::as_str)
        .and_then(|rid| rid.rsplit('_').next())
        .map(str::to_owned)
}

async fn kuwo(info: &TrackInfo) -> Option<String> {
    let id = kuwo_search_async(info).await?;
    let http = client(KUWO_UA)?;
    let url = format!(
        "http://antiserver.kuwo.cn/anti.s?type=convert_url&format=mp3&response=url&rid=MUSIC_{id}"
    );
    let body = http.get(url).send().await.ok()?.text().await.ok()?;
    let found = first_url(&body);
    if found.is_none() {
        log::debug!("unm: kuwo antiserver gave no url for {}", info.name);
    }
    found
}

// ---------------- bilibili ----------------

async fn bilibili(info: &TrackInfo) -> Option<String> {
    let http = client(AGENT)?;
    let url = format!(
        "https://api.bilibili.com/audio/music-service-c/s?search_type=music&page=1&pagesize=30&keyword={}",
        encode(&info.keyword)
    );
    let body: serde_json::Value = http.get(url).send().await.ok()?.json().await.ok()?;
    let list = body
        .pointer("/data/result")
        .and_then(serde_json::Value::as_array)?
        .clone();
    let picked = select(list, info, |_| 0)?;
    let id = picked.get("id")?.to_string();
    let url = format!(
        "https://www.bilibili.com/audio/music-service-c/web/url?rivilege=2&quality=2&sid={id}"
    );
    let answer: serde_json::Value = http.get(url).send().await.ok()?.json().await.ok()?;
    if answer.get("code").and_then(serde_json::Value::as_i64) != Some(0) {
        return None;
    }
    answer
        .pointer("/data/cdns/0")
        .and_then(serde_json::Value::as_str)
        .map(|url| url.replace("https", "http"))
}

// ---------------- qq（需要 `uin; qqmusic_key` Cookie）----------------

async fn qq(_info: &TrackInfo, _quality: Quality, enable_flac: bool, cookie: Option<Arc<String>>) -> Option<String> {
    let cookie = cookie?;
    let http = client(AGENT)?;
    let headers = |builder: reqwest::RequestBuilder| {
        builder
            .header("Cookie", cookie.as_str())
            .header("Origin", "http://y.qq.com/")
            .header("Referer", "http://y.qq.com/")
    };

    let search = serde_json::json!({
        "search": {
            "method": "DoSearchForQQMusicDesktop",
            "module": "music.search.SearchCgiService",
            "param": {
                "num_per_page": 5,
                "page_num": 1,
                "query": _info.keyword,
                "search_type": 0,
            },
        }
    });
    let url = format!(
        "https://u.y.qq.com/cgi-bin/musicu.fcg?data={}",
        encode(&search.to_string())
    );
    let answer: serde_json::Value = headers(http.get(url)).send().await.ok()?.json().await.ok()?;
    let list = answer
        .pointer("/search/data/body/song/list")
        .and_then(serde_json::Value::as_array)?
        .clone();
    let picked = select(list, _info, |song| {
        song.get("interval").and_then(serde_json::Value::as_f64).unwrap_or(0.) as i64 * 1000
    })?;
    let mid = picked.get("mid").and_then(serde_json::Value::as_str)?.to_owned();

    let uin = cookie
        .split(';')
        .find_map(|pair| pair.trim().strip_prefix("uin="))
        .unwrap_or("0")
        .to_owned();
    let _ = _quality;
    let formats: &[(&str, &str)] = if enable_flac {
        &[("F000", ".flac"), ("M800", ".mp3"), ("M500", ".mp3")]
    } else {
        &[("M800", ".mp3"), ("M500", ".mp3")]
    };
    for (prefix, suffix) in formats {
        let query = serde_json::json!({
            "req_0": {
                "module": "vkey.GetVkeyServer",
                "method": "CgiGetVkey",
                "param": {
                    "guid": fastrand::u64(..10_000_000).to_string(),
                    "loginflag": 1,
                    "filename": [format!("{prefix}{suffix}{mid}")],
                    "songmid": [mid.clone()],
                    "songtype": [0],
                    "uin": uin.clone(),
                    "platform": "20",
                },
            }
        });
        let url = format!(
            "https://u.y.qq.com/cgi-bin/musicu.fcg?data={}",
            encode(&query.to_string())
        );
        let answer: serde_json::Value = headers(http.get(url)).send().await.ok()?.json().await.ok()?;
        let data = answer.pointer("/req_0/data")?;
        let purl = data.pointer("/midurlinfo/0/purl").and_then(serde_json::Value::as_str)?;
        let sip = data.pointer("/sip/0").and_then(serde_json::Value::as_str)?;
        let playurl = format!("{sip}{purl}");
        // 上游会用 range 请求验证直链可用
        let ok = http
            .get(&playurl)
            .header("range", "bytes=0-8191")
            .header("accept-encoding", "identity")
            .send()
            .await
            .ok()?
            .status()
            .is_success();
        if ok {
            return Some(playurl);
        }
    }
    None
}

// ---------------- joox（需要 `wmid; session_key` Cookie）----------------

/// JOOX 的搜索响应用单引号包 JSON，上游直接替换后解析。
fn joox_b64(text: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(text.as_bytes())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| text.to_owned())
}

/// 把 `M\d00xxx.mp3` 里的码率前缀统一成 `M800`（上游规则）。
fn joox_upgrade(url: &str) -> String {
    let bytes = url.as_bytes();
    let mut index = 0;
    while index + 4 < bytes.len() {
        if bytes[index] == b'M'
            && bytes[index + 1].is_ascii_digit()
            && bytes[index + 2] == b'0'
            && bytes[index + 3] == b'0'
        {
            let rest = &url[index + 4..];
            if let Some(end) = rest.find(".mp3") {
                return format!("{}M800{}{}", &url[..index], &rest[..end], &rest[end..]);
            }
        }
        index += 1;
    }
    url.to_owned()
}

async fn joox(info: &TrackInfo, cookie: Option<Arc<String>>) -> Option<String> {
    let cookie = cookie?;
    let http = client(AGENT)?;
    let japanese = info.name.chars().any(|c| ('\u{0800}'..='\u{4e00}').contains(&c));
    let keyword = if japanese { info.name.as_str() } else { info.keyword.as_str() };
    let url = format!(
        "http://api-jooxtt.sanook.com/web-fcgi-bin/web_search?country=hk&lang=zh_TW&search_input={}&sin=0&ein=30",
        encode(keyword)
    );
    let body = http
        .get(url)
        .header("Origin", "http://www.joox.com")
        .header("Referer", "http://www.joox.com")
        .header("Cookie", cookie.as_str())
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    let json: serde_json::Value = serde_json::from_str(&body.replace('\'', "\""))
        .or_else(|_| serde_json::from_str(&body))
        .ok()?;
    let list = json
        .get("itemlist")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .map(|song| {
            let id = song.get("songid")?.to_string();
            let name = song
                .get("info1")
                .and_then(serde_json::Value::as_str)
                .map(joox_b64)
                .unwrap_or_default();
            let duration = song
                .get("playtime")
                .and_then(serde_json::Value::as_f64)
                .unwrap_or(0.) as i64
                * 1000;
            Some((id, name, duration))
        })
        .collect::<Vec<Option<(String, String, i64)>>>();
    let flat: Vec<(String, String, i64)> = list.into_iter().flatten().collect();
    let picked = select(flat.clone(), info, |entry| entry.2)?;
    let id = picked.0.trim_matches('"');

    let url = format!(
        "http://api.joox.com/web-fcgi-bin/web_get_songinfo?songid={id}&country=hk&lang=zh_cn&from_type=-1&channel_id=-1&_={}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis()
    );
    let body = http
        .get(url)
        .header("Cookie", cookie.as_str())
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    // JSONP：截取第一个 '{' 到最后一个 '}'
    let begin = body.find('{')?;
    let end = body.rfind('}')?;
    let answer: serde_json::Value = serde_json::from_str(&body[begin..=end]).ok()?;
    let url = ["r320Url", "r192Url", "mp3Url", "m4aUrl"]
        .into_iter()
        .find_map(|key| answer.get(key).and_then(serde_json::Value::as_str))
        .filter(|url| !url.is_empty())?;
    Some(joox_upgrade(url))
}
