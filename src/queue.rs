use crate::{
    config::DATASET,
    normalize::Observation,
    store::{Store, chicago_date, now},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

pub fn list(store: &Store) -> Result<Value> {
    let mut stmt=store.db.prepare("SELECT e.event_key,e.disposition,e.reason,e.application_id,e.evidence_version,e.detection_kind,p.permit_number FROM events e LEFT JOIN permit_event_evidence p USING(event_key) ORDER BY e.observed_at,e.event_key")?;
    let rows=stmt.query_map([],|r|Ok(json!({"event_key":r.get::<_,String>(0)?,"disposition":r.get::<_,String>(1)?,"reason":r.get::<_,String>(2)?,"application_id":r.get::<_,String>(3)?,"evidence_version":r.get::<_,i64>(4)?,"kind":r.get::<_,String>(5)?,"permit_number":r.get::<_,Option<String>>(6)?})))?;
    Ok(Value::Array(rows.collect::<rusqlite::Result<Vec<_>>>()?))
}
pub fn inspect(store: &Store, key: &str) -> Result<Value> {
    let (id,version,initial,disposition,reason):(String,i64,i64,String,String)=store.db.query_row("SELECT application_id,evidence_version,initial_evidence_version,disposition,reason FROM events WHERE event_key=?1",[key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
    let version_obs = |v| -> Result<Value> {
        let s:String=store.db.query_row("SELECT observation FROM application_versions WHERE dataset_id=?1 AND application_id=?2 AND version=?3",params![DATASET,id,v],|r|r.get(0))?;
        Ok(serde_json::from_str(&s)?)
    };
    let mut stmt=store.db.prepare("SELECT platform,account_id,state,record_key,record_json,attempts,last_error,remote_uri FROM deliveries WHERE event_key=?1")?;
    let deliveries=stmt.query_map([key],|r|Ok(json!({"platform":r.get::<_,String>(0)?,"account":r.get::<_,String>(1)?,"state":r.get::<_,String>(2)?,"record_key":r.get::<_,Option<String>>(3)?,"record":r.get::<_,Option<String>>(4)?,"attempts":r.get::<_,i64>(5)?,"error":r.get::<_,Option<String>>(6)?,"uri":r.get::<_,Option<String>>(7)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt=store.db.prepare("SELECT platform,account_id,state,record_key,record_json,attempts,last_error,remote_uri FROM permit_replies WHERE event_key=?1")?;
    let replies=stmt.query_map([key],|r|Ok(json!({"platform":r.get::<_,String>(0)?,"account":r.get::<_,String>(1)?,"state":r.get::<_,String>(2)?,"record_key":r.get::<_,Option<String>>(3)?,"record":r.get::<_,Option<String>>(4)?,"attempts":r.get::<_,i64>(5)?,"error":r.get::<_,Option<String>>(6)?,"uri":r.get::<_,Option<String>>(7)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt=store.db.prepare("SELECT action,reason,at,evidence_version FROM review_actions WHERE event_key=?1 ORDER BY id")?;
    let actions=stmt.query_map([key],|r|Ok(json!({"action":r.get::<_,String>(0)?,"reason":r.get::<_,String>(1)?,"at":r.get::<_,i64>(2)?,"version":r.get::<_,i64>(3)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let permit:Option<(String,String,String)>=store.db.query_row("SELECT permit_number,match_method,observation FROM permit_event_evidence WHERE event_key=?1",[key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let permit=permit.map(|(number,method,serialized)|Ok::<_,anyhow::Error>(json!({"permit_number":number,"match_method":method,"evidence":serde_json::from_str::<Value>(&serialized)?}))).transpose()?;
    Ok(
        json!({"event_key":key,"disposition":disposition,"reason":reason,"initial_evidence":version_obs(initial)?,"approved_evidence":version_obs(version)?,"permit":permit,"current":store.application(&id)?,"deliveries":deliveries,"replies":replies,"review_actions":actions}),
    )
}
pub fn review(store: &mut Store, key: &str, action: &str, reason: &str) -> Result<()> {
    ensure!(!reason.trim().is_empty(), "a review reason is required");
    let kind: String = store.db.query_row(
        "SELECT detection_kind FROM events WHERE event_key=?1",
        [key],
        |r| r.get(0),
    )?;
    ensure!(
        kind != "building_permit" || action != "approve",
        "use permit-matches confirm to approve a permit link"
    );
    let tx = store.db.transaction()?;
    let (prior, id): (String, String) = tx
        .query_row(
            "SELECT disposition,application_id FROM events WHERE event_key=?1",
            [key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .context("event not found")?;
    let (version,obs,present):(i64,String,bool)=tx.query_row("SELECT version,observation,present FROM applications WHERE dataset_id=?1 AND application_id=?2",params![DATASET,id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let obs: Observation = serde_json::from_str(&obs)?;
    let disposition = match action {
        "approve" => "pending",
        "suppress" => "suppressed",
        "retry" => prior.as_str(),
        _ => anyhow::bail!("invalid action"),
    };
    if action == "approve" {
        ensure!(
            present && obs.status.qualifying(),
            "only present, pre-certified applications can be approved"
        );
        ensure!(
            !obs.date_problem(chicago_date(now())?),
            "repair invalid/future dates before approving"
        );
        let unsafe_change:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM deliveries WHERE event_key=?1 AND attempts>0 AND state!='sent')",[key],|r|r.get(0))?;
        ensure!(
            !unsafe_change,
            "attempted deliveries must be reconciled before changing evidence; use retry"
        );
        tx.execute(
            "UPDATE events SET evidence_version=?2 WHERE event_key=?1",
            params![key, version],
        )?;
        tx.execute("UPDATE deliveries SET state='pending',record_key=NULL,record_json=NULL,record_hash=NULL,template_version=NULL,evidence_version=NULL,next_attempt=0,last_error=NULL WHERE event_key=?1 AND attempts=0",[key])?;
    } else if action == "suppress" {
        tx.execute(
            "UPDATE deliveries SET state='suppressed' WHERE event_key=?1 AND attempts=0",
            [key],
        )?;
        tx.execute(
            "UPDATE permit_replies SET state='suppressed' WHERE event_key=?1 AND attempts=0",
            [key],
        )?;
    } else {
        // Retain identity and frozen payload, including when prior results were ambiguous.
        tx.execute("UPDATE deliveries SET state='retry',next_attempt=0,last_error=NULL WHERE event_key=?1 AND state IN ('failed','retry','sending','held') AND attempts>0",[key])?;
        tx.execute("UPDATE permit_replies SET state='retry',next_attempt=0,last_error=NULL WHERE event_key=?1 AND state IN ('failed','retry','sending','held')",[key])?;
    }
    tx.execute(
        "UPDATE events SET disposition=?2,reason=?3 WHERE event_key=?1",
        params![key, disposition, reason],
    )?;
    tx.execute("INSERT INTO review_actions(event_key,action,reason,at,prior_disposition,new_disposition,evidence_version) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![key,action,reason,now(),prior,disposition,version])?;
    tx.commit()?;
    Ok(())
}
