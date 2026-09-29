//! Host filesystem selection. Paths refer to the host, never the browser's filesystem.

#[garmin_macros::portable(eq)]
pub struct Directory {
    pub path: String,
    pub entries: Vec<crate::DeviceCatalogEntry>,
}

#[garmin_macros::portable(copy, eq)]
pub enum Operation {
    Open,
    Save,
}

#[garmin_macros::portable(eq)]
pub struct Selection {
    pub operation: Operation,
    pub path: String,
    pub replace: bool,
}
