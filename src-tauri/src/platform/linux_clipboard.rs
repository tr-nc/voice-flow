//! Clipboard subprocesses with bounded lifetimes. The read helper uses native
//! data control (or GNOME's XWayland bridge), never a focus-taking wl-paste
//! window. Isolating it also bounds reads from an unresponsive clipboard owner.

use std::io::{Read, Write};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use arboard::Clipboard;

pub const READ_HELPER_ARGUMENT: &str = "--clipboard-read-helper";
const COMMAND_TIMEOUT: Duration = Duration::from_millis(500);
const POLL_DELAY: Duration = Duration::from_millis(5);

pub fn run_read_helper() -> Result<()> {
    let result = (|| {
        let mut clipboard =
            Clipboard::new().context("failed to open the native Linux clipboard")?;
        match clipboard.get_text() {
            Ok(text) => Ok(Some(text)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(error) => Err(error).context("failed to read the native Linux clipboard"),
        }
    })();
    let response: std::result::Result<Option<String>, String> =
        result.map_err(|error| format!("{error:#}"));
    // This private pipe intentionally carries clipboard text. The helper runs
    // before Tauri/logging startup; never send this response to tracing.
    serde_json::to_writer(std::io::stdout().lock(), &response)?;
    Ok(())
}

pub fn read_text() -> Result<Option<String>> {
    // Re-exec the running image even if a local build/install replaced its
    // pathname. current_exe() can point to an already unlinked "(deleted)"
    // file in that case, or launch a different version of the helper protocol.
    let mut command = Command::new("/proc/self/exe");
    command.arg(READ_HELPER_ARGUMENT);
    let output = read_output(command, COMMAND_TIMEOUT).context("native clipboard read failed")?;
    let result: std::result::Result<Option<String>, String> = serde_json::from_slice(&output)
        .context("invalid response from the clipboard read helper")?;
    result.map_err(anyhow::Error::msg)
}

fn read_output(mut command: Command, timeout: Duration) -> Result<Vec<u8>> {
    command.stdout(Stdio::piped());
    let mut process = ClipboardProcess::spawn(command, None)?;
    let mut stdout = process
        .child
        .stdout
        .take()
        .context("clipboard reader has no stdout")?;
    // Drain concurrently: clipboard text can exceed the OS pipe buffer.
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let completion = process.wait(timeout);
    // wait() kills/reaps on timeout, so the helper closes the pipe before join.
    let output = reader
        .join()
        .map_err(|_| anyhow::anyhow!("clipboard reader panicked"))?;
    let status = completion?;
    if !status.success() {
        bail!("clipboard reader exited with status {status}");
    }
    output.context("failed to collect the clipboard reader response")
}

pub struct ClipboardProcess {
    child: Child,
}

impl ClipboardProcess {
    pub fn spawn(mut command: Command, text: Option<&str>) -> Result<Self> {
        command
            .stdin(if text.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            // A background wl-copy inherits stderr. Capturing it would keep a
            // pipe open indefinitely, and protocol dumps must not reach logs.
            .stderr(Stdio::null());
        let mut process = Self {
            child: command
                .spawn()
                .context("failed to start clipboard command")?,
        };
        if let Some(text) = text {
            let mut stdin = process
                .child
                .stdin
                .take()
                .context("clipboard command has no stdin")?;
            let text = text.as_bytes().to_owned();
            let writer = thread::spawn(move || stdin.write_all(&text));
            let started = Instant::now();
            while !writer.is_finished() && started.elapsed() < COMMAND_TIMEOUT {
                thread::sleep(POLL_DELAY);
            }
            if !writer.is_finished() {
                process.stop()?;
                let _ = writer.join();
                bail!(
                    "clipboard command did not read its input within {} ms",
                    COMMAND_TIMEOUT.as_millis()
                );
            }
            writer
                .join()
                .map_err(|_| anyhow::anyhow!("clipboard input writer panicked"))?
                .context("failed to write clipboard command input")?;
        }
        Ok(process)
    }

    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub fn wait(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let started = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Ok(status),
                Ok(None) => {}
                Err(error) => {
                    let _ = self.stop();
                    return Err(error).context("failed to inspect clipboard command");
                }
            }
            if started.elapsed() >= timeout {
                self.stop()?;
                bail!(
                    "clipboard command timed out after {} ms",
                    timeout.as_millis()
                );
            }
            thread::sleep(POLL_DELAY);
        }
    }

    pub fn stop(&mut self) -> Result<()> {
        if self.child.try_wait()?.is_none() {
            self.child
                .kill()
                .context("failed to stop clipboard command")?;
        }
        self.child
            .wait()
            .context("failed to reap clipboard command")?;
        Ok(())
    }
}

impl Drop for ClipboardProcess {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalled_reader_is_killed_and_its_output_pipe_is_closed() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf partial; exec sleep 10"]);
        let started = Instant::now();
        let error = read_output(command, Duration::from_millis(30)).unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn failed_and_successful_readers_are_distinguished() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf invalid; exit 7"]);
        let error = read_output(command, COMMAND_TIMEOUT).unwrap_err();
        assert!(error.to_string().contains("exit status: 7"));
        let mut command = Command::new("printf");
        command.arg("{\"Ok\":null}");
        assert_eq!(
            read_output(command, COMMAND_TIMEOUT).unwrap(),
            b"{\"Ok\":null}"
        );
    }

    #[test]
    fn reader_output_larger_than_a_pipe_buffer_does_not_deadlock() {
        let mut command = Command::new("head");
        command.args(["-c", "131072", "/dev/zero"]);
        assert_eq!(read_output(command, COMMAND_TIMEOUT).unwrap().len(), 131072);
    }

    #[test]
    fn abandoned_foreground_owner_is_reaped() {
        let mut command = Command::new("sleep");
        command.arg("10");
        let process = ClipboardProcess::spawn(command, None).unwrap();
        let path = format!("/proc/{}", process.child.id());
        drop(process);
        assert!(!std::path::Path::new(&path).exists());
    }

    #[test]
    fn command_that_never_reads_stdin_cannot_block_insertion_forever() {
        let mut command = Command::new("sleep");
        command.arg("10");
        let started = Instant::now();
        let result = ClipboardProcess::spawn(command, Some(&"x".repeat(1024 * 1024)));
        assert!(
            result
                .err()
                .unwrap()
                .to_string()
                .contains("did not read its input")
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
