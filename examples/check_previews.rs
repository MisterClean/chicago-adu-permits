//! Render and validate all current qualifying observations without network access.
use adu_bot::{normalize::Observation, render, store::Store};
use anyhow::{Context, Result};
fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: check_previews STATE_DIR")?;
    let store = Store::open(std::path::Path::new(&path))?;
    let mut statement = store
        .db
        .prepare("SELECT observation FROM applications WHERE present=1")?;
    let mut rows = statement.query([])?;
    let mut count = 0;
    while let Some(row) = rows.next()? {
        let observation: Observation = serde_json::from_str(&row.get::<_, String>(0)?)?;
        if observation.status.qualifying() {
            render::validate(&render::record(&observation, chrono::Utc::now())?)?;
            count += 1;
        }
    }
    println!("Validated {count} qualifying previews without network access.");
    Ok(())
}
