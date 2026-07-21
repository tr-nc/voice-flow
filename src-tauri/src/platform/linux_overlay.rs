use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, mpsc};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use gtk::gdk;
use gtk::glib::{self, ControlFlow};
use gtk::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

const HELPER_ARGUMENT: &str = "--x11-overlay-helper";
const READY_PROMPT: &str = "Your mic is ready start speaking";
const OVERLAY_WIDTH: i32 = 720;
const OVERLAY_MIN_HEIGHT: i32 = 94;
const OVERLAY_MAX_HEIGHT: i32 = 280;
const OVERLAY_LABEL_WIDTH: i32 = 684;
const OVERLAY_VERTICAL_CHROME: i32 = 62;
const MAX_VISIBLE_CHARACTERS: usize = 700;

static HELPER_AVAILABLE: AtomicBool = AtomicBool::new(false);
static HELPER_SENDER: OnceLock<mpsc::Sender<OverlayCommand>> = OnceLock::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum OverlayCommand {
    SelectMonitor {
        name: String,
        logical_width: i32,
        logical_height: i32,
    },
    Show {
        phase: String,
        text: String,
        message: String,
    },
    Hide,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewStyle {
    Prompt,
    Streaming,
    Final,
}

pub fn initialize() -> Result<()> {
    if !should_start_helper(
        std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
        std::env::var_os("WAYLAND_DISPLAY").is_some(),
        std::env::var_os("DISPLAY").is_some(),
        std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref(),
    ) {
        return Ok(());
    }
    if HELPER_SENDER.get().is_some() {
        return Ok(());
    }

    let executable =
        std::env::current_exe().context("failed to locate the Voice Flow executable")?;
    let mut child = Command::new(executable)
        .arg(HELPER_ARGUMENT)
        .env("GDK_BACKEND", "x11")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start the X11 dictation overlay helper")?;
    let stdin = child
        .stdin
        .take()
        .context("the X11 overlay helper has no stdin")?;
    let stdout = child
        .stdout
        .take()
        .context("the X11 overlay helper has no stdout")?;
    let (sender, receiver) = mpsc::channel();
    HELPER_SENDER
        .set(sender)
        .map_err(|_| anyhow::anyhow!("the X11 overlay helper was already initialized"))?;

    thread::Builder::new()
        .name("voice-flow-x11-overlay".to_owned())
        .spawn(move || run_helper_writer(child, stdin, stdout, receiver))
        .context("failed to start the X11 overlay writer")?;
    Ok(())
}

pub fn is_available() -> bool {
    HELPER_AVAILABLE.load(Ordering::Acquire)
}

pub fn show(phase: &str, text: &str, message: &str) {
    send(OverlayCommand::Show {
        phase: phase.to_owned(),
        text: text.to_owned(),
        message: message.to_owned(),
    });
}

pub fn select_monitor(name: Option<&str>, physical_width: u32, physical_height: u32, scale: f64) {
    let logical_width = (f64::from(physical_width) / scale).round() as i32;
    let logical_height = (f64::from(physical_height) / scale).round() as i32;
    send(OverlayCommand::SelectMonitor {
        name: name.unwrap_or_default().to_owned(),
        logical_width,
        logical_height,
    });
}

pub fn hide() {
    send(OverlayCommand::Hide);
}

fn send(command: OverlayCommand) {
    if !is_available() {
        return;
    }
    if let Some(sender) = HELPER_SENDER.get()
        && sender.send(command).is_err()
    {
        HELPER_AVAILABLE.store(false, Ordering::Release);
        warn!("X11 dictation overlay helper stopped");
    }
}

fn run_helper_writer(
    mut child: Child,
    mut stdin: ChildStdin,
    stdout: impl Read + Send + 'static,
    receiver: mpsc::Receiver<OverlayCommand>,
) {
    let mut stdout = BufReader::new(stdout);
    let mut ready = String::new();
    if stdout.read_line(&mut ready).is_err() || ready.trim() != "ready" {
        let _ = child.wait();
        warn!("X11 dictation overlay helper failed before becoming ready");
        return;
    }

    if let Err(error) = thread::Builder::new()
        .name("voice-flow-x11-overlay-diagnostics".to_owned())
        .spawn(move || {
            for line in stdout.lines().map_while(Result::ok) {
                info!(diagnostic = %line, "X11 dictation overlay position");
            }
        })
    {
        warn!(%error, "failed to start the X11 overlay diagnostic reader");
    }

    HELPER_AVAILABLE.store(true, Ordering::Release);
    info!("X11 dictation overlay helper ready");
    while let Ok(command) = receiver.recv() {
        let write_result = serde_json::to_writer(&mut stdin, &command)
            .and_then(|_| stdin.write_all(b"\n").map_err(serde_json::Error::io))
            .and_then(|_| stdin.flush().map_err(serde_json::Error::io));
        if let Err(error) = write_result {
            HELPER_AVAILABLE.store(false, Ordering::Release);
            warn!(%error, "failed to update the X11 dictation overlay helper");
            break;
        }
    }
    HELPER_AVAILABLE.store(false, Ordering::Release);
    let _ = child.wait();
}

pub fn run_helper() -> Result<()> {
    gtk::init().context("failed to initialize GTK for the X11 overlay")?;
    let display = gdk::Display::default().context("the X11 overlay has no display")?;
    if display.type_().name() != "GdkX11Display" {
        bail!("the overlay helper did not start on X11");
    }

    let screen = gdk::Screen::default().context("the X11 overlay has no screen")?;
    let window = gtk::Window::new(gtk::WindowType::Popup);
    window.set_title("Voice Flow Dictation");
    window.set_widget_name("voice-flow-overlay-window");
    window.set_accept_focus(false);
    window.set_focus_on_map(false);
    window.set_keep_above(true);
    window.set_skip_pager_hint(true);
    window.set_skip_taskbar_hint(true);
    window.set_type_hint(gdk::WindowTypeHint::Notification);
    window.set_app_paintable(true);
    window.set_decorated(false);
    window.set_resizable(false);
    if let Some(visual) = screen.rgba_visual() {
        window.set_visual(Some(&visual));
    }

    let panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
    panel.set_widget_name("voice-flow-overlay-panel");
    let label = gtk::Label::new(None);
    label.set_widget_name("voice-flow-overlay-label");
    label.set_xalign(0.0);
    label.set_yalign(0.0);
    label.set_line_wrap(true);
    label.set_line_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_size_request(OVERLAY_LABEL_WIDTH, -1);
    panel.add(&label);
    window.add(&panel);

    let css = gtk::CssProvider::new();
    css.load_from_data(
        br#"
          #voice-flow-overlay-window { background-color: transparent; }
          #voice-flow-overlay-panel {
            min-height: 52px;
            padding: 12px 16px 38px;
            border: 1px solid rgba(255, 255, 255, 0.12);
            border-radius: 13px;
            background-color: rgba(37, 39, 44, 0.88);
            box-shadow: none;
          }
          #voice-flow-overlay-label {
            color: #b8c4bc;
            font-size: 16px;
            font-weight: 600;
          }
          #voice-flow-overlay-label.prompt {
            color: #9297a0;
            font-style: italic;
            font-weight: 400;
          }
          #voice-flow-overlay-label.final { color: #79d6a5; }
        "#,
    )
    .context("failed to load the X11 overlay style")?;
    gtk::StyleContext::add_provider_for_screen(
        &screen,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("voice-flow-overlay-input".to_owned())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else {
                    break;
                };
                if let Ok(command) = serde_json::from_str(&line)
                    && sender.send(command).is_err()
                {
                    return;
                }
            }
            let _ = sender.send(OverlayCommand::Quit);
        })
        .context("failed to start the X11 overlay input reader")?;

    let ui_display = display.clone();
    let mut target_monitor = None;

    // Keep one transparent, click-through X11 surface mapped. Mutter only
    // refreshes XWayland's global pointer coordinates while an X11 surface is
    // present; hiding the last surface leaves the pointer on its old monitor.
    window.set_opacity(0.0);
    window.show_all();
    if let Some(gdk_window) = window.window() {
        gdk_window.set_pass_through(true);
    }

    glib::timeout_add_local(Duration::from_millis(16), move || {
        loop {
            let command = match receiver.try_recv() {
                Ok(command) => command,
                Err(mpsc::TryRecvError::Empty) => return ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    window.hide();
                    gtk::main_quit();
                    return ControlFlow::Break;
                }
            };
            match command {
                OverlayCommand::SelectMonitor {
                    name,
                    logical_width,
                    logical_height,
                } => {
                    target_monitor = Some(MonitorTarget {
                        name,
                        logical_width,
                        logical_height,
                    });
                }
                OverlayCommand::Show {
                    phase,
                    text,
                    message,
                } => {
                    if let Some((content, style)) = preview_content(&phase, &text, &message) {
                        update_style(&label, style);
                        label.set_text(&content);
                        panel.show_all();
                        let natural_height = label
                            .layout()
                            .map(|layout| {
                                layout.set_width(OVERLAY_LABEL_WIDTH * gtk::pango::SCALE);
                                layout.set_wrap(gtk::pango::WrapMode::WordChar);
                                layout.pixel_size().1
                            })
                            .unwrap_or_else(|| {
                                label.preferred_height_for_width(OVERLAY_LABEL_WIDTH).1
                            });
                        let height = (natural_height + OVERLAY_VERTICAL_CHROME)
                            .clamp(OVERLAY_MIN_HEIGHT, OVERLAY_MAX_HEIGHT);
                        window.resize(OVERLAY_WIDTH, height);
                        let placement = overlay_placement(
                            &ui_display,
                            target_monitor.as_ref(),
                            OVERLAY_WIDTH,
                            height,
                        );
                        if let Some(placement) = placement {
                            window.move_(placement.x, placement.y);
                        }
                        window.show_all();
                        if let Some(gdk_window) = window.window() {
                            // XWayland does not reliably retain the initial GtkWindow
                            // position for an override-redirect popup. Apply the same
                            // coordinates directly after the native window is mapped.
                            if let Some(placement) = placement {
                                gdk_window.move_resize(
                                    placement.x,
                                    placement.y,
                                    OVERLAY_WIDTH,
                                    height,
                                );
                            }
                            gdk_window.set_pass_through(true);
                            gdk_window.raise();
                            window.set_opacity(1.0);
                            if let Some(placement) = placement {
                                report_placement(&gdk_window, placement);
                            }
                        }
                    } else {
                        window.set_opacity(0.0);
                    }
                }
                OverlayCommand::Hide => {
                    window.set_opacity(0.0);
                }
                OverlayCommand::Quit => {
                    window.hide();
                    gtk::main_quit();
                    return ControlFlow::Break;
                }
            }
        }
    });

    println!("ready");
    std::io::stdout().flush().ok();
    report_monitor_layout(&display);
    gtk::main();
    Ok(())
}

#[derive(Clone, Copy)]
struct OverlayPlacement {
    pointer_x: i32,
    pointer_y: i32,
    monitor_x: i32,
    monitor_y: i32,
    monitor_width: i32,
    monitor_height: i32,
    monitor_scale: i32,
    x: i32,
    y: i32,
}

struct MonitorTarget {
    name: String,
    logical_width: i32,
    logical_height: i32,
}

fn overlay_placement(
    display: &gdk::Display,
    target: Option<&MonitorTarget>,
    width: i32,
    height: i32,
) -> Option<OverlayPlacement> {
    let pointer_position = display
        .default_seat()
        .and_then(|seat| seat.pointer())
        .map(|pointer| {
            let (_, x, y) = pointer.position();
            (x, y)
        });
    let monitor = pointer_position
        .and_then(|(x, y)| display.monitor_at_point(x, y))
        .or_else(|| target.and_then(|target| find_target_monitor(display, target)))
        .or_else(|| display.primary_monitor());
    let monitor = monitor?;
    let geometry = monitor.geometry();
    let x = geometry.x() + (geometry.width() - width) / 2;
    let y = geometry.y() + (geometry.height() - height) / 2;
    let (pointer_x, pointer_y) = pointer_position.unwrap_or((-1, -1));
    Some(OverlayPlacement {
        pointer_x,
        pointer_y,
        monitor_x: geometry.x(),
        monitor_y: geometry.y(),
        monitor_width: geometry.width(),
        monitor_height: geometry.height(),
        monitor_scale: monitor.scale_factor(),
        x,
        y,
    })
}

fn find_target_monitor(display: &gdk::Display, target: &MonitorTarget) -> Option<gdk::Monitor> {
    let monitors: Vec<_> = (0..display.n_monitors())
        .filter_map(|index| display.monitor(index))
        .collect();
    monitors
        .iter()
        .find(|monitor| {
            !target.name.is_empty()
                && monitor
                    .model()
                    .is_some_and(|model| model.eq_ignore_ascii_case(&target.name))
        })
        .cloned()
        .or_else(|| {
            monitors.into_iter().find(|monitor| {
                let geometry = monitor.geometry();
                geometry.width() == target.logical_width
                    && geometry.height() == target.logical_height
            })
        })
}

fn report_monitor_layout(display: &gdk::Display) {
    for index in 0..display.n_monitors() {
        let Some(monitor) = display.monitor(index) else {
            continue;
        };
        let geometry = monitor.geometry();
        println!(
            "monitor index={index} x={} y={} width={} height={} scale={}",
            geometry.x(),
            geometry.y(),
            geometry.width(),
            geometry.height(),
            monitor.scale_factor(),
        );
    }
    std::io::stdout().flush().ok();
}

fn report_placement(window: &gdk::Window, placement: OverlayPlacement) {
    let (_, actual_x, actual_y) = window.origin();
    let (_, _, actual_width, actual_height) = window.geometry();
    println!(
        "pointer_x={} pointer_y={} monitor_x={} monitor_y={} monitor_width={} monitor_height={} monitor_scale={} requested_x={} requested_y={} actual_x={} actual_y={} actual_width={} actual_height={}",
        placement.pointer_x,
        placement.pointer_y,
        placement.monitor_x,
        placement.monitor_y,
        placement.monitor_width,
        placement.monitor_height,
        placement.monitor_scale,
        placement.x,
        placement.y,
        actual_x,
        actual_y,
        actual_width,
        actual_height,
    );
    std::io::stdout().flush().ok();
}

fn preview_content(phase: &str, text: &str, message: &str) -> Option<(String, PreviewStyle)> {
    if phase == "idle" || (phase == "complete" && !text.trim().is_empty()) {
        return None;
    }

    let trimmed = text.trim();
    if !trimmed.is_empty() {
        return Some((
            visible_tail(trimmed),
            if phase == "inserting" {
                PreviewStyle::Final
            } else {
                PreviewStyle::Streaming
            },
        ));
    }
    if phase == "connecting" || phase == "listening" {
        return Some((READY_PROMPT.to_owned(), PreviewStyle::Prompt));
    }
    let message = message.trim();
    (!message.is_empty()).then(|| (visible_tail(message), PreviewStyle::Prompt))
}

fn visible_tail(text: &str) -> String {
    let count = text.chars().count();
    if count <= MAX_VISIBLE_CHARACTERS {
        return text.to_owned();
    }
    let tail: String = text.chars().skip(count - MAX_VISIBLE_CHARACTERS).collect();
    format!("…{tail}")
}

fn update_style(label: &gtk::Label, style: PreviewStyle) {
    let context = label.style_context();
    context.remove_class("prompt");
    context.remove_class("final");
    match style {
        PreviewStyle::Prompt => context.add_class("prompt"),
        PreviewStyle::Streaming => {}
        PreviewStyle::Final => context.add_class("final"),
    }
}

fn should_start_helper(
    session_type: Option<&str>,
    has_wayland_display: bool,
    has_x11_display: bool,
    current_desktop: Option<&str>,
) -> bool {
    let is_wayland = session_type.is_some_and(|value| value.eq_ignore_ascii_case("wayland"))
        || has_wayland_display;
    let is_gnome = current_desktop.is_some_and(|desktop| {
        desktop
            .split(':')
            .any(|part| part.eq_ignore_ascii_case("gnome"))
    });
    is_wayland && is_gnome && has_x11_display
}

#[cfg(test)]
mod tests {
    use super::{PreviewStyle, preview_content, should_start_helper, visible_tail};

    #[test]
    fn starts_only_for_gnome_wayland_with_xwayland() {
        assert!(should_start_helper(
            Some("wayland"),
            true,
            true,
            Some("GNOME"),
        ));
        assert!(!should_start_helper(
            Some("wayland"),
            true,
            false,
            Some("GNOME"),
        ));
        assert!(!should_start_helper(
            Some("wayland"),
            true,
            true,
            Some("KDE"),
        ));
    }

    #[test]
    fn maps_runtime_states_to_overlay_content() {
        assert_eq!(
            preview_content("connecting", "", "Opening"),
            Some((
                "Your mic is ready start speaking".to_owned(),
                PreviewStyle::Prompt,
            )),
        );
        assert_eq!(
            preview_content("listening", "live words", "Live"),
            Some(("live words".to_owned(), PreviewStyle::Streaming)),
        );
        assert_eq!(
            preview_content("inserting", "final words", "Inserting"),
            Some(("final words".to_owned(), PreviewStyle::Final)),
        );
        assert_eq!(preview_content("idle", "", "Ready"), None);
        assert_eq!(preview_content("complete", "final words", "Inserted"), None);
    }

    #[test]
    fn long_previews_keep_the_latest_text() {
        let text = "a".repeat(710);
        let visible = visible_tail(&text);
        assert!(visible.starts_with('…'));
        assert_eq!(visible.chars().count(), 701);
    }
}
