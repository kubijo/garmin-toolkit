//! Supported CSV semantics, with issue codes shared with Frictionless reports.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::{Field, FieldNames, Package, Resource, TableSchema, V2, hash};

/// Stable validation categories for metadata, rows, keys, and resource integrity.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IssueCode {
    PackageError,
    ResourceError,
    BlankRow,
    BlankLabel,
    DuplicateLabel,
    MissingCell,
    ExtraCell,
    TypeError,
    ConstraintError,
    PrimaryKey,
    ForeignKey,
    HashCount,
    ByteCount,
    SchemaError,
    Unsupported,
}

/// An invalid or unsupported part of a resource, without embedding private cell values.
#[derive(Debug, Serialize)]
pub struct Issue {
    pub resource: String,
    pub code: IssueCode,
    pub row: Option<u64>,
}

/// Full validation result, including multiple independent failures.
#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub issues: Vec<Issue>,
}

impl Report {
    #[must_use]
    pub fn valid(&self) -> bool {
        self.issues.is_empty()
    }

    fn issue(&mut self, resource: &str, code: IssueCode, row: Option<u64>) {
        self.issues.push(Issue {
            resource: resource.to_owned(),
            code,
            row,
        });
    }
}

struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
}

pub(crate) fn validate(package: &Package) -> Report {
    let mut report = Report::default();
    // The package imposes its version on every resource, including resources
    // declaring their own profile. An absent package profile means v1.
    if package.descriptor.schema.as_deref() != Some(V2) {
        if package
            .descriptor
            .name
            .as_deref()
            .is_some_and(|name| !v1_name(name))
        {
            report.issue("", IssueCode::PackageError, None);
        }
        for resource in &package.resources {
            if !v1_name(&resource.descriptor.name) {
                report.issue(&resource.descriptor.name, IssueCode::ResourceError, None);
            }
        }
    }
    if !report.valid() {
        return report;
    }
    let mut tables = BTreeMap::new();
    for resource in &package.resources {
        let metadata = &resource.descriptor;
        if metadata
            .bytes
            .is_some_and(|expected| expected != resource.bytes.len() as u64)
        {
            report.issue(&metadata.name, IssueCode::ByteCount, None);
        }
        if let Some(expected) = &metadata.hash {
            if !expected.starts_with("sha256:") {
                report.issue(&metadata.name, IssueCode::Unsupported, None);
            } else if expected != &hash(&resource.bytes) {
                report.issue(&metadata.name, IssueCode::HashCount, None);
            }
        }
        let is_csv = metadata.kind.as_deref() == Some("table")
            || metadata
                .format
                .as_deref()
                .is_some_and(|format| format.eq_ignore_ascii_case("csv"))
            || metadata.data.is_some()
            || metadata
                .path
                .as_deref()
                .is_some_and(|path| path.to_ascii_lowercase().ends_with(".csv"));
        if is_csv {
            if metadata.metadata.contains_key("dialect")
                || metadata.kind.as_deref().is_some_and(|kind| kind != "table")
            {
                report.issue(&metadata.name, IssueCode::Unsupported, None);
                continue;
            }
            if let Some(table) = read_table(resource, &mut report) {
                tables.insert(metadata.name.as_str(), table);
            }
        }
    }
    for resource in &package.resources {
        if let Some(schema) = &resource.descriptor.schema {
            validate_foreign_keys(resource, schema, &tables, &mut report);
        }
    }
    report
}

fn v1_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'.' | b'_' | b'/')
        })
}

fn headers(
    name: &str,
    schema: Option<&TableSchema>,
    reader: &mut csv::Reader<&[u8]>,
    report: &mut Report,
) -> Option<Vec<String>> {
    let mut record = csv::StringRecord::new();
    match reader.read_record(&mut record) {
        Ok(true) => {}
        Ok(false) => {
            report.issue(name, IssueCode::BlankLabel, None);
            return None;
        }
        Err(_) => {
            report.issue(name, IssueCode::TypeError, None);
            return None;
        }
    }
    let headers: Vec<String> = record.iter().map(str::to_owned).collect();
    let mut names = BTreeSet::new();
    for header in &headers {
        if header.is_empty() {
            report.issue(name, IssueCode::BlankLabel, None);
        }
        if !names.insert(header) {
            report.issue(name, IssueCode::DuplicateLabel, None);
        }
    }
    if let Some(schema) = schema {
        if !headers
            .iter()
            .eq(schema.fields.iter().map(|field| &field.name))
        {
            report.issue(name, IssueCode::SchemaError, None);
            return None;
        }
        for field in &schema.fields {
            if !matches!(
                field.kind.as_str(),
                "string" | "integer" | "number" | "boolean" | "any"
            ) || !field.constraints.metadata.is_empty()
                || field
                    .metadata
                    .get("format")
                    .is_some_and(|format| format != "default")
            {
                report.issue(name, IssueCode::Unsupported, None);
            }
        }
    }
    Some(headers)
}

fn read_table(resource: &Resource, report: &mut Report) -> Option<Table> {
    let name = &resource.descriptor.name;
    let schema = resource.descriptor.schema.as_ref();
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(resource.bytes.as_slice());
    let headers = headers(name, schema, &mut reader, report)?;
    let mut record = csv::StringRecord::new();
    let mut table = Table {
        headers,
        rows: Vec::new(),
    };
    let mut row_number = 1_u64;
    loop {
        let before = usize::try_from(reader.position().byte()).ok()?;
        let result = reader.read_record(&mut record);
        let after = usize::try_from(reader.position().byte()).ok()?;
        // csv intentionally skips empty physical records. Recover those using its record
        // boundaries; quoted multiline fields are never scanned as separate records.
        let prefix = &resource.bytes[before..after];
        for line in prefix.split_inclusive(|byte| *byte == b'\n') {
            if line != b"\n" && line != b"\r\n" {
                break;
            }
            row_number += 1;
            report.issue(name, IssueCode::BlankRow, Some(row_number));
            if schema.is_some_and(|schema| !schema.primary_key.is_empty()) {
                report.issue(name, IssueCode::PrimaryKey, Some(row_number));
            }
        }
        match result {
            Ok(false) => break,
            Err(_) => {
                report.issue(name, IssueCode::TypeError, Some(row_number + 1));
                return None;
            }
            Ok(true) => {}
        }
        row_number += 1;
        if record.iter().all(str::is_empty) {
            report.issue(name, IssueCode::BlankRow, Some(row_number));
        }
        if record.len() != table.headers.len() {
            report.issue(
                name,
                if record.len() < table.headers.len() {
                    IssueCode::MissingCell
                } else {
                    IssueCode::ExtraCell
                },
                Some(row_number),
            );
        }
        let mut values = Vec::new();
        for (index, raw) in record.iter().enumerate() {
            let field = schema.and_then(|schema| schema.fields.get(index));
            let missing = schema.map_or(raw.is_empty(), |schema| {
                schema.missing_values.iter().any(|value| value == raw)
            });
            if missing && field.is_some_and(|field| field.constraints.required) {
                report.issue(name, IssueCode::ConstraintError, Some(row_number));
            }
            values.push(if missing {
                None
            } else if let Some(value) =
                field.map_or_else(|| Some(raw.to_owned()), |field| cell(raw, field))
            {
                Some(value)
            } else {
                report.issue(name, IssueCode::TypeError, Some(row_number));
                None
            });
        }
        table.rows.push(values);
    }
    if let Some(schema) = schema {
        validate_primary_key(name, schema, &table, report);
    }
    Some(table)
}

fn cell(raw: &str, field: &Field) -> Option<String> {
    match field.kind.as_str() {
        "string" | "any" => Some(raw.to_owned()),
        "integer" => raw.parse::<i128>().ok().map(|value| value.to_string()),
        "number" => raw
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(|value| value.to_string()),
        "boolean" => match raw {
            "true" | "True" | "TRUE" | "1" => Some("true".to_owned()),
            "false" | "False" | "FALSE" | "0" => Some("false".to_owned()),
            _ => None,
        },
        _ => None,
    }
}

fn columns(names: &FieldNames, table: &Table) -> Option<Vec<usize>> {
    names
        .as_slice()
        .iter()
        .map(|name| table.headers.iter().position(|header| header == name))
        .collect()
}

fn key(row: &[Option<String>], columns: &[usize]) -> Option<Vec<String>> {
    columns
        .iter()
        .map(|index| row.get(*index)?.clone())
        .collect()
}

fn validate_primary_key(name: &str, schema: &TableSchema, table: &Table, report: &mut Report) {
    if schema.primary_key.is_empty() {
        return;
    }
    let Some(columns) = columns(&schema.primary_key, table) else {
        report.issue(name, IssueCode::SchemaError, None);
        return;
    };
    let mut keys = BTreeSet::new();
    for (index, row) in table.rows.iter().enumerate() {
        if !key(row, &columns).is_some_and(|key| keys.insert(key)) {
            report.issue(name, IssueCode::PrimaryKey, Some(index as u64 + 2));
        }
    }
}

fn validate_foreign_keys(
    resource: &Resource,
    schema: &TableSchema,
    tables: &BTreeMap<&str, Table>,
    report: &mut Report,
) {
    let name = &resource.descriptor.name;
    let Some(table) = tables.get(name.as_str()) else {
        return;
    };
    for foreign_key in &schema.foreign_keys {
        let reference = &foreign_key.reference;
        let target_name = if reference.resource.is_empty() {
            name
        } else {
            &reference.resource
        };
        let Some(target) = tables.get(target_name.as_str()) else {
            report.issue(name, IssueCode::SchemaError, None);
            continue;
        };
        let (Some(local), Some(remote)) = (
            columns(&foreign_key.fields, table),
            columns(&reference.fields, target),
        ) else {
            report.issue(name, IssueCode::SchemaError, None);
            continue;
        };
        if local.is_empty() || local.len() != remote.len() {
            report.issue(name, IssueCode::SchemaError, None);
            continue;
        }
        let keys: BTreeSet<_> = target
            .rows
            .iter()
            .filter_map(|row| key(row, &remote))
            .collect();
        for (index, row) in table.rows.iter().enumerate() {
            if key(row, &local).is_some_and(|value| !keys.contains(&value)) {
                report.issue(name, IssueCode::ForeignKey, Some(index as u64 + 2));
            }
        }
    }
}
