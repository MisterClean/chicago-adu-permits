use crate::{
    config::DATASET,
    normalize::{Observation, select, source_date},
};
use anyhow::{Result, ensure};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use unicode_segmentation::UnicodeSegmentation;

pub const TEMPLATE_VERSION: i64 = 1;
pub fn record(obs: &Observation, created_at: DateTime<Utc>) -> Result<Value> {
    let mut link = url::Url::parse(&format!(
        "https://data.cityofchicago.org/resource/{DATASET}.json"
    ))?;
    link.query_pairs_mut()
        .append_pair("$select", &select())
        .append_pair("$where", &format!("id = {}", obs.id));
    let clean = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    for level in 0..=4 {
        let mut lines = vec!["Chicago ADU preapproval".to_string()];
        let mut location = if level < 4 {
            obs.text("address").map(clean).filter(|s| !s.is_empty())
        } else {
            None
        }
        .unwrap_or_else(|| format!("Application {}", obs.id));
        if level < 1
            && let Some(ward) = obs.number("ward").filter(|w| (1..=50).contains(w))
        {
            location.push_str(&format!(" · Ward {ward}"));
        }
        lines.push(location);
        lines.push(format!(
            "City now lists application {} as pre-certified.",
            obs.id
        ));
        let kind = if level < 2
            && !obs.canonical["coach_house"].is_object()
            && !obs.canonical["conversion_unit"].is_object()
        {
            match (
                obs.canonical["coach_house"].as_bool(),
                obs.canonical["conversion_unit"].as_bool(),
            ) {
                (Some(true), Some(true)) => Some("Coach house and conversion units"),
                (Some(true), _) => Some("Coach house"),
                (_, Some(true)) => Some("Conversion units"),
                _ => None,
            }
        } else {
            None
        };
        if let Some(units) = obs.number("adu_applying_for").filter(|n| *n > 0) {
            let mut requested = format!(
                "Requested: {units} {}",
                if units == 1 { "ADU" } else { "ADUs" }
            );
            if let Some(kind) = kind {
                requested.push_str(&format!(" · {kind}"));
            }
            lines.push(requested);
        } else if let Some(kind) = kind {
            lines.push(kind.into());
        }
        let date = if level < 3 {
            source_date(&obs.canonical["action_date"])
        } else {
            None
        };
        lines.push(match date {
            Some(d) => format!(
                "Status dated {}. Building permit still required.",
                d.format("%b %-d, %Y")
            ),
            None => "Building permit still required.".into(),
        });
        lines.push("City record".into());
        let text = lines.join("\n");
        if text.graphemes(true).count() <= 300 && text.len() <= 3000 {
            let start = text.len() - "City record".len();
            return Ok(
                json!({"$type":"app.bsky.feed.post","text":text,"createdAt":created_at.to_rfc3339_opts(SecondsFormat::Millis,true),"langs":["en"],"facets":[{"index":{"byteStart":start,"byteEnd":text.len()},"features":[{"$type":"app.bsky.richtext.facet#link","uri":link.as_str()}]}]}),
            );
        }
    }
    anyhow::bail!("required post text exceeds platform limits")
}
pub fn validate(record: &Value) -> Result<()> {
    let text = record["text"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing text"))?;
    ensure!(
        text.graphemes(true).count() <= 300 && text.len() <= 3000,
        "post too long"
    );
    for facet in record["facets"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing facets"))?
    {
        let start = facet["index"]["byteStart"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("invalid facet"))? as usize;
        let end = facet["index"]["byteEnd"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("invalid facet"))? as usize;
        ensure!(
            start < end
                && end <= text.len()
                && text.is_char_boundary(start)
                && text.is_char_boundary(end),
            "invalid facet bounds"
        );
    }
    Ok(())
}
