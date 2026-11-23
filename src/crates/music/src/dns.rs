//! 跨执行器的 DNS：把 reqwest 的域名解析绑到一个 tokio runtime 上。
//!
//! reqwest 默认的解析器（hyper-util 的 `GaiResolver`）在发起解析时要求
//! 「当前线程处于 tokio runtime」，而 Wisp 的请求 future 会在多个执行器上
//! 被 poll：tokio（`Io` 的 runtime）、gpui 的前台执行器（主线程）与后台
//! 执行器。只要有一个请求落在了 tokio 之外，DNS 一解析就是
//! 「there is no reactor running」的 panic——启动时主线程渲染期间的
//! 网易云搜索联想就是这么崩的。
//!
//! 解法：自定义 resolver 把解析丢进启动时绑定的 tokio runtime，poll 发生
//! 在哪个线程都与它无关。运行时若没有绑定（早期请求），解析会失败成一条
//! 请求错误并记日志，而不是崩掉整个进程。

use std::net::ToSocketAddrs as _;
use std::sync::{Arc, OnceLock};

use anyhow::{anyhow, Result};

static HANDLE: OnceLock<tokio::runtime::Handle> = OnceLock::new();

/// 绑定 DNS 解析用的 tokio runtime。在应用启动、`Io` 建好之后调用一次。
pub fn bind_dns_runtime(handle: tokio::runtime::Handle) {
    let _ = HANDLE.set(handle);
}

fn handle() -> Result<tokio::runtime::Handle> {
    HANDLE.get().cloned().ok_or_else(|| {
        anyhow!("no tokio runtime bound for DNS; call bind_dns_runtime at startup")
    })
}

/// The runtime handle bound at startup, for hosts that need to block on HTTP from a plugin
/// thread. `None` before `bind_dns_runtime`.
pub(crate) fn try_handle() -> Option<tokio::runtime::Handle> {
    HANDLE.get().cloned()
}

/// 每个解析一个独立 resolver 实例（reqwest 要求 `Clone`/`Send`/`Sync`）。
#[derive(Clone, Default)]
pub struct Resolver;

impl reqwest::dns::Resolve for Resolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let runtime = handle();
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let runtime = runtime.map_err(|error| anyhow!(error))?;
            let addrs = runtime
                .spawn_blocking(move || (host.as_str(), 0).to_socket_addrs().map(|addrs| {
                    use std::iter::Iterator as _;
                    let mut addrs: Vec<_> = addrs.collect();
                    addrs.sort();
                    addrs
                }))
                .await
                .map_err(|error| anyhow!(error))?
                .map_err(|error| anyhow!(error))?;
            let addrs: Box<dyn Iterator<Item = std::net::SocketAddr> + Send> =
                Box::new(addrs.into_iter());
            Ok(addrs)
        })
    }
}

/// 给 `reqwest::Client::builder()` 用的解析器。
pub fn dns() -> Arc<dyn reqwest::dns::Resolve> {
    Arc::new(Resolver)
}
