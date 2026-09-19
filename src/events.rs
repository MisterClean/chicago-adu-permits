use crate::normalize::{Observation, Status, source_date};
use chrono::NaiveDate;

pub struct Decision {
    pub disposition: &'static str,
    pub kind: &'static str,
    pub reason: &'static str,
}
pub fn decide(
    current: &Observation,
    previous: Option<Status>,
    baseline: Option<NaiveDate>,
    today: NaiveDate,
) -> Option<Decision> {
    if !current.status.qualifying() {
        return None;
    }
    let Some(baseline) = baseline else {
        return Some(Decision {
            disposition: "suppressed",
            kind: "baseline",
            reason: "suppressed baseline",
        });
    };
    let kind = if previous.is_none() {
        "first_seen_preapproved"
    } else {
        "observed_transition"
    };
    let reason = if current.status == Status::Adjustment {
        Some("administrative adjustment")
    } else if previous == Some(Status::Unknown) {
        Some("unknown prior status")
    } else if current.date_problem(today) {
        Some("invalid or future source date")
    } else if source_date(&current.canonical["action_date"]).is_some_and(|d| d < baseline) {
        Some("possible historical correction")
    } else if previous.is_none() && source_date(&current.canonical["action_date"]).is_none() {
        Some("possible backfill without action date")
    } else {
        None
    };
    Some(Decision {
        disposition: if reason.is_some() { "held" } else { "pending" },
        kind,
        reason: reason.unwrap_or("new qualifying observation"),
    })
}
pub fn key(id: &str) -> String {
    format!("chicago:j4h8-ug9m:{id}:preapproval-first-observed:v1")
}
