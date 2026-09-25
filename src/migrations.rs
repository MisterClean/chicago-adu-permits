//! Ordered, transactional schema upgrades. Existing databases migrate only explicitly.
use anyhow::{Result, ensure};
use rusqlite::Connection;

pub const CURRENT: i64 = 3;
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/001_initial.sql"),
    include_str!("../migrations/002_scorecards.sql"),
    include_str!("../migrations/003_permits.sql"),
];

pub fn apply(db: &mut Connection) -> Result<()> {
    apply_steps(db, MIGRATIONS)
}

fn apply_steps(db: &mut Connection, steps: &[&str]) -> Result<()> {
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    ensure!(
        (0..=steps.len() as i64).contains(&version),
        "unsupported database version {version}"
    );
    if version == steps.len() as i64 {
        return Ok(());
    }
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    for (index, sql) in steps.iter().enumerate().skip(version as usize) {
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (index + 1) as i64)?;
    }
    ensure!(
        tx.prepare("PRAGMA foreign_key_check")?
            .query([])?
            .next()?
            .is_none(),
        "migration violates foreign keys"
    );
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrades_preserve_rows_and_failed_steps_roll_back() {
        let mut db = Connection::open_in_memory().unwrap();
        let first = "CREATE TABLE records(id INTEGER PRIMARY KEY); INSERT INTO records VALUES(7);";
        apply_steps(&mut db, &[first]).unwrap();
        assert!(
            apply_steps(
                &mut db,
                &[
                    first,
                    "ALTER TABLE records ADD COLUMN note TEXT; INVALID SQL;"
                ]
            )
            .is_err()
        );
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(db.prepare("SELECT note FROM records").is_err());
        apply_steps(
            &mut db,
            &[first, "ALTER TABLE records ADD COLUMN note TEXT;"],
        )
        .unwrap();
        assert_eq!(
            db.query_row("SELECT id FROM records", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            7
        );
        apply_steps(
            &mut db,
            &[first, "ALTER TABLE records ADD COLUMN note TEXT;"],
        )
        .unwrap();
        assert!(apply_steps(&mut db, &[first]).is_err());
    }

    #[test]
    fn scorecard_upgrade_preserves_sent_parent_identity() {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        apply_steps(&mut db, &MIGRATIONS[..1]).unwrap();
        db.execute_batch("INSERT INTO ingest_runs(id,started_at,ended_at,status) VALUES(1,1,1,'success');
            INSERT INTO applications VALUES('j4h8-ug9m','123',1,'{}','hash',1,1,1,1);
            INSERT INTO application_versions VALUES('j4h8-ug9m','123',1,1,'hash','{}',1,NULL,'[]');
            INSERT INTO source_state VALUES('j4h8-ug9m',1,'2026-09-19',1,1,0);
            INSERT INTO events(event_key,dataset_id,application_id,initial_evidence_version,evidence_version,detection_kind,observed_at,disposition,reason) VALUES('event','j4h8-ug9m','123',1,1,'new',1,'pending','test');
            INSERT INTO deliveries(event_key,platform,account_id,state,record_key,record_json,record_hash,remote_uri,remote_cid,sent_at) VALUES('event','bluesky','did:plc:test','sent','tid','{}','digest','at://did:plc:test/app.bsky.feed.post/tid','cid',1);").unwrap();
        let before:String=db.query_row("SELECT json_array(event_key,record_key,record_json,remote_uri,remote_cid) FROM deliveries",[],|r|r.get(0)).unwrap();
        apply(&mut db).unwrap();
        let after:String=db.query_row("SELECT json_array(event_key,record_key,record_json,remote_uri,remote_cid) FROM deliveries",[],|r|r.get(0)).unwrap();
        assert_eq!(before, after);
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM reply_deliveries", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn permit_upgrade_preserves_existing_scorecard_state() {
        let mut db = Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        apply_steps(&mut db, &MIGRATIONS[..2]).unwrap();
        db.execute(
            "INSERT INTO scorecard_state(id,activated_at) VALUES(1,123)",
            [],
        )
        .unwrap();
        apply(&mut db).unwrap();
        let activated: i64 = db
            .query_row(
                "SELECT activated_at FROM scorecard_state WHERE id=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let permit_tables: i64 = db
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('permit_runs','permit_replies')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(activated, 123);
        assert_eq!(permit_tables, 2);
        assert_eq!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            3
        );
    }
}
