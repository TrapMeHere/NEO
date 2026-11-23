//! 进程峰值内存（Windows 用 Win32 API；其他平台退化为 None）。

/// 返回当前进程的峰值工作集（MiB）。
#[cfg(windows)]
pub fn peak_working_set_mib() -> Option<f64> {
    use windows_sys::Win32::Foundation::FALSE;
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: 只读查询当前进程的内存统计；结构体按文档初始化，句柄来自 GetCurrentProcess。
    unsafe {
        let mut counters = PROCESS_MEMORY_COUNTERS {
            cb: core::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            ..core::mem::zeroed()
        };
        let ok = K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb);
        if ok == FALSE {
            return None;
        }
        Some(counters.PeakWorkingSetSize as f64 / (1024.0 * 1024.0))
    }
}

/// 非 Windows 平台暂不统计（Linux 可读 /proc/self/status，macOS 用 task_info）。
#[cfg(not(windows))]
pub fn peak_working_set_mib() -> Option<f64> {
    None
}
