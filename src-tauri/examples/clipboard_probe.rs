//! Local Linux insertion probe. Focus a disposable text field before sending
//! JSON strings on stdin. This emits real paste shortcuts, never Enter, and
//! reports metadata only. It does not start ASR or the application logger.

#[cfg(target_os = "linux")]
#[allow(dead_code)]
#[path = "../src/platform/mod.rs"]
mod platform;

#[cfg(target_os = "linux")]
fn main() -> anyhow::Result<()> {
    use std::io::{BufRead, Write};

    if std::env::args().nth(1).as_deref() == Some("--clipboard-read-helper") {
        return platform::run_clipboard_read_helper();
    }
    if std::env::args().nth(1).as_deref() == Some("--help") {
        println!(
            "Focus a disposable text field, then send one JSON string per stdin line.\n\
             Each line performs real cursor insertion and returns JSON status metadata.\n\
             Example input: \"Voice Flow clipboard check\"\n\
             No ASR requests or application log writes are made. EOF exits the probe."
        );
        return Ok(());
    }

    for line in std::io::stdin().lock().lines() {
        let text: String = serde_json::from_str(&line?)?;
        let started = std::time::Instant::now();
        let report = platform::insert_at_active_cursor(&text);
        println!(
            "{}",
            serde_json::json!({
                "status": report.insertion_status(),
                "error": report.insertion_error(),
                "elapsed_ms": started.elapsed().as_millis(),
            })
        );
        std::io::stdout().flush()?;
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("This desktop insertion probe requires Linux.");
}
