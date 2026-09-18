//! One bounded MVT decode shared by native, browser worker, and browser fallback.

use std::collections::HashMap;

use egui::{Color32, Rect, Shape, pos2, vec2};
use fast_mvt::MvtValueRef;
use geo::{CoordsIter as _, MapCoords as _};
use geo_types::{Coord, Geometry};
use walkers::{Context, Layer, Paint, Style, Tile};

use super::gpu_map::{BrowserTileLimits, TileBudget, TileDecodeBudget};

#[cfg(test)]
#[path = "tile_decode_tests.rs"]
mod tests;

struct Feature {
    geometry: Geometry<f32>,
    context: Context,
}

struct Source {
    name: String,
    features: Vec<Feature>,
}

pub(super) fn decode(bytes: &[u8], style: &Style, zoom: u8, size: u32) -> Result<Tile, String> {
    let reader = BrowserTileLimits::read(bytes)?;
    if bytes.is_empty() {
        return Ok(Tile::Vector {
            shapes: Vec::new(),
            texts: Vec::new(),
        });
    }
    let mut budget = TileDecodeBudget::default();
    let mut sources = Vec::new();
    for layer in reader.layers() {
        if !style
            .layers
            .iter()
            .any(|rule| source(rule).is_some_and(|(name, _)| name.matches(layer.name())))
        {
            continue;
        }
        // Keep Walkers' existing 4096-extent contract; never divide by untrusted extent.
        if layer.extent() != 4096 {
            continue;
        }
        let mut features = Vec::new();
        for feature in layer.features() {
            budget.feature(feature)?;
            let geometry = feature.geometry().map_err(|error| error.to_string())?;
            budget.geometry(&geometry)?;
            if geometry
                .coords_iter()
                .any(|p| p.x.unsigned_abs() > 65_536 || p.y.unsigned_abs() > 65_536)
            {
                return Err("map tile coordinates exceeded the supported tile buffer".to_owned());
            }
            let properties = feature
                .properties()
                .map(|property| property.map(|(key, value)| (key.to_owned(), json_value(value))))
                .collect::<Result<HashMap<_, _>, _>>()
                .map_err(|error| error.to_string())?;
            let kind = match feature.geom_type_value() {
                Some(1) => "Point",
                Some(2) => "LineString",
                Some(3) => "Polygon",
                _ => return Err("map tile contained unsupported geometry".to_owned()),
            };
            features.push(Feature {
                geometry: scaled(&geometry, size),
                context: Context::new(kind.to_owned(), properties, zoom),
            });
        }
        sources.push(Source {
            name: layer.name().to_owned(),
            features,
        });
    }
    render(&sources, style, zoom, size)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "coordinates are bounded to the tile buffer before conversion"
)]
fn scaled(geometry: &Geometry<i32>, size: u32) -> Geometry<f32> {
    let scale = size as f32 / 4096.0;
    geometry.map_coords(|p| Coord {
        x: p.x as f32 * scale,
        y: p.y as f32 * scale,
    })
}

fn source(layer: &Layer) -> Option<(&walkers::SourceLayer, Option<&walkers::Filter>)> {
    match layer {
        Layer::Fill {
            source_layer,
            filter,
            ..
        }
        | Layer::Line {
            source_layer,
            filter,
            ..
        }
        | Layer::Symbol {
            source_layer,
            filter,
            ..
        } => Some((source_layer, filter.as_ref())),
        _ => None,
    }
}

#[expect(clippy::cast_precision_loss, reason = "tile size is 512 pixels")]
fn render(sources: &[Source], style: &Style, zoom: u8, size: u32) -> Result<Tile, String> {
    let mut shapes = Vec::new();
    let mut texts = Vec::new();
    let mut budget = TileBudget::default();
    for rule in &style.layers {
        if let Layer::Background { paint } = rule {
            let context = Context::new("None".to_owned(), HashMap::new(), zoom);
            let color = paint
                .background_color
                .as_ref()
                .map_or(Color32::WHITE, |c| c.evaluate(&context));
            shapes.push(Shape::rect_filled(
                Rect::from_min_size(pos2(0.0, 0.0), vec2(size as f32, size as f32)),
                0.0,
                color,
            ));
            budget.shapes(&shapes[shapes.len() - 1..])?;
            continue;
        }
        let Some((name, filter)) = source(rule) else {
            continue;
        };
        for feature in sources
            .iter()
            .filter(|s| name.matches(&s.name))
            .flat_map(|s| &s.features)
        {
            if filter.is_some_and(|filter| !filter.matches(&feature.context)) {
                continue;
            }
            let start = shapes.len();
            match rule {
                Layer::Fill { paint, .. } => fill(feature, paint, &mut shapes)?,
                Layer::Line { paint, .. } => {
                    walkers::render_line(&feature.geometry, &feature.context, &mut shapes, paint)
                        .map_err(|e| e.to_string())?;
                }
                Layer::Symbol { layout, paint, .. } => {
                    if let Some(text) = layout.text(&feature.context) {
                        budget.repeated_text(text.len(), label_count(&feature.geometry))?;
                    }
                    walkers::render_symbol(
                        &feature.geometry,
                        &feature.context,
                        &mut texts,
                        layout,
                        paint,
                    )
                    .map_err(|e| e.to_string())?;
                }
                _ => {}
            }
            budget.shapes(&shapes[start..])?;
        }
    }
    Ok(Tile::Vector { shapes, texts })
}

fn label_count(geometry: &Geometry<f32>) -> usize {
    match geometry {
        Geometry::Point(_) | Geometry::LineString(_) => 1,
        Geometry::MultiPoint(points) => points.0.len(),
        Geometry::MultiLineString(lines) => lines.0.len(),
        _ => 0,
    }
}

fn fill(feature: &Feature, paint: &Paint, shapes: &mut Vec<Shape>) -> Result<(), String> {
    let Some(color) = &paint.fill_color else {
        return Ok(());
    };
    let color = color.evaluate(&feature.context).gamma_multiply(
        paint
            .fill_opacity
            .as_ref()
            .map_or(1.0, |opacity| opacity.evaluate(&feature.context)),
    );
    let polygons = match &feature.geometry {
        Geometry::Polygon(polygon) => std::slice::from_ref(polygon),
        Geometry::MultiPolygon(polygons) => &polygons.0,
        _ => return Ok(()),
    };
    for polygon in polygons {
        let points = |ring: &geo_types::LineString<f32>| {
            ring.0
                .iter()
                .map(|p| lyon_path::math::point(p.x, p.y))
                .collect::<Vec<_>>()
        };
        let exterior = points(polygon.exterior());
        let interiors = polygon.interiors().iter().map(points).collect::<Vec<_>>();
        let mesh =
            walkers::tessellate_polygon(&exterior, &interiors, color).map_err(|e| e.to_string())?;
        shapes.push(Shape::mesh(mesh));
    }
    Ok(())
}

fn json_value(value: MvtValueRef<'_>) -> serde_json::Value {
    use serde_json::Value;
    match value {
        MvtValueRef::String(s) => Value::String(s.to_owned()),
        MvtValueRef::Int(n) | MvtValueRef::SInt(n) => n.into(),
        MvtValueRef::Double(n) => {
            serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
        }
        MvtValueRef::Bool(b) => b.into(),
        // Preserve the existing style adapter's treatment of unsupported numeric variants.
        MvtValueRef::Float(_) | MvtValueRef::UInt(_) | MvtValueRef::Null => Value::Null,
    }
}
