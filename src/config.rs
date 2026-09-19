use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub const DATASET: &str = "j4h8-ug9m";

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub state_dir: PathBuf,
    pub source_base: String,
    pub page_size: usize,
    pub response_limit: u64,
    pub timeout_seconds: u64,
    pub ingest_interval_seconds: i64,
    pub stale_after_seconds: i64,
    pub publish_enabled: bool,
    pub max_posts_per_run: usize,
    pub min_send_interval_seconds: i64,
    pub max_run_seconds: u64,
    pub bluesky: BlueskyConfig,
}
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BlueskyConfig {
    pub did: Option<String>,
    pub identifier: Option<String>,
    pub pds: String,
    pub app_password_file: Option<PathBuf>,
}
impl Default for BlueskyConfig {
    fn default() -> Self {
        Self {
            did: None,
            identifier: None,
            pds: "https://bsky.social".into(),
            app_password_file: None,
        }
    }
}
impl Default for Config {
    fn default() -> Self {
        Self {
            state_dir: "state".into(),
            source_base: "https://data.cityofchicago.org".into(),
            page_size: 100,
            response_limit: 2 * 1024 * 1024,
            timeout_seconds: 30,
            ingest_interval_seconds: 21600,
            stale_after_seconds: 86400,
            publish_enabled: false,
            max_posts_per_run: 10,
            min_send_interval_seconds: 60,
            max_run_seconds: 720,
            bluesky: BlueskyConfig::default(),
        }
    }
}
impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let config: Self = match path {
            Some(p) => toml::from_str(&std::fs::read_to_string(p).context("read config")?)?,
            None => Self::default(),
        };
        ensure!(
            (1..=1000).contains(&config.page_size),
            "page_size must be 1..1000"
        );
        ensure!(
            (1024..=2 * 1024 * 1024).contains(&config.response_limit),
            "response_limit must be 1 KiB..2 MiB"
        );
        ensure!(
            config.timeout_seconds > 0 && config.timeout_seconds <= 60,
            "invalid timeout"
        );
        ensure!(
            config.ingest_interval_seconds > 0 && config.stale_after_seconds > 0,
            "invalid intervals"
        );
        ensure!(
            config.min_send_interval_seconds >= 60 && (1..=10).contains(&config.max_posts_per_run),
            "publication caps must be <=10 posts and >=60 seconds apart"
        );
        ensure!(
            (30..=720).contains(&config.max_run_seconds),
            "max_run_seconds must be 30..720"
        );
        secure_url(&config.source_base)?;
        secure_url(&config.bluesky.pds)?;
        if let Some(did) = &config.bluesky.did {
            ensure!(
                did.starts_with("did:") && did.len() < 2048,
                "invalid account DID"
            );
        }
        Ok(config)
    }
    pub fn agent(&self) -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(self.timeout_seconds)))
            .http_status_as_error(false)
            .max_redirects(0)
            .build()
            .into()
    }
}
pub fn secure_url(value: &str) -> Result<()> {
    let u = url::Url::parse(value)?;
    ensure!(
        u.username().is_empty() && u.password().is_none(),
        "credentials cannot be embedded in URLs"
    );
    ensure!(
        u.scheme() == "https"
            || (u.scheme() == "http"
                && matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))),
        "HTTPS required except loopback test servers"
    );
    Ok(())
}
