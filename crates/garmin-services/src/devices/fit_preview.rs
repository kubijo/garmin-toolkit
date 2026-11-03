//! Read-only FIT inspection shared by native and Home Assistant file browsers.

use garmin_service_api::{
    DeviceFitPreview, DeviceFitPreviewActivity, DeviceFitPreviewContent, DeviceFitPreviewCourse,
};

/// Inspect the FIT file type and construct a preview without importing or persisting it.
/// # Errors
/// Rejects malformed data and file types without a preview renderer.
pub fn inspect(file_name: String, bytes: &[u8]) -> Result<DeviceFitPreview, String> {
    let content = match garmin_fit::normalize_activities(bytes) {
        Ok(import) => {
            let activities = import
                .normalized_activities()
                .map(|activity| DeviceFitPreviewActivity {
                    source: activity
                        .creator()
                        .product_name()
                        .map_or_else(|| "FIT".to_owned(), ToString::to_string),
                    summary: activity.activity().summary(),
                    recording: garmin_service_api::ActivityRecordingSnapshot::from(
                        activity.activity(),
                    ),
                })
                .collect::<Vec<_>>();
            if activities.is_empty() {
                return Err("the FIT file does not contain an activity".to_owned());
            }
            DeviceFitPreviewContent::Activities(activities)
        }
        Err(garmin_fit::ActivityError::UnsupportedFileType(garmin_fit::FileType::COURSE)) => {
            let course = garmin_fit::course::preview(bytes).map_err(|error| error.to_string())?;
            DeviceFitPreviewContent::Course(DeviceFitPreviewCourse {
                name: course.name,
                sport: course.sport,
                points: course.points,
                cue_count: course.cue_count,
            })
        }
        Err(error) => return Err(error.to_string()),
    };
    Ok(DeviceFitPreview { file_name, content })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_course_previews_geometry_without_becoming_an_activity()
    -> Result<(), Box<dyn std::error::Error>> {
        let bytes = garmin_fit::fixture::course()?;
        let preview = inspect("course.fit".to_owned(), &bytes)?;
        assert!(!preview.supports_activity_import());
        assert_eq!(preview.activities().len(), 0);
        let DeviceFitPreviewContent::Course(course) = preview.content else {
            panic!("expected course preview");
        };
        assert_eq!(course.name.as_deref(), Some("Synthetic canal loop"));
        assert_eq!(course.points.len(), 3);
        assert_eq!(course.sport, Some(garmin_model::route::RouteSport::Cycling));
        Ok(())
    }

    #[test]
    fn activity_preview_remains_available_for_explicit_import()
    -> Result<(), Box<dyn std::error::Error>> {
        let bytes = garmin_fit::fixture::activity(garmin_fit::fixture::Sport::Cycling)?;
        let preview = inspect("activity.fit".to_owned(), &bytes)?;
        assert!(preview.supports_activity_import());
        assert_eq!(preview.activities().len(), 1);
        Ok(())
    }

    #[test]
    fn corrupt_fit_is_not_presented_as_a_course() {
        assert!(inspect("broken.fit".to_owned(), b"invalid synthetic FIT").is_err());
    }
}
