//! Shared locale selection and embedded translation catalogs.

use std::{collections::HashMap, sync::Arc};

pub use formatjs_intl::{Intl, format_message, message_descriptor};
use formatjs_intl::{IntlCache, MessageCatalog, Messages};
use thiserror::Error;

const DEFAULT_LOCALE: &str = "en";
const CZECH_CATALOG: &str = include_str!("../catalogs/cs.json");

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
        Ok(Intl::try_new(
            [language.tag()],
            DEFAULT_LOCALE,
            self.catalog.clone(),
            self.cache.clone(),
        )?)
    }
}

/// Localization setup failure.
#[derive(Debug, Error)]
pub enum Error {
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
}
