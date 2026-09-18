use super::*;
use buffa::Message as _;
use fast_mvt::{MvtReaderRef, proto};

#[test]
fn overviews_preserve_styling_and_fit_browser_preparation() {
    for (zoom, bytes) in [
        (
            0,
            include_bytes!(concat!(env!("GARMIN_MAP_FIXTURES_DIR"), "/world-z0.pbf")).as_slice(),
        ),
        (
            2,
            include_bytes!(concat!(
                env!("GARMIN_MAP_FIXTURES_DIR"),
                "/europe-africa-z2.pbf"
            ))
            .as_slice(),
        ),
        (
            3,
            include_bytes!(concat!(
                env!("GARMIN_MAP_FIXTURES_DIR"),
                "/europe-africa-z3.pbf"
            ))
            .as_slice(),
        ),
        (
            3,
            include_bytes!(concat!(env!("GARMIN_MAP_FIXTURES_DIR"), "/asia-z3.pbf")).as_slice(),
        ),
        (
            7,
            include_bytes!(concat!(env!("GARMIN_MAP_FIXTURES_DIR"), "/london-z7.pbf")).as_slice(),
        ),
        (
            8,
            include_bytes!(concat!(env!("GARMIN_MAP_FIXTURES_DIR"), "/london-z8.pbf")).as_slice(),
        ),
        (
            9,
            include_bytes!(concat!(env!("GARMIN_MAP_FIXTURES_DIR"), "/london-z9.pbf")).as_slice(),
        ),
        (
            11,
            include_bytes!(concat!(env!("GARMIN_MAP_FIXTURES_DIR"), "/london-z11.pbf")).as_slice(),
        ),
        (
            10,
            include_bytes!(concat!(env!("GARMIN_MAP_FIXTURES_DIR"), "/london-z10.pbf")).as_slice(),
        ),
    ] {
        for dark in [false, true] {
            let style = super::super::tile_store::map_style(dark);
            assert_render_parity(bytes, &style, zoom);
            let tile = decode(bytes, &style, zoom, 512).unwrap();
            let local = super::super::gpu_map::prepare_local_browser_tile(tile.clone()).unwrap();
            assert!(local.into_prepared().is_some());
            let transfer = super::super::gpu_map::encode_browser_tile(tile).unwrap();
            assert!(transfer.parts().iter().all(|part| !part.is_empty()));
        }
    }
}

fn feature(kind: proto::GeomType, geometry: &[u32]) -> proto::Feature {
    proto::Feature {
        r#type: Some(kind),
        geometry: geometry.to_vec(),
        tags: vec![0, 0],
        ..Default::default()
    }
}

fn tile(features: Vec<proto::Feature>, value: &str) -> Vec<u8> {
    proto::Tile {
        layers: vec![proto::Layer {
            name: "test".to_owned(),
            version: 2,
            extent: Some(4096),
            features,
            keys: vec!["name".to_owned()],
            values: vec![proto::Value {
                string_value: Some(value.to_owned()),
                ..Default::default()
            }],
        }],
    }
    .encode_to_vec()
}

fn style() -> Style {
    serde_json::from_value(walkers::json!({"layers": [
        {"type": "background", "paint": {"background-color": "#123456"}},
        {"type": "fill", "source-layer": "test", "paint": {"fill-color": "#246824", "fill-opacity": 0.5}},
        {"type": "line", "source-layer": "test", "paint": {"line-color": "#112233", "line-width": 2}},
        {"type": "symbol", "source-layer": "test", "layout": {"text-field": "{name}", "text-size": 12}}
    ]})).unwrap()
}

fn assert_render_parity(bytes: &[u8], style: &Style, zoom: u8) {
    let Tile::Vector {
        shapes: expected,
        texts: labels,
    } = Tile::from_mvt(bytes, style, zoom, 512).unwrap()
    else {
        panic!("vector")
    };
    let Tile::Vector { shapes, texts } = decode(bytes, style, zoom, 512).unwrap() else {
        panic!("vector")
    };
    assert!(!shapes.is_empty());
    assert_eq!(shapes, expected);
    assert_eq!(texts.len(), labels.len());
    for (text, label) in texts.iter().zip(labels) {
        assert_eq!(text.text, label.text);
        assert_eq!(text.position, label.position);
        assert_eq!(text.text_color, label.text_color);
        assert_eq!(text.halo_color, label.halo_color);
        assert_eq!(text.placement, label.placement);
        assert!((text.font_size - label.font_size).abs() < f32::EPSILON);
        assert!((text.angle - label.angle).abs() < f32::EPSILON);
        assert!((text.halo_width - label.halo_width).abs() < f32::EPSILON);
    }
}

#[test]
fn browser_decoder_preserves_styled_points_lines_polygons_and_holes() {
    let bytes = tile(
        vec![
            feature(proto::GeomType::Point, &[17, 2, 4, 6, 8]),
            feature(proto::GeomType::Linestring, &[9, 20, 20, 18, 10, 0, 0, 10]),
            feature(
                proto::GeomType::Linestring,
                &[9, 0, 0, 10, 10, 10, 9, 10, 10, 10, 10, 10],
            ),
            feature(
                proto::GeomType::Polygon,
                &[
                    9, 0, 0, 26, 20, 0, 0, 20, 19, 0, 15, 9, 4, 15, 26, 0, 12, 12, 0, 0, 11, 15, 9,
                    24, 3, 26, 20, 0, 0, 20, 19, 0, 15,
                ],
            ),
        ],
        "Example 🚲",
    );
    assert_render_parity(&bytes, &style(), 14);
}

#[test]
fn browser_decoder_preserves_real_style_place_labels() {
    let bytes = fixture(include_str!(concat!(
        env!("GARMIN_MAP_FIXTURES_DIR"),
        "/place.pbf.hex"
    )));
    for dark in [false, true] {
        assert_render_parity(&bytes, &super::super::tile_store::map_style(dark), 14);
    }
}

fn fixture(hex: &str) -> Vec<u8> {
    hex.trim()
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|s| u8::from_str_radix(std::str::from_utf8(s).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn browser_decoder_bounds_shared_properties_without_expanding_unused_layers() {
    let bytes = tile(
        vec![feature(proto::GeomType::Point, &[9, 0, 0]); 3000],
        &"x".repeat(4096),
    );
    assert!(bytes.len() < 64 * 1024);
    let error = decode(&bytes, &style(), 14, 512).err().unwrap();
    assert!(error.contains("expanded property limit"), "{error}");
    let Tile::Vector { shapes, texts } = decode(&bytes, &Style::default(), 14, 512).unwrap() else {
        panic!("vector")
    };
    assert!(shapes.is_empty());
    assert!(texts.is_empty());
}

#[test]
fn browser_decoder_enforces_metadata_budget_inside_codec() {
    let bytes = tile(vec![proto::Feature::default(); 128], "");
    let error = MvtReaderRef::with_decode_options(
        &bytes,
        &buffa::DecodeOptions::new().with_element_memory_limit(128),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        fast_mvt::MvtError::Decode(buffa::DecodeError::ElementMemoryLimitExceeded)
    ));
    let bytes = proto::Tile {
        layers: vec![proto::Layer {
            name: "unused".to_owned(),
            version: 2,
            values: vec![proto::Value::default(); 200_000],
            ..Default::default()
        }],
    }
    .encode_to_vec();
    assert!(bytes.len() < 512 * 1024);
    let error = decode(&bytes, &Style::default(), 3, 512).err().unwrap();
    assert!(error.contains("element memory limit exceeded"), "{error}");
}

#[test]
fn browser_decoder_retains_feature_and_property_reference_limits() {
    let bytes = tile(vec![proto::Feature::default(); 8193], "");
    let error = BrowserTileLimits::read(&bytes).unwrap_err();
    assert!(error.contains("layer or feature limit"), "{error}");

    for count in [131_072, 131_073] {
        let mut point = feature(proto::GeomType::Point, &[9, 0, 0]);
        point.tags = vec![0; count * 2];
        let bytes = tile(vec![point], "");
        let result = decode(&bytes, &style(), 3, 512);
        if count == 131_072 {
            result.unwrap();
        } else {
            let error = result.err().unwrap();
            assert!(error.contains("expanded property limit"), "{error}");
        }
    }
}

#[test]
fn browser_decoder_handles_extreme_polygon_without_arithmetic_trap() {
    let bytes = fixture(include_str!(concat!(
        env!("GARMIN_MAP_FIXTURES_DIR"),
        "/extreme-polygon.pbf.hex"
    )));
    let reader = BrowserTileLimits::read(&bytes).unwrap();
    let geometry = reader
        .layers()
        .next()
        .unwrap()
        .features()
        .next()
        .unwrap()
        .geometry()
        .unwrap();
    assert!(matches!(geometry, Geometry::Polygon(_)));
    let error = decode(&bytes, &super::super::tile_store::map_style(true), 14, 512)
        .err()
        .unwrap();
    assert!(error.contains("coordinates exceeded"), "{error}");
}

#[test]
fn browser_decoder_rejects_malformed_tags_and_truncated_geometry() {
    for tags in [vec![0], vec![1, 0], vec![0, 1]] {
        let mut point = feature(proto::GeomType::Point, &[9, 0, 0]);
        point.tags = tags;
        assert!(decode(&tile(vec![point], ""), &style(), 14, 512).is_err());
    }
    for kind in [
        proto::GeomType::Point,
        proto::GeomType::Linestring,
        proto::GeomType::Polygon,
    ] {
        for commands in [&[0xffff_fff9, 0, 0][..], &[9, 0], &[9, 0, 0, 0xffff_fffa]] {
            assert!(decode(&tile(vec![feature(kind, commands)], ""), &style(), 14, 512).is_err());
        }
    }
}
