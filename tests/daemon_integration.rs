use chrono::{Duration, Utc};
use lazytime::config::{Config, ThemePreference, TimeRange};
use lazytime::daemon::state::DaemonState;
use lazytime::platform::types::WindowInfo;
use lazytime::{db, rules};
use std::collections::BTreeMap;
use tempfile::tempdir;

fn test_config(db_path: &std::path::Path) -> Config {
    Config {
        onboarding_done: true,
        default_project: "DefaultProject".to_string(),
        tracking_stability_seconds: 5,
        working_hours: all_day_working_hours(),
        track_reminder_seconds: 300,
        track_reminder_snooze_seconds: 1800,
        summary_update_seconds: 5,
        report_start: None,
        report_end: None,
        db_file: db_path.to_string_lossy().to_string(),
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
async fn daemon_persists_workspace_and_output_fields() {
    let dir = tempdir().expect("tempdir");
    let db_path = dir.path().join("test.sqlite");
    let mut conn = db::open(&db_path).expect("open");
    db::migrate(&conn).expect("migrate");
    db::replace_rules(&mut conn, "Alpha", Some("CP1"), &[("app-a", None, ".*", 0)]).expect("rules");

    let ruleset = rules::load_rules(&conn).expect("rules load");
    let cache = rules::RuleCache::default();
    cache.replace(ruleset).await;

    let config = test_config(&db_path);
    let mut state = DaemonState::new(config.clone());
    let conn_read = db::open(config.db_path()).expect("open read");
    state
        .process_event(
            &conn_read,
            &cache,
            WindowInfo {
                app_id: Some("app-a".to_string()),
                instance: Some("inst".to_string()),
                class: Some("class".to_string()),
                title: "title".to_string(),
                workspace: Some("2:web".to_string()),
                output: Some("HDMI-A-1".to_string()),
            },
            Utc::now(),
        )
        .await
        .expect("event");

    let conn_check = db::open(config.db_path()).expect("open check");
    let (workspace, output): (Option<String>, Option<String>) = conn_check
        .query_row(
            "SELECT workspace, output FROM trackings WHERE end_ts IS NULL LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("row");
    assert_eq!(workspace.as_deref(), Some("2:web"));
    assert_eq!(output.as_deref(), Some("HDMI-A-1"));
}

#[tokio::test]
async fn daemon_debounce_switches_only_after_threshold() {
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

    let ruleset = rules::load_rules(&conn).expect("rules load");
    let cache = rules::RuleCache::default();
    cache.replace(ruleset).await;

    let config = test_config(&db_path);
    let mut state = DaemonState::new(config.clone());
    let now = Utc::now();

    let conn_read = db::open(config.db_path()).expect("open read");
    state
        .process_event(
            &conn_read,
            &cache,
            WindowInfo {
                app_id: Some("app-alpha".to_string()),
                instance: None,
                class: None,
                title: "a".to_string(),
                workspace: None,
                output: None,
            },
            now,
        )
        .await
        .expect("first");

    let conn_read = db::open(config.db_path()).expect("open read");
    state
        .process_event(
            &conn_read,
            &cache,
            WindowInfo {
                app_id: Some("app-beta".to_string()),
                instance: None,
                class: None,
                title: "b".to_string(),
                workspace: None,
                output: None,
            },
            now + Duration::seconds(2),
        )
        .await
        .expect("second");

    let conn_check = db::open(config.db_path()).expect("open check");
    let active = db::get_active_tracking(&conn_check)
        .expect("active")
        .expect("row");
    assert_eq!(active.project_name, "Alpha");

    let conn_read = db::open(config.db_path()).expect("open read");
    state
        .process_event(
            &conn_read,
            &cache,
            WindowInfo {
                app_id: Some("app-beta".to_string()),
                instance: None,
                class: None,
                title: "b".to_string(),
                workspace: None,
                output: None,
            },
            now + Duration::seconds(7),
        )
        .await
        .expect("third");

    let conn_check = db::open(config.db_path()).expect("open check");
    let active = db::get_active_tracking(&conn_check)
        .expect("active")
        .expect("row");
    assert_eq!(active.project_name, "Beta");
}
