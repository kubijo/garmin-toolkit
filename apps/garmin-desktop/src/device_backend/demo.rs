use std::path::Path;

use garmin_device::attachments;
use garmin_fixtures::device::{self as fixture, Device as Fixture};

#[derive(Clone, Debug)]
pub struct Candidate {
    fixture: Fixture,
}

impl attachments::Candidate for Candidate {
    fn key(&self) -> &str {
        fixture::KEY
    }

    fn name(&self) -> &str {
        fixture::NAME
    }

    fn inspect(&self) -> Result<attachments::Metadata, String> {
        match self.fixture.presence().map_err(|error| error.to_string())? {
            fixture::Presence::Present => Ok(fixture::metadata()),
            fixture::Presence::Missing => Err("the mock device is unavailable".to_owned()),
        }
    }

    fn browse(&self) -> Result<garmin_device::DeviceCatalog, String> {
        self.fixture.catalog()
    }
}

pub type Device = garmin_device::storage::DirectoryDevice;

pub struct Platform {
    candidate: Candidate,
}

impl attachments::Backend for Platform {
    type Candidate = Candidate;

    fn poll_changed(&self) -> bool {
        false
    }

    fn candidates(&self) -> Vec<Self::Candidate> {
        match self.candidate.fixture.presence() {
            Ok(fixture::Presence::Present) | Err(_) => vec![self.candidate.clone()],
            Ok(fixture::Presence::Missing) => Vec::new(),
        }
    }
}

pub fn open(data_root: &Path) -> Result<Platform, super::Error> {
    Ok(Platform {
        candidate: Candidate {
            fixture: Fixture::recreate(data_root.join("device"))?,
        },
    })
}

#[must_use]
pub fn transport(candidate: &Candidate) -> Device {
    candidate.fixture.transport()
}

#[cfg(test)]
mod tests {
    use garmin_device::attachments::{Backend as _, Candidate as _};

    use super::*;

    #[test]
    fn platform_exposes_one_ready_browsable_device() {
        let directory = tempfile::tempdir().unwrap();
        let platform = open(directory.path()).unwrap();
        let candidates = platform.candidates();
        let [candidate] = candidates.as_slice() else {
            panic!("demo platform did not expose exactly one device");
        };

        assert_eq!(candidate.key(), fixture::KEY);
        assert_eq!(candidate.inspect().unwrap().name, fixture::NAME);
        assert!(!candidate.browse().unwrap().storages[0].entries.is_empty());
    }
}
