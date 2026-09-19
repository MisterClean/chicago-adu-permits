//! Kill the actual promotion path while its SQLite transaction is open.
use adu_bot::{
    normalize::Observation,
    store::{Store, now},
};
use serde_json::json;
use std::{
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;

#[test]
fn crash_child() {
    let Ok(path) = std::env::var("ADU_TEST_CRASH_DIR") else {
        return;
    };
    let mut store = Store::open(std::path::Path::new(&path)).unwrap();
    let run = store.begin_run().unwrap();
    store
        .stage(
            run,
            &[Observation::parse(json!({"id":"1","status":"Pre-Certified"})).unwrap()],
        )
        .unwrap();
    if std::env::var("ADU_TEST_CRASH_PHASE").unwrap() == "fetch" {
        std::fs::write(format!("{path}/ready"), "fetch").unwrap();
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
    store
        .db
        .create_scalar_function(
            "promotion_boundary",
            0,
            rusqlite::functions::FunctionFlags::SQLITE_UTF8,
            move |_| -> rusqlite::Result<i32> {
                std::fs::write(format!("{path}/ready"), "promote").unwrap();
                loop {
                    thread::sleep(Duration::from_secs(1));
                }
            },
        )
        .unwrap();
    store.db.execute_batch("CREATE TRIGGER crash_boundary BEFORE INSERT ON events BEGIN SELECT promotion_boundary(); END;").unwrap();
    store.promote(run, now()).unwrap();
}
#[test]
fn killed_fetch_and_promotion_leave_no_partial_baseline() {
    for phase in ["fetch", "promote"] {
        let dir = TempDir::new().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_child", "--nocapture"])
            .env("ADU_TEST_CRASH_DIR", dir.path())
            .env("ADU_TEST_CRASH_PHASE", phase)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let start = Instant::now();
        while !dir.path().join("ready").exists() {
            if start.elapsed() > Duration::from_secs(10) {
                child.kill().unwrap();
                panic!("child did not reach {phase} boundary");
            }
            thread::sleep(Duration::from_millis(10));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        let store = Store::open(dir.path()).unwrap();
        for table in [
            "applications",
            "application_versions",
            "events",
            "source_state",
        ] {
            assert_eq!(
                store
                    .db
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        assert_eq!(
            store
                .db
                .query_row("SELECT status FROM ingest_runs", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "failed"
        );
    }
}
#[test]
fn sqlite_full_during_staging_keeps_last_successful_state() {
    let dir = TempDir::new().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let first = Observation::parse(json!({"id":"1","status":"Submitted"})).unwrap();
    let run = store.begin_run().unwrap();
    store.stage(run, &[first]).unwrap();
    store.promote(run, now()).unwrap();
    let page_count: i64 = store
        .db
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    store
        .db
        .execute_batch(&format!("PRAGMA max_page_count={page_count}"))
        .unwrap();
    let run = store.begin_run().unwrap();
    let huge =
        Observation::parse(json!({"id":"2","status":"Submitted","address":"x".repeat(100_000)}))
            .unwrap();
    assert!(store.stage(run, &[huge]).is_err());
    assert_eq!(
        store
            .db
            .query_row("SELECT count(*) FROM applications", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .db
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}
