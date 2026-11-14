use egui::Ui;
use garmin_i18n::format_message;
use garmin_model::garmin_connect::{ActivityFilters, ActivityRequest, ActivityType};

use super::super::Props;
use crate::{Size, button, date_input, icons, input, select};

#[derive(Clone, Default)]
struct Draft {
    search: String,
    kind: usize,
    status: usize,
    from: String,
    through: String,
}

impl Draft {
    fn filters(&self) -> Option<ActivityFilters> {
        let date = |value: &str| {
            if value.trim().is_empty() {
                Some(None)
            } else {
                value.trim().parse().ok().map(Some)
            }
        };
        let filters = ActivityFilters {
            search: self.search.trim().to_owned(),
            activity_type: match self.kind {
                1 => Some(ActivityType::Cycling),
                2 => Some(ActivityType::Running),
                3 => Some(ActivityType::Swimming),
                4 => Some(ActivityType::Walking),
                5 => Some(ActivityType::Hiking),
                _ => None,
            },
            from: date(&self.from)?,
            through: date(&self.through)?,
            not_imported: self.status == 1,
        };
        filters.valid().then_some(filters)
    }

    fn from_filters(filters: &ActivityFilters) -> Self {
        Self {
            search: filters.search.clone(),
            kind: match filters.activity_type {
                None => 0,
                Some(ActivityType::Cycling) => 1,
                Some(ActivityType::Running) => 2,
                Some(ActivityType::Swimming) => 3,
                Some(ActivityType::Walking) => 4,
                Some(ActivityType::Hiking) => 5,
            },
            status: usize::from(filters.not_imported),
            from: filters
                .from
                .map_or_else(String::new, |date| date.to_string()),
            through: filters
                .through
                .map_or_else(String::new, |date| date.to_string()),
        }
    }
}

pub(super) fn show(
    ui: &mut Ui,
    props: &Props<'_>,
    applied: &ActivityFilters,
    enabled: bool,
) -> Option<ActivityRequest> {
    ui.scope(|ui| {
        ui.set_max_width(ui.available_width().min(960.0));
        let reserve = if props
            .garmin_connect
            .batch
            .is_some_and(super::Batch::running)
            || (props.garmin_connect.activities_busy
                && props.garmin_connect.load_progress.is_some())
        {
            352.0
        } else {
            192.0
        };
        egui::ScrollArea::vertical()
            .id_salt(props.id.with("cloud-filter-overflow"))
            .max_height((ui.available_height() - reserve).max(96.0))
            .auto_shrink([false, true])
            .show(ui, |ui| form(ui, props, applied, enabled))
            .inner
    })
    .inner
}

fn form(
    ui: &mut Ui,
    props: &Props<'_>,
    applied: &ActivityFilters,
    enabled: bool,
) -> Option<ActivityRequest> {
    let id = props.id.with("cloud-filters");
    let mut draft = ui
        .data(|data| data.get_temp::<Draft>(id))
        .unwrap_or_else(|| Draft::from_filters(applied));
    let mut request = None;
    search(ui, props, &mut draft, enabled);
    ui.add_space(16.0);
    fields(ui, props, &mut draft, enabled);
    let parsed = draft.filters();
    let helper = format_message!(props.intl, default_message: "Dates use the activity’s local time. Filters search the whole cached account.");
    crate::typography::body(ui, &helper);
    ui.add_space(16.0);
    ui.horizontal_wrapped(|ui| {
        let label = format_message!(props.intl, default_message: "Apply filters");
        if filter_action(
            ui,
            &label,
            button::Kind::Primary,
            enabled && parsed.as_ref().is_some_and(|filters| filters != applied),
        ) {
            request = parsed.map(|filters| ActivityRequest::List { start: 0, filters });
        }
        let label = format_message!(props.intl, default_message: "Clear filters");
        if filter_action(
            ui,
            &label,
            button::Kind::Ghost,
            enabled
                && (draft.kind != 0
                    || !draft.search.is_empty()
                    || draft.status != 0
                    || !draft.from.is_empty()
                    || !draft.through.is_empty()
                    || *applied != ActivityFilters::default()),
        ) {
            draft = Draft::default();
            request = Some(ActivityRequest::List {
                start: 0,
                filters: ActivityFilters::default(),
            });
        }
    });
    if draft.search.chars().count() > 256 {
        ui.label(format_message!(props.intl, default_message: "Use at most 256 characters in the search"));
    } else if draft.filters().is_none() {
        ui.label(format_message!(props.intl, default_message: "Enter dates as YYYY-MM-DD, with the from date on or before the through date"));
    }
    ui.data_mut(|data| data.insert_temp(id, draft));
    request
}

fn filter_action(ui: &mut Ui, label: &str, kind: button::Kind, enabled: bool) -> bool {
    button::Props {
        label,
        icon: None,
        kind,
        size: Size::Medium,
        width: button::Width::Fit,
        enabled,
    }
    .show(ui)
    .clicked()
}

fn search(ui: &mut Ui, props: &Props<'_>, draft: &mut Draft, enabled: bool) {
    ui.allocate_ui(egui::vec2(ui.available_width(), 64.0), |ui| {
        ui.vertical(|ui| {
            let label = format_message!(props.intl, default_message: "Search activities");
            let response = input::show(
                ui,
                &mut draft.search,
                input::Props::new(&label).disabled(!enabled),
            );
            crate::semantics::target(ui, &response, "cloud.filters.search");
        });
    });
    let helper = format_message!(props.intl, default_message: "Fuzzy search across all cached names, descriptions, activity types, dates and IDs.");
    crate::typography::body(ui, &helper);
}

fn fields(ui: &mut Ui, props: &Props<'_>, draft: &mut Draft, enabled: bool) {
    let id = props.id.with("cloud-filters");
    let columns = if ui.available_width() >= 720.0 {
        4.0
    } else if ui.available_width() >= 320.0 {
        2.0
    } else {
        1.0
    };
    let width = (ui.available_width() - (columns - 1.0) * 8.0) / columns;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.y = 24.0;
        let types = [
            format_message!(props.intl, default_message: "All activity types"),
            format_message!(props.intl, default_message: "Cycling"),
            format_message!(props.intl, default_message: "Running"),
            format_message!(props.intl, default_message: "Swimming"),
            format_message!(props.intl, default_message: "Walking"),
            format_message!(props.intl, default_message: "Hiking"),
        ];
        let choices = types
            .iter()
            .zip([
                None,
                Some(icons::BICYCLE),
                Some(icons::PERSON_SIMPLE_RUN),
                Some(icons::PERSON_SIMPLE_SWIM),
                Some(icons::PERSON_SIMPLE_WALK),
                Some(icons::PERSON_SIMPLE_HIKE),
            ])
            .map(|(label, icon)| {
                icon.map_or_else(
                    || select::Choice::new(label),
                    |icon| select::Choice::new(label).icon(icon),
                )
            })
            .collect::<Vec<_>>();
        let label = format_message!(props.intl, default_message: "Activity type");
        ui.allocate_ui(egui::vec2(width, 64.0), |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.vertical(|ui| {
                let response = select::show(
                    ui,
                    id.with("type"),
                    &mut draft.kind,
                    &choices,
                    select::Props::new(&label).width(width).disabled(!enabled),
                );
                crate::semantics::target(ui, &response, "cloud.filters.type");
            });
        });
        let statuses = [
            format_message!(props.intl, default_message: "All activities"),
            format_message!(props.intl, default_message: "Not yet imported"),
        ];
        let choices = statuses
            .iter()
            .map(|label| select::Choice::new(label))
            .collect::<Vec<_>>();
        let label = format_message!(props.intl, default_message: "Import status");
        ui.allocate_ui(egui::vec2(width, 64.0), |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.vertical(|ui| {
                select::show(
                    ui,
                    id.with("status"),
                    &mut draft.status,
                    &choices,
                    select::Props::new(&label).width(width).disabled(!enabled),
                )
            });
        });
        for (label, value, target) in [
            (
                format_message!(props.intl, default_message: "From date"),
                &mut draft.from,
                "cloud.filters.from",
            ),
            (
                format_message!(props.intl, default_message: "Through date"),
                &mut draft.through,
                "cloud.filters.through",
            ),
        ] {
            ui.allocate_ui(egui::vec2(width, 64.0), |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.vertical(|ui| {
                    let response = date_input::show(
                        ui,
                        id.with(target),
                        value,
                        props.intl,
                        &label,
                        enabled,
                        props.garmin_connect.today,
                    );
                    crate::semantics::target(ui, &response, target);
                });
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_round_trips_and_validates_unicode_length() {
        let mut draft = Draft {
            search: "  hamnia coastl  ".into(),
            ..Draft::default()
        };
        let filters = draft.filters().expect("valid account-wide query");
        assert_eq!(filters.search, "hamnia coastl");
        assert_eq!(Draft::from_filters(&filters).filters(), Some(filters));
        draft.search = "č".repeat(256);
        assert!(draft.filters().is_some());
        draft.search.push('x');
        assert!(draft.filters().is_none());
        draft.search = "  ".into();
        assert_eq!(
            draft.filters().expect("blank query clears search").search,
            ""
        );
    }

    #[test]
    fn date_filters_reject_invalid_and_reversed_ranges() {
        let mut draft = Draft::default();
        assert_eq!(draft.filters(), Some(ActivityFilters::default()));
        draft.from = "2026-02-30".into();
        assert!(draft.filters().is_none());
        draft.from = "2026-11-10".into();
        draft.through = "2026-11-09".into();
        assert!(draft.filters().is_none());
        draft.through = draft.from.clone();
        draft.kind = 1;
        draft.status = 1;
        let filters = draft.filters().expect("inclusive same-day range");
        assert_eq!(filters.activity_type, Some(ActivityType::Cycling));
        assert!(filters.not_imported);
        assert_eq!(Draft::from_filters(&filters).filters(), Some(filters));
    }
}
