use adu_bot::{
    normalize::Observation,
    store::{Store, now},
};
use fs2::FileExt;
use std::{
    fs,
    process::{Command, Stdio},
    time::Duration,
};

#[test]
fn check_is_read_only_and_production_never_recreates_missing_state() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let mut store = Store::open(&state).unwrap();
    let run = store.begin_run().unwrap();
    store
        .stage(
            run,
            &[Observation::parse(serde_json::json!({"id":"1","status":"Submitted"})).unwrap()],
        )
        .unwrap();
    store.promote(run, now()).unwrap();
    drop(store);
    let config = dir.path().join("config.toml");
    fs::write(
        &config,
        format!("state_dir = {:?}\nrequire_existing_state = true\n", state),
    )
    .unwrap();
    let before = fs::read(state.join("adu.sqlite3")).unwrap();
    for action in ["check", "migrate", "check"] {
        assert!(
            Command::new(env!("CARGO_BIN_EXE_adu-bot"))
                .arg("--config")
                .arg(&config)
                .arg(action)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(fs::read(state.join("adu.sqlite3")).unwrap(), before);
    }
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state.join("deployment.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_adu-bot"))
        .arg("--config")
        .arg(&config)
        .arg("status")
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(child.try_wait().unwrap().is_none());
    FileExt::unlock(&lock).unwrap();
    assert!(child.wait().unwrap().success());
    fs::write(state.join("deployment-blocked"), "test").unwrap();
    assert!(
        !Command::new(env!("CARGO_BIN_EXE_adu-bot"))
            .arg("--config")
            .arg(&config)
            .arg("run")
            .status()
            .unwrap()
            .success()
    );
    fs::remove_file(state.join("adu.sqlite3")).unwrap();
    assert!(
        !Command::new(env!("CARGO_BIN_EXE_adu-bot"))
            .arg("--config")
            .arg(&config)
            .arg("run")
            .status()
            .unwrap()
            .success()
    );
    assert!(!state.join("adu.sqlite3").exists());
}
