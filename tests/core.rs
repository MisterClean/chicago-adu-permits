use adu_bot::{
    config::{Config, DATASET},
    events,
    normalize::{Observation, Status},
    publish::{self, DeliveryError, Publisher, Receipt, Reconciliation},
    queue, render,
    source::{self, Metadata, Source},
    store::{Store, chicago_date, now},
};
use anyhow::Result;
use chrono::Utc;
use serde_json::{Value, json};
use tempfile::TempDir;

fn row(id: i64, status: &str) -> Value {
    json!({"id":id.to_string(),"status":status,"address":"100 W EXAMPLE ST","ward":"1","adu_applying_for":"1","coach_house":true,"conversion_unit":false,"action_date":format!("{}T00:00:00.000",chicago_date(now()).unwrap())})
}
fn obs(id: i64, status: &str) -> Observation {
    Observation::parse(row(id, status)).unwrap()
}
fn db() -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
}
fn promote(store: &mut Store, rows: &[Observation]) {
    let run = store.begin_run().unwrap();
    store.stage(run, rows).unwrap();
    store.promote(run, now()).unwrap();
}
fn count(store: &Store, table: &str) -> i64 {
    store
        .db
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}
fn disposition(store: &Store, id: i64) -> String {
    store
        .db
        .query_row(
            "SELECT disposition FROM events WHERE event_key=?1",
            [events::key(&id.to_string())],
            |r| r.get(0),
        )
        .unwrap()
}
#[test]
fn baseline_suppresses_both_labels_and_replay_is_inert() {
    let (_dir, mut s) = db();
    let rows = [
        obs(1, "Pre-Certified"),
        obs(2, "Pre-Certified: admin adjust"),
    ];
    promote(&mut s, &rows);
    promote(&mut s, &rows);
    assert_eq!(count(&s, "application_versions"), 2);
    assert_eq!(count(&s, "events"), 2);
    assert_eq!(disposition(&s, 1), "suppressed");
    assert_eq!(disposition(&s, 2), "suppressed");
}
#[test]
fn transitions_create_one_event_even_after_reversal() {
    let (_dir, mut s) = db();
    for status in ["Submitted", "Pre-Certified", "Denied", "Pre-Certified"] {
        promote(&mut s, &[obs(1, status)]);
    }
    assert_eq!(count(&s, "application_versions"), 4);
    assert_eq!(count(&s, "events"), 1);
    assert_eq!(disposition(&s, 1), "held");
}
#[test]
fn absence_and_reappearance_append_versions() {
    let (_dir, mut s) = db();
    promote(&mut s, &[obs(1, "Submitted")]);
    promote(&mut s, &[]);
    promote(&mut s, &[obs(1, "Submitted")]);
    assert_eq!(count(&s, "application_versions"), 3);
    let presence: Vec<bool> =
        s.db.prepare("SELECT present FROM application_versions ORDER BY version")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
    assert_eq!(presence, vec![true, false, true]);
}
#[test]
fn decisions_cover_adjustment_backfill_unknown_and_missing_date() {
    let today = chicago_date(now()).unwrap();
    for (status, previous, date, expected) in [
        ("Pre-Certified", Some(Status::InProcess), None, "pending"),
        ("Pre-Certified", None, None, "held"),
        ("Pre-Certified", Some(Status::Unknown), None, "held"),
        (
            "Pre-Certified: admin adjust",
            Some(Status::InProcess),
            None,
            "held",
        ),
        (
            "Pre-Certified",
            None,
            Some("2000-01-01T00:00:00.000"),
            "held",
        ),
        (
            "Pre-Certified",
            Some(Status::InProcess),
            Some("bad"),
            "held",
        ),
        (
            "Pre-Certified",
            Some(Status::InProcess),
            Some("2099-01-01T00:00:00.000"),
            "held",
        ),
    ] {
        let mut v = row(1, status);
        v["action_date"] = json!(date);
        let o = Observation::parse(v).unwrap();
        assert_eq!(
            events::decide(&o, previous, Some(today), today)
                .unwrap()
                .disposition,
            expected
        );
    }
}
#[test]
fn exact_status_mapping_rejects_prefixes() {
    assert_eq!(Status::classify(" Pre-Certified "), Status::Preapproved);
    assert_eq!(
        Status::classify("Pre-Certified: something else"),
        Status::Unknown
    );
    assert_eq!(Status::classify(""), Status::Unknown);
}
#[test]
fn normalization_is_exact_and_personal_fields_are_dropped() {
    let a = Observation::parse(json!({"id":"000123.00","ward":1,"applicant_first_name":"PRIVATE"}))
        .unwrap();
    let b = Observation::parse(json!({"id":123,"ward":"01.0","status":null})).unwrap();
    assert_eq!(a.hash, b.hash);
    assert_eq!(a.id, "123");
    assert!(!a.raw.to_string().contains("PRIVATE"));
    assert!(Observation::parse(json!({"id":"9007199254740993"})).is_ok());
    assert!(Observation::parse(json!({"id":"1.5"})).is_err());
    assert!(Observation::parse(json!({"id":null})).is_err());
}
#[test]
fn rendering_handles_unicode_both_flags_zero_units_and_long_addresses() {
    for address in ["100 ÉCOLE 🏘️ ST".into(), "🦀".repeat(1000)] {
        let mut v = row(1, "Pre-Certified");
        v["address"] = json!(address);
        v["conversion_unit"] = json!(true);
        v["adu_applying_for"] = json!(0);
        let record = render::record(&Observation::parse(v).unwrap(), Utc::now()).unwrap();
        render::validate(&record).unwrap();
        let text = record["text"].as_str().unwrap();
        assert!(!text.contains("Requested: 0"));
        assert_eq!(
            text,
            "New ADU preapproved\n\nType: Coach house + conversion.\n\nData Portal Record"
        );
        let facet = &record["facets"][0]["index"];
        assert_eq!(
            &text[facet["byteStart"].as_u64().unwrap() as usize
                ..facet["byteEnd"].as_u64().unwrap() as usize],
            "Data Portal Record"
        );
    }
}
#[test]
fn post_copy_describes_requested_units_types_and_calendar_days() {
    for (coach, conversion, count, details) in [
        (
            false,
            true,
            2,
            "2 ADUs proposed for the property.\nType: Conversion.",
        ),
        (
            true,
            false,
            1,
            "1 ADU proposed for the property.\nType: Coach house.",
        ),
        (
            true,
            true,
            3,
            "3 ADUs proposed for the property.\nType: Coach house + conversion.",
        ),
        (false, false, 2, "2 ADUs proposed for the property."),
    ] {
        let observation = Observation::parse(json!({
            "id": 1, "status": "Pre-Certified", "coach_house": coach,
            "conversion_unit": conversion, "adu_applying_for": count,
            "submission_date": "2024-02-28T23:59:00.000",
            "action_date": "2024-03-01T00:00:00.000"
        }))
        .unwrap();
        let record = render::record(&observation, Utc::now()).unwrap();
        render::validate(&record).unwrap();
        assert_eq!(
            record["text"],
            format!(
                "New ADU preapproved\n\n{details}\n\nPreapproved 2 days after submission.\n\nData Portal Record"
            )
        );
    }
}
#[test]
fn post_copy_omits_unknown_counts_and_types() {
    for count in [
        Value::Null,
        json!(0),
        json!(-1),
        json!("invalid"),
        json!("1.5"),
    ] {
        let observation = Observation::parse(json!({
            "id": 1, "status": "Pre-Certified", "adu_applying_for": count,
            "coach_house": "true", "conversion_unit": "true"
        }))
        .unwrap();
        let record = render::record(&observation, Utc::now()).unwrap();
        assert_eq!(record["text"], "New ADU preapproved\n\nData Portal Record");
    }
}
#[test]
fn preapproval_duration_handles_missing_invalid_reversed_and_adjustment_dates() {
    for (status, submitted, action, duration) in [
        (
            "Pre-Certified",
            "2026-01-01T00:00:00",
            "2026-01-01T00:00:00",
            Some("0 days"),
        ),
        (
            "Pre-Certified",
            "2026-01-01T00:00:00",
            "2026-01-02T00:00:00",
            Some("1 day"),
        ),
        (
            "Pre-Certified",
            "2026-01-02T00:00:00",
            "2026-01-01T00:00:00",
            None,
        ),
        ("Pre-Certified", "invalid", "2026-01-02T00:00:00", None),
        ("Pre-Certified", "2026-01-01T00:00:00", "invalid", None),
        (
            "Pre-Certified: admin adjust",
            "2026-01-01T00:00:00",
            "2026-01-02T00:00:00",
            None,
        ),
    ] {
        let observation = Observation::parse(json!({
            "id": 1, "status": status, "submission_date": submitted, "action_date": action
        }))
        .unwrap();
        let record = render::record(&observation, Utc::now()).unwrap();
        let expected = duration.map_or_else(String::new, |d| {
            format!("\n\nPreapproved {d} after submission.")
        });
        assert_eq!(
            record["text"],
            format!("New ADU preapproved{expected}\n\nData Portal Record")
        );
    }
}
#[test]
fn submission_date_changes_are_post_relevant() {
    let mut original = row(1, "Pre-Certified");
    original["submission_date"] = json!("2026-01-01T00:00:00");
    let mut corrected = original.clone();
    corrected["submission_date"] = json!("2026-01-02T00:00:00");
    assert_ne!(
        Observation::parse(original).unwrap().post_facts(),
        Observation::parse(corrected).unwrap().post_facts()
    );
}
#[test]
fn baseline_unknown_and_unrelated_good_transition_are_independent() {
    let (_dir, mut s) = db();
    promote(&mut s, &[obs(1, "mystery"), obs(2, "Submitted")]);
    promote(&mut s, &[obs(1, "Pre-Certified"), obs(2, "Pre-Certified")]);
    assert_eq!(disposition(&s, 1), "held");
    assert_eq!(disposition(&s, 2), "pending");
}
#[test]
fn application_identity_does_not_deduplicate_shared_addresses() {
    let (_dir, mut s) = db();
    promote(&mut s, &[]);
    promote(&mut s, &[obs(1, "Pre-Certified"), obs(2, "Pre-Certified")]);
    assert_eq!(count(&s, "events"), 2);
}
#[test]
fn review_is_explicit_and_append_only() {
    let (_dir, mut s) = db();
    promote(&mut s, &[obs(1, "Pre-Certified")]);
    let key = events::key("1");
    queue::review(&mut s, &key, "approve", "verified historical backfill").unwrap();
    assert_eq!(disposition(&s, 1), "pending");
    assert_eq!(count(&s, "review_actions"), 1);
    assert!(s.db.execute("DELETE FROM review_actions", []).is_err());
    queue::review(&mut s, &key, "suppress", "do not publish").unwrap();
    assert_eq!(disposition(&s, 1), "suppressed");
}
#[test]
fn promotion_failure_rolls_back_baseline_and_all_current_state() {
    let (_dir, mut s) = db();
    let run = s.begin_run().unwrap();
    s.stage(run, &[obs(1, "Pre-Certified")]).unwrap();
    s.db.execute_batch("CREATE TRIGGER fail_event BEFORE INSERT ON events BEGIN SELECT RAISE(ABORT,'injected disk write failure'); END;").unwrap();
    assert!(s.promote(run, now()).is_err());
    assert_eq!(count(&s, "applications"), 0);
    assert_eq!(count(&s, "source_state"), 0);
    assert_eq!(count(&s, "application_versions"), 0);
}
#[test]
fn process_lock_excludes_second_writer_and_backup_restores_paused() {
    let (dir, mut s) = db();
    assert!(Store::open(dir.path()).is_err());
    promote(&mut s, &[obs(1, "Submitted")]);
    let backup = dir.path().join("backup.sqlite");
    s.backup(&backup).unwrap();
    let restored = rusqlite::Connection::open(backup).unwrap();
    assert!(
        restored
            .query_row("SELECT posting_paused FROM source_state", [], |r| r
                .get::<_, bool>(0))
            .unwrap()
    );
    assert_eq!(
        restored
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}
struct Fixture {
    rows: Vec<Value>,
    wrong_count: bool,
    revision_changed: bool,
    meta_calls: usize,
    broken: bool,
}
impl Fixture {
    fn new(rows: Vec<Value>) -> Self {
        Self {
            rows,
            wrong_count: false,
            revision_changed: false,
            meta_calls: 0,
            broken: false,
        }
    }
}
impl Source for Fixture {
    fn metadata(&mut self) -> Result<Metadata> {
        self.meta_calls += 1;
        Ok(Metadata {
            revision: if self.revision_changed {
                self.meta_calls.to_string()
            } else {
                "1".into()
            },
            schema: vec![],
        })
    }
    fn count(&mut self) -> Result<i64> {
        Ok(self.rows.len() as i64 + i64::from(self.wrong_count))
    }
    fn page(&mut self, after: Option<i64>, limit: usize) -> Result<Vec<Value>> {
        if self.broken {
            anyhow::bail!("truncated response");
        }
        Ok(self
            .rows
            .iter()
            .filter(|v| after.is_none_or(|a| v["id"].as_str().unwrap().parse::<i64>().unwrap() > a))
            .take(limit)
            .cloned()
            .collect())
    }
    fn requests(&self) -> usize {
        self.meta_calls + 2
    }
}
#[test]
fn incomplete_duplicate_disordered_or_changed_scans_never_establish_baseline() {
    let mut fixtures = vec![
        Fixture::new(vec![row(1, "Submitted"), row(1, "Submitted")]),
        Fixture::new(vec![row(2, "Submitted"), row(1, "Submitted")]),
    ];
    let mut count_mismatch = Fixture::new(vec![row(1, "Submitted")]);
    count_mismatch.wrong_count = true;
    fixtures.push(count_mismatch);
    let mut changed = Fixture::new(vec![row(1, "Submitted")]);
    changed.revision_changed = true;
    fixtures.push(changed);
    let mut broken = Fixture::new(vec![row(1, "Submitted")]);
    broken.broken = true;
    fixtures.push(broken);
    for mut fixture in fixtures {
        let (_dir, mut s) = db();
        assert!(source::ingest(&mut s, &mut fixture, &Config::default()).is_err());
        assert_eq!(count(&s, "applications"), 0);
        assert_eq!(count(&s, "source_state"), 0);
        assert_eq!(
            s.db.query_row("SELECT status FROM ingest_runs", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "failed"
        );
    }
}
#[test]
fn exact_page_multiple_completes_and_large_drop_rejected() {
    let (_dir, mut s) = db();
    let config = Config {
        page_size: 10,
        ..Config::default()
    };
    source::ingest(
        &mut s,
        &mut Fixture::new((1..=20).map(|id| row(id, "Submitted")).collect()),
        &config,
    )
    .unwrap();
    assert_eq!(count(&s, "applications"), 20);
    assert!(
        source::ingest(
            &mut s,
            &mut Fixture::new(vec![row(1, "Submitted")]),
            &config
        )
        .is_err()
    );
    assert!(source::ingest(&mut s, &mut Fixture::new(vec![]), &config).is_err());
    assert_eq!(count(&s, "applications"), 20);
}
struct FakePublisher {
    remote: Option<(String, Value)>,
    sends: usize,
    ambiguous: bool,
    conflict: bool,
    preparation_error: bool,
    prepared_units: Option<i64>,
}
impl FakePublisher {
    fn new() -> Self {
        Self {
            remote: None,
            sends: 0,
            ambiguous: false,
            conflict: false,
            preparation_error: false,
            prepared_units: None,
        }
    }
}
impl Publisher for FakePublisher {
    fn platform(&self) -> &'static str {
        "bluesky"
    }
    fn account(&self) -> &str {
        "did:plc:test"
    }
    fn allocate_identity(&self, previous_clock: i64) -> Result<publish::DeliveryIdentity> {
        publish::bluesky::next_identity(previous_clock)
    }
    fn template_version(&self) -> i64 {
        1
    }
    fn prepare(&mut self, o: &Observation) -> Result<Value> {
        if self.preparation_error {
            anyhow::bail!("Street View unavailable");
        }
        self.prepared_units = o.number("adu_applying_for");
        render::record(o, Utc::now())
    }
    fn reconcile(
        &mut self,
        key: &str,
        record: &Value,
    ) -> std::result::Result<Reconciliation, DeliveryError> {
        if self.conflict {
            return Ok(Reconciliation::Conflict);
        }
        match &self.remote {
            Some((k, v)) if k == key && v == record => Ok(Reconciliation::Same(Receipt {
                uri: format!("at://test/{key}"),
                cid: "cid".into(),
            })),
            Some(_) => Ok(Reconciliation::Conflict),
            None => Ok(Reconciliation::Absent),
        }
    }
    fn send(&mut self, key: &str, record: &Value) -> std::result::Result<Receipt, DeliveryError> {
        self.sends += 1;
        self.remote = Some((key.into(), record.clone()));
        if self.ambiguous {
            Err(DeliveryError::Retry {
                message: "accepted then timeout".into(),
                after: None,
            })
        } else {
            Ok(Receipt {
                uri: format!("at://test/{key}"),
                cid: "cid".into(),
            })
        }
    }
}
fn publishing_fixture() -> (TempDir, Store, Config) {
    let (dir, mut s) = db();
    promote(&mut s, &[obs(1, "Submitted")]);
    promote(&mut s, &[obs(1, "Pre-Certified")]);
    let mut c = Config {
        publish_enabled: true,
        state_dir: dir.path().into(),
        ..Config::default()
    };
    c.bluesky.did = Some("did:plc:test".into());
    (dir, s, c)
}
#[test]
fn new_approvals_publish_once_while_baseline_and_adjustments_do_not() {
    let (dir, mut s) = db();
    let mut config = Config {
        publish_enabled: true,
        state_dir: dir.path().into(),
        ..Config::default()
    };
    config.bluesky.did = Some("did:plc:test".into());
    source::ingest(
        &mut s,
        &mut Fixture::new(vec![row(1, "Pre-Certified"), row(2, "Submitted")]),
        &config,
    )
    .unwrap();
    let changed = vec![
        row(1, "Pre-Certified"),
        row(2, "Pre-Certified"),
        row(3, "Pre-Certified"),
        row(4, "Pre-Certified: admin adjust"),
    ];
    source::ingest(&mut s, &mut Fixture::new(changed.clone()), &config).unwrap();
    let mut publisher = FakePublisher::new();
    publish::publish(&mut s, &config, &mut publisher).unwrap();
    s.db.execute("UPDATE adapter_state SET last_send=0", [])
        .unwrap();
    publish::publish(&mut s, &config, &mut publisher).unwrap();
    assert_eq!(publisher.sends, 2);
    assert_eq!(disposition(&s, 1), "suppressed");
    assert_eq!(disposition(&s, 4), "held");
    drop(s);
    let mut s = Store::open(dir.path()).unwrap();
    source::ingest(&mut s, &mut Fixture::new(changed), &config).unwrap();
    publish::publish(&mut s, &config, &mut publisher).unwrap();
    assert_eq!(publisher.sends, 2);
    assert_eq!(
        s.db.query_row(
            "SELECT count(*) FROM deliveries WHERE state='sent'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
}
#[test]
fn ambiguous_acceptance_survives_restart_and_reconciles_before_stale_gate() {
    let (dir, mut s, c) = publishing_fixture();
    let mut p = FakePublisher::new();
    p.ambiguous = true;
    assert!(publish::publish(&mut s, &c, &mut p).is_err());
    let frozen: (String, String) =
        s.db.query_row("SELECT record_key,record_json FROM deliveries", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(frozen.0.len(), 13);
    drop(s);
    let mut s = Store::open(dir.path()).unwrap();
    s.db.execute("UPDATE deliveries SET next_attempt=0", [])
        .unwrap();
    s.db.execute(
        "UPDATE ingest_runs SET ended_at=0 WHERE status='success'",
        [],
    )
    .unwrap();
    publish::publish(&mut s, &c, &mut p).unwrap();
    assert_eq!(p.sends, 1);
    assert_eq!(
        s.db.query_row("SELECT state FROM deliveries", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "sent"
    );
    let same: (String, String) =
        s.db.query_row("SELECT record_key,record_json FROM deliveries", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(frozen, same);
}
#[test]
fn remote_conflict_never_overwritten() {
    let (_dir, mut s, c) = publishing_fixture();
    let mut p = FakePublisher::new();
    p.ambiguous = true;
    assert!(publish::publish(&mut s, &c, &mut p).is_err());
    p.conflict = true;
    s.db.execute("UPDATE deliveries SET next_attempt=0", [])
        .unwrap();
    assert!(publish::publish(&mut s, &c, &mut p).is_err());
    assert_eq!(p.sends, 1);
    assert_eq!(
        s.db.query_row("SELECT state FROM deliveries", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "held"
    );
}
#[test]
fn disabled_and_dry_run_never_send() {
    let (_dir, mut s, mut c) = publishing_fixture();
    c.publish_enabled = false;
    let mut p = FakePublisher::new();
    assert!(publish::publish(&mut s, &c, &mut p).is_err());
    publish::dry_run(&s).unwrap();
    assert_eq!(p.sends, 0);
    assert_eq!(count(&s, "deliveries"), 0);
}
#[test]
fn correction_holds_first_send_and_review_uses_current_evidence() {
    let (_dir, mut s, c) = publishing_fixture();
    let mut p = FakePublisher::new();
    let mut corrected = row(1, "Pre-Certified");
    corrected["adu_applying_for"] = json!(2);
    promote(&mut s, &[Observation::parse(corrected).unwrap()]);
    publish::publish(&mut s, &c, &mut p).unwrap();
    assert_eq!(p.sends, 0);
    queue::review(
        &mut s,
        &events::key("1"),
        "approve",
        "confirmed revised request",
    )
    .unwrap();
    publish::publish(&mut s, &c, &mut p).unwrap();
    assert_eq!(p.sends, 1);
    assert_eq!(p.prepared_units, Some(2));
}
#[test]
fn foreign_keys_and_platform_account_uniqueness_are_enforced() {
    let (_dir, s, c) = publishing_fixture();
    s.sync_deliveries(&c).unwrap();
    s.sync_deliveries(&c).unwrap();
    assert_eq!(count(&s, "deliveries"), 1);
    assert!(s.db.execute("INSERT INTO deliveries(event_key,platform,account_id) VALUES('missing','other','did:plc:test')",[]).is_err());
    s.db.execute(
        "INSERT INTO deliveries(event_key,platform,account_id) VALUES(?1,'future','did:plc:test')",
        [events::key("1")],
    )
    .unwrap();
    assert_eq!(count(&s, "deliveries"), 2);
    assert_eq!(s.application("1").unwrap().id, "1");
    assert_eq!(DATASET, "j4h8-ug9m");
}

#[test]
fn identity_is_monotonic_even_when_clock_moves_backwards() {
    let first =
        publish::bluesky::next_identity(Utc::now().timestamp_micros() + 10_000_000).unwrap();
    let second = publish::bluesky::next_identity(first.clock).unwrap();
    assert!(second.key > first.key);
    assert_eq!(second.clock, first.clock + 1);
    assert!(
        second
            .key
            .bytes()
            .all(|b| b"234567abcdefghijklmnopqrstuvwxyz".contains(&b))
    );
}
#[test]
fn sent_work_is_not_counted_as_pending_queue_age_or_recreated() {
    let (_dir, mut s, c) = publishing_fixture();
    let mut p = FakePublisher::new();
    publish::publish(&mut s, &c, &mut p).unwrap();
    assert!(s.status().unwrap()["oldest_queue_age_seconds"].is_null());
    p.remote = None;
    publish::publish(&mut s, &c, &mut p).unwrap();
    assert_eq!(p.sends, 1);
}
#[test]
fn stale_source_holds_first_send_without_authentication() {
    let (_dir, mut s, c) = publishing_fixture();
    let mut p = FakePublisher::new();
    s.db.execute(
        "UPDATE ingest_runs SET ended_at=0 WHERE status='success'",
        [],
    )
    .unwrap();
    publish::publish(&mut s, &c, &mut p).unwrap();
    assert_eq!(p.sends, 0);
    assert_eq!(
        s.db.query_row("SELECT state FROM deliveries", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "held"
    );
}
#[test]
fn reviewed_suppression_reconciles_ambiguous_write_but_never_resends_absence() {
    let (_dir, mut s, c) = publishing_fixture();
    let mut p = FakePublisher::new();
    p.ambiguous = true;
    assert!(publish::publish(&mut s, &c, &mut p).is_err());
    queue::review(&mut s, &events::key("1"), "suppress", "operator canceled").unwrap();
    p.remote = None;
    s.db.execute("UPDATE deliveries SET next_attempt=0", [])
        .unwrap();
    publish::publish(&mut s, &c, &mut p).unwrap();
    assert_eq!(p.sends, 1);
}
#[test]
fn delivery_failure_rolls_back_promotion_and_baseline_together() {
    let (_dir, mut s) = db();
    let run = s.begin_run().unwrap();
    s.stage(run, &[obs(1, "Pre-Certified")]).unwrap();
    s.db.execute_batch("CREATE TRIGGER reject_delivery BEFORE INSERT ON deliveries BEGIN SELECT RAISE(ABORT,'injected write failure'); END;").unwrap();
    assert!(
        s.promote_for_account(run, now(), Some("did:plc:test"))
            .is_err()
    );
    assert_eq!(count(&s, "events"), 0);
    assert_eq!(count(&s, "source_state"), 0);
    assert_eq!(count(&s, "applications"), 0);
}

#[test]
fn corrupted_frozen_payload_fails_closed() {
    let (_dir, mut s, c) = publishing_fixture();
    let mut p = FakePublisher::new();
    p.ambiguous = true;
    assert!(publish::publish(&mut s, &c, &mut p).is_err());
    s.db.execute("UPDATE deliveries SET next_attempt=0,record_json=json_set(record_json,'$.text','unexpected change')",[]).unwrap();
    assert!(publish::publish(&mut s, &c, &mut p).is_err());
    assert_eq!(p.sends, 1);
}

#[test]
fn preparation_failure_defers_without_freezing_or_blocking_evidence_review() {
    let (_dir, mut s, c) = publishing_fixture();
    let mut p = FakePublisher::new();
    p.preparation_error = true;
    assert!(publish::publish(&mut s, &c, &mut p).is_err());
    assert_eq!(p.sends, 0);
    let (state, attempts, key, due): (String, i64, Option<String>, i64) =
        s.db.query_row(
            "SELECT state,attempts,record_key,next_attempt FROM deliveries",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(state, "retry");
    assert_eq!(attempts, 0);
    assert!(key.is_none());
    assert!(due > now());
    queue::review(
        &mut s,
        &events::key("1"),
        "approve",
        "Confirmed current evidence after image failure",
    )
    .unwrap();
    p.preparation_error = false;
    publish::publish(&mut s, &c, &mut p).unwrap();
    assert_eq!(p.sends, 1);
}
