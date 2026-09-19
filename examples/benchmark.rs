//! Reproducible bounded-source benchmark; uses synthetic public-shaped rows only.
use adu_bot::{
    config::Config,
    normalize::FIELDS,
    source::{self, Metadata, Source},
    store::{Store, chicago_date, now},
};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::time::Instant;
struct Generated {
    count: i64,
    requests: usize,
}
impl Source for Generated {
    fn metadata(&mut self) -> Result<Metadata> {
        self.requests += 1;
        Ok(Metadata {
            revision: "synthetic-v1".into(),
            schema: FIELDS
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
        })
    }
    fn count(&mut self) -> Result<i64> {
        self.requests += 1;
        Ok(self.count)
    }
    fn page(&mut self, after: Option<i64>, limit: usize) -> Result<Vec<Value>> {
        self.requests += 1;
        let start = after.unwrap_or(0) + 1;
        let end = self.count.min(start + limit as i64 - 1);
        let date = chicago_date(now())?;
        Ok((start..=end).map(|id|json!({"id":id.to_string(),"submission_date":format!("{date}T00:00:00.000"),"status":"Pre-Certified","address":format!("{id} W EXAMPLE AVE"),"street_number":id.to_string(),"street_direction":"W","street_name":"EXAMPLE AVE","zip":"60601","ward":"1","zoning":"RS-3","coach_house":true,"conversion_unit":false,"adu_applying_for":"1","action_date":format!("{date}T00:00:00.000")})).collect())
    }
    fn requests(&self) -> usize {
        self.requests
    }
}
fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().context("usage: benchmark STATE_DIR [ROWS]")?;
    let rows = args.next().unwrap_or_else(|| "50000".into()).parse()?;
    let mut store = Store::open(std::path::Path::new(&path))?;
    let config = Config::default();
    for label in ["baseline", "replay"] {
        let start = Instant::now();
        source::ingest(
            &mut store,
            &mut Generated {
                count: rows,
                requests: 0,
            },
            &config,
        )?;
        println!(
            "{}",
            json!({"phase":label,"rows":rows,"elapsed_ms":start.elapsed().as_millis(),"sqlite":rusqlite::version()})
        );
    }
    println!("{}", store.status()?);
    Ok(())
}
