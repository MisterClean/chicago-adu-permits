//! Native announcement cards. Street View is kept intact, including Google's attribution.
use crate::{config::Config, normalize::Observation, permits::Permit, render};
use anyhow::{Context, Result, ensure};
use fontdue::{Font, FontSettings};
use image::{ExtendedColorType, ImageReader, RgbImage, codecs::jpeg::JpegEncoder};
use serde_json::{Value, json};
use std::io::Cursor;
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

// Bluesky social-app IMAGE_SIZE_CONFIG_POSTS and app.bsky.embed.images (September 2026).
pub const MAX_IMAGE_BYTES: usize = 2_000_000;
pub const WIDTH: u32 = 3200;
pub const HEIGHT: u32 = 4000;
const SCALE: f32 = WIDTH as f32 / 1080.;
const BLUE: u32 = 0x41b6e6;
const RED: u32 = 0xe4002b;
const GREEN: u32 = 0x29a366;
const WHITE: u32 = 0xffffff;
const GRAY: u32 = 0xb3b3b3;

pub struct PostImage {
    pub bytes: Vec<u8>,
    pub alt: String,
    pub quality: u8,
    pub width: u32,
    pub height: u32,
}
impl PostImage {
    pub fn embed(&self, blob: Value) -> Value {
        json!({"$type":"app.bsky.embed.images","images":[{"image":blob,"alt":self.alt,"aspectRatio":{"width":self.width,"height":self.height}}]})
    }
}

pub fn render(config: &Config, obs: &Observation) -> Result<PostImage> {
    let photo = street_view(config, obs)?;
    render_card(obs, &photo)
}

fn street_view(config: &Config, obs: &Observation) -> Result<RgbImage> {
    let key = match &config.media.google_api_key_file {
        Some(path) => std::fs::read_to_string(path).context("read Google Maps API key file")?,
        None => std::env::var("GOOGLE_MAPS_API_KEY")
            .context("set GOOGLE_MAPS_API_KEY or media.google_api_key_file for Street View")?,
    };
    ensure!(!key.trim().is_empty(), "Google Maps API key is empty");
    let address = obs
        .text("address")
        .map(render::clean)
        .filter(|s| !s.is_empty())
        .context("project address required for Street View")?;
    let mut url = url::Url::parse("https://maps.googleapis.com/maps/api/streetview")?;
    url.query_pairs_mut()
        .append_pair("size", "640x360")
        .append_pair("location", &format!("{address}, Chicago, IL"))
        .append_pair("source", "outdoor")
        .append_pair("fov", "80")
        .append_pair("pitch", "0")
        .append_pair("return_error_code", "true")
        .append_pair("key", key.trim());
    let bytes = fetch(config, url.as_str())?;
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(24_000_000);
    reader.limits(limits);
    let photo = reader
        .decode()
        .context("decode Street View image")?
        .into_rgb8();
    ensure!(
        photo.dimensions() == (640, 360),
        "unexpected Street View dimensions"
    );
    Ok(photo)
}

pub fn render_permit(config: &Config, obs: &Observation, permit: &Permit) -> Result<PostImage> {
    let photo = street_view(config, obs)?;
    render_permit_card(obs, permit, &photo)
}

pub fn render_permit_card(
    obs: &Observation,
    permit: &Permit,
    photo: &RgbImage,
) -> Result<PostImage> {
    // Keep the established Chicago type, color, Street View, and source treatment.
    let mut canvas = Pixmap::new(WIDTH, HEIGHT).context("allocate permit card")?;
    canvas.fill(hex(0x000000));
    let headline = Font::from_bytes(
        include_bytes!("../assets/fonts/BigShouldersText-Bold.ttf") as &[u8],
        FontSettings::default(),
    )
    .map_err(|_| anyhow::anyhow!("invalid headline font"))?;
    let body = Font::from_bytes(
        include_bytes!("../assets/fonts/Roboto.ttf") as &[u8],
        FontSettings::default(),
    )
    .map_err(|_| anyhow::anyhow!("invalid body font"))?;
    rect(&mut canvas, 0., 0., 1080., 10., BLUE);
    for i in 0..4 {
        star(&mut canvas, 850. + i as f32 * 51., 58., 18.);
    }
    text(&mut canvas, &body, "BUILDING PERMIT", 48., 139., 25., BLUE);
    check_badge(&mut canvas, 82., 245., 34.);
    fitted(
        &mut canvas,
        &headline,
        "ADU BUILDING",
        132.,
        291.,
        150.,
        WHITE,
    );
    fitted(
        &mut canvas,
        &headline,
        "PERMIT ISSUED",
        48.,
        437.,
        158.,
        WHITE,
    );
    rect(&mut canvas, 48., 475., 66., 7., RED);
    fitted(
        &mut canvas,
        &headline,
        &render::address(obs).to_uppercase(),
        48.,
        548.,
        58.,
        WHITE,
    );
    let mut details = Vec::new();
    if let Some(n) = obs.number("adu_applying_for").filter(|n| *n > 0) {
        details.push(format!(
            "{n} {} PROPOSED",
            if n == 1 { "ADU" } else { "ADUS" }
        ));
    }
    if let Some(ward) = obs.number("ward").filter(|n| (1..=50).contains(n)) {
        details.push(format!("WARD {ward}"));
    }
    if let Some(date) = permit.date("issue_date") {
        details.push(date.format("%b %-d, %Y").to_string().to_uppercase());
    }
    fitted(
        &mut canvas,
        &body,
        &details.join("   /   "),
        48.,
        596.,
        24.,
        BLUE,
    );
    let photo_height = (WIDTH as f64 * photo.height() as f64 / photo.width() as f64).round() as u32;
    ensure!(photo_height <= 1800, "photo must fit the landscape panel");
    let photo = image::imageops::resize(
        photo,
        WIDTH,
        photo_height,
        image::imageops::FilterType::Lanczos3,
    );
    let top = (624. * SCALE).round() as u32;
    for (x, y, pixel) in photo.enumerate_pixels() {
        let offset = (((top + y) * WIDTH + x) * 4) as usize;
        canvas.data_mut()[offset..offset + 3].copy_from_slice(&pixel.0);
        canvas.data_mut()[offset + 3] = 255;
    }
    text(
        &mut canvas,
        &body,
        "ISSUED BY CITY OF CHICAGO",
        48.,
        1281.,
        31.,
        WHITE,
    );
    fitted(
        &mut canvas,
        &body,
        &format!("PERMIT #{}  /  UNOFFICIAL FEED", permit.number),
        48.,
        1321.,
        20.,
        GRAY,
    );
    rect(&mut canvas, 0., 1340., 1080., 10., BLUE);
    drop(headline);
    drop(body);
    let mut rgb = canvas.take();
    let pixels = rgb.len() / 4;
    for pixel in 0..pixels {
        rgb.copy_within(pixel * 4..pixel * 4 + 3, pixel * 3);
    }
    rgb.truncate(pixels * 3);
    let (bytes, quality) = jpeg(&rgb, WIDTH, HEIGHT, MAX_IMAGE_BYTES)?;
    let alt = format!(
        "✅ ADU BUILDING PERMIT ISSUED. {} at {}, Chicago. Permit {} issued {}. {} ADUs proposed in housing preapproval application {}. Street View imagery depicts street-facing context and may predate the project. Data: City of Chicago. Unofficial community feed.",
        render::unit_name(obs),
        render::address(obs),
        permit.number,
        permit
            .date("issue_date")
            .map(|d| d.to_string())
            .unwrap_or_default(),
        obs.number("adu_applying_for")
            .map(|n| n.to_string())
            .unwrap_or_else(|| "Unknown number of".into()),
        obs.id
    );
    Ok(PostImage {
        bytes,
        alt,
        quality,
        width: WIDTH,
        height: HEIGHT,
    })
}

fn fetch(config: &Config, url: &str) -> Result<Vec<u8>> {
    // Do not propagate HTTP-library errors: their URLs can include the API key.
    let mut response = config
        .agent()
        .get(url)
        .call()
        .map_err(|_| anyhow::anyhow!("Street View transport failed"))?;
    ensure!(
        response.status().as_u16() == 200,
        "Street View HTTP {}; card not published",
        response.status().as_u16()
    );
    response
        .body_mut()
        .with_config()
        .limit(4_000_000)
        .read_to_vec()
        .map_err(|_| anyhow::anyhow!("Street View response incomplete or oversized"))
}

pub fn image_alt(obs: &Observation) -> String {
    let mut facts = vec![
        format!(
            "Chicago housing preapproval announcement. {} preapproved at {}, Chicago.",
            render::unit_name(obs),
            render::address(obs)
        ),
        format!(
            "Application {}. City status: {}.",
            obs.id,
            obs.text("status").unwrap_or("unknown")
        ),
    ];
    if let Some(n) = obs.number("adu_applying_for").filter(|n| *n > 0) {
        facts.push(format!(
            "Requested: {n} additional {}.",
            if n == 1 { "home" } else { "homes" }
        ));
    }
    if let Some(ward) = obs.number("ward").filter(|n| (1..=50).contains(n)) {
        facts.push(format!("Ward {ward}."));
    }
    if let Some(date) = render::status_date(obs) {
        facts.push(format!("Preapproval dated {date}."));
    }
    if obs.canonical["conversion_unit"] == true {
        facts.push("The city identifies a conversion within the existing building; it does not specify whether it is a garden, attic, or other apartment.".into());
    }
    facts.push("Building permit still required. Preapproval does not mean construction is permitted or complete. Bold white lettering on black, Chicago flag blue accents and four red stars above Google Street View imagery returned for the project address. The street-facing image may predate the application and may not show the proposed unit or rear coach house. Data: City of Chicago. Unofficial community feed.".into());
    facts.join(" ")
}

/// Compose a 4:5 card without cropping, obscuring, or recoloring Street View imagery.
pub fn render_card(obs: &Observation, photo: &RgbImage) -> Result<PostImage> {
    ensure!(photo.width() > 0 && photo.height() > 0, "empty photo");
    let mut canvas = Pixmap::new(WIDTH, HEIGHT).context("allocate announcement canvas")?;
    canvas.fill(hex(0x000000));
    let headline = Font::from_bytes(
        include_bytes!("../assets/fonts/BigShouldersText-Bold.ttf") as &[u8],
        FontSettings::default(),
    )
    .map_err(|_| anyhow::anyhow!("invalid headline font"))?;
    let body = Font::from_bytes(
        include_bytes!("../assets/fonts/Roboto.ttf") as &[u8],
        FontSettings::default(),
    )
    .map_err(|_| anyhow::anyhow!("invalid body font"))?;
    rect(&mut canvas, 0., 0., 1080., 10., BLUE);
    for i in 0..4 {
        star(&mut canvas, 850. + i as f32 * 51., 58., 18.);
    }
    text(
        &mut canvas,
        &body,
        "HOUSING PREAPPROVAL",
        48.,
        139.,
        25.,
        BLUE,
    );
    let name = render::unit_name(obs).to_uppercase();
    let name = if name == "COACH HOUSE" {
        "NEW COACH HOUSE"
    } else {
        &name
    };
    fitted(&mut canvas, &headline, name, 48., 291., 150., WHITE);
    fitted(
        &mut canvas,
        &headline,
        "PREAPPROVED",
        48.,
        437.,
        158.,
        WHITE,
    );
    rect(&mut canvas, 48., 475., 66., 7., RED);
    let address = render::address(obs).to_uppercase();
    let address = if address.chars().count() > 75 || address.chars().any(|c| !headline.has_glyph(c))
    {
        format!("APPLICATION #{}", obs.id)
    } else {
        address
    };
    fitted(&mut canvas, &headline, &address, 48., 548., 58., WHITE);
    let mut details = Vec::new();
    if let Some(n) = obs.number("adu_applying_for").filter(|n| *n > 0) {
        details.push(format!(
            "{n} {} REQUESTED",
            if n == 1 { "HOME" } else { "HOMES" }
        ));
    }
    if let Some(ward) = obs.number("ward").filter(|n| (1..=50).contains(n)) {
        details.push(format!("WARD {ward}"));
    }
    if let Some(date) = render::status_date(obs) {
        details.push(date.to_uppercase());
    }
    fitted(
        &mut canvas,
        &body,
        &details.join("   /   "),
        48.,
        596.,
        24.,
        BLUE,
    );

    let photo_height = (WIDTH as f64 * photo.height() as f64 / photo.width() as f64).round() as u32;
    ensure!(photo_height <= 1800, "photo must fit the landscape panel");
    let photo = image::imageops::resize(
        photo,
        WIDTH,
        photo_height,
        image::imageops::FilterType::Lanczos3,
    );
    let top = (624. * SCALE).round() as u32;
    for (x, y, pixel) in photo.enumerate_pixels() {
        let offset = (((top + y) * WIDTH + x) * 4) as usize;
        canvas.data_mut()[offset..offset + 3].copy_from_slice(&pixel.0);
        canvas.data_mut()[offset + 3] = 255;
    }
    drop(photo);
    text(
        &mut canvas,
        &body,
        "BUILDING PERMIT STILL REQUIRED",
        48.,
        1281.,
        31.,
        WHITE,
    );
    fitted(
        &mut canvas,
        &body,
        &format!("CITY OF CHICAGO DATA  /  #{}  /  UNOFFICIAL FEED", obs.id),
        48.,
        1321.,
        20.,
        GRAY,
    );
    rect(&mut canvas, 0., 1340., 1080., 10., BLUE);
    drop(headline);
    drop(body);
    // Compact the opaque canvas in place instead of allocating another full-size image.
    let mut rgb = canvas.take();
    let pixels = rgb.len() / 4;
    for pixel in 0..pixels {
        rgb.copy_within(pixel * 4..pixel * 4 + 3, pixel * 3);
    }
    rgb.truncate(pixels * 3);
    let (bytes, quality) = jpeg(&rgb, WIDTH, HEIGHT, MAX_IMAGE_BYTES)?;
    Ok(PostImage {
        bytes,
        alt: image_alt(obs),
        quality,
        width: WIDTH,
        height: HEIGHT,
    })
}

/// Find the highest JPEG quality that fits, preserving native canvas dimensions.
pub fn jpeg(rgb: &[u8], width: u32, height: u32, limit: usize) -> Result<(Vec<u8>, u8)> {
    let encode = |quality| -> Result<Vec<u8>> {
        let mut output = Vec::new();
        JpegEncoder::new_with_quality(&mut output, quality).encode(
            rgb,
            width,
            height,
            ExtendedColorType::Rgb8,
        )?;
        Ok(output)
    };
    let maximum = encode(100)?;
    if maximum.len() <= limit {
        return Ok((maximum, 100));
    }
    let (mut low, mut high) = (1u8, 99u8);
    let mut best = None;
    while low <= high {
        let quality = low + (high - low) / 2;
        let output = encode(quality)?;
        if output.len() <= limit {
            best = Some((output, quality));
            low = quality + 1;
        } else {
            high = quality - 1;
        }
    }
    best.context("cannot encode announcement within Bluesky byte limit")
}
fn hex(value: u32) -> Color {
    Color::from_rgba8((value >> 16) as u8, (value >> 8) as u8, value as u8, 255)
}
fn paint(value: u32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(hex(value));
    p
}
fn rect(canvas: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, color: u32) {
    if let Some(r) = Rect::from_xywh(x * SCALE, y * SCALE, w * SCALE, h * SCALE) {
        canvas.fill_rect(r, &paint(color), Transform::identity(), None);
    }
}
fn star(canvas: &mut Pixmap, x: f32, y: f32, radius: f32) {
    let mut path = PathBuilder::new();
    for i in 0..12 {
        let a = i as f32 * std::f32::consts::PI / 6. - std::f32::consts::PI / 2.;
        let r = if i % 2 == 0 { radius } else { radius * 0.42 };
        let (px, py) = ((x + a.cos() * r) * SCALE, (y + a.sin() * r) * SCALE);
        if i == 0 {
            path.move_to(px, py);
        } else {
            path.line_to(px, py);
        }
    }
    path.close();
    if let Some(path) = path.finish() {
        canvas.fill_path(
            &path,
            &paint(RED),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}
fn check_badge(canvas: &mut Pixmap, x: f32, y: f32, radius: f32) {
    let mut circle = PathBuilder::new();
    circle.push_circle(x * SCALE, y * SCALE, radius * SCALE);
    if let Some(path) = circle.finish() {
        canvas.fill_path(
            &path,
            &paint(GREEN),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let mut check = PathBuilder::new();
    check.move_to((x - radius * 0.48) * SCALE, (y - radius * 0.01) * SCALE);
    check.line_to((x - radius * 0.12) * SCALE, (y + radius * 0.34) * SCALE);
    check.line_to((x + radius * 0.52) * SCALE, (y - radius * 0.34) * SCALE);
    if let Some(path) = check.finish() {
        let stroke = tiny_skia::Stroke {
            width: 7. * SCALE,
            line_cap: tiny_skia::LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Default::default()
        };
        canvas.stroke_path(&path, &paint(WHITE), &stroke, Transform::identity(), None);
    }
}
fn text_width(font: &Font, label: &str, size: f32) -> f32 {
    label
        .chars()
        .map(|c| font.metrics(c, size).advance_width)
        .sum()
}
fn fitted(
    canvas: &mut Pixmap,
    font: &Font,
    label: &str,
    x: f32,
    baseline: f32,
    size: f32,
    color: u32,
) {
    let size = size.min((1080. - x - 48.) / text_width(font, label, 1.).max(1.));
    text(canvas, font, label, x, baseline, size, color);
}
fn text(
    canvas: &mut Pixmap,
    font: &Font,
    label: &str,
    x: f32,
    baseline: f32,
    size: f32,
    color: u32,
) {
    let mut pen = x * SCALE;
    let color = [(color >> 16) as u8, (color >> 8) as u8, color as u8];
    for ch in label.chars() {
        let (metrics, bitmap) = font.rasterize(ch, size * SCALE);
        let top = (baseline * SCALE).round() as i32 - metrics.height as i32 - metrics.ymin;
        let left = pen.round() as i32 + metrics.xmin;
        for gy in 0..metrics.height {
            let py = top + gy as i32;
            if py < 0 || py >= HEIGHT as i32 {
                continue;
            }
            for gx in 0..metrics.width {
                let px = left + gx as i32;
                if px < 0 || px >= WIDTH as i32 {
                    continue;
                }
                let alpha = bitmap[gy * metrics.width + gx] as f32 / 255.;
                if alpha == 0. {
                    continue;
                }
                let offset = (py as usize * WIDTH as usize + px as usize) * 4;
                for (i, value) in color.iter().enumerate() {
                    let old = canvas.data()[offset + i];
                    canvas.data_mut()[offset + i] =
                        (*value as f32 * alpha + old as f32 * (1. - alpha)).round() as u8;
                }
            }
        }
        pen += metrics.advance_width;
    }
}
