//! Runtime composition supplies a destination, never a transaction engine.

use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;
use garmin_capture::SessionCapture;
use garmin_device::{
    DeviceManifest,
    storage::{DeviceRead, DeviceWrite},
};
use garmin_model::map::MapAuthorization;
use garmin_progress::ProgressReporter;
use garmin_simulator::shadow::Shadow;
use garmin_update::UpdatePlan;

#[async_trait]
pub trait UpdateTarget: Send + Sync {
    fn modifies_device(&self) -> bool;
    async fn state(&self) -> Result<garmin_device::DeviceStateSnapshot>;
    fn fixture_root(&self) -> Option<&Path> {
        None
    }
    async fn prepare(
        self: Box<Self>,
        manifest: &DeviceManifest,
        plan: &UpdatePlan,
        authorization: &MapAuthorization,
        capture: &SessionCapture,
        progress: &ProgressReporter,
    ) -> Result<Box<dyn TargetSession>>;
}

#[async_trait]
pub trait TargetSession: Send + Sync {
    fn device(&self) -> &dyn DeviceWrite;
    async fn finish(&self, capture: &SessionCapture, succeeded: bool) -> Result<Option<PathBuf>>;
}

pub struct PhysicalTarget(pub Box<dyn DeviceWrite>);

#[async_trait]
impl UpdateTarget for PhysicalTarget {
    async fn state(&self) -> Result<garmin_device::DeviceStateSnapshot> {
        Ok(self.0.state().await?)
    }
    fn modifies_device(&self) -> bool {
        true
    }
    async fn prepare(
        self: Box<Self>,
        _manifest: &DeviceManifest,
        _plan: &UpdatePlan,
        _authorization: &MapAuthorization,
        _capture: &SessionCapture,
        _progress: &ProgressReporter,
    ) -> Result<Box<dyn TargetSession>> {
        Ok(self)
    }
}

#[async_trait]
impl TargetSession for PhysicalTarget {
    fn device(&self) -> &dyn DeviceWrite {
        self.0.as_ref()
    }
    async fn finish(&self, _capture: &SessionCapture, _succeeded: bool) -> Result<Option<PathBuf>> {
        Ok(None)
    }
}

pub struct SimulatedTarget {
    pub source: Box<dyn DeviceRead>,
    pub fixture: Option<PathBuf>,
}

#[async_trait]
impl UpdateTarget for SimulatedTarget {
    async fn state(&self) -> Result<garmin_device::DeviceStateSnapshot> {
        Ok(self.source.state().await?)
    }
    fn modifies_device(&self) -> bool {
        false
    }
    fn fixture_root(&self) -> Option<&Path> {
        self.fixture.as_deref()
    }
    async fn prepare(
        self: Box<Self>,
        manifest: &DeviceManifest,
        plan: &UpdatePlan,
        authorization: &MapAuthorization,
        capture: &SessionCapture,
        progress: &ProgressReporter,
    ) -> Result<Box<dyn TargetSession>> {
        Ok(Box::new(
            Shadow::create(
                self.source.as_ref(),
                manifest,
                plan,
                authorization,
                capture,
                progress,
            )
            .await?,
        ))
    }
}

#[async_trait]
impl TargetSession for Shadow {
    fn device(&self) -> &dyn DeviceWrite {
        self.device()
    }
    async fn finish(&self, capture: &SessionCapture, succeeded: bool) -> Result<Option<PathBuf>> {
        self.finish(capture, succeeded).await?;
        Ok(Some(capture.root().join("simulation")))
    }
}
