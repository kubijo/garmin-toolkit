//! Whole-account matching over the locally cached metadata index.

use garmin_model::garmin_connect::{ActivityFilters, CachedActivity};

pub(super) fn matches(filters: &ActivityFilters, summary: &CachedActivity) -> bool {
    let activity = &summary.activity;
    if filters.not_imported && activity.imported {
        return false;
    }
    if let Some(kind) = &filters.activity_type
        && activity.sport
            != match kind {
                garmin_model::garmin_connect::ActivityType::Cycling => {
                    Some(garmin_model::activity::ActivitySport::Cycling)
                }
                garmin_model::garmin_connect::ActivityType::Running => {
                    Some(garmin_model::activity::ActivitySport::Running)
                }
                garmin_model::garmin_connect::ActivityType::Swimming => {
                    Some(garmin_model::activity::ActivitySport::Swimming)
                }
                garmin_model::garmin_connect::ActivityType::Walking => {
                    Some(garmin_model::activity::ActivitySport::Walking)
                }
                garmin_model::garmin_connect::ActivityType::Hiking => {
                    Some(garmin_model::activity::ActivitySport::Hiking)
                }
            }
    {
        return false;
    }
    let date = summary
        .started_local
        .as_deref()
        .and_then(|text| text.replace(' ', "T").parse::<jiff::civil::DateTime>().ok())
        .map(jiff::civil::DateTime::date);
    if filters
        .from
        .is_some_and(|from| date.is_none_or(|date| date < from))
        || filters
            .through
            .is_some_and(|through| date.is_none_or(|date| date > through))
    {
        return false;
    }
    let id = activity.id.to_string();
    let fields = [
        &activity.name,
        summary.description.as_deref().unwrap_or_default(),
        summary.activity_type.as_deref().unwrap_or_default(),
        summary.started_local.as_deref().unwrap_or_default(),
        activity.started_at.as_deref().unwrap_or_default(),
        &id,
    ];
    text_matches(&filters.search, &fields)
}

fn text_matches(query: &str, fields: &[&str]) -> bool {
    let query = query.trim().to_lowercase();
    let fields = fields
        .iter()
        .map(|field| field.to_lowercase())
        .collect::<Vec<_>>();
    let terms = words(&query).collect::<Vec<_>>();
    if terms.is_empty() {
        return fields.iter().any(|field| field.contains(&query));
    }
    let candidates = fields
        .iter()
        .flat_map(|field| words(field))
        .collect::<Vec<_>>();
    terms.iter().all(|term| {
        candidates
            .iter()
            .any(|candidate| word_matches(term, candidate))
    })
}

fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
}

fn word_matches(term: &str, candidate: &str) -> bool {
    if candidate.contains(term) {
        return true;
    }
    let length = term.chars().count();
    // Short terms and numbers need an exact substring; fuzzy IDs and dates are surprising.
    if length < 4 || !term.chars().all(char::is_alphabetic) {
        return false;
    }
    let edits = if length >= 8 { 2 } else { 1 };
    length.abs_diff(candidate.chars().count()) <= edits
        && strsim::osa_distance(term, candidate) <= edits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_words_across_fields_with_case_and_typo_tolerance() {
        let fields = [
            "Hamina Road Cycling",
            "Beautiful coastal ride",
            "road_cycling",
            "2026-11-10",
            "24963724629",
        ];
        assert!(text_matches("hamnia COASTL", &fields));
        assert!(text_matches("cycling 2026 24963724629", &fields));
        assert!(text_matches("road coast", &fields));
        assert!(!text_matches("cycling mountain", &fields));
        assert!(!text_matches("24963724628", &fields));
        assert!(!text_matches("2025", &fields));
        assert!(text_matches("ČESKÝ", &["Český výlet"]));
        assert!(text_matches("!!!", &["Moist!!!"]));
        assert!(!text_matches("!!!", &["Morning ride"]));
    }
}
