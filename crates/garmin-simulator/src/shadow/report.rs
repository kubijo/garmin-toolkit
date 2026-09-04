use byte_unit::{Byte, UnitType};
use comrak::{
    Arena, Options, format_commonmark,
    nodes::{AstNode, NodeHeading, NodeTable, NodeValue, TableAlignment},
};
use garmin_device::SafeRelativePath;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FileState {
    Absent,
    File { bytes: u64, sha256: String },
    Unobserved { reason: String },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlannedChange {
    Write,
    Remove,
    Context,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Replaced,
    Removed,
    Unchanged,
    Unobserved,
}

impl ChangeKind {
    pub(super) fn between(before: &FileState, after: &FileState) -> Self {
        match (before, after) {
            (FileState::Unobserved { .. }, _) | (_, FileState::Unobserved { .. }) => {
                Self::Unobserved
            }
            (FileState::Absent, FileState::File { .. }) => Self::Added,
            (FileState::File { .. }, FileState::Absent) => Self::Removed,
            (before, after) if before == after => Self::Unchanged,
            _ => Self::Replaced,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Added => "Added",
            Self::Replaced => "Replaced",
            Self::Removed => "Removed",
            Self::Unchanged => "Unchanged",
            Self::Unobserved => "Unobserved",
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SimulationStatus {
    Complete,
    Incomplete,
}

#[derive(Debug, Serialize)]
pub struct ObservedChange {
    pub storage: String,
    pub path: SafeRelativePath,
    pub planned: PlannedChange,
    pub observed: ChangeKind,
    pub before: FileState,
    pub after: FileState,
}

#[derive(Debug, Serialize)]
pub struct SimulationReport {
    pub version: u8,
    pub execution_target: String,
    pub source_device: String,
    pub plan_digest: String,
    pub status: SimulationStatus,
    pub physical_device_modified: bool,
    pub changes: Vec<ObservedChange>,
}

impl SimulationReport {
    /// Render typed report data as a Markdown document.
    /// # Errors
    /// AST formatting failure.
    pub fn markdown(&self) -> Result<String, std::fmt::Error> {
        let arena = Arena::new();
        let document = arena.alloc(NodeValue::Document.into());
        let title = match self.status {
            SimulationStatus::Complete => "Simulation complete",
            SimulationStatus::Incomplete => "Simulation incomplete",
        };
        text_block(
            &arena,
            document,
            NodeValue::Heading(NodeHeading {
                level: 1,
                ..NodeHeading::default()
            }),
            title,
        );
        text_block(
            &arena,
            document,
            NodeValue::Paragraph,
            "Physical device unchanged. Compare device/ with before/. This snapshot contains affected files only.",
        );
        text_block(
            &arena,
            document,
            NodeValue::Paragraph,
            "Scope and storage mapping: snapshot.json. Exact byte counts and hashes: changes.json.",
        );
        let table = arena.alloc(
            NodeValue::Table(Box::new(NodeTable {
                alignments: vec![TableAlignment::None; 4],
                num_columns: 4,
                num_rows: self.changes.len() + 1,
                num_nonempty_cells: (self.changes.len() + 1) * 4,
            }))
            .into(),
        );
        document.append(table);
        table_row(&arena, table, true, ["Observed", "File", "Before", "After"]);
        for change in &self.changes {
            table_row(
                &arena,
                table,
                false,
                [
                    change.observed.label().to_owned(),
                    format!("{}/{}", change.storage, change.path),
                    describe(&change.before),
                    describe(&change.after),
                ],
            );
        }
        let mut options = Options::default();
        options.extension.table = true;
        options.render.width = 100;
        let mut output = String::new();
        format_commonmark(document, &options, &mut output)?;
        Ok(output)
    }
}

fn text_block<'a>(
    arena: &'a Arena<'a>,
    parent: &'a AstNode<'a>,
    value: NodeValue,
    text: impl Into<String>,
) {
    let node = arena.alloc(value.into());
    node.append(arena.alloc(NodeValue::Text(text.into().into()).into()));
    parent.append(node);
}

fn table_row<'a>(
    arena: &'a Arena<'a>,
    table: &'a AstNode<'a>,
    header: bool,
    cells: [impl Into<String>; 4],
) {
    let row = arena.alloc(NodeValue::TableRow(header).into());
    for cell in cells {
        text_block(arena, row, NodeValue::TableCell, cell);
    }
    table.append(row);
}

fn describe(state: &FileState) -> String {
    match state {
        FileState::Absent => "absent".to_owned(),
        FileState::File { bytes, .. } => format!(
            "{:.2}",
            Byte::from_u64(*bytes).get_appropriate_unit(UnitType::Decimal)
        ),
        FileState::Unobserved { .. } => "unobserved".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_names_remain_literal_table_cells() {
        let path = "Garmin/[map](link)&<b>|*data*.img";
        let report = SimulationReport {
            version: 1,
            execution_target: "virtual:test".to_owned(),
            source_device: "test".to_owned(),
            plan_digest: "plan".to_owned(),
            status: SimulationStatus::Complete,
            physical_device_modified: false,
            changes: vec![ObservedChange {
                storage: "storage-001".to_owned(),
                path: SafeRelativePath::parse(path).unwrap(),
                planned: PlannedChange::Write,
                observed: ChangeKind::Added,
                before: FileState::Absent,
                after: FileState::File {
                    bytes: 42,
                    sha256: "hash".to_owned(),
                },
            }],
        };
        let markdown = report.markdown().unwrap();
        let arena = Arena::new();
        let mut options = Options::default();
        options.extension.table = true;
        let document = comrak::parse_document(&arena, &markdown, &options);
        let row = document
            .descendants()
            .find(|node| matches!(node.data.borrow().value, NodeValue::TableRow(false)))
            .unwrap();
        let cells = row.children().collect::<Vec<_>>();
        assert_eq!(cells.len(), 4);
        let text = cells[1]
            .descendants()
            .filter_map(|node| match &node.data.borrow().value {
                NodeValue::Text(text) => Some(text.to_string()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(text, format!("storage-001/{path}"));
        assert!(!row.descendants().any(|node| matches!(
            node.data.borrow().value,
            NodeValue::Link(_) | NodeValue::HtmlInline(_) | NodeValue::Strong | NodeValue::Emph
        )));
    }
}
