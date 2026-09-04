//! Deterministic map planning, transfer, mutation, and recovery.

mod backup;
mod download;
mod install;
mod mounted_install;
mod plan;
mod removal;
pub mod space;

pub use backup::{BackupError, BackupFile, BackupReport, backup_mass_storage};
pub use download::{
    ArtifactCacheSummary, DownloadError, DownloadProgress, DownloadUrlAuthorizer, Downloader,
    inspect_artifact_cache,
};
pub use install::{
    ApplyError, ApplyReport, DeviceTiming, RecoveryOutcome, apply_mass_storage,
    apply_mass_storage_with_progress, preflight_mass_storage_update, recover_mass_storage,
};
pub use mounted_install::{
    MountedInstallError, MountedMtpPreflight, MountedUpdateRecoveryOutcome,
    MountedUpdateRecoveryReport, apply_mounted_mtp_with_progress, preflight_mounted_mtp_update,
    recover_mounted_mtp_update,
};
pub use plan::{
    BackupPolicy, DownloadSpec, UPDATE_PLAN_SCHEMA_VERSION, UpdatePlan, UpdatePlanError,
};
pub use removal::{
    ComponentDisposition, RemovalApplyReport, RemovalBackup, RemovalComponent,
    RemovalExecutionError, RemovalExecutionFile, RemovalExecutionPlan, RemovalFile, RemovalPlan,
    RemovalPlanError, RemovalRecoveryOutcome, RemovalRecoveryReport, RemovalTransactionJournal,
    RemovalTransactionState, execute_removal, recover_removal,
};
