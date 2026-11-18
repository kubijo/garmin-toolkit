//! Account-summary analytics with optional imported FIT measurements.

use std::collections::{BTreeMap, BTreeSet};

use jiff::civil::Date;

use super::CachedActivity;
use crate::activity::{ActivityDuration, ActivitySport, Distance, HeartRate};

#[garmin_macros::portable(copy, eq, ord)]
pub enum Sport {
    Running,
    Cycling,
    Swimming,
    Walking,
    Hiking,
    Other,
}

impl Sport {
    pub const ALL: [Self; 6] = [
        Self::Running,
        Self::Cycling,
        Self::Swimming,
        Self::Walking,
        Self::Hiking,
        Self::Other,
    ];
}

#[garmin_macros::portable(default)]
pub struct Totals {
    pub activities: usize,
    pub distance: Distance,
    pub duration: ActivityDuration,
    pub elevation_gain: Distance,
    pub elevation_activities: usize,
    pub missing_distance: usize,
    pub missing_duration: usize,
    #[serde(default)]
    pub heart_rate: HeartRateAverage,
}

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct ImportedMeasurements {
    pub ascent: Option<Distance>,
    pub average_heart_rate: Option<HeartRate>,
}

/// Each activity with a reported average contributes once.
#[garmin_macros::portable(default)]
pub struct HeartRateAverage {
    sum_bpm: u128,
    activities: usize,
}

impl HeartRateAverage {
    fn record(&mut self, rate: HeartRate) {
        self.sum_bpm += u128::from(rate.as_beats_per_minute());
        self.activities += 1;
    }

    #[must_use]
    pub const fn activities(&self) -> usize {
        self.activities
    }

    #[must_use]
    pub fn value(&self) -> Option<HeartRate> {
        let count = self.activities as u128;
        let bpm = (self.sum_bpm + count / 2).checked_div(count)?;
        u16::try_from(bpm)
            .ok()
            .map(HeartRate::from_beats_per_minute)
    }
}

impl Totals {
    pub fn add(&mut self, other: &Self) {
        self.activities += other.activities;
        self.distance = self.distance.saturating_add(other.distance);
        self.duration = self.duration.saturating_add(other.duration);
        self.elevation_gain = self.elevation_gain.saturating_add(other.elevation_gain);
        self.elevation_activities += other.elevation_activities;
        self.missing_distance += other.missing_distance;
        self.missing_duration += other.missing_duration;
        self.heart_rate.sum_bpm += other.heart_rate.sum_bpm;
        self.heart_rate.activities += other.heart_rate.activities;
    }
}

#[garmin_macros::portable]
pub struct Day {
    pub date: Date,
    pub sport: Sport,
    pub totals: Totals,
}

/// Measurements available for a dated activity in a supported sport.
#[garmin_macros::portable]
pub struct ActivitySummary {
    pub id: i64,
    pub date: Date,
    pub sport: Sport,
    pub name: String,
    pub distance: Option<Distance>,
    pub duration: Option<ActivityDuration>,
    pub elevation_gain: Option<Distance>,
    #[serde(default)]
    pub average_heart_rate: Option<HeartRate>,
}

/// Only published generations are exposed. No incomplete refresh is interpreted as zero activity.
#[garmin_macros::portable(default)]
pub struct History {
    pub days: Vec<Day>,
    pub activities: Vec<ActivitySummary>,
    pub undated: usize,
    pub summaries: usize,
}

impl History {
    #[must_use]
    pub fn series(
        &self,
        range: Range,
        sport: Option<Sport>,
        metric: Metric,
        monthly: bool,
    ) -> Vec<Bucket> {
        let mut groups = BTreeMap::<Date, Range>::new();
        for date in range.dates() {
            let key = if monthly {
                Date::new(date.year(), date.month(), 1).unwrap_or(date)
            } else {
                date
            };
            groups
                .entry(key)
                .and_modify(|bucket| bucket.through = date)
                .or_insert(Range {
                    from: date,
                    through: date,
                });
        }
        groups
            .into_iter()
            .map(|(date, bucket)| {
                let totals = self.totals(bucket, sport);
                let value = match metric {
                    Metric::Distance if totals.missing_distance == 0 => {
                        Some(BucketValue::Distance(totals.distance))
                    }
                    Metric::Duration if totals.missing_duration == 0 => {
                        Some(BucketValue::Duration(totals.duration))
                    }
                    Metric::Count => Some(BucketValue::Count(totals.activities)),
                    Metric::ActiveDays => {
                        Some(BucketValue::Count(self.active_days_for(bucket, sport)))
                    }
                    Metric::Elevation if totals.elevation_activities == totals.activities => {
                        Some(BucketValue::Distance(totals.elevation_gain))
                    }
                    Metric::AverageHeartRate => {
                        totals.heart_rate.value().map(BucketValue::HeartRate)
                    }
                    _ => None,
                };
                Bucket {
                    date,
                    value,
                    has_activities: totals.activities > 0,
                }
            })
            .collect()
    }

    #[must_use]
    pub fn from_summaries(summaries: &[CachedActivity]) -> Self {
        Self::from_summaries_with_measurements(summaries, &BTreeMap::new())
    }

    #[must_use]
    pub fn from_summaries_with_measurements(
        summaries: &[CachedActivity],
        imported: &BTreeMap<i64, ImportedMeasurements>,
    ) -> Self {
        let mut grouped = BTreeMap::<(Date, Sport), Totals>::new();
        let mut seen = BTreeSet::new();
        let mut undated = 0;
        let mut activities = Vec::new();
        for summary in summaries {
            if !seen.insert(summary.activity.id) {
                continue;
            }
            let Some(date) = summary
                .started_local
                .as_deref()
                .and_then(|text| text.get(..10))
                .and_then(|text| text.parse::<Date>().ok())
            else {
                undated += 1;
                continue;
            };
            let sport = match summary.activity.sport {
                Some(ActivitySport::Running) => Sport::Running,
                Some(ActivitySport::Cycling) => Sport::Cycling,
                Some(ActivitySport::Swimming) => Sport::Swimming,
                Some(ActivitySport::Walking) => Sport::Walking,
                Some(ActivitySport::Hiking) => Sport::Hiking,
                _ => Sport::Other,
            };
            let totals = grouped.entry((date, sport)).or_default();
            totals.activities += 1;
            let distance = summary.activity.distance;
            let duration = summary.activity.duration;
            let measurements = imported
                .get(&summary.activity.id)
                .copied()
                .unwrap_or_default();
            let elevation_gain = measurements.ascent;
            if let Some(gain) = elevation_gain {
                totals.elevation_gain = totals.elevation_gain.saturating_add(gain);
                totals.elevation_activities += 1;
            }
            if let Some(rate) = measurements.average_heart_rate {
                totals.heart_rate.record(rate);
            }
            if let Some(value) = distance {
                totals.distance = totals.distance.saturating_add(value);
            } else {
                totals.missing_distance += 1;
            }
            if let Some(value) = duration {
                totals.duration = totals.duration.saturating_add(value);
            } else {
                totals.missing_duration += 1;
            }
            if sport != Sport::Other {
                activities.push(ActivitySummary {
                    id: summary.activity.id,
                    date,
                    sport,
                    name: summary.activity.name.clone(),
                    distance,
                    duration,
                    elevation_gain,
                    average_heart_rate: measurements.average_heart_rate,
                });
            }
        }
        activities.sort_unstable_by_key(|activity| std::cmp::Reverse((activity.date, activity.id)));
        Self {
            days: grouped
                .into_iter()
                .map(|((date, sport), totals)| Day {
                    date,
                    sport,
                    totals,
                })
                .collect(),
            activities,
            undated,
            summaries: seen.len(),
        }
    }

    #[must_use]
    pub fn totals(&self, range: Range, sport: Option<Sport>) -> Totals {
        let mut result = Totals::default();
        for day in &self.days {
            if range.contains(day.date) && sport.is_none_or(|sport| day.sport == sport) {
                result.add(&day.totals);
            }
        }
        result
    }

    #[must_use]
    pub fn active_days(&self, range: Range) -> usize {
        self.active_days_for(range, None)
    }

    fn active_days_for(&self, range: Range, sport: Option<Sport>) -> usize {
        self.days
            .iter()
            .filter(|day| range.contains(day.date) && sport.is_none_or(|sport| day.sport == sport))
            .map(|day| day.date)
            .collect::<BTreeSet<_>>()
            .len()
    }
}

#[garmin_macros::portable(copy, default, eq)]
pub enum Metric {
    #[default]
    Distance,
    Duration,
    Count,
    ActiveDays,
    Elevation,
    AverageHeartRate,
}

#[garmin_macros::portable]
pub struct Bucket {
    pub date: Date,
    pub value: Option<BucketValue>,
    #[serde(default)]
    pub has_activities: bool,
}

#[garmin_macros::portable(copy, eq)]
pub enum BucketValue {
    Distance(Distance),
    Duration(ActivityDuration),
    Count(usize),
    HeartRate(HeartRate),
}

impl BucketValue {
    fn add(self, other: Self) -> Option<Self> {
        match (self, other) {
            (Self::Distance(total), Self::Distance(value)) => {
                Some(Self::Distance(total.saturating_add(value)))
            }
            (Self::Duration(total), Self::Duration(value)) => {
                Some(Self::Duration(total.saturating_add(value)))
            }
            (Self::Count(total), Self::Count(value)) => {
                Some(Self::Count(total.saturating_add(value)))
            }
            _ => None,
        }
    }
}

/// Unknown measurements make subsequent cumulative totals unknown too.
pub fn cumulative(values: &mut [Bucket]) {
    let Some((first, remaining)) = values.split_first_mut() else {
        return;
    };
    let mut total = first.value;
    for bucket in remaining {
        total = total
            .zip(bucket.value)
            .and_then(|(total, value)| total.add(value));
        bucket.value = total;
    }
}

#[garmin_macros::portable(copy, default, eq)]
pub enum Period {
    Day,
    Week,
    #[default]
    Month,
    Year,
}

#[garmin_macros::portable(copy, eq)]
pub struct Range {
    pub from: Date,
    pub through: Date,
}

impl Range {
    #[must_use]
    pub fn contains(self, date: Date) -> bool {
        date >= self.from && date <= self.through
    }

    #[must_use]
    pub fn dates(self) -> Vec<Date> {
        let mut result = Vec::new();
        let mut date = self.from;
        while date <= self.through {
            result.push(date);
            let Ok(next) = date.checked_add(jiff::Span::new().days(1)) else {
                break;
            };
            date = next;
        }
        result
    }
}

impl Period {
    /// Calendar periods, ISO Monday weeks, and matching elapsed windows in the preceding period.
    /// # Errors
    /// Returns calendar overflow at the bounds of the supported date range.
    pub fn ranges(self, anchor: Date, today: Date) -> Result<(Range, Range), jiff::Error> {
        self.ranges_with_week_start(anchor, today, jiff::civil::Weekday::Monday)
    }

    /// # Errors
    /// Returns calendar overflow.
    pub fn ranges_with_week_start(
        self,
        anchor: Date,
        today: Date,
        week_start: jiff::civil::Weekday,
    ) -> Result<(Range, Range), jiff::Error> {
        let start = if self == Self::Week {
            let offset = (anchor.weekday().to_monday_zero_offset()
                - week_start.to_monday_zero_offset())
            .rem_euclid(7);
            anchor.checked_sub(jiff::Span::new().days(offset))?
        } else {
            self.start(anchor)?
        };
        let next = self.shift(start, 1)?;
        let end = next.checked_sub(jiff::Span::new().days(1))?.min(today);
        let previous_start = self.shift(start, -1)?;
        let previous_end = start.checked_sub(jiff::Span::new().days(1))?;
        let previous_through = self.shift(end, -1)?.min(previous_end);
        Ok((
            Range {
                from: start,
                through: end,
            },
            Range {
                from: previous_start,
                through: previous_through,
            },
        ))
    }

    /// # Errors
    /// Returns calendar overflow.
    pub fn shift(self, date: Date, amount: i32) -> Result<Date, jiff::Error> {
        let span = match self {
            Self::Day => jiff::Span::new().days(amount),
            Self::Week => jiff::Span::new().days(amount * 7),
            Self::Month => jiff::Span::new().months(amount),
            Self::Year => jiff::Span::new().years(amount),
        };
        date.checked_add(span)
    }

    /// # Errors
    /// Returns calendar overflow.
    pub fn start(self, date: Date) -> Result<Date, jiff::Error> {
        match self {
            Self::Day => Ok(date),
            Self::Week => {
                date.checked_sub(jiff::Span::new().days(date.weekday().to_monday_zero_offset()))
            }
            Self::Month => Date::new(date.year(), date.month(), 1),
            Self::Year => Date::new(date.year(), 1, 1),
        }
    }
}

#[cfg(test)]
mod tests;
