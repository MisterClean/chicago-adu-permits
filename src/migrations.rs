//! Ordered, transactional schema upgrades. Existing databases migrate only explicitly.
use anyhow::{Result, ensure};
use rusqlite::Connection;

pub const CURRENT: i64 = 2;
const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/001_initial.sql"),
    include_str!("../migrations/002_permits.sql"),
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
}
