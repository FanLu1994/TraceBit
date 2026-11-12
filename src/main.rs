// Release builds are GUI-subsystem apps: no console window, and closing a
// terminal can no longer kill TraceBit. Debug builds keep the console for logs.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    let smoke = std::env::args().any(|arg| arg == "--native-smoke");
    if smoke {
        attach_parent_console();
    }
    let result = if smoke {
        tracebit::app::native_smoke()
    } else {
        tracebit::app::run()
    };
    if let Err(error) = result {
        eprintln!("TraceBit 启动失败：{error:#}");
        std::process::exit(1);
    }
}

/// A GUI-subsystem process has no console; when run from a terminal for the
/// smoke test, write to that terminal instead.
fn attach_parent_console() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
