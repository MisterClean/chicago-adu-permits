//! Source-backed neighborhood and ward maps for permit replies.
use crate::{
    config::Config,
    media::{self, PostImage},
    permits::Permit,
};
use anyhow::{Context, Result, ensure};
use image::RgbImage;
use serde_json::Value;

pub struct Bounds {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}
impl Bounds {
    fn include(&mut self, lon: f64, lat: f64) {
        self.west = self.west.min(lon);
        self.south = self.south.min(lat);
        self.east = self.east.max(lon);
        self.north = self.north.max(lat);
    }
    fn padded(&self, factor: f64) -> Self {
        let (cx, cy) = ((self.west + self.east) / 2., (self.south + self.north) / 2.);
        let span =
            ((self.east - self.west).max((self.north - self.south) * 1.6) * factor).max(0.008);
        Self {
            west: cx - span / 2.,
            east: cx + span / 2.,
            south: cy - span / 3.2,
            north: cy + span / 3.2,
        }
    }
    fn value(&self) -> String {
        format!("{},{},{},{}", self.west, self.south, self.east, self.north)
    }
}
pub struct WardShape {
    pub rings: Vec<Vec<(f64, f64)>>,
    pub bounds: Bounds,
}
fn fetch(config: &Config, url: &str, limit: u64) -> Result<Vec<u8>> {
    let mut response = config
        .agent()
        .get(url)
        .header("Accept", "*/*")
        .call()
        .map_err(|_| anyhow::anyhow!("map source transport failed"))?;
    ensure!(
        response.status().is_success(),
        "map source HTTP {}",
        response.status().as_u16()
    );
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .context("bounded map source response")
}
pub fn ward_shape(config: &Config, ward: i64) -> Result<WardShape> {
    ensure!((1..=50).contains(&ward), "invalid ward");
    let mut url = url::Url::parse(
        "https://gis.cookcountyil.gov/hosting/rest/services/cookviewer_political_boundaries/MapServer/28/query",
    )?;
    url.query_pairs_mut()
        .append_pair("where", &format!("WARD={ward}"))
        .append_pair("outFields", "WARD")
        .append_pair("returnGeometry", "true")
        .append_pair("outSR", "4326")
        .append_pair("f", "geojson");
    let value: Value = serde_json::from_slice(&fetch(config, url.as_str(), 2_000_000)?)?;
    let features = value["features"]
        .as_array()
        .context("ward geometry missing features")?;
    ensure!(
        features.len() == 1 && features[0]["properties"]["WARD"].as_i64() == Some(ward),
        "ward geometry identity mismatch"
    );
    let geom = &features[0]["geometry"];
    let polys: Vec<&Value> = match geom["type"].as_str() {
        Some("Polygon") => vec![&geom["coordinates"]],
        Some("MultiPolygon") => geom["coordinates"]
            .as_array()
            .context("ward multipolygon missing")?
            .iter()
            .collect(),
        _ => anyhow::bail!("unsupported ward geometry"),
    };
    let mut bounds = Bounds {
        west: f64::INFINITY,
        south: f64::INFINITY,
        east: f64::NEG_INFINITY,
        north: f64::NEG_INFINITY,
    };
    let mut rings = Vec::new();
    for poly in polys {
        for ring in poly.as_array().context("ward polygon missing rings")? {
            let mut points = Vec::new();
            for point in ring.as_array().context("ward ring missing points")? {
                let lon = point[0].as_f64().context("ward longitude missing")?;
                let lat = point[1].as_f64().context("ward latitude missing")?;
                ensure!(
                    (-88.1..=-87.4).contains(&lon) && (41.5..=42.2).contains(&lat),
                    "ward coordinates outside Chicago"
                );
                bounds.include(lon, lat);
                points.push((lon, lat));
            }
            ensure!(points.len() >= 4, "ward ring too short");
            rings.push(points);
        }
    }
    ensure!(!rings.is_empty(), "ward geometry empty");
    Ok(WardShape {
        rings,
        bounds: bounds.padded(1.3),
    })
}
fn basemap(config: &Config, bounds: &Bounds) -> Result<RgbImage> {
    let mut url = url::Url::parse(
        "https://server.arcgisonline.com/ArcGIS/rest/services/World_Street_Map/MapServer/export",
    )?;
    url.query_pairs_mut()
        .append_pair("bbox", &bounds.value())
        .append_pair("bboxSR", "4326")
        .append_pair("imageSR", "4326")
        .append_pair("size", "640,400")
        .append_pair("format", "jpg")
        .append_pair("f", "image");
    let bytes = fetch(config, url.as_str(), 3_000_000)?;
    let photo = image::load_from_memory(&bytes)?.into_rgb8();
    ensure!(
        photo.dimensions() == (640, 400),
        "unexpected basemap dimensions"
    );
    Ok(photo)
}
pub fn render_pair(
    config: &Config,
    permit: &Permit,
    ward: i64,
    address: &str,
) -> Result<(PostImage, PostImage)> {
    let lat = permit
        .text("latitude")
        .and_then(|s| s.parse::<f64>().ok())
        .context("permit latitude missing")?;
    let lon = permit
        .text("longitude")
        .and_then(|s| s.parse::<f64>().ok())
        .context("permit longitude missing")?;
    ensure!(
        (-88.1..=-87.4).contains(&lon) && (41.5..=42.2).contains(&lat),
        "permit coordinates outside Chicago"
    );
    let shape = ward_shape(config, ward)?;
    let near = Bounds {
        west: lon - 0.008,
        south: lat - 0.003125,
        east: lon + 0.008,
        north: lat + 0.003125,
    };
    let neighborhood = media::map_card(
        &basemap(config, &near)?,
        "AROUND THE PERMIT",
        address,
        &near,
        Some((lon, lat)),
        None,
        format!(
            "Street map around permit {} at {}, Chicago. Red marker uses the permit dataset's point, which is approximate geographic context, not a building footprint. Basemap: Esri World Street Map. Permit: City of Chicago. Unofficial feed.",
            permit.number, address
        ),
    )?;
    let ward_map = media::map_card(
        &basemap(config, &shape.bounds)?,
        "THE WARD",
        &format!("WARD {ward}"),
        &shape.bounds,
        Some((lon, lat)),
        Some(&shape.rings),
        format!(
            "Map of Chicago Ward {ward}, outlined in red, with the permit dataset's approximate location marked. Ward geometry: Cook County GIS. Basemap: Esri World Street Map. Unofficial feed."
        ),
    )?;
    Ok((neighborhood, ward_map))
}
