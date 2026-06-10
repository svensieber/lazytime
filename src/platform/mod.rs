pub mod types;

#[cfg(all(feature = "backend-linux", target_os = "linux"))]
mod atspi;
#[cfg(all(feature = "backend-macos", target_os = "macos"))]
mod ax;
#[cfg(all(feature = "backend-macos", target_os = "macos"))]
mod cgwindow;
#[cfg(all(feature = "backend-linux", target_os = "linux"))]
mod desktop;
#[cfg(all(feature = "backend-linux", target_os = "linux"))]
mod linux_lock;
#[cfg(all(feature = "backend-macos", target_os = "macos"))]
mod macos;
#[cfg(all(feature = "backend-macos", target_os = "macos"))]
mod macos_lock;
#[cfg(all(feature = "backend-sway", target_os = "linux"))]
mod sway;
#[cfg(all(feature = "backend-windows", target_os = "windows"))]
mod windows;
#[cfg(all(feature = "backend-linux", target_os = "linux"))]
mod x11;

use anyhow::Result;
use chrono::Utc;
use std::sync::mpsc;
use tokio::time::{Duration, sleep};

use crate::config::Config;
use crate::daemon::state::{DaemonState, PausedTracking};
use crate::popup::PopupAction;
use crate::popup::{
    PopupRequest, ResumeAction, ResumePopupRequest, spawn_popup_thread, spawn_resume_popup_thread,
};
use crate::rules::RuleCache;
use types::{LockEvent, OutputRect, WindowInfo};

#[cfg(all(feature = "backend-sway", target_os = "linux"))]
pub fn detected_backend_name() -> &'static str {
    if sway::is_available() {
        "sway"
    } else {
        "linux-desktop"
    }
}

#[cfg(all(
    not(feature = "backend-sway"),
    feature = "backend-linux",
    target_os = "linux"
))]
pub fn detected_backend_name() -> &'static str {
    "linux-desktop"
}

#[cfg(all(feature = "backend-windows", target_os = "windows"))]
pub fn detected_backend_name() -> &'static str {
    "windows"
}

#[cfg(all(feature = "backend-macos", target_os = "macos"))]
pub fn detected_backend_name() -> &'static str {
    "macos"
}

#[cfg(all(feature = "backend-macos", target_os = "macos"))]
pub fn request_macos_permissions_if_needed() {
    macos::request_permissions_if_needed();
}

#[cfg(not(all(feature = "backend-macos", target_os = "macos")))]
pub fn request_macos_permissions_if_needed() {}

#[cfg(not(any(
    all(feature = "backend-sway", target_os = "linux"),
    all(
        not(feature = "backend-sway"),
        feature = "backend-linux",
        target_os = "linux"
    ),
    all(feature = "backend-windows", target_os = "windows"),
    all(feature = "backend-macos", target_os = "macos")
)))]
pub fn detected_backend_name() -> &'static str {
    "none"
}

pub async fn run_event_loop(
    config: &Config,
    cache: RuleCache,
    mut state: DaemonState,
) -> Result<()> {
    tracing::info!("starting daemon event loop");
    #[cfg(not(feature = "popup-ui"))]
    tracing::warn!(
        "popup-ui feature is disabled; daemon reminders and resume dialogs will not be shown"
    );

    let (tx_popup, rx_popup) = mpsc::channel::<PopupAction>();
    let (tx_resume, rx_resume) = mpsc::channel::<ResumeAction>();
    let (tx_lock, rx_lock) = mpsc::channel::<LockEvent>();
    let (tx_window, rx_window) = mpsc::channel::<WindowInfo>();
    spawn_backend_monitors(tx_lock.clone(), tx_window);
    let mut resume_popup_open = false;

    loop {
        while let Ok(lock_event) = rx_lock.try_recv() {
            let now = Utc::now();
            match lock_event {
                LockEvent::Locked(source) => {
                    tracing::info!(
                        "lock_event: type=locked source={} output={:?} time={}",
                        source.as_str(),
                        state.last_output(),
                        crate::time::format_ts_local(&now)
                    );
                    if state.paused().is_some() {
                        tracing::info!(
                            "lock_event: duplicate locked signal ignored source={}",
                            source.as_str()
                        );
                        continue;
                    }
                    let conn = crate::db::open(config.db_path())?;
                    if let Some(active) = crate::db::get_active_tracking(&conn)? {
                        crate::db::update_tracking_times(
                            &conn,
                            active.id,
                            &active.project_name,
                            &active.start_ts,
                            Some(&crate::time::format_ts(&now)),
                            None::<&str>,
                        )?;
                        state.mark_paused(PausedTracking {
                            id: active.id,
                            project_name: active.project_name,
                            start_ts: active.start_ts,
                            paused_at: now,
                            output: state.last_output().map(ToString::to_string),
                        });
                    }
                }
                LockEvent::Unlocked(source) => {
                    tracing::info!(
                        "lock_event: type=unlocked source={} output={:?} time={}",
                        source.as_str(),
                        state.last_output(),
                        crate::time::format_ts_local(&now)
                    );
                    if resume_popup_open {
                        tracing::info!("resume_dialog: already open; skipping duplicate unlock");
                        continue;
                    }
                    let Some(paused) = state.paused().cloned() else {
                        continue;
                    };
                    let _ = spawn_resume_popup_thread(
                        ResumePopupRequest {
                            output: paused.output.clone(),
                            project_name: paused.project_name,
                            paused_tracking_id: paused.id,
                            paused_at_ts: crate::time::format_ts(&paused.paused_at),
                        },
                        tx_resume.clone(),
                    );
                    resume_popup_open = true;
                }
            }
        }

        while let Ok(info) = rx_window.try_recv() {
            tracing::debug!(
                "window focus change detected app_id={:?} instance={:?} class={:?} title={} workspace={:?} output={:?}",
                info.app_id,
                info.instance,
                info.class,
                info.title,
                info.workspace,
                info.output
            );
            let conn = crate::db::open(config.db_path())?;
            state.process_event(&conn, &cache, info, Utc::now()).await?;
        }

        {
            let conn = crate::db::open(config.db_path())?;
            state.refresh_autotracking_suspension(&conn, Utc::now())?;
        }

        while let Ok(action) = rx_popup.try_recv() {
            state.clear_reminder_popup();
            let conn = crate::db::open(config.db_path())?;
            match action {
                PopupAction::Yes => {
                    state.resume_autotracking(&conn)?;
                    let mut conn = crate::db::open(config.db_path())?;
                    crate::db::start_tracking(
                        &mut conn,
                        &config.default_project,
                        "daemon",
                        None,
                        None,
                        Some("popup-yes"),
                        None,
                        None,
                        Utc::now(),
                    )?;
                    tracing::info!(
                        "popup accepted; started default project '{}' at={}",
                        config.default_project,
                        crate::time::format_ts_local(&Utc::now())
                    );
                }
                PopupAction::No => state.reminder_no(&conn, Utc::now())?,
                PopupAction::Snooze => state.reminder_snooze(&conn, Utc::now())?,
            }
        }

        while let Ok(action) = rx_resume.try_recv() {
            resume_popup_open = false;
            let Some(paused) = state.paused().cloned() else {
                tracing::info!("resume_choice: no paused tracking in memory; ignoring action");
                continue;
            };
            let now = Utc::now();

            let apply_result = match action {
                ResumeAction::ContinueFromLockTime => {
                    let conn = crate::db::open(config.db_path())?;
                    let result = crate::db::update_tracking_times(
                        &conn,
                        paused.id,
                        &paused.project_name,
                        &paused.start_ts,
                        None,
                        None::<&str>,
                    );
                    if result.is_ok() {
                        tracing::info!(
                            "resume_action: reopened paused tracking id={} project={}",
                            paused.id,
                            paused.project_name
                        );
                    }
                    result
                }
                ResumeAction::ContinueFromNow => {
                    let mut conn = crate::db::open(config.db_path())?;
                    let result = crate::db::start_tracking(
                        &mut conn,
                        &paused.project_name,
                        "daemon",
                        None,
                        None,
                        Some("resume-from-lock"),
                        None,
                        paused.output.as_deref(),
                        now,
                    );
                    if result.is_ok() {
                        tracing::info!(
                            "resume_action: started new tracking project={} start_at={} replaced_paused_id={}",
                            paused.project_name,
                            crate::time::format_ts(&now),
                            paused.id
                        );
                    }
                    result
                }
                ResumeAction::Ignore => {
                    tracing::info!(
                        "resume_action: ignored paused tracking id={} project={}",
                        paused.id,
                        paused.project_name
                    );
                    Ok(())
                }
            };

            match apply_result {
                Ok(()) => {
                    let _ = state.take_paused();
                }
                Err(err) => tracing::error!("resume_action failed: {err}"),
            }
        }

        let reminder_now = Utc::now();
        if state.paused().is_none() && state.reminder_due(reminder_now) {
            let conn = crate::db::open(config.db_path())?;
            if crate::db::get_active_tracking(&conn)?.is_none()
                && !state.manual_stop_snooze_active(&conn, reminder_now)?
            {
                if state.reminder_popup_open() {
                    tracing::debug!("reminder popup already open; skipping spawn");
                } else {
                    tracing::info!("tracking reminder is due; launching popup thread");
                    let _ = spawn_popup_thread(
                        PopupRequest {
                            output: None,
                            message: "No active tracking. Start default project?".to_string(),
                        },
                        tx_popup.clone(),
                    );
                    state.mark_popup_shown(reminder_now);
                }
            }
        }

        sleep(Duration::from_millis(200)).await;
    }
}

#[cfg(all(feature = "backend-sway", target_os = "linux"))]
pub fn output_rect(output_name: &str) -> Option<OutputRect> {
    sway::output_rect(output_name)
}

#[cfg(all(feature = "backend-windows", target_os = "windows"))]
pub fn output_rect(output_name: &str) -> Option<OutputRect> {
    windows::output_rect(output_name)
}

#[cfg(all(feature = "backend-macos", target_os = "macos"))]
pub fn output_rect(output_name: &str) -> Option<OutputRect> {
    macos::output_rect(output_name)
}

#[cfg(not(any(
    all(feature = "backend-sway", target_os = "linux"),
    all(feature = "backend-windows", target_os = "windows"),
    all(feature = "backend-macos", target_os = "macos")
)))]
pub fn output_rect(_output_name: &str) -> Option<OutputRect> {
    None
}

#[cfg(all(feature = "backend-sway", target_os = "linux"))]
fn spawn_backend_monitors(tx_lock: mpsc::Sender<LockEvent>, tx_window: mpsc::Sender<WindowInfo>) {
    if sway::is_available() {
        tracing::info!("platform backend selected: sway");
        sway::spawn_sway_monitors(tx_lock, tx_window);
        return;
    }

    tracing::info!("platform backend selected: linux-desktop");
    desktop::spawn_desktop_monitors(tx_lock, tx_window);
}

#[cfg(all(
    not(feature = "backend-sway"),
    feature = "backend-linux",
    target_os = "linux"
))]
fn spawn_backend_monitors(tx_lock: mpsc::Sender<LockEvent>, tx_window: mpsc::Sender<WindowInfo>) {
    tracing::info!("platform backend selected: linux-desktop");
    desktop::spawn_desktop_monitors(tx_lock, tx_window);
}

#[cfg(not(any(
    all(feature = "backend-sway", target_os = "linux"),
    all(
        not(feature = "backend-sway"),
        feature = "backend-linux",
        target_os = "linux"
    ),
    all(feature = "backend-windows", target_os = "windows"),
    all(feature = "backend-macos", target_os = "macos")
)))]
fn spawn_backend_monitors(tx_lock: mpsc::Sender<LockEvent>, tx_window: mpsc::Sender<WindowInfo>) {
    let _ = tx_lock;
    let _ = tx_window;
    tracing::warn!("no platform backend enabled; daemon will only run reminder loop");
}

#[cfg(all(feature = "backend-windows", target_os = "windows"))]
fn spawn_backend_monitors(tx_lock: mpsc::Sender<LockEvent>, tx_window: mpsc::Sender<WindowInfo>) {
    tracing::info!("platform backend selected: windows");
    windows::spawn_windows_monitors(tx_lock, tx_window);
}

#[cfg(all(feature = "backend-macos", target_os = "macos"))]
fn spawn_backend_monitors(tx_lock: mpsc::Sender<LockEvent>, tx_window: mpsc::Sender<WindowInfo>) {
    tracing::info!("platform backend selected: macos");
    macos::spawn_macos_monitors(tx_lock, tx_window);
}
