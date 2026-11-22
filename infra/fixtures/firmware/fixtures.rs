//! Synthetic device inventories paired with redacted firmware service responses.

pub struct Capture {
    pub inventory: &'static str,
    pub catalog: &'static str,
}

macro_rules! capture {
    ($stem:literal) => {
        Capture {
            inventory: include_str!(concat!($stem, ".xml")),
            catalog: include_str!(concat!($stem, ".json")),
        }
    };
}

pub const EDGE_31_29: Capture = capture!("edge-1050-31.29");
pub const EDGE_32_20: Capture = capture!("edge-1050-32.20");
pub const FENIX_22_44: Capture = capture!("fenix-8-solar-47mm-22.44");
pub const FENIX_24_20: Capture = capture!("fenix-8-solar-47mm-24.20");
pub const FENIX_GSP_22_44: Capture = capture!("fenix-8-solar-47mm-gsp-22.44");

pub const GSP_METADATA: &str = include_str!("fenix-8-solar-47mm-gsp-metadata.json");
pub const VIDEO_METADATA: &str = include_str!("fenix-8-solar-47mm-video-metadata.json");
pub const FONT_METADATA: &str = include_str!("fenix-8-solar-47mm-font-metadata.json");
pub const STORAGE_ROUTING: &str = include_str!("storage-routing.xml");
pub const AMBIGUOUS_DESTINATION: &str = include_str!("ambiguous-destination.xml");
