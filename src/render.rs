use crate::{
    config::DATASET,
    normalize::{Observation, select, source_date},
};
use anyhow::{Result, ensure};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use unicode_segmentation::UnicodeSegmentation;

pub const TEMPLATE_VERSION: i64 = 3;

/// Only name a specific kind when supported by the source's typed flags.
/// This dataset has no project description or reliable floor designation.
pub fn unit_name(obs: &Observation) -> &'static str {
    let plural = obs.number("adu_applying_for").is_some_and(|n| n > 1);
    if obs.canonical["coach_house"].is_object() || obs.canonical["conversion_unit"].is_object() {
        return if plural {
            "Additional homes"
        } else {
            "Additional home"
        };
    }
    match (
        obs.canonical["coach_house"].as_bool(),
        obs.canonical["conversion_unit"].as_bool(),
    ) {
        (Some(true), Some(true)) => "Coach house + apartments",
        (Some(true), Some(false) | None) => "Coach house",
        (Some(false) | None, Some(true)) if plural => "ADU apartments",
        (Some(false) | None, Some(true)) => "ADU apartment",
        _ if plural => "Additional homes",
        _ => "Additional home",
    }
}
pub fn clean(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
pub fn address(obs: &Observation) -> String {
    obs.text("address")
        .map(clean)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("Application {}", obs.id))
}
pub fn status_date(obs: &Observation) -> Option<String> {
    source_date(&obs.canonical["action_date"]).map(|d| d.format("%b %-d, %Y").to_string())
}
pub fn source_url(obs: &Observation) -> Result<url::Url> {
    let mut link = url::Url::parse(&format!(
        "https://data.cityofchicago.org/resource/{DATASET}.json"
    ))?;
    link.query_pairs_mut()
        .append_pair("$select", &select())
        .append_pair("$where", &format!("id = {}", obs.id));
    Ok(link)
}
pub fn record(obs: &Observation, created_at: DateTime<Utc>) -> Result<Value> {
    let link = source_url(obs)?;
    let label = "Data Portal Record";
    let text = format!("New ADU preapproved\n\n{label}");
    let start = text.len() - label.len();
    Ok(
        json!({"$type":"app.bsky.feed.post","text":text,"createdAt":created_at.to_rfc3339_opts(SecondsFormat::Millis,true),"langs":["en"],"facets":[{"index":{"byteStart":start,"byteEnd":text.len()},"features":[{"$type":"app.bsky.richtext.facet#link","uri":link.as_str()}]}]}),
    )
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
    if let Some(embed) = record.get("embed") {
        ensure!(
            embed["$type"] == "app.bsky.embed.images",
            "unexpected image embed"
        );
        let images = embed["images"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing images"))?;
        ensure!(images.len() == 1, "expected one announcement card");
        for image in images {
            ensure!(
                image["alt"].as_str().is_some_and(|s| !s.trim().is_empty()),
                "image requires alt text"
            );
            ensure!(
                image["image"]["$type"] == "blob"
                    && image["image"]["mimeType"] == "image/jpeg"
                    && image["image"]["size"]
                        .as_u64()
                        .is_some_and(|n| n > 0 && n <= crate::media::MAX_IMAGE_BYTES as u64)
                    && image["image"]["ref"]["$link"]
                        .as_str()
                        .is_some_and(|s| !s.is_empty()),
                "invalid image blob"
            );
            ensure!(
                image["aspectRatio"]["width"] == crate::media::WIDTH
                    && image["aspectRatio"]["height"] == crate::media::HEIGHT,
                "invalid card dimensions"
            );
        }
    }
    Ok(())
}
