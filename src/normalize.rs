use anyhow::{Context, Result, ensure};
use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub const FIELDS: &[(&str, &str)] = &[
    ("id", "number"),
    ("submission_date", "calendar_date"),
    ("status", "text"),
    ("address", "text"),
    ("street_number", "text"),
    ("street_direction", "text"),
    ("street_name", "text"),
    ("unit", "text"),
    ("zip", "text"),
    ("ward", "number"),
    ("zoning", "text"),
    ("coach_house", "checkbox"),
    ("conversion_unit", "checkbox"),
    ("adu_applying_for", "number"),
    ("user_amended_date", "calendar_date"),
    ("action_date", "calendar_date"),
];
pub fn select() -> String {
    FIELDS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(",")
}
pub fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Preapproved,
    Adjustment,
    Canceled,
    Denied,
    InProcess,
    Unknown,
}
impl Status {
    pub fn classify(text: &str) -> Self {
        match text.trim() {
            "Pre-Certified" => Self::Preapproved,
            "Pre-Certified: admin adjust" => Self::Adjustment,
            "Canceled" | "Cancelled" => Self::Canceled,
            "Denied" => Self::Denied,
            "Submitted"
            | "In Review"
            | "Resubmitted"
            | "Request Additional Docs"
            | "Upload affordability receipt"
            | "Upload missing information" => Self::InProcess,
            _ => Self::Unknown,
        }
    }
    pub fn qualifying(self) -> bool {
        matches!(self, Self::Preapproved | Self::Adjustment)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub status: Status,
    pub canonical: Value,
    pub raw: Value,
    pub hash: String,
    pub issues: Vec<String>,
}
// Parse decimal spelling directly; no floating-point round trips, even for JSON numbers.
pub fn integer(v: &Value) -> Result<i64> {
    let text = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => anyhow::bail!("not an integer"),
    };
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    ensure!(
        !whole.is_empty()
            && whole.bytes().all(|b| b.is_ascii_digit())
            && fraction.bytes().all(|b| b == b'0'),
        "not a nonnegative integral decimal"
    );
    whole.parse().context("integer out of range")
}
pub fn source_date(v: &Value) -> Option<NaiveDate> {
    v.as_str()
        .and_then(|s| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok())
        .map(|d| d.date())
}
impl Observation {
    pub fn parse(raw: Value) -> Result<Self> {
        ensure!(raw.is_object(), "source row is not an object");
        let id = integer(&raw["id"])
            .context("invalid application id")?
            .to_string();
        let mut canonical = serde_json::Map::new();
        let mut selected = serde_json::Map::new();
        let mut issues = Vec::new();
        for &(name, ty) in FIELDS {
            let value = raw.get(name).cloned().unwrap_or(Value::Null);
            if raw.get(name).is_some() {
                selected.insert(name.into(), value.clone());
            }
            let normalized = if value.is_null() {
                Value::Null
            } else {
                match ty {
                    "number" => match integer(&value) {
                        Ok(n) => json!(n.to_string()),
                        Err(_) => {
                            issues.push(format!("invalid {name}"));
                            json!({"invalid": value})
                        }
                    },
                    "checkbox" if value.is_boolean() => value,
                    "text" | "calendar_date" if value.is_string() => value,
                    _ => {
                        issues.push(format!("invalid {name}"));
                        json!({"invalid": value})
                    }
                }
            };
            if ty == "calendar_date" && !normalized.is_null() && source_date(&normalized).is_none()
            {
                issues.push(format!("invalid {name}"));
            }
            canonical.insert(name.into(), normalized);
        }
        let canonical = Value::Object(canonical);
        let status = Status::classify(canonical["status"].as_str().unwrap_or(""));
        if status == Status::Unknown {
            issues.push("unknown status".into());
        }
        if let Some(ward) = canonical["ward"]
            .as_str()
            .and_then(|s| s.parse::<u32>().ok())
            && !(1..=50).contains(&ward)
        {
            issues.push("invalid ward".into());
        }
        let hash = hash(&serde_json::to_string(
            &json!({"contract": 1, "fields": canonical}),
        )?);
        Ok(Self {
            id,
            status,
            canonical,
            raw: Value::Object(selected),
            hash,
            issues,
        })
    }
    pub fn number(&self, field: &str) -> Option<i64> {
        self.canonical[field].as_str()?.parse().ok()
    }
    pub fn text(&self, field: &str) -> Option<&str> {
        self.canonical[field].as_str()
    }
    pub fn date_problem(&self, today: NaiveDate) -> bool {
        ["submission_date", "user_amended_date", "action_date"]
            .iter()
            .any(|field| {
                let v = &self.canonical[*field];
                !v.is_null() && source_date(v).is_none_or(|d| d > today)
            })
    }
    pub fn post_facts(&self) -> Value {
        let fields = [
            "status",
            "address",
            "ward",
            "adu_applying_for",
            "coach_house",
            "conversion_unit",
            "action_date",
        ];
        fields
            .iter()
            .map(|key| ((*key).to_string(), self.canonical[*key].clone()))
            .collect()
    }
}
