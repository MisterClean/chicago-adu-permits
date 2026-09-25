use adu_bot::{
    config::Config,
    normalize::Observation,
    permits::{self, Permit, PermitMetadata, PermitSource},
    publish::{self, DeliveryError, DeliveryIdentity, Publisher, Receipt, Reconciliation},
    render,
    store::{Store, chicago_date, now},
};
use anyhow::Result;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use tempfile::TempDir;

struct FixtureSource {
    rows: Vec<Value>,
    reported: Option<i64>,
}
impl PermitSource for FixtureSource {
    fn metadata(&mut self) -> Result<PermitMetadata> {
        Ok(PermitMetadata {
            revision: "1".into(),
            schema: vec![],
        })
    }
    fn count(&mut self) -> Result<i64> {
        Ok(self.reported.unwrap_or(self.rows.len() as i64))
    }
    fn page(&mut self, after: Option<&str>, limit: usize) -> Result<Vec<Value>> {
        Ok(self
            .rows
            .iter()
            .filter(|r| after.is_none_or(|a| r["id"].as_str().is_some_and(|id| id > a)))
            .take(limit)
            .cloned()
            .collect())
    }
}
fn date(days: i64) -> String {
    (chicago_date(now()).unwrap() + Duration::days(days))
        .format("%Y-%m-%dT00:00:00.000")
        .to_string()
}
fn application(id: &str, address: &str) -> Observation {
    Observation::parse(json!({"id":id,"status":"Pre-Certified","address":address,"street_number":"816","street_direction":"W","street_name":"AGATITE AVE","ward":"46","adu_applying_for":"2","coach_house":false,"conversion_unit":true,"submission_date":date(-40),"action_date":date(-10)})).unwrap()
}
fn permit(id: &str, number: &str, scope: &str) -> Value {
    json!({"id":id,"permit_":number,"permit_status":"ACTIVE","permit_type":"PERMIT - RENOVATION/ALTERATION","application_start_date":date(-5),"issue_date":date(0),"street_number":"816","street_direction":"W","street_name":"AGATITE AVE","work_description":scope,"reported_cost":"1200000","ward":"46","latitude":"41.9628","longitude":"-87.6506"})
}
fn setup() -> (TempDir, Store, Config) {
    let dir = TempDir::new().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let mut config = Config {
        state_dir: dir.path().into(),
        publish_enabled: true,
        page_size: 2,
        ..Config::default()
    };
    config.bluesky.did = Some("did:plc:test".into());
    let run = store.begin_run().unwrap();
    store
        .stage(run, &[application("914381", "816 W AGATITE AVE")])
        .unwrap();
    store
        .promote_for_account(run, now(), config.bluesky.did.as_deref())
        .unwrap();
    (dir, store, config)
}
fn scan(store: &mut Store, config: &Config, rows: Vec<Value>) -> Result<()> {
    permits::ingest(
        store,
        &mut FixtureSource {
            rows,
            reported: None,
        },
        config,
    )
}
#[test]
fn ward_snapshot_maps_each_preapproval_site_with_its_confirmed_permit_count() {
    let (_dir, mut store, config) = setup();
    let other = |id: &str, number: &str| {
        Observation::parse(json!({"id":id,"status":"Pre-Certified","address":format!("{number} W AGATITE AVE"),"street_number":number,"street_direction":"W","street_name":"AGATITE AVE","ward":"46","adu_applying_for":"2","coach_house":false,"conversion_unit":true,"submission_date":date(-40),"action_date":date(-10)})).unwrap()
    };
    let run = store.begin_run().unwrap();
    store
        .stage(
            run,
            &[
                application("914381", "816 W AGATITE AVE"),
                other("914382", "820"),
                other("914383", "824"),
            ],
        )
        .unwrap();
    store
        .promote_for_account(run, now(), config.bluesky.did.as_deref())
        .unwrap();
    let at = |id: &str, number: &str, app_id: &str, street: &str, longitude: Option<&str>| {
        let mut row = permit(id, number, &format!("ADU ID {app_id} CONVERSION UNIT"));
        row["street_number"] = json!(street);
        row["longitude"] = json!(longitude);
        row
    };
    let first = at("1", "101083804", "914381", "816", Some("-87.6506"));
    let second = at("2", "101083805", "914382", "820", Some("-87.6507"));
    let unmapped = at("3", "101083806", "914383", "824", None);
    scan(
        &mut store,
        &config,
        vec![first.clone(), second.clone(), unmapped.clone()],
    )
    .unwrap();
    let fourth = at("4", "101083807", "914381", "816", Some("-87.6506"));
    let mut proposed = at("5", "101083808", "914382", "820", Some("-87.6507"));
    proposed["work_description"] = json!("ONE NEW DWELLING UNIT");
    scan(
        &mut store,
        &config,
        vec![first, second, unmapped, fourth, proposed],
    )
    .unwrap();
    let snapshot = permits::ward_snapshot(&store, "914381", "101083807").unwrap();
    assert_eq!(
        (snapshot.ward, snapshot.permits, snapshot.sites),
        (46, 4, 3)
    );
    assert_eq!((snapshot.city_permits, snapshot.mapped_sites), (4, 2));
    assert_eq!(snapshot.focus.quantity, 2);
    assert_eq!(snapshot.points.len(), 2);
    assert_eq!(snapshot.rank, 1);
    assert!(!snapshot.tied);
    let reply = render::permit_reply_record(
        &snapshot,
        "at://did:plc:test/app.bsky.feed.post/root",
        "cid",
        Utc::now(),
    )
    .unwrap();
    let text = reply["text"].as_str().unwrap();
    assert!(text.contains("4 issued ADU building permits linked to 3 preapproved sites"));
    assert!(text.contains("2/3 sites mapped"));
    let map = json!({"alt":"Approximate linked permit locations","image":{"$type":"blob","mimeType":"image/jpeg","size":1000,"ref":{"$link":"bafyreitest"}},"aspectRatio":{"width":2160,"height":2160}});
    let mut with_maps = reply.clone();
    with_maps["embed"] = json!({"$type":"app.bsky.embed.images","images":[map.clone(),map]});
    assert!(render::validate(&with_maps).is_ok());
    with_maps.as_object_mut().unwrap().remove("reply");
    assert!(render::validate(&with_maps).is_err());
    store
        .db
        .execute(
            "INSERT INTO permit_matches(application_id,source_id,method,score,status,reason,first_seen,last_seen) VALUES('914381','2','review',100,'confirmed','conflicting test link',1,1)",
            [],
        )
        .unwrap();
    let unambiguous = permits::ward_snapshot(&store, "914381", "101083807").unwrap();
    assert_eq!(
        (
            unambiguous.permits,
            unambiguous.sites,
            unambiguous.mapped_sites
        ),
        (3, 2, 1)
    );
}
#[test]
fn permit_baseline_and_new_pair_are_distinct_and_replay_is_inert() {
    let (_dir, mut store, config) = setup();
    let first = permit("1", "101083804", "ADU ID 914381 ADD TWO DWELLING UNITS");
    scan(&mut store, &config, vec![first.clone()]).unwrap();
    let second = permit("2", "101083805", "CONVERSION ADU IN EXISTING BUILDING");
    scan(&mut store, &config, vec![first.clone(), second.clone()]).unwrap();
    scan(&mut store, &config, vec![first, second]).unwrap();
    let suppressed: String = store
        .db
        .query_row(
            "SELECT disposition FROM events WHERE event_key=?1",
            [permits::event_key("914381", "101083804")],
            |r| r.get(0),
        )
        .unwrap();
    let pending: String = store
        .db
        .query_row(
            "SELECT disposition FROM events WHERE event_key=?1",
            [permits::event_key("914381", "101083805")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        (suppressed.as_str(), pending.as_str()),
        ("suppressed", "pending")
    );
    assert_eq!(
        store
            .db
            .query_row("SELECT count(*) FROM permit_event_evidence", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}
#[test]
fn weak_address_candidate_requires_review_and_incomplete_scan_keeps_baseline() {
    let (_dir, mut store, config) = setup();
    let first = permit("1", "101083804", "ADU ID 914381");
    scan(&mut store, &config, vec![first.clone()]).unwrap();
    let second = permit("2", "101083805", "NEW DWELLING UNIT IN BASEMENT");
    scan(&mut store, &config, vec![first.clone(), second.clone()]).unwrap();
    let status: String = store
        .db
        .query_row(
            "SELECT status FROM permit_matches WHERE source_id='2'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(status, "proposed");
    assert!(
        store
            .db
            .query_row(
                "SELECT event_key FROM events WHERE event_key=?1",
                [permits::event_key("914381", "101083805")],
                |r| r.get::<_, String>(0)
            )
            .is_err()
    );
    permits::review_match(
        &mut store,
        "914381",
        "101083805",
        "confirm",
        "description verifies the conversion ADU",
        config.bluesky.did.as_deref(),
    )
    .unwrap();
    let state: String = store
        .db
        .query_row(
            "SELECT state FROM deliveries WHERE event_key=?1",
            [permits::event_key("914381", "101083805")],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "pending");
    assert!(
        permits::ingest(
            &mut store,
            &mut FixtureSource {
                rows: vec![first, second],
                reported: Some(3)
            },
            &config
        )
        .is_err()
    );
    assert_eq!(
        store
            .db
            .query_row("SELECT count(*) FROM permits WHERE present=1", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}
#[test]
fn permit_copy_has_three_supported_durations_and_source_links() {
    let app = application("914381", "816 W AGATITE AVE");
    let raw = permit("1", "101083804", "ADU ID 914381");
    let record = render::permit_record(&app, &Permit::parse(raw).unwrap(), Utc::now()).unwrap();
    let text = record["text"].as_str().unwrap();
    assert!(text.contains("Preapproval → permit application: 5 days"));
    assert!(text.contains("Permit application → issued: 5 days"));
    assert!(text.contains("Preapproval → building permit: 10 days"));
    assert!(text.contains("Reported project cost: $1,200,000"));
    assert_eq!(record["facets"].as_array().unwrap().len(), 2);
}
#[test]
fn matching_rejects_other_permit_types_and_conflicting_adu_ids() {
    let app = application("914381", "816 W AGATITE AVE");
    let mut wrong_type = permit("1", "101083804", "ADU ID 914381");
    wrong_type["permit_type"] = json!("PERMIT - SIGNS");
    assert!(
        permits::candidates(
            &Permit::parse(wrong_type).unwrap(),
            std::slice::from_ref(&app)
        )
        .is_empty()
    );
    let wrong_id = permit("2", "101083805", "ADU ID 999999 IN EXISTING BUILDING");
    assert!(permits::candidates(&Permit::parse(wrong_id).unwrap(), &[app]).is_empty());
}
struct FakePublisher {
    sent: Vec<Value>,
    clock: i64,
    fail_reply: bool,
}
impl Publisher for FakePublisher {
    fn platform(&self) -> &'static str {
        "bluesky"
    }
    fn account(&self) -> &str {
        "did:plc:test"
    }
    fn prepare(&mut self, _: &Observation) -> Result<Value> {
        unreachable!()
    }
    fn prepare_permit(&mut self, app: &Observation, permit: &Permit) -> Result<Value> {
        render::permit_record(app, permit, Utc::now())
    }
    fn prepare_permit_reply(
        &mut self,
        _: &Observation,
        _: &Permit,
        summary: &permits::PermitWardSnapshot,
        uri: &str,
        cid: &str,
    ) -> Result<Value> {
        if self.fail_reply {
            anyhow::bail!("map service unavailable");
        }
        render::permit_reply_record(summary, uri, cid, Utc::now())
    }
    fn allocate_identity(&self, _: i64) -> Result<DeliveryIdentity> {
        Ok(DeliveryIdentity {
            key: format!("key{}", self.clock + 1),
            clock: self.clock + 1,
        })
    }
    fn template_version(&self) -> i64 {
        1
    }
    fn reconcile(
        &mut self,
        _: &str,
        _: &Value,
    ) -> std::result::Result<Reconciliation, DeliveryError> {
        Ok(Reconciliation::Absent)
    }
    fn send(&mut self, key: &str, record: &Value) -> std::result::Result<Receipt, DeliveryError> {
        self.sent.push(record.clone());
        self.clock += 1;
        Ok(Receipt {
            uri: format!("at://did:plc:test/app.bsky.feed.post/{key}"),
            cid: format!("cid{}", self.clock),
        })
    }
}
#[test]
fn root_and_map_reply_have_independent_receipts_and_correct_thread_refs() {
    let (_dir, mut store, config) = setup();
    let first = permit("1", "101083804", "ADU ID 914381");
    scan(&mut store, &config, vec![first.clone()]).unwrap();
    scan(
        &mut store,
        &config,
        vec![first, permit("2", "101083805", "ADU ID 914381")],
    )
    .unwrap();
    let mut publisher = FakePublisher {
        sent: vec![],
        clock: 0,
        fail_reply: false,
    };
    publish::publish(&mut store, &config, &mut publisher).unwrap();
    assert_eq!(publisher.sent.len(), 1);
    store
        .db
        .execute("UPDATE adapter_state SET last_send=0", [])
        .unwrap();
    publish::publish(&mut store, &config, &mut publisher).unwrap();
    assert_eq!(publisher.sent.len(), 2);
    let reply = &publisher.sent[1];
    assert_eq!(
        reply["reply"]["root"]["uri"],
        "at://did:plc:test/app.bsky.feed.post/key1"
    );
    assert_eq!(reply["reply"]["parent"]["cid"], "cid1");
    assert_eq!(store.db.query_row("SELECT count(*) FROM deliveries WHERE state='sent' AND event_key LIKE '%building-permit%'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    assert_eq!(
        store
            .db
            .query_row(
                "SELECT count(*) FROM permit_replies WHERE state='sent'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn map_preparation_failure_does_not_resend_successful_root() {
    let (_dir, mut store, config) = setup();
    let first = permit("1", "101083804", "ADU ID 914381");
    scan(&mut store, &config, vec![first.clone()]).unwrap();
    scan(
        &mut store,
        &config,
        vec![first, permit("2", "101083805", "ADU ID 914381")],
    )
    .unwrap();
    let mut publisher = FakePublisher {
        sent: vec![],
        clock: 0,
        fail_reply: true,
    };
    assert!(publish::publish(&mut store, &config, &mut publisher).is_err());
    assert_eq!(publisher.sent.len(), 1);
    assert_eq!(
        store
            .db
            .query_row(
                "SELECT count(*) FROM permit_replies WHERE state='retry'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    publisher.fail_reply = false;
    store
        .db
        .execute("UPDATE permit_replies SET next_attempt=0", [])
        .unwrap();
    store
        .db
        .execute("UPDATE adapter_state SET last_send=0", [])
        .unwrap();
    publish::publish(&mut store, &config, &mut publisher).unwrap();
    assert_eq!(publisher.sent.len(), 2);
}
