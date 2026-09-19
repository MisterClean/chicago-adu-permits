use adu_bot::{media, normalize::Observation, render};
use serde_json::json;

fn obs(coach: serde_json::Value, conversion: serde_json::Value, count: i64) -> Observation {
    Observation::parse(json!({"id":123,"status":"Pre-Certified","address":"100 W TEST ST","ward":1,"coach_house":coach,"conversion_unit":conversion,"adu_applying_for":count,"action_date":"2026-09-01T00:00:00.000"})).unwrap()
}
#[test]
fn headlines_follow_flags_without_inventing_a_floor_or_unit_count() {
    for (coach, conversion, count, name) in [
        (json!(true), json!(false), 1, "Coach house"),
        (json!(false), json!(true), 1, "ADU apartment"),
        (json!(false), json!(true), 2, "ADU apartments"),
        (json!(true), json!(true), 2, "Coach house + apartments"),
        (json!(false), json!(false), 0, "Additional home"),
        (json!("yes"), json!(true), 1, "Additional home"),
        (json!(true), json!("no"), 1, "Additional home"),
    ] {
        let o = obs(coach, conversion, count);
        assert_eq!(render::unit_name(&o), name);
        let record = render::record(&o, chrono::Utc::now()).unwrap();
        let text = record["text"].as_str().unwrap();
        render::validate(&record).unwrap();
        assert!(!text.contains("0 ADUs"));
    }
}
#[test]
fn alt_describes_facts_status_and_limits_of_street_imagery() {
    let alt = media::image_alt(&obs(json!(false), json!(true), 2));
    for fact in [
        "100 W TEST ST",
        "Application 123",
        "Ward 1",
        "Sep 1, 2026",
        "2 additional homes",
        "does not specify",
        "Building permit still required",
        "may predate",
        "Google Street View",
    ] {
        assert!(alt.contains(fact), "missing {fact}");
    }
}
#[test]
fn jpeg_keeps_dimensions_and_selects_maximum_fitting_quality() {
    let mut photo = image::RgbImage::new(128, 128);
    for (x, y, p) in photo.enumerate_pixels_mut() {
        *p = image::Rgb([
            (x * 73 + y * 19) as u8,
            (x * 31 + y * 67) as u8,
            (x * 11 + y * 113) as u8,
        ]);
    }
    let (bytes, quality) = media::jpeg(photo.as_raw(), 128, 128, 12_000).unwrap();
    assert!(bytes.len() <= 12_000);
    assert!(quality < 100);
    let decoded = image::load_from_memory(&bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (128, 128));
    let mut next = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut next, quality + 1)
        .encode(photo.as_raw(), 128, 128, image::ExtendedColorType::Rgb8)
        .unwrap();
    assert!(next.len() > 12_000);
}
#[test]
fn native_card_is_full_resolution_and_has_valid_accessible_embed() {
    let o = obs(json!(true), json!(false), 1);
    let photo = image::RgbImage::from_pixel(640, 360, image::Rgb([90, 110, 130]));
    let card = media::render_card(&o, &photo).unwrap();
    let decoded = image::load_from_memory(&card.bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (3200, 4000));
    assert!(card.bytes.len() <= 2_000_000);
    let mut record = render::record(&o, chrono::Utc::now()).unwrap();
    record["embed"] = card.embed(json!({"$type":"blob","ref":{"$link":"test-cid"},"mimeType":"image/jpeg","size":card.bytes.len()}));
    render::validate(&record).unwrap();
    record["embed"]["images"][0]["alt"] = json!("");
    assert!(render::validate(&record).is_err());
}
