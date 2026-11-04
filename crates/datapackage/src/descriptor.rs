//! Standard metadata, independent of any application schema.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Canonical Data Package v2 profile.
pub const V2: &str = "https://datapackage.org/profiles/2.0/datapackage.json";
/// Canonical Data Package v1 profile.
pub const V1: &str = "https://datapackage.org/profiles/1.0/datapackage.json";

/// Additional standard or application metadata, preserved during round trips.
pub type Metadata = BTreeMap<String, Value>;

/// Data Package descriptor.
/// An absent `$schema` denotes v1.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Descriptor {
    #[serde(rename = "$schema", skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub resources: Vec<ResourceDescriptor>,
    #[serde(flatten)]
    pub metadata: Metadata,
}

/// A resource's metadata.
/// Local paths and inline row arrays are supported.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ResourceDescriptor {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mediatype: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<TableSchema>,
    #[serde(flatten)]
    pub metadata: Metadata,
}

/// Table Schema for CSV and inline tables.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TableSchema {
    pub fields: Vec<Field>,
    #[serde(
        rename = "primaryKey",
        default,
        skip_serializing_if = "FieldNames::is_empty"
    )]
    pub primary_key: FieldNames,
    #[serde(rename = "foreignKeys", default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys: Vec<ForeignKey>,
    #[serde(rename = "missingValues", default = "missing_values")]
    pub missing_values: Vec<String>,
    #[serde(flatten)]
    pub metadata: Metadata,
}

/// A field name or a composite key's ordered field names.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum FieldNames {
    One(String),
    Many(Vec<String>),
}

impl Default for FieldNames {
    fn default() -> Self {
        Self::Many(Vec::new())
    }
}

impl FieldNames {
    #[must_use]
    pub fn as_slice(&self) -> &[String] {
        match self {
            Self::One(value) => std::slice::from_ref(value),
            Self::Many(values) => values,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }
}

/// One table column.
/// Unknown metadata is retained rather than discarded.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Field {
    pub name: String,
    #[serde(rename = "type", default = "string_type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default)]
    pub constraints: Constraints,
    #[serde(flatten)]
    pub metadata: Metadata,
}

/// Supported field constraints.
/// Other constraints are retained and reported unsupported.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Constraints {
    #[serde(default)]
    pub required: bool,
    #[serde(flatten)]
    pub metadata: Metadata,
}

/// Ordered local fields referring to a table's ordered key fields.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ForeignKey {
    pub fields: FieldNames,
    pub reference: Reference,
}

/// An empty resource name denotes the same resource.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Reference {
    #[serde(default)]
    pub resource: String,
    pub fields: FieldNames,
}

fn string_type() -> String {
    "string".to_owned()
}
fn missing_values() -> Vec<String> {
    vec![String::new()]
}
