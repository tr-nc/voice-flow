use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, mpsc};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use dbus::blocking::Connection;
use tracing::{info, warn};

const BUS_NAME: &str = "dev.voiceflow.Preview";
const OBJECT_PATH: &str = "/dev/voiceflow/Preview";
const INTERFACE: &str = "dev.voiceflow.Preview";
const DBUS_TIMEOUT: Duration = Duration::from_millis(250);

static AVAILABLE: AtomicBool = AtomicBool::new(false);
static SENDER: OnceLock<mpsc::Sender<ShellOverlayCommand>> = OnceLock::new();

enum ShellOverlayCommand {
    Show {
        phase: String,
        text: String,
        message: String,
    },
    Hide,
}

pub fn initialize() -> Result<bool> {
    if SENDER.get().is_some() {
        return Ok(is_available());
    }

    let connection = Connection::new_session()
        .context("failed to connect to the session bus for the GNOME Shell preview")?;
    let proxy = connection.with_proxy(BUS_NAME, OBJECT_PATH, DBUS_TIMEOUT);
    let ping: std::result::Result<(bool,), dbus::Error> = proxy.method_call(INTERFACE, "Ping", ());
    if !matches!(ping, Ok((true,))) {
        return Ok(false);
    }

    let (sender, receiver) = mpsc::channel();
    SENDER
        .set(sender)
        .map_err(|_| anyhow::anyhow!("the GNOME Shell preview was already initialized"))?;
    AVAILABLE.store(true, Ordering::Release);
    thread::Builder::new()
        .name("voice-flow-shell-overlay".to_owned())
        .spawn(move || run_writer(connection, receiver))
        .context("failed to start the GNOME Shell preview writer")?;
    info!("GNOME Shell dictation preview ready");
    Ok(true)
}

pub fn is_available() -> bool {
    AVAILABLE.load(Ordering::Acquire)
}

pub fn show(phase: &str, text: &str, message: &str) {
    send(ShellOverlayCommand::Show {
        phase: phase.to_owned(),
        text: text.to_owned(),
        message: message.to_owned(),
    });
}

pub fn hide() {
    send(ShellOverlayCommand::Hide);
}

fn send(command: ShellOverlayCommand) {
    if !is_available() {
        return;
    }
    if let Some(sender) = SENDER.get()
        && sender.send(command).is_err()
    {
        AVAILABLE.store(false, Ordering::Release);
        warn!("GNOME Shell dictation preview stopped");
    }
}

fn run_writer(connection: Connection, receiver: mpsc::Receiver<ShellOverlayCommand>) {
    let proxy = connection.with_proxy(BUS_NAME, OBJECT_PATH, DBUS_TIMEOUT);
    while let Ok(command) = receiver.recv() {
        let result: std::result::Result<(), dbus::Error> = match command {
            ShellOverlayCommand::Show {
                phase,
                text,
                message,
            } => proxy.method_call(INTERFACE, "Show", (phase, text, message)),
            ShellOverlayCommand::Hide => proxy.method_call(INTERFACE, "Hide", ()),
        };
        if let Err(error) = result {
            let _: std::result::Result<(), dbus::Error> = proxy.method_call(INTERFACE, "Hide", ());
            AVAILABLE.store(false, Ordering::Release);
            warn!(%error, "failed to update the GNOME Shell dictation preview; using fallback");
            return;
        }
    }
    AVAILABLE.store(false, Ordering::Release);
}
