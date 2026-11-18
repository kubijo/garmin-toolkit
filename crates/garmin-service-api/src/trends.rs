//! Compatibility payload for the original Trends RPC.

use garmin_model::garmin_connect::trends::{History, Sport};
use jiff::civil::Date;

#[garmin_macros::portable]
pub struct LegacyHistory {
    pub days: Vec<LegacyDay>,
    pub undated: usize,
    pub summaries: usize,
}

#[garmin_macros::portable]
pub struct LegacyDay {
    pub date: Date,
    pub sport: Sport,
    pub totals: LegacyTotals,
}

#[garmin_macros::portable]
pub struct LegacyTotals {
    pub activities: usize,
    pub distance_metres: f64,
    pub duration_seconds: f64,
    pub missing_distance: usize,
    pub missing_duration: usize,
}

impl From<History> for LegacyHistory {
    fn from(history: History) -> Self {
        Self {
            days: history
                .days
                .into_iter()
                .map(|day| LegacyDay {
                    date: day.date,
                    sport: day.sport,
                    totals: LegacyTotals {
                        activities: day.totals.activities,
                        distance_metres: day.totals.distance.as_metres(),
                        duration_seconds: day.totals.duration.as_seconds(),
                        missing_distance: day.totals.missing_distance,
                        missing_duration: day.totals.missing_duration,
                    },
                })
                .collect(),
            undated: history.undated,
            summaries: history.summaries,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use remoc::ConnectExt as _;

    #[derive(serde::Serialize, serde::Deserialize)]
    struct OldHistory {
        days: Vec<OldDay>,
        undated: usize,
        summaries: usize,
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct OldDay {
        date: Date,
        sport: Sport,
        totals: OldTotals,
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct OldTotals {
        activities: usize,
        distance_metres: f64,
        duration_seconds: f64,
        missing_distance: usize,
        missing_duration: usize,
    }

    #[tokio::test]
    async fn legacy_reply_decodes_with_original_fields_over_remoc_codec() {
        use garmin_model::{
            activity::{ActivityDuration, Distance},
            garmin_connect::trends::{Day, Totals},
        };
        let date = jiff::civil::date(2026, 11, 18);
        let reply = LegacyHistory::from(History {
            days: vec![Day {
                date,
                sport: Sport::Running,
                totals: Totals {
                    activities: 3,
                    distance: Distance::from_millimeters(123_456),
                    duration: ActivityDuration::from_milliseconds(4_250),
                    missing_distance: 1,
                    missing_duration: 2,
                    ..Totals::default()
                },
            }],
            undated: 4,
            summaries: 7,
            ..History::default()
        });
        let (server, browser) = tokio::io::duplex(64 * 1024);
        let (read, write) = tokio::io::split(server);
        let sending =
            tokio::spawn(remoc::Connect::io(remoc::Cfg::default(), read, write).provide(reply));
        let (read, write) = tokio::io::split(browser);
        let old: OldHistory = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            remoc::Connect::io(remoc::Cfg::default(), read, write).consume(),
        )
        .await
        .unwrap()
        .unwrap();
        sending.await.unwrap().unwrap();
        assert_eq!((old.undated, old.summaries), (4, 7));
        let day = &old.days[0];
        assert_eq!((day.date, day.sport), (date, Sport::Running));
        assert_eq!(
            (
                day.totals.activities,
                day.totals.missing_distance,
                day.totals.missing_duration
            ),
            (3, 1, 2)
        );
        assert!((day.totals.distance_metres - 123.456).abs() < 0.000_001);
        assert!((day.totals.duration_seconds - 4.25).abs() < 0.000_001);
    }
}
