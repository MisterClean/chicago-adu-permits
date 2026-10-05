//! Native changing ward overlays on a versioned, north-up MapLibre background.
use crate::{media, normalize::Location, scorecard::Point};
use anyhow::{Context, Result, ensure};
use fontdue::{Font, FontSettings};
use serde::Deserialize;
use serde_json::Value;
use std::io::Cursor;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};
const BLUE: u32 = 0x41b6e6;
const RED: u32 = 0xe4002b;
#[derive(Deserialize)]
struct Camera {
    projection: String,
    center: [f64; 2],
    origin: [f64; 2],
    world_size: f64,
    width: u32,
    height: u32,
    pixel_ratio: u32,
    bearing: f64,
    pitch: f64,
}
impl Camera {
    fn check(&self) -> Result<()> {
        ensure!(
            self.projection == "web-mercator"
                && self.width == 1080
                && self.height == 800
                && self.pixel_ratio == 2
                && self.pitch == 0.
                && self.bearing == 0.,
            "unsupported ward camera"
        );
        ensure!(
            self.center
                .iter()
                .chain(&self.origin)
                .all(|x| x.is_finite())
                && self.world_size.is_finite()
                && (512. ..=134_217_728.).contains(&self.world_size)
                && Location {
                    longitude: self.center[0],
                    latitude: self.center[1]
                }
                .valid(),
            "invalid ward camera"
        );
        Ok(())
    }
    fn project(&self, p: Location) -> [f64; 2] {
        let (x, y) = mercator(p.longitude, p.latitude);
        let (cx, cy) = mercator(self.center[0], self.center[1]);
        [
            self.origin[0] + (x - cx) * self.world_size,
            self.origin[1] + (y - cy) * self.world_size,
        ]
    }
}
fn mercator(lon: f64, lat: f64) -> (f64, f64) {
    (
        lon / 360. + 0.5,
        0.5 - ((std::f64::consts::PI / 4. + lat.to_radians() / 2.)
            .tan()
            .ln())
            / (2. * std::f64::consts::PI),
    )
}
pub fn validate_camera(camera: &Value, references: &Value) -> Result<()> {
    let c: Camera = serde_json::from_value(camera.clone())?;
    c.check()?;
    let refs = references
        .as_array()
        .context("ward projection references")?;
    ensure!(
        !refs.is_empty() && refs.len() <= 1000,
        "invalid ward projection references"
    );
    for r in refs {
        let location: Location = serde_json::from_value(r["location"].clone())?;
        ensure!(location.valid(), "invalid projection location");
        let pixel = c.project(location);
        for (axis, key) in ["x", "y"].iter().enumerate() {
            let expected = r["pixel"][key].as_f64().context("projection pixel")?;
            ensure!(
                expected.is_finite() && (expected - pixel[axis]).abs() < 0.05,
                "ward projection differs from MapLibre"
            );
        }
    }
    Ok(())
}
pub fn boundary_center(boundary: &Value) -> Result<[f64; 2]> {
    fn visit(v: &Value, b: &mut [f64; 4]) -> Result<()> {
        let a = v.as_array().context("boundary coordinates")?;
        if let Some(x) = a.first().and_then(Value::as_f64) {
            let y = a
                .get(1)
                .and_then(Value::as_f64)
                .context("boundary latitude")?;
            ensure!(
                x.is_finite() && y.is_finite(),
                "invalid boundary coordinate"
            );
            b[0] = b[0].min(x);
            b[1] = b[1].min(y);
            b[2] = b[2].max(x);
            b[3] = b[3].max(y);
        } else {
            for v in a {
                visit(v, b)?;
            }
        }
        Ok(())
    }
    let mut b = [
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    ];
    for f in boundary["features"].as_array().context("ward features")? {
        visit(&f["geometry"]["coordinates"], &mut b)?;
    }
    ensure!(b.iter().all(|x| x.is_finite()), "empty boundary");
    Ok([(b[0] + b[2]) / 2., (b[1] + b[3]) / 2.])
}
fn color(rgb: u32) -> Color {
    Color::from_rgba8((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255)
}
fn paint(rgb: u32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color(rgb));
    p
}
fn rect(canvas: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, rgb: u32) {
    if let Some(r) = Rect::from_xywh(x * 2., y * 2., w * 2., h * 2.) {
        canvas.fill_rect(r, &paint(rgb), Transform::identity(), None);
    }
}
fn stroke(canvas: &mut Pixmap, path: &tiny_skia::Path, rgb: u32, width: f32) {
    canvas.stroke_path(
        path,
        &paint(rgb),
        &Stroke {
            width: width * 2.,
            ..Stroke::default()
        },
        Transform::identity(),
        None,
    );
}
fn width(font: &Font, text: &str, size: f32) -> f32 {
    let mut last = None;
    let mut width = 0.;
    for ch in text.chars() {
        if let Some(prev) = last {
            width += font.horizontal_kern(prev, ch, size).unwrap_or(0.);
        }
        width += font.metrics(ch, size).advance_width;
        last = Some(ch);
    }
    width
}
#[allow(clippy::too_many_arguments)]
fn text(canvas: &mut Pixmap, font: &Font, label: &str, x: f32, baseline: f32, size: f32, rgb: u32) {
    let mut pen = x * 2.;
    let mut last = None;
    let channels = [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8];
    for ch in label.chars() {
        if let Some(prev) = last {
            pen += font.horizontal_kern(prev, ch, size * 2.).unwrap_or(0.);
        }
        let (m, bitmap) = font.rasterize(ch, size * 2.);
        let left = pen.round() as i32 + m.xmin;
        let top = (baseline * 2.).round() as i32 - m.height as i32 - m.ymin;
        for gy in 0..m.height {
            let py = top + gy as i32;
            if !(0..2160).contains(&py) {
                continue;
            }
            for gx in 0..m.width {
                let px = left + gx as i32;
                if !(0..2160).contains(&px) {
                    continue;
                }
                let a = bitmap[gy * m.width + gx] as f32 / 255.;
                if a == 0. {
                    continue;
                }
                let offset = (py as usize * 2160 + px as usize) * 4;
                for (i, c) in channels.iter().enumerate() {
                    let old = canvas.data()[offset + i];
                    canvas.data_mut()[offset + i] =
                        (*c as f32 * a + old as f32 * (1. - a)).round() as u8;
                }
            }
        }
        pen += m.advance_width;
        last = Some(ch);
    }
}
#[allow(clippy::too_many_arguments)]
fn fitted(
    canvas: &mut Pixmap,
    font: &Font,
    s: &str,
    x: f32,
    y: f32,
    max: f32,
    mut size: f32,
    rgb: u32,
) {
    while width(font, s, size) > max && size > 28. {
        size -= 2.;
    }
    text(canvas, font, s, x, y, size, rgb);
}
fn star(canvas: &mut Pixmap, x: f32, y: f32) {
    let mut b = PathBuilder::new();
    for i in 0..12 {
        let a = -std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / 6.;
        let r = if i % 2 == 0 { 16. } else { 16. * 0.36 };
        let p = ((x + a.cos() * r) * 2., (y + a.sin() * r) * 2.);
        if i == 0 {
            b.move_to(p.0, p.1);
        } else {
            b.line_to(p.0, p.1);
        }
    }
    b.close();
    if let Some(p) = b.finish() {
        canvas.fill_path(
            &p,
            &paint(RED),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}
fn dot(canvas: &mut Pixmap, font: &Font, p: [f64; 2], n: i64, focus: bool) {
    let (x, y) = (p[0] as f32, p[1] as f32 + 194.);
    let r = 7. + (n as f32).sqrt() * 5.;
    let mut b = PathBuilder::new();
    b.push_circle(x * 2., y * 2., r * 2.);
    if let Some(path) = b.finish() {
        canvas.fill_path(
            &path,
            &paint(if n >= 4 {
                0x075f91
            } else if n >= 2 {
                0x398cac
            } else {
                0x76c6e5
            }),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        stroke(canvas, &path, 0x193d50, 1.7);
    }
    let s = n.to_string();
    text(
        canvas,
        font,
        &s,
        x - width(font, &s, 15.) / 2.,
        y + 5.,
        15.,
        if n > 1 { 0xffffff } else { 0x102e40 },
    );
    if focus {
        let mut b = PathBuilder::new();
        b.push_circle(x * 2., y * 2., (r + 8.) * 2.);
        if let Some(p) = b.finish() {
            stroke(canvas, &p, RED, 4.);
        }
    }
}
fn positions(points: &[Point], camera: &Camera, focus: &str) -> Vec<([f64; 2], [f64; 2], f64)> {
    let mut items: Vec<_> = points
        .iter()
        .map(|f| {
            let p = camera.project(f.location);
            (
                p,
                p,
                7. + (f.quantity as f64).sqrt() * 5. + if f.id == focus { 8. } else { 0. },
            )
        })
        .collect();
    for _ in 0..32 {
        for i in 0..items.len() {
            for j in i + 1..items.len() {
                let (a, b) = (items[i], items[j]);
                let dx = b.1[0] - a.1[0];
                let dy = b.1[1] - a.1[1];
                let d = dx.hypot(dy);
                let gap = a.2 + b.2 + 7.;
                if d >= gap {
                    continue;
                }
                let (ux, uy) = if d > 0.1 { (dx / d, dy / d) } else { (1., 0.) };
                let m = (gap - d) / 2. + 0.15;
                if points[i].id != focus {
                    items[i].1[0] -= ux * m;
                    items[i].1[1] -= uy * m;
                }
                if points[j].id != focus {
                    items[j].1[0] += ux * m;
                    items[j].1[1] += uy * m;
                }
            }
        }
    }
    items
}
fn count(snapshot: &Value, key: &str) -> Result<i64> {
    snapshot[key]
        .as_i64()
        .filter(|n| *n >= 0)
        .with_context(|| format!("invalid ward count {key}"))
}
pub fn compose(snapshot: &Value, background: &[u8], camera: &Value) -> Result<Vec<u8>> {
    let c: Camera = serde_json::from_value(camera.clone())?;
    c.check()?;
    let focus: Point = serde_json::from_value(snapshot["focus"].clone())?;
    let points: Vec<Point> = serde_json::from_value(snapshot["points"].clone())?;
    ensure!(
        !points.is_empty()
            && points.len() <= 1000
            && points.iter().any(|p| p.id == focus.id)
            && points
                .iter()
                .all(|p| p.location.valid() && (0..=10000).contains(&p.quantity)),
        "invalid ward points"
    );
    let mut reader =
        image::ImageReader::with_format(Cursor::new(background), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2160);
    limits.max_image_height = Some(1600);
    limits.max_alloc = Some(20_000_000);
    reader.limits(limits);
    let base = reader.decode()?.into_rgba8();
    ensure!(base.dimensions() == (2160, 1600), "invalid ward background");
    let mut canvas = Pixmap::new(2160, 2160).context("ward canvas")?;
    canvas.fill(color(0));
    // An opaque basemap can be copied directly into tiny-skia's premultiplied canvas.
    ensure!(
        base.pixels().all(|p| p.0[3] == 255),
        "ward basemap must be opaque"
    );
    canvas.data_mut()[388 * 2160 * 4..1988 * 2160 * 4].copy_from_slice(base.as_raw());
    drop(base);
    let big = Font::from_bytes(
        include_bytes!("../assets/fonts/BigShouldersText-Bold.ttf") as &[u8],
        FontSettings::default(),
    )
    .map_err(|_| anyhow::anyhow!("headline font"))?;
    let body = Font::from_bytes(
        include_bytes!("../assets/fonts/Roboto.ttf") as &[u8],
        FontSettings::default(),
    )
    .map_err(|_| anyhow::anyhow!("body font"))?;
    let permit = snapshot["mode"] == "permit";
    let ward = count(snapshot, "ward")?;
    let rank = count(snapshot, "rank")?;
    ensure!(
        (1..=50).contains(&ward) && (1..=50).contains(&rank),
        "invalid ward rank"
    );
    rect(&mut canvas, 0., 0., 1080., 8., BLUE);
    for i in 0..4 {
        star(&mut canvas, 874. + i as f32 * 47., 44.);
    }
    text(
        &mut canvas,
        &body,
        if permit {
            "THE WARD"
        } else {
            "THE WARD SO FAR"
        },
        42.,
        55.,
        21.,
        BLUE,
    );
    fitted(
        &mut canvas,
        &big,
        &format!("WARD {ward}"),
        42.,
        133.,
        995.,
        83.,
        0xffffff,
    );
    let rank = format!(
        "{}#{rank} OF 50",
        if snapshot["tied"] == true {
            "TIED "
        } else {
            ""
        }
    );
    let detail = if permit {
        format!(
            "{} ISSUED PERMITS  /  {} SITES  /  {rank}",
            count(snapshot, "permits")?,
            count(snapshot, "sites")?
        )
    } else {
        format!(
            "{} ADUs  /  {} APPLICATIONS  /  {rank}",
            count(snapshot, "adus")?,
            count(snapshot, "applications")?
        )
    };
    fitted(&mut canvas, &body, &detail, 43., 171., 993., 24., BLUE);
    for (point, (anchor, placed, _)) in points.iter().zip(positions(&points, &c, &focus.id)) {
        if point.id == focus.id {
            continue;
        }
        if (placed[0] - anchor[0]).hypot(placed[1] - anchor[1]) > 4. {
            let mut b = PathBuilder::new();
            b.move_to(anchor[0] as f32 * 2., (anchor[1] as f32 + 194.) * 2.);
            b.line_to(placed[0] as f32 * 2., (placed[1] as f32 + 194.) * 2.);
            if let Some(p) = b.finish() {
                stroke(&mut canvas, &p, 0x354f60, 1.5);
            }
        }
        dot(&mut canvas, &body, placed, point.quantity, false);
    }
    dot(
        &mut canvas,
        &body,
        c.project(focus.location),
        focus.quantity,
        true,
    );
    let frame =
        PathBuilder::from_rect(Rect::from_xywh(40., 428., 2080., 1520.).context("map frame")?);
    stroke(&mut canvas, &frame, 0xb1c4ce, 1.);
    let date = chrono::NaiveDate::parse_from_str(
        snapshot["as_of"].as_str().context("ward date")?,
        "%Y-%m-%d",
    )?
    .format("%B %-d, %Y")
    .to_string();
    let caption = if permit {
        format!(
            "Linked permits at preapproved sites · As of {date} · {}/{} sites mapped",
            count(snapshot, "mapped_sites")?,
            count(snapshot, "sites")?
        )
    } else {
        format!("Applications submitted since April 1, 2026 · As of {date}")
    };
    text(&mut canvas, &body, &caption, 42., 1027., 19., 0xcad1d5);
    let credits =
        "Data: City of Chicago Data Portal · © OpenMapTiles · © OpenStreetMap contributors";
    text(
        &mut canvas,
        &body,
        credits,
        1038. - width(&body, credits, 12.),
        1061.,
        12.,
        0xc1c9cc,
    );
    rect(&mut canvas, 0., 1072., 1080., 8., BLUE);
    drop(big);
    drop(body);
    let mut rgb = canvas.take();
    let pixels = rgb.len() / 4;
    for i in 0..pixels {
        rgb.copy_within(i * 4..i * 4 + 3, i * 3);
    }
    rgb.truncate(pixels * 3);
    Ok(media::jpeg(&rgb, 2160, 2160, media::MAX_IMAGE_BYTES)?.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn projection_preserves_camera_origin_and_rejects_misalignment() {
        let camera = json!({"projection":"web-mercator","center":[-87.7,41.91],"origin":[540.,395.],"world_size":4194304.,"width":1080,"height":800,"pixel_ratio":2,"bearing":0.,"pitch":0.});
        let location = Location {
            longitude: -87.7,
            latitude: 41.91,
        };
        let c: Camera = serde_json::from_value(camera.clone()).unwrap();
        assert_eq!(c.project(location), [540., 395.]);
        let east = c.project(Location {
            longitude: -87.69,
            ..location
        });
        assert!((east[0] - 656.5084444444).abs() < 1e-6);
        let refs = json!([{"location":location,"pixel":{"x":540.,"y":395.}}]);
        validate_camera(&camera, &refs).unwrap();
        let mut shifted = refs;
        shifted[0]["pixel"]["x"] = json!(540.1);
        assert!(validate_camera(&camera, &shifted).is_err());
        let mut tilted = camera;
        tilted["pitch"] = json!(60.);
        assert!(validate_camera(&tilted, &shifted).is_err());
    }
}

#[cfg(test)]
mod live_projection_tests {
    #[test]
    fn camera_matches_actual_maplibre_worker_export_at_ward_edges_and_focus() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ward26-camera.json")).unwrap();
        super::validate_camera(&fixture["camera"], &fixture["references"]).unwrap();
    }
}
