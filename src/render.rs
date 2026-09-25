use crate::{
    config::DATASET,
    normalize::{Observation, Status, select, source_date},
    permits::{Permit, PermitWardSnapshot},
};
use anyhow::{Result, ensure};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use unicode_segmentation::UnicodeSegmentation;

pub const TEMPLATE_VERSION: i64 = 4;

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
    let mut text = String::from("New ADU preapproved");
    let kind = match (
        obs.canonical["coach_house"].as_bool(),
        obs.canonical["conversion_unit"].as_bool(),
    ) {
        (Some(true), Some(true)) => Some("Coach house + conversion"),
        (_, Some(true)) => Some("Conversion"),
        (Some(true), _) => Some("Coach house"),
        _ => None,
    };
    if let Some(count) = obs.number("adu_applying_for").filter(|&n| n > 0) {
        let units = if count == 1 { "ADU" } else { "ADUs" };
        text.push_str(&format!("\n\n{count} {units} proposed for the property."));
        if let Some(kind) = kind {
            text.push_str(&format!("\nType: {kind}."));
        }
    } else if let Some(kind) = kind {
        text.push_str(&format!("\n\nType: {kind}."));
    }
    // An adjustment's action date may be later than the original preapproval.
    if obs.status == Status::Preapproved
        && let (Some(submitted), Some(preapproved)) = (
            source_date(&obs.canonical["submission_date"]),
            source_date(&obs.canonical["action_date"]),
        )
        && preapproved >= submitted
    {
        let days = (preapproved - submitted).num_days();
        let unit = if days == 1 { "day" } else { "days" };
        text.push_str(&format!("\n\nPreapproved {days} {unit} after submission."));
    }
    text.push_str(&format!("\n\n{label}"));
    let start = text.len() - label.len();
    Ok(
        json!({"$type":"app.bsky.feed.post","text":text,"createdAt":created_at.to_rfc3339_opts(SecondsFormat::Millis,true),"langs":["en"],"facets":[{"index":{"byteStart":start,"byteEnd":text.len()},"features":[{"$type":"app.bsky.richtext.facet#link","uri":link.as_str()}]}]}),
    )
}

fn calendar_days(from: Option<chrono::NaiveDate>, to: Option<chrono::NaiveDate>) -> Option<i64> {
    let (from, to) = (from?, to?);
    (to >= from).then_some((to - from).num_days())
}

pub fn permit_record(
    obs: &Observation,
    permit: &Permit,
    created_at: DateTime<Utc>,
) -> Result<Value> {
    let issued = permit
        .date("issue_date")
        .ok_or_else(|| anyhow::anyhow!("permit issue date missing"))?;
    let address = address(obs);
    let address = if address.chars().count() > 50 {
        format!("Application #{}", obs.id)
    } else {
        address
    };
    let mut lines = vec![
        "ADU building permit issued".to_owned(),
        format!("{} · {}", address, issued.format("%b %-d, %Y")),
    ];
    if let Some(n) = obs.number("adu_applying_for").filter(|n| *n > 0) {
        lines.push(format!(
            "{n} {} proposed",
            if n == 1 { "ADU" } else { "ADUs" }
        ));
    }
    if let Some(cost) = permit
        .text("reported_cost")
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|n| *n > 0. && *n < 100_000_000.)
    {
        let rounded = (cost.round() as i64).to_string();
        let mut grouped = String::new();
        for (i, c) in rounded.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(c);
        }
        lines.push(format!(
            "Reported project cost: ${}",
            grouped.chars().rev().collect::<String>()
        ));
    }
    let preapproved = source_date(&obs.canonical["action_date"]);
    let applied = permit.date("application_start_date");
    for (label, days) in [
        (
            "Preapproval → permit application",
            calendar_days(preapproved, applied),
        ),
        (
            "Permit application → issued",
            calendar_days(applied, Some(issued)),
        ),
        (
            "Preapproval → building permit",
            calendar_days(preapproved, Some(issued)),
        ),
    ] {
        if let Some(days) = days {
            lines.push(format!("{label}: {days} days"));
        }
    }
    let mut text = lines.join("\n");
    let mut facets = Vec::new();
    for (label, link) in [
        ("Permit record", permit.url()?),
        ("Preapproval record", source_url(obs)?),
    ] {
        text.push('\n');
        let start = text.len();
        text.push_str(label);
        facets.push(json!({"index":{"byteStart":start,"byteEnd":text.len()},"features":[{"$type":"app.bsky.richtext.facet#link","uri":link.as_str()}]}));
    }
    let record = json!({"$type":"app.bsky.feed.post","text":text,"createdAt":created_at.to_rfc3339_opts(SecondsFormat::Millis,true),"langs":["en"],"facets":facets});
    validate(&record)?;
    Ok(record)
}

pub fn permit_reply_record(
    summary: &PermitWardSnapshot,
    root_uri: &str,
    root_cid: &str,
    created_at: DateTime<Utc>,
) -> Result<Value> {
    ensure!(
        root_uri.starts_with("at://") && !root_cid.is_empty(),
        "invalid root reference"
    );
    let share = 100. * summary.permits as f64 / summary.city_permits as f64;
    let rank = if summary.tied {
        format!("Tied #{}", summary.rank)
    } else {
        format!("#{}", summary.rank)
    };
    let text = format!(
        "Around the permit + Ward {}\n\n{} issued ADU building permits linked to {} preapproved sites. {} of 50 wards by linked permits ({share:.1}% citywide).\n\n{}/{} sites mapped. As of {}. Dots count permits; locations are approximate.",
        summary.ward,
        summary.permits,
        summary.sites,
        rank,
        summary.mapped_sites,
        summary.sites,
        summary.as_of.format("%b %-d")
    );
    let parent = json!({"uri":root_uri,"cid":root_cid});
    let record = json!({"$type":"app.bsky.feed.post","text":text,"createdAt":created_at.to_rfc3339_opts(SecondsFormat::Millis,true),"langs":["en"],"facets":[],"reply":{"root":parent,"parent":parent}});
    validate(&record)?;
    Ok(record)
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
        ensure!(
            (1..=2).contains(&images.len()),
            "expected one or two announcement images"
        );
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
            let dimensions = (
                image["aspectRatio"]["width"].as_u64(),
                image["aspectRatio"]["height"].as_u64(),
            );
            ensure!(
                matches!(
                    dimensions,
                    (Some(3200), Some(4000)) | (Some(3200), Some(3200))
                ),
                "invalid card dimensions"
            );
        }
    }
    Ok(())
}
