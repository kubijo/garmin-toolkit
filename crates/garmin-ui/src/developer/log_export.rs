//! An incomplete export must never be offered as a successful download.
use garmin_service_api::logging::Batch;

#[derive(Default)]
pub(super) struct Export {
    output: String,
}

impl Export {
    pub fn push(&mut self, batch: &Batch) -> Result<(), String> {
        if batch.gap {
            return Err(
                "Log export interrupted: records expired during export. Retry the export.".into(),
            );
        }
        if let Some(error) = &batch.error {
            return Err(format!("Log export failed: {error}"));
        }
        for record in &batch.records {
            let line = serde_json::to_string(record).map_err(|error| error.to_string())?;
            if self.output.len() + line.len() + 1 > 64 * 1024 * 1024 {
                return Err("log export exceeds 64 MiB".into());
            }
            self.output.push_str(&line);
            self.output.push('\n');
        }
        Ok(())
    }

    pub fn finish(self) -> String {
        self.output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch() -> Batch {
        Batch {
            records: vec![garmin_model::logging::Record {
                sequence: 1,
                timestamp_ms: 42,
                level: garmin_model::logging::Level::Info,
                component: "test".into(),
                source: "test".into(),
                session: "test".into(),
                source_sequence: 1,
                message: "diagnostic".into(),
                fields: std::collections::BTreeMap::new(),
            }],
            ..Batch::default()
        }
    }

    #[test]
    fn complete_export_contains_the_records_as_json_lines() {
        let mut export = Export::default();
        let batch = batch();
        export.push(&batch).unwrap();
        let output = export.finish();
        assert!(output.ends_with('\n'));
        let decoded: garmin_model::logging::Record = serde_json::from_str(output.trim()).unwrap();
        assert_eq!(decoded, batch.records[0]);
    }

    #[test]
    fn retention_gaps_and_storage_errors_fail_the_export() {
        let mut export = Export::default();
        export.push(&batch()).unwrap();
        assert!(
            export
                .push(&Batch {
                    gap: true,
                    ..Batch::default()
                })
                .is_err()
        );
        let error = Export::default()
            .push(&Batch {
                error: Some("disk write failed".into()),
                ..Batch::default()
            })
            .unwrap_err();
        assert!(error.contains("disk write failed"));
    }
}
