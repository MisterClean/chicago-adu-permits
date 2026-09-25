pub mod bluesky;
pub mod scorecards;

use crate::{
    config::{Config, DATASET},
    maps,
    media::PostImage,
    normalize::{Observation, hash},
    permits::{self, Permit, PermitWardSnapshot},
    render,
    store::{Store, chicago_date, now},
};
use anyhow::{Context, Result, ensure};
use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use serde_json::Value;
use std::{path::Path, time::Instant};

#[derive(Debug)]
pub struct Receipt {
    pub uri: String,
    pub cid: String,
}
#[derive(Debug)]
pub enum Reconciliation {
    Same(Receipt),
    Absent,
    Conflict,
}
#[derive(Debug)]
pub enum DeliveryError {
    Retry { message: String, after: Option<i64> },
    Auth,
    Invalid,
    Conflict,
}
impl std::fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retry { message, .. } => write!(f, "{message}"),
            Self::Auth => write!(f, "authentication or account identity failed"),
            Self::Invalid => write!(f, "platform rejected record"),
            Self::Conflict => write!(f, "remote record conflict"),
        }
    }
}
impl std::error::Error for DeliveryError {}
/// A platform owns record formatting, durable remote identity and recovery semantics.
/// Implementations must never silently allocate a replacement identity during retries.
pub struct DeliveryIdentity {
    pub key: String,
    pub clock: i64,
}

pub trait Publisher {
    fn platform(&self) -> &'static str;
    fn account(&self) -> &str;
    fn prepare(&mut self, observation: &Observation) -> Result<Value>;
    fn prepare_permit(&mut self, _observation: &Observation, _permit: &Permit) -> Result<Value> {
        anyhow::bail!("permit delivery is unsupported by this publisher")
    }
    fn prepare_permit_reply(
        &mut self,
        _observation: &Observation,
        _permit: &Permit,
        _snapshot: &PermitWardSnapshot,
        _root_uri: &str,
        _root_cid: &str,
    ) -> Result<Value> {
        anyhow::bail!("permit map reply is unsupported by this publisher")
    }
    fn prepare_permit_reply_prepared(
        &mut self,
        _snapshot: &PermitWardSnapshot,
        _root_uri: &str,
        _root_cid: &str,
        _near: &PostImage,
        _ward: &PostImage,
    ) -> Result<Value> {
        anyhow::bail!("prepared permit map reply is unsupported by this publisher")
    }
    fn allocate_identity(&self, previous_clock: i64) -> Result<DeliveryIdentity>;
    fn template_version(&self) -> i64;
    fn reconcile(
        &mut self,
        key: &str,
        record: &Value,
    ) -> std::result::Result<Reconciliation, DeliveryError>;
    fn send(&mut self, key: &str, record: &Value) -> std::result::Result<Receipt, DeliveryError>;
}
fn gate(
    store: &Store,
    event: &str,
    evidence_version: i64,
    config: &Config,
) -> Result<Option<String>> {
    let (present,current,evidence,disposition,last,paused,kind):(bool,String,String,String,i64,bool,String)=store.db.query_row(
        "SELECT a.present,a.observation,v.observation,e.disposition,r.ended_at,s.posting_paused,e.detection_kind FROM events e JOIN applications a USING(dataset_id,application_id) JOIN application_versions v ON v.dataset_id=e.dataset_id AND v.application_id=e.application_id AND v.version=?2 JOIN source_state s ON s.dataset_id=e.dataset_id JOIN ingest_runs r ON r.id=s.last_successful_run WHERE e.event_key=?1",
        params![event,evidence_version],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?)))?;
    let current: Observation = serde_json::from_str(&current)?;
    let evidence: Observation = serde_json::from_str(&evidence)?;
    let permit_problem = if kind == "building_permit" {
        let (source_id, serialized, last_permit, permit_paused):(String,String,i64,bool)=store.db.query_row("SELECT p.source_id,p.observation,r.ended_at,s.posting_paused FROM permit_event_evidence p JOIN permit_state s ON s.id=1 JOIN permit_runs r ON r.id=s.last_successful_run WHERE p.event_key=?1",[event],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        let (current_permit, present): (String, bool) = store.db.query_row(
            "SELECT observation,present FROM permits WHERE source_id=?1",
            [source_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if permit_paused {
            Some("permit publishing paused")
        } else if now() - last_permit > config.stale_after_seconds {
            Some("permit scan is stale")
        } else if !present {
            Some("permit absent from current scan")
        } else if current_permit != serialized {
            Some("permit facts changed since match")
        } else {
            None
        }
    } else {
        None
    };
    let reason = if paused {
        Some("publishing paused in database")
    } else if let Some(problem) = permit_problem {
        Some(problem)
    } else if now() - last > config.stale_after_seconds {
        Some("source reconciliation is stale")
    } else if disposition != "pending" {
        Some("event requires review or is suppressed")
    } else if !present {
        Some("application absent")
    } else if !current.status.qualifying() {
        Some("application no longer pre-certified")
    } else if current.post_facts() != evidence.post_facts() {
        Some("post facts changed since approval")
    } else if current.date_problem(chicago_date(now())?) {
        Some("invalid or future date")
    } else {
        None
    };
    Ok(reason.map(str::to_owned))
}
pub fn dry_run(store: &Store) -> Result<()> {
    let mut stmt=store.db.prepare("SELECT e.event_key,v.observation,e.detection_kind,p.observation FROM events e JOIN application_versions v ON v.dataset_id=e.dataset_id AND v.application_id=e.application_id AND v.version=e.evidence_version LEFT JOIN permit_event_evidence p USING(event_key) WHERE e.disposition='pending' ORDER BY e.observed_at")?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let obs: Observation = serde_json::from_str(&row.get::<_, String>(1)?)?;
        let kind: String = row.get(2)?;
        let preview = if kind == "building_permit" {
            let permit: Permit = serde_json::from_str(&row.get::<_, String>(3)?)?;
            render::permit_record(&obs, &permit, Utc::now())?
        } else {
            render::record(&obs, Utc::now())?
        };
        println!(
            "{}",
            serde_json::json!({"event_key":row.get::<_,String>(0)?,"preview":preview})
        );
    }
    Ok(())
}
struct DueDelivery {
    id: i64,
    event: String,
    key: Option<String>,
    frozen: Option<String>,
    digest: Option<String>,
    attempts: i64,
    version: i64,
}
struct DueReply {
    id: i64,
    event: String,
    key: Option<String>,
    frozen: Option<String>,
    digest: Option<String>,
    attempts: i64,
    root_uri: String,
    root_cid: String,
    disposition: String,
}

pub fn publish(store: &mut Store, config: &Config, publisher: &mut impl Publisher) -> Result<()> {
    ensure!(
        config.publish_enabled,
        "publishing disabled; set publish_enabled=true explicitly"
    );
    store.sync_deliveries(config)?;
    let started = Instant::now();
    let mut processed = 0;
    let mut failed = false;
    loop {
        if processed >= config.max_posts_per_run
            || started.elapsed().as_secs() >= config.max_run_seconds
        {
            break;
        }
        let paused: bool = store.db.query_row(
            "SELECT paused FROM adapter_state WHERE platform=?1 AND account_id=?2",
            params![publisher.platform(), publisher.account()],
            |r| r.get(0),
        )?;
        if paused {
            anyhow::bail!("adapter paused; correct credentials and use adapter resume --reason");
        }
        let delivery: Option<DueDelivery> = store.db.query_row(
            "SELECT d.id,d.event_key,d.record_key,d.record_json,d.attempts,COALESCE(d.evidence_version,e.evidence_version),d.record_hash FROM deliveries d JOIN events e USING(event_key) WHERE platform=?1 AND account_id=?2 AND state IN ('pending','prepared','sending','retry') AND next_attempt<=?3 ORDER BY CASE WHEN attempts>0 THEN 0 ELSE 1 END,d.id LIMIT 1",
            params![publisher.platform(),publisher.account(),now()],
            |r| Ok(DueDelivery { id:r.get(0)?, event:r.get(1)?, key:r.get(2)?, frozen:r.get(3)?, attempts:r.get(4)?, version:r.get(5)?, digest:r.get(6)? })
        ).optional()?;
        let Some(DueDelivery {
            id,
            event,
            key,
            frozen,
            digest,
            attempts,
            version,
        }) = delivery
        else {
            break;
        };
        processed += 1;
        let (key, record) = if let (Some(key), Some(frozen)) = (key, frozen) {
            ensure!(
                digest.as_deref() == Some(hash(&frozen).as_str()),
                "frozen record hash mismatch; inspect delivery {id}"
            );
            (key, serde_json::from_str(&frozen)?)
        } else {
            if let Some(reason) = gate(store, &event, version, config)? {
                hold(store, id, &reason)?;
                continue;
            }
            let obs:String=store.db.query_row("SELECT v.observation FROM events e JOIN application_versions v ON v.dataset_id=e.dataset_id AND v.application_id=e.application_id AND v.version=e.evidence_version WHERE event_key=?1",[&event],|r|r.get(0))?;
            let observation = serde_json::from_str(&obs)?;
            let kind: String = store.db.query_row(
                "SELECT detection_kind FROM events WHERE event_key=?1",
                [&event],
                |r| r.get(0),
            )?;
            let record = match if kind == "building_permit" {
                let serialized: String = store.db.query_row(
                    "SELECT observation FROM permit_event_evidence WHERE event_key=?1",
                    [&event],
                    |r| r.get(0),
                )?;
                publisher
                    .prepare_permit(&observation, &serde_json::from_str::<Permit>(&serialized)?)
            } else {
                publisher.prepare(&observation)
            } {
                Ok(record) => record,
                Err(error) => {
                    let fallback = DeliveryError::Retry {
                        message: format!("prepare announcement: {error}"),
                        after: None,
                    };
                    delivery_error(
                        store,
                        id,
                        publisher,
                        error.downcast_ref::<DeliveryError>().unwrap_or(&fallback),
                        // Blob uploads cannot create posts. Keep evidence reviewable
                        // until the first putRecord attempt actually begins.
                        attempts,
                    )?;
                    failed = true;
                    continue;
                }
            };
            let tx = store.db.transaction()?;
            let last: i64 = tx.query_row(
                "SELECT last_identity_clock FROM adapter_state WHERE platform=?1 AND account_id=?2",
                params![publisher.platform(), publisher.account()],
                |r| r.get(0),
            )?;
            let DeliveryIdentity { key, clock } = publisher.allocate_identity(last)?;
            let serialized = serde_json::to_string(&record)?;
            tx.execute(
                "UPDATE adapter_state SET last_identity_clock=?3 WHERE platform=?1 AND account_id=?2",
                params![publisher.platform(), publisher.account(), clock],
            )?;
            tx.execute("UPDATE deliveries SET record_key=?2,record_json=?3,record_hash=?4,template_version=?5,evidence_version=?6,state='prepared' WHERE id=?1",params![id,key,serialized,hash(&serialized),publisher.template_version(),version])?;
            tx.commit()?;
            (key, record)
        };
        if attempts > 0 {
            match publisher.reconcile(&key, &record) {
                Ok(Reconciliation::Same(receipt)) => {
                    sent(store, id, &receipt)?;
                    continue;
                }
                Ok(Reconciliation::Conflict) => {
                    hold(store, id, "remote record conflict; never overwrite")?;
                    failed = true;
                    continue;
                }
                Ok(Reconciliation::Absent) => {}
                Err(error) => {
                    delivery_error(store, id, publisher, &error, attempts + 1)?;
                    failed = true;
                    continue;
                }
            }
        }
        if let Some(reason) = gate(store, &event, version, config)? {
            hold(store, id, &reason)?;
            continue;
        }
        let last_send: Option<i64> = store.db.query_row(
            "SELECT last_send FROM adapter_state WHERE platform=?1 AND account_id=?2",
            params![publisher.platform(), publisher.account()],
            |r| r.get(0),
        )?;
        if let Some(last) = last_send
            && now() - last < config.min_send_interval_seconds
        {
            break;
        }
        // Persist uncertainty before the external side effect. A killed process resumes with getRecord.
        let tx = store.db.transaction()?;
        tx.execute(
            "UPDATE deliveries SET state='sending',attempts=attempts+1 WHERE id=?1",
            [id],
        )?;
        tx.execute(
            "UPDATE adapter_state SET last_send=?3 WHERE platform=?1 AND account_id=?2",
            params![publisher.platform(), publisher.account(), now()],
        )?;
        tx.execute(
            "INSERT INTO delivery_attempts(delivery_id,at,outcome) VALUES(?1,?2,'started')",
            params![id, now()],
        )?;
        tx.commit()?;
        match publisher.send(&key, &record) {
            Ok(receipt) => sent(store, id, &receipt)?,
            Err(DeliveryError::Conflict) => match publisher.reconcile(&key, &record) {
                Ok(Reconciliation::Same(receipt)) => sent(store, id, &receipt)?,
                Ok(Reconciliation::Conflict) => {
                    hold(store, id, "remote record conflict; never overwrite")?;
                    failed = true;
                }
                Ok(Reconciliation::Absent) => {
                    delivery_error(
                        store,
                        id,
                        publisher,
                        &DeliveryError::Retry {
                            message: "write conflicted but record absent".into(),
                            after: None,
                        },
                        attempts + 1,
                    )?;
                    failed = true;
                }
                Err(error) => {
                    delivery_error(store, id, publisher, &error, attempts + 1)?;
                    failed = true;
                }
            },
            Err(error) => {
                delivery_error(store, id, publisher, &error, attempts + 1)?;
                failed = true;
            }
        }
    }
    publish_replies(
        store,
        config,
        publisher,
        config.max_posts_per_run.saturating_sub(processed),
        &mut failed,
        None,
    )?;
    ensure!(
        !failed,
        "one or more deliveries failed; inspect status and queue"
    );
    Ok(())
}

/// Send one permit reply using maps rendered from the current source snapshot elsewhere.
pub fn publish_prepared_permit_reply(
    store: &mut Store,
    config: &Config,
    publisher: &mut impl Publisher,
    event_key: &str,
    preview: &Path,
) -> Result<()> {
    ensure!(config.publish_enabled, "publishing disabled");
    store.sync_deliveries(config)?;
    let paused: bool = store.db.query_row(
        "SELECT paused FROM adapter_state WHERE platform=?1 AND account_id=?2",
        params![publisher.platform(), publisher.account()],
        |row| row.get(0),
    )?;
    ensure!(!paused, "adapter paused; correct credentials and resume it");
    let mut failed = false;
    publish_replies(
        store,
        config,
        publisher,
        1,
        &mut failed,
        Some((event_key, preview)),
    )?;
    ensure!(
        !failed,
        "prepared permit reply failed; inspect its outbox state"
    );
    let state: String = store.db.query_row(
        "SELECT p.state FROM permit_replies p JOIN deliveries d ON d.event_key=p.event_key AND d.platform=p.platform AND d.account_id=p.account_id WHERE p.event_key=?1 AND p.platform=?2 AND p.account_id=?3 AND d.state='sent'",
        params![event_key, publisher.platform(), publisher.account()],
        |row| row.get(0),
    )?;
    ensure!(
        state == "sent",
        "permit reply remains {state}; retry after send interval"
    );
    Ok(())
}

fn publish_replies(
    store: &mut Store,
    config: &Config,
    publisher: &mut impl Publisher,
    remaining: usize,
    failed: &mut bool,
    prepared: Option<(&str, &Path)>,
) -> Result<()> {
    if remaining == 0 {
        return Ok(());
    }
    store.db.execute("INSERT OR IGNORE INTO permit_replies(event_key,platform,account_id,state) SELECT e.event_key,d.platform,d.account_id,'pending' FROM deliveries d JOIN events e USING(event_key) WHERE e.detection_kind='building_permit' AND e.disposition='pending' AND d.state='sent' AND d.platform=?1 AND d.account_id=?2",params![publisher.platform(),publisher.account()])?;
    for _ in 0..remaining {
        let row:Option<DueReply>=store.db.query_row(
            "SELECT p.id,p.event_key,p.record_key,p.record_json,p.record_hash,p.attempts,d.remote_uri,d.remote_cid,e.disposition FROM permit_replies p JOIN deliveries d ON d.event_key=p.event_key AND d.platform=p.platform AND d.account_id=p.account_id JOIN events e ON e.event_key=p.event_key WHERE p.platform=?1 AND p.account_id=?2 AND p.state IN ('pending','prepared','sending','retry') AND p.next_attempt<=?3 AND d.state='sent' AND (e.disposition='pending' OR p.attempts>0) AND (?4 IS NULL OR p.event_key=?4) ORDER BY CASE WHEN p.attempts>0 THEN 0 ELSE 1 END,p.id LIMIT 1",
            params![publisher.platform(),publisher.account(),now(),prepared.map(|(key,_)|key)],|r|Ok(DueReply{id:r.get(0)?,event:r.get(1)?,key:r.get(2)?,frozen:r.get(3)?,digest:r.get(4)?,attempts:r.get(5)?,root_uri:r.get(6)?,root_cid:r.get(7)?,disposition:r.get(8)?})).optional()?;
        let Some(DueReply {
            id,
            event,
            key,
            frozen,
            digest,
            attempts,
            root_uri,
            root_cid,
            disposition,
        }) = row
        else {
            break;
        };
        let (key, record) = if let (Some(key), Some(frozen)) = (key, frozen) {
            ensure!(
                digest.as_deref() == Some(hash(&frozen).as_str()),
                "frozen permit reply hash mismatch"
            );
            (key, serde_json::from_str::<Value>(&frozen)?)
        } else {
            let (app_serialized,permit_serialized):(String,String)=store.db.query_row("SELECT application_observation,observation FROM permit_event_evidence WHERE event_key=?1",[&event],|r|Ok((r.get(0)?,r.get(1)?)))?;
            let obs: Observation = serde_json::from_str(&app_serialized)?;
            let permit: Permit = serde_json::from_str(&permit_serialized)?;
            let prepared = (|| {
                let snapshot = permits::ward_snapshot(store, &obs.id, &permit.number)?;
                if let Some((_, preview)) = prepared {
                    let (near, ward, mapped_snapshot) = maps::read_prepared(&snapshot, preview)?;
                    publisher.prepare_permit_reply_prepared(
                        &mapped_snapshot,
                        &root_uri,
                        &root_cid,
                        &near,
                        &ward,
                    )
                } else {
                    publisher.prepare_permit_reply(&obs, &permit, &snapshot, &root_uri, &root_cid)
                }
            })();
            let record = match prepared {
                Ok(record) => record,
                Err(error) => {
                    reply_error(
                        store,
                        id,
                        publisher,
                        error
                            .downcast_ref::<DeliveryError>()
                            .unwrap_or(&DeliveryError::Retry {
                                message: format!("prepare permit maps: {error}"),
                                after: None,
                            }),
                        attempts,
                    )?;
                    *failed = true;
                    continue;
                }
            };
            let tx = store.db.transaction()?;
            let last: i64 = tx.query_row(
                "SELECT last_identity_clock FROM adapter_state WHERE platform=?1 AND account_id=?2",
                params![publisher.platform(), publisher.account()],
                |r| r.get(0),
            )?;
            let DeliveryIdentity { key, clock } = publisher.allocate_identity(last)?;
            let serialized = serde_json::to_string(&record)?;
            tx.execute("UPDATE adapter_state SET last_identity_clock=?3 WHERE platform=?1 AND account_id=?2",params![publisher.platform(),publisher.account(),clock])?;
            tx.execute("UPDATE permit_replies SET record_key=?2,record_json=?3,record_hash=?4,state='prepared' WHERE id=?1",params![id,key,serialized,hash(&serialized)])?;
            tx.commit()?;
            (key, record)
        };
        if attempts > 0 {
            match publisher.reconcile(&key, &record) {
                Ok(Reconciliation::Same(receipt)) => {
                    reply_sent(store, id, &receipt)?;
                    continue;
                }
                Ok(Reconciliation::Conflict) => {
                    reply_hold(store, id, "remote reply record conflict")?;
                    *failed = true;
                    continue;
                }
                Ok(Reconciliation::Absent) => {}
                Err(error) => {
                    reply_error(store, id, publisher, &error, attempts + 1)?;
                    *failed = true;
                    continue;
                }
            }
        }
        if disposition != "pending" {
            reply_hold(
                store,
                id,
                "permit event is no longer approved; remote absence reconciled",
            )?;
            continue;
        }
        let last_send: Option<i64> = store.db.query_row(
            "SELECT last_send FROM adapter_state WHERE platform=?1 AND account_id=?2",
            params![publisher.platform(), publisher.account()],
            |r| r.get(0),
        )?;
        if last_send.is_some_and(|t| now() - t < config.min_send_interval_seconds) {
            break;
        }
        let tx = store.db.transaction()?;
        tx.execute(
            "UPDATE permit_replies SET state='sending',attempts=attempts+1 WHERE id=?1",
            [id],
        )?;
        tx.execute(
            "UPDATE adapter_state SET last_send=?3 WHERE platform=?1 AND account_id=?2",
            params![publisher.platform(), publisher.account(), now()],
        )?;
        tx.execute(
            "INSERT INTO permit_reply_attempts(reply_id,at,outcome) VALUES(?1,?2,'started')",
            params![id, now()],
        )?;
        tx.commit()?;
        match publisher.send(&key, &record) {
            Ok(receipt) => reply_sent(store, id, &receipt)?,
            Err(DeliveryError::Conflict) => match publisher.reconcile(&key, &record) {
                Ok(Reconciliation::Same(receipt)) => reply_sent(store, id, &receipt)?,
                Ok(Reconciliation::Conflict) => {
                    reply_hold(store, id, "remote reply record conflict")?;
                    *failed = true;
                }
                Ok(Reconciliation::Absent) => {
                    reply_error(
                        store,
                        id,
                        publisher,
                        &DeliveryError::Retry {
                            message: "reply write conflicted but record absent".into(),
                            after: None,
                        },
                        attempts + 1,
                    )?;
                    *failed = true;
                }
                Err(error) => {
                    reply_error(store, id, publisher, &error, attempts + 1)?;
                    *failed = true;
                }
            },
            Err(error) => {
                reply_error(store, id, publisher, &error, attempts + 1)?;
                *failed = true;
            }
        }
    }
    Ok(())
}
fn reply_sent(store: &Store, id: i64, receipt: &Receipt) -> Result<()> {
    ensure!(
        !receipt.uri.is_empty() && !receipt.cid.is_empty(),
        "invalid reply receipt"
    );
    store.db.execute("UPDATE permit_replies SET state='sent',remote_uri=?2,remote_cid=?3,sent_at=?4,last_error=NULL WHERE id=?1",params![id,receipt.uri,receipt.cid,now()])?;
    store.db.execute(
        "INSERT INTO permit_reply_attempts(reply_id,at,outcome) VALUES(?1,?2,'sent')",
        params![id, now()],
    )?;
    Ok(())
}
fn reply_hold(store: &Store, id: i64, reason: &str) -> Result<()> {
    store.db.execute(
        "UPDATE permit_replies SET state='held',last_error=?2 WHERE id=?1",
        params![id, reason],
    )?;
    store.db.execute(
        "INSERT INTO permit_reply_attempts(reply_id,at,outcome,detail) VALUES(?1,?2,'held',?3)",
        params![id, now(), reason],
    )?;
    Ok(())
}
fn reply_error(
    store: &Store,
    id: i64,
    publisher: &impl Publisher,
    error: &DeliveryError,
    attempts: i64,
) -> Result<()> {
    let (state, after) = match error {
        DeliveryError::Auth => {
            store.db.execute(
                "UPDATE adapter_state SET paused=1 WHERE platform=?1 AND account_id=?2",
                params![publisher.platform(), publisher.account()],
            )?;
            ("retry", 3600)
        }
        DeliveryError::Invalid | DeliveryError::Conflict => ("held", 0),
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
    store.db.execute("UPDATE permit_replies SET state=?2,attempts=MAX(attempts,?3),next_attempt=?4,last_error=?5 WHERE id=?1",params![id,state,attempts,now()+after,error.to_string()])?;
    store.db.execute(
        "INSERT INTO permit_reply_attempts(reply_id,at,outcome,detail) VALUES(?1,?2,?3,?4)",
        params![id, now(), state, error.to_string()],
    )?;
    Ok(())
}
fn hold(store: &Store, id: i64, reason: &str) -> Result<()> {
    store.db.execute(
        "UPDATE deliveries SET state='held',last_error=?2 WHERE id=?1",
        params![id, reason],
    )?;
    Ok(())
}
fn sent(store: &Store, id: i64, receipt: &Receipt) -> Result<()> {
    ensure!(
        !receipt.uri.is_empty() && !receipt.cid.is_empty(),
        "invalid delivery receipt"
    );
    store.db.execute("UPDATE deliveries SET state='sent',remote_uri=?2,remote_cid=?3,sent_at=?4,last_error=NULL WHERE id=?1",params![id,receipt.uri,receipt.cid,now()])?;
    store.db.execute(
        "INSERT INTO delivery_attempts(delivery_id,at,outcome) VALUES(?1,?2,'sent')",
        params![id, now()],
    )?;
    eprintln!(
        "{}",
        serde_json::json!({"level":"info","event":"delivery_sent","delivery":id,"uri":receipt.uri})
    );
    Ok(())
}
fn delivery_error(
    store: &Store,
    id: i64,
    publisher: &impl Publisher,
    error: &DeliveryError,
    attempts: i64,
) -> Result<()> {
    let (state, after) = match error {
        DeliveryError::Auth => {
            store.db.execute(
                "UPDATE adapter_state SET paused=1 WHERE platform=?1 AND account_id=?2",
                params![publisher.platform(), publisher.account()],
            )?;
            ("retry", 3600)
        }
        DeliveryError::Invalid | DeliveryError::Conflict => ("held", 0),
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
                after
                    .unwrap_or(0)
                    .max(base + i64::from(rand::random::<u16>() % 31)),
            )
        }
    };
    store.db.execute("UPDATE deliveries SET state=?2,attempts=MAX(attempts,?3),next_attempt=?4,last_error=?5 WHERE id=?1",params![id,state,attempts,now()+after,error.to_string()])?;
    store.db.execute(
        "INSERT INTO delivery_attempts(delivery_id,at,outcome,detail) VALUES(?1,?2,?3,?4)",
        params![id, now(), state, error.to_string()],
    )?;
    Ok(())
}
pub fn resume(store: &Store, config: &Config, reason: &str) -> Result<()> {
    ensure!(!reason.trim().is_empty(), "reason required");
    let did = config
        .bluesky
        .did
        .as_deref()
        .context("configure account DID first")?;
    store.db.execute(
        "UPDATE adapter_state SET paused=0 WHERE platform='bluesky' AND account_id=?1",
        [did],
    )?;
    store.db.execute(
        "UPDATE source_state SET posting_paused=0 WHERE dataset_id=?1",
        [DATASET],
    )?;
    store
        .db
        .execute("UPDATE permit_state SET posting_paused=0", [])?;
    store.db.execute(
        "INSERT INTO issues(kind,detail,at) VALUES('operator_resume',?1,?2)",
        params![reason, now()],
    )?;
    Ok(())
}
