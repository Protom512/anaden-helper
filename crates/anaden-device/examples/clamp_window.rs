#![cfg(windows)]

use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};

fn main() -> Result<(), String> {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_err() {
            return Err("dpi awareness setup failed".into());
        }
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut process = "AnotherEden.exe".to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--process" => {
                process = args
                    .get(i + 1)
                    .cloned()
                    .ok_or_else(|| "--process needs value".to_string())?;
                i += 2;
            }
            other => return Err(format!("unknown arg: {other}")),
        }
    }

    match anaden_device::clamp_window_into_monitor(&process) {
        Ok(v) => {
            println!("{v:?}");
            println!("VERDICT=CLAMP_OK");
            Ok(())
        }
        Err(e) => {
            println!("VERDICT=CLAMP_FAILED err={e}");
            Err(format!("{e}"))
        }
    }
}
