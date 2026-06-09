use chrono::{Duration, Utc};
use lazytime::config::{Config, ThemePreference, TimeRange};
use lazytime::daemon::state::{DaemonState, PausedTracking};
use lazytime::platform::types::WindowInfo;
use lazytime::{db, rules};
use std::collections::BTreeMap;
use tempfile::tempdir;

fn test_config(db_file: String) -> Config {
    Config {
        onboarding_done: true,
        default_project: "DefaultProject".to_string(),
        tracking_stability_seconds: 10,
        working_hours: all_day_working_hours(),
        track_reminder_seconds: 300,
        track_reminder_snooze_seconds: 1800,
        summary_update_seconds: 5,
        report_start: None,
        report_end: None,
        db_file,
        jira_url: None,
        jira_token: None,
        jira_email: None,
        jira_project: None,
        jira_assignee: None,
        jira_issue_type: "Story".to_string(),
        jira_sap_field: "sap_project".to_string(),
        ipc_socket_path: None,
        theme_preference: ThemePreference::Auto,
        sidebar_collapsed: false,
    }
}

fn all_day_working_hours() -> BTreeMap<u8, Vec<TimeRange>> {
    (0..=6)
        .map(|day| {
            (
                day,
                vec![TimeRange {
                    start: "00:00".to_string(),
                    end: "23:59".to_string(),
                }],
            )
        })
        .collect()
}

#[tokio::test]
async fn debounce_switches_after_stability_window() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("test.sqlite");
    let mut conn = db::open(&db_path).expect("open");
    db::migrate(&conn).expect("migrate");
    db::replace_rules(
        &mut conn,
        "Alpha",
        Some("CP1"),
        &[("app-alpha", None, ".*", 0)],
    )
    .expect("alpha rules");
    db::replace_rules(
        &mut conn,
        "Beta",
        Some("CP2"),
        &[("app-beta", None, ".*", 0)],
    )
    .expect("beta rules");

    let config = test_config(db_path.to_string_lossy().to_string());

    let ruleset = rules::load_rules(&conn).expect("rules load");
    let cache = rules::RuleCache::default();
    cache.replace(ruleset).await;

    let mut state = DaemonState::new(config.clone());
    let now = Utc::now();

    let conn_read = db::open(config.db_path()).expect("open read conn");
    state
        .process_event(
            &conn_read,
            &cache,
            WindowInfo {
                app_id: Some("app-alpha".to_string()),
                instance: None,
                class: None,
                title: "alpha".to_string(),
                workspace: None,
                output: None,
            },
            now,
        )
        .await
        .expect("first event");

    let conn_read = db::open(config.db_path()).expect("open read conn");
    state
        .process_event(
            &conn_read,
            &cache,
            WindowInfo {
                app_id: Some("app-beta".to_string()),
                instance: None,
                class: None,
                title: "beta".to_string(),
                workspace: None,
                output: None,
            },
            now + Duration::seconds(1),
        )
        .await
        .expect("second event");

    let conn_check = db::open(config.db_path()).expect("open check");
    let active = db::get_active_tracking(&conn_check)
        .expect("active")
        .expect("active present");
    assert_eq!(active.project_name, "Alpha");

    let conn_read = db::open(config.db_path()).expect("open read conn");
    state
        .process_event(
            &conn_read,
            &cache,
            WindowInfo {
                app_id: Some("app-beta".to_string()),
                instance: None,
                class: None,
                title: "beta".to_string(),
                workspace: None,
                output: None,
            },
            now + Duration::seconds(12),
        )
        .await
        .expect("third event");

    let conn_check = db::open(config.db_path()).expect("open check");
    let active = db::get_active_tracking(&conn_check)
        .expect("active")
        .expect("active present");
    assert_eq!(active.project_name, "Beta");
}

#[tokio::test]
async fn switches_based_on_last_tracking_change_not_last_window_event() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("test.sqlite");
    let mut conn = db::open(&db_path).expect("open");
    db::migrate(&conn).expect("migrate");
    db::replace_rules(
        &mut conn,
        "Alpha",
        Some("CP1"),
        &[("app-alpha", None, ".*", 0)],
    )
    .expect("alpha rules");
    db::replace_rules(
        &mut conn,
        "Beta",
        Some("CP2"),
        &[("app-beta", None, ".*ABC.*", 0)],
    )
    .expect("beta rules");

    let config = test_config(db_path.to_string_lossy().to_string());

    let ruleset = rules::load_rules(&conn).expect("rules load");
    let cache = rules::RuleCache::default();
    cache.replace(ruleset).await;

    let mut state = DaemonState::new(config.clone());
    let now = Utc::now();

    // Start tracking Alpha at t0.
    let conn_read = db::open(config.db_path()).expect("open read conn");
    state
        .process_event(
            &conn_read,
            &cache,
            WindowInfo {
                app_id: Some("app-alpha".to_string()),
                instance: None,
                class: None,
                title: "alpha".to_string(),
                workspace: None,
                output: None,
            },
            now,
        )
        .await
        .expect("first event");

    // Another Alpha event shortly before threshold should not block switch at t+11.
    let conn_read = db::open(config.db_path()).expect("open read conn");
    state
        .process_event(
            &conn_read,
            &cache,
            WindowInfo {
                app_id: Some("app-alpha".to_string()),
                instance: None,
                class: None,
                title: "alpha still".to_string(),
                workspace: None,
                output: None,
            },
            now + Duration::seconds(9),
        )
        .await
        .expect("alpha refresh event");

    let conn_read = db::open(config.db_path()).expect("open read conn");
    state
        .process_event(
            &conn_read,
            &cache,
            WindowInfo {
                app_id: Some("app-beta".to_string()),
                instance: None,
                class: None,
                title: "ABC Backends - Entitlements.java".to_string(),
                workspace: None,
                output: None,
            },
            now + Duration::seconds(11),
        )
        .await
        .expect("beta event");

    let conn_check = db::open(config.db_path()).expect("open check");
    let active = db::get_active_tracking(&conn_check)
        .expect("active")
        .expect("active present");
    assert_eq!(active.project_name, "Beta");
}

#[test]
fn paused_tracking_roundtrip_in_memory() {
    let config = test_config("/tmp/lazytime-test.sqlite".to_string());

    let mut state = DaemonState::new(config);
    let paused = PausedTracking {
        id: 42,
        project_name: "Alpha".to_string(),
        start_ts: "2026-01-01T09:00:00Z".to_string(),
        paused_at: Utc::now(),
        output: Some("HDMI-A-1".to_string()),
    };

    state.mark_paused(paused.clone());
    assert!(state.paused().is_some());
    assert_eq!(state.paused().expect("paused").id, paused.id);

    let taken = state.take_paused().expect("take paused");
    assert_eq!(taken.id, paused.id);
    assert!(state.paused().is_none());
}

#[tokio::test]
async fn does_not_autostart_while_paused_from_lock() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("test.sqlite");
    let conn = db::open(&db_path).expect("open");
    db::migrate(&conn).expect("migrate");

    let config = test_config(db_path.to_string_lossy().to_string());

    let cache = rules::RuleCache::default();
    cache.replace(rules::RuleSet::default()).await;

    let mut state = DaemonState::new(config.clone());
    state.mark_paused(PausedTracking {
        id: 51,
        project_name: "Alpha".to_string(),
        start_ts: "2026-01-01T09:00:00Z".to_string(),
        paused_at: Utc::now(),
        output: Some("HDMI-A-1".to_string()),
    });

    state
        .process_event(
            &conn,
            &cache,
            WindowInfo {
                app_id: Some("app-alpha".to_string()),
                instance: None,
                class: None,
                title: "alpha".to_string(),
                workspace: None,
                output: Some("HDMI-A-1".to_string()),
            },
            Utc::now(),
        )
        .await
        .expect("event while paused");

    let conn_check = db::open(config.db_path()).expect("open check");
    let active = db::get_active_tracking(&conn_check).expect("active query");
    assert!(active.is_none());
}
