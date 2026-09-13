//! dev 专属生命周期保障：终端里结束 `pnpm tdev` 时，让被拉起的测试应用跟着退出。
//!
//! 背景：dev 下应用虽然是控制台程序（见 `main.rs` 的 `windows_subsystem` 条件），
//! 但终端侧"结束测试进程"的多种做法都只杀掉 dev CLI 链
//! （Ctrl+C 因进程组原因未送达应用、只 taskkill 掉 pnpm/tauri 的 PID、
//! IDE 的停止按钮、直接关闭终端窗口等），把应用留成孤儿：它既不响应窗口关闭按钮
//! （托盘应用的语义是收起而非退出），也不会自己结束——于是"上一次的实例还在跑"
//! 会持续误导后续调试（旧 build 的界面、端口占用、单实例插件把新实例顶掉等）。
//!
//! 做法：启动时抓到**父进程**（`tauri dev` → `cargo run` → 本进程）的句柄，
//! 之后每秒检查它是否已退出，退出即自行退出。两个关键取舍：
//!
//! * 句柄在父进程存活时打开，之后只等这个句柄——不受 Windows PID 复用影响；
//! * 启动瞬间父进程就已不在（被短命 shim 拉起、手动双击 exe 等）则**不启用**看门狗，
//!   宁可留着孤儿也不误杀正在调试的实例。
//!
//! release 构建不编译任何相关代码（`#[cfg(debug_assertions)]`），
//! 安装版行为完全不受影响。

/// 检查间隔：dev 场景下对退出延迟不敏感，1 秒足够且几乎不耗资源
#[cfg(debug_assertions)]
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// 连续命中次数：父进程退出后句柄会持续保持 signal，
/// 连续两轮确认只为抵御一次性的调度抖动
#[cfg(debug_assertions)]
const CONFIRM_ROUNDS: u32 = 2;

#[cfg(debug_assertions)]
mod imp {
    use super::{CONFIRM_ROUNDS, POLL_INTERVAL};
    use windows::Win32::Foundation::{BOOL, CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    /// 取指定进程的父进程 PID（遍历系统进程快照；查不到返回 None）
    pub(super) fn parent_process_id(self_pid: u32) -> Option<u32> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.ok()?;

        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        let mut found = None;
        let mut walking = unsafe { Process32FirstW(snapshot, &mut entry) }.as_bool();
        while walking {
            if entry.th32ProcessID == self_pid {
                found = Some(entry.th32ParentProcessID);
                break;
            }
            walking = unsafe { Process32NextW(snapshot, &mut entry) }.as_bool();
        }

        let _ = unsafe { CloseHandle(snapshot) };
        found.filter(|pid| *pid != 0 && *pid != self_pid)
    }

    /// 打开指定进程的句柄（只申请 SYNCHRONIZE：不读内存、不发信号）。
    /// 进程不存在或无权限时返回 None——"安全阀"依赖这个 None。
    pub(super) fn open_handle_for_pid(pid: u32) -> Option<HANDLE> {
        unsafe { OpenProcess(PROCESS_SYNCHRONIZE, BOOL(0), pid) }.ok()
    }

    /// 句柄是否已 signal（等价于"该进程已退出"）
    pub(super) fn is_process_signaled(handle: HANDLE) -> bool {
        // 注意：不能直接写成 `unsafe { .. } == WAIT_OBJECT_0`——
        // 语句位置上的块表达式会被解析成语句，后接二元运算符即语法错误
        let wait_result = unsafe { WaitForSingleObject(handle, 0) };
        wait_result == WAIT_OBJECT_0
    }

    pub fn spawn(app: tauri::AppHandle) {
        let self_pid = std::process::id();
        let parent = parent_process_id(self_pid).and_then(open_handle_for_pid);

        // 父进程在启动瞬间就已不在（或打不开）→ 不启用看门狗：
        // 这种情况说明拉起者不是长命的 dev 进程，宁可留着也不误杀
        let Some(parent) = parent else {
            return;
        };

        std::thread::spawn(move || {
            let mut hits = 0u32;
            loop {
                std::thread::sleep(POLL_INTERVAL);

                if is_process_signaled(parent) {
                    hits += 1;
                    if hits >= CONFIRM_ROUNDS {
                        eprintln!("[dev] 父进程已退出（dev 会话结束），测试应用随之退出");
                        let _ = unsafe { CloseHandle(parent) };
                        app.exit(0);
                        return;
                    }
                } else {
                    hits = 0;
                }
            }
        });
    }
}

/// 启动看门狗。release 构建下是空实现（调用处无需 cfg）。
pub fn spawn_watchdog(app: tauri::AppHandle) {
    #[cfg(debug_assertions)]
    imp::spawn(app);

    #[cfg(not(debug_assertions))]
    let _ = app;
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::imp;
    use std::process::Command;

    /// 能取到自身父进程 PID（cargo test 场景下父进程是 cargo）
    #[test]
    fn finds_parent_process_id() {
        let parent = imp::parent_process_id(std::process::id()).expect("应能取到父进程 PID");
        assert!(parent > 0);
        assert_ne!(parent, std::process::id());
    }

    /// 不存在的 PID 打不开句柄——安全阀（父进程已消失则不启用看门狗）依赖这条
    #[test]
    fn invalid_pid_yields_no_handle() {
        assert!(imp::open_handle_for_pid(0x7fff_fff0).is_none());
    }

    /// 进程退出后，它的句柄会进入 signal 状态——看门狗的核心判据
    #[test]
    fn handle_signals_after_process_exit() {
        let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
        let mut child = Command::new(shell)
            .args(["/C", "exit", "0"])
            .spawn()
            .expect("应能拉起测试用子进程");

        let handle = imp::open_handle_for_pid(child.id()).expect("应能打开子进程句柄");
        let _ = child.wait();

        let mut waited_ms = 0;
        while !imp::is_process_signaled(handle) && waited_ms < 2000 {
            std::thread::sleep(std::time::Duration::from_millis(20));
            waited_ms += 20;
        }

        assert!(
            imp::is_process_signaled(handle),
            "子进程退出后句柄应在 2s 内 signal（等待了 {waited_ms}ms）"
        );
    }
}
