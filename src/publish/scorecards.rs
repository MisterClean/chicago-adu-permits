//! Independent, recoverable second posts for sent announcements.
use super::{DeliveryError, Publisher, Receipt, Reconciliation, bluesky::Bluesky};
use crate::{
    config::Config,
    normalize::hash,
    scorecard::{self, Snapshot},
    store::{Store, now},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

struct DueReply {
    id: i64,
    parent: i64,
    attempts: i64,
    key: Option<String>,
    frozen: Option<String>,
    digest: Option<String>,
    snapshot: Option<String>,
    snapshot_hash: Option<String>,
    parent_key: String,
    parent_record: String,
    parent_uri: String,
    parent_cid: String,
}

pub fn enqueue(store: &Store, config: &Config, application_id: &str, reason: &str) -> Result<i64> {
    ensure!(!reason.trim().is_empty(), "backfill reason required");
    let account = config
        .bluesky
        .did
        .as_deref()
        .context("configure Bluesky DID")?;
    let parent: i64=store.db.query_row(
        "SELECT d.id FROM deliveries d JOIN events e ON e.event_key=d.event_key WHERE e.application_id=?1 AND d.account_id=?2 AND d.state='sent' AND d.platform='bluesky' AND d.remote_uri IS NOT NULL AND d.remote_cid IS NOT NULL AND d.record_key IS NOT NULL AND d.record_json IS NOT NULL",
        params![application_id,account],|r|r.get(0),
    ).context("sent announcement with a verified receipt required")?;
    store.db.execute("INSERT OR IGNORE INTO reply_deliveries(parent_delivery_id,enqueued_at,enqueue_reason) VALUES(?1,?2,?3)",params![parent,now(),reason])?;
    let id = store.db.query_row(
        "SELECT id FROM reply_deliveries WHERE parent_delivery_id=?1",
        [parent],
        |r| r.get(0),
    )?;
    Ok(id)
}

pub fn list(store: &Store) -> Result<Value> {
    let mut stmt=store.db.prepare("SELECT r.id,e.application_id,r.state,r.attempts,r.last_error,r.remote_uri FROM reply_deliveries r JOIN deliveries d ON d.id=r.parent_delivery_id JOIN events e ON e.event_key=d.event_key ORDER BY r.id DESC")?;
    let rows=stmt.query_map([],|r|Ok(json!({"id":r.get::<_,i64>(0)?,"application_id":r.get::<_,String>(1)?,"state":r.get::<_,String>(2)?,"attempts":r.get::<_,i64>(3)?,"error":r.get::<_,Option<String>>(4)?,"uri":r.get::<_,Option<String>>(5)?})))?;
    Ok(Value::Array(rows.collect::<rusqlite::Result<Vec<_>>>()?))
}

pub fn inspect(store: &Store, id: i64) -> Result<Value> {
    let mut reply: Value = store.db.query_row(
        "SELECT e.application_id,r.state,d.remote_uri,r.remote_uri,r.record_key,r.snapshot_json,r.record_json,r.snapshot_hash,r.record_hash,r.attempts,r.next_attempt,r.last_error,r.enqueue_reason FROM reply_deliveries r JOIN deliveries d ON d.id=r.parent_delivery_id JOIN events e ON e.event_key=d.event_key WHERE r.id=?1",
        [id],
        |r| Ok(json!({"id":id,"application_id":r.get::<_,String>(0)?,"state":r.get::<_,String>(1)?,"parent_uri":r.get::<_,Option<String>>(2)?,"reply_uri":r.get::<_,Option<String>>(3)?,"record_key":r.get::<_,Option<String>>(4)?,"snapshot":r.get::<_,Option<String>>(5)?,"record":r.get::<_,Option<String>>(6)?,"snapshot_hash":r.get::<_,Option<String>>(7)?,"record_hash":r.get::<_,Option<String>>(8)?,"attempts":r.get::<_,i64>(9)?,"next_attempt":r.get::<_,i64>(10)?,"last_error":r.get::<_,Option<String>>(11)?,"enqueue_reason":r.get::<_,String>(12)?})),
    ).context("scorecard reply not found")?;
    for field in ["snapshot", "record"] {
        if let Some(serialized) = reply[field].as_str() {
            reply[field] = serde_json::from_str(serialized)?;
        }
    }
    let mut stmt = store.db.prepare(
        "SELECT at,outcome,detail FROM reply_attempts WHERE reply_delivery_id=?1 ORDER BY id",
    )?;
    let attempts = stmt.query_map([id], |r| Ok(json!({"at":r.get::<_,i64>(0)?,"outcome":r.get::<_,String>(1)?,"detail":r.get::<_,Option<String>>(2)?})))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    reply["attempt_history"] = Value::Array(attempts);
    Ok(reply)
}

pub fn preview(
    store: &Store,
    config: &Config,
    application_id: &str,
    output_dir: &Path,
) -> Result<()> {
    let snapshot = scorecard::snapshot_for_application(store, config, application_id)?;
    let outputs = [
        "n5.jpg",
        "wc.jpg",
        "n5.alt.txt",
        "wc.alt.txt",
        "text.txt",
        "snapshot.json",
    ];
    ensure!(
        outputs.iter().all(|name| !output_dir.join(name).exists()),
        "preview output already exists"
    );
    let work = config.state_dir.join("scorecard-preview").join(format!(
        "{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let images = render_maps(config, &work, &snapshot)?;
    fs::create_dir_all(output_dir)?;
    fs::write(output_dir.join("n5.jpg"), &images[0].0)?;
    fs::write(output_dir.join("wc.jpg"), &images[1].0)?;
    fs::write(output_dir.join("n5.alt.txt"), &images[0].1)?;
    fs::write(output_dir.join("wc.alt.txt"), &images[1].1)?;
    fs::write(output_dir.join("text.txt"), snapshot.text())?;
    fs::write(
        output_dir.join("snapshot.json"),
        serde_json::to_vec_pretty(&snapshot)?,
    )?;
    println!("{}", output_dir.display());
    Ok(())
}

fn sync_new(store: &Store, publisher: &Bluesky) -> Result<()> {
    store.db.execute(
        "INSERT OR IGNORE INTO scorecard_state(id,activated_at) VALUES(1,?1)",
        [now()],
    )?;
    store.db.execute("INSERT OR IGNORE INTO reply_deliveries(parent_delivery_id,enqueued_at) SELECT d.id,?3 FROM deliveries d JOIN scorecard_state s ON s.id=1 WHERE d.platform=?1 AND d.account_id=?2 AND d.state='sent' AND d.sent_at>=s.activated_at AND d.remote_uri IS NOT NULL AND d.remote_cid IS NOT NULL AND d.record_key IS NOT NULL AND d.record_json IS NOT NULL",params![publisher.platform(),publisher.account(),now()])?;
    Ok(())
}

pub fn retry(store: &Store, id: i64, reason: &str) -> Result<()> {
    ensure!(!reason.trim().is_empty(), "retry reason required");
    let changed=store.db.execute("UPDATE reply_deliveries SET state='retry',next_attempt=0,last_error=NULL WHERE id=?1 AND state IN ('held','failed','retry','sending')",[id])?;
    ensure!(changed == 1, "reply is not retryable");
    store.db.execute("INSERT INTO reply_attempts(reply_delivery_id,at,outcome,detail) VALUES(?1,?2,'operator_retry',?3)",params![id,now(),reason])?;
    Ok(())
}

pub fn run(store: &mut Store, config: &Config, publisher: &mut Bluesky) -> Result<()> {
    ensure!(
        config.publish_enabled && config.scorecards.enabled,
        "scorecard publishing disabled"
    );
    run_impl(store, config, publisher, None)
}

pub fn run_prepared(
    store: &mut Store,
    config: &Config,
    publisher: &mut Bluesky,
    id: i64,
    input_dir: &Path,
) -> Result<()> {
    ensure!(config.publish_enabled, "publishing disabled");
    ensure!(id > 0, "positive reply ID required");
    run_impl(store, config, publisher, Some((id, input_dir)))?;
    let state: String = store.db.query_row(
        "SELECT state FROM reply_deliveries WHERE id=?1",
        [id],
        |row| row.get(0),
    )?;
    ensure!(
        state == "sent",
        "reply {id} remains {state}; inspect and retry it"
    );
    Ok(())
}

fn run_impl(
    store: &mut Store,
    config: &Config,
    publisher: &mut Bluesky,
    prepared: Option<(i64, &Path)>,
) -> Result<()> {
    if prepared.is_none() {
        sync_new(store, publisher)?;
    }
    let paused: bool = store.db.query_row(
        "SELECT paused FROM adapter_state WHERE platform=?1 AND account_id=?2",
        params![publisher.platform(), publisher.account()],
        |r| r.get(0),
    )?;
    ensure!(!paused, "Bluesky adapter paused");
    let posting_paused: bool =
        store
            .db
            .query_row("SELECT posting_paused FROM source_state LIMIT 1", [], |r| {
                r.get(0)
            })?;
    ensure!(!posting_paused, "publishing paused in database");
    let due:Option<DueReply>=store.db.query_row(
        "SELECT r.id,r.parent_delivery_id,r.attempts,r.record_key,r.record_json,r.record_hash,r.snapshot_json,r.snapshot_hash,d.record_key,d.record_json,d.remote_uri,d.remote_cid FROM reply_deliveries r JOIN deliveries d ON d.id=r.parent_delivery_id WHERE d.platform=?1 AND d.account_id=?2 AND d.state='sent' AND r.state IN ('pending','prepared','sending','retry') AND r.next_attempt<=?3 AND (?4 IS NULL OR r.id=?4) ORDER BY CASE WHEN r.attempts>0 THEN 0 ELSE 1 END,r.id LIMIT 1",
        params![publisher.platform(),publisher.account(),now(),prepared.map(|(id,_)|id)],
        |r|Ok(DueReply{id:r.get(0)?,parent:r.get(1)?,attempts:r.get(2)?,key:r.get(3)?,frozen:r.get(4)?,digest:r.get(5)?,snapshot:r.get(6)?,snapshot_hash:r.get(7)?,parent_key:r.get(8)?,parent_record:r.get(9)?,parent_uri:r.get(10)?,parent_cid:r.get(11)?}),
    ).optional()?;
    let Some(due) = due else {
        ensure!(
            prepared.is_none(),
            "requested reply is not due; inspect its state and retry time"
        );
        return Ok(());
    };
    if let (Some(key), Some(frozen)) = (&due.key, &due.frozen) {
        ensure!(
            due.digest.as_deref() == Some(hash(frozen).as_str()),
            "frozen reply hash mismatch"
        );
        let record: Value = serde_json::from_str(frozen)?;
        scorecard::validate_reply(&record)?;
        if due.attempts > 0 {
            match publisher.reconcile(key, &record) {
                Ok(Reconciliation::Same(receipt)) => {
                    sent(store, due.id, &receipt)?;
                    return Ok(());
                }
                Ok(Reconciliation::Conflict) => {
                    hold(store, due.id, "remote reply record conflict")?;
                    return Ok(());
                }
                Ok(Reconciliation::Absent) => {}
                Err(error) => {
                    failure(store, due.id, publisher, &error, due.attempts + 1)?;
                    return Ok(());
                }
            }
        }
        send(store, config, publisher, &due, key, &record)?;
        return Ok(());
    }
    ensure!(
        due.key.is_none() && due.frozen.is_none(),
        "partial reply preparation; inspect manually"
    );
    let parent: Value = serde_json::from_str(&due.parent_record)?;
    match publisher.reconcile(&due.parent_key, &parent) {
        Ok(Reconciliation::Same(receipt))
            if receipt.uri == due.parent_uri && receipt.cid == due.parent_cid => {}
        Ok(Reconciliation::Same(_) | Reconciliation::Absent | Reconciliation::Conflict) => {
            hold(
                store,
                due.id,
                "parent announcement missing or changed on PDS",
            )?;
            return Ok(());
        }
        Err(error) => {
            failure(store, due.id, publisher, &error, due.attempts + 1)?;
            return Ok(());
        }
    }
    let snapshot = if let Some(serialized) = &due.snapshot {
        ensure!(
            due.snapshot_hash.as_deref() == Some(hash(serialized).as_str()),
            "frozen map evidence hash mismatch"
        );
        serde_json::from_str(serialized)?
    } else {
        let snapshot = match scorecard::snapshot(store, config, due.parent) {
            Ok(value) => value,
            Err(error) => {
                failure(
                    store,
                    due.id,
                    publisher,
                    &DeliveryError::Retry {
                        message: format!("scorecard evidence: {error:#}"),
                        after: Some(3600),
                    },
                    due.attempts + 1,
                )?;
                return Ok(());
            }
        };
        let serialized = serde_json::to_string(&snapshot)?;
        store.db.execute(
            "UPDATE reply_deliveries SET snapshot_json=?2,snapshot_hash=?3 WHERE id=?1",
            params![due.id, serialized, hash(&serialized)],
        )?;
        snapshot
    };
    if let Some((_, input_dir)) = prepared {
        let supplied: Snapshot =
            serde_json::from_slice(&fs::read(input_dir.join("snapshot.json"))?)?;
        ensure!(
            serde_json::to_value(&supplied)? == serde_json::to_value(&snapshot)?,
            "prepared maps do not match the frozen source snapshot"
        );
    }
    let work = config
        .state_dir
        .join("scorecard-work")
        .join(due.id.to_string());
    let images = match prepared {
        Some((_, input_dir)) => read_prepared(input_dir, &snapshot)?,
        None => match render_maps(config, &work, &snapshot) {
            Ok(images) => images,
            Err(error) => {
                failure(
                    store,
                    due.id,
                    publisher,
                    &DeliveryError::Retry {
                        message: format!("render scorecard: {error:#}"),
                        after: Some(900),
                    },
                    due.attempts + 1,
                )?;
                return Ok(());
            }
        },
    };
    let mut record = scorecard::reply_record(
        &snapshot,
        &due.parent_uri,
        &due.parent_cid,
        chrono::Utc::now(),
    )?;
    if let Err(error) = publisher.attach_scorecard_images(&mut record, &images) {
        let fallback = DeliveryError::Retry {
            message: format!("upload scorecard: {error:#}"),
            after: None,
        };
        failure(
            store,
            due.id,
            publisher,
            error.downcast_ref::<DeliveryError>().unwrap_or(&fallback),
            due.attempts + 1,
        )?;
        return Ok(());
    }
    scorecard::validate_reply(&record)?;
    let tx = store.db.transaction()?;
    let last: i64 = tx.query_row(
        "SELECT last_identity_clock FROM adapter_state WHERE platform=?1 AND account_id=?2",
        params![publisher.platform(), publisher.account()],
        |r| r.get(0),
    )?;
    let identity = publisher.allocate_identity(last)?;
    let serialized = serde_json::to_string(&record)?;
    tx.execute(
        "UPDATE adapter_state SET last_identity_clock=?3 WHERE platform=?1 AND account_id=?2",
        params![publisher.platform(), publisher.account(), identity.clock],
    )?;
    tx.execute("UPDATE reply_deliveries SET record_key=?2,record_json=?3,record_hash=?4,template_version=?5,state='prepared' WHERE id=?1",params![due.id,identity.key,serialized,hash(&serialized),scorecard::TEMPLATE_VERSION])?;
    tx.commit()?;
    send(store, config, publisher, &due, &identity.key, &record)
}

fn send(
    store: &Store,
    config: &Config,
    publisher: &mut Bluesky,
    due: &DueReply,
    key: &str,
    record: &Value,
) -> Result<()> {
    let parent: Value = serde_json::from_str(&due.parent_record)?;
    match publisher.reconcile(&due.parent_key, &parent) {
        Ok(Reconciliation::Same(receipt))
            if receipt.uri == due.parent_uri && receipt.cid == due.parent_cid => {}
        Ok(Reconciliation::Same(_) | Reconciliation::Absent | Reconciliation::Conflict) => {
            hold(
                store,
                due.id,
                "parent announcement missing or changed on PDS",
            )?;
            return Ok(());
        }
        Err(error) => {
            failure(store, due.id, publisher, &error, due.attempts + 1)?;
            return Ok(());
        }
    }
    let last: Option<i64> = store.db.query_row(
        "SELECT last_send FROM adapter_state WHERE platform=?1 AND account_id=?2",
        params![publisher.platform(), publisher.account()],
        |r| r.get(0),
    )?;
    if last.is_some_and(|at| now() - at < config.min_send_interval_seconds) {
        return Ok(());
    }
    let tx = store.db.unchecked_transaction()?;
    tx.execute(
        "UPDATE reply_deliveries SET state='sending',attempts=attempts+1 WHERE id=?1",
        [due.id],
    )?;
    tx.execute(
        "UPDATE adapter_state SET last_send=?3 WHERE platform=?1 AND account_id=?2",
        params![publisher.platform(), publisher.account(), now()],
    )?;
    tx.execute(
        "INSERT INTO reply_attempts(reply_delivery_id,at,outcome) VALUES(?1,?2,'started')",
        params![due.id, now()],
    )?;
    tx.commit()?;
    match publisher.send(key, record) {
        Ok(receipt) => sent(store, due.id, &receipt)?,
        Err(DeliveryError::Conflict) => match publisher.reconcile(key, record) {
            Ok(Reconciliation::Same(receipt)) => sent(store, due.id, &receipt)?,
            Ok(Reconciliation::Conflict) => hold(store, due.id, "remote reply record conflict")?,
            Ok(Reconciliation::Absent) => failure(
                store,
                due.id,
                publisher,
                &DeliveryError::Retry {
                    message: "write conflicted but reply absent".into(),
                    after: None,
                },
                due.attempts + 1,
            )?,
            Err(error) => failure(store, due.id, publisher, &error, due.attempts + 1)?,
        },
        Err(error) => failure(store, due.id, publisher, &error, due.attempts + 1)?,
    }
    Ok(())
}

fn sent(store: &Store, id: i64, receipt: &Receipt) -> Result<()> {
    store.db.execute("UPDATE reply_deliveries SET state='sent',remote_uri=?2,remote_cid=?3,sent_at=?4,last_error=NULL WHERE id=?1",params![id,receipt.uri,receipt.cid,now()])?;
    store.db.execute(
        "INSERT INTO reply_attempts(reply_delivery_id,at,outcome) VALUES(?1,?2,'sent')",
        params![id, now()],
    )?;
    eprintln!(
        "{}",
        json!({"level":"info","event":"scorecard_sent","reply":id,"uri":receipt.uri})
    );
    Ok(())
}
fn hold(store: &Store, id: i64, reason: &str) -> Result<()> {
    store.db.execute(
        "UPDATE reply_deliveries SET state='held',last_error=?2 WHERE id=?1",
        params![id, reason],
    )?;
    eprintln!(
        "{}",
        json!({"level":"error","event":"scorecard_held","reply":id,"reason":reason})
    );
    Ok(())
}
fn failure(
    store: &Store,
    id: i64,
    publisher: &Bluesky,
    error: &DeliveryError,
    attempts: i64,
) -> Result<()> {
    if matches!(error, DeliveryError::Auth) {
        store.db.execute(
            "UPDATE adapter_state SET paused=1 WHERE platform=?1 AND account_id=?2",
            params![publisher.platform(), publisher.account()],
        )?;
    }
    let (state, delay) = match error {
        DeliveryError::Invalid | DeliveryError::Conflict => ("held", 0),
        DeliveryError::Auth => ("retry", 3600),
        DeliveryError::Retry { after, .. } => {
            let base = match attempts {
                0 | 1 => 60,
                2 => 300,
                3 => 900,
                4 => 3600,
                _ => 21600,
            };
            (
                if attempts >= 10 { "failed" } else { "retry" },
                after.unwrap_or(0).max(base),
            )
        }
    };
    store.db.execute("UPDATE reply_deliveries SET state=?2,attempts=MAX(attempts,?3),next_attempt=?4,last_error=?5 WHERE id=?1",params![id,state,attempts,now()+delay,error.to_string()])?;
    store.db.execute(
        "INSERT INTO reply_attempts(reply_delivery_id,at,outcome,detail) VALUES(?1,?2,?3,?4)",
        params![id, now(), state, error.to_string()],
    )?;
    eprintln!(
        "{}",
        json!({"level":"error","event":"scorecard_delivery_error","reply":id,"state":state,"error":error.to_string()})
    );
    Ok(())
}

fn read_prepared(input_dir: &Path, snapshot: &Snapshot) -> Result<Vec<(Vec<u8>, String)>> {
    ensure!(
        fs::read_to_string(input_dir.join("text.txt"))? == snapshot.text(),
        "prepared post text differs from snapshot"
    );
    let (neighborhood_alt, ward_alt) = map_alts(snapshot);
    let mut images = Vec::with_capacity(2);
    for (name, alt_name, expected_alt) in [
        ("n5.jpg", "n5.alt.txt", neighborhood_alt),
        ("wc.jpg", "wc.alt.txt", ward_alt),
    ] {
        ensure!(
            fs::read_to_string(input_dir.join(alt_name))? == expected_alt,
            "prepared map alt text differs from snapshot"
        );
        let bytes = fs::read(input_dir.join(name))?;
        ensure!(valid_jpeg(&bytes), "invalid prepared map JPEG");
        let dimensions =
            image::ImageReader::with_format(std::io::Cursor::new(&bytes), image::ImageFormat::Jpeg)
                .into_dimensions()?;
        ensure!(
            dimensions == (2160, 2160),
            "invalid prepared map dimensions"
        );
        images.push((bytes, expected_alt));
    }
    Ok(images)
}

fn render_maps(
    config: &Config,
    work: &Path,
    snapshot: &Snapshot,
) -> Result<Vec<(Vec<u8>, String)>> {
    fs::create_dir_all(work)?;
    let input = work.join("snapshot.json");
    fs::write(&input, serde_json::to_vec(snapshot)?)?;
    let output = Command::new(&config.scorecards.node_bin)
        .arg(config.scorecards.renderer_dir.join("render-live.mjs"))
        .arg(&input)
        .arg(work)
        .envs(
            config
                .scorecards
                .chrome_bin
                .as_ref()
                .map(|p| ("CHROME_BIN", p))
                .into_iter(),
        )
        .output()
        .context("start map renderer")?;
    ensure!(
        output.status.success(),
        "map renderer failed: {}",
        String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(500)
            .collect::<String>()
    );
    let verification: Value = serde_json::from_slice(&fs::read(work.join("verification.json"))?)?;
    ensure!(
        verification["source_run"] == snapshot.source_run
            && verification["focus"] == snapshot.focus.id
            && verification["renders"]
                .as_array()
                .is_some_and(|a| a.len() == 2),
        "map verification mismatch"
    );
    let neighborhood = fs::read(work.join("n5.jpg"))?;
    let ward = fs::read(work.join("wc.jpg"))?;
    ensure!(
        valid_jpeg(&neighborhood) && valid_jpeg(&ward),
        "invalid rendered JPEGs"
    );
    fs::remove_dir_all(work)?;
    let (neighborhood_alt, ward_alt) = map_alts(snapshot);
    Ok(vec![(neighborhood, neighborhood_alt), (ward, ward_alt)])
}

fn map_alts(snapshot: &Snapshot) -> (String, String) {
    let neighborhood_alt = format!(
        "Oblique neighborhood map centered on {}, marked by a red pin among existing building volumes. {} ADU{} requested in Ward {}. Streets, transit, and named places provide context; buildings do not show the proposed ADU. Coordinates: City of Chicago Data Portal. Basemap: OpenMapTiles and OpenStreetMap.",
        snapshot.focus.address,
        snapshot.focus.quantity,
        if snapshot.focus.quantity == 1 {
            ""
        } else {
            "s"
        },
        snapshot.ward
    );
    let ward_alt = format!(
        "Ward {} outlined with {} mapped application{} pinned. Wardwide total: {} requested ADU{} across {} qualifying application{}. A red ring marks {}. Rank {} of 50 wards by requested ADUs. Submitted since April 1, 2026; as of {}. Counts are requested units, not building permits or completed homes. Application coordinates: City of Chicago Data Portal. Boundary: Cook County. Basemap: OpenMapTiles and OpenStreetMap.",
        snapshot.ward,
        snapshot.mapped_applications,
        if snapshot.mapped_applications == 1 {
            ""
        } else {
            "s"
        },
        snapshot.adus,
        if snapshot.adus == 1 { "" } else { "s" },
        snapshot.applications,
        if snapshot.applications == 1 { "" } else { "s" },
        snapshot.focus.address,
        snapshot.rank,
        snapshot.as_of
    );
    (neighborhood_alt, ward_alt)
}

fn valid_jpeg(bytes: &[u8]) -> bool {
    bytes.len() > 1000
        && bytes.len() <= 2_000_000
        && bytes.starts_with(&[0xff, 0xd8])
        && bytes.ends_with(&[0xff, 0xd9])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{normalize::Location, scorecard::Point};

    #[test]
    fn prepared_maps_require_matching_text_alt_and_jpeg_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = Snapshot {
            source_run: 1,
            as_of: "2026-09-24".into(),
            cohort_start: "2026-04-01".into(),
            ward: 43,
            applications: 1,
            adus: 1,
            rank: 1,
            tied: false,
            city_adus: 1,
            city_applications: 1,
            mapped_applications: 1,
            focus: Point {
                id: "123".into(),
                address: "1 W TEST ST".into(),
                quantity: 1,
                location: Location {
                    latitude: 41.9,
                    longitude: -87.6,
                },
            },
            points: Vec::new(),
            boundary: json!({}),
        };
        let mut jpeg = Vec::new();
        let image = image::RgbImage::from_pixel(2160, 2160, image::Rgb([255, 255, 255]));
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80)
            .encode_image(&image)
            .unwrap();
        let (n_alt, w_alt) = map_alts(&snapshot);
        assert!(w_alt.contains("1 mapped application pinned"));
        fs::write(dir.path().join("text.txt"), snapshot.text()).unwrap();
        fs::write(dir.path().join("n5.alt.txt"), n_alt).unwrap();
        fs::write(dir.path().join("wc.alt.txt"), w_alt).unwrap();
        fs::write(dir.path().join("n5.jpg"), &jpeg).unwrap();
        fs::write(dir.path().join("wc.jpg"), &jpeg).unwrap();
        assert_eq!(read_prepared(dir.path(), &snapshot).unwrap().len(), 2);
        fs::write(dir.path().join("text.txt"), "changed").unwrap();
        assert!(read_prepared(dir.path(), &snapshot).is_err());
    }

    #[test]
    fn activation_excludes_history_and_queues_new_sent_roots_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        store.db.execute_batch("INSERT INTO ingest_runs(id,started_at,ended_at,status) VALUES(1,1,1,'success');
            INSERT INTO applications VALUES('j4h8-ug9m','123',1,'{}','hash',1,1,1,1);
            INSERT INTO application_versions VALUES('j4h8-ug9m','123',1,1,'hash','{}',1,NULL,'[]');
            INSERT INTO events(event_key,dataset_id,application_id,initial_evidence_version,evidence_version,detection_kind,observed_at,disposition,reason) VALUES('event','j4h8-ug9m','123',1,1,'new',1,'pending','test');
            INSERT INTO deliveries(id,event_key,platform,account_id,state,record_key,record_json,remote_uri,remote_cid,sent_at) VALUES(1,'event','bluesky','did:plc:test','sent','rootkey','{}','at://did:plc:test/app.bsky.feed.post/rootkey','rootcid',1);").unwrap();
        let mut config = Config::default();
        config.bluesky.did = Some("did:plc:test".into());
        let publisher = Bluesky::new(&config).unwrap();
        sync_new(&store, &publisher).unwrap();
        assert!(list(&store).unwrap().as_array().unwrap().is_empty());
        let activated: i64 = store
            .db
            .query_row("SELECT activated_at FROM scorecard_state", [], |r| r.get(0))
            .unwrap();
        store
            .db
            .execute(
                "UPDATE deliveries SET sent_at=?1 WHERE id=1",
                [activated + 1],
            )
            .unwrap();
        sync_new(&store, &publisher).unwrap();
        sync_new(&store, &publisher).unwrap();
        assert_eq!(list(&store).unwrap().as_array().unwrap().len(), 1);
    }
}
