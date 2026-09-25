//! Complete, bounded permit scans and conservative links to housing preapprovals.
use crate::{
    config::{Config, PERMIT_DATASET},
    normalize::{Location, Observation, source_date},
    store::{Store, chicago_date, now},
};
use anyhow::{Context, Result, ensure};
use chrono::NaiveDate;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    time::Instant,
};

pub const FIELDS: &[(&str, &str)] = &[
    ("id", "text"),
    ("permit_", "text"),
    ("permit_status", "text"),
    ("permit_type", "text"),
    ("review_type", "text"),
    ("application_start_date", "calendar_date"),
    ("issue_date", "calendar_date"),
    ("street_number", "number"),
    ("street_direction", "text"),
    ("street_name", "text"),
    ("work_description", "text"),
    ("permit_condition", "text"),
    ("reported_cost", "number"),
    ("ward", "number"),
    ("latitude", "number"),
    ("longitude", "number"),
];
const SCOPE: &str = "(upper(work_description) like '%ADU%' OR upper(work_description) like '%ACCESSORY DWELLING%' OR upper(work_description) like '%ADDITIONAL DWELLING%' OR upper(work_description) like '%COACH HOUSE%' OR upper(work_description) like '%CONVERSION UNIT%' OR upper(work_description) like '%DWELLING UNIT%' OR upper(work_description) like '%D.U.%' OR upper(permit_condition) like '%ADU%')";
const SINCE: &str = "2026-04-01";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permit {
    pub source_id: String,
    pub number: String,
    pub fields: Value,
}
impl Permit {
    pub fn parse(raw: Value) -> Result<Self> {
        ensure!(raw.is_object(), "permit row is not an object");
        let source_id = raw["id"]
            .as_str()
            .context("permit source id missing")?
            .to_owned();
        let number = raw["permit_"]
            .as_str()
            .context("permit number missing")?
            .to_owned();
        ensure!(
            source_id.bytes().all(|b| b.is_ascii_alphanumeric()) && !source_id.is_empty(),
            "invalid permit source id"
        );
        ensure!(
            number.bytes().all(|b| b.is_ascii_alphanumeric()) && !number.is_empty(),
            "invalid permit number"
        );
        let mut fields = serde_json::Map::new();
        for &(name, _) in FIELDS {
            if let Some(value) = raw.get(name) {
                ensure!(
                    value.is_string() || value.is_null(),
                    "unexpected permit field type: {name}"
                );
                fields.insert(name.into(), value.clone());
            }
        }
        let fields = Value::Object(fields);
        ensure!(
            source_date(&fields["issue_date"]).is_some(),
            "permit issue date missing or invalid"
        );
        Ok(Self {
            source_id,
            number,
            fields,
        })
    }
    pub fn text(&self, key: &str) -> Option<&str> {
        self.fields[key].as_str()
    }
    pub fn date(&self, key: &str) -> Option<NaiveDate> {
        source_date(&self.fields[key])
    }
    pub fn address(&self) -> String {
        [
            self.text("street_number"),
            self.text("street_direction"),
            self.text("street_name"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
    }
    pub fn url(&self) -> Result<url::Url> {
        let mut url = url::Url::parse(&format!(
            "https://data.cityofchicago.org/resource/{PERMIT_DATASET}.json"
        ))?;
        url.query_pairs_mut().append_pair("permit_", &self.number);
        Ok(url)
    }
}

pub struct PermitMetadata {
    pub revision: String,
    pub schema: Vec<(String, String)>,
}
pub trait PermitSource {
    fn metadata(&mut self) -> Result<PermitMetadata>;
    fn count(&mut self) -> Result<i64>;
    fn page(&mut self, after: Option<&str>, limit: usize) -> Result<Vec<Value>>;
}
pub struct SocrataPermits {
    config: Config,
    agent: ureq::Agent,
}
impl SocrataPermits {
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.clone(),
            agent: config.agent(),
        }
    }
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value> {
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
        let mut response = req
            .call()
            .map_err(|_| anyhow::anyhow!("permit source transport failure"))?;
        ensure!(
            response.status().is_success(),
            "permit source HTTP {}",
            response.status().as_u16()
        );
        let body = response
            .body_mut()
            .with_config()
            .limit(self.config.response_limit)
            .read_to_vec()
            .context("bounded permit response")?;
        serde_json::from_slice(&body).context("invalid permit JSON")
    }
}
impl PermitSource for SocrataPermits {
    fn metadata(&mut self) -> Result<PermitMetadata> {
        let v = self.get(&format!("/api/views/{PERMIT_DATASET}.json"), &[])?;
        let columns = v["columns"]
            .as_array()
            .context("permit metadata missing columns")?;
        let mut schema = Vec::new();
        for &(name, ty) in FIELDS {
            let c = columns
                .iter()
                .find(|c| c["fieldName"] == name)
                .with_context(|| format!("missing permit column {name}"))?;
            ensure!(
                c["dataTypeName"] == ty,
                "permit source type changed for {name}"
            );
            schema.push((name.into(), ty.into()));
        }
        Ok(PermitMetadata {
            revision: v["rowsUpdatedAt"].to_string(),
            schema,
        })
    }
    fn count(&mut self) -> Result<i64> {
        let value = self.get(
            &format!("/resource/{PERMIT_DATASET}.json"),
            &[
                ("$select", "count(*)".into()),
                ("$where", format!("issue_date >= '{SINCE}' AND {SCOPE}")),
            ],
        )?;
        crate::normalize::integer(&value[0]["count"])
    }
    fn page(&mut self, after: Option<&str>, limit: usize) -> Result<Vec<Value>> {
        let select = FIELDS
            .iter()
            .map(|(name, _)| *name)
            .collect::<Vec<_>>()
            .join(",");
        let mut clause = format!("issue_date >= '{SINCE}' AND {SCOPE}");
        if let Some(after) = after {
            ensure!(
                after.bytes().all(|b| b.is_ascii_alphanumeric()),
                "invalid permit cursor"
            );
            clause.push_str(&format!(" AND id > '{after}'"));
        }
        let value = self.get(
            &format!("/resource/{PERMIT_DATASET}.json"),
            &[
                ("$select", select),
                ("$where", clause),
                ("$order", "id ASC".into()),
                ("$limit", limit.to_string()),
            ],
        )?;
        serde_json::from_value(value).context("permit page not an array")
    }
}

fn normalized_address(
    number: &str,
    direction: &str,
    street: &str,
) -> Option<(String, String, String)> {
    let number: String = number.chars().filter(|c| c.is_ascii_digit()).collect();
    let direction = direction.trim().to_ascii_uppercase();
    if number.is_empty() || !matches!(direction.as_str(), "N" | "S" | "E" | "W") {
        return None;
    }
    let normalized = street.to_ascii_uppercase().replace(['.', '-'], " ");
    let tokens = normalized
        .split_whitespace()
        .map(|s| match s {
            "STREET" | "ST" => "ST",
            "AVENUE" | "AVE" => "AVE",
            "BOULEVARD" | "BLVD" => "BLVD",
            "PARKWAY" | "PKWY" => "PKWY",
            "PLACE" | "PL" => "PL",
            "DRIVE" | "DR" => "DR",
            "ROAD" | "RD" => "RD",
            "COURT" | "CT" => "CT",
            "TERRACE" | "TER" => "TER",
            _ => s,
        })
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }
    // Keep suffix and street name together, so 22nd Street cannot become 22nd Place.
    Some((number, direction, tokens.join(" ")))
}
fn app_address(obs: &Observation) -> Option<(String, String, String)> {
    if let (Some(n), Some(d), Some(s)) = (
        obs.text("street_number"),
        obs.text("street_direction"),
        obs.text("street_name"),
    ) && let Some(parts) = normalized_address(n, d, s)
    {
        return Some(parts);
    }
    let address = obs.text("address")?;
    let mut words = address.split_whitespace();
    normalized_address(
        words.next()?,
        words.next()?,
        &words.collect::<Vec<_>>().join(" "),
    )
}
fn explicit_ids(permit: &Permit) -> Vec<String> {
    let combined = format!(
        "{} {}",
        permit.text("work_description").unwrap_or(""),
        permit.text("permit_condition").unwrap_or("")
    );
    let words = combined
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let mut ids = Vec::new();
    for (i, word) in words.iter().enumerate() {
        if word.eq_ignore_ascii_case("ADU") {
            let next = words.get(i + 1).copied().unwrap_or("");
            let id = if next.eq_ignore_ascii_case("ID") || next.eq_ignore_ascii_case("APPLICATION")
            {
                words.get(i + 2).copied().unwrap_or("")
            } else {
                next
            };
            if (5..=8).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit()) {
                ids.push(id.to_owned());
            }
        }
    }
    ids.sort();
    ids.dedup();
    ids
}
fn strong_scope(permit: &Permit) -> bool {
    let s = format!(
        "{} {}",
        permit.text("work_description").unwrap_or(""),
        permit.text("permit_condition").unwrap_or("")
    )
    .to_ascii_uppercase();
    s.split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| word == "ADU")
        || s.contains("ACCESSORY DWELLING")
        || s.contains("ADDITIONAL DWELLING")
        || s.contains("COACH HOUSE")
        || s.contains("CONVERSION UNIT")
}
fn building_permit(permit: &Permit) -> bool {
    matches!(
        permit.text("permit_type"),
        Some("PERMIT - RENOVATION/ALTERATION" | "PERMIT - NEW CONSTRUCTION")
    ) && matches!(permit.text("permit_status"), Some("ACTIVE" | "COMPLETE"))
}
fn edit_distance_at_most_one(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let (mut i, mut j, mut edits) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            i += 1;
            j += 1;
        } else {
            edits += 1;
            if edits > 1 {
                return false;
            }
            if a.len() > b.len() {
                i += 1;
            } else if b.len() > a.len() {
                j += 1;
            } else {
                i += 1;
                j += 1;
            }
        }
    }
    edits + (a.len() - i).max(b.len() - j) <= 1
}
#[derive(Debug, Clone)]
pub struct Match {
    pub application_id: String,
    pub method: &'static str,
    pub score: i64,
    pub reason: &'static str,
}
pub fn candidates(permit: &Permit, apps: &[Observation]) -> Vec<Match> {
    let ids = explicit_ids(permit);
    let permit_addr = normalized_address(
        permit.text("street_number").unwrap_or(""),
        permit.text("street_direction").unwrap_or(""),
        permit.text("street_name").unwrap_or(""),
    );
    let mut out = Vec::new();
    if !building_permit(permit) {
        return out;
    }
    for app in apps.iter().filter(|a| a.status.qualifying()) {
        let Some((an, ad, as_)) = app_address(app) else {
            continue;
        };
        let Some((pn, pd, ps)) = &permit_addr else {
            continue;
        };
        if &an != pn || &ad != pd {
            continue;
        }
        let exact = &as_ == ps;
        let fuzzy = !exact && edit_distance_at_most_one(&as_, ps);
        if !exact && !fuzzy {
            continue;
        }
        let explicit = ids.iter().any(|id| id == &app.id);
        if !ids.is_empty() && !explicit {
            continue;
        }
        let (score, method, reason) = if explicit {
            (100, "explicit_adu_id", "ADU ID and address agree")
        } else if exact && strong_scope(permit) {
            (85, "address_scope", "exact address and explicit ADU scope")
        } else if exact {
            (
                65,
                "address_dwellings",
                "exact address; ADU scope needs review",
            )
        } else {
            (55, "fuzzy_address", "street spelling differs; needs review")
        };
        out.push(Match {
            application_id: app.id.clone(),
            method,
            score,
            reason,
        });
    }
    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.application_id.cmp(&b.application_id))
    });
    out
}

pub fn event_key(application_id: &str, permit_number: &str) -> String {
    format!("chicago:j4h8-ug9m:{application_id}:building-permit:{permit_number}:v1")
}

pub fn ingest(store: &mut Store, source: &mut impl PermitSource, config: &Config) -> Result<()> {
    let start = now();
    store.db.execute(
        "INSERT INTO permit_runs(started_at,status) VALUES(?1,'fetching')",
        [start],
    )?;
    let run = store.db.last_insert_rowid();
    let result = fetch_and_promote(store, source, config, run, start);
    if let Err(error) = &result {
        store.db.execute("UPDATE permit_runs SET status='failed',ended_at=?2,failure=?3 WHERE id=?1 AND status='fetching'",params![run,now(),format!("{error:#}")])?;
    }
    result
}
fn fetch_and_promote(
    store: &mut Store,
    source: &mut impl PermitSource,
    config: &Config,
    run: i64,
    start: i64,
) -> Result<()> {
    let clock = Instant::now();
    let before = source.metadata()?;
    ensure!(
        !before.revision.is_empty() && before.revision != "null",
        "permit revision missing"
    );
    let count = source.count()?;
    ensure!(count > 0, "empty permit candidate feed");
    store.db.execute(
        "UPDATE permit_runs SET revision_before=?2,count_before=?3 WHERE id=?1",
        params![run, before.revision, count],
    )?;
    let mut after: Option<String> = None;
    let mut fetched = 0_i64;
    loop {
        ensure!(
            clock.elapsed().as_secs() < config.max_run_seconds,
            "permit ingestion time budget exhausted"
        );
        let page = source.page(after.as_deref(), config.page_size)?;
        ensure!(page.len() <= config.page_size, "permit page exceeds limit");
        let len = page.len();
        let tx = store.db.transaction()?;
        for row in page {
            let permit = Permit::parse(row)?;
            ensure!(
                after
                    .as_deref()
                    .is_none_or(|last| permit.source_id.as_str() > last),
                "duplicate or unordered permit id"
            );
            after = Some(permit.source_id.clone());
            tx.execute(
                "INSERT INTO staged_permits VALUES(?1,?2,?3)",
                params![run, permit.source_id, serde_json::to_string(&permit)?],
            )?;
        }
        tx.execute(
            "UPDATE permit_runs SET fetched_rows=fetched_rows+?2 WHERE id=?1",
            params![run, len as i64],
        )?;
        tx.commit()?;
        fetched += len as i64;
        ensure!(fetched <= count, "permit scan exceeded count");
        if len < config.page_size {
            break;
        }
    }
    let end = source.metadata()?;
    let final_count = source.count()?;
    store.db.execute(
        "UPDATE permit_runs SET revision_after=?2,count_after=?3 WHERE id=?1",
        params![run, end.revision, final_count],
    )?;
    ensure!(
        before.revision == end.revision && before.schema == end.schema,
        "permit revision or schema changed during scan"
    );
    let staged: i64 = store.db.query_row(
        "SELECT count(*) FROM staged_permits WHERE run_id=?1",
        [run],
        |r| r.get(0),
    )?;
    ensure!(
        fetched == count && fetched == final_count && fetched == staged,
        "incomplete permit scan"
    );
    let prior: i64 =
        store
            .db
            .query_row("SELECT count(*) FROM permits WHERE present=1", [], |r| {
                r.get(0)
            })?;
    ensure!(
        prior - fetched <= 10.max(prior / 20),
        "large permit feed decrease; prior snapshot retained"
    );
    promote(store, run, start, config.bluesky.did.as_deref())
}
fn promote(store: &mut Store, run: i64, at: i64, account: Option<&str>) -> Result<()> {
    let tx = store.db.transaction()?;
    let baseline: Option<i64> = tx
        .query_row(
            "SELECT baseline_run FROM permit_state WHERE id=1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let baseline_date = baseline
        .map(|id| {
            tx.query_row(
                "SELECT started_at FROM permit_runs WHERE id=?1",
                [id],
                |r| r.get::<_, i64>(0),
            )
        })
        .transpose()?
        .map(chicago_date)
        .transpose()?;
    let today = chicago_date(at)?;
    let mut apps = Vec::new();
    let mut app_versions = HashMap::new();
    {
        let mut stmt = tx.prepare(
            "SELECT application_id,version,observation FROM applications WHERE present=1",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (id, version, serialized) = row?;
            apps.push(serde_json::from_str::<Observation>(&serialized)?);
            app_versions.insert(id, version);
        }
    }
    let mut stmt =
        tx.prepare("SELECT observation FROM staged_permits WHERE run_id=?1 ORDER BY source_id")?;
    let rows = stmt.query_map([run], |r| r.get::<_, String>(0))?;
    for row in rows {
        let permit: Permit = serde_json::from_str(&row?)?;
        let serialized = serde_json::to_string(&permit)?;
        tx.execute("INSERT INTO permits VALUES(?1,?2,?3,1,?4,?4) ON CONFLICT(source_id) DO UPDATE SET observation=excluded.observation,present=1,last_seen=excluded.last_seen",params![permit.source_id,permit.number,serialized,at])?;
        let candidates = candidates(&permit, &apps);
        let ambiguous = candidates.len() > 1 && candidates[0].score == candidates[1].score;
        for candidate in candidates {
            let Some(app) = apps.iter().find(|a| a.id == candidate.application_id) else {
                continue;
            };
            let issue = permit.date("issue_date");
            let preapproved = source_date(&app.canonical["action_date"]);
            let ward_mismatch = permit
                .text("ward")
                .and_then(|s| s.parse::<i64>().ok())
                .is_some_and(|w| app.number("ward").is_some_and(|a| a != w));
            let future_or_reversed = issue.is_none_or(|d| d > today)
                || preapproved.is_none_or(|d| issue.is_some_and(|i| d > i));
            let auto = candidate.score >= 85 && !ambiguous && !future_or_reversed && !ward_mismatch;
            let status = if auto { "confirmed" } else { "proposed" };
            let reason = if ward_mismatch {
                "permit and preapproval wards differ"
            } else if ambiguous {
                "multiple preapprovals at address"
            } else if future_or_reversed {
                "missing, future, or reversed dates"
            } else {
                candidate.reason
            };
            tx.execute("INSERT INTO permit_matches VALUES(?1,?2,?3,?4,?5,?6,?7,?7) ON CONFLICT(application_id,source_id) DO UPDATE SET score=excluded.score,method=excluded.method,last_seen=excluded.last_seen",params![app.id,permit.source_id,candidate.method,candidate.score,status,reason,at])?;
            if candidate.score < 85 || ambiguous {
                continue;
            }
            let key = event_key(&app.id, &permit.number);
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM events WHERE event_key=?1)",
                [&key],
                |r| r.get(0),
            )?;
            if exists {
                continue;
            }
            let version = app_versions[&app.id];
            let historical = baseline_date.is_none()
                || issue.is_some_and(|d| baseline_date.is_some_and(|b| d < b));
            let disposition = if historical {
                "suppressed"
            } else if auto {
                "pending"
            } else {
                "held"
            };
            let event_reason = if historical {
                "historical permit at permit baseline"
            } else {
                reason
            };
            tx.execute("INSERT INTO events(event_key,dataset_id,application_id,initial_evidence_version,evidence_version,detection_kind,observed_at,action_date,disposition,reason) VALUES(?1,'j4h8-ug9m',?2,?3,?3,'building_permit',?4,?5,?6,?7)",params![key,app.id,version,at,permit.text("issue_date"),disposition,event_reason])?;
            tx.execute(
                "INSERT INTO permit_event_evidence VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    key,
                    permit.source_id,
                    permit.number,
                    serialized,
                    serde_json::to_string(app)?,
                    candidate.method
                ],
            )?;
        }
    }
    drop(stmt);
    tx.execute("UPDATE permits SET present=0 WHERE source_id NOT IN (SELECT source_id FROM staged_permits WHERE run_id=?1)",[run])?;
    tx.execute("INSERT INTO permit_state(id,baseline_run,last_successful_run) VALUES(1,?1,?1) ON CONFLICT(id) DO UPDATE SET last_successful_run=excluded.last_successful_run",[run])?;
    tx.execute(
        "UPDATE permit_runs SET status='success',ended_at=?2 WHERE id=?1",
        params![run, at],
    )?;
    if let Some(did) = account {
        tx.execute(
            "INSERT OR IGNORE INTO adapter_state(platform,account_id) VALUES('bluesky',?1)",
            [did],
        )?;
        tx.execute("INSERT OR IGNORE INTO deliveries(event_key,platform,account_id,state) SELECT event_key,'bluesky',?1,disposition FROM events WHERE detection_kind='building_permit'",[did])?;
    }
    tx.execute("DELETE FROM staged_permits WHERE run_id=?1", [run])?;
    tx.commit()?;
    Ok(())
}

pub fn due_ingest(store: &Store, interval: i64) -> Result<bool> {
    let last: Option<i64> = store.db.query_row(
        "SELECT max(ended_at) FROM permit_runs WHERE status='success'",
        [],
        |r| r.get(0),
    )?;
    Ok(last.is_none_or(|t| now() - t >= interval))
}
pub fn status(store: &Store) -> Result<Value> {
    let scalar = |sql| -> Result<i64> { Ok(store.db.query_row(sql, [], |r| r.get(0))?) };
    Ok(
        json!({"baseline_established":scalar("SELECT EXISTS(SELECT 1 FROM permit_state)")?==1,
        "current_permits":scalar("SELECT count(*) FROM permits WHERE present=1")?,
        "proposed_matches":scalar("SELECT count(*) FROM permit_matches WHERE status='proposed'")?,
        "confirmed_matches":scalar("SELECT count(*) FROM permit_matches WHERE status='confirmed'")?,
        "last_success":store.db.query_row("SELECT max(ended_at) FROM permit_runs WHERE status='success'",[],|r|r.get::<_,Option<i64>>(0))?}),
    )
}

pub fn matches(store: &Store) -> Result<Value> {
    let mut stmt=store.db.prepare("SELECT m.application_id,p.permit_number,m.method,m.score,m.status,m.reason,p.observation FROM permit_matches m JOIN permits p ON p.source_id=m.source_id ORDER BY m.application_id,p.permit_number")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (application_id, permit_number, method, score, status, reason, serialized) = row?;
        let permit: Permit = serde_json::from_str(&serialized)?;
        out.push(json!({"application_id":application_id,"permit_number":permit_number,"method":method,"score":score,"status":status,"reason":reason,"issue_date":permit.text("issue_date"),"address":permit.address(),"permit_url":permit.url()?.as_str()}));
    }
    Ok(Value::Array(out))
}

pub fn review_match(
    store: &mut Store,
    application_id: &str,
    permit_number: &str,
    action: &str,
    reason: &str,
    account: Option<&str>,
) -> Result<()> {
    ensure!(!reason.trim().is_empty(), "a review reason is required");
    ensure!(
        matches!(action, "confirm" | "reject"),
        "invalid match review action"
    );
    let tx = store.db.transaction()?;
    let (source_id,prior,method,serialized):(String,String,String,String)=tx.query_row("SELECT m.source_id,m.status,m.method,p.observation FROM permit_matches m JOIN permits p ON p.source_id=m.source_id WHERE m.application_id=?1 AND p.permit_number=?2",params![application_id,permit_number],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).context("permit match not found")?;
    ensure!(
        prior == "proposed" || prior == "confirmed",
        "rejected match cannot be reviewed without fresh evidence"
    );
    let (version,app_serialized,present):(i64,String,bool)=tx.query_row("SELECT version,observation,present FROM applications WHERE application_id=?1 AND dataset_id='j4h8-ug9m'",[application_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let app: Observation = serde_json::from_str(&app_serialized)?;
    ensure!(
        present && app.status.qualifying(),
        "application is no longer preapproved"
    );
    let permit: Permit = serde_json::from_str(&serialized)?;
    let permit_present: bool = tx.query_row(
        "SELECT present FROM permits WHERE source_id=?1",
        [&source_id],
        |r| r.get(0),
    )?;
    ensure!(permit_present, "permit is absent from current scan");
    let key = event_key(application_id, permit_number);
    let attempts: Option<i64> = tx.query_row(
        "SELECT max(attempts) FROM deliveries WHERE event_key=?1",
        [&key],
        |r| r.get(0),
    )?;
    ensure!(
        attempts.unwrap_or(0) == 0,
        "attempted delivery requires queue retry review"
    );
    let status = if action == "confirm" {
        "confirmed"
    } else {
        "rejected"
    };
    tx.execute(
        "UPDATE permit_matches SET status=?3,reason=?4 WHERE application_id=?1 AND source_id=?2",
        params![application_id, source_id, status, reason],
    )?;
    tx.execute("INSERT INTO permit_match_reviews(application_id,source_id,action,reason,at,prior_status,new_status) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![application_id,source_id,action,reason,now(),prior,status])?;
    if action == "confirm" {
        tx.execute("INSERT INTO events(event_key,dataset_id,application_id,initial_evidence_version,evidence_version,detection_kind,observed_at,action_date,disposition,reason) VALUES(?1,'j4h8-ug9m',?2,?3,?3,'building_permit',?4,?5,'pending',?6) ON CONFLICT(event_key) DO UPDATE SET evidence_version=excluded.evidence_version,disposition='pending',reason=excluded.reason",params![key,application_id,version,now(),permit.text("issue_date"),reason])?;
        tx.execute("INSERT INTO permit_event_evidence(event_key,source_id,permit_number,observation,application_observation,match_method) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(event_key) DO UPDATE SET observation=excluded.observation,application_observation=excluded.application_observation,match_method=excluded.match_method",params![key,source_id,permit_number,serialized,app_serialized,method])?;
        if let Some(did) = account {
            tx.execute(
                "INSERT OR IGNORE INTO adapter_state(platform,account_id) VALUES('bluesky',?1)",
                [did],
            )?;
            tx.execute("INSERT OR IGNORE INTO deliveries(event_key,platform,account_id,state) VALUES(?1,'bluesky',?2,'pending')",params![key,did])?;
            tx.execute("UPDATE deliveries SET state='pending',last_error=NULL WHERE event_key=?1 AND account_id=?2 AND attempts=0",params![key,did])?;
        }
    } else {
        tx.execute(
            "UPDATE events SET disposition='suppressed',reason=?2 WHERE event_key=?1",
            params![key, reason],
        )?;
        tx.execute("UPDATE deliveries SET state='suppressed',last_error=?2 WHERE event_key=?1 AND attempts=0 AND state IN ('pending','prepared','held')",params![key,reason])?;
    }
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct PermitMapPoint {
    pub id: String,
    pub address: String,
    /// Number of distinct confirmed issued permits at this preapproval site.
    pub quantity: i64,
    pub location: Location,
}

#[derive(Debug, Clone, Serialize)]
pub struct PermitWardSnapshot {
    pub source_run: i64,
    pub as_of: NaiveDate,
    pub ward: i64,
    pub permits: i64,
    pub sites: i64,
    pub mapped_sites: i64,
    pub city_permits: i64,
    pub rank: i64,
    pub tied: bool,
    pub focus: PermitMapPoint,
    pub points: Vec<PermitMapPoint>,
    pub permit_number: String,
}

struct PermitSite {
    address: String,
    permits: i64,
    location: Option<Location>,
}

fn permit_location(permit: &Permit) -> Option<Location> {
    let location = Location {
        latitude: permit.text("latitude")?.parse().ok()?,
        longitude: permit.text("longitude")?.parse().ok()?,
    };
    location.valid().then_some(location)
}

/// Freeze current, uniquely linked issued permits before preparing a permit reply.
/// One map marker represents a preapproval site; its number is issued permits,
/// not the requested or authorized ADU count.
pub fn ward_snapshot(
    store: &Store,
    focus_application_id: &str,
    focus_permit_number: &str,
) -> Result<PermitWardSnapshot> {
    let (source_run, ended_at): (i64, i64) = store.db.query_row(
        "SELECT r.id,r.ended_at FROM permit_state s JOIN permit_runs r ON r.id=s.last_successful_run WHERE s.id=1 AND r.status='success'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let as_of = chicago_date(ended_at)?;
    let mut stmt = store.db.prepare(
        "SELECT m.application_id,p.observation,a.observation FROM permit_matches m JOIN permits p ON p.source_id=m.source_id JOIN applications a ON a.dataset_id=?1 AND a.application_id=m.application_id WHERE m.status='confirmed' AND p.present=1 AND a.present=1 ORDER BY p.source_id,m.application_id",
    )?;
    let rows = stmt.query_map([crate::config::DATASET], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })?;
    let mut candidates = Vec::new();
    let mut matches_per_permit = HashMap::<String, usize>::new();
    for row in rows {
        let (application_id, permit_json, application_json) = row?;
        let permit: Permit = serde_json::from_str(&permit_json)?;
        let application: Observation = serde_json::from_str(&application_json)?;
        if application.id != application_id
            || !application.status.qualifying()
            || !building_permit(&permit)
            || permit.date("issue_date").is_none_or(|date| date > as_of)
        {
            continue;
        }
        let ward = permit
            .text("ward")
            .and_then(|value| value.parse::<i64>().ok())
            .or_else(|| application.number("ward"))
            .filter(|ward| (1..=50).contains(ward));
        let Some(ward) = ward else { continue };
        *matches_per_permit
            .entry(permit.source_id.clone())
            .or_default() += 1;
        candidates.push((ward, application_id, application, permit));
    }
    let mut sites = BTreeMap::<(i64, String), PermitSite>::new();
    let mut ward_permits = [0_i64; 50];
    let mut focus_ward = None;
    let mut focus_location = None;
    for (ward, application_id, application, permit) in candidates {
        // A permit confirmed against more than one application has no unique site.
        if matches_per_permit[&permit.source_id] != 1 {
            continue;
        }
        ward_permits[(ward - 1) as usize] += 1;
        let location = permit_location(&permit);
        let site = sites
            .entry((ward, application_id.clone()))
            .or_insert_with(|| PermitSite {
                address: application
                    .text("address")
                    .map(str::to_owned)
                    .unwrap_or_else(|| permit.address()),
                permits: 0,
                location: None,
            });
        site.permits += 1;
        if site.location.is_none() {
            site.location = location;
        }
        if application_id == focus_application_id && permit.number == focus_permit_number {
            focus_ward = Some(ward);
            focus_location = location;
            site.location = location;
        }
    }
    let ward = focus_ward.context("focus permit has no unique current preapproval match")?;
    let focus_location = focus_location.context("focus permit has no valid map coordinates")?;
    let permits = ward_permits[(ward - 1) as usize];
    let rank = 1 + ward_permits
        .iter()
        .filter(|&&count| count > permits)
        .count() as i64;
    let tied = ward_permits
        .iter()
        .filter(|&&count| count == permits)
        .count()
        > 1;
    let city_permits = ward_permits.iter().sum();
    let site_count = sites
        .keys()
        .filter(|(site_ward, _)| *site_ward == ward)
        .count() as i64;
    let points: Vec<PermitMapPoint> = sites
        .into_iter()
        .filter(|((site_ward, _), _)| *site_ward == ward)
        .filter_map(|((_, id), site)| {
            site.location.map(|location| PermitMapPoint {
                id,
                address: site.address,
                quantity: site.permits,
                location,
            })
        })
        .collect();
    let focus = points
        .iter()
        .find(|point| point.id == focus_application_id)
        .cloned()
        .context("focus permit site has no map point")?;
    ensure!(
        focus.location == focus_location,
        "focus permit point changed"
    );
    let mapped_sites = points.len() as i64;
    Ok(PermitWardSnapshot {
        source_run,
        as_of,
        ward,
        permits,
        sites: site_count,
        mapped_sites,
        city_permits,
        rank,
        tied,
        focus,
        points,
        permit_number: focus_permit_number.to_owned(),
    })
}
