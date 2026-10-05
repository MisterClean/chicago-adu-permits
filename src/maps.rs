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
use std::{fs, path::Path};

pub fn render_pair(
    config: &Config,
    snapshot: &PermitWardSnapshot,
) -> Result<(PostImage, PostImage, i64)> {
    let evidence = serde_json::to_vec(snapshot)?;
    let identity = crate::map_renderer::digest(&evidence);
    let payload: Value =
        crate::map_renderer::freeze(config, "permit-payload", &identity, &evidence, || {
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
            Ok(payload)
        })?;
    let mapped_sites = payload["mapped_sites"]
        .as_i64()
        .context("mapped site count")?;
    let (near, ward) = render_payload(config, &payload, snapshot, mapped_sites)?;
    Ok((near, ward, mapped_sites))
}

fn render_payload(
    config: &Config,
    payload: &Value,
    snapshot: &PermitWardSnapshot,
    mapped_sites: i64,
) -> Result<(PostImage, PostImage)> {
    let (near, ward) = crate::map_renderer::pair(config, payload)?;
    let (near_alt, ward_alt) = map_alts(snapshot, mapped_sites);
    let near = post_image(near, near_alt)?;
    let ward_map = post_image(ward, ward_alt)?;
    Ok((near, ward_map))
}

fn map_alts(snapshot: &PermitWardSnapshot, mapped_sites: i64) -> (String, String) {
    (
        format!(
            "Oblique neighborhood map centered on the approximate location of issued ADU building permit {} at {}, Chicago. A red pin marks the location. Streets, transit, and named places provide context; building shapes do not show the proposed ADU. Location: City of Chicago Data Portal. Basemap: OpenMapTiles and OpenStreetMap.",
            snapshot.permit_number, snapshot.focus.address
        ),
        format!(
            "Chicago Ward {} outlined with {mapped_sites} of {} preapproved sites that have a uniquely linked issued ADU building permit. Numbered blue dots show the count of confirmed issued permits at each site; a red ring highlights {}. {} linked permits in the ward as of {}. Permits do not establish how many ADUs were authorized or completed. Locations: City of Chicago Data Portal. Boundary: Cook County GIS. Basemap: OpenMapTiles and OpenStreetMap.",
            snapshot.ward, snapshot.sites, snapshot.focus.address, snapshot.permits, snapshot.as_of
        ),
    )
}

/// Load maps rendered from a copy of this exact source snapshot on a capable host.
pub fn read_prepared(
    snapshot: &PermitWardSnapshot,
    preview: &Path,
) -> Result<(PostImage, PostImage, PermitWardSnapshot)> {
    let supplied: Value =
        serde_json::from_slice(&fs::read(preview.with_extension("ward-snapshot.json"))?)?;
    let mapped_sites = supplied["mapped_sites"]
        .as_i64()
        .context("prepared mapped site count")?;
    ensure!(
        mapped_sites > 0 && mapped_sites <= snapshot.points.len() as i64,
        "invalid prepared map coverage"
    );
    let mut expected_snapshot = snapshot.clone();
    expected_snapshot.mapped_sites = mapped_sites;
    ensure!(
        supplied == serde_json::to_value(&expected_snapshot)?,
        "prepared maps do not match the current permit snapshot"
    );
    let (near_alt, ward_alt) = map_alts(&expected_snapshot, mapped_sites);
    ensure!(
        fs::read_to_string(preview.with_extension("near.alt.txt"))? == near_alt
            && fs::read_to_string(preview.with_extension("ward.alt.txt"))? == ward_alt,
        "prepared map alt text differs from snapshot"
    );
    let near = post_image(fs::read(preview.with_extension("near.jpg"))?, near_alt)?;
    let ward = post_image(fs::read(preview.with_extension("ward.jpg"))?, ward_alt)?;
    Ok((near, ward, expected_snapshot))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{normalize::Location, permits::PermitMapPoint};
    use chrono::NaiveDate;

    #[test]
    fn prepared_maps_require_current_snapshot_alt_and_jpegs() {
        let dir = tempfile::tempdir().unwrap();
        let preview = dir.path().join("permit.jpg");
        let focus = PermitMapPoint {
            id: "123".into(),
            address: "1 W TEST ST".into(),
            quantity: 1,
            location: Location {
                latitude: 41.9,
                longitude: -87.6,
            },
        };
        let snapshot = PermitWardSnapshot {
            source_run: 7,
            as_of: NaiveDate::from_ymd_opt(2026, 9, 24).unwrap(),
            ward: 43,
            permits: 1,
            sites: 1,
            mapped_sites: 1,
            city_permits: 1,
            rank: 1,
            tied: false,
            focus: focus.clone(),
            points: vec![focus],
            permit_number: "101000000".into(),
        };
        let (near_alt, ward_alt) = map_alts(&snapshot, 1);
        let image = image::RgbImage::from_pixel(2160, 2160, image::Rgb([255, 255, 255]));
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80)
            .encode_image(&image)
            .unwrap();
        fs::write(
            preview.with_extension("ward-snapshot.json"),
            serde_json::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
        fs::write(preview.with_extension("near.alt.txt"), &near_alt).unwrap();
        fs::write(preview.with_extension("ward.alt.txt"), &ward_alt).unwrap();
        fs::write(preview.with_extension("near.jpg"), &jpeg).unwrap();
        fs::write(preview.with_extension("ward.jpg"), &jpeg).unwrap();
        assert!(read_prepared(&snapshot, &preview).is_ok());
        let mut changed = snapshot.clone();
        changed.source_run += 1;
        assert!(read_prepared(&changed, &preview).is_err());
        fs::write(preview.with_extension("ward.alt.txt"), "wrong ward").unwrap();
        assert!(read_prepared(&snapshot, &preview).is_err());
    }
}
