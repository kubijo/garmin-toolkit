use egui::Shape;
use fast_mvt::{MvtFeatureRef, MvtReaderRef, MvtValueRef};
use geo_types::{CoordNum, Geometry};

use super::{BrowserTextRecord, Vertex};

/// Resource policy shared by browser tile preparation and admission.
pub struct BrowserTileLimits {
    encoded_bytes: usize,
    pub(super) upload_bytes: usize,
    pub(super) text_bytes: usize,
    pub(super) prepared_bytes: usize,
    points: usize,
    pub(super) path_points: usize,
    polygon_points: usize,
    features: usize,
    layers: usize,
    metadata_bytes: usize,
    properties: usize,
    property_bytes: usize,
}

impl BrowserTileLimits {
    // Real city overviews exceed 64K input points and 8 MiB after antialiasing; see map fixtures.
    pub(super) const BROWSER: Self = Self {
        encoded_bytes: 2 * 1024 * 1024,
        upload_bytes: 16 * 1024 * 1024,
        text_bytes: 4 * 1024,
        prepared_bytes: 8 * 1024 * 1024,
        points: 131_072,
        path_points: 4_096,
        polygon_points: 8_192,
        features: 8_192,
        layers: 64,
        // Low-zoom multilingual dictionaries need ~4 MiB of borrowed protobuf structs.
        // This bounds codec element storage, not encoded strings or styled output.
        metadata_bytes: 8 * 1024 * 1024,
        // The Asia overview has 68,387 references to its multilingual value dictionary.
        properties: 131_072,
        property_bytes: 8 * 1024 * 1024,
    };

    /// Reject an oversized fetch before copying its bytes into WASM.
    ///
    /// # Errors
    /// Returns an error when the encoded tile exceeds the browser input limit.
    pub fn check_encoded_bytes(bytes: usize) -> Result<(), String> {
        if bytes > Self::BROWSER.encoded_bytes {
            return Err("map tile exceeded the 2 MiB browser limit".to_owned());
        }
        Ok(())
    }

    pub(super) fn check_upload(bytes: usize) -> Result<(), String> {
        if bytes > Self::BROWSER.upload_bytes {
            return Err("prepared map tile exceeded the 16 MiB upload limit".to_owned());
        }
        Ok(())
    }

    pub(super) fn check_text(bytes: usize) -> Result<(), String> {
        if bytes > Self::BROWSER.text_bytes {
            return Err("browser tile text exceeded the 4 KiB limit".to_owned());
        }
        Ok(())
    }

    pub(in crate::activity) fn read(bytes: &[u8]) -> Result<MvtReaderRef<'_>, String> {
        Self::check_encoded_bytes(bytes.len())?;
        let reader = MvtReaderRef::with_decode_options(
            bytes,
            &buffa::DecodeOptions::new()
                .with_max_message_size(Self::BROWSER.encoded_bytes)
                .with_element_memory_limit(Self::BROWSER.metadata_bytes),
        )
        .map_err(|error| error.to_string())?;
        let features = reader.layers().try_fold(0usize, |count, layer| {
            count.checked_add(layer.feature_count())
        });
        if reader.layer_count() > Self::BROWSER.layers
            || features.is_none_or(|count| count > Self::BROWSER.features)
        {
            return Err("browser tile exceeded the layer or feature limit".to_owned());
        }
        Ok(reader)
    }

    pub(in crate::activity) const fn upload_byte_limit() -> usize {
        Self::BROWSER.upload_bytes
    }
}

#[derive(Default)]
pub(in crate::activity) struct TileDecodeBudget {
    points: usize,
    command_words: usize,
    property_bytes: usize,
    properties: usize,
}

impl TileDecodeBudget {
    pub(in crate::activity) fn feature(
        &mut self,
        feature: MvtFeatureRef<'_>,
    ) -> Result<(), String> {
        self.command_words = self
            .command_words
            .saturating_add(feature.geometry_commands().len());
        if self.command_words > BrowserTileLimits::BROWSER.points * 3 {
            return Err("browser tile exceeded the geometry command limit".to_owned());
        }
        for property in feature.properties() {
            let (key, value) = property.map_err(|error| error.to_string())?;
            let bytes = match value {
                MvtValueRef::String(value) => {
                    BrowserTileLimits::check_text(value.len())?;
                    value.len()
                }
                _ => std::mem::size_of::<MvtValueRef<'_>>(),
            };
            self.properties = self.properties.saturating_add(1);
            self.property_bytes = self
                .property_bytes
                .saturating_add(key.len())
                .saturating_add(bytes);
            if self.properties > BrowserTileLimits::BROWSER.properties
                || self.property_bytes > BrowserTileLimits::BROWSER.property_bytes
            {
                return Err("browser tile exceeded the expanded property limit".to_owned());
            }
        }
        Ok(())
    }

    fn path(&mut self, points: usize) -> Result<(), String> {
        if points > BrowserTileLimits::BROWSER.path_points {
            return Err("browser tile exceeded the per-path complexity limit".to_owned());
        }
        self.add_points(points)
    }

    fn add_points(&mut self, points: usize) -> Result<(), String> {
        self.points = self.points.saturating_add(points);
        if self.points > BrowserTileLimits::BROWSER.points {
            return Err("browser tile exceeded the geometry complexity limit".to_owned());
        }
        Ok(())
    }

    pub(in crate::activity) fn geometry<T: CoordNum>(
        &mut self,
        geometry: &Geometry<T>,
    ) -> Result<(), String> {
        match geometry {
            Geometry::Point(_) => self.path(1),
            Geometry::MultiPoint(points) => self.path(points.0.len()),
            Geometry::LineString(line) => self.path(line.0.len()),
            Geometry::MultiLineString(lines) => {
                lines.0.iter().try_for_each(|line| self.path(line.0.len()))
            }
            Geometry::Polygon(polygon) => self.polygon(polygon),
            Geometry::MultiPolygon(polygons) => polygons
                .0
                .iter()
                .try_for_each(|polygon| self.polygon(polygon)),
            _ => Err("browser tile contains unsupported geometry".to_owned()),
        }
    }

    fn polygon<T: CoordNum>(&mut self, polygon: &geo_types::Polygon<T>) -> Result<(), String> {
        let count = polygon
            .interiors()
            .iter()
            .fold(polygon.exterior().0.len(), |count, ring| {
                count.saturating_add(ring.0.len())
            });
        if count > BrowserTileLimits::BROWSER.polygon_points {
            return Err("browser tile exceeded the per-polygon complexity limit".to_owned());
        }
        self.add_points(count)
    }
}

/// Byte accounting for styled geometry and final GPU output.
#[derive(Default)]
pub(in crate::activity) struct TileBudget {
    prepared: usize,
    text: usize,
    mesh: usize,
}

impl TileBudget {
    pub(in crate::activity) fn shapes(&mut self, shapes: &[Shape]) -> Result<(), String> {
        for shape in shapes {
            let heap_bytes = match shape {
                Shape::Path(path) => {
                    if path.points.len() > BrowserTileLimits::BROWSER.path_points {
                        return Err(
                            "browser tile exceeded the per-path complexity limit".to_owned()
                        );
                    }
                    path.points
                        .len()
                        .saturating_mul(std::mem::size_of::<egui::Pos2>())
                }
                Shape::Mesh(mesh) => {
                    self.mesh = self.mesh.saturating_add(mesh_bytes(mesh));
                    BrowserTileLimits::check_upload(self.mesh.saturating_add(self.text))?;
                    mesh.vertices
                        .len()
                        .saturating_mul(std::mem::size_of::<egui::epaint::Vertex>())
                        .saturating_add(
                            mesh.indices
                                .len()
                                .saturating_mul(std::mem::size_of::<u32>()),
                        )
                }
                Shape::Rect(_) | Shape::LineSegment { .. } | Shape::Circle(_) | Shape::Noop => 0,
                _ => return Err("browser tile contains an unsupported shape".to_owned()),
            };
            self.prepared = self
                .prepared
                .saturating_add(std::mem::size_of::<Shape>())
                .saturating_add(heap_bytes);
            self.check_prepared()?;
        }
        Ok(())
    }

    fn check_prepared(&self) -> Result<(), String> {
        if self.prepared.saturating_add(self.text) > BrowserTileLimits::BROWSER.prepared_bytes {
            return Err("browser tile exceeded the 8 MiB styled geometry limit".to_owned());
        }
        Ok(())
    }

    pub(in crate::activity) fn text(&mut self, bytes: usize) -> Result<(), String> {
        self.repeated_text(bytes, 1)
    }

    pub(in crate::activity) fn repeated_text(
        &mut self,
        bytes: usize,
        count: usize,
    ) -> Result<(), String> {
        BrowserTileLimits::check_text(bytes)?;
        self.text = self.text.saturating_add(
            std::mem::size_of::<BrowserTextRecord>()
                .saturating_add(bytes)
                .saturating_mul(count),
        );
        self.check_prepared()?;
        BrowserTileLimits::check_upload(self.text.saturating_add(self.mesh))
    }

    pub(super) fn mesh(&self, mesh: &egui::Mesh) -> Result<(), String> {
        BrowserTileLimits::check_upload(mesh_bytes(mesh).saturating_add(self.text))
    }
}

fn mesh_bytes(mesh: &egui::Mesh) -> usize {
    mesh.vertices
        .len()
        .saturating_mul(std::mem::size_of::<Vertex>())
        .saturating_add(
            mesh.indices
                .len()
                .saturating_mul(std::mem::size_of::<u32>()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polygon_budget_includes_holes_and_keeps_lines_separately_bounded() {
        let ring = |count| geo_types::LineString::from(vec![(0.0, 0.0); count]);
        let limit = BrowserTileLimits::BROWSER.polygon_points;
        let geometry = Geometry::Polygon(geo_types::Polygon::new(ring(limit - 4), vec![ring(4)]));
        let mut budget = TileDecodeBudget::default();
        budget.geometry(&geometry).unwrap();
        assert_eq!(budget.points, limit);
        let oversized = Geometry::Polygon(geo_types::Polygon::new(ring(limit - 4), vec![ring(5)]));
        assert_eq!(
            TileDecodeBudget::default()
                .geometry(&oversized)
                .unwrap_err(),
            "browser tile exceeded the per-polygon complexity limit"
        );
        assert!(
            TileDecodeBudget::default()
                .geometry(&Geometry::LineString(ring(limit)))
                .is_err()
        );
        for _ in 1..BrowserTileLimits::BROWSER.points / limit {
            budget.geometry(&geometry).unwrap();
        }
        assert!(budget.geometry(&geometry).is_err());
    }

    #[test]
    fn styled_paths_have_an_aggregate_memory_budget_not_an_input_point_budget() {
        let path = Shape::line(
            vec![egui::pos2(0.0, 0.0); 1024],
            egui::Stroke::new(1.0, egui::Color32::WHITE),
        );
        let path_bytes = std::mem::size_of::<Shape>() + 1024 * std::mem::size_of::<egui::Pos2>();
        let mut budget = TileBudget::default();
        for _ in 0..BrowserTileLimits::BROWSER.prepared_bytes / path_bytes {
            budget.shapes(std::slice::from_ref(&path)).unwrap();
        }
        assert_eq!(
            budget.shapes(&[path]).unwrap_err(),
            "browser tile exceeded the 8 MiB styled geometry limit"
        );
    }

    #[test]
    fn browser_mesh_budget_counts_upload_bytes_not_triangle_indices_as_input_points() {
        let mesh = egui::Mesh {
            vertices: vec![egui::epaint::Vertex::default(); 3],
            indices: [0, 1, 2].repeat(22_000),
            ..Default::default()
        };
        let mut budget = TileBudget::default();
        budget.shapes(&[Shape::mesh(mesh)]).unwrap();
        assert!(budget.repeated_text(4096, 2048).is_err());
        let mesh = egui::Mesh {
            vertices: vec![egui::epaint::Vertex::default(); 3],
            indices: [0, 1, 2].repeat(BrowserTileLimits::BROWSER.upload_bytes / 12 + 1),
            ..Default::default()
        };
        assert!(TileBudget::default().shapes(&[Shape::mesh(mesh)]).is_err());
    }

    #[test]
    fn browser_preflight_rejects_layer_and_geometry_amplification() {
        let fixture =
            include_str!(concat!(env!("GARMIN_MAP_FIXTURES_DIR"), "/place.pbf.hex")).trim();
        let bytes: Vec<_> = fixture
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|hex| u8::from_str_radix(std::str::from_utf8(hex).unwrap(), 16).unwrap())
            .collect();
        assert!(BrowserTileLimits::read(&bytes).is_ok());
        assert!(
            BrowserTileLimits::read(&bytes.repeat(BrowserTileLimits::BROWSER.layers + 1)).is_err()
        );
        assert!(BrowserTileLimits::check_encoded_bytes(2 * 1024 * 1024 + 1).is_err());
        let line = Geometry::LineString(geo_types::LineString::from(vec![
            (0.0, 0.0);
            BrowserTileLimits::BROWSER.path_points
                + 1
        ]));
        assert!(TileDecodeBudget::default().geometry(&line).is_err());
        let mut budget = TileDecodeBudget::default();
        for _ in 0..BrowserTileLimits::BROWSER.points / 4096 {
            budget.path(4096).unwrap();
        }
        assert!(budget.path(1).is_err());
    }

    #[test]
    fn browser_producer_checks_total_output_before_transfer_allocation() {
        let mut budget = TileBudget::default();
        for _ in 0..2_048 {
            if budget.text(4096).is_err() {
                return;
            }
        }
        panic!("text record overhead must count towards the output cap");
    }

    #[test]
    fn browser_preflight_rejects_many_features_before_styling() {
        // Valid MVT layer "x", with repeated point features at (0, 0).
        let mut layer = vec![120, 2, 10, 1, b'x'];
        for _ in 0..=BrowserTileLimits::BROWSER.features {
            layer.extend_from_slice(&[18, 7, 24, 1, 34, 3, 9, 0, 0]);
        }
        let mut tile = vec![26];
        let mut length = layer.len();
        while length >= 128 {
            tile.push(u8::try_from(length & 127).unwrap() | 128);
            length >>= 7;
        }
        tile.push(u8::try_from(length).unwrap());
        tile.extend_from_slice(&layer);
        assert_eq!(
            BrowserTileLimits::read(&tile).unwrap_err(),
            "browser tile exceeded the layer or feature limit"
        );
    }
}
