use adu_bot::{
    config::Config,
    normalize::{FIELDS, MAP_FIELDS, Observation},
    publish::{DeliveryError, Publisher, Reconciliation, bluesky::Bluesky},
    source::{self, Socrata},
    store::Store,
};
use serde_json::{Value, json};
use std::{sync::Arc, thread, time::Duration};
use tempfile::TempDir;
use tiny_http::{Header, Response, Server};

fn reply(req: tiny_http::Request, status: u16, value: Value) {
    req.respond(
        Response::from_string(value.to_string())
            .with_status_code(status)
            .with_header(Header::from_bytes("Content-Type", "application/json").unwrap()),
    )
    .unwrap();
}
fn server() -> (Arc<Server>, String) {
    let s = Arc::new(Server::http("127.0.0.1:0").unwrap());
    let url = format!("http://{}", s.server_addr());
    (s, url)
}
fn request(s: &Server) -> tiny_http::Request {
    s.recv_timeout(Duration::from_secs(10))
        .unwrap()
        .expect("expected HTTP request")
}
fn session() -> Value {
    json!({"did":"did:plc:test","accessJwt":"test-access","refreshJwt":"test-refresh"})
}
fn publisher(url: &str) -> (TempDir, Bluesky) {
    let dir = TempDir::new().unwrap();
    let secret = dir.path().join("app-password");
    std::fs::write(&secret, "test-password").unwrap();
    let mut config = Config {
        state_dir: dir.path().into(),
        timeout_seconds: 1,
        ..Config::default()
    };
    config.bluesky.did = Some("did:plc:test".into());
    config.bluesky.pds = url.into();
    config.bluesky.app_password_file = Some(secret);
    (dir, Bluesky::new(&config).unwrap())
}
fn record(_p: &Bluesky) -> Value {
    adu_bot::render::record(
        &Observation::parse(json!({"id":"123","status":"Pre-Certified"})).unwrap(),
        chrono::Utc::now(),
    )
    .unwrap()
}
#[test]
fn actual_http_put_uses_explicit_null_and_reconcile_compares_frozen_record() {
    let (s, url) = server();
    let (_dir, mut p) = publisher(&url);
    let record = record(&p);
    let expected = record.clone();
    let worker = thread::spawn(move || {
        let r = request(&s);
        assert!(r.url().contains("createSession"));
        reply(r, 200, session());
        let mut r = request(&s);
        assert!(r.url().contains("putRecord"));
        let body: Value = serde_json::from_reader(r.as_reader()).unwrap();
        assert_eq!(body["swapRecord"], Value::Null);
        assert!(body.as_object().unwrap().contains_key("swapRecord"));
        assert_eq!(body["validate"], true);
        assert_eq!(body["record"], expected);
        reply(
            r,
            200,
            json!({"uri":"at://did:plc:test/app.bsky.feed.post/3jzfcijpj2z2a","cid":"cid1"}),
        );
        let r = request(&s);
        assert!(r.url().contains("getRecord"));
        reply(
            r,
            200,
            json!({"uri":"at://did:plc:test/app.bsky.feed.post/3jzfcijpj2z2a","cid":"cid1","value":expected}),
        );
    });
    p.send("3jzfcijpj2z2a", &record).unwrap();
    assert!(matches!(
        p.reconcile("3jzfcijpj2z2a", &record).unwrap(),
        Reconciliation::Same(_)
    ));
    worker.join().unwrap();
}
#[test]
fn expired_access_refreshes_with_refresh_token_and_persists_rotation() {
    let (s, url) = server();
    let (dir, mut p) = publisher(&url);
    let record = record(&p);
    let worker = thread::spawn(move || {
        reply(request(&s), 200, session());
        reply(request(&s), 400, json!({"error":"ExpiredToken"}));
        let r = request(&s);
        assert!(r.url().contains("refreshSession"));
        assert_eq!(r.body_length(), Some(0));
        assert!(!r.headers().iter().any(|h| h.field.equiv("Content-Type")));
        assert!(
            r.headers().iter().any(
                |h| h.field.equiv("Authorization") && h.value.as_str() == "Bearer test-refresh"
            )
        );
        reply(
            r,
            200,
            json!({"did":"did:plc:test","accessJwt":"new-access","refreshJwt":"new-refresh"}),
        );
        let r = request(&s);
        assert!(
            r.headers()
                .iter()
                .any(|h| h.field.equiv("Authorization") && h.value.as_str() == "Bearer new-access")
        );
        reply(
            r,
            200,
            json!({"uri":"at://did:plc:test/app.bsky.feed.post/3jzfcijpj2z2a","cid":"cid1"}),
        );
    });
    p.send("3jzfcijpj2z2a", &record).unwrap();
    worker.join().unwrap();
    let path = dir.path().join("bluesky-session.json");
    let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(v["refreshJwt"], "new-refresh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
#[test]
fn remote_http_errors_are_typed_and_only_record_not_found_means_absent() {
    let (s, url) = server();
    let (_dir, mut p) = publisher(&url);
    let record = record(&p);
    let worker = thread::spawn(move || {
        reply(request(&s), 200, session());
        let r = request(&s);
        r.respond(
            Response::from_string("{\"error\":\"RateLimitExceeded\"}")
                .with_status_code(429)
                .with_header(Header::from_bytes("Retry-After", "120").unwrap()),
        )
        .unwrap();
        reply(request(&s), 503, json!({"error":"Unavailable"}));
        reply(request(&s), 404, json!({"error":"NotFound"}));
        reply(request(&s), 400, json!({"error":"RecordNotFound"}));
        reply(request(&s), 400, json!({"error":"InvalidSwap"}));
        reply(request(&s), 400, json!({"error":"InvalidRecord"}));
    });
    assert!(matches!(
        p.reconcile("key", &record),
        Err(DeliveryError::Retry {
            after: Some(120),
            ..
        })
    ));
    assert!(matches!(
        p.reconcile("key", &record),
        Err(DeliveryError::Retry { .. })
    ));
    assert!(matches!(
        p.reconcile("key", &record),
        Err(DeliveryError::Invalid)
    ));
    assert!(matches!(
        p.reconcile("key", &record),
        Ok(Reconciliation::Absent)
    ));
    assert!(matches!(
        p.send("key", &record),
        Err(DeliveryError::Conflict)
    ));
    assert!(matches!(
        p.send("key", &record),
        Err(DeliveryError::Invalid)
    ));
    worker.join().unwrap();
}
#[test]
fn wrong_account_did_never_writes() {
    let (s, url) = server();
    let (_dir, mut p) = publisher(&url);
    let record = record(&p);
    let worker = thread::spawn(move || {
        let mut v = session();
        v["did"] = json!("did:plc:wrong");
        reply(request(&s), 200, v);
        assert!(
            s.recv_timeout(Duration::from_millis(100))
                .unwrap()
                .is_none()
        );
    });
    assert!(matches!(p.send("key", &record), Err(DeliveryError::Auth)));
    worker.join().unwrap();
}
#[test]
fn timeout_after_acceptance_is_reconciled_by_authoritative_http_read() {
    let (s, url) = server();
    let (_dir, mut p) = publisher(&url);
    let record = record(&p);
    let expected = record.clone();
    let worker = thread::spawn(move || {
        reply(request(&s), 200, session());
        let mut r = request(&s);
        let body: Value = serde_json::from_reader(r.as_reader()).unwrap();
        assert_eq!(body["record"], expected);
        thread::sleep(Duration::from_millis(1200));
        drop(r);
        let r = request(&s);
        assert!(r.url().contains("getRecord"));
        reply(
            r,
            200,
            json!({"uri":"at://did:plc:test/app.bsky.feed.post/3jzfcijpj2z2a","cid":"cid1","value":expected}),
        );
    });
    assert!(matches!(
        p.send("3jzfcijpj2z2a", &record),
        Err(DeliveryError::Retry { .. })
    ));
    assert!(matches!(
        p.reconcile("3jzfcijpj2z2a", &record),
        Ok(Reconciliation::Same(_))
    ));
    worker.join().unwrap();
}
fn metadata() -> Value {
    json!({"rowsUpdatedAt":1,"columns":FIELDS.iter().chain(MAP_FIELDS).map(|(name,ty)|json!({"fieldName":name,"dataTypeName":ty})).collect::<Vec<_>>()})
}
#[test]
fn socrata_http_keyset_allowlist_and_final_empty_page() {
    let (s, url) = server();
    let dir = TempDir::new().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    let config = Config {
        source_base: url,
        page_size: 1,
        ..Config::default()
    };
    let worker = thread::spawn(move || {
        reply(request(&s), 200, metadata());
        reply(request(&s), 200, json!([{"count":"1"}]));
        let r = request(&s);
        let url = url::Url::parse(&format!("http://localhost{}", r.url())).unwrap();
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert!(!pairs["$select"].contains("applicant"));
        assert!(pairs["$select"].contains("latitude,longitude"));
        assert_eq!(pairs["$limit"], "1");
        reply(r, 200, json!([{"id":"123","status":"Pre-Certified"}]));
        let r = request(&s);
        let url = url::Url::parse(&format!("http://localhost{}", r.url())).unwrap();
        assert!(
            url.query_pairs()
                .any(|(k, v)| k == "$where" && v == "id > 123")
        );
        reply(r, 200, json!([]));
        reply(request(&s), 200, metadata());
        reply(request(&s), 200, json!([{"count":"1"}]));
    });
    source::ingest(&mut store, &mut Socrata::new(&config), &config).unwrap();
    worker.join().unwrap();
    assert_eq!(store.status().unwrap()["present"], 1);
}
#[test]
fn malformed_oversized_and_schema_drift_fail_closed_over_http() {
    for scenario in ["malformed", "oversized", "schema"] {
        let (s, url) = server();
        let dir = TempDir::new().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let config = Config {
            source_base: url,
            response_limit: 4096,
            ..Config::default()
        };
        let worker = thread::spawn(move || {
            let mut meta = metadata();
            if scenario == "schema" {
                meta["columns"][0]["dataTypeName"] = json!("text");
            }
            reply(request(&s), 200, meta);
            if scenario == "schema" {
                return;
            }
            reply(request(&s), 200, json!([{"count":"1"}]));
            let r = request(&s);
            r.respond(Response::from_string(if scenario == "malformed" {
                "[{".into()
            } else {
                "x".repeat(5000)
            }))
            .unwrap();
        });
        assert!(source::ingest(&mut store, &mut Socrata::new(&config), &config).is_err());
        worker.join().unwrap();
        assert_eq!(store.status().unwrap()["baseline_established"], false);
    }
}

#[test]
fn repeated_auth_failure_stops_after_one_refresh() {
    let (s, url) = server();
    let (_dir, mut p) = publisher(&url);
    let record = record(&p);
    let worker = thread::spawn(move || {
        reply(request(&s), 200, session());
        reply(request(&s), 401, json!({"error":"ExpiredToken"}));
        reply(request(&s), 200, session());
        reply(request(&s), 401, json!({"error":"ExpiredToken"}));
        assert!(
            s.recv_timeout(Duration::from_millis(100))
                .unwrap()
                .is_none()
        );
    });
    assert!(matches!(p.send("key", &record), Err(DeliveryError::Auth)));
    worker.join().unwrap();
}

#[test]
fn restarted_adapter_reuses_session_and_checks_identity_before_read() {
    let (s, url) = server();
    let (dir, mut p) = publisher(&url);
    let record = record(&p);
    let expected = record.clone();
    let worker = thread::spawn(move || {
        reply(request(&s), 200, session());
        reply(
            request(&s),
            200,
            json!({"uri":"at://did:plc:test/app.bsky.feed.post/3jzfcijpj2z2a","cid":"cid1"}),
        );
        let r = request(&s);
        assert!(r.url().contains("getSession"));
        reply(r, 200, json!({"did":"did:plc:test"}));
        let r = request(&s);
        assert!(r.url().contains("getRecord"));
        reply(
            r,
            200,
            json!({"uri":"at://did:plc:test/app.bsky.feed.post/3jzfcijpj2z2a","cid":"cid1","value":expected}),
        );
    });
    p.send("3jzfcijpj2z2a", &record).unwrap();
    drop(p);
    let mut c = Config {
        state_dir: dir.path().into(),
        ..Config::default()
    };
    c.bluesky.did = Some("did:plc:test".into());
    c.bluesky.pds = url;
    let mut p = Bluesky::new(&c).unwrap();
    assert!(matches!(
        p.reconcile("3jzfcijpj2z2a", &record),
        Ok(Reconciliation::Same(_))
    ));
    worker.join().unwrap();
}

#[test]
fn auth_check_logs_in_without_publishing_or_exposing_tokens() {
    let (s, url) = server();
    let (_dir, mut p) = publisher(&url);
    let worker = thread::spawn(move || {
        let mut request = request(&s);
        assert!(request.url().contains("createSession"));
        let body: Value = serde_json::from_reader(request.as_reader()).unwrap();
        assert_eq!(body["password"], "test-password");
        reply(request, 200, session());
        assert!(
            s.recv_timeout(Duration::from_millis(100))
                .unwrap()
                .is_none()
        );
    });
    let result = p.check_auth().unwrap();
    assert_eq!(result["authenticated"], true);
    assert_eq!(result["did"], "did:plc:test");
    assert!(!result.to_string().contains("test-access"));
    assert!(!result.to_string().contains("test-refresh"));
    worker.join().unwrap();
}

#[test]
fn image_upload_refreshes_auth_and_preserves_blob_and_alt_in_post() {
    let (s, url) = server();
    let (_dir, mut p) = publisher(&url);
    let mut record = record(&p);
    let bytes = vec![255, 216, 255, 217];
    let expected = bytes.clone();
    let worker = thread::spawn(move || {
        reply(request(&s), 200, session());
        for attempt in 0..2 {
            let mut r = request(&s);
            assert!(r.url().ends_with("uploadBlob"));
            assert!(
                r.headers()
                    .iter()
                    .any(|h| h.field.equiv("Content-Type") && h.value.as_str() == "image/jpeg")
            );
            let mut actual = Vec::new();
            r.as_reader().read_to_end(&mut actual).unwrap();
            assert_eq!(actual, expected);
            if attempt == 0 {
                reply(r, 401, json!({"error":"ExpiredToken"}));
                let r = request(&s);
                assert!(r.url().contains("refreshSession"));
                reply(r, 200, session());
            } else {
                reply(
                    r,
                    200,
                    json!({"blob":{"$type":"blob","mimeType":"image/jpeg","size":4,"ref":{"$link":"image-cid"}}}),
                );
            }
        }
        let mut r = request(&s);
        assert!(r.url().contains("putRecord"));
        let value: Value = serde_json::from_reader(r.as_reader()).unwrap();
        assert_eq!(
            value["record"]["embed"]["images"][0]["image"]["ref"]["$link"],
            "image-cid"
        );
        assert_eq!(
            value["record"]["embed"]["images"][0]["alt"],
            "Permit preapproval card"
        );
        assert_eq!(
            value["record"]["embed"]["images"][0]["aspectRatio"],
            json!({"width":3200,"height":4000})
        );
        reply(
            r,
            200,
            json!({"uri":"at://did:plc:test/app.bsky.feed.post/key","cid":"record-cid"}),
        );
    });
    p.attach_image(
        &mut record,
        &adu_bot::media::PostImage {
            bytes,
            alt: "Permit preapproval card".into(),
            quality: 100,
            width: 3200,
            height: 4000,
        },
    )
    .unwrap();
    p.send("key", &record).unwrap();
    worker.join().unwrap();
}

#[test]
fn rejected_upload_never_creates_a_text_only_post() {
    let (s, url) = server();
    let (_dir, mut p) = publisher(&url);
    let mut record = record(&p);
    let worker = thread::spawn(move || {
        reply(request(&s), 200, session());
        reply(request(&s), 429, json!({"error":"RateLimitExceeded"}));
        assert!(
            s.recv_timeout(Duration::from_millis(100))
                .unwrap()
                .is_none()
        );
    });
    let error = p
        .attach_image(
            &mut record,
            &adu_bot::media::PostImage {
                bytes: vec![1, 2],
                alt: "Card".into(),
                quality: 100,
                width: 3200,
                height: 4000,
            },
        )
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<DeliveryError>(),
        Some(DeliveryError::Retry { .. })
    ));
    assert!(record.get("embed").is_none());
    worker.join().unwrap();
}
