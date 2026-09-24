//! Pull tested public releases. Host paths and repository identity are operator configuration.
use anyhow::{Context, Result, ensure};
use clap::Parser;
use fs2::FileExt;
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Parser)]
#[command(version)]
struct Args {
    #[arg(long)]
    config: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    repository: String,
    install_dir: PathBuf,
    runtime_config: PathBuf,
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
}
#[derive(Deserialize)]
struct Manifest {
    commit: String,
    sha256: String,
    schema: i64,
    platform: String,
    #[serde(default)]
    renderer_sha256: BTreeMap<String, String>,
}
const RENDERER_ASSETS: &[(&str, &str)] = &[
    ("map-renderer-render-live.mjs", "render-live.mjs"),
    ("map-renderer-render-live.js", "render-live.js"),
    ("map-renderer-render-live.html", "render-live.html"),
    ("map-renderer-base-style.json", "data/base-style.json"),
    ("map-renderer-maplibre-gl.js", "vendor/maplibre-gl.js"),
    (
        "map-renderer-BigShouldersText-Bold.ttf",
        "fonts/BigShouldersText-Bold.ttf",
    ),
    ("map-renderer-Roboto.ttf", "fonts/Roboto.ttf"),
];
fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn valid_repository(value: &str) -> bool {
    let parts: Vec<_> = value.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && *part != "."
                && *part != ".."
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        })
}
fn download(agent: &ureq::Agent, url: &str, path: &Path, limit: u64) -> Result<String> {
    let mut response = agent.get(url).call()?;
    let mut reader = response.body_mut().as_reader().take(limit + 1);
    let mut out = File::create(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    let mut total = 0;
    loop {
        let n = reader.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        ensure!(total <= limit, "download exceeds size limit");
        out.write_all(&buffer[..n])?;
        hash.update(&buffer[..n]);
    }
    out.sync_all()?;
    Ok(format!("{:x}", hash.finalize()))
}
fn checksum(path: &Path) -> Result<String> {
    let mut hash = Sha256::new();
    std::io::copy(&mut File::open(path)?, &mut hash)?;
    Ok(format!("{:x}", hash.finalize()))
}
fn command(binary: &Path, config: &Path, action: &str) -> Result<()> {
    let mut child = Command::new(binary)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .arg("--config")
        .arg(config)
        .arg(action)
        .stdin(Stdio::null())
        .spawn()?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            ensure!(status.success(), "candidate {action} failed");
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(120) {
            child.kill()?;
            child.wait()?;
            anyhow::bail!("candidate {action} timed out");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn point(root: &Path, name: &str, target: &Path) -> Result<()> {
    let pending = root.join(format!(".{name}-next"));
    if pending.is_symlink() {
        fs::remove_file(&pending)?;
    }
    symlink(target, &pending)?;
    fs::rename(pending, root.join(name))?;
    File::open(root)?.sync_all()?;
    Ok(())
}
fn source(path: &Path) -> Result<Connection> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.execute_batch("PRAGMA query_only=ON; PRAGMA busy_timeout=5000;")?;
    Ok(db)
}
fn schema(path: &Path) -> Result<i64> {
    Ok(source(path)?.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}
// These identities are essential to avoiding lost history and duplicate posts.
fn identities(path: &Path) -> Result<Vec<String>> {
    let db = source(path)?;
    let mut values = Vec::new();
    for sql in [
        "SELECT event_key FROM events ORDER BY event_key",
        "SELECT json_array(event_key,platform,account_id,record_key,record_json,remote_uri,remote_cid) FROM deliveries WHERE state='sent' ORDER BY id",
        "SELECT json_array(dataset_id,baseline_run,baseline_date) FROM source_state ORDER BY dataset_id",
    ] {
        let mut query = db.prepare(sql)?;
        values.extend(
            query
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    if schema(path)? >= 2 {
        let mut query = db.prepare("SELECT json_array(id,parent_delivery_id,record_key,record_json,remote_uri,remote_cid,snapshot_json) FROM reply_deliveries ORDER BY id")?;
        values.extend(
            query
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    Ok(values)
}
fn install(cfg: &Config, root: &Path) -> Result<()> {
    ensure!(
        valid_repository(&cfg.repository),
        "expected owner/repository"
    );
    let runtime = adu_bot::config::Config::load(Some(&cfg.runtime_config))?;
    ensure!(
        root.is_absolute() && runtime.state_dir.is_absolute(),
        "deployment paths must be absolute"
    );
    ensure!(
        runtime.require_existing_state,
        "production must require existing state"
    );
    let state = &runtime.state_dir;
    ensure!(
        state.join("adu.sqlite3").is_file(),
        "production database missing"
    );
    if root.join("deploy-paused").exists() {
        println!("Automatic deployment paused");
        return Ok(());
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("update.lock"))?;
    lock.try_lock_exclusive()
        .context("deployment already active")?;
    let work = root.join(".candidate");
    if work.exists() {
        fs::remove_dir_all(&work)?;
    }
    fs::create_dir(&work)?;
    fs::set_permissions(&work, fs::Permissions::from_mode(0o700))?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent("adu-bot-updater")
        .timeout_global(Some(Duration::from_secs(120)))
        .max_redirects(5)
        .build()
        .into();
    let api = format!("https://api.github.com/repos/{}", cfg.repository);
    let release_path = work.join("release.json");
    download(
        &agent,
        &format!("{api}/releases/latest"),
        &release_path,
        256_000,
    )?;
    let release: Release = serde_json::from_reader(File::open(release_path)?)?;
    let commit = release
        .tag_name
        .strip_prefix("build-")
        .context("unexpected release tag")?;
    ensure!(
        hex(commit, 40) && !release.draft && !release.prerelease,
        "ineligible release"
    );
    let candidate = root.join("releases").join(commit);
    if root.join("current").is_symlink() && fs::canonicalize(root.join("current"))? == candidate {
        ensure!(
            !state.join("deployment-blocked").exists(),
            "deployment recovery required"
        );
        println!("Already deployed {commit}");
        fs::remove_dir_all(work)?;
        return Ok(());
    }
    // A late-finishing build or manually marked latest release cannot roll main back.
    let head = work.join("head.json");
    download(&agent, &format!("{api}/commits/main"), &head, 512_000)?;
    let head: serde_json::Value = serde_json::from_reader(File::open(head)?)?;
    if head["sha"].as_str() != Some(commit) {
        println!("Waiting for a successful release of current main");
        fs::remove_dir_all(work)?;
        return Ok(());
    }
    let base = format!(
        "https://github.com/{}/releases/download/{}",
        cfg.repository, release.tag_name
    );
    let manifest_path = work.join("manifest.json");
    download(
        &agent,
        &format!("{base}/manifest.json"),
        &manifest_path,
        4096,
    )?;
    let manifest: Manifest = serde_json::from_reader(File::open(&manifest_path)?)?;
    ensure!(
        manifest.commit == commit
            && hex(&manifest.sha256, 64)
            && manifest.platform == "linux-x86_64"
            && manifest.schema > 0,
        "invalid release manifest"
    );
    if manifest.schema >= 2 {
        ensure!(
            manifest.renderer_sha256.len() == RENDERER_ASSETS.len(),
            "incomplete renderer manifest"
        );
        for (asset, _) in RENDERER_ASSETS {
            ensure!(
                manifest
                    .renderer_sha256
                    .get(*asset)
                    .is_some_and(|digest| hex(digest, 64)),
                "invalid renderer checksum"
            );
        }
    }
    let binary = work.join("adu-bot");
    ensure!(
        download(
            &agent,
            &format!("{base}/adu-bot-linux-x86_64"),
            &binary,
            40_000_000
        )? == manifest.sha256,
        "checksum mismatch"
    );
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
    let renderer = work.join("map-renderer");
    if manifest.schema >= 2 {
        fs::create_dir(&renderer)?;
        for (asset, relative) in RENDERER_ASSETS {
            let destination = renderer.join(relative);
            fs::create_dir_all(destination.parent().context("renderer asset parent")?)?;
            ensure!(
                download(&agent, &format!("{base}/{asset}"), &destination, 12_000_000)?
                    == manifest.renderer_sha256[*asset],
                "renderer asset checksum mismatch: {asset}"
            );
        }
    }
    ensure!(
        fs2::available_space(root)? > 64 * 1024 * 1024,
        "insufficient release disk space"
    );
    let deployment = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state.join("deployment.lock"))?;
    // Busy runtimes defer deployment to the next scheduled check, never interrupt a post.
    if deployment.try_lock_exclusive().is_err() {
        println!("Runtime active; deployment deferred");
        fs::remove_dir_all(work)?;
        return Ok(());
    }
    let database = state.join("adu.sqlite3");
    let before = identities(&database)?;
    let old_version = schema(&database)?;
    ensure!(
        old_version <= manifest.schema,
        "automatic downgrade forbidden"
    );
    let backups = state.join("backups");
    fs::create_dir_all(&backups)?;
    ensure!(
        fs2::available_space(state)? > database.metadata()?.len() * 3 + 64 * 1024 * 1024,
        "insufficient backup disk space"
    );
    let backup = backups.join(format!(
        "deploy-{}-{commit}.sqlite3",
        chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ")
    ));
    source(&database)?.backup("main", &backup, None)?;
    Connection::open(&backup)?.execute("UPDATE source_state SET posting_paused=1", [])?;
    let shadow = work.join("shadow");
    fs::create_dir(&shadow)?;
    fs::copy(&backup, shadow.join("adu.sqlite3"))?;
    File::create(shadow.join("writer.lock"))?;
    let mut shadow_config: toml::Value = toml::from_str(&fs::read_to_string(&cfg.runtime_config)?)?;
    shadow_config["state_dir"] = toml::Value::String(shadow.to_string_lossy().into_owned());
    shadow_config["publish_enabled"] = toml::Value::Boolean(false);
    let shadow_config_path = work.join("shadow.toml");
    fs::write(&shadow_config_path, toml::to_string(&shadow_config)?)?;
    command(&binary, &shadow_config_path, "migrate")?;
    command(&binary, &shadow_config_path, "check")?;
    ensure!(
        schema(&shadow.join("adu.sqlite3"))? == manifest.schema
            && identities(&shadow.join("adu.sqlite3"))? == before,
        "migration changed durable identities"
    );
    fs::create_dir_all(root.join("releases"))?;
    if candidate.exists() {
        ensure!(
            checksum(&candidate.join("adu-bot"))? == manifest.sha256,
            "immutable release collision"
        );
        if manifest.schema >= 2 {
            for (asset, relative) in RENDERER_ASSETS {
                ensure!(
                    checksum(&candidate.join("map-renderer").join(relative))?
                        == manifest.renderer_sha256[*asset],
                    "immutable renderer collision: {asset}"
                );
            }
        }
    } else {
        let staging = work.join("release");
        fs::create_dir(&staging)?;
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o755))?;
        fs::copy(&binary, staging.join("adu-bot"))?;
        if manifest.schema >= 2 {
            fs::rename(&renderer, staging.join("map-renderer"))?;
        }
        File::open(staging.join("adu-bot"))?.sync_all()?;
        fs::copy(&manifest_path, staging.join("manifest.json"))?;
        fs::rename(staging, &candidate)?;
        File::open(root.join("releases"))?.sync_all()?;
    }
    if old_version != manifest.schema {
        // A crash after a schema change must block old code until activation is recovered.
        fs::write(state.join("deployment-blocked"), commit)?;
        File::open(state.join("deployment-blocked"))?.sync_all()?;
        File::open(state)?.sync_all()?;
        command(&candidate.join("adu-bot"), &cfg.runtime_config, "migrate")?;
        ensure!(
            identities(&database)? == before,
            "live migration changed durable identities; recovery required"
        );
    }
    command(&candidate.join("adu-bot"), &cfg.runtime_config, "check")?;
    if root.join("current").is_symlink() {
        point(root, "previous", &fs::canonicalize(root.join("current"))?)?;
    }
    point(root, "current", &candidate)?;
    if state.join("deployment-blocked").exists() {
        fs::remove_file(state.join("deployment-blocked"))?;
    }
    let previous = fs::canonicalize(root.join("previous")).ok();
    for entry in fs::read_dir(root.join("releases"))? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && hex(&entry.file_name().to_string_lossy(), 40)
            && entry.path() != candidate
            && Some(entry.path()) != previous
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    fs::remove_dir_all(work)?;
    println!(
        "Deployed {commit}; schema {old_version} -> {}; backup {}",
        manifest.schema,
        backup.display()
    );
    Ok(())
}
fn main() -> Result<()> {
    let args = Args::parse();
    let cfg: Config = toml::from_str(&fs::read_to_string(args.config)?)?;
    install(&cfg, &cfg.install_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unsafe_repository_and_commit_names() {
        for name in [
            "../repo",
            "owner/../repo",
            "https://example.com/repo",
            "owner/repo?x=y",
            "owner/",
        ] {
            assert!(!valid_repository(name));
        }
        assert!(valid_repository("example/bot"));
        assert!(!hex("../../release", 40));
        assert!(hex(&"a".repeat(40), 40));
    }

    #[test]
    fn downloads_are_bounded_and_hashed_before_activation() {
        let dir = tempfile::tempdir().unwrap();
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.server_addr());
        let worker = std::thread::spawn(move || {
            for _ in 0..2 {
                server
                    .recv()
                    .unwrap()
                    .respond(tiny_http::Response::from_string("release-data"))
                    .unwrap();
            }
        });
        let agent = ureq::Agent::new_with_defaults();
        let output = dir.path().join("candidate");
        assert_eq!(
            download(&agent, &url, &output, 100).unwrap(),
            format!("{:x}", Sha256::digest(b"release-data"))
        );
        assert!(download(&agent, &url, &output, 4).is_err());
        assert!(!dir.path().join("current").exists());
        worker.join().unwrap();
    }

    #[test]
    fn pointer_switch_keeps_previous_executable_available() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        let new = dir.path().join("new");
        fs::create_dir(&old).unwrap();
        fs::create_dir(&new).unwrap();
        point(dir.path(), "current", &old).unwrap();
        point(dir.path(), "previous", &old).unwrap();
        point(dir.path(), "current", &new).unwrap();
        assert_eq!(
            fs::canonicalize(dir.path().join("current")).unwrap(),
            fs::canonicalize(new).unwrap()
        );
        assert_eq!(
            fs::canonicalize(dir.path().join("previous")).unwrap(),
            fs::canonicalize(old).unwrap()
        );
    }
}
