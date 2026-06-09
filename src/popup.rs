use anyhow::Result;
#[cfg(all(feature = "popup-ui", target_os = "macos"))]
use std::process::Command;
use std::sync::mpsc;
#[cfg(feature = "popup-ui")]
use std::sync::OnceLock;
use std::thread;

#[cfg(feature = "popup-ui")]
use eframe::egui;
#[cfg(all(feature = "popup-ui", target_os = "linux"))]
use winit::platform::wayland::EventLoopBuilderExtWayland;
#[cfg(all(feature = "popup-ui", target_os = "windows"))]
use winit::platform::windows::EventLoopBuilderExtWindows;
#[cfg(all(feature = "popup-ui", target_os = "linux"))]
use winit::platform::x11::EventLoopBuilderExtX11;

#[cfg(all(feature = "popup-ui", target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinuxPopupBackend {
    Wayland,
    X11,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupAction {
    Yes,
    No,
    Snooze,
}

#[derive(Debug, Clone)]
pub struct PopupRequest {
    pub output: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeAction {
    ContinueFromLockTime,
    ContinueFromNow,
    Ignore,
}

#[derive(Debug, Clone)]
pub struct ResumePopupRequest {
    pub output: Option<String>,
    pub project_name: String,
    pub paused_tracking_id: i64,
    pub paused_at_ts: String,
}

#[cfg(feature = "popup-ui")]
enum PopupUiCommand {
    Reminder {
        request: PopupRequest,
        tx_action: mpsc::Sender<PopupAction>,
    },
    Resume {
        request: ResumePopupRequest,
        tx_action: mpsc::Sender<ResumeAction>,
    },
}

pub fn spawn_popup_thread(
    request: PopupRequest,
    tx_action: mpsc::Sender<PopupAction>,
) -> thread::JoinHandle<Result<()>> {
    thread::spawn(move || {
        tracing::info!("popup requested for output {:?}", request.output);
        tracing::info!("popup message: {}", request.message);

        #[cfg(feature = "popup-ui")]
        {
            popup_ui_sender().send(PopupUiCommand::Reminder { request, tx_action })?;
        }

        #[cfg(not(feature = "popup-ui"))]
        {
            // Fallback behavior when popup-ui feature is disabled.
            tx_action.send(PopupAction::No)?;
        }
        Ok(())
    })
}

pub fn spawn_resume_popup_thread(
    request: ResumePopupRequest,
    tx_action: mpsc::Sender<ResumeAction>,
) -> thread::JoinHandle<Result<()>> {
    thread::spawn(move || {
        tracing::info!(
            "resume_dialog: spawned for tracking id={} project={} paused_at={} output={:?}",
            request.paused_tracking_id,
            request.project_name,
            request.paused_at_ts,
            request.output
        );

        #[cfg(feature = "popup-ui")]
        {
            popup_ui_sender().send(PopupUiCommand::Resume { request, tx_action })?;
        }

        #[cfg(not(feature = "popup-ui"))]
        {
            tracing::info!("resume_dialog: popup-ui feature disabled; defaulting to Ignore");
            tx_action.send(ResumeAction::Ignore)?;
        }

        Ok(())
    })
}

#[cfg(feature = "popup-ui")]
fn popup_ui_sender() -> mpsc::Sender<PopupUiCommand> {
    static POPUP_UI_SENDER: OnceLock<mpsc::Sender<PopupUiCommand>> = OnceLock::new();
    POPUP_UI_SENDER
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel::<PopupUiCommand>();
            thread::spawn(move || popup_ui_worker(rx));
            tx
        })
        .clone()
}

#[cfg(feature = "popup-ui")]
fn popup_ui_worker(rx: mpsc::Receiver<PopupUiCommand>) {
    tracing::info!("popup ui worker started");
    while let Ok(cmd) = rx.recv() {
        match cmd {
            PopupUiCommand::Reminder { request, tx_action } => {
                run_reminder_popup(request, tx_action);
            }
            PopupUiCommand::Resume { request, tx_action } => {
                run_resume_popup(request, tx_action);
            }
        }
    }
    tracing::warn!("popup ui worker stopped");
}

#[cfg(feature = "popup-ui")]
fn run_reminder_popup(request: PopupRequest, tx_action: mpsc::Sender<PopupAction>) {
    let tx_for_ui = tx_action.clone();
    let app = PopupApp::new(request.message, tx_for_ui);
    let options = popup_native_options("LazyTime Reminder", [420.0, 160.0]);
    let run_result = eframe::run_native(
        "LazyTime Reminder",
        options,
        Box::new(move |_cc| Ok(Box::new(app))),
    );
    if let Err(err) = run_result {
        tracing::error!(
            "popup ui failed to start: {err}; WAYLAND_DISPLAY={:?} DISPLAY={:?} XDG_RUNTIME_DIR={:?}",
            std::env::var("WAYLAND_DISPLAY").ok(),
            std::env::var("DISPLAY").ok(),
            std::env::var("XDG_RUNTIME_DIR").ok()
        );
        let _ = tx_action.send(PopupAction::No);
    }
}

#[cfg(all(feature = "popup-ui", target_os = "macos"))]
fn run_resume_popup(request: ResumePopupRequest, tx_action: mpsc::Sender<ResumeAction>) {
    run_macos_resume_dialog(request, tx_action);
}

#[cfg(all(feature = "popup-ui", not(target_os = "macos")))]
fn run_resume_popup(request: ResumePopupRequest, tx_action: mpsc::Sender<ResumeAction>) {
    let tx_for_ui = tx_action.clone();
    let app = ResumePopupApp::new(request, tx_for_ui);
    let options = popup_native_options("LazyTime Resume Tracking", [520.0, 220.0]);
    let run_result = eframe::run_native(
        "LazyTime Resume Tracking",
        options,
        Box::new(move |_cc| Ok(Box::new(app))),
    );
    if let Err(err) = run_result {
        tracing::error!(
            "resume_dialog ui failed to start: {err}; WAYLAND_DISPLAY={:?} DISPLAY={:?} XDG_RUNTIME_DIR={:?}",
            std::env::var("WAYLAND_DISPLAY").ok(),
            std::env::var("DISPLAY").ok(),
            std::env::var("XDG_RUNTIME_DIR").ok()
        );
        let _ = tx_action.send(ResumeAction::Ignore);
    }
}

#[cfg(all(feature = "popup-ui", target_os = "macos"))]
fn run_macos_resume_dialog(request: ResumePopupRequest, tx_action: mpsc::Sender<ResumeAction>) {
    let paused_at = crate::time::parse_ts(&request.paused_at_ts)
        .map(|dt| crate::time::format_ts_local(&dt))
        .unwrap_or_else(|_| request.paused_at_ts.clone());
    let message = format!(
        "Project '{}' was paused at {}.\n\nChoose how to continue:",
        request.project_name, paused_at
    );
    let script = format!(
        "display dialog {} buttons {{\"Ignore\", \"Continue from now\", \"Continue from lock time\"}} default button \"Continue from now\" with title \"LazyTime Resume Tracking\"\nbutton returned of result",
        applescript_string(&message)
    );

    let action = match Command::new("osascript").arg("-e").arg(script).output() {
        Ok(output) if output.status.success() => {
            let button = String::from_utf8_lossy(&output.stdout);
            match button.trim() {
                "Continue from lock time" => ResumeAction::ContinueFromLockTime,
                "Continue from now" => ResumeAction::ContinueFromNow,
                _ => ResumeAction::Ignore,
            }
        }
        Ok(output) => {
            tracing::warn!(
                "resume_dialog: macos dialog failed status={:?} stderr={}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
            ResumeAction::Ignore
        }
        Err(err) => {
            tracing::warn!("resume_dialog: macos dialog failed to start: {err}");
            ResumeAction::Ignore
        }
    };

    tracing::info!(
        "resume_choice: id={} project={} choice={:?} choice_time={}",
        request.paused_tracking_id,
        request.project_name,
        action,
        crate::time::format_ts_local(&chrono::Utc::now())
    );
    let _ = tx_action.send(action);
}

#[cfg(all(feature = "popup-ui", target_os = "macos"))]
fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(feature = "popup-ui")]
struct PopupApp {
    message: String,
    tx_action: mpsc::Sender<PopupAction>,
    sent: bool,
}

#[cfg(all(feature = "popup-ui", not(target_os = "macos")))]
struct ResumePopupApp {
    request: ResumePopupRequest,
    tx_action: mpsc::Sender<ResumeAction>,
    sent: bool,
    positioned: bool,
}

#[cfg(feature = "popup-ui")]
impl PopupApp {
    fn new(message: String, tx_action: mpsc::Sender<PopupAction>) -> Self {
        Self {
            message,
            tx_action,
            sent: false,
        }
    }

    fn send_once(&mut self, action: PopupAction, ctx: &egui::Context) {
        if !self.sent {
            let _ = self.tx_action.send(action);
            self.sent = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

#[cfg(all(feature = "popup-ui", not(target_os = "macos")))]
impl ResumePopupApp {
    fn new(request: ResumePopupRequest, tx_action: mpsc::Sender<ResumeAction>) -> Self {
        Self {
            request,
            tx_action,
            sent: false,
            positioned: false,
        }
    }

    fn send_once(&mut self, action: ResumeAction, ctx: &egui::Context) {
        if !self.sent {
            tracing::info!(
                "resume_choice: id={} project={} choice={:?} choice_time={}",
                self.request.paused_tracking_id,
                self.request.project_name,
                action,
                crate::time::format_ts_local(&chrono::Utc::now())
            );
            let _ = self.tx_action.send(action);
            self.sent = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn place_on_output_once(&mut self, ctx: &egui::Context) {
        if self.positioned {
            return;
        }
        self.positioned = true;

        let Some(ref output_name) = self.request.output else {
            return;
        };
        let Some(pos) = output_center_position(output_name) else {
            tracing::info!("resume_dialog: could not resolve output geometry for {output_name}");
            return;
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
    }
}

#[cfg(feature = "popup-ui")]
#[cfg(not(target_os = "macos"))]
#[cfg(feature = "popup-output-placement")]
fn output_center_position(output_name: &str) -> Option<egui::Pos2> {
    let rect = crate::platform::output_rect(output_name)?;
    let x = rect.x as f32 + rect.width as f32 * 0.5 - 260.0;
    let y = rect.y as f32 + rect.height as f32 * 0.5 - 110.0;
    Some(egui::pos2(x.max(0.0), y.max(0.0)))
}

#[cfg(feature = "popup-ui")]
#[cfg(not(target_os = "macos"))]
#[cfg(not(feature = "popup-output-placement"))]
fn output_center_position(_output_name: &str) -> Option<egui::Pos2> {
    None
}

#[cfg(feature = "popup-ui")]
impl eframe::App for PopupApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("LazyTime Tracking Reminder");
                ui.separator();
                ui.label(&self.message);
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if padded_button(ui, "Yes").clicked() {
                        self.send_once(PopupAction::Yes, ctx);
                    }
                    if padded_button(ui, "No").clicked() {
                        self.send_once(PopupAction::No, ctx);
                    }
                    if padded_button(ui, "Snooze").clicked() {
                        self.send_once(PopupAction::Snooze, ctx);
                    }
                });
            });
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if !self.sent {
            let _ = self.tx_action.send(PopupAction::No);
            self.sent = true;
        }
    }
}

#[cfg(all(feature = "popup-ui", not(target_os = "macos")))]
impl eframe::App for ResumePopupApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.place_on_output_once(ctx);

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.heading("Resume tracking?");
                ui.separator();
                ui.label(format!(
                    "Project '{}' was paused at {}.",
                    self.request.project_name,
                    crate::time::parse_ts(&self.request.paused_at_ts)
                        .map(|dt| crate::time::format_ts_local(&dt))
                        .unwrap_or_else(|_| self.request.paused_at_ts.clone())
                ));
                ui.label("Choose how to continue:");
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if padded_button(ui, "Continue from lock time").clicked() {
                        self.send_once(ResumeAction::ContinueFromLockTime, ctx);
                    }
                    if padded_button(ui, "Continue from now").clicked() {
                        self.send_once(ResumeAction::ContinueFromNow, ctx);
                    }
                    if padded_button(ui, "Ignore").clicked() {
                        self.send_once(ResumeAction::Ignore, ctx);
                    }
                });
            });
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if !self.sent {
            let _ = self.tx_action.send(ResumeAction::Ignore);
            self.sent = true;
        }
    }
}

#[cfg(feature = "popup-ui")]
fn padded_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add_sized([160.0, 32.0], egui::Button::new(label))
}

#[cfg(feature = "popup-ui")]
fn popup_native_options(title: &str, size: [f32; 2]) -> eframe::NativeOptions {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_always_on_top()
            .with_inner_size(size),
        ..Default::default()
    };

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(title)
            .with_always_on_top()
            .with_inner_size(size),
        ..Default::default()
    };

    #[cfg(target_os = "windows")]
    {
        options.event_loop_builder = Some(Box::new(move |builder| {
            EventLoopBuilderExtWindows::with_any_thread(builder, true);
        }));
    }

    #[cfg(target_os = "linux")]
    {
        let backend = resolve_linux_popup_backend();
        tracing::debug!("popup ui backend selected: {:?}", backend);
        options.event_loop_builder = Some(Box::new(move |builder| match backend {
            LinuxPopupBackend::Wayland => {
                EventLoopBuilderExtWayland::with_any_thread(builder, true);
            }
            LinuxPopupBackend::X11 => {
                EventLoopBuilderExtX11::with_any_thread(builder, true);
            }
        }));
    }

    options
}

#[cfg(all(feature = "popup-ui", target_os = "linux"))]
fn resolve_linux_popup_backend() -> LinuxPopupBackend {
    if let Some(force_backend) = std::env::var("LAZYTIME_POPUP_BACKEND").ok() {
        match force_backend.to_ascii_lowercase().as_str() {
            "wayland" => return LinuxPopupBackend::Wayland,
            "x11" => return LinuxPopupBackend::X11,
            _ => tracing::warn!(
                "unsupported LAZYTIME_POPUP_BACKEND={:?}; expected 'wayland' or 'x11'; using auto",
                force_backend
            ),
        }
    }

    let has_x11 = std::env::var("DISPLAY")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);

    if has_x11 {
        LinuxPopupBackend::X11
    } else {
        LinuxPopupBackend::Wayland
    }
}
