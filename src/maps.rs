//! Permit map replies rendered with the same source-backed map engine as scorecards.
use crate::{
    config::Config,
    media::{MAX_IMAGE_BYTES, PostImage},
    permits::PermitWardSnapshot,
    scorecard,
};
use anyhow::{Context, Result, ensure};
use image::GenericImageView;
use serde_json::{Value, json};
use std::{fs, process::Command};

pub fn render_pair(
    config: &Config,
    snapshot: &PermitWardSnapshot,
) -> Result<(PostImage, PostImage, i64)> {
    let boundary = scorecard::fetch_boundary(config, snapshot.ward)?;
    ensure!(
        scorecard::inside_ward(&boundary, snapshot.focus.location),
        "permit location lies outside the preapproval ward"
    );
    let points: Vec<_> = snapshot
        .points
        .iter()
        .filter(|point| scorecard::inside_ward(&boundary, point.location))
        .collect();
    ensure!(
        points.iter().any(|point| point.id == snapshot.focus.id),
        "focus permit site is absent from ward map"
    );
    let mapped_sites = points.len() as i64;
    let mut payload = serde_json::to_value(snapshot)?;
    payload["mode"] = json!("permit");
    payload["points"] = json!(points);
    payload["mapped_sites"] = json!(mapped_sites);
    payload["boundary"] = boundary;
    let (near, ward) = render_payload(config, &payload, snapshot, mapped_sites)?;
    Ok((near, ward, mapped_sites))
}

fn render_payload(
    config: &Config,
    payload: &Value,
    snapshot: &PermitWardSnapshot,
    mapped_sites: i64,
) -> Result<(PostImage, PostImage)> {
    let work = tempfile::tempdir().context("create permit map work directory")?;
    let input = work.path().join("snapshot.json");
    fs::write(&input, serde_json::to_vec(payload)?)?;
    let output = Command::new(&config.scorecards.node_bin)
        .arg(config.scorecards.renderer_dir.join("render-live.mjs"))
        .arg(&input)
        .arg(work.path())
        .envs(
            config
                .scorecards
                .chrome_bin
                .as_ref()
                .map(|path| ("CHROME_BIN", path))
                .into_iter(),
        )
        .output()
        .context("start permit map renderer")?;
    ensure!(
        output.status.success(),
        "permit map renderer failed: {}",
        String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(500)
            .collect::<String>()
    );
    let verification: Value =
        serde_json::from_slice(&fs::read(work.path().join("verification.json"))?)?;
    ensure!(
        verification["source_run"] == payload["source_run"]
            && verification["focus"] == payload["focus"]["id"]
            && verification["renders"]
                .as_array()
                .is_some_and(|renders| renders.len() == 2),
        "permit map verification mismatch"
    );
    let near = post_image(
        fs::read(work.path().join("n5.jpg"))?,
        format!(
            "Oblique neighborhood map centered on the approximate location of issued ADU building permit {} at {}, Chicago. A red pin marks the location. Streets, transit, and named places provide context; building shapes do not show the proposed ADU. Location: City of Chicago Data Portal. Basemap: OpenMapTiles and OpenStreetMap.",
            snapshot.permit_number, snapshot.focus.address
        ),
    )?;
    let ward_map = post_image(
        fs::read(work.path().join("wc.jpg"))?,
        format!(
            "Chicago Ward {} outlined with {mapped_sites} of {} preapproved sites that have a uniquely linked issued ADU building permit. Numbered blue dots show the count of confirmed issued permits at each site; a red ring highlights {}. {} linked permits in the ward as of {}. Permits do not establish how many ADUs were authorized or completed. Locations: City of Chicago Data Portal. Boundary: Cook County GIS. Basemap: OpenMapTiles and OpenStreetMap.",
            snapshot.ward, snapshot.sites, snapshot.focus.address, snapshot.permits, snapshot.as_of
        ),
    )?;
    Ok((near, ward_map))
}

fn post_image(bytes: Vec<u8>, alt: String) -> Result<PostImage> {
    ensure!(
        bytes.len() > 1_000 && bytes.len() <= MAX_IMAGE_BYTES,
        "permit map JPEG exceeds Bluesky limits"
    );
    let format = image::guess_format(&bytes)?;
    ensure!(
        format == image::ImageFormat::Jpeg,
        "permit map is not a JPEG"
    );
    let dimensions = image::load_from_memory(&bytes)?.dimensions();
    ensure!(
        dimensions == (2160, 2160),
        "unexpected permit map dimensions"
    );
    Ok(PostImage {
        bytes,
        alt,
        quality: 0,
        width: 2160,
        height: 2160,
    })
}
