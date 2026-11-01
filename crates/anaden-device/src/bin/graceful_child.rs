#[cfg(windows)]
fn main() -> std::process::ExitCode {
    match graceful_child::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("graceful_child: {message}");
            std::process::ExitCode::from(2)
        }
    }
}

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
mod graceful_child {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::Duration;

    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
        GetMessageW, MSG, PostQuitMessage, RegisterClassW, TranslateMessage, WINDOW_EX_STYLE,
        WM_CLOSE, WM_DESTROY, WNDCLASSW, WS_OVERLAPPEDWINDOW,
    };
    use windows::core::PCWSTR;

    static IGNORE_WM_CLOSE: AtomicBool = AtomicBool::new(false);

    static CLOSE_DELAY_MS: AtomicU64 = AtomicU64::new(0);

    pub(super) fn run() -> Result<(), String> {
        let args: Vec<String> = std::env::args().collect();
        if args.len() != 4 {
            return Err(format!(
                "usage: graceful_child <close|ignore> <ready_file> <close_delay_ms> (args={})",
                args.len()
            ));
        }
        let mode = args[1].as_str();
        let ready_file = args[2].as_str();
        let close_delay_ms: u64 = args[3]
            .parse()
            .map_err(|e| format!("close_delay_ms パース失敗 ({e}): {}", args[3]))?;
        let ignore = mode == "ignore";
        if !ignore && mode != "close" {
            return Err(format!("未知の mode: {mode} (close|ignore)"));
        }
        IGNORE_WM_CLOSE.store(ignore, Ordering::Relaxed);
        CLOSE_DELAY_MS.store(close_delay_ms, Ordering::Relaxed);

        create_window()?;
        std::fs::write(ready_file, b"ready").map_err(|e| format!("ready_file 書込失敗: {e}"))?;
        pump_messages();
        Ok(())
    }

    fn create_window() -> Result<HWND, String> {
        let class_name: Vec<u16> = "ANADEN_T266_GRACEFUL_CHILD\0".encode_utf16().collect();
        let window_name: Vec<u16> = "anaden graceful_child fixture\0".encode_utf16().collect();
        unsafe {
            let instance = GetModuleHandleW(None).map_err(|e| format!("GetModuleHandleW: {e}"))?;
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: instance.into(),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                return Err("RegisterClassW 失敗".to_string());
            }
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(window_name.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                160,
                120,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .map_err(|e| format!("CreateWindowExW: {e}"))
        }
    }

    fn pump_messages() {
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        unsafe {
            loop {
                let fetched = GetMessageW(&mut msg, None, 0, 0);
                if fetched.0 <= 0 {
                    break;
                }
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }
        }
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            match msg {
                WM_CLOSE => {
                    if IGNORE_WM_CLOSE.load(Ordering::Relaxed) {
                        LRESULT(0)
                    } else {
                        let delay = Duration::from_millis(CLOSE_DELAY_MS.load(Ordering::Relaxed));
                        if !delay.is_zero() {
                            std::thread::sleep(delay);
                        }
                        let _ = DestroyWindow(hwnd);
                        LRESULT(0)
                    }
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, msg, wparam, lparam),
            }
        }
    }
}
