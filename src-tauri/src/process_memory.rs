//! 进程内存归因（诊断用，仅调试入口手动触发）。
//!
//! WebView2 把渲染、GPU、网络等跑在**子进程**里，任务管理器里看到的
//! "WispMusic" 常常只是宿主进程，真正的大头在 msedgewebview2.exe 上。
//! 所以这里从本进程出发，沿 ToolHelp 快照里的父子关系收集整棵进程树，
//! 逐个读工作集与私有提交量，前端据此给出「钱花在哪」的真实数字。
//!
//! 不做任何常驻采样：只有前端点「采样」时才走一次。

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessMemoryEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    /// WebView2/Chromium 进程角色：browser / renderer / gpu / network / audio / utility
    /// （取自命令行 `--type=` / `--utility-sub-type=`；读不到则为空）
    pub role: String,
    /// 工作集（占用的物理内存），字节
    pub working_set: u64,
    /// 私有提交量，字节——更接近「关掉它能还回去多少」
    pub private_bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessMemoryReport {
    pub self_pid: u32,
    /// 各进程工作集之和。注意：Chromium 各进程共享大量代码页与共享内存，
    /// 直接相加会**重复计算**共享部分，只适合看相对大小
    pub total_working_set: u64,
    /// 各进程私有提交量之和：更接近「这套进程实际占掉多少、关掉能还回多少」
    pub total_private_bytes: u64,
    /// 按工作集从大到小
    pub processes: Vec<ProcessMemoryEntry>,
}

/// 从命令行判断 Chromium 进程角色（纯函数，便于单测）。
///
/// WebView2 是把渲染、GPU、网络、音频拆成独立子进程的，不区分角色就只能看到
/// 一串同名进程，无法判断「省掉一个窗口能省多少」。
pub fn process_role_from_command_line(command_line: &str) -> &'static str {
    if command_line.contains("--type=renderer") {
        return "renderer";
    }
    if command_line.contains("--type=gpu-process") {
        return "gpu";
    }
    if command_line.contains("--type=utility") {
        if command_line.contains("network.mojom.NetworkService") {
            return "network";
        }
        if command_line.contains("audio.mojom.AudioService") {
            return "audio";
        }
        return "utility";
    }
    if command_line.contains("--type=") {
        return "other";
    }
    // 没有 --type= 的那个就是浏览器主进程（WebView2 运行时的根）
    "browser"
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{ProcessMemoryEntry, ProcessMemoryReport};
    use windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::{
        NtQueryInformationProcess, OpenProcess, PROCESSINFOCLASS,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use std::ffi::c_void;

    struct RawProcess {
        pid: u32,
        parent_pid: u32,
        name: String,
    }

    /// 把 PROCESSENTRY32W 里定长的 UTF-16 名截到第一个 NUL
    fn decode_process_name(raw: &[u16]) -> String {
        let end = raw.iter().position(|unit| *unit == 0).unwrap_or(raw.len());
        String::from_utf16_lossy(&raw[..end])
    }

    fn snapshot_processes() -> Result<Vec<RawProcess>, String> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
            .map_err(|err| format!("create process snapshot failed: {err}"))?;

        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        let mut processes = Vec::new();
        let mut walking = unsafe { Process32FirstW(snapshot, &mut entry) }.as_bool();
        while walking {
            processes.push(RawProcess {
                pid: entry.th32ProcessID,
                parent_pid: entry.th32ParentProcessID,
                name: decode_process_name(&entry.szExeFile),
            });
            walking = unsafe { Process32NextW(snapshot, &mut entry) }.as_bool();
        }

        let _ = unsafe { CloseHandle(snapshot) };
        Ok(processes)
    }

    /// 本进程 + 其全部后代。WebView2 的渲染/GPU 进程是宿主进程的子进程，
    /// 按父 PID 逐层展开即可；只认自己这棵树，避免把系统里其他 WebView2 应用
    /// （同样叫 msedgewebview2.exe）算进来。
    fn collect_tree(root_pid: u32, all: &[RawProcess]) -> Vec<u32> {
        let mut tree = vec![root_pid];
        let mut frontier = vec![root_pid];

        while let Some(parent) = frontier.pop() {
            for process in all {
                if process.parent_pid == parent && !tree.contains(&process.pid) {
                    tree.push(process.pid);
                    frontier.push(process.pid);
                }
            }
        }

        tree
    }

    /// 读工作集与私有提交量
    fn query_memory_counters(handle: HANDLE) -> Option<(u64, u64)> {
        // 传 EX 结构体指针 + EX 的大小，才能在同一份数据里拿到 PrivateUsage
        let mut counters = PROCESS_MEMORY_COUNTERS_EX {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        let ok = unsafe {
            K32GetProcessMemoryInfo(
                handle,
                &mut counters as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            )
        };

        if !ok.as_bool() {
            return None;
        }
        Some((counters.WorkingSetSize as u64, counters.PrivateUsage as u64))
    }

    /// 读目标进程的命令行。`NtQueryInformationProcess(ProcessCommandLineInformation)`
    /// 只需 QUERY_LIMITED_INFORMATION，且会把 UNICODE_STRING 与字符串本体一并写进
    /// 我们提供的缓冲（Buffer 指向缓冲内部），因此可以安全地就地读出。
    /// 读不到返回 None——角色只是附加信息，不该影响内存数字本身。
    pub(super) fn query_command_line(handle: HANDLE) -> Option<String> {
        // WebView2 的子进程命令行很长（一堆 enable-features 开关），给足缓冲一次成功
        const BUFFER_BYTES: usize = 64 * 1024;
        // PROCESSINFOCLASS::ProcessCommandLineInformation（0.44 未生成该常量）
        const PROCESS_COMMAND_LINE_INFORMATION: PROCESSINFOCLASS = PROCESSINFOCLASS(60);

        let mut buffer = vec![0u8; BUFFER_BYTES];
        let mut returned = 0u32;
        let status = unsafe {
            NtQueryInformationProcess(
                handle,
                PROCESS_COMMAND_LINE_INFORMATION,
                buffer.as_mut_ptr() as *mut c_void,
                BUFFER_BYTES as u32,
                &mut returned,
            )
        };
        if status.is_err() {
            return None;
        }

        // UNICODE_STRING { u16 Length; u16 MaximumLength; (4 字节对齐填充) PWSTR Buffer }
        let length = u16::from_ne_bytes([buffer[0], buffer[1]]) as usize;
        let pointer = usize::from_ne_bytes([
            buffer[8], buffer[9], buffer[10], buffer[11], buffer[12], buffer[13], buffer[14],
            buffer[15],
        ]);
        let base = buffer.as_ptr() as usize;
        let offset = pointer.checked_sub(base)?;
        let end = offset.checked_add(length)?;
        if length == 0 || end > buffer.len() || length % 2 != 0 {
            return None;
        }

        let mut units = Vec::with_capacity(length / 2);
        for chunk in buffer[offset..end].chunks_exact(2) {
            units.push(u16::from_ne_bytes([chunk[0], chunk[1]]));
        }
        Some(String::from_utf16_lossy(&units))
    }

    fn query_process(pid: u32) -> Option<(u64, u64, String)> {
        // 只申请 QUERY_LIMITED_INFORMATION：读内存用量与命令行够用，对受保护进程也常能成功
        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL(0), pid) }.ok()?;

        let memory = query_memory_counters(handle);
        let role = query_command_line(handle)
            .map(|line| super::process_role_from_command_line(&line).to_string())
            .unwrap_or_default();
        let _ = unsafe { CloseHandle(handle) };

        let (working_set, private_bytes) = memory?;
        Some((working_set, private_bytes, role))
    }

    pub(super) fn report() -> Result<ProcessMemoryReport, String> {
        let self_pid = std::process::id();
        let all = snapshot_processes()?;

        let mut processes: Vec<ProcessMemoryEntry> = collect_tree(self_pid, &all)
            .into_iter()
            .filter_map(|pid| {
                let (working_set, private_bytes, role) = query_process(pid)?;
                let raw = all.iter().find(|process| process.pid == pid);
                let name = raw
                    .map(|process| process.name.clone())
                    .unwrap_or_else(|| format!("pid {pid}"));
                let parent_pid = raw.map(|process| process.parent_pid).unwrap_or(0);
                Some(ProcessMemoryEntry {
                    pid,
                    parent_pid,
                    name,
                    role,
                    working_set,
                    private_bytes,
                })
            })
            .collect();

        processes.sort_by(|a, b| b.working_set.cmp(&a.working_set));

        Ok(ProcessMemoryReport {
            self_pid,
            total_working_set: processes.iter().map(|entry| entry.working_set).sum(),
            total_private_bytes: processes.iter().map(|entry| entry.private_bytes).sum(),
            processes,
        })
    }
}

#[tauri::command]
pub fn get_process_memory_report() -> Result<ProcessMemoryReport, String> {
    #[cfg(target_os = "windows")]
    {
        imp::report()
    }

    #[cfg(not(target_os = "windows"))]
    {
        Err("process memory report is only available on Windows".to_string())
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use windows::Win32::Foundation::{BOOL, CloseHandle};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// 真跑一遍 Win32 链路（进程快照 → OpenProcess → K32GetProcessMemoryInfo），
    /// 确保命令不是「编译得过但运行时拿不到数」。
    #[test]
    fn reports_current_process_and_sorted_totals() {
        let report = get_process_memory_report().expect("应能读到进程内存");

        assert_eq!(report.self_pid, std::process::id());

        let own = report
            .processes
            .iter()
            .find(|entry| entry.pid == report.self_pid)
            .expect("报告里应包含本进程");
        assert!(own.working_set > 0, "本进程工作集应大于 0");
        assert!(own.private_bytes > 0, "本进程私有提交量应大于 0");
        assert!(!own.name.is_empty(), "进程名不应为空");

        // 合计与排序：合计等于各项之和，且按工作集降序
        let summed: u64 = report.processes.iter().map(|entry| entry.working_set).sum();
        assert_eq!(report.total_working_set, summed);
        assert!(report
            .processes
            .windows(2)
            .all(|pair| pair[0].working_set >= pair[1].working_set));
    }

    #[test]
    fn identifies_chromium_process_roles() {
        assert_eq!(
            process_role_from_command_line(
                r#""C:\x\msedgewebview2.exe" --type=renderer --lang=zh-CN"#
            ),
            "renderer"
        );
        assert_eq!(
            process_role_from_command_line(
                r#""msedgewebview2.exe" --type=gpu-process --gpu-preferences=..."#
            ),
            "gpu"
        );
        assert_eq!(
            process_role_from_command_line(
                r#""msedgewebview2.exe" --type=utility --utility-sub-type=network.mojom.NetworkService"#
            ),
            "network"
        );
        assert_eq!(
            process_role_from_command_line(
                r#""msedgewebview2.exe" --type=utility --utility-sub-type=audio.mojom.AudioService"#
            ),
            "audio"
        );
        assert_eq!(
            process_role_from_command_line(
                r#""msedgewebview2.exe" --type=utility --utility-sub-type=storage.mojom.StorageService"#
            ),
            "utility"
        );
        // 没有 --type= 的就是 WebView2 运行时的浏览器主进程
        assert_eq!(
            process_role_from_command_line(r#""msedgewebview2.exe" --embedded-browser-webview"#),
            "browser"
        );
    }

    /// 真跑 NtQueryInformationProcess 那条路：能读到自身命令行。
    /// 这条链路（缓冲 + UNICODE_STRING 就地解析）出错时不会报错、只会静默降级成
    /// 「读不到角色」，所以必须用测试钉住。
    #[test]
    fn reads_own_command_line() {
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION,
                BOOL(0),
                std::process::id(),
            )
        }
        .expect("应能打开自身进程");

        let command_line = imp::query_command_line(handle);
        let _ = unsafe { CloseHandle(handle) };

        let command_line = command_line.expect("应能读到自身命令行");
        assert!(!command_line.is_empty(), "命令行不应为空");
        // 测试二进制没有 --type=，按规则应判为浏览器主进程
        assert_eq!(process_role_from_command_line(&command_line), "browser");
    }
}
