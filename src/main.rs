fn main() {
    let result = if std::env::args().any(|arg| arg == "--native-smoke") {
        tracebit::app::native_smoke()
    } else {
        tracebit::app::run()
    };
    if let Err(error) = result {
        eprintln!("TraceBit 启动失败：{error:#}");
        std::process::exit(1);
    }
}
