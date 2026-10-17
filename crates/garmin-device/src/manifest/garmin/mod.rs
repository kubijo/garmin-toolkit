use std::path::Path;

use super::model::paths_equal;

pub(super) mod v2;

const GARMIN_DEVICE_MANIFEST: &str = "GARMIN/GarminDevice.xml";

pub(super) fn is_device_manifest(path: &Path) -> bool {
    paths_equal(path, Path::new(GARMIN_DEVICE_MANIFEST))
}
