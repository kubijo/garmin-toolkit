//! Durable storage contracts.

use std::error::Error;

use futures_lite::future::block_on;
use garmin_color::Color;
use garmin_model::identity::{
    LanguagePreference, Profile, ProfilePreferences, Role, ThemePreference, UnitSystem, User,
    UserId,
};
use garmin_storage::Storage;
use tempfile::tempdir;

#[test]
fn users_survive_reopen_and_profile_replacement() -> Result<(), Box<dyn Error>> {
    block_on(async {
        let root = tempdir()?;
        let path = root.path().join("garmin-toolkit.sqlite3");
        let user_id = UserId::new_v4();
        let mut user = User::from_parts(
            user_id,
            Role::Owner,
            Profile::from_display_name("Rider".parse()?),
        );

        let storage = Storage::open(&path).await?;
        storage.save_user(&user).await?;
        assert_eq!(storage.user(user_id).await?, Some(user.clone()));

        user.replace_profile(Profile::from_complete(
            "Renamed".parse()?,
            Some(Color::from_rgba(0x45, 0x89, 0xff, 0xc0)),
            None,
            ProfilePreferences::from_parts(
                UnitSystem::Imperial,
                LanguagePreference::Czech,
                ThemePreference::Light,
            ),
        ));
        storage.save_user(&user).await?;
        storage.check_integrity().await?;
        storage.close().await;

        let reopened = Storage::open(&path).await?;
        assert_eq!(reopened.user(user_id).await?, Some(user));
        assert_eq!(reopened.user(UserId::new_v4()).await?, None);
        reopened.check_integrity().await?;
        reopened.close().await;
        Ok(())
    })
}
