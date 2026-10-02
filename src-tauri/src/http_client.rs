use reqwest::Client;
use std::sync::OnceLock;
use std::time::Duration;

/// 全应用共用的 reqwest 客户端。
///
/// reqwest 的连接池挂在 `Client` 实例上：每次新建 `Client` 等于丢掉整池连接，
/// 下一个请求必须重做 DNS 解析 + TCP 三次握手 + TLS 协商。音频 CDN 地址是
/// 逐首解析出来的，但域名高度集中，共用客户端后同一域名下的后续音源可以
/// 直接复用已建立的连接——这段握手时间原本是逐首叠加在起播延迟上的。
///
/// 只约束连接建立阶段（`connect_timeout`），**不设整体 `.timeout()`**：正常
/// 的长音频流会被整体超时掐断。分块之间的空转由各读取循环自己的
/// `tokio::time::timeout` 兜底（见 audio/source.rs 的 CHUNK_IDLE_TIMEOUT）。
static SHARED_CLIENT: OnceLock<Client> = OnceLock::new();

pub(crate) fn shared_client() -> &'static Client {
    SHARED_CLIENT.get_or_init(|| {
        Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default()
    })
}
