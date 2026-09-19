use crate::{
    config::{Config, DATASET},
    events,
    normalize::Observation,
};
use anyhow::{Context, Result, ensure};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::America::Chicago;
use fs2::FileExt;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
};

pub fn now() -> i64 {
    Utc::now().timestamp()
}
pub fn chicago_date(timestamp: i64) -> Result<NaiveDate> {
    Ok(DateTime::from_timestamp(timestamp, 0)
        .context("invalid clock")?
        .with_timezone(&Chicago)
        .date_naive())
}
pub struct Store {
    pub db: Connection,
    _lock: File,
}
impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join("writer.lock"))?;
        FileExt::try_lock_exclusive(&lock)
            .context("another adu-bot command owns this state directory")?;
        let mut db = Connection::open(dir.join("adu.sqlite3"))?;
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA cache_size=-2048; PRAGMA temp_store=FILE; PRAGMA mmap_size=0; PRAGMA busy_timeout=5000;")?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        ensure!(version <= 1, "database is newer than this binary");
        if version == 0 {
            let tx = db.transaction()?;
            tx.execute_batch(include_str!("../migrations/001_initial.sql"))?;
            tx.commit()?;
        }
        db.execute("UPDATE ingest_runs SET status='failed', ended_at=?1, failure='interrupted before promotion' WHERE status='fetching'", [now()])?;
        db.execute("DELETE FROM staged_applications WHERE run_id IN (SELECT id FROM ingest_runs WHERE status='failed' AND started_at < ?1)", [now()-7*86400])?;
        Ok(Self { db, _lock: lock })
    }
    pub fn begin_run(&self) -> Result<i64> {
        self.db.execute(
            "INSERT INTO ingest_runs(started_at,status) VALUES(?1,'fetching')",
            [now()],
        )?;
        Ok(self.db.last_insert_rowid())
    }
    pub fn stage(&mut self, run: i64, page: &[Observation]) -> Result<()> {
        let tx = self.db.transaction()?;
        for obs in page {
            tx.execute(
                "INSERT INTO staged_applications VALUES(?1,?2,?3,?4)",
                params![run, obs.id, serde_json::to_string(obs)?, obs.hash],
            )?;
        }
        tx.execute(
            "UPDATE ingest_runs SET fetched_rows=fetched_rows+?2 WHERE id=?1",
            params![run, page.len() as i64],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn application(&self, id: &str) -> Result<Observation> {
        let s: String = self.db.query_row(
            "SELECT observation FROM applications WHERE dataset_id=?1 AND application_id=?2",
            params![DATASET, id],
            |r| r.get(0),
        )?;
        Ok(serde_json::from_str(&s)?)
    }
    pub fn promote(&mut self, run: i64, timestamp: i64) -> Result<()> {
        self.promote_for_account(run, timestamp, None)
    }
    pub fn promote_for_account(
        &mut self,
        run: i64,
        timestamp: i64,
        account: Option<&str>,
    ) -> Result<()> {
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let baseline: Option<String> = tx
            .query_row(
                "SELECT baseline_date FROM source_state WHERE dataset_id=?1",
                [DATASET],
                |r| r.get(0),
            )
            .optional()?;
        let baseline_date = baseline
            .as_deref()
            .map(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d"))
            .transpose()?;
        let today = chicago_date(timestamp)?;
        let mut digest = Sha256::new();
        let mut changed = 0;
        {
            let mut stmt = tx.prepare("SELECT observation FROM staged_applications WHERE run_id=?1 ORDER BY CAST(application_id AS INTEGER)")?;
            let mut rows = stmt.query([run])?;
            while let Some(row) = rows.next()? {
                let serialized: String = row.get(0)?;
                let obs: Observation = serde_json::from_str(&serialized)?;
                digest.update(format!("{}:{}\n", obs.id, obs.hash).as_bytes());
                let prior: Option<(i64,String,bool)> = tx.query_row("SELECT version,observation,present FROM applications WHERE dataset_id=?1 AND application_id=?2", params![DATASET,obs.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
                let previous = prior
                    .as_ref()
                    .map(|(_, s, _)| serde_json::from_str::<Observation>(s))
                    .transpose()?;
                let is_changed = previous.as_ref().is_none_or(|p| p.hash != obs.hash)
                    || prior.as_ref().is_some_and(|p| !p.2);
                let version = prior.as_ref().map_or(1, |p| p.0 + i64::from(is_changed));
                tx.execute("INSERT INTO applications VALUES(?1,?2,?3,?4,?5,1,?6,?6,?7) ON CONFLICT(dataset_id,application_id) DO UPDATE SET version=excluded.version,observation=CASE WHEN applications.version != excluded.version OR applications.present=0 THEN excluded.observation ELSE applications.observation END,content_hash=excluded.content_hash,present=1,last_seen=excluded.last_seen,changed_run=CASE WHEN applications.version != excluded.version THEN excluded.changed_run ELSE applications.changed_run END", params![DATASET,obs.id,version,serialized,obs.hash,timestamp,run])?;
                if is_changed {
                    changed += 1;
                    let mut fields: Vec<&str> = obs
                        .canonical
                        .as_object()
                        .context("canonical object")?
                        .keys()
                        .filter(|k| {
                            previous
                                .as_ref()
                                .is_none_or(|p| p.canonical[*k] != obs.canonical[*k])
                        })
                        .map(String::as_str)
                        .collect();
                    if prior.as_ref().is_some_and(|p| !p.2) {
                        fields.push("present");
                    }
                    tx.execute(
                        "INSERT INTO application_versions VALUES(?1,?2,?3,?4,?5,?6,1,?7,?8)",
                        params![
                            DATASET,
                            obs.id,
                            version,
                            run,
                            obs.hash,
                            serialized,
                            prior.as_ref().map(|p| p.0),
                            serde_json::to_string(&fields)?
                        ],
                    )?;
                    let event_key = events::key(&obs.id);
                    if let Some(p) = &previous
                        && p.post_facts() != obs.post_facts()
                    {
                        let exists: bool = tx.query_row(
                            "SELECT EXISTS(SELECT 1 FROM events WHERE event_key=?1)",
                            [&event_key],
                            |r| r.get(0),
                        )?;
                        if exists {
                            tx.execute("INSERT OR IGNORE INTO issues(application_id,run_id,event_key,kind,detail,at) VALUES(?1,?2,?3,'correction','post facts changed; inspect sent and unsent deliveries',?4)", params![obs.id,run,event_key,timestamp])?;
                            tx.execute("UPDATE deliveries SET state='held',last_error='post facts changed' WHERE event_key=?1 AND attempts=0 AND state IN ('pending','prepared')", [&event_key])?;
                            tx.execute("UPDATE events SET disposition='held',reason='post facts changed' WHERE event_key=?1 AND disposition='pending'", [&event_key])?;
                        }
                    }
                    for issue in &obs.issues {
                        tx.execute("INSERT OR IGNORE INTO issues(application_id,run_id,kind,detail,at) VALUES(?1,?2,?3,?3,?4)", params![obs.id,run,issue,timestamp])?;
                    }
                }
                if let Some(decision) = events::decide(
                    &obs,
                    previous.as_ref().map(|p| p.status),
                    baseline_date,
                    today,
                ) {
                    tx.execute("INSERT OR IGNORE INTO events(event_key,dataset_id,application_id,initial_evidence_version,evidence_version,detection_kind,observed_at,action_date,disposition,reason) VALUES(?1,?2,?3,?4,?4,?5,?6,?7,?8,?9)", params![events::key(&obs.id),DATASET,obs.id,version,decision.kind,timestamp,obs.text("action_date"),decision.disposition,decision.reason])?;
                }
            }
        }
        let missing: i64 = tx.query_row("SELECT count(*) FROM applications a WHERE dataset_id=?1 AND present=1 AND NOT EXISTS(SELECT 1 FROM staged_applications s WHERE s.run_id=?2 AND s.application_id=a.application_id)", params![DATASET,run], |r|r.get(0))?;
        tx.execute("INSERT INTO application_versions SELECT dataset_id,application_id,version+1,?2,content_hash,observation,0,version,'[\"present\"]' FROM applications a WHERE dataset_id=?1 AND present=1 AND NOT EXISTS(SELECT 1 FROM staged_applications s WHERE s.run_id=?2 AND s.application_id=a.application_id)", params![DATASET,run])?;
        tx.execute("INSERT INTO issues(application_id,run_id,event_key,kind,detail,at) SELECT a.application_id,?2,e.event_key,'absence','missing from complete snapshot',?3 FROM applications a JOIN events e USING(dataset_id,application_id) WHERE a.dataset_id=?1 AND a.present=1 AND NOT EXISTS(SELECT 1 FROM staged_applications s WHERE s.run_id=?2 AND s.application_id=a.application_id)", params![DATASET,run,timestamp])?;
        tx.execute("UPDATE applications SET version=version+1,present=0,changed_run=?2 WHERE dataset_id=?1 AND present=1 AND NOT EXISTS(SELECT 1 FROM staged_applications s WHERE s.run_id=?2 AND s.application_id=applications.application_id)", params![DATASET,run])?;
        tx.execute("UPDATE events SET disposition='held',reason='application absent' WHERE disposition='pending' AND application_id IN (SELECT application_id FROM applications WHERE present=0)", [])?;
        tx.execute("UPDATE deliveries SET state='held',last_error='application absent' WHERE attempts=0 AND state IN ('pending','prepared') AND event_key IN (SELECT event_key FROM events WHERE reason='application absent')", [])?;
        tx.execute("UPDATE ingest_runs SET status='success',ended_at=?2,digest=?3,changed_rows=?4,missing_rows=?5 WHERE id=?1", params![run,timestamp,format!("{:x}",digest.finalize()),changed,missing])?;
        tx.execute("INSERT INTO source_state(dataset_id,baseline_run,baseline_date,last_successful_run,contract_version) VALUES(?1,?2,?3,?2,1) ON CONFLICT(dataset_id) DO UPDATE SET last_successful_run=excluded.last_successful_run", params![DATASET,run,today.to_string()])?;
        if let Some(did) = account {
            tx.execute(
                "INSERT OR IGNORE INTO adapter_state(platform,account_id) VALUES('bluesky',?1)",
                [did],
            )?;
            tx.execute("INSERT OR IGNORE INTO deliveries(event_key,platform,account_id,state) SELECT event_key,'bluesky',?1,disposition FROM events", [did])?;
        }
        tx.execute("DELETE FROM staged_applications WHERE run_id=?1", [run])?;
        tx.commit()?;
        Ok(())
    }
    pub fn sync_deliveries(&self, config: &Config) -> Result<()> {
        if let Some(did) = &config.bluesky.did {
            self.db.execute(
                "INSERT OR IGNORE INTO adapter_state(platform,account_id) VALUES('bluesky',?1)",
                [did],
            )?;
            self.db.execute("INSERT OR IGNORE INTO deliveries(event_key,platform,account_id,state) SELECT event_key,'bluesky',?1,disposition FROM events",[did])?;
        }
        Ok(())
    }
    pub fn due_ingest(&self, seconds: i64) -> Result<bool> {
        let last: Option<i64> = self.db.query_row(
            "SELECT max(ended_at) FROM ingest_runs WHERE status='success'",
            [],
            |r| r.get(0),
        )?;
        Ok(last.is_none_or(|t| now() - t >= seconds))
    }
    pub fn backup(&self, destination: &Path) -> Result<()> {
        ensure!(!destination.exists(), "backup destination already exists");
        self.db.backup("main", destination, None)?;
        let backup = Connection::open(destination)?;
        // A restored backup must never silently resume public writes.
        backup.execute("UPDATE source_state SET posting_paused=1", [])?;
        let result: String = backup.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        ensure!(result == "ok", "backup integrity check failed");
        Ok(())
    }
    pub fn status(&self) -> Result<Value> {
        let scalar = |sql| -> Result<Value> {
            let v: Option<i64> = self.db.query_row(sql, [], |r| r.get(0))?;
            Ok(json!(v))
        };
        let groups = |table: &str, field: &str| -> Result<Value> {
            let mut stmt = self.db.prepare(&format!(
                "SELECT {field},count(*) FROM {table} GROUP BY {field}"
            ))?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            let mut map = serde_json::Map::new();
            for row in rows {
                let (k, v) = row?;
                map.insert(k, json!(v));
            }
            Ok(Value::Object(map))
        };
        let revision: Option<String> = self.db.query_row("SELECT revision_after FROM ingest_runs WHERE status='success' ORDER BY id DESC LIMIT 1",[],|r|r.get(0)).optional()?.flatten();
        let oldest: Option<i64> = self.db.query_row(
            "SELECT min(e.observed_at) FROM events e WHERE e.disposition='pending' AND (NOT EXISTS(SELECT 1 FROM deliveries d WHERE d.event_key=e.event_key) OR EXISTS(SELECT 1 FROM deliveries d WHERE d.event_key=e.event_key AND d.state NOT IN ('sent','suppressed')))", [], |r| r.get(0))?;
        let last_attempt_status: Option<String> = self
            .db
            .query_row(
                "SELECT status FROM ingest_runs ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let last_failure: Option<String> = self
            .db
            .query_row(
                "SELECT failure FROM ingest_runs ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(
            json!({"last_attempt":scalar("SELECT max(started_at) FROM ingest_runs")?,"last_success":scalar("SELECT max(ended_at) FROM ingest_runs WHERE status='success'")?,
                "baseline_established": self.db.query_row("SELECT EXISTS(SELECT 1 FROM source_state)",[],|r|r.get::<_,bool>(0))?,
                "source_revision":revision,"present":scalar("SELECT count(*) FROM applications WHERE present=1")?,"missing":scalar("SELECT count(*) FROM applications WHERE present=0")?,
                "changed_last_run":scalar("SELECT (SELECT changed_rows FROM ingest_runs WHERE status='success' ORDER BY id DESC LIMIT 1)")?,
                "unknown_statuses":scalar("SELECT count(*) FROM applications WHERE present=1 AND json_extract(observation,'$.status')='unknown'")?,
                "events":groups("events","disposition")?,"deliveries":groups("deliveries","state")?,"issues":groups("issues","kind")?,
                "oldest_pending_observed_at":oldest,
                "oldest_queue_age_seconds":oldest.map(|at|(now()-at).max(0)),
                "last_attempt_status":last_attempt_status,"last_attempt_failure":last_failure,
                "last_successful_post":scalar("SELECT max(sent_at) FROM deliveries")?,
                "posting_paused":scalar("SELECT max(posting_paused) FROM source_state")?,"adapter_paused":scalar("SELECT max(paused) FROM adapter_state")?
            }),
        )
    }
}
