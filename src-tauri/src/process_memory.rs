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
    pub name: String,
    /// 工作集（占用的物理内存），字节
    pub working_set: u64,
    /// 私有提交量，字节——更接近「关掉它能还回去多少」
    pub private_bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessMemoryReport {
    pub self_pid: u32,
    pub total_working_set: u64,
    pub total_private_bytes: u64,
    /// 按工作集从大到小
    pub processes: Vec<ProcessMemoryEntry>,
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{ProcessMemoryEntry, ProcessMemoryReport};
    use windows::Win32::Foundation::{CloseHandle, BOOL};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

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

    fn query_process_memory(pid: u32) -> Option<(u64, u64)> {
        // 只申请 QUERY_LIMITED_INFORMATION：读内存用量够用，对受保护进程也常能成功
        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, BOOL(0), pid) }.ok()?;

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
        let _ = unsafe { CloseHandle(handle) };

        if !ok.as_bool() {
            return None;
        }
        Some((counters.WorkingSetSize as u64, counters.PrivateUsage as u64))
    }

    pub(super) fn report() -> Result<ProcessMemoryReport, String> {
        let self_pid = std::process::id();
        let all = snapshot_processes()?;

        let mut processes: Vec<ProcessMemoryEntry> = collect_tree(self_pid, &all)
            .into_iter()
            .filter_map(|pid| {
                let (working_set, private_bytes) = query_process_memory(pid)?;
                let name = all
                    .iter()
                    .find(|process| process.pid == pid)
                    .map(|process| process.name.clone())
                    .unwrap_or_else(|| format!("pid {pid}"));
                Some(ProcessMemoryEntry {
                    pid,
                    name,
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
}
