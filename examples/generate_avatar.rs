//! Regenerate the profile artwork with `cargo run --locked --example generate_avatar`.
use std::{fmt::Write, fs, path::Path};

use anyhow::{Context, Result};
use image::{RgbaImage, imageops::FilterType};
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, PathSegment, Pixmap, Stroke, Transform,
};

const BLUE: u32 = 0x41b6e6;
const RED: u32 = 0xe4002b;
const VIEWBOX: f32 = 1000.;
const OUTPUT_SIZE: u32 = 1024;
const RENDER_SIZE: u32 = OUTPUT_SIZE * 2;

struct Shape {
    path: tiny_skia::Path,
    color: u32,
    stroke_width: Option<f32>,
}

fn shape(path: PathBuilder, color: u32, stroke_width: Option<f32>) -> Result<Shape> {
    Ok(Shape {
        path: path.finish().context("empty avatar path")?,
        color,
        stroke_width,
    })
}

fn artwork() -> Result<Vec<Shape>> {
    let mut walls = PathBuilder::new();
    walls.move_to(250., 390.);
    walls.line_to(250., 750.);
    walls.quad_to(250., 780., 280., 780.);
    walls.line_to(720., 780.);
    walls.quad_to(750., 780., 750., 750.);
    walls.line_to(750., 390.);

    let mut roof = PathBuilder::new();
    roof.move_to(190., 438.);
    roof.line_to(500., 188.);
    roof.line_to(810., 438.);

    let mut garage = PathBuilder::new();
    garage.move_to(350., 780.);
    garage.line_to(350., 654.);
    garage.quad_to(350., 640., 364., 640.);
    garage.line_to(636., 640.);
    garage.quad_to(650., 640., 650., 654.);
    garage.line_to(650., 780.);

    let mut panel = PathBuilder::new();
    panel.move_to(350., 700.);
    panel.line_to(650., 700.);

    // Chicago's star uses a 14-inch height around a six-inch inner circle.
    // https://design.chicago.gov/basics/
    let mut star = PathBuilder::new();
    for i in 0..12 {
        let angle = (i as f32 * 30. - 90.).to_radians();
        let radius = if i % 2 == 0 { 135. } else { 135. * 3. / 7. };
        let x = 500. + radius * angle.cos();
        let y = 460. + radius * angle.sin();
        if i == 0 {
            star.move_to(x, y);
        } else {
            star.line_to(x, y);
        }
    }
    star.close();

    Ok(vec![
        shape(walls, BLUE, Some(64.))?,
        shape(roof, BLUE, Some(64.))?,
        shape(garage, BLUE, Some(28.))?,
        shape(panel, BLUE, Some(20.))?,
        shape(star, RED, None)?,
    ])
}

fn svg(shapes: &[Shape]) -> Result<String> {
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{OUTPUT_SIZE}\" height=\"{OUTPUT_SIZE}\" viewBox=\"0 0 1000 1000\" role=\"img\" aria-labelledby=\"title desc\">\n\
         <title id=\"title\">Chicago ADU Preapprovals</title>\n\
         <desc id=\"desc\">A red six-point Chicago star inside a blue coach house outline with a garage door, on white.</desc>\n\
         <rect width=\"1000\" height=\"1000\" fill=\"#FFFFFF\"/>\n"
    );
    for shape in shapes {
        let mut data = String::new();
        for segment in shape.path.segments() {
            match segment {
                PathSegment::MoveTo(p) => write!(data, "M{:.3} {:.3}", p.x, p.y)?,
                PathSegment::LineTo(p) => write!(data, "L{:.3} {:.3}", p.x, p.y)?,
                PathSegment::QuadTo(c, p) => {
                    write!(data, "Q{:.3} {:.3} {:.3} {:.3}", c.x, c.y, p.x, p.y)?;
                }
                PathSegment::CubicTo(a, b, p) => {
                    write!(
                        data,
                        "C{:.3} {:.3} {:.3} {:.3} {:.3} {:.3}",
                        a.x, a.y, b.x, b.y, p.x, p.y
                    )?;
                }
                PathSegment::Close => data.push('Z'),
            }
        }
        if let Some(width) = shape.stroke_width {
            writeln!(
                svg,
                "<path d=\"{data}\" fill=\"none\" stroke=\"#{:06X}\" stroke-width=\"{width}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>",
                shape.color
            )?;
        } else {
            writeln!(svg, "<path d=\"{data}\" fill=\"#{:06X}\"/>", shape.color)?;
        }
    }
    svg.push_str("</svg>\n");
    Ok(svg)
}

fn png(shapes: &[Shape]) -> Result<RgbaImage> {
    let mut canvas = Pixmap::new(RENDER_SIZE, RENDER_SIZE).context("allocate avatar canvas")?;
    canvas.fill(Color::WHITE);
    let scale = RENDER_SIZE as f32 / VIEWBOX;
    let transform = Transform::from_scale(scale, scale);
    for shape in shapes {
        let mut paint = Paint::default();
        paint.set_color_rgba8(
            (shape.color >> 16) as u8,
            (shape.color >> 8) as u8,
            shape.color as u8,
            255,
        );
        if let Some(width) = shape.stroke_width {
            let stroke = Stroke {
                width,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Stroke::default()
            };
            canvas.stroke_path(&shape.path, &paint, &stroke, transform, None);
        } else {
            canvas.fill_path(&shape.path, &paint, FillRule::Winding, transform, None);
        }
    }
    // The opaque white canvas means tiny-skia's premultiplied pixels are also straight RGBA.
    let image = RgbaImage::from_raw(RENDER_SIZE, RENDER_SIZE, canvas.take())
        .context("invalid avatar pixel buffer")?;
    Ok(image::imageops::resize(
        &image,
        OUTPUT_SIZE,
        OUTPUT_SIZE,
        FilterType::Lanczos3,
    ))
}

fn main() -> Result<()> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/profile");
    fs::create_dir_all(&directory).context("create profile artwork directory")?;
    let shapes = artwork()?;
    fs::write(directory.join("avatar.svg"), svg(&shapes)?).context("write avatar SVG")?;
    png(&shapes)?
        .save(directory.join("avatar.png"))
        .context("write avatar PNG")?;
    println!("Generated {}", directory.display());
    Ok(())
}
