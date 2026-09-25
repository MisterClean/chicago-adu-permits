use adu_bot::{
    config::Config,
    normalize::hash,
    publish::{bluesky::Bluesky, scorecards},
    store::{Store, now},
};
use serde_json::{Value, json};
use std::{sync::Arc, thread, time::Duration};
use tempfile::TempDir;
use tiny_http::{Response, Server};

fn server() -> (Arc<Server>, String) {
    let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
    let url = format!("http://{}", server.server_addr());
    (server, url)
}
fn next(server: &Server) -> tiny_http::Request {
    server
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .expect("expected PDS request")
}
fn reply(request: tiny_http::Request, status: u16, value: Value) {
    request
        .respond(Response::from_string(value.to_string()).with_status_code(status))
        .unwrap();
}
struct Fixture {
    _dir: TempDir,
    store: Store,
    config: Config,
    root: Value,
    child: Value,
}
fn fixture(pds: &str) -> Fixture {
    let dir = TempDir::new().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let root = json!({"$type":"app.bsky.feed.post","text":"parent","createdAt":"2026-09-24T00:00:00.000Z","langs":["en"],"facets":[]});
    let reference =
        json!({"uri":"at://did:plc:test/app.bsky.feed.post/parentkey","cid":"parentcid"});
    let image = json!({"alt":"Ward scorecard map","image":{"$type":"blob","mimeType":"image/jpeg","size":1000,"ref":{"$link":"bafyreitest"}},"aspectRatio":{"width":2160,"height":2160}});
    let child = json!({"$type":"app.bsky.feed.post","text":"ward scorecard","createdAt":"2026-09-24T00:01:00.000Z","langs":["en"],"facets":[],"reply":{"root":reference,"parent":reference},"embed":{"$type":"app.bsky.embed.images","images":[image.clone(),image]}});
    store.db.execute_batch("INSERT INTO ingest_runs(id,started_at,ended_at,status) VALUES(1,1,1,'success');
        INSERT INTO applications VALUES('j4h8-ug9m','123',1,'{}','hash',1,1,1,1);
        INSERT INTO application_versions VALUES('j4h8-ug9m','123',1,1,'hash','{}',1,NULL,'[]');
        INSERT INTO source_state VALUES('j4h8-ug9m',1,'2026-09-24',1,1,0);
        INSERT INTO events(event_key,dataset_id,application_id,initial_evidence_version,evidence_version,detection_kind,observed_at,disposition,reason) VALUES('event','j4h8-ug9m','123',1,1,'new',1,'pending','test');
        INSERT INTO adapter_state(platform,account_id) VALUES('bluesky','did:plc:test');").unwrap();
    store.db.execute("INSERT INTO deliveries(id,event_key,platform,account_id,state,record_key,record_json,record_hash,remote_uri,remote_cid,sent_at) VALUES(1,'event','bluesky','did:plc:test','sent','parentkey',?1,?2,'at://did:plc:test/app.bsky.feed.post/parentkey','parentcid',?3)",rusqlite::params![root.to_string(),hash(&root.to_string()),now()-3600]).unwrap();
    store.db.execute("INSERT INTO reply_deliveries(id,parent_delivery_id,state,record_key,record_json,record_hash,attempts) VALUES(1,1,'sending','replykey',?1,?2,1)",rusqlite::params![child.to_string(),hash(&child.to_string())]).unwrap();
    let secret = dir.path().join("password");
    std::fs::write(&secret, "test-password").unwrap();
    let mut config = Config {
        state_dir: dir.path().into(),
        publish_enabled: true,
        ..Config::default()
    };
    config.bluesky.did = Some("did:plc:test".into());
    config.bluesky.pds = pds.into();
    config.bluesky.app_password_file = Some(secret);
    config.scorecards.enabled = true;
    Fixture {
        _dir: dir,
        store,
        config,
        root,
        child,
    }
}

#[test]
fn uncertain_reply_reconciles_same_identity_without_another_write() {
    let (server, url) = server();
    let mut fixture = fixture(&url);
    let child = fixture.child.clone();
    let worker = thread::spawn(move || {
        reply(
            next(&server),
            200,
            json!({"did":"did:plc:test","accessJwt":"access","refreshJwt":"refresh"}),
        );
        let read = next(&server);
        assert!(read.url().contains("rkey=replykey"));
        reply(
            read,
            200,
            json!({"uri":"at://did:plc:test/app.bsky.feed.post/replykey","cid":"replycid","value":child}),
        );
        assert!(
            server
                .recv_timeout(Duration::from_millis(250))
                .unwrap()
                .is_none()
        );
    });
    scorecards::run(
        &mut fixture.store,
        &fixture.config,
        &mut Bluesky::new(&fixture.config).unwrap(),
    )
    .unwrap();
    worker.join().unwrap();
    let state: String = fixture
        .store
        .db
        .query_row("SELECT state FROM reply_deliveries WHERE id=1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(state, "sent");
    let parent: String = fixture
        .store
        .db
        .query_row("SELECT state FROM deliveries WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(parent, "sent");
}

#[test]
fn absent_reply_failed_write_keeps_parent_and_reply_key() {
    let (server, url) = server();
    let mut fixture = fixture(&url);
    let root = fixture.root.clone();
    let worker = thread::spawn(move || {
        reply(
            next(&server),
            200,
            json!({"did":"did:plc:test","accessJwt":"access","refreshJwt":"refresh"}),
        );
        let read = next(&server);
        assert!(read.url().contains("rkey=replykey"));
        reply(read, 400, json!({"error":"RecordNotFound"}));
        let parent = next(&server);
        assert!(parent.url().contains("rkey=parentkey"));
        reply(
            parent,
            200,
            json!({"uri":"at://did:plc:test/app.bsky.feed.post/parentkey","cid":"parentcid","value":root}),
        );
        let mut write = next(&server);
        assert!(write.url().contains("putRecord"));
        let mut body = String::new();
        write.as_reader().read_to_string(&mut body).unwrap();
        let value: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["rkey"], "replykey");
        assert!(value.get("swapRecord").is_some_and(Value::is_null));
        reply(write, 503, json!({"error":"Unavailable"}));
    });
    scorecards::run(
        &mut fixture.store,
        &fixture.config,
        &mut Bluesky::new(&fixture.config).unwrap(),
    )
    .unwrap();
    worker.join().unwrap();
    let (state, key, attempts): (String, String, i64) = fixture
        .store
        .db
        .query_row(
            "SELECT state,record_key,attempts FROM reply_deliveries WHERE id=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        (state, key, attempts),
        ("retry".into(), "replykey".into(), 2)
    );
    let parent: String = fixture
        .store
        .db
        .query_row("SELECT state FROM deliveries WHERE id=1", [], |r| r.get(0))
        .unwrap();
    assert_eq!(parent, "sent");
}
