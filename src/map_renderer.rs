//! Shared, bounded map transport, exact image cache, and account quota ledger.
use crate::{
    config::{Config, MapBackend},
    media::MAX_IMAGE_BYTES,
    publish::DeliveryError,
    store::now,
    ward_card,
};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use image::ImageReader;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const VERSION: u64 = 1;
const MAX_BASE_BYTES: usize = 8_000_000;
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().context("cache parent")?)?;
    use std::io::Write;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}
fn bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= limit, "map artifact exceeds limit");
    Ok(bytes)
}
#[derive(Debug, Serialize, Deserialize, Default)]
struct Budget {
    day: i64,
    seconds: u64,
    pairs: u64,
    next_acquisition: i64,
    #[serde(default)]
    inflight: bool,
}
impl Budget {
    fn rollover(&mut self, time: i64, reservation: u64) {
        let day = time.div_euclid(86400);
        if self.day != day {
            let debt = if self.inflight { reservation } else { 0 };
            *self = Self {
                day,
                seconds: debt,
                pairs: u64::from(self.inflight),
                next_acquisition: self.next_acquisition,
                ..Self::default()
            };
        }
    }
    fn reserve(&mut self, config: &Config, time: i64, reservation: u64) -> Result<()> {
        self.rollover(time, reservation);
        if time < self.next_acquisition {
            return Err(DeliveryError::Deferred {
                message: "Cloudflare browser acquisition interval".into(),
                until: self.next_acquisition,
            }
            .into());
        }
        if self.pairs >= config.maps.daily_pairs
            || self.seconds.saturating_add(reservation) > config.maps.daily_seconds
        {
            return Err(DeliveryError::Deferred {
                message: "Cloudflare map budget exhausted for this UTC day".into(),
                until: (self.day + 1) * 86400,
            }
            .into());
        }
        self.seconds += reservation;
        self.pairs += 1;
        self.inflight = true;
        self.next_acquisition = time + 20;
        Ok(())
    }
    fn finish(&mut self, start: i64, elapsed: u64, reservation: u64, closed: bool) {
        if closed && start.div_euclid(86400) == now().div_euclid(86400) {
            self.seconds = self
                .seconds
                .saturating_sub(reservation)
                .saturating_add(elapsed);
            self.inflight = false;
        }
        // A crash or uncertain closure retains the full reservation.
    }
}
#[derive(Serialize, Deserialize)]
struct PairManifest {
    version: u64,
    key: String,
    near: String,
    ward: String,
}
#[derive(Serialize, Deserialize)]
struct BackgroundManifest {
    version: u64,
    key: String,
    image: String,
    transform_hash: String,
    camera: Value,
    references: Value,
}

pub fn delivery_failure(error: &anyhow::Error) -> DeliveryError {
    if let Some(e) = error.downcast_ref::<DeliveryError>() {
        return e.clone();
    }
    DeliveryError::Retry {
        message: format!("map rendering: {error:#}"),
        after: Some(900),
    }
}
fn dirs(config: &Config) -> (&Path, &Path) {
    (
        config
            .maps
            .renderer_dir
            .as_deref()
            .unwrap_or(&config.scorecards.renderer_dir),
        config
            .maps
            .node_bin
            .as_deref()
            .unwrap_or(&config.scorecards.node_bin),
    )
}
fn asset_digest(config: &Config) -> Result<String> {
    let (root, _) = dirs(config);
    let mut hash = Sha256::new();
    hash.update(VERSION.to_le_bytes());
    hash.update(include_bytes!("ward_card.rs"));
    hash.update(include_bytes!("media.rs"));
    hash.update(include_bytes!("../Cargo.lock"));
    for (name, alternates) in [
        ("render-live.mjs", vec![root.join("render-live.mjs")]),
        (
            "render-cloudflare.mjs",
            vec![
                root.join("render-cloudflare.mjs"),
                root.join("render-live.mjs"),
            ],
        ),
        ("render-live.js", vec![root.join("render-live.js")]),
        ("base-style", vec![root.join("data/base-style.json")]),
        (
            "maplibre",
            vec![
                root.join("vendor/maplibre-gl.js"),
                root.join("node_modules/maplibre-gl/dist/maplibre-gl.js"),
            ],
        ),
        (
            "headline",
            vec![
                root.join("fonts/BigShouldersText-Bold.ttf"),
                root.join("../../assets/fonts/BigShouldersText-Bold.ttf"),
            ],
        ),
        (
            "body",
            vec![
                root.join("fonts/Roboto.ttf"),
                root.join("../../assets/fonts/Roboto.ttf"),
            ],
        ),
    ] {
        let path = alternates
            .iter()
            .find(|p| p.is_file())
            .with_context(|| format!("missing map asset {name}"))?;
        hash.update(name.as_bytes());
        hash.update(digest(&bounded(path, 10_000_000)?).as_bytes());
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn jpeg(bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() > 1000 && bytes.len() <= MAX_IMAGE_BYTES,
        "map JPEG exceeds upload limit"
    );
    let reader = ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Jpeg);
    ensure!(
        image::guess_format(bytes)? == image::ImageFormat::Jpeg
            && reader.into_dimensions()? == (2160, 2160),
        "invalid map JPEG dimensions or format"
    );
    Ok(())
}
fn background_image(bytes: &[u8]) -> Result<()> {
    ensure!(
        bytes.len() > 1000 && bytes.len() <= MAX_BASE_BYTES,
        "invalid ward background size"
    );
    ensure!(
        image::guess_format(bytes)? == image::ImageFormat::Png
            && ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png)
                .into_dimensions()?
                == (2160, 1600),
        "invalid ward background dimensions"
    );
    Ok(())
}
fn read_pair(dir: &Path, key: &str) -> Result<(Vec<u8>, Vec<u8>)> {
    let m: PairManifest = serde_json::from_slice(&bounded(&dir.join("pair.json"), 4096)?)?;
    ensure!(
        m.version == VERSION && m.key == key,
        "map cache manifest mismatch"
    );
    let near = bounded(&dir.join("n5.jpg"), MAX_IMAGE_BYTES)?;
    let ward = bounded(&dir.join("wc.jpg"), MAX_IMAGE_BYTES)?;
    ensure!(
        digest(&near) == m.near && digest(&ward) == m.ward,
        "map cache checksum mismatch"
    );
    jpeg(&near)?;
    jpeg(&ward)?;
    Ok((near, ward))
}
fn transform_hash(camera: &Value, references: &Value) -> Result<String> {
    Ok(digest(&serde_json::to_vec(
        &json!({"camera":camera,"references":references}),
    )?))
}
fn read_background(dir: &Path, key: &str) -> Result<(Vec<u8>, Value)> {
    let m: BackgroundManifest =
        serde_json::from_slice(&bounded(&dir.join("background.json"), 512_000)?)?;
    ensure!(
        m.version == VERSION && m.key == key,
        "ward cache manifest mismatch"
    );
    let bytes = bounded(&dir.join("ward-base.png"), MAX_BASE_BYTES)?;
    background_image(&bytes)?;
    ensure!(digest(&bytes) == m.image, "ward cache checksum mismatch");
    ensure!(
        m.transform_hash == transform_hash(&m.camera, &m.references)?,
        "ward camera checksum mismatch"
    );
    ward_card::validate_camera(&m.camera, &m.references)?;
    Ok((bytes, m.camera))
}
fn validate_output(
    dir: &Path,
    input: &[u8],
    assets: &str,
    snapshot: &Value,
    targets: &[&str],
) -> Result<Value> {
    let manifest: Value =
        serde_json::from_slice(&bounded(&dir.join("verification.json"), 512_000)?)?;
    ensure!(
        manifest["schema_version"] == 1
            && manifest["input_sha256"] == digest(input)
            && manifest["asset_sha256"] == assets
            && manifest["source_run"] == snapshot["source_run"]
            && manifest["ward"] == snapshot["ward"]
            && manifest["focus"] == snapshot["focus"]["id"]
            && manifest["mode"].as_str()
                == Some(
                    snapshot
                        .get("mode")
                        .and_then(Value::as_str)
                        .unwrap_or("scorecard")
                ),
        "map input verification mismatch"
    );
    let renders = manifest["renders"]
        .as_array()
        .context("missing map outputs")?;
    ensure!(renders.len() == targets.len(), "partial map pair");
    for target in targets {
        let matching: Vec<_> = renders.iter().filter(|r| r["id"] == *target).collect();
        ensure!(matching.len() == 1, "missing or duplicate map output");
        let r = matching[0];
        let name = if *target == "ward-base" {
            "ward-base.png".into()
        } else {
            format!("{target}.jpg")
        };
        ensure!(
            r["name"] == name && r["errors"].as_array().is_some_and(|e| e.is_empty()),
            "map errors or unexpected export"
        );
        let bytes = bounded(
            &dir.join(name),
            if *target == "ward-base" {
                MAX_BASE_BYTES
            } else {
                MAX_IMAGE_BYTES
            },
        )?;
        ensure!(
            r["bytes"] == bytes.len() as u64
                && r["sha256"] == digest(&bytes)
                && r["width"] == 2160
                && r["height"] == if *target == "ward-base" { 1600 } else { 2160 },
            "map output checksum or dimension mismatch"
        );
        if *target == "ward-base" {
            background_image(&bytes)?;
            ward_card::validate_camera(&r["camera"], &r["references"])?;
        } else {
            jpeg(&bytes)?;
        }
    }
    Ok(manifest)
}
fn child(
    config: &Config,
    input: &Path,
    out: &Path,
    assets: &str,
    timeout: u64,
    session: &Path,
) -> Result<bool> {
    let (root, node) = dirs(config);

    let mut command = Command::new(node);
    command
        .arg(root.join("render-live.mjs"))
        .arg(input)
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .env("ADU_MAP_ASSET_DIGEST", assets)
        .env("ADU_MAP_TIMEOUT_SECONDS", timeout.to_string());
    if config.maps.backend == MapBackend::Cloudflare {
        command
            .env("ADU_MAP_BACKEND", "cloudflare")
            .env(
                "ADU_CF_ACCOUNT_ID",
                config
                    .maps
                    .account_id
                    .as_deref()
                    .context("Cloudflare account")?,
            )
            .env(
                "ADU_CF_TOKEN_FILE",
                config
                    .maps
                    .token_file
                    .as_ref()
                    .context("Cloudflare token file")?,
            )
            .env("ADU_CF_SESSION_FILE", session);
    } else {
        command.env_remove("ADU_MAP_BACKEND");
        if let Some(p) = &config.scorecards.chrome_bin {
            command.env("CHROME_BIN", p);
        }
    }
    let mut process = command.spawn().context("start map controller")?;
    let mut stderr = process.stderr.take().context("renderer diagnostics")?;
    let diagnostics = out.join("renderer-stderr.txt");
    let reader = thread::spawn(move || -> std::io::Result<()> {
        let mut file = fs::File::create(diagnostics)?;
        let mut remaining = 32_000;
        let mut buffer = [0u8; 4096];
        use std::io::Write;
        loop {
            let n = stderr.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            let keep = n.min(remaining);
            file.write_all(&buffer[..keep])?;
            remaining -= keep;
        }
        Ok(())
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = process.try_wait()? {
            break status;
        }
        if start.elapsed() > Duration::from_secs(timeout + 15) {
            process.kill()?;
            process.wait()?;
            let _ = reader.join();
            anyhow::bail!("map controller exceeded cleanup deadline");
        }
        thread::sleep(Duration::from_millis(50));
    };
    let _ = reader.join();
    // Child stderr is deliberately never echoed: vendor errors can contain credentials.
    if !status.success() {
        if let Ok(bytes) = bounded(&out.join("render-error.json"), 4096) {
            let e: Value = serde_json::from_slice(&bytes)?;
            let category = e["category"].as_str().unwrap_or("transport");
            return Err(match category{
                "auth"=>DeliveryError::Hold{message:"Cloudflare renderer authentication failed; repair maps.token_file and retry this reply".into()},
                "quota"=>DeliveryError::Deferred{message:"Cloudflare browser quota exhausted".into(),until:(now().div_euclid(86400)+1)*86400},
                "rate_limit"=>DeliveryError::Deferred{message:"Cloudflare browser rate limit".into(),until:now()+e["retry_after_seconds"].as_i64().unwrap_or(60).clamp(20,86400)},
                _=>DeliveryError::Retry{message:format!("Cloudflare map controller {category} failure"),after:Some(900)},
            }.into());
        }
        anyhow::bail!("map controller failed; inspect local diagnostics");
    }
    Ok(!session.exists())
}
fn context(config: &Config, snapshot: &Value) -> Result<(String, String, String)> {
    let assets = asset_digest(config)?;
    let epoch = now().div_euclid((config.maps.basemap_max_age_days * 86400) as i64);
    let key = digest(&serde_json::to_vec(
        &json!({"version":VERSION,"snapshot":snapshot,"assets":assets,"epoch":epoch,"native_ward":config.maps.cache_ward_backgrounds,"backend":format!("{:?}",config.maps.backend)}),
    )?);
    let background = digest(&serde_json::to_vec(
        &json!({"version":VERSION,"ward":snapshot["ward"],"boundary":snapshot["boundary"],"assets":assets,"epoch":epoch}),
    )?);
    Ok((assets, key, background))
}
fn lock(config: &Config) -> Result<(PathBuf, fs::File)> {
    let root = config.state_dir.join("map-cache");
    fs::create_dir_all(&root)?;
    let f = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("renderer.lock"))?;
    FileExt::try_lock_exclusive(&f).map_err(|_| DeliveryError::Deferred {
        message: "another map render is active".into(),
        until: now() + 30,
    })?;
    Ok((root, f))
}
fn render(
    config: &Config,
    snapshot: &Value,
    targets: &[&str],
    out: &Path,
    assets: &str,
    root: &Path,
) -> Result<Value> {
    let cloud = config.maps.backend == MapBackend::Cloudflare;
    let mut timeout = config
        .maps
        .attempt_timeout_seconds
        .min(config.max_run_seconds.saturating_sub(15));
    if let Some(deadline) = config.render_deadline {
        timeout = timeout.min(
            deadline
                .saturating_duration_since(Instant::now())
                .as_secs()
                .saturating_sub(15),
        );
    }
    if timeout < 10 {
        return Err(DeliveryError::Deferred {
            message: "map job exceeds remaining run time".into(),
            until: now() + 60,
        }
        .into());
    }
    if cloud {
        let file = config
            .maps
            .token_file
            .as_ref()
            .context("Cloudflare token file")?;
        let token = bounded(file, 4096).map_err(|_| DeliveryError::Hold {
            message: "Cloudflare token file is missing or unreadable".into(),
        })?;
        let token = std::str::from_utf8(&token).unwrap_or("").trim();
        if token.is_empty() || token.chars().any(char::is_whitespace) {
            return Err(DeliveryError::Hold {
                message: "Cloudflare token file is invalid".into(),
            }
            .into());
        }
    }
    let mut request = snapshot.clone();
    request["_render"] = json!({"targets":targets});
    let input = serde_json::to_vec(&request)?;
    ensure!(input.len() <= 4_000_000, "map snapshot is too large");
    fs::write(out.join("snapshot.json"), &input)?;
    let budget_path = root.join(format!(
        "budget-{}.json",
        config.maps.account_id.as_deref().unwrap_or("local")
    ));
    let started = now();
    let timer = Instant::now();
    let reservation = timeout + 75;
    let mut budget: Budget = if budget_path.exists() {
        serde_json::from_slice(&bounded(&budget_path, 4096)?)?
    } else {
        Budget::default()
    };
    if cloud {
        budget.reserve(config, started, reservation)?;
        atomic(&budget_path, &serde_json::to_vec(&budget)?)?;
    }
    let session = root.join("cloudflare-active.json");
    let result = child(
        config,
        &out.join("snapshot.json"),
        out,
        assets,
        timeout,
        &session,
    );
    if cloud {
        let closed = result.as_ref().is_ok_and(|closed| *closed)
            || (!session.exists()
                && bounded(&out.join("render-error.json"), 4096)
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                    .is_some_and(|e| {
                        e["session_closed"] == true
                            || matches!(
                                e["category"].as_str(),
                                Some("auth" | "quota" | "rate_limit")
                            )
                    }));
        budget.finish(started, timer.elapsed().as_secs() + 1, reservation, closed);
        atomic(&budget_path, &serde_json::to_vec(&budget)?)?;
    }
    result?;
    validate_output(out, &input, assets, snapshot, targets)
}

/// Persist source evidence before a remote render or upload can fail.
pub fn freeze<T: Serialize + serde::de::DeserializeOwned>(
    config: &Config,
    namespace: &str,
    identity: &str,
    evidence: &[u8],
    produce: impl FnOnce() -> Result<T>,
) -> Result<T> {
    #[derive(Serialize, Deserialize)]
    struct Frozen {
        version: u64,
        evidence: String,
        hash: String,
        value: Value,
    }
    let dir = config.state_dir.join("map-evidence").join(namespace);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", digest(identity.as_bytes())));
    let evidence = digest(evidence);
    if path.exists() {
        let f: Frozen = serde_json::from_slice(&bounded(&path, 4_500_000)?)?;
        ensure!(
            f.version == VERSION
                && f.evidence == evidence
                && f.hash == digest(&serde_json::to_vec(&f.value)?),
            "frozen map evidence mismatch"
        );
        return Ok(serde_json::from_value(f.value)?);
    }
    let value = produce()?;
    let value_json = serde_json::to_value(&value)?;
    let f = Frozen {
        version: VERSION,
        evidence,
        hash: digest(&serde_json::to_vec(&value_json)?),
        value: value_json,
    };
    let bytes = serde_json::to_vec(&f)?;
    ensure!(bytes.len() <= 4_500_000, "map evidence exceeds limit");
    atomic(&path, &bytes)?;
    Ok(value)
}

fn validate_snapshot(snapshot: &Value) -> Result<()> {
    let allowed = [
        "mode",
        "source_run",
        "as_of",
        "cohort_start",
        "ward",
        "applications",
        "adus",
        "rank",
        "tied",
        "city_adus",
        "city_applications",
        "mapped_applications",
        "focus",
        "points",
        "boundary",
        "permits",
        "sites",
        "mapped_sites",
        "city_permits",
        "permit_number",
    ];
    ensure!(
        snapshot
            .as_object()
            .is_some_and(|o| o.keys().all(|k| allowed.contains(&k.as_str()))),
        "unexpected map snapshot fields"
    );
    ensure!(
        snapshot["ward"]
            .as_i64()
            .is_some_and(|w| (1..=50).contains(&w))
            && snapshot["rank"]
                .as_i64()
                .is_some_and(|w| (1..=50).contains(&w)),
        "invalid map ward or rank"
    );
    for point in std::iter::once(&snapshot["focus"])
        .chain(snapshot["points"].as_array().context("map points")?)
    {
        ensure!(
            point.as_object().is_some_and(|p| p
                .keys()
                .all(|k| ["id", "address", "quantity", "location"].contains(&k.as_str())))
                && point["location"]
                    .as_object()
                    .is_some_and(|p| p
                        .keys()
                        .all(|k| ["longitude", "latitude"].contains(&k.as_str()))),
            "unexpected map point fields"
        );
    }
    let focus: crate::scorecard::Point = serde_json::from_value(snapshot["focus"].clone())?;
    let points: Vec<crate::scorecard::Point> = serde_json::from_value(snapshot["points"].clone())?;
    ensure!(
        !points.is_empty()
            && points.len() <= 1000
            && points.iter().any(|p| p.id == focus.id
                && p.location == focus.location
                && p.quantity == focus.quantity
                && p.address == focus.address),
        "invalid map focus"
    );
    ensure!(
        points.iter().all(|p| p.location.valid()
            && ((if snapshot["mode"] == "permit" { 1 } else { 0 })..=10000).contains(&p.quantity)
            && p.id.len() <= 128
            && p.address.len() <= 512),
        "invalid map points"
    );
    let ids: std::collections::HashSet<_> = points.iter().map(|p| &p.id).collect();
    ensure!(ids.len() == points.len(), "duplicate map points");
    ensure!(
        snapshot["source_run"].as_i64().is_some_and(|n| n >= 0) && snapshot["tied"].is_boolean(),
        "invalid map evidence"
    );
    chrono::NaiveDate::parse_from_str(snapshot["as_of"].as_str().context("map date")?, "%Y-%m-%d")?;
    let counts = match snapshot["mode"].as_str() {
        Some("permit") => {
            ensure!(
                snapshot["permit_number"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty() && s.len() <= 128),
                "invalid permit number"
            );
            ["permits", "sites", "mapped_sites"]
        }
        None | Some("scorecard") => ["adus", "applications", "mapped_applications"],
        _ => anyhow::bail!("unsupported map mode"),
    };
    for key in counts {
        ensure!(
            snapshot[key]
                .as_i64()
                .is_some_and(|n| n >= i64::from(key != "adus")),
            "invalid map totals"
        );
    }
    let center = ward_card::boundary_center(&snapshot["boundary"])?;
    ensure!(
        crate::normalize::Location {
            longitude: center[0],
            latitude: center[1]
        }
        .valid(),
        "invalid ward boundary"
    );
    ensure!(
        crate::scorecard::inside_ward(&snapshot["boundary"], focus.location)
            && points
                .iter()
                .all(|p| crate::scorecard::inside_ward(&snapshot["boundary"], p.location)),
        "map points outside ward"
    );
    Ok(())
}

pub fn pair(config: &Config, snapshot: &Value) -> Result<(Vec<u8>, Vec<u8>)> {
    validate_snapshot(snapshot)?;
    let (root, _lock) = lock(config)?;
    let (assets, key, basekey) = context(config, snapshot)?;
    let pairs = root.join("pairs");
    fs::create_dir_all(&pairs)?;
    let dest = pairs.join(&key);
    if dest.exists() {
        return read_pair(&dest, &key);
    }
    let work = tempfile::tempdir_in(&pairs)?;
    let native =
        config.maps.backend == MapBackend::Cloudflare && config.maps.cache_ward_backgrounds;
    let backgrounds = root.join("wards");
    fs::create_dir_all(&backgrounds)?;
    let base_dir = backgrounds.join(&basekey);
    let base = if native && base_dir.exists() {
        Some(read_background(&base_dir, &basekey)?)
    } else {
        None
    };
    let targets = if !native {
        vec!["n5", "wc"]
    } else if base.is_some() {
        vec!["n5"]
    } else {
        vec!["n5", "ward-base"]
    };
    let manifest = match render(config, snapshot, &targets, work.path(), &assets, &root) {
        Ok(manifest) => manifest,
        Err(error) => {
            // Retain bounded diagnostics for inspection after the temporary job is removed.
            let failure = root.join("last-failure");
            fs::create_dir_all(&failure)?;
            for name in ["renderer-stderr.txt", "render-error.json"] {
                if let Ok(bytes) = bounded(&work.path().join(name), 32_000) {
                    atomic(&failure.join(name), &bytes)?;
                } else if failure.join(name).exists() {
                    fs::remove_file(failure.join(name))?;
                }
            }
            return Err(error);
        }
    };
    if native {
        let (bytes, camera) = if let Some(base) = base {
            base
        } else {
            let entry = manifest["renders"]
                .as_array()
                .context("outputs")?
                .iter()
                .find(|r| r["id"] == "ward-base")
                .context("missing ward background")?;
            let bytes = bounded(&work.path().join("ward-base.png"), MAX_BASE_BYTES)?;
            let staging = tempfile::tempdir_in(&backgrounds)?;
            fs::write(staging.path().join("ward-base.png"), &bytes)?;
            let m = BackgroundManifest {
                version: VERSION,
                key: basekey.clone(),
                image: digest(&bytes),
                transform_hash: transform_hash(&entry["camera"], &entry["references"])?,
                camera: entry["camera"].clone(),
                references: entry["references"].clone(),
            };
            atomic(
                &staging.path().join("background.json"),
                &serde_json::to_vec(&m)?,
            )?;
            fs::rename(staging.path(), &base_dir)?;
            (bytes, entry["camera"].clone())
        };
        let ward = ward_card::compose(snapshot, &bytes, &camera)?;
        fs::write(work.path().join("wc.jpg"), ward)?;
    }
    let near = bounded(&work.path().join("n5.jpg"), MAX_IMAGE_BYTES)?;
    let ward = bounded(&work.path().join("wc.jpg"), MAX_IMAGE_BYTES)?;
    jpeg(&near)?;
    jpeg(&ward)?;
    let m = PairManifest {
        version: VERSION,
        key: key.clone(),
        near: digest(&near),
        ward: digest(&ward),
    };
    atomic(&work.path().join("pair.json"), &serde_json::to_vec(&m)?)?;
    fs::rename(work.path(), &dest)?;
    Ok((near, ward))
}

/// Compose changing ward markers from an existing basemap without any browser.
pub fn cached_ward(config: &Config, snapshot: &Value) -> Result<Vec<u8>> {
    validate_snapshot(snapshot)?;
    let (root, _lock) = lock(config)?;
    let (_, _, key) = context(config, snapshot)?;
    let (bytes, camera) = read_background(&root.join("wards").join(key.clone()), &key)
        .context("ward background missing or invalid; warm this ward first")?;
    ward_card::compose(snapshot, &bytes, &camera)
}

/// Warm fixed basemaps without opening SQLite or authenticating a social publisher.
pub fn warm_wards(config: &Config, wards: &[i64]) -> Result<()> {
    ensure!(
        config.maps.backend == MapBackend::Cloudflare,
        "ward warming requires maps.backend=cloudflare"
    );
    for &ward in wards {
        ensure!((1..=50).contains(&ward), "ward must be 1..50");
        let boundary = crate::scorecard::fetch_boundary(config, ward)?;
        let location = crate::ward_card::boundary_center(&boundary)?;
        let snapshot = json!({"ward":ward,"boundary":boundary,"focus":{"id":"ward-background","location":{"longitude":location[0],"latitude":location[1]}},"points":[{"id":"ward-background","quantity":0,"location":{"longitude":location[0],"latitude":location[1]}}],"source_run":0});
        let (root, _lock) = lock(config)?;
        let (assets, _, key) = context(config, &snapshot)?;
        let backgrounds = root.join("wards");
        fs::create_dir_all(&backgrounds)?;
        let dest = backgrounds.join(&key);
        if dest.exists() {
            read_background(&dest, &key)?;
            continue;
        }
        let budget = root.join(format!(
            "budget-{}.json",
            config
                .maps
                .account_id
                .as_deref()
                .context("Cloudflare account")?
        ));
        if budget.exists() {
            let ledger: Budget = serde_json::from_slice(&bounded(&budget, 4096)?)?;
            let wait = (ledger.next_acquisition - now()).clamp(0, 20) as u64;
            thread::sleep(Duration::from_secs(wait));
        }
        let work = tempfile::tempdir_in(&backgrounds)?;
        let m = render(
            config,
            &snapshot,
            &["ward-base"],
            work.path(),
            &assets,
            &root,
        )?;
        let entry = &m["renders"][0];
        let bytes = bounded(&work.path().join("ward-base.png"), MAX_BASE_BYTES)?;
        let manifest = BackgroundManifest {
            version: VERSION,
            key: key.clone(),
            image: digest(&bytes),
            transform_hash: transform_hash(&entry["camera"], &entry["references"])?,
            camera: entry["camera"].clone(),
            references: entry["references"].clone(),
        };
        atomic(
            &work.path().join("background.json"),
            &serde_json::to_vec(&manifest)?,
        )?;
        fs::rename(work.path(), &dest)?;
        println!("{}", json!({"event":"ward_background_cached","ward":ward}));
        // Quota exhaustion stops warming; completed entries remain reusable.
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reservations_survive_crashes_and_roll_over_conservatively() {
        let config = Config::default();
        let time = now();
        let mut b = Budget::default();
        b.reserve(&config, time, 165).unwrap();
        assert_eq!(b.seconds, 165);
        assert!(
            matches!(b.reserve(&config, time+1,165).unwrap_err().downcast_ref::<DeliveryError>(),Some(DeliveryError::Deferred{until,..}) if *until==time+20)
        );
        b.finish(time, 25, 165, false);
        assert_eq!(b.seconds, 165);
        b.rollover((time.div_euclid(86400) + 1) * 86400, 165);
        assert_eq!(b.seconds, 165);
        assert_eq!(b.pairs, 1);
        b.seconds = 400;
        assert!(matches!(
            b.reserve(&config, (time.div_euclid(86400) + 1) * 86400 + 20, 165)
                .unwrap_err()
                .downcast_ref::<DeliveryError>(),
            Some(DeliveryError::Deferred { .. })
        ));
    }
    #[test]
    fn confirmed_closure_refunds_reservation_but_retains_attempt_cap() {
        let mut b = Budget::default();
        let time = now();
        b.reserve(&Config::default(), time, 165).unwrap();
        b.finish(time, 24, 165, true);
        assert_eq!((b.seconds, b.pairs, b.inflight), (24, 1, false));
    }
    #[test]
    fn evidence_is_frozen_before_work_and_mismatches_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            state_dir: dir.path().into(),
            ..Config::default()
        };
        let original = json!({"as_of":"2026-10-05","points":[1,2]});
        let first: Value = freeze(&config, "test", "reply-123", b"evidence", || {
            Ok(original.clone())
        })
        .unwrap();
        let again: Value = freeze(&config, "test", "reply-123", b"evidence", || {
            panic!("must reuse snapshot")
        })
        .unwrap();
        assert_eq!(first, again);
        assert!(
            freeze::<Value>(&config, "test", "reply-123", b"changed", || Ok(json!({}))).is_err()
        );
        let path = dir
            .path()
            .join("map-evidence/test")
            .join(format!("{}.json", digest(b"reply-123")));
        let mut corrupt: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        corrupt["value"]["points"] = json!([99]);
        fs::write(path, serde_json::to_vec(&corrupt).unwrap()).unwrap();
        assert!(
            freeze::<Value>(&config, "test", "reply-123", b"evidence", || Ok(json!({}))).is_err()
        );
    }
    #[test]
    fn public_input_keeps_zero_requested_units_and_rejects_extra_personal_fields() {
        let focus = json!({"id":"a","address":"PUBLIC PROJECT ADDRESS","quantity":0,"location":{"longitude":-87.7,"latitude":41.91}});
        let mut snapshot = json!({"ward":26,"rank":50,"tied":false,"source_run":1,"as_of":"2026-10-05","adus":0,"applications":1,"mapped_applications":1,"focus":focus,"points":[focus],"boundary":{"type":"FeatureCollection","features":[{"type":"Feature","properties":{"WARD":26},"geometry":{"type":"Polygon","coordinates":[[[-87.71,41.90],[-87.69,41.90],[-87.69,41.92],[-87.71,41.92],[-87.71,41.90]]]}}]}});
        validate_snapshot(&snapshot).unwrap();
        snapshot["focus"]["applicant_name"] = json!("unnecessary personal field");
        assert!(validate_snapshot(&snapshot).is_err());
    }
    #[test]
    fn complete_manifest_checks_actual_output_hashes_before_cache_promotion() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = json!({"ward":26,"source_run":48,"focus":{"id":"123"}});
        let rgb = vec![255u8; 2160 * 2160 * 3];
        let bytes = crate::media::jpeg(&rgb, 2160, 2160, MAX_IMAGE_BYTES)
            .unwrap()
            .0;
        let renders:Vec<_>=["n5","wc"].into_iter().map(|id| {
            let name=format!("{id}.jpg");fs::write(dir.path().join(&name),&bytes).unwrap();
            json!({"id":id,"name":name,"sha256":digest(&bytes),"bytes":bytes.len(),"width":2160,"height":2160,"errors":[]})
        }).collect();
        let manifest = json!({"schema_version":1,"input_sha256":digest(b"input"),"asset_sha256":"assets","ward":26,"source_run":48,"focus":"123","mode":"scorecard","renders":renders});
        fs::write(
            dir.path().join("verification.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        validate_output(dir.path(), b"input", "assets", &snapshot, &["n5", "wc"]).unwrap();
        let mut altered = bytes;
        altered[500] ^= 1;
        fs::write(dir.path().join("wc.jpg"), altered).unwrap();
        assert!(validate_output(dir.path(), b"input", "assets", &snapshot, &["n5", "wc"]).is_err());
    }
    #[test]
    fn manifests_reject_partial_wrong_source_and_oversize_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let snapshot = json!({"ward":26,"source_run":48,"focus":{"id":"123"}});
        let manifest = json!({"schema_version":1,"input_sha256":digest(b"input"),"asset_sha256":"assets","ward":26,"source_run":48,"focus":"123","mode":"scorecard","renders":[]});
        fs::write(
            dir.path().join("verification.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        assert!(validate_output(dir.path(), b"input", "assets", &snapshot, &["n5", "wc"]).is_err());
        assert!(validate_output(dir.path(), b"other", "assets", &snapshot, &[]).is_err());
        fs::write(dir.path().join("large"), [0u8; 101]).unwrap();
        assert!(bounded(&dir.path().join("large"), 100).is_err());
    }
}
