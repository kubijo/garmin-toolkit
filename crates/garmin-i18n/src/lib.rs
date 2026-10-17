//! Shared locale selection and embedded translation catalogs.

use std::{collections::HashMap, ops::Deref, sync::Arc};

use formatjs_intl::{IntlCache, MessageCatalog, Messages};
pub use formatjs_intl::{format_message, message_descriptor};
use icu_calendar::{Date, options::DateAddOptions, types::DateDuration, week::WeekInformation};
use icu_datetime::{DateTimeFormatter, fieldsets, input};
use icu_decimal::options::DecimalFormatterOptions;
use icu_locale::{Locale, LocaleExpander};
use thiserror::Error;

const DEFAULT_LOCALE: &str = "en";
const CZECH_CATALOG: &str = include_str!(concat!(env!("OUT_DIR"), "/cs.json"));

/// A bundled application language.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Language {
    /// English source messages.
    #[default]
    English,
    Czech,
}

impl Language {
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::English => DEFAULT_LOCALE,
            Self::Czech => "cs",
        }
    }

    /// Matches a supported language from a BCP-47 locale.
    #[must_use]
    pub fn from_locale(locale: &str) -> Option<Self> {
        match locale.split('-').next()? {
            language if language.eq_ignore_ascii_case("c") => Some(Self::English),
            language if language.eq_ignore_ascii_case("posix") => Some(Self::English),
            language if language.eq_ignore_ascii_case("en") => Some(Self::English),
            language if language.eq_ignore_ascii_case("cs") => Some(Self::Czech),
            _ => None,
        }
    }
}

/// Immutable translation catalogs and their shared compiled-message cache.
#[derive(Clone)]
pub struct Translations {
    catalog: Arc<MessageCatalog>,
    cache: Arc<IntlCache>,
}

impl Translations {
    /// Loads the catalogs embedded in the application.
    /// # Errors
    /// [`enum@Error`] when the Czech JSON or `FormatJS` catalog is invalid.
    pub fn bundled() -> Result<Self, Error> {
        let czech: Messages = serde_json::from_str(CZECH_CATALOG)?;
        let mut catalog = MessageCatalog::new();
        catalog.insert(DEFAULT_LOCALE, HashMap::new())?;
        catalog.insert(Language::Czech.tag(), czech)?;
        Ok(Self {
            catalog: Arc::new(catalog),
            cache: Arc::new(IntlCache::new()),
        })
    }

    /// Creates a formatter for one supported language.
    /// # Errors
    /// [`enum@Error`] when `FormatJS` rejects the locale configuration.
    pub fn formatter(&self, language: Language) -> Result<Intl, Error> {
        self.formatter_with_locale(language, language.tag())
    }

    /// Uses the client's regional preferences while retaining the selected UI language.
    /// # Errors
    /// [`enum@Error`] if a formatter cannot load its bundled locale data.
    pub fn formatter_for_client(&self, language: Language) -> Result<Intl, Error> {
        let locale = client_locale().unwrap_or_else(|| language.tag().to_owned());
        self.formatter_with_locale(language, &locale)
    }

    /// Selects message language independently from the regional date/time preferences.
    /// # Errors
    /// [`enum@Error`] if the locale or its bundled data is invalid.
    pub fn formatter_with_locale(&self, language: Language, locale: &str) -> Result<Intl, Error> {
        let mut locale: Locale = locale
            .parse()
            .map_err(|_| Error::Locale(locale.to_owned()))?;
        // Infer the original region before replacing its language. Keep Unicode preferences.
        LocaleExpander::new_extended().maximize(&mut locale.id);
        locale.id.language = match language {
            Language::English => icu_locale::locale!("en").id.language,
            Language::Czech => icu_locale::locale!("cs").id.language,
        };
        locale.id.script = None;
        let dates = DateFormats::new(&locale)?;
        let messages = formatjs_intl::Intl::try_new(
            [language.tag()],
            DEFAULT_LOCALE,
            self.catalog.clone(),
            self.cache.clone(),
        )?;
        Ok(Intl { messages, dates })
    }
}

fn client_locale() -> Option<String> {
    // sys-locale discovers platform/browser locales; Unix date formatting has its own category.
    #[cfg(unix)]
    for key in ["LC_ALL", "LC_TIME", "LANG"] {
        if let Ok(value) = std::env::var(key)
            && !value.is_empty()
        {
            return posix_locale(&value);
        }
    }
    sys_locale::get_locales().find(|locale| locale.parse::<Locale>().is_ok())
}

#[cfg(unix)]
fn posix_locale(value: &str) -> Option<String> {
    let tag = value.split(['.', '@']).next()?.replace('_', "-");
    tag.parse::<Locale>().ok().map(|locale| locale.to_string())
}

/// Message translations and independent regional date/time formatting.
pub struct Intl {
    messages: formatjs_intl::Intl,
    dates: DateFormats,
}

impl Deref for Intl {
    type Target = formatjs_intl::Intl;

    fn deref(&self) -> &Self::Target {
        &self.messages
    }
}

impl Intl {
    #[must_use]
    pub const fn dates(&self) -> &DateFormats {
        &self.dates
    }
}

/// ICU-backed civil calendar and formatting. Instant-to-local conversion belongs to Jiff.
pub struct DateFormats {
    locale: String,
    week: WeekInformation,
    month: DateTimeFormatter<fieldsets::YM>,
    weekday: DateTimeFormatter<fieldsets::E>,
    day: icu_decimal::DecimalFormatter,
    date: DateTimeFormatter<fieldsets::YMD>,
    datetime: DateTimeFormatter<fieldsets::YMDT>,
    time: DateTimeFormatter<fieldsets::T>,
}

/// Display data for a month, with ISO civil dates for matching stored activity instants.
pub struct MonthGrid {
    pub title: String,
    pub weekdays: [String; 7],
    pub days: [Option<CalendarDay>; 42],
    pub previous: Option<jiff::civil::Date>,
    pub next: Option<jiff::civil::Date>,
}

pub struct CalendarDay {
    pub date: jiff::civil::Date,
    pub label: String,
    pub description: String,
}

impl DateFormats {
    fn new(locale: &Locale) -> Result<Self, Error> {
        Ok(Self {
            locale: locale.to_string(),
            week: WeekInformation::try_new(locale.into())
                .map_err(|error| Error::Calendar(error.to_string()))?,
            month: DateTimeFormatter::try_new(locale.into(), fieldsets::YM::long())?,
            weekday: DateTimeFormatter::try_new(locale.into(), fieldsets::E::short())?,
            day: icu_decimal::DecimalFormatter::try_new(
                locale.into(),
                DecimalFormatterOptions::default(),
            )
            .map_err(|error| Error::Calendar(error.to_string()))?,
            date: DateTimeFormatter::try_new(locale.into(), fieldsets::YMD::long())?,
            datetime: DateTimeFormatter::try_new(
                locale.into(),
                fieldsets::YMDT::medium()
                    .with_time_precision(icu_datetime::options::TimePrecision::Minute),
            )?,
            time: DateTimeFormatter::try_new(
                locale.into(),
                fieldsets::T::short()
                    .with_time_precision(icu_datetime::options::TimePrecision::Minute),
            )?,
        })
    }

    #[must_use]
    pub fn locale(&self) -> &str {
        &self.locale
    }

    /// Produce six display rows using ICU's selected calendar, month arithmetic, and week rules.
    /// # Panics
    /// If ICU violates its supported civil range or month-size invariants.
    #[must_use]
    pub fn month(&self, anchor: jiff::civil::Date) -> MonthGrid {
        let date = iso_date(anchor).to_calendar(self.month.calendar());
        let first = date
            .try_added_with_options(
                DateDuration::for_days(1 - i32::from(date.day_of_month().0)),
                DateAddOptions::default(),
            )
            .expect("the containing month starts within ICU's civil range");
        // This is only the seven-column placement; ICU owns the dates and calendar boundaries.
        let offset = (first.weekday() as i32 - self.week.first_weekday as i32).rem_euclid(7);
        let mut days = std::array::from_fn(|_| None);
        for day in 0..first.days_in_month() {
            let date = first
                .try_added_with_options(
                    DateDuration::for_days(i32::from(day)),
                    DateAddOptions::default(),
                )
                .expect("days in an ICU month remain in range");
            let Some(iso) = civil_date(date.to_calendar(icu_calendar::cal::Iso)) else {
                continue;
            };
            days[usize::try_from(offset).expect("weekday offset is nonnegative")
                + usize::from(day)] = Some(CalendarDay {
                date: iso,
                label: self.day.format_to_string(&date.day_of_month().0.into()),
                description: self.date.format(&date).to_string(),
            });
        }
        MonthGrid {
            title: self.month.format(&first).to_string(),
            weekdays: std::array::from_fn(|column| {
                let weekday = icu_calendar::types::Weekday::from_days_since_sunday(
                    self.week.first_weekday as isize
                        + isize::try_from(column).expect("seven columns"),
                );
                self.weekday.format(&weekday).to_string()
            }),
            days,
            previous: first
                .try_added_with_options(DateDuration::for_months(-1), DateAddOptions::default())
                .ok()
                .and_then(|date| civil_date(date.to_calendar(icu_calendar::cal::Iso))),
            next: first
                .try_added_with_options(DateDuration::for_months(1), DateAddOptions::default())
                .ok()
                .and_then(|date| civil_date(date.to_calendar(icu_calendar::cal::Iso))),
        }
    }

    #[must_use]
    pub fn datetime(&self, local: jiff::civil::DateTime) -> String {
        self.datetime
            .format(&input::DateTime {
                date: iso_date(local.date()),
                time: icu_time(local.time()),
            })
            .to_string()
    }

    #[must_use]
    pub fn time(&self, local: jiff::civil::Time) -> String {
        self.time.format(&icu_time(local)).to_string()
    }
}

fn iso_date(date: jiff::civil::Date) -> Date<icu_calendar::cal::Iso> {
    Date::try_new_iso(
        i32::from(date.year()),
        date.month().unsigned_abs(),
        date.day().unsigned_abs(),
    )
    .expect("Jiff civil dates fit ICU's ISO range")
}

fn civil_date(date: Date<icu_calendar::cal::Iso>) -> Option<jiff::civil::Date> {
    jiff::civil::Date::new(
        i16::try_from(date.year().extended_year()).ok()?,
        i8::try_from(date.month().ordinal).ok()?,
        i8::try_from(date.day_of_month().0).ok()?,
    )
    .ok()
}

fn icu_time(time: jiff::civil::Time) -> input::Time {
    input::Time::try_new(
        time.hour().unsigned_abs(),
        time.minute().unsigned_abs(),
        time.second().unsigned_abs(),
        time.subsec_nanosecond().unsigned_abs(),
    )
    .expect("Jiff civil times fit ICU's time range")
}

/// Localization setup failure.
#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid formatting locale: {0}")]
    Locale(String),
    #[error("calendar data unavailable: {0}")]
    Calendar(String),
    #[error("date formatter unavailable: {0}")]
    DateFormatter(#[from] icu_datetime::DateTimeFormatterLoadError),
    #[error("invalid embedded translation catalog: {0}")]
    Catalog(#[from] serde_json::Error),
    #[error("localization runtime failed: {0}")]
    Runtime(#[from] formatjs_intl::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_fallback_and_czech_plural_rules_are_operational() -> Result<(), Error> {
        let translations = Translations::bundled()?;
        let english = translations.formatter(Language::English)?;
        let czech = translations.formatter(Language::Czech)?;

        let english_message = format_message!(
            &english,
            default_message: "{count, plural, one {# activity} other {# activities}}",
            description: "FormatJS plural validation fixture",
            values: { count: 2_i64 },
        );
        let czech_message = format_message!(
            &czech,
            default_message: "{count, plural, one {# activity} other {# activities}}",
            description: "FormatJS plural validation fixture",
            values: { count: 2_i64 },
        );

        assert_eq!(english_message, "2 activities");
        assert_eq!(czech_message, "2 aktivity");
        Ok(())
    }

    #[test]
    fn supported_languages_match_bcp47_locales() {
        assert_eq!(Language::from_locale("cs-CZ"), Some(Language::Czech));
        assert_eq!(Language::from_locale("en-US"), Some(Language::English));
        assert_eq!(Language::from_locale("C"), Some(Language::English));
        assert_eq!(Language::from_locale("de-DE"), None);
    }

    #[test]
    fn regional_week_rules_survive_message_language_selection() {
        let translations = Translations::bundled().unwrap();
        let march = jiff::civil::date(2026, 3, 1);
        for (locale, column) in [
            ("en-US", 0),
            ("en-GB", 6),
            ("fi-FI", 6),
            ("en-US-u-rg-gbzzzz", 6),
            ("en-US-u-fw-wed", 4),
        ] {
            let intl = translations
                .formatter_with_locale(Language::Czech, locale)
                .unwrap();
            let month = intl.dates().month(march);
            assert_eq!(
                month.days.iter().position(Option::is_some),
                Some(column),
                "{locale}"
            );
            assert!(month.title.contains("březen"), "{}", month.title);
            assert_eq!(intl.locale().id.language.as_str(), "cs");
        }
    }

    #[test]
    fn hour_cycle_preferences_and_civil_dates_are_preserved() {
        let translations = Translations::bundled().unwrap();
        let local = jiff::civil::date(2026, 3, 29).at(17, 45, 0, 0);
        for locale in ["en-GB", "en-US-u-hc-h23"] {
            let intl = translations
                .formatter_with_locale(Language::English, locale)
                .unwrap();
            assert_eq!(intl.dates().time(local.time()), "17:45");
            assert!(intl.dates().datetime(local).contains("29"));
        }
        let twelve = translations
            .formatter_with_locale(Language::English, "en-GB-u-hc-h12")
            .unwrap();
        assert!(twelve.dates().time(local.time()).contains("5:45"));
    }

    #[test]
    fn icu_month_arithmetic_handles_centuries_and_year_boundaries() {
        let intl = Translations::bundled()
            .unwrap()
            .formatter(Language::English)
            .unwrap();
        for (year, count) in [(1900, 28), (2000, 29), (2024, 29), (2100, 28)] {
            let month = intl.dates().month(jiff::civil::date(year, 2, 15));
            let dates: Vec<_> = month.days.iter().flatten().map(|day| day.date).collect();
            assert_eq!(dates.len(), count);
            assert_eq!(dates[0], jiff::civil::date(year, 2, 1));
            assert_eq!(month.next, Some(jiff::civil::date(year, 3, 1)));
        }
        let december = intl.dates().month(jiff::civil::date(2025, 12, 31));
        assert_eq!(december.next, Some(jiff::civil::date(2026, 1, 1)));
        let january = intl.dates().month(december.next.unwrap());
        assert_eq!(january.previous, Some(jiff::civil::date(2025, 12, 1)));
        assert!(
            intl.dates()
                .month(jiff::civil::Date::MIN)
                .previous
                .is_none()
        );
        assert!(intl.dates().month(jiff::civil::Date::MAX).next.is_none());
    }

    #[test]
    fn non_gregorian_grid_labels_and_navigation_use_the_same_calendar() {
        let translations = Translations::bundled().unwrap();
        let intl = translations
            .formatter_with_locale(Language::English, "en-US-u-ca-hebrew")
            .unwrap();
        // Adar I exists only in a Hebrew leap year; adjacent months cross Gregorian boundaries.
        let grid = intl.dates().month(jiff::civil::date(2024, 2, 20));
        assert!(grid.title.contains("Adar"), "{}", grid.title);
        let dates: Vec<_> = grid.days.iter().flatten().map(|day| day.date).collect();
        assert_eq!(dates.len(), 30);
        assert_eq!(dates[0], jiff::civil::date(2024, 2, 10));
        assert_eq!(grid.next, Some(jiff::civil::date(2024, 3, 11)));
        assert_eq!(
            intl.dates().month(grid.next.unwrap()).previous,
            Some(dates[0])
        );
        assert_eq!(grid.days.iter().flatten().next().unwrap().label, "1");
    }

    #[test]
    #[cfg(unix)]
    fn posix_time_category_is_normalized_without_changing_process_environment() {
        assert_eq!(posix_locale("en_GB.UTF-8"), Some("en-GB".into()));
        assert_eq!(posix_locale("fi_FI.UTF-8"), Some("fi-FI".into()));
        assert_eq!(posix_locale("C.UTF-8"), None);
        assert_eq!(posix_locale("POSIX"), None);
    }
}
