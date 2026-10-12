//! What was Ben typing into when he hit the hotkey?
//!
//! The recorder already snapshots the foreground window handle before the pill
//! appears (it needs it to restore focus before pasting). The same handle also
//! tells us which application the text is destined for, which is the whole
//! basis of picking a transformation mode without Ben having to choose one.
//!
//! Everything here is best-effort. If the OS gives us nothing, the caller
//! falls back to the default mode — context is never required for dictation
//! to work.

use crate::applog;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct AppContext {
    /// Lowercased executable name without the path, e.g. `"code.exe"`.
    pub exe: String,
    /// The window title, as shown in the task bar.
    pub title: String,
}

impl AppContext {
    pub fn is_empty(&self) -> bool {
        self.exe.is_empty() && self.title.is_empty()
    }
}

/// Read the executable name and window title for a window handle.
pub fn for_hwnd(hwnd: usize) -> AppContext {
    if hwnd == 0 {
        return AppContext::default();
    }
    let ctx = read(hwnd);
    if !ctx.is_empty() {
        applog::log(&format!("Context: exe={} title={}", ctx.exe, ctx.title));
    }
    ctx
}

#[cfg(target_os = "windows")]
fn read(hwnd: usize) -> AppContext {
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    extern "system" {
        fn GetWindowThreadProcessId(hwnd: usize, pid: *mut u32) -> u32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn QueryFullProcessImageNameW(
            handle: isize,
            flags: u32,
            buf: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn CloseHandle(handle: isize) -> i32;
        fn GetWindowTextW(hwnd: usize, buf: *mut u16, max: i32) -> i32;
    }

    let mut ctx = AppContext::default();

    unsafe {
        // ── Window title ──
        let mut title_buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), title_buf.len() as i32);
        if len > 0 {
            ctx.title = String::from_utf16_lossy(&title_buf[..len as usize]);
        }

        // ── Executable name ──
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return ctx;
        }

        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle == 0 {
            return ctx;
        }

        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(handle);

        if ok != 0 && size > 0 {
            let full = String::from_utf16_lossy(&buf[..size as usize]);
            ctx.exe = full
                .rsplit(['\\', '/'])
                .next()
                .unwrap_or(&full)
                .to_lowercase();
        }
    }

    ctx
}

#[cfg(not(target_os = "windows"))]
fn read(_hwnd: usize) -> AppContext {
    AppContext::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_null_handle_yields_no_context() {
        assert!(for_hwnd(0).is_empty());
    }

    #[test]
    fn empty_context_is_detected() {
        assert!(AppContext::default().is_empty());
        assert!(!AppContext {
            exe: "code.exe".into(),
            title: String::new()
        }
        .is_empty());
    }
}
