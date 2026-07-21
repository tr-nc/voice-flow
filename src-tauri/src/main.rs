fn main() {
    #[cfg(target_os = "linux")]
    if std::env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--x11-overlay-helper")) {
        if let Err(error) = voice_flow_lib::run_linux_overlay_helper() {
            eprintln!("Voice Flow overlay helper failed: {error}");
            std::process::exit(1);
        }
        return;
    }

    voice_flow_lib::run();
}
