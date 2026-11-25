//! Process peak memory (Win32 API on Windows; degrades to None on other platforms).

/// Returns the current process's peak working set (MiB).
#[cfg(windows)]
pub fn peak_working_set_mib() -> Option<f64> {
    use windows_sys::Win32::Foundation::FALSE;
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: read-only query of the current process's memory statistics; the struct is initialized per the docs and the handle comes from GetCurrentProcess.
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

/// Not measured on non-Windows platforms for now (Linux could read /proc/self/status, macOS could use task_info).
#[cfg(not(windows))]
pub fn peak_working_set_mib() -> Option<f64> {
    None
}
