use std::collections::VecDeque;
use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use chrono::Local;
use eframe::egui;
use egui_phosphor_icons::icons;

use crate::config::Config;
use crate::daemon::DAEMON_RUNTIME_LOCK_KEY;
use crate::db;

use super::super::style;

const MAX_LOG_LINES: usize = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonStatus {
    Outside,
    Running,
    Stopped,
}

#[derive(Default)]
pub struct DaemonView {
    logs: VecDeque<String>,
    owner_id: String,
    child: Option<Child>,
    receiver: Option<Receiver<String>>,
    status: Option<DaemonStatus>,
    debug_enabled: bool,
    keep_running: bool,
    last_restart_attempt: Option<Instant>,
}

impl DaemonView {
    pub fn auto_start_on_gui_launch(&mut self, config: &Config) -> Option<String> {
        self.ensure_owner_id();
        let status = self.compute_status(config);
        self.status = Some(status);
        if status == DaemonStatus::Stopped {
            let start_msg = self.start_daemon(config);
            if start_msg == "daemon running" {
                self.keep_running = true;
                self.push_log("daemon auto-started by GUI".to_string());
                return Some("daemon auto-started".to_string());
            }
            return Some(start_msg);
        }
        None
    }

    pub fn status_text(&self, config: &Config) -> &'static str {
        match self.compute_status(config) {
            DaemonStatus::Outside => "outside",
            DaemonStatus::Running => "running",
            DaemonStatus::Stopped => "stopped",
        }
    }

    pub fn poll(&mut self, config: &Config) {
        self.ensure_owner_id();
        self.poll_events();
        self.poll_child_exit(config);
        self.restart_if_needed(config);
        self.status = Some(self.compute_status(config));
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, config: &Config) -> Option<String> {
        let mut message = None;
        let status = self.compute_status(config);
        let can_start = status == DaemonStatus::Stopped;
        let can_stop = status != DaemonStatus::Stopped;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Daemon").size(18.0).strong());
            ui.label(format!("status: {}", self.status_text(config)));
            let changed = ui
                .checkbox(&mut self.debug_enabled, "DEBUG")
                .on_hover_text("Run daemon with --loglevel=DEBUG")
                .changed();
            if changed && status != DaemonStatus::Stopped {
                message = Some(self.restart_daemon(config));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(
                        can_stop,
                        egui::Button::new(style::icon_label(ui, icons::STOP, "")),
                    )
                    .on_hover_text("Stop")
                    .clicked()
                {
                    message = Some(self.stop(config));
                }
                if ui
                    .add_enabled(
                        can_start,
                        egui::Button::new(style::icon_label(ui, icons::PLAY, "")),
                    )
                    .on_hover_text("Start")
                    .clicked()
                {
                    message = Some(self.start_daemon(config));
                    if message.as_deref() == Some("daemon running") {
                        self.keep_running = true;
                    }
                }
            });
        });

        if status == DaemonStatus::Outside {
            ui.label("Daemon running outside GUI process");
        }

        let log_bg = ui.visuals().extreme_bg_color.gamma_multiply(0.8);
        egui::Frame::new()
            .fill(log_bg)
            .inner_margin(egui::Margin::same(style::BUTTON_PAD_Y))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        for line in &self.logs {
                            ui.label(line);
                        }
                    });
            });

        message
    }

    fn compute_status(&self, config: &Config) -> DaemonStatus {
        let lock_owner = db::open(config.db_path()).ok().and_then(|conn| {
            db::get_config_key(&conn, DAEMON_RUNTIME_LOCK_KEY)
                .ok()
                .flatten()
        });
        match lock_owner {
            Some(owner) if lock_owner_matches_tui(&owner, &self.owner_id) => DaemonStatus::Running,
            Some(_) => DaemonStatus::Outside,
            None => DaemonStatus::Stopped,
        }
    }

    fn start_daemon(&mut self, config: &Config) -> String {
        if self.compute_status(config) == DaemonStatus::Running {
            return "daemon already running".to_string();
        }
        let exe = match std::env::current_exe() {
            Ok(v) => v,
            Err(err) => return format!("failed to locate executable: {err}"),
        };

        let mut cmd = Command::new(exe);
        let loglevel = if self.debug_enabled { "DEBUG" } else { "INFO" };
        cmd.arg("--daemon")
            .arg("--loglevel")
            .arg(loglevel)
            .env("LAZYTIME_DAEMON_OWNER", self.owner_id.clone())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => return format!("failed to start daemon: {err}"),
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (tx, rx) = mpsc::channel::<String>();

        if let Some(stdout) = stdout {
            let tx_out = tx.clone();
            std::thread::spawn(move || {
                let reader = std::io::BufReader::new(stdout);
                for line in reader.lines().map_while(|line| line.ok()) {
                    let _ = tx_out.send(line);
                }
            });
        }
        if let Some(stderr) = stderr {
            let tx_err = tx.clone();
            std::thread::spawn(move || {
                let reader = std::io::BufReader::new(stderr);
                for line in reader.lines().map_while(|line| line.ok()) {
                    let _ = tx_err.send(line);
                }
            });
        }

        self.child = Some(child);
        self.receiver = Some(rx);
        self.keep_running = true;
        self.push_log(format!("daemon process started (loglevel={loglevel})"));
        "daemon running".to_string()
    }

    fn restart_daemon(&mut self, config: &Config) -> String {
        let stop_msg = self.stop(config);
        let start_msg = self.start_daemon(config);
        if start_msg == "daemon running" {
            format!(
                "daemon restarted ({})",
                if self.debug_enabled { "DEBUG" } else { "INFO" }
            )
        } else {
            format!("{stop_msg}; {start_msg}")
        }
    }

    fn stop(&mut self, config: &Config) -> String {
        self.keep_running = false;
        let status = self.compute_status(config);
        if status == DaemonStatus::Outside {
            return self.stop_outside_daemon(config);
        }
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.cleanup_owned_lock(config, true);
        self.receiver = None;
        self.push_log("daemon stop requested".to_string());
        "daemon stopped".to_string()
    }

    fn stop_outside_daemon(&mut self, config: &Config) -> String {
        let conn = match db::open(config.db_path()) {
            Ok(c) => c,
            Err(err) => return format!("cannot open db: {err}"),
        };
        let lock_owner = match db::get_config_key(&conn, DAEMON_RUNTIME_LOCK_KEY) {
            Ok(v) => v,
            Err(err) => return format!("cannot read lock: {err}"),
        };
        let Some(owner) = lock_owner else {
            return "daemon is not running".to_string();
        };
        let parsed = parse_lock_owner(&owner);
        let Some(pid) = parsed.pid else {
            return format!("cannot stop outside daemon (owner={owner})");
        };
        if !stop_process_by_pid(pid) {
            return format!("failed to stop outside daemon pid={pid}");
        }
        self.push_log(format!("stopped outside daemon pid={pid}"));
        "outside daemon stopped".to_string()
    }

    fn poll_events(&mut self) {
        loop {
            let event = match self.receiver.as_ref() {
                Some(rx) => match rx.try_recv() {
                    Ok(ev) => ev,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.receiver = None;
                        break;
                    }
                },
                None => break,
            };
            self.push_log(event);
        }
    }

    fn poll_child_exit(&mut self, config: &Config) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                self.push_log(format!("daemon exited: {status}"));
                self.cleanup_owned_lock(config, false);
                self.child = None;
            }
            Ok(None) => {}
            Err(err) => {
                self.push_log(format!("failed to poll daemon process: {err}"));
                self.cleanup_owned_lock(config, false);
                self.child = None;
            }
        }
    }

    fn restart_if_needed(&mut self, config: &Config) {
        if !self.keep_running || self.compute_status(config) != DaemonStatus::Stopped {
            return;
        }
        if self
            .last_restart_attempt
            .is_some_and(|last| last.elapsed() < Duration::from_secs(5))
        {
            return;
        }
        self.last_restart_attempt = Some(Instant::now());
        let msg = self.start_daemon(config);
        self.push_log(format!("daemon auto-restart: {msg}"));
    }

    fn cleanup_owned_lock(&mut self, config: &Config, stop_pid: bool) {
        let lock_owner = db::open(config.db_path()).ok().and_then(|conn| {
            db::get_config_key(&conn, DAEMON_RUNTIME_LOCK_KEY)
                .ok()
                .flatten()
        });
        if let Some(owner) = lock_owner {
            let parsed = parse_lock_owner(&owner);
            let owned_by_tui =
                parsed.token.as_deref() == Some(self.owner_id.as_str()) || owner == self.owner_id;
            if owned_by_tui {
                if stop_pid {
                    if let Some(pid) = parsed.pid {
                        let _ = stop_process_by_pid(pid);
                    }
                }
                if let Ok(conn) = db::open(config.db_path()) {
                    let _ = db::release_lock_if_value(&conn, DAEMON_RUNTIME_LOCK_KEY, &owner);
                }
            }
        }
    }

    fn push_log(&mut self, line: String) {
        self.logs.push_back(format_log_entry(&line));
        while self.logs.len() > MAX_LOG_LINES {
            self.logs.pop_front();
        }
    }

    fn ensure_owner_id(&mut self) {
        if self.owner_id.is_empty() {
            self.owner_id = format!(
                "gui:{}:{}",
                std::process::id(),
                chrono::Utc::now().timestamp()
            );
        }
    }
}

#[derive(Debug, Clone)]
struct LockOwner {
    token: Option<String>,
    pid: Option<u32>,
}

fn lock_owner_matches_tui(owner: &str, owner_id: &str) -> bool {
    let parsed = parse_lock_owner(owner);
    match parsed.token {
        Some(token) => token == owner_id,
        None => owner == owner_id,
    }
}

fn format_log_entry(raw: &str) -> String {
    let ts = Local::now().format("%Y-%m-%d %H:%M:%S");
    let msg = extract_log_message(raw);
    format!("{ts} {msg}")
}

fn extract_log_message(raw: &str) -> String {
    let text = strip_ansi(raw);
    let text = text.trim();
    let Some((_, mut rest)) = split_after_level(text) else {
        return text.to_string();
    };

    for _ in 0..3 {
        let trimmed = rest.trim_start();
        if let Some(end) = trimmed.find("] ")
            && trimmed.starts_with('[')
            && end > 1
        {
            rest = &trimmed[end + 2..];
            continue;
        }

        if let Some(pos) = trimmed.find(" - ") {
            let prefix = &trimmed[..pos];
            if is_boilerplate_prefix(prefix) {
                rest = &trimmed[pos + 3..];
                continue;
            }
        }

        if let Some(pos) = trimmed.find(": ") {
            let prefix = &trimmed[..pos];
            if is_boilerplate_prefix(prefix) {
                rest = &trimmed[pos + 2..];
                continue;
            }
        }

        rest = trimmed;
        break;
    }

    let message = rest.trim();
    if looks_like_noise(message) {
        return text.to_string();
    }
    message.to_string()
}

fn looks_like_noise(message: &str) -> bool {
    if message.is_empty() {
        return true;
    }
    if message.len() > 3 {
        return false;
    }
    message.chars().all(|c| !c.is_ascii_alphanumeric())
}

fn split_after_level(input: &str) -> Option<(&str, &str)> {
    for level in ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"] {
        if let Some(idx) = input.find(level) {
            let start_ok = idx == 0
                || !input[..idx]
                    .chars()
                    .next_back()
                    .unwrap_or(' ')
                    .is_ascii_alphanumeric();
            let end_idx = idx + level.len();
            let end_ok = end_idx >= input.len()
                || !input[end_idx..]
                    .chars()
                    .next()
                    .unwrap_or(' ')
                    .is_ascii_alphanumeric();
            if start_ok && end_ok {
                return Some((&input[..idx], &input[end_idx..]));
            }
        }
    }
    None
}

fn is_boilerplate_prefix(prefix: &str) -> bool {
    let trimmed = prefix.trim();
    if trimmed.is_empty() || trimmed.len() > 80 {
        return false;
    }
    if trimmed.contains("::") {
        return true;
    }
    trimmed.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '_' | '-' | '.' | '[' | ']' | '(' | ')' | '{' | '}' | '=')
    })
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            i += 1;
            if i < bytes.len() && bytes[i] == b'[' {
                i += 1;
                while i < bytes.len() {
                    let b = bytes[i];
                    i += 1;
                    if b.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

fn parse_lock_owner(owner: &str) -> LockOwner {
    if let Some((token, tail)) = owner.split_once("|pid:") {
        let pid = tail.trim().parse::<u32>().ok();
        return LockOwner {
            token: Some(token.to_string()),
            pid,
        };
    }

    if let Some(pid_str) = owner.strip_prefix("pid:") {
        return LockOwner {
            token: None,
            pid: pid_str.trim().parse::<u32>().ok(),
        };
    }

    LockOwner {
        token: Some(owner.to_string()),
        pid: None,
    }
}

#[cfg(target_family = "unix")]
fn stop_process_by_pid(pid: u32) -> bool {
    Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_family = "windows")]
fn stop_process_by_pid(pid: u32) -> bool {
    Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
