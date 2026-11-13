use garmin_model::garmin_connect::{AccountFacts, PersonalApiKey};

use crate::{Application, Error, UserContext};

trait Verifier: Sync {
    fn check(
        &self,
        key: &PersonalApiKey,
    ) -> impl std::future::Future<Output = Result<AccountFacts, garmin_connect::Error>> + Send;
}

impl Verifier for garmin_connect::Client {
    async fn check(&self, key: &PersonalApiKey) -> Result<AccountFacts, garmin_connect::Error> {
        Self::check(self, key).await
    }
}

impl Application {
    /// Verifies a new key against Garmin before replacing the saved credential.
    /// Removal is local and requires no service access.
    /// # Errors
    /// Preserves the previous key on verification failures.
    pub async fn verify_and_save_garmin_connect_key(
        &self,
        user: UserContext,
        key: Option<PersonalApiKey>,
    ) -> Result<Option<AccountFacts>, Error> {
        self.verify_with(user, key, &garmin_connect::Client::default())
            .await
    }

    async fn verify_with(
        &self,
        user: UserContext,
        key: Option<PersonalApiKey>,
        verifier: &impl Verifier,
    ) -> Result<Option<AccountFacts>, Error> {
        self.garmin_connect_configured(user).await?;
        let facts = match key.as_ref() {
            Some(key) => Some(verifier.check(key).await?),
            None => None,
        };
        self.set_garmin_connect_key(user, key).await?;
        Ok(facts)
    }

    /// Tests the saved key without returning it to the client.
    /// # Errors
    /// Reports missing credentials or redacted Garmin service failures.
    pub async fn check_garmin_connect(&self, user: UserContext) -> Result<AccountFacts, Error> {
        self.garmin_connect_configured(user).await?;
        let key = self
            .storage
            .garmin_connect_key(user.user_id())
            .await?
            .ok_or(Error::GarminConnectKeyMissing)?;
        Ok(garmin_connect::Client::default().check(&key).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Response(bool);

    impl Verifier for Response {
        fn check(
            &self,
            _key: &PersonalApiKey,
        ) -> impl std::future::Future<Output = Result<AccountFacts, garmin_connect::Error>> + Send
        {
            std::future::ready(if self.0 {
                Ok(AccountFacts {
                    display_name: Some("Garmin Rider".into()),
                    locale: None,
                    measurement_system: None,
                    time_zone: None,
                })
            } else {
                Err(garmin_connect::Error::Rejected {
                    operation: "key exchange",
                    status: 401,
                })
            })
        }
    }

    #[test]
    fn failed_verification_preserves_the_saved_key_and_success_replaces_it()
    -> Result<(), Box<dyn std::error::Error>> {
        futures_lite::future::block_on(async {
            let root = tempfile::tempdir()?;
            let application = Application::new(
                garmin_storage::Storage::open(root.path().join("keys.sqlite3")).await?,
            );
            let owner = application.create_profile("Rider".parse()?).await?;
            let user = UserContext::new(owner.id());
            let previous = PersonalApiKey::from_string("previous-key".into())?;
            application
                .set_garmin_connect_key(user, Some(previous))
                .await?;
            let replacement = PersonalApiKey::from_string("replacement-key".into())?;
            assert!(
                application
                    .verify_with(user, Some(replacement.clone()), &Response(false))
                    .await
                    .is_err()
            );
            assert_eq!(
                application
                    .storage
                    .garmin_connect_key(owner.id())
                    .await?
                    .expect("original key retained")
                    .expose(),
                "previous-key"
            );
            let facts = application
                .verify_with(user, Some(replacement), &Response(true))
                .await?
                .expect("verified facts");
            assert_eq!(facts.display_name.as_deref(), Some("Garmin Rider"));
            assert_eq!(
                application
                    .storage
                    .garmin_connect_key(owner.id())
                    .await?
                    .expect("new key saved")
                    .expose(),
                "replacement-key"
            );
            assert!(
                application
                    .verify_with(user, None, &Response(false))
                    .await?
                    .is_none()
            );
            assert!(!application.garmin_connect_configured(user).await?);
            application.close().await;
            Ok(())
        })
    }
}
