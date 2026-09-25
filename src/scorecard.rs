//! Source-backed ward facts frozen before a map reply is prepared.
use crate::{
    config::{Config, DATASET},
    normalize::{Location, Observation, Status, source_date},
    store::{Store, chicago_date, now},
};
use anyhow::{Context, Result, ensure};
use chrono::NaiveDate;
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::json;
use unicode_segmentation::UnicodeSegmentation;

pub const TEMPLATE_VERSION: i64 = 1;
const COHORT_START: &str = "2026-04-01";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Point {
    pub id: String,
    pub address: String,
    pub quantity: i64,
    pub location: Location,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub source_run: i64,
    pub as_of: String,
    pub cohort_start: String,
    pub ward: i64,
    pub applications: i64,
    pub adus: i64,
    pub rank: i64,
    pub tied: bool,
    pub city_adus: i64,
    pub city_applications: i64,
    pub mapped_applications: i64,
    pub focus: Point,
    pub points: Vec<Point>,
    pub boundary: Value,
}

impl Snapshot {
    pub fn text(&self) -> String {
        let rank = if self.tied {
            format!("Tied #{}", self.rank)
        } else {
            format!("#{}", self.rank)
        };
        let share = 100.0 * self.adus as f64 / self.city_adus as f64;
        let as_of = NaiveDate::parse_from_str(&self.as_of, "%Y-%m-%d")
            .map_or_else(|_| self.as_of.clone(), |d| d.format("%b %-d").to_string());
        let coverage = if self.mapped_applications == self.applications {
            String::new()
        } else {
            format!(
                "\n{}/{} applications mapped.",
                self.mapped_applications, self.applications
            )
        };
        format!(
            "Ward {} has {} ADUs across {} preapproved applications.\n\n{} of 50 wards by requested ADUs — {:.1}% of the citywide total.\n\nApplications submitted since Apr 1, 2026. As of {}.{}",
            self.ward, self.adus, self.applications, rank, share, as_of, coverage
        )
    }
}

pub fn reply_record(
    snapshot: &Snapshot,
    parent_uri: &str,
    parent_cid: &str,
    created_at: DateTime<Utc>,
) -> Result<Value> {
    ensure!(
        parent_uri.starts_with("at://") && !parent_cid.is_empty(),
        "verified parent reference required"
    );
    let reference = json!({"uri":parent_uri,"cid":parent_cid});
    let record = json!({"$type":"app.bsky.feed.post","text":snapshot.text(),"createdAt":created_at.to_rfc3339_opts(SecondsFormat::Millis,true),"langs":["en"],"facets":[],"reply":{"root":reference,"parent":reference}});
    validate_reply_base(&record)?;
    Ok(record)
}

pub fn validate_reply(record: &Value) -> Result<()> {
    validate_reply_base(record)?;
    let embed = record.get("embed").context("scorecard images required")?;
    ensure!(
        embed["$type"] == "app.bsky.embed.images",
        "invalid scorecard embed"
    );
    let images = embed["images"]
        .as_array()
        .context("missing scorecard images")?;
    ensure!(images.len() == 2, "scorecard needs two images");
    for image in images {
        ensure!(
            image["alt"]
                .as_str()
                .is_some_and(|s| !s.is_empty() && s.len() <= 2000),
            "invalid map alt text"
        );
        ensure!(
            image["image"]["$type"] == "blob"
                && image["image"]["mimeType"] == "image/jpeg"
                && image["image"]["size"]
                    .as_u64()
                    .is_some_and(|n| n > 0 && n <= 2_000_000)
                && image["image"]["ref"]["$link"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty()),
            "invalid map image blob"
        );
        ensure!(
            image["aspectRatio"]["width"] == 2160 && image["aspectRatio"]["height"] == 2160,
            "invalid map dimensions"
        );
    }
    Ok(())
}

fn validate_reply_base(record: &Value) -> Result<()> {
    let text = record["text"].as_str().context("scorecard text required")?;
    ensure!(
        text.graphemes(true).count() <= 300 && text.len() <= 3000,
        "scorecard text too long"
    );
    let reply = &record["reply"];
    ensure!(
        reply["root"] == reply["parent"]
            && reply["root"]["uri"]
                .as_str()
                .is_some_and(|s| s.starts_with("at://"))
            && reply["root"]["cid"].as_str().is_some_and(|s| !s.is_empty()),
        "invalid scorecard thread reference"
    );
    Ok(())
}

pub fn snapshot(store: &Store, config: &Config, parent_delivery_id: i64) -> Result<Snapshot> {
    let focus_id: String = store.db.query_row(
        "SELECT e.application_id FROM deliveries d JOIN events e ON e.event_key=d.event_key WHERE d.id=?1 AND d.state='sent' AND d.remote_uri IS NOT NULL AND d.remote_cid IS NOT NULL",
        [parent_delivery_id], |r|r.get(0),
    ).context("verified parent announcement required")?;
    snapshot_for_application(store, config, &focus_id)
}

pub fn snapshot_for_application(
    store: &Store,
    config: &Config,
    application_id: &str,
) -> Result<Snapshot> {
    snapshot_with_boundary(store, application_id, config.stale_after_seconds, |ward| {
        fetch_boundary(config, ward)
    })
}

fn snapshot_with_boundary(
    store: &Store,
    focus_id: &str,
    stale_after_seconds: i64,
    fetch: impl FnOnce(i64) -> Result<Value>,
) -> Result<Snapshot> {
    let (source_run, ended_at): (i64, i64) = store.db.query_row(
        "SELECT r.id,r.ended_at FROM source_state s JOIN ingest_runs r ON r.id=s.last_successful_run WHERE s.dataset_id=?1 AND r.status='success'",
        [DATASET], |r| Ok((r.get(0)?,r.get(1)?)),
    )?;
    ensure!(
        now() - ended_at <= stale_after_seconds,
        "source snapshot is stale"
    );
    let as_of = chicago_date(ended_at)?.to_string();
    let present: i64 = store.db.query_row(
        "SELECT count(*) FROM applications WHERE dataset_id=?1 AND present=1",
        [DATASET],
        |r| r.get(0),
    )?;
    let mapped: i64 = store.db.query_row("SELECT count(*) FROM map_locations m JOIN applications a USING(dataset_id,application_id) WHERE m.dataset_id=?1 AND a.present=1 AND m.observed_run=?2",params![DATASET,source_run],|r|r.get(0))?;
    ensure!(
        mapped == present,
        "map locations require a complete post-migration scan"
    );
    let mut stmt = store.db.prepare("SELECT a.observation,m.latitude,m.longitude FROM applications a JOIN map_locations m USING(dataset_id,application_id) WHERE a.dataset_id=?1 AND a.present=1 AND m.observed_run=?2")?;
    let mut rows = stmt.query(params![DATASET, source_run])?;
    let mut wards = [(0_i64, 0_i64); 50];
    let mut all_points: Vec<(i64, Point)> = Vec::new();
    let mut focus_ward = None;
    let mut focus = None;
    let mut city_adus = 0_i64;
    let mut city_applications = 0_i64;
    let start = NaiveDate::parse_from_str(COHORT_START, "%Y-%m-%d")?;
    while let Some(row) = rows.next()? {
        let obs: Observation = serde_json::from_str(&row.get::<_, String>(0)?)?;
        if !matches!(obs.status, Status::Preapproved | Status::Adjustment)
            || source_date(&obs.canonical["submission_date"]).is_none_or(|d| d < start)
        {
            continue;
        }
        let ward = obs
            .number("ward")
            .filter(|n| (1..=50).contains(n))
            .context("qualifying application has unknown ward")?;
        let quantity = obs
            .number("adu_applying_for")
            .filter(|n| *n >= 0)
            .context("qualifying application has unknown requested ADUs")?;
        city_adus = city_adus
            .checked_add(quantity)
            .context("city total overflow")?;
        city_applications += 1;
        let count = &mut wards[(ward - 1) as usize];
        count.0 += 1;
        count.1 = count
            .1
            .checked_add(quantity)
            .context("ward total overflow")?;
        let latitude: Option<f64> = row.get(1)?;
        let longitude: Option<f64> = row.get(2)?;
        if let (Some(latitude), Some(longitude)) = (latitude, longitude) {
            let location = Location {
                latitude,
                longitude,
            };
            ensure!(location.valid(), "stored map location is invalid");
            let point = Point {
                id: obs.id.clone(),
                address: obs.text("address").unwrap_or("").to_string(),
                quantity,
                location,
            };
            if obs.id == focus_id {
                focus = Some(point.clone());
            }
            all_points.push((ward, point));
        }
        if obs.id == focus_id {
            focus_ward = Some(ward);
        }
    }
    ensure!(city_adus > 0, "no eligible citywide ADUs");
    let ward = focus_ward.context("parent application is no longer in the scorecard cohort")?;
    let focus = focus.context("parent application has no mapped location")?;
    let (applications, adus) = wards[(ward - 1) as usize];
    let rank = 1 + wards.iter().filter(|(_, n)| *n > adus).count() as i64;
    let tied = wards.iter().filter(|(_, n)| *n == adus).count() > 1;
    let boundary = fetch(ward)?;
    ensure!(
        inside_ward(&boundary, focus.location),
        "parent location lies outside its reported ward"
    );
    let points: Vec<Point> = all_points
        .into_iter()
        .filter(|(w, _)| *w == ward)
        .map(|(_, p)| p)
        .filter(|p| inside_ward(&boundary, p.location))
        .collect();
    let mapped_applications = points.len() as i64;
    Ok(Snapshot {
        source_run,
        as_of,
        cohort_start: COHORT_START.into(),
        ward,
        applications,
        adus,
        rank,
        tied,
        city_adus,
        city_applications,
        mapped_applications,
        focus,
        points,
        boundary,
    })
}

fn fetch_boundary(config: &Config, ward: i64) -> Result<Value> {
    let mut url = url::Url::parse(
        "https://gis.cookcountyil.gov/hosting/rest/services/cookviewer_political_boundaries/MapServer/28/query",
    )?;
    url.query_pairs_mut()
        .append_pair("where", &format!("WARD={ward}"))
        .append_pair("outFields", "WARD")
        .append_pair("returnGeometry", "true")
        .append_pair("f", "geojson")
        .append_pair("outSR", "4326");
    let mut response = config
        .agent()
        .get(url.as_str())
        .call()
        .context("fetch Cook County ward boundary")?;
    ensure!(
        response.status().is_success(),
        "Cook County ward boundary HTTP {}",
        response.status()
    );
    let bytes = response
        .body_mut()
        .with_config()
        .limit(4_000_000)
        .read_to_vec()?;
    let boundary: Value = serde_json::from_slice(&bytes)?;
    let features = boundary["features"]
        .as_array()
        .context("missing ward features")?;
    ensure!(
        features.len() == 1 && features[0]["properties"]["WARD"].as_i64() == Some(ward),
        "unexpected ward boundary"
    );
    ensure!(
        matches!(
            features[0]["geometry"]["type"].as_str(),
            Some("Polygon" | "MultiPolygon")
        ),
        "invalid ward geometry"
    );
    Ok(boundary)
}

fn inside_ward(boundary: &Value, point: Location) -> bool {
    let geometry = &boundary["features"][0]["geometry"];
    let Some(coordinates) = geometry["coordinates"].as_array() else {
        return false;
    };
    match geometry["type"].as_str() {
        Some("Polygon") => polygon_contains(coordinates, point),
        Some("MultiPolygon") => coordinates.iter().any(|p| {
            p.as_array()
                .is_some_and(|rings| polygon_contains(rings, point))
        }),
        _ => false,
    }
}

fn polygon_contains(rings: &[Value], point: Location) -> bool {
    rings.first().is_some_and(|r| ring_contains(r, point))
        && !rings.iter().skip(1).any(|r| ring_contains(r, point))
}

fn ring_contains(ring: &Value, point: Location) -> bool {
    let Some(vertices) = ring.as_array() else {
        return false;
    };
    let mut inside = false;
    for edge in vertices.windows(2) {
        let coords = |p: &Value| Some((p.get(0)?.as_f64()?, p.get(1)?.as_f64()?));
        let (Some((x1, y1)), Some((x2, y2))) = (coords(&edge[0]), coords(&edge[1])) else {
            return false;
        };
        if (y1 > point.latitude) != (y2 > point.latitude)
            && point.longitude < (x2 - x1) * (point.latitude - y1) / (y2 - y1) + x1
        {
            inside = !inside;
        }
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use serde_json::json;

    fn row(id: i64, ward: i64, quantity: i64, location: Option<Location>) -> Observation {
        let mut value = json!({"id":id,"status":"Pre-Certified","submission_date":"2026-09-01T00:00:00.000","action_date":"2026-09-02T00:00:00.000","ward":ward,"adu_applying_for":quantity,"address":format!("{id} W TEST ST")});
        if let Some(point) = location {
            value["latitude"] = json!(point.latitude);
            value["longitude"] = json!(point.longitude);
        }
        Observation::parse(value).unwrap()
    }

    #[test]
    fn scorecard_uses_one_complete_snapshot_and_tied_rank() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        let a = Location {
            latitude: 41.90,
            longitude: -87.60,
        };
        let b = Location {
            latitude: 41.80,
            longitude: -87.65,
        };
        let rows = [
            row(1, 1, 4, Some(a)),
            row(2, 1, 1, None),
            row(3, 2, 5, Some(b)),
            row(4, 3, 5, Some(b)),
            row(5, 1, 0, Some(a)),
        ];
        let run = store.begin_run().unwrap();
        store.stage(run, &rows).unwrap();
        store.promote(run, now()).unwrap();
        let boundary = json!({"type":"FeatureCollection","features":[{"type":"Feature","properties":{"WARD":1},"geometry":{"type":"Polygon","coordinates":[[[-87.7,41.85],[-87.5,41.85],[-87.5,42.0],[-87.7,42.0],[-87.7,41.85]]]}}]});
        let summary = snapshot_with_boundary(&store, "1", 86400, |ward| {
            assert_eq!(ward, 1);
            Ok(boundary)
        })
        .unwrap();
        assert_eq!(
            (
                summary.adus,
                summary.applications,
                summary.city_adus,
                summary.city_applications,
                summary.rank,
                summary.tied,
                summary.mapped_applications
            ),
            (5, 3, 15, 5, 1, true, 2)
        );
        assert!(summary.text().contains("2/3 applications mapped"));
        let reply = reply_record(
            &summary,
            "at://did:plc:test/app.bsky.feed.post/key",
            "cid",
            Utc::now(),
        )
        .unwrap();
        assert_eq!(reply["reply"]["root"], reply["reply"]["parent"]);
        assert!(validate_reply(&reply).is_err());
    }

    #[test]
    fn coordinate_updates_do_not_change_announcement_version() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(dir.path()).unwrap();
        for longitude in [-87.60, -87.61] {
            let run = store.begin_run().unwrap();
            store
                .stage(
                    run,
                    &[row(
                        1,
                        1,
                        1,
                        Some(Location {
                            latitude: 41.90,
                            longitude,
                        }),
                    )],
                )
                .unwrap();
            store.promote(run, now()).unwrap();
        }
        let versions: i64 = store
            .db
            .query_row("SELECT count(*) FROM application_versions", [], |r| {
                r.get(0)
            })
            .unwrap();
        let longitude: f64 = store
            .db
            .query_row("SELECT longitude FROM map_locations", [], |r| r.get(0))
            .unwrap();
        assert_eq!((versions, longitude), (1, -87.61));
    }
}
