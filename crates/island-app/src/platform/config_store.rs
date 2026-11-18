//! Pure configuration observations and runtime write decisions.

use std::{fmt, io};

use island_core::config_state::{
    ChangeFingerprint, PrimaryObservation, RuntimeMode, StartupOutcome,
};
use island_core::migration::{MigrationError, MigrationOutcome};

use super::settings::classify_ignoring_unsupported;

/// Classify the result of reading the primary configuration.
#[allow(dead_code, reason = "wired at startup in 2C-2 S2")]
pub(crate) fn observe(read: io::Result<String>) -> PrimaryObservation {
    let text = match read {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return PrimaryObservation::Missing;
        }
        Err(_) => return PrimaryObservation::Unreadable,
    };
    let fingerprint = ChangeFingerprint::of(text.as_bytes());
    match classify_ignoring_unsupported(&text) {
        Ok(MigrationOutcome::AlreadyCurrent) => PrimaryObservation::Current { fingerprint },
        Ok(MigrationOutcome::Future { version }) => PrimaryObservation::Future {
            schema: u32::try_from(version).unwrap_or(u32::MAX),
            fingerprint,
        },
        Ok(MigrationOutcome::Prepared { .. }) => PrimaryObservation::Legacy {
            schema: 1,
            fingerprint,
        },
        Err(MigrationError::InputTooLarge)
        | Err(MigrationError::OutputTooLarge)
        | Err(MigrationError::DuplicateCriticalKey(_))
        | Err(MigrationError::MalformedCriticalSetting(_))
        | Err(MigrationError::InvalidSchemaVersion)
        | Err(MigrationError::InvalidLegacyValue)
        | Err(MigrationError::InvalidModernValue(_))
        | Err(MigrationError::UnsupportedFormat { .. })
        | Err(MigrationError::InvalidOutput) => PrimaryObservation::Invalid { fingerprint },
    }
}

/// Select the runtime mode with migration disabled.
#[allow(dead_code, reason = "wired at startup in 2C-2 S2")]
pub(crate) fn effective_outcome(outcome: StartupOutcome) -> RuntimeMode {
    match outcome {
        StartupOutcome::Migrate { .. } => RuntimeMode::LegacyCompatible,
        other => other.mode(),
    }
}

/// Return whether the runtime mode permits settings writes.
#[allow(dead_code, reason = "wired at startup in 2C-2 S2")]
pub(crate) fn writes_allowed(mode: &RuntimeMode) -> bool {
    match mode {
        RuntimeMode::Normal | RuntimeMode::LegacyCompatible => true,
        RuntimeMode::ReadOnly | RuntimeMode::Recovery => false,
    }
}

/// Identifies a write refused by the runtime mode.
#[allow(dead_code, reason = "wired at startup in 2C-2 S2")]
#[derive(Debug)]
pub(crate) struct ModeRefused;

impl fmt::Display for ModeRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("settings are read-only in this mode")
    }
}

impl std::error::Error for ModeRefused {}

/// Construct a typed runtime-mode refusal.
#[allow(dead_code, reason = "wired at startup in 2C-2 S2")]
pub(crate) fn mode_refused() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, ModeRefused)
}

/// Recognize a typed runtime-mode refusal.
#[allow(dead_code, reason = "wired at startup in 2C-2 S2")]
pub(crate) fn is_mode_refused(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|cause| cause.downcast_ref::<ModeRefused>().is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use island_core::config_state::{ArtifactInventory, ProtectedBackupName, plan_startup};

    fn outcome(observation: &PrimaryObservation, inventory: &ArtifactInventory) -> RuntimeMode {
        effective_outcome(plan_startup(observation, inventory))
    }

    #[test]
    fn observe_current_is_ready() {
        let text = "# exact bytes\r\nconfig_version = \"2\"\r\n";
        let observation = observe(Ok(text.into()));
        assert_eq!(
            observation,
            PrimaryObservation::Current {
                fingerprint: ChangeFingerprint::of(text.as_bytes())
            }
        );
        assert_eq!(
            outcome(&observation, &ArtifactInventory::default()),
            RuntimeMode::Normal
        );
    }

    #[test]
    fn observe_v1_maps_to_legacy_fallback() {
        let text = "config_version = \"1\"\n";
        let observation = observe(Ok(text.into()));
        assert_eq!(
            observation,
            PrimaryObservation::Legacy {
                schema: 1,
                fingerprint: ChangeFingerprint::of(text.as_bytes())
            }
        );
        assert_eq!(
            outcome(&observation, &ArtifactInventory::default()),
            RuntimeMode::LegacyCompatible
        );
    }

    #[test]
    fn observe_future_is_read_only() {
        for (version, schema) in [(100_u64, 100), (u64::MAX, u32::MAX)] {
            let text = format!("unsupported line\nconfig_version = \"{version}\"\n");
            let observation = observe(Ok(text.clone()));
            assert_eq!(
                observation,
                PrimaryObservation::Future {
                    schema,
                    fingerprint: ChangeFingerprint::of(text.as_bytes())
                }
            );
            assert_eq!(
                outcome(&observation, &ArtifactInventory::default()),
                RuntimeMode::ReadOnly
            );
        }
    }

    #[test]
    fn observe_invalid_version_is_invalid() {
        for text in ["config_version = \"x\"\n", "config_version = \"0\"\n"] {
            assert_eq!(
                observe(Ok(text.into())),
                PrimaryObservation::Invalid {
                    fingerprint: ChangeFingerprint::of(text.as_bytes())
                }
            );
        }
    }

    #[test]
    fn observe_duplicate_critical_is_invalid() {
        let text = "config_version = \"2\"\nconfig_version = \"2\"\n";
        assert_eq!(
            observe(Ok(text.into())),
            PrimaryObservation::Invalid {
                fingerprint: ChangeFingerprint::of(text.as_bytes())
            }
        );
    }

    #[test]
    fn observe_unreadable() {
        assert_eq!(
            observe(Err(io::ErrorKind::PermissionDenied.into())),
            PrimaryObservation::Unreadable
        );
    }

    #[test]
    fn missing_with_rolling_backup_is_recovery() {
        let observation = observe(Err(io::ErrorKind::NotFound.into()));
        let inventory = ArtifactInventory {
            rolling_backup: true,
            ..Default::default()
        };
        assert_eq!(outcome(&observation, &inventory), RuntimeMode::Recovery);
    }

    #[test]
    fn missing_alone_is_new_install() {
        let observation = observe(Err(io::ErrorKind::NotFound.into()));
        let plan = plan_startup(&observation, &ArtifactInventory::default());
        assert_eq!(plan, StartupOutcome::NewInstall);
        assert_eq!(effective_outcome(plan), RuntimeMode::Normal);
    }

    #[test]
    fn migrate_never_returned() {
        let fingerprint = ChangeFingerprint::of(b"source");
        let observations = [
            PrimaryObservation::Missing,
            PrimaryObservation::Unreadable,
            PrimaryObservation::Invalid { fingerprint },
            PrimaryObservation::Future {
                schema: 3,
                fingerprint,
            },
            PrimaryObservation::Current { fingerprint },
            PrimaryObservation::Legacy {
                schema: 1,
                fingerprint,
            },
        ];
        let inventories = [
            ArtifactInventory::default(),
            ArtifactInventory {
                rolling_backup: true,
                ..Default::default()
            },
            ArtifactInventory {
                protected_backups: vec![ProtectedBackupName::new(1, 1, fingerprint).unwrap()],
                ..Default::default()
            },
            ArtifactInventory {
                leftover_drafts: 1,
                ..Default::default()
            },
            ArtifactInventory {
                recorded_failure: Some(fingerprint),
                ..Default::default()
            },
        ];
        for observation in &observations {
            for inventory in &inventories {
                let plan = plan_startup(observation, inventory);
                let expected = if matches!(plan, StartupOutcome::Migrate { .. }) {
                    RuntimeMode::LegacyCompatible
                } else {
                    plan.mode()
                };
                assert_eq!(effective_outcome(plan), expected);
            }
        }
    }

    #[test]
    fn write_gate_table() {
        for (mode, expected) in [
            (RuntimeMode::Normal, true),
            (RuntimeMode::LegacyCompatible, true),
            (RuntimeMode::ReadOnly, false),
            (RuntimeMode::Recovery, false),
        ] {
            assert_eq!(writes_allowed(&mode), expected);
        }
    }

    #[test]
    fn mode_refused_round_trip() {
        let error = mode_refused();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert!(is_mode_refused(&error));
        assert!(!is_mode_refused(&io::Error::from(
            io::ErrorKind::PermissionDenied
        )));
        let changed = io::Error::other(super::super::config_fs::PrimaryChanged);
        assert!(!is_mode_refused(&changed));
        assert!(!super::super::config_commit::is_primary_changed(&error));
        assert!(!error.to_string().contains(['/', '\\']));
    }
}
