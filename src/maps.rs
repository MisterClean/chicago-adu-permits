//! Permit map replies rendered with the same source-backed map engine as scorecards.
use crate::{
    config::Config,
    media::{MAX_IMAGE_BYTES, PostImage},
    normalize::{Location, Observation},
    permits::Permit,
    render, scorecard,
};
use anyhow::{Context, Result, ensure};
use image::GenericImageView;
use serde_json::{Value, json};
use std::{fs, process::Command};

pub fn render_pair(
    config: &Config,
    permit: &Permit,
    application: &Observation,
) -> Result<(PostImage, PostImage)> {
    let latitude = permit
        .text("latitude")
        .and_then(|value| value.parse::<f64>().ok())
        .context("permit latitude missing")?;
    let longitude = permit
        .text("longitude")
        .and_then(|value| value.parse::<f64>().ok())
        .context("permit longitude missing")?;
    let location = Location {
        latitude,
        longitude,
    };
    ensure!(location.valid(), "permit coordinates outside Chicago");
    let ward = application
        .number("ward")
        .filter(|ward| (1..=50).contains(ward))
        .context("preapproval ward required for maps")?;
    let boundary = scorecard::fetch_boundary(config, ward)?;
    ensure!(
        scorecard::inside_ward(&boundary, location),
        "permit location lies outside the preapproval ward"
    );
    let issued = permit
        .date("issue_date")
        .context("permit issue date required for maps")?
        .to_string();
    let address = render::address(application);
    let focus = json!({
        "id":permit.number,
        "address":address,
        "quantity":application.number("adu_applying_for").unwrap_or(0),
        "location":location
    });
    let payload = json!({
        "mode":"permit",
        "source_run":0,
        "as_of":issued,
        "ward":ward,
        "focus":focus,
        "points":[focus],
        "boundary":boundary,
        "permit_number":permit.number
    });
    render_payload(config, &payload, permit, ward, &address)
}

fn render_payload(
    config: &Config,
    payload: &Value,
    permit: &Permit,
    ward: i64,
    address: &str,
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
            permit.number, address
        ),
    )?;
    let ward_map = post_image(
        fs::read(work.path().join("wc.jpg"))?,
        format!(
            "Chicago Ward {ward} outlined, with the approximate location of issued ADU building permit {} at {} marked by a red pin. Boundary: Cook County GIS. Permit location: City of Chicago Data Portal. Basemap: OpenMapTiles and OpenStreetMap.",
            permit.number, address
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
