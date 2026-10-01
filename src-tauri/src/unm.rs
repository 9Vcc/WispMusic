// 本地解灰 sidecar：拉起 unm-server（Node + @unblockneteasemusic/server）
// 监听 127.0.0.1，渲染进程经 unm_match 命令调用，与 VutronMusic 的本地解灰同引擎。
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command},
    sync::{Arc, OnceLock},
    time::Duration,
};
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

#[derive(Debug)]
struct UnmProcess {
    port: u16,
    child: Option<Child>,
}

static STATE: OnceLock<Mutex<Option<Arc<UnmProcess>>>> = OnceLock::new();

fn state() -> &'static Mutex<Option<Arc<UnmProcess>>> {
    STATE.get_or_init(|| Mutex::new(None))
}

fn http_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            // 仅请求本机 sidecar，禁用系统代理检测（部分环境下 WinINET 代理查询会挂起）
            .no_proxy()
            .timeout(Duration::from_secs(90))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("failed to build unm http client")
    })
}

// 内嵌运行时（仅 release 编译进二进制，避免拖慢 debug 构建）：
// unm-server（含 node_modules）+ node.exe，首次解灰时解压到用户数据目录，
// 使生产环境完全免于系统 Node 与安装目录资源（方案：单 exe 自包含）
#[cfg(not(debug_assertions))]
static EMBED_UNM_SERVER: include_dir::Dir<'_> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../unm-server");
#[cfg(not(debug_assertions))]
static EMBED_NODE: include_dir::Dir<'_> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../node");

/// 解析 unm-server 目录与 node 可执行文件。
/// 开发环境用仓库源码目录；release 用内嵌运行时解压出的用户数据目录。
/// 注意 dev 模式下不能优先 resource_dir（tauri 复制出的副本不含 node_modules）。
async fn resolve_runtime(app: &AppHandle) -> Result<(PathBuf, String), String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dev = manifest_dir.join("../unm-server");
    if dev.join("index.js").exists() && dev.join("node_modules").exists() {
        let dev_node = manifest_dir.join("../node/node.exe");
        let node = if dev_node.exists() {
            dev_node.to_string_lossy().to_string()
        } else {
            // 系统 PATH 中的 node（spawn 失败时由启动逻辑报错）
            "node".to_string()
        };
        return Ok((dev, node));
    }

    // release：内嵌运行时解压到 app_local_data_dir/unm-runtime
    #[cfg(not(debug_assertions))]
    {
        let base = app
            .path()
            .app_local_data_dir()
            .map_err(|e| format!("resolve app_local_data_dir failed: {e}"))?
            .join("unm-runtime");
        ensure_extracted(app, &base).await?;
        let server = base.join("unm-server");
        let node = base.join("node").join("node.exe");
        if server.join("index.js").is_file() && node.is_file() {
            return Ok((server, node.to_string_lossy().to_string()));
        }
        return Err("embedded unm runtime incomplete after extraction".to_string());
    }

    #[cfg(debug_assertions)]
    {
        let _ = app;
        Err("unm-server not found (dev source dir missing node_modules; run npm install in unm-server)".to_string())
    }
}

/// 将内嵌运行时解压到 base（幂等）：版本标记一致且关键文件在位则跳过，
/// 否则全量重解压（升级后自动刷新）。约 84MB，放 blocking 线程执行。
#[cfg(not(debug_assertions))]
async fn ensure_extracted(app: &AppHandle, base: &PathBuf) -> Result<(), String> {
    let version = app.package_info().version.to_string();
    let marker = base.join(".runtime-version");
    let intact = marker.is_file()
        && std::fs::read_to_string(&marker)
            .map(|v| v == version)
            .unwrap_or(false)
        && base.join("unm-server").join("index.js").is_file()
        && base.join("node").join("node.exe").is_file();
    if intact {
        return Ok(());
    }

    let base = base.clone();
    let version = version.clone();
    tokio::task::spawn_blocking(move || {
        // 注意：include_dir 的 Dir::extract 不会创建 base 参数目录本身（只建子目录），
        // 根下还有 index.js 等文件，必须先建好两个目标子目录再解压
        std::fs::create_dir_all(base.join("unm-server"))?;
        std::fs::create_dir_all(base.join("node"))?;
        EMBED_UNM_SERVER.extract(base.join("unm-server"))?;
        EMBED_NODE.extract(base.join("node"))?;
        std::fs::write(base.join(".runtime-version"), version)
    })
    .await
    .map_err(|e| format!("extract embedded runtime task failed: {e}"))?
    .map_err(|e| format!("extract embedded runtime failed: {e}"))
}

fn pick_free_port() -> Option<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).ok()?;
    listener.local_addr().ok()?.port().into()
}

async fn health_ok(port: u16) -> bool {
    // 用原生 TCP 探测（避开 reqwest 在个别环境下对本机地址挂起的问题）；
    // 连上后发送一个最小 HTTP 请求并等待任意响应字节即视为健康
    let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let ok = stream
        .write_all(b"GET /healthz HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n")
        .is_ok();
    let mut buf = [0u8; 1];
    ok && matches!(std::io::Read::read(&mut stream, &mut buf), Ok(n) if n > 0)
}

/// 确保 sidecar 已启动；返回进程句柄。已启动时直接复用。
async fn ensure_started(app: &AppHandle) -> Result<Arc<UnmProcess>, String> {
    if let Some(process) = state().lock().await.as_ref() {
        return Ok(Arc::clone(process));
    }

    // 上一次宿主退出时遗留的 node 进程可能还在默认端口上
    if health_ok(25863).await {
        let process = Arc::new(UnmProcess {
            port: 25863,
            child: None,
        });
        *state().lock().await = Some(Arc::clone(&process));
        return Ok(process);
    }

    let (dir, node) = resolve_runtime(app).await?;
    let port = pick_free_port().ok_or_else(|| "failed to pick free port".to_string())?;

    let mut command = Command::new(&node);
    command
        .arg(dir.join("index.js"))
        .current_dir(&dir)
        .env("UNM_PORT", port.to_string());

    // stderr 写入日志文件：sidecar 启动失败（缺依赖/Node 版本问题等）时可排查
    if let Ok(log_dir) = app.path().app_log_dir() {
        let _ = std::fs::create_dir_all(&log_dir);
        let log_path = log_dir.join("unm-server.log");
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
        {
            command.stdout(std::process::Stdio::from(file));
            if let Ok(err_file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
            {
                command.stderr(std::process::Stdio::from(err_file));
            }
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：避免打包后拉起控制台窗口
        command.creation_flags(0x0800_0000);
    }

    let mut child = command
        .spawn()
        .map_err(|err| format!("spawn node sidecar failed ({node}): {err}"))?;

    #[cfg(windows)]
    {
        // 把 sidecar 挂进 KILL_ON_JOB_CLOSE 的 Job Object：
        // 宿主进程退出（正常退出、被 watchdog 结束、被强杀都一样）时，
        // 系统会连带清掉这个 node 进程，避免解灰 sidecar 在 127.0.0.1 上一直挂着。
        // job 句柄刻意不关闭：它的生命周期就应当与宿主进程一致。
        use std::os::windows::io::AsRawHandle;
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        unsafe {
            let job_result = CreateJobObjectW(None, PCWSTR::null());
            if let Ok(job) = job_result {
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

                let configured = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const std::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
                .as_bool();

                let child_handle = HANDLE(child.as_raw_handle() as isize);
                let assigned = AssignProcessToJobObject(job, child_handle).as_bool();

                if !(configured && assigned) {
                    // 挂载失败只影响"退出时自动清理"，不影响解灰功能本身
                    eprintln!(
                        "[unm] sidecar 未挂入 Job Object（configured={configured} assigned={assigned}），退出时可能残留"
                    );
                }
            } else if let Err(err) = job_result {
                eprintln!("[unm] 创建 sidecar Job Object 失败：{err}");
            }
        }
    }

    // 轮询健康检查，最多 15s；超时则杀掉子进程，避免泄漏
    for _ in 0..75 {
        if health_ok(port).await {
            let process = Arc::new(UnmProcess {
                port,
                child: Some(child),
            });
            *state().lock().await = Some(Arc::clone(&process));
            return Ok(process);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let _ = child.kill();
    let _ = child.wait();
    Err("unm-server did not become healthy within 15s (see unm-server.log)".to_string())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnmMatchData {
    pub url: String,
    pub source: String,
    pub br: Option<u64>,
    pub size: Option<u64>,
    pub md5: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UnmMatchResponse {
    code: i32,
    #[serde(default)]
    message: Option<String>,
    data: Option<UnmMatchData>,
}

/// 渲染进程解灰入口：本地 unm 引擎（跨平台搜索匹配，可解无损），
/// 参数语义与 VutronMusic 的解灰设置一致（enableFlac/orderFirst/qqCookie/jooxCookie），
/// 由前端在失败时回退到纯 HTTP 音源。
#[tauri::command]
pub async fn unm_match(
    app: AppHandle,
    songmid: String,
    source: Option<String>,
    enable_flac: Option<bool>,
    order_first: Option<bool>,
    qq_cookie: Option<String>,
    joox_cookie: Option<String>,
) -> Result<Option<UnmMatchData>, String> {
    let process = ensure_started(&app).await?;

    let mut params: Vec<(&str, String)> = vec![("songmid", songmid)];
    if let Some(source) = source.as_deref() {
        if !source.is_empty() {
            params.push(("source", source.to_string()));
        }
    }
    if enable_flac.unwrap_or(false) {
        params.push(("flac", "1".to_string()));
    }
    if order_first.unwrap_or(false) {
        params.push(("orderFirst", "1".to_string()));
    }
    if let Some(qq_cookie) = qq_cookie.as_deref() {
        if !qq_cookie.is_empty() {
            params.push(("qqCookie", qq_cookie.to_string()));
        }
    }
    if let Some(joox_cookie) = joox_cookie.as_deref() {
        if !joox_cookie.is_empty() {
            params.push(("jooxCookie", joox_cookie.to_string()));
        }
    }

    let url = Url::parse_with_params(
        &format!("http://127.0.0.1:{}/match", process.port),
        &params,
    )
    .map_err(|err| format!("build unm url failed: {err}"))?;

    let response: UnmMatchResponse = match http_client().get(url).send().await {
        Ok(resp) => resp
            .json()
            .await
            .map_err(|err| format!("unm-server response parse failed: {err}"))?,
        // sidecar 可能因空闲超时自行退出（或异常崩溃）：清理陈旧状态，下次调用重新拉起
        Err(err) => {
            let stale = state()
                .lock()
                .await
                .as_ref()
                .map(|p| p.port == process.port);
            if stale == Some(true) {
                *state().lock().await = None;
            }
            return Err(format!("unm-server request failed: {err}"));
        }
    };

    if response.code == 200 {
        Ok(response.data)
    } else {
        Err(response
            .message
            .unwrap_or_else(|| "unm match failed".to_string()))
    }
}

/// 渲染进程预热入口：只把 sidecar 拉起来并等健康检查通过，**不发起任何匹配**。
///
/// 首次播放灰色歌曲的等待里，「Node 进程冷启动 + require 解灰引擎（十几个 provider）
/// + release 首次解压内嵌运行时」这部分是可以提前付的：应用启动后在空闲时调一次，
/// 真正播放时 unm_match 命中 State 缓存直接可用（见 ensure_started 的复用分支）。
/// 幂等：已启动时立即返回；真实匹配的网络往返不在预热范围内，那部分只能等。
#[tauri::command]
pub async fn unm_warmup(app: AppHandle) -> Result<u16, String> {
    let process = ensure_started(&app).await?;
    Ok(process.port)
}

#[cfg(all(test, not(debug_assertions)))]
mod tests {
    use super::*;

    /// 验证内嵌运行时完整链路：解压 → 关键文件在位 → 用解压出的 node 拉起 sidecar 并通过健康检查。
    /// 仅 release 生效（嵌入静态量在 debug 下不存在）；跑法：cargo test --release
    #[test]
    fn embedded_runtime_extract_and_spawn() {
        let base = std::env::temp_dir().join("wisp-unm-embed-test");
        let _ = std::fs::remove_dir_all(&base);
        // extract 不创建 base 目录本身，先建目标子目录
        std::fs::create_dir_all(base.join("unm-server")).unwrap();
        std::fs::create_dir_all(base.join("node")).unwrap();

        EMBED_UNM_SERVER.extract(base.join("unm-server")).unwrap();
        EMBED_NODE.extract(base.join("node")).unwrap();
        assert!(base.join("unm-server/index.js").is_file());
        assert!(base
            .join("unm-server/node_modules/@unblockneteasemusic/server/package.json")
            .is_file());
        assert!(base.join("node/node.exe").is_file());

        let port = 25898u16;
        let mut child = Command::new(base.join("node/node.exe"))
            .arg(base.join("unm-server/index.js"))
            .current_dir(base.join("unm-server"))
            .env("UNM_PORT", port.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn embedded node failed");

        // 与 health_ok 相同的 TCP 探测逻辑（测试内保持同步实现）
        fn tcp_probe(port: u16) -> bool {
            use std::io::{Read, Write};
            let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
                return false;
            };
            let _ = stream.write_all(b"GET /healthz HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n");
            let mut buf = [0u8; 1];
            matches!(stream.read(&mut buf), Ok(n) if n > 0)
        }

        let mut healthy = false;
        for _ in 0..50 {
            if tcp_probe(port) {
                healthy = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&base);
        assert!(healthy, "embedded sidecar did not become healthy");
    }
}
