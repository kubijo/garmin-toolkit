//! Portable users, connector sources, and physical devices.

use garmin_color::Color;
use thiserror::Error;

use crate::artifact::ArtifactId;

define_id!(
    UserIdKind,
    UserId,
    "user",
    "Type marker for portable user IDs.",
    "A portable application-user ID."
);
define_id!(
    SourceIdKind,
    SourceId,
    "source",
    "Type marker for connector-source IDs.",
    "An application-owned connector-source ID."
);
define_id!(
    DeviceIdKind,
    DeviceId,
    "device",
    "Type marker for physical-device IDs.",
    "An application-owned physical-device ID."
);

text_value!(
    DisplayName,
    Error,
    Error::EmptyDisplayName,
    "A user-selected, non-unique display name."
);
text_value!(
    SourceLabel,
    Error,
    Error::EmptySourceLabel,
    "A user-visible connector-source label."
);
text_value!(
    DeviceLabel,
    Error,
    Error::EmptyDeviceLabel,
    "A user-visible physical-device label."
);

/// An artifact selected as a profile avatar.
#[garmin_macros::portable(copy, hash)]
pub struct AvatarArtifactId(ArtifactId);

impl AvatarArtifactId {
    #[must_use]
    pub const fn from_artifact_id(id: ArtifactId) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_artifact_id(&self) -> ArtifactId {
        self.0
    }

    #[must_use]
    pub const fn into_artifact_id(self) -> ArtifactId {
        self.0
    }
}

/// Non-zero pixel dimensions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageDimensions {
    width: u32,
    height: u32,
}

/// Units used to present physical measurements.
#[garmin_macros::portable(copy, default, hash)]
pub enum UnitSystem {
    /// Metric units.
    #[default]
    Metric,
    Imperial,
}

/// Bundled presentation language.
#[garmin_macros::portable(copy, default, hash)]
pub enum LanguagePreference {
    /// English source messages.
    #[default]
    English,
    Czech,
}

/// Selected semantic color theme.
#[garmin_macros::portable(copy, default, hash)]
pub enum ThemePreference {
    /// Follow the system theme.
    #[default]
    Auto,
    Dark,
    Light,
}

/// Portable profile presentation preferences.
#[garmin_macros::portable(copy, default, hash)]
pub struct ProfilePreferences {
    unit_system: UnitSystem,
    language: LanguagePreference,
    theme: ThemePreference,
}

impl ProfilePreferences {
    #[must_use]
    pub const fn from_parts(
        unit_system: UnitSystem,
        language: LanguagePreference,
        theme: ThemePreference,
    ) -> Self {
        Self {
            unit_system,
            language,
            theme,
        }
    }

    #[must_use]
    pub const fn unit_system(self) -> UnitSystem {
        self.unit_system
    }

    #[must_use]
    pub const fn language(self) -> LanguagePreference {
        self.language
    }

    #[must_use]
    pub const fn theme(self) -> ThemePreference {
        self.theme
    }
}

impl ImageDimensions {
    /// Validates pixel dimensions.
    /// # Errors
    /// [`ImageDimensionsError`] when either dimension is zero.
    pub const fn from_width_height(width: u32, height: u32) -> Result<Self, ImageDimensionsError> {
        if width == 0 || height == 0 {
            return Err(ImageDimensionsError);
        }
        Ok(Self { width, height })
    }

    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    #[must_use]
    pub const fn into_width_height(self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// Zero image dimension.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("image dimensions must be non-zero")]
pub struct ImageDimensionsError;

/// A mutable portable-user profile.
#[garmin_macros::portable(eq)]
pub struct Profile {
    display_name: DisplayName,
    accent: Option<Color>,
    avatar_artifact_id: Option<AvatarArtifactId>,
    preferences: ProfilePreferences,
}

impl Profile {
    #[must_use]
    pub const fn from_display_name(display_name: DisplayName) -> Self {
        Self::from_parts(display_name, None, None)
    }

    #[must_use]
    pub const fn from_parts(
        display_name: DisplayName,
        accent: Option<Color>,
        avatar_artifact_id: Option<AvatarArtifactId>,
    ) -> Self {
        Self::from_complete(
            display_name,
            accent,
            avatar_artifact_id,
            ProfilePreferences::from_parts(
                UnitSystem::Metric,
                LanguagePreference::English,
                ThemePreference::Auto,
            ),
        )
    }

    #[must_use]
    pub const fn from_complete(
        display_name: DisplayName,
        accent: Option<Color>,
        avatar_artifact_id: Option<AvatarArtifactId>,
        preferences: ProfilePreferences,
    ) -> Self {
        Self {
            display_name,
            accent,
            avatar_artifact_id,
            preferences,
        }
    }

    #[must_use]
    pub const fn display_name(&self) -> &DisplayName {
        &self.display_name
    }

    #[must_use]
    pub const fn accent(&self) -> Option<Color> {
        self.accent
    }

    #[must_use]
    pub const fn avatar_artifact_id(&self) -> Option<AvatarArtifactId> {
        self.avatar_artifact_id
    }

    #[must_use]
    pub const fn preferences(&self) -> ProfilePreferences {
        self.preferences
    }

    /// Replaces presentation preferences.
    pub const fn replace_preferences(
        &mut self,
        preferences: ProfilePreferences,
    ) -> ProfilePreferences {
        std::mem::replace(&mut self.preferences, preferences)
    }
}

/// A deployment-level user role.
#[garmin_macros::portable(copy, hash)]
pub enum Role {
    Owner,
    Member,
}

/// A portable application user.
#[garmin_macros::portable(eq)]
pub struct User {
    id: UserId,
    role: Role,
    profile: Profile,
}

impl User {
    #[must_use]
    pub const fn from_parts(id: UserId, role: Role, profile: Profile) -> Self {
        Self { id, role, profile }
    }

    #[must_use]
    pub const fn id(&self) -> UserId {
        self.id
    }

    #[must_use]
    pub const fn role(&self) -> Role {
        self.role
    }

    #[must_use]
    pub const fn profile(&self) -> &Profile {
        &self.profile
    }

    /// Replaces all mutable profile data.
    pub const fn replace_profile(&mut self, profile: Profile) -> Profile {
        std::mem::replace(&mut self.profile, profile)
    }
}

/// A physical device independent of external identifiers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Device {
    id: DeviceId,
    label: DeviceLabel,
}

impl Device {
    #[must_use]
    pub const fn from_parts(id: DeviceId, label: DeviceLabel) -> Self {
        Self { id, label }
    }

    #[must_use]
    pub const fn id(&self) -> DeviceId {
        self.id
    }

    #[must_use]
    pub const fn label(&self) -> &DeviceLabel {
        &self.label
    }
}

/// One user-owned connector instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Source {
    id: SourceId,
    owner_id: UserId,
    label: SourceLabel,
    device_id: Option<DeviceId>,
}

impl Source {
    #[must_use]
    pub const fn from_parts(
        id: SourceId,
        owner_id: UserId,
        label: SourceLabel,
        device_id: Option<DeviceId>,
    ) -> Self {
        Self {
            id,
            owner_id,
            label,
            device_id,
        }
    }

    #[must_use]
    pub const fn id(&self) -> SourceId {
        self.id
    }

    #[must_use]
    pub const fn owner_id(&self) -> UserId {
        self.owner_id
    }

    #[must_use]
    pub const fn label(&self) -> &SourceLabel {
        &self.label
    }

    #[must_use]
    pub const fn device_id(&self) -> Option<DeviceId> {
        self.device_id
    }
}

/// Invalid identity data.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum Error {
    #[error("user display name cannot be blank")]
    EmptyDisplayName,
    #[error("source label cannot be blank")]
    EmptySourceLabel,
    #[error("device label cannot be blank")]
    EmptyDeviceLabel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profiles_trim_names_without_making_them_unique() -> Result<(), Error> {
        let profile_a = Profile::from_display_name(" Rider ".parse()?);
        let profile_b = Profile::from_display_name("Rider".parse()?);

        assert_eq!(profile_a, profile_b);
        assert_eq!(profile_a.display_name().as_str(), "Rider");
        Ok(())
    }

    #[test]
    fn users_replace_only_mutable_profile_data() -> Result<(), Error> {
        let id = UserId::new_v4();
        let mut user =
            User::from_parts(id, Role::Owner, Profile::from_display_name("One".parse()?));

        let previous = user.replace_profile(Profile::from_display_name("Two".parse()?));

        assert_eq!(user.id(), id);
        assert_eq!(user.role(), Role::Owner);
        assert_eq!(previous.display_name().as_str(), "One");
        assert_eq!(user.profile().display_name().as_str(), "Two");
        Ok(())
    }

    #[test]
    fn profile_preferences_are_complete_and_replaceable() -> Result<(), Error> {
        let mut profile = Profile::from_display_name("Rider".parse()?);
        assert_eq!(profile.preferences(), ProfilePreferences::default());

        let preferences = ProfilePreferences::from_parts(
            UnitSystem::Imperial,
            LanguagePreference::Czech,
            ThemePreference::Light,
        );
        let previous = profile.replace_preferences(preferences);

        assert_eq!(previous, ProfilePreferences::default());
        assert_eq!(profile.preferences(), preferences);
        let encoded = postcard::to_stdvec(&preferences).unwrap();
        let decoded = postcard::from_bytes::<ProfilePreferences>(&encoded).unwrap();
        assert_eq!(decoded, preferences);
        Ok(())
    }

    #[test]
    fn display_names_are_revalidated_when_deserialized() {
        let encoded = postcard::to_stdvec(&" \t".to_owned()).unwrap();
        assert!(postcard::from_bytes::<DisplayName>(&encoded).is_err());
    }

    #[test]
    fn blank_identity_text_is_rejected() {
        assert_eq!(" \t".parse::<DisplayName>(), Err(Error::EmptyDisplayName));
        assert_eq!("".parse::<SourceLabel>(), Err(Error::EmptySourceLabel));
        assert_eq!("\n".parse::<DeviceLabel>(), Err(Error::EmptyDeviceLabel));
    }
}
