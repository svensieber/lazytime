use chrono::Utc;
use lazytime::db;
use lazytime::ipc::{client, server};
use lazytime::platform::types::WindowEventInfo;
use lazytime::rules::{self, RuleCache};
use lazytime::time;
use std::path::{Path, PathBuf};
use tempfile::tempdir;
use tokio::sync::mpsc;

fn ipc_endpoint(_dir: &Path) -> PathBuf {
    #[cfg(feature = "ipc-tcp")]
    {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind free tcp port");
        let addr = listener.local_addr().expect("local addr");
        drop(listener);
        return PathBuf::from(addr.to_string());
    }

    #[cfg(all(not(feature = "ipc-tcp"), feature = "ipc-unix", target_family = "unix"))]
    {
        return _dir.join("lazytime.sock");
    }

    #[allow(unreachable_code)]
    _dir.join("lazytime.sock")
}

#[tokio::test]
async fn ipc_projects_updated_roundtrip() {
    let dir = tempdir().expect("tempdir");
    let socket_path = ipc_endpoint(dir.path());

    let (tx, mut rx) = mpsc::channel::<String>(8);
    let socket_for_server = socket_path.clone();
    let server_task = tokio::spawn(async move {
        let _ = server::run_ipc_server(&socket_for_server, tx).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;

    let ts = time::format_ts(&Utc::now());
    client::notify_projects_updated(&socket_path, &ts)
        .await
        .expect("notify");

    let received = tokio::time::timeout(tokio::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("timeout")
        .expect("recv");
    assert_eq!(received, ts);

    server_task.abort();
}

#[tokio::test]
async fn ipc_reload_signal_swaps_rule_cache_for_detection() {
    let dir = tempdir().expect("tempdir");
    let socket_path = ipc_endpoint(dir.path());
    let db_path = dir.path().join("rules.sqlite");

    let mut conn = db::open(&db_path).expect("open db");
    db::migrate(&conn).expect("migrate");
    db::replace_rules(&mut conn, "Alpha", Some("CP1"), &[("app-a", None, ".*", 0)]).expect("alpha");

    let cache = RuleCache::default();
    let initial = rules::load_rules(&conn).expect("initial rules");
    cache.replace(initial).await;

    let (tx, mut rx) = mpsc::channel::<String>(8);
    let socket_for_server = socket_path.clone();
    let server_task = tokio::spawn(async move {
        let _ = server::run_ipc_server(&socket_for_server, tx).await;
    });

    let cache_for_reload = cache.clone();
    let db_path_for_reload = db_path.clone();
    let reload_task = tokio::spawn(async move {
        if rx.recv().await.is_some() {
            let conn = db::open(&db_path_for_reload).expect("reload open");
            let loaded = rules::load_rules(&conn).expect("reload rules");
            cache_for_reload.replace(loaded).await;
        }
    });

    let before = cache.get().await.detect_project(&WindowEventInfo {
        app_id: Some("app-b".to_string()),
        instance: None,
        class: None,
        title: "x".to_string(),
    });
    assert!(before.is_none());

    db::replace_rules(&mut conn, "Beta", Some("CP2"), &[("app-b", None, ".*", 0)]).expect("beta");

    tokio::time::sleep(tokio::time::Duration::from_millis(150)).await;
    client::notify_projects_updated(&socket_path, &time::format_ts(&Utc::now()))
        .await
        .expect("notify");

    tokio::time::timeout(tokio::time::Duration::from_secs(2), reload_task)
        .await
        .expect("reload timeout")
        .expect("reload join");

    let after = cache.get().await.detect_project(&WindowEventInfo {
        app_id: Some("app-b".to_string()),
        instance: None,
        class: None,
        title: "x".to_string(),
    });
    assert_eq!(after.as_deref(), Some("Beta"));

    server_task.abort();
}
