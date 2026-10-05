use adu_bot::ward_card;
use image::{ImageBuffer, ImageFormat, Rgb};
use serde_json::{Value, json};
use std::io::Cursor;

fn red_near(bytes: &[u8], x: i32, y: i32) -> usize {
    let image = image::load_from_memory(bytes).unwrap().to_rgb8();
    assert_eq!(image.dimensions(), (2160, 2160));
    let mut count = 0;
    for py in y - 70..=y + 70 {
        for px in x - 70..=x + 70 {
            let p = image.get_pixel(px as u32, py as u32).0;
            if p[0] > 150 && p[1] < 80 && p[2] < 100 {
                count += 1;
            }
        }
    }
    count
}
#[test]
fn native_ward_repaints_current_focus_and_permit_counts_on_same_background() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/ward26-camera.json")).unwrap();
    let camera = &fixture["camera"];
    let center = camera["center"].as_array().unwrap();
    let lon = center[0].as_f64().unwrap();
    let lat = center[1].as_f64().unwrap();
    let a = json!({"id":"a","address":"1714 N KEDZIE AVE","quantity":2,"location":{"longitude":lon,"latitude":lat}});
    let b = json!({"id":"b","address":"A LONG PUBLIC ADDRESS","quantity":1,"location":{"longitude":lon+0.01,"latitude":lat}});
    let mut snapshot = json!({"mode":"scorecard","ward":26,"rank":1,"tied":false,"as_of":"2026-10-05","adus":3,"applications":2,"focus":a,"points":[a,b]});
    let base = ImageBuffer::from_pixel(2160, 1600, Rgb([255u8, 255, 255]));
    let mut png = Cursor::new(Vec::new());
    base.write_to(&mut png, ImageFormat::Png).unwrap();
    drop(base);
    let first = ward_card::compose(&snapshot, png.get_ref(), camera).unwrap();
    assert!(first.len() <= 2_000_000);
    assert!(red_near(&first, 1080, 1188) > 200);
    let moved_x = 1080 + (camera["world_size"].as_f64().unwrap() * 0.01 / 360. * 2.).round() as i32;
    assert_eq!(red_near(&first, moved_x, 1188), 0);
    snapshot["focus"] = snapshot["points"][1].clone();
    snapshot["mode"] = json!("permit");
    snapshot["permits"] = json!(3);
    snapshot["sites"] = json!(2);
    snapshot["mapped_sites"] = json!(2);
    let second = ward_card::compose(&snapshot, png.get_ref(), camera).unwrap();
    assert!(red_near(&second, moved_x, 1188) > 200);
    assert_eq!(red_near(&second, 1080, 1188), 0);
    assert_ne!(first, second);
}
