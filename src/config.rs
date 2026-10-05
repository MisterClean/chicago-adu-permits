use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub const DATASET: &str = "j4h8-ug9m";
pub const PERMIT_DATASET: &str = "ydr8-5enu";

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub state_dir: PathBuf,
    pub require_existing_state: bool,
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
    pub media: MediaConfig,
    pub scorecards: ScorecardConfig,
    pub maps: MapConfig,
    #[serde(skip)]
    pub render_deadline: Option<std::time::Instant>,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MapBackend {
    #[default]
    LocalChrome,
    Cloudflare,
}
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MapConfig {
    pub backend: MapBackend,
    pub renderer_dir: Option<PathBuf>,
    pub node_bin: Option<PathBuf>,
    pub account_id: Option<String>,
    pub token_file: Option<PathBuf>,
    pub attempt_timeout_seconds: u64,
    pub daily_seconds: u64,
    pub daily_pairs: u64,
    pub cache_ward_backgrounds: bool,
    pub basemap_max_age_days: u64,
}
impl Default for MapConfig {
    fn default() -> Self {
        Self {
            backend: MapBackend::LocalChrome,
            renderer_dir: None,
            node_bin: None,
            account_id: None,
            token_file: None,
            attempt_timeout_seconds: 90,
            daily_seconds: 480,
            daily_pairs: 16,
            cache_ward_backgrounds: true,
            basemap_max_age_days: 7,
        }
    }
}
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScorecardConfig {
    pub enabled: bool,
    pub renderer_dir: PathBuf,
    pub node_bin: PathBuf,
    pub chrome_bin: Option<PathBuf>,
}
impl Default for ScorecardConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            renderer_dir: PathBuf::from("tools/map-preview"),
            node_bin: PathBuf::from("node"),
            chrome_bin: None,
        }
    }
}
#[derive(Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MediaConfig {
    /// Otherwise read GOOGLE_MAPS_API_KEY from the environment.
    pub google_api_key_file: Option<PathBuf>,
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
            require_existing_state: false,
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
            media: MediaConfig::default(),
            scorecards: ScorecardConfig::default(),
            maps: MapConfig::default(),
            render_deadline: None,
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
        ensure!(
            (10..=120).contains(&config.maps.attempt_timeout_seconds),
            "map timeout must be 10..120 seconds"
        );
        ensure!(
            (120..=600).contains(&config.maps.daily_seconds),
            "map daily budget must be 120..600 seconds"
        );
        ensure!(
            (1..=50).contains(&config.maps.daily_pairs),
            "map daily pair cap must be 1..50"
        );
        ensure!(
            (1..=30).contains(&config.maps.basemap_max_age_days),
            "basemap cache age must be 1..30 days"
        );
        if config.maps.backend == MapBackend::Cloudflare {
            ensure!(
                config.maps.daily_seconds >= config.maps.attempt_timeout_seconds + 75,
                "map budget must accommodate one conservative timeout reservation"
            );
            ensure!(
                config
                    .maps
                    .account_id
                    .as_ref()
                    .is_some_and(|s| s.len() == 32
                        && s.bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))),
                "maps.account_id must be a Cloudflare account ID"
            );
            ensure!(
                config.maps.token_file.is_some(),
                "maps.token_file is required for Cloudflare rendering"
            );
        }
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
