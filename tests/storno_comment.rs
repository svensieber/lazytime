use lazytime::config::ThemePreference;
use lazytime::db;
use lazytime::tui::trackings_storno::storno_tracking;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

fn test_config(db_file: String, jira_url: String) -> lazytime::config::Config {
    lazytime::config::Config {
        onboarding_done: true,
        default_project: "Default".to_string(),
        tracking_stability_seconds: 60,
        working_hours: Default::default(),
        track_reminder_seconds: 300,
        track_reminder_snooze_seconds: 1800,
        summary_update_seconds: 5,
        report_start: None,
        report_end: None,
        db_file,
        jira_url: Some(jira_url),
        jira_token: Some("token".to_string()),
        jira_email: None,
        jira_project: Some("LT".to_string()),
        jira_assignee: None,
        jira_issue_type: "Story".to_string(),
        jira_sap_field: "sap_project".to_string(),
        ipc_socket_path: None,
        theme_preference: ThemePreference::Auto,
        sidebar_collapsed: false,
    }
}

#[test]
fn storno_reduces_time_and_removes_description_line_when_unused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.sqlite");
    let conn = db::open(&db_path).expect("open db");
    db::migrate(&conn).expect("migrate");

    db::add_manual_tracking(
        &conn,
        "Project A",
        "2026-05-05T10:00:00+00:00",
        Some("2026-05-05T10:30:00+00:00"),
        Some("Unique line"),
    )
    .expect("add tracking");
    let trackings = db::list_all_trackings(&conn).expect("list");
    let tracking = trackings.first().expect("tracking").clone();
    db::mark_synced(&conn, tracking.id, "LT-1", "w1").expect("mark synced");
    let tracking = db::list_all_trackings(&conn)
        .expect("list after mark")
        .into_iter()
        .find(|t| t.id == tracking.id)
        .expect("tracking after mark");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let bodies: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = bodies.clone();

    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = vec![0u8; 32768];
            let n = stream.read(&mut buf).expect("read");
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            if req.starts_with("GET /rest/api/3/issue/LT-1/worklog") {
                let body = json!({
                    "startAt": 0,
                    "maxResults": 100,
                    "total": 1,
                    "worklogs": [{
                        "id": "w1",
                        "started": "2026-05-05T10:00:00.000+0000",
                        "timeSpentSeconds": 7200,
                        "author": {"accountId": "a1"},
                        "comment": {
                            "type": "doc",
                            "version": 1,
                            "content": [
                                {"type":"paragraph","content":[{"type":"text","text":"Unique line"}]},
                                {"type":"paragraph","content":[{"type":"text","text":"Keep line"}]}
                            ]
                        }
                    }]
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            } else if req.starts_with("PUT /rest/api/3/issue/LT-1/worklog/w1") {
                let body = r#"{"id":"w1"}"#;
                let payload = req.split("\r\n\r\n").nth(1).unwrap_or_default().to_string();
                captured.lock().expect("lock").push(payload);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        }
    });

    let config = test_config(
        db_path.to_string_lossy().to_string(),
        format!("http://{}", addr),
    );
    let msg = storno_tracking(&conn, &config, &tracking).expect("storno");
    assert!(msg.contains("reduced Jira worklog"));

    server.join().expect("server join");
    let payloads = bodies.lock().expect("lock");
    let payload: serde_json::Value =
        serde_json::from_str(payloads.first().expect("put payload")).expect("json");
    assert_eq!(payload["timeSpentSeconds"], 5400);
    let comment_text = payload["comment"]["content"][0]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(comment_text, "Keep line");
}

#[test]
fn storno_keeps_description_line_when_other_synced_tracking_uses_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.sqlite");
    let conn = db::open(&db_path).expect("open db");
    db::migrate(&conn).expect("migrate");

    db::add_manual_tracking(
        &conn,
        "Project A",
        "2026-05-05T10:00:00+00:00",
        Some("2026-05-05T10:30:00+00:00"),
        Some("Shared line"),
    )
    .expect("add t1");
    db::add_manual_tracking(
        &conn,
        "Project A",
        "2026-05-05T11:00:00+00:00",
        Some("2026-05-05T11:30:00+00:00"),
        Some("Shared line"),
    )
    .expect("add t2");
    let mut trackings = db::list_all_trackings(&conn).expect("list");
    trackings.sort_by_key(|t| t.start_ts.clone());
    let first = trackings.first().expect("first").clone();
    let second = trackings.get(1).expect("second").clone();
    db::mark_synced(&conn, first.id, "LT-2", "w2").expect("mark first synced");
    db::mark_synced(&conn, second.id, "LT-2", "w2").expect("mark second synced");
    let first = db::list_all_trackings(&conn)
        .expect("list after mark")
        .into_iter()
        .find(|t| t.id == first.id)
        .expect("first after mark");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let bodies: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = bodies.clone();

    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = vec![0u8; 32768];
            let n = stream.read(&mut buf).expect("read");
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            if req.starts_with("GET /rest/api/3/issue/LT-2/worklog") {
                let body = json!({
                    "startAt": 0,
                    "maxResults": 100,
                    "total": 1,
                    "worklogs": [{
                        "id": "w2",
                        "started": "2026-05-05T10:00:00.000+0000",
                        "timeSpentSeconds": 7200,
                        "author": {"accountId": "a1"},
                        "comment": {
                            "type": "doc",
                            "version": 1,
                            "content": [
                                {"type":"paragraph","content":[{"type":"text","text":"Shared line"}]},
                                {"type":"paragraph","content":[{"type":"text","text":"Other"}]}
                            ]
                        }
                    }]
                })
                .to_string();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            } else if req.starts_with("PUT /rest/api/3/issue/LT-2/worklog/w2") {
                let body = r#"{"id":"w2"}"#;
                let payload = req.split("\r\n\r\n").nth(1).unwrap_or_default().to_string();
                captured.lock().expect("lock").push(payload);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        }
    });

    let config = test_config(
        db_path.to_string_lossy().to_string(),
        format!("http://{}", addr),
    );
    let msg = storno_tracking(&conn, &config, &first).expect("storno");
    assert!(msg.contains("reduced Jira worklog"));

    server.join().expect("server join");
    let payloads = bodies.lock().expect("lock");
    let payload: serde_json::Value =
        serde_json::from_str(payloads.first().expect("put payload")).expect("json");
    assert_eq!(payload["timeSpentSeconds"], 5400);
    let comment_text = payload["comment"]["content"][0]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert_eq!(comment_text, "Shared line\nOther");
}
