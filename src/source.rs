use crate::{
    config::{Config, DATASET},
    normalize::{FIELDS, Observation, integer, select},
    store::{Store, now},
};
use anyhow::{Context, Result, ensure};
use rusqlite::params;
use serde_json::Value;
use std::time::Instant;

#[derive(Debug, PartialEq, Eq)]
pub struct Metadata {
    pub revision: String,
    pub schema: Vec<(String, String)>,
}
pub trait Source {
    fn metadata(&mut self) -> Result<Metadata>;
    fn count(&mut self) -> Result<i64>;
    fn page(&mut self, after: Option<i64>, limit: usize) -> Result<Vec<Value>>;
    fn requests(&self) -> usize;
}
pub struct Socrata {
    agent: ureq::Agent,
    config: Config,
    requests: usize,
}
impl Socrata {
    pub fn new(config: &Config) -> Self {
        Self {
            agent: config.agent(),
            config: config.clone(),
            requests: 0,
        }
    }
    fn get(&mut self, path: &str, query: &[(&str, String)]) -> Result<Value> {
        self.requests += 1;
        let mut url = url::Url::parse(&format!(
            "{}{}",
            self.config.source_base.trim_end_matches('/'),
            path
        ))?;
        url.query_pairs_mut()
            .extend_pairs(query.iter().map(|(k, v)| (*k, v)));
        let mut req = self
            .agent
            .get(url.as_str())
            .header("Accept", "application/json");
        if let Ok(token) = std::env::var("SOCRATA_APP_TOKEN") {
            req = req.header("X-App-Token", token);
        }
        let mut res = req
            .call()
            .map_err(|_| anyhow::anyhow!("Socrata transport failure"))?;
        ensure!(
            res.status().is_success(),
            "Socrata HTTP {}",
            res.status().as_u16()
        );
        let body = res
            .body_mut()
            .with_config()
            .limit(self.config.response_limit)
            .read_to_vec()
            .context("bounded source response")?;
        serde_json::from_slice(&body).context("invalid source JSON")
    }
}
impl Source for Socrata {
    fn metadata(&mut self) -> Result<Metadata> {
        let value = self.get(&format!("/api/views/{DATASET}.json"), &[])?;
        let columns = value["columns"]
            .as_array()
            .context("metadata has no columns")?;
        let mut schema = Vec::new();
        for &(name, ty) in FIELDS {
            let column = columns
                .iter()
                .find(|c| c["fieldName"] == name)
                .with_context(|| format!("missing source column {name}"))?;
            ensure!(
                column["dataTypeName"] == ty,
                "source type changed for {name}"
            );
            schema.push((name.into(), ty.into()));
        }
        let revision = value
            .get("rowsUpdatedAt")
            .filter(|v| !v.is_null())
            .context("missing source revision")?
            .to_string();
        Ok(Metadata { revision, schema })
    }
    fn count(&mut self) -> Result<i64> {
        let value = self.get(
            &format!("/resource/{DATASET}.json"),
            &[("$select", "count(*)".into())],
        )?;
        integer(&value[0]["count"])
    }
    fn page(&mut self, after: Option<i64>, limit: usize) -> Result<Vec<Value>> {
        let mut query = vec![
            ("$select", select()),
            ("$order", "id ASC".into()),
            ("$limit", limit.to_string()),
        ];
        if let Some(id) = after {
            query.push(("$where", format!("id > {id}")));
        }
        let value = self.get(&format!("/resource/{DATASET}.json"), &query)?;
        serde_json::from_value(value).context("source page is not an array")
    }
    fn requests(&self) -> usize {
        self.requests
    }
}
pub fn ingest(store: &mut Store, source: &mut impl Source, config: &Config) -> Result<()> {
    let run = store.begin_run()?;
    let result = fetch_and_promote(store, source, config, run);
    store.db.execute(
        "UPDATE ingest_runs SET requests=?2 WHERE id=?1",
        params![run, source.requests() as i64],
    )?;
    if let Err(e) = &result {
        store.db.execute("UPDATE ingest_runs SET status='failed',ended_at=?2,failure=?3 WHERE id=?1 AND status='fetching'",params![run,now(),format!("{e:#}")])?;
    }
    result
}
fn fetch_and_promote(
    store: &mut Store,
    source: &mut impl Source,
    config: &Config,
    run: i64,
) -> Result<()> {
    let started = Instant::now();
    let before = source.metadata()?;
    let count = source.count()?;
    ensure!(count >= 0, "negative count");
    store.db.execute(
        "UPDATE ingest_runs SET revision_before=?2,count_before=?3 WHERE id=?1",
        params![run, before.revision, count],
    )?;
    let mut after = None;
    let mut fetched = 0_i64;
    loop {
        ensure!(
            started.elapsed().as_secs() < config.max_run_seconds,
            "ingestion time budget exhausted"
        );
        let values = source.page(after, config.page_size)?;
        ensure!(
            values.len() <= config.page_size,
            "source exceeded page size"
        );
        let length = values.len();
        let mut page = Vec::with_capacity(length);
        for value in values {
            let obs = Observation::parse(value)?;
            let id = obs.id.parse::<i64>()?;
            ensure!(
                after.is_none_or(|last| id > last),
                "duplicate or disordered application IDs"
            );
            after = Some(id);
            page.push(obs);
        }
        fetched += length as i64;
        ensure!(fetched <= count, "fetched more rows than initial count");
        store.stage(run, &page)?;
        if length < config.page_size {
            break;
        }
    }
    let end = source.metadata()?;
    let final_count = source.count()?;
    store.db.execute(
        "UPDATE ingest_runs SET revision_after=?2,count_after=?3 WHERE id=?1",
        params![run, end.revision, final_count],
    )?;
    ensure!(
        before == end,
        "source revision or schema changed during scan"
    );
    let staged: i64 = store.db.query_row(
        "SELECT count(*) FROM staged_applications WHERE run_id=?1",
        [run],
        |r| r.get(0),
    )?;
    ensure!(
        fetched == count && fetched == final_count && fetched == staged,
        "incomplete snapshot: counts disagree"
    );
    let prior: i64 = store.db.query_row(
        "SELECT count(*) FROM applications WHERE present=1",
        [],
        |r| r.get(0),
    )?;
    ensure!(
        prior == 0 || fetched > 0,
        "empty snapshot after nonempty baseline"
    );
    ensure!(
        prior - fetched <= 10.max((prior * 5) / 100),
        "large unexplained source decrease; snapshot retained for inspection"
    );
    store.promote_for_account(run, now(), config.bluesky.did.as_deref())?;
    eprintln!(
        "{}",
        serde_json::json!({"level":"info","event":"ingest_complete","run":run,"rows":fetched,"requests":source.requests()})
    );
    Ok(())
}
