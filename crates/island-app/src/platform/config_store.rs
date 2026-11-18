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

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use super::config_fs::{ConfigDir, VerifiedFile};
use super::settings;
use island_core::config_state::{ArtifactInventory, ProtectedBackupName, plan_startup};

static STORE: Mutex<Option<Arc<ConfigSnapshot>>> = Mutex::new(None);

pub(crate) struct ConfigSnapshot {
    text: Option<String>,
    mode: RuntimeMode,
    storage_ready: bool,
}

// Readers and writers switch to these accessors in 2C-2 S3.
#[allow(dead_code, reason = "readers switch in 2C-2 S3")]
impl ConfigSnapshot {
    pub(crate) fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }
    pub(crate) fn mode(&self) -> &RuntimeMode {
        &self.mode
    }
    pub(crate) fn writes_allowed(&self) -> bool {
        self.storage_ready && writes_allowed(&self.mode)
    }
}

impl fmt::Debug for ConfigSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        struct TextPresence(Option<usize>);
        impl fmt::Debug for TextPresence {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                match self.0 {
                    Some(len) => write!(f, "Some({len} bytes)"),
                    None => f.write_str("None"),
                }
            }
        }
        formatter
            .debug_struct("ConfigSnapshot")
            .field("mode", &self.mode)
            .field("text", &TextPresence(self.text.as_ref().map(String::len)))
            .field("storage_ready", &self.storage_ready)
            .finish()
    }
}

fn prepared(result: io::Result<bool>) -> bool {
    match result {
        Ok(_) => true,
        Err(error) => {
            eprintln!("settings storage could not be prepared: {:?}", error.kind());
            false
        }
    }
}

fn observe_folder(folder: io::Result<PathBuf>, storage_ready: bool) -> ConfigSnapshot {
    let result = folder.and_then(|path| ConfigDir::open(&path).map_err(io::Error::from));
    let dir = match result {
        Ok(dir) => dir,
        Err(error) => {
            let observation = observe(Err(error));
            return ConfigSnapshot {
                text: None,
                mode: effective_outcome(plan_startup(&observation, &ArtifactInventory::default())),
                storage_ready: false,
            };
        }
    };
    let mut inventory = ArtifactInventory::default();
    if let Ok(entries) = dir.entries() {
        for name in entries {
            if name == "config.toml.bak" {
                inventory.rolling_backup = true;
            }
            if let Some(parsed) = name.to_str().and_then(ProtectedBackupName::parse) {
                inventory.protected_backups.push(parsed);
            }
            if settings::is_stale_draft_name(&name) {
                inventory.leftover_drafts += 1;
            }
        }
    }
    let read = VerifiedFile::open(&dir, "config.toml".as_ref())
        .map_err(io::Error::from)
        .and_then(|mut file| settings::read_verified(&mut file));
    let (observation, text) = match read {
        Ok(text) => (observe(Ok(text.clone())), Some(text)),
        Err(error) => (observe(Err(error)), None),
    };
    ConfigSnapshot {
        text,
        mode: effective_outcome(plan_startup(&observation, &inventory)),
        storage_ready,
    }
}

#[allow(dead_code, reason = "instance bootstrap is used by isolated tests")]
pub(crate) fn bootstrap_at(base: &Path) -> ConfigSnapshot {
    let storage_ready = prepared(settings::prepare_storage_at(base));
    observe_folder(Ok(base.join("Islora")), storage_ready)
}

// make runs under the slot mutex and must not access the store.
fn install_once(
    slot: &Mutex<Option<Arc<ConfigSnapshot>>>,
    make: impl FnOnce() -> ConfigSnapshot,
) -> Arc<ConfigSnapshot> {
    let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(snapshot) = guard.as_ref() {
        return Arc::clone(snapshot);
    }
    let snapshot = Arc::new(make());
    *guard = Some(Arc::clone(&snapshot));
    snapshot
}

fn config_folder() -> io::Result<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|base| PathBuf::from(base).join("Islora"))
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
}

pub(crate) fn bootstrap() -> Arc<ConfigSnapshot> {
    install_once(&STORE, || {
        let storage_ready = prepared(settings::prepare_storage());
        let snapshot = observe_folder(config_folder(), storage_ready);
        eprintln!("settings startup mode: {:?}", snapshot.mode);
        snapshot
    })
}

#[allow(dead_code, reason = "readers switch in 2C-2 S3")]
pub(crate) fn snapshot() -> Option<Arc<ConfigSnapshot>> {
    STORE.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

fn text_in(slot: &Mutex<Option<Arc<ConfigSnapshot>>>) -> Option<String> {
    slot.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|snapshot| snapshot.text.clone())
}

pub(crate) fn text() -> Option<String> {
    text_in(&STORE)
}

fn refresh_folder(
    slot: &Mutex<Option<Arc<ConfigSnapshot>>>,
    folder: io::Result<PathBuf>,
) -> Arc<ConfigSnapshot> {
    let previous = slot.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let storage_ready = previous
        .as_ref()
        .is_some_and(|snapshot| snapshot.storage_ready);
    let snapshot = Arc::new(observe_folder(folder, storage_ready));
    if previous.is_some() {
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&snapshot));
    }
    snapshot
}

#[cfg(test)]
fn refresh_in(slot: &Mutex<Option<Arc<ConfigSnapshot>>>, base: &Path) -> Arc<ConfigSnapshot> {
    refresh_folder(slot, Ok(base.join("Islora")))
}

/// Refresh the installed snapshot; bootstrap must run first.
pub(crate) fn refresh() -> Arc<ConfigSnapshot> {
    refresh_folder(&STORE, config_folder())
}

#[cfg(test)]
mod tests {
    use super::*;
    use island_core::config_state::{ArtifactInventory, ProtectedBackupName, plan_startup};

    #[test]
    fn app_never_reads_config_directly() {
        let app = include_str!("../app.rs");
        let production = app.split("#[cfg(test)]").next().unwrap();
        assert!(!production.contains("settings::read("));
    }

    #[test]
    fn text_has_no_side_effects() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let slot = Mutex::new(None);
        install_once(&slot, || bootstrap_at(&base));
        let old = text_in(&slot).unwrap();
        std::fs::write(base.join("Islora/config.toml"), b"changed").unwrap();
        assert_eq!(text_in(&slot), Some(old));
    }

    #[test]
    fn refresh_reads_once_and_replaces_snapshot() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let slot = Mutex::new(None);
        let old = install_once(&slot, || bootstrap_at(&base));
        assert!(old.storage_ready && old.writes_allowed());
        let new_text = "config_version = \"1\"\n";
        std::fs::write(base.join("Islora/config.toml"), new_text).unwrap();
        let new = refresh_in(&slot, &base);
        assert!(new.storage_ready && new.writes_allowed());
        assert!(!Arc::ptr_eq(&old, &new));
        assert_eq!(text_in(&slot).as_deref(), Some(new_text));
    }

    #[test]
    fn refresh_keeps_storage_ready_false_after_prepare_failure() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("config.toml.bak"), b"invalid backup").unwrap();
        let slot = Mutex::new(None);
        install_once(&slot, || bootstrap_at(&base));
        std::fs::write(folder.join("config.toml"), b"config_version = \"1\"\n").unwrap();
        let snapshot = refresh_in(&slot, &base);
        assert!(!snapshot.storage_ready && !snapshot.writes_allowed());
    }

    #[test]
    fn refresh_failed_read_gives_none_text() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let slot = Mutex::new(None);
        install_once(&slot, || bootstrap_at(&base));
        std::fs::write(
            base.join("Islora/config.toml"),
            vec![b'x'; island_core::migration::MAX_CONFIG_BYTES + 1],
        )
        .unwrap();
        refresh_in(&slot, &base);
        assert!(text_in(&slot).is_none());
    }

    #[test]
    fn refresh_on_empty_slot_is_not_ready() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        bootstrap_at(&base);
        let slot = Mutex::new(None);
        let snapshot = refresh_in(&slot, &base);
        assert!(!snapshot.storage_ready);
        assert!(snapshot.text.is_some());
        assert!(
            slot.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_none()
        );
    }

    #[test]
    fn folder_open_failure_clears_storage_ready() {
        let base = settings::tests::temp_base();
        let file = base.join("not_a_folder");
        std::fs::write(&file, b"file").unwrap();
        let snapshot = observe_folder(Ok(file.clone()), true);
        assert!(!snapshot.storage_ready && !snapshot.writes_allowed());
        let slot = Mutex::new(Some(Arc::new(ConfigSnapshot {
            text: None,
            mode: RuntimeMode::Normal,
            storage_ready: true,
        })));
        let refreshed = refresh_in(&slot, &file);
        assert!(!refreshed.storage_ready && !refreshed.writes_allowed());
    }

    fn sample() -> ConfigSnapshot {
        ConfigSnapshot {
            text: None,
            mode: RuntimeMode::Normal,
            storage_ready: true,
        }
    }

    #[test]
    fn bootstrap_new_install_is_ready() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let snap = bootstrap_at(&base);
        assert_eq!(snap.mode(), &RuntimeMode::Normal);
        assert!(snap.storage_ready && snap.writes_allowed());
        assert!(snap.text().is_some());
    }

    #[test]
    fn bootstrap_v1_is_legacy_fallback_bytes_unchanged() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("config.toml");
        let bytes = b"config_version = \"1\"\n";
        std::fs::write(&path, bytes).unwrap();
        let snap = bootstrap_at(&base);
        assert_eq!(snap.mode(), &RuntimeMode::LegacyCompatible);
        assert!(snap.writes_allowed());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn bootstrap_future_is_read_only_bytes_unchanged() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("config.toml");
        let bytes = b"config_version = \"100\"\n";
        std::fs::write(&path, bytes).unwrap();
        let snap = bootstrap_at(&base);
        assert_eq!(snap.mode(), &RuntimeMode::ReadOnly);
        assert!(!snap.writes_allowed());
        assert!(snap.text().is_some());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn bootstrap_unreadable_gives_none_text() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(
            folder.join("config.toml"),
            vec![b'x'; island_core::migration::MAX_CONFIG_BYTES + 1],
        )
        .unwrap();
        let snap = bootstrap_at(&base);
        assert_eq!(snap.mode(), &RuntimeMode::LegacyCompatible);
        assert!(snap.text().is_none());
    }

    #[test]
    fn bootstrap_prepare_failure_refuses_writes() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("config.toml.bak");
        let bytes = b"invalid backup";
        std::fs::write(&path, bytes).unwrap();
        let snap = bootstrap_at(&base);
        assert!(!snap.storage_ready);
        assert!(!snap.writes_allowed());
        assert_eq!(snap.mode(), &RuntimeMode::Recovery);
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }

    #[test]
    fn bootstrap_missing_primary_with_protected_backup_records_current_behaviour() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        let name = ProtectedBackupName::new(1, 1, ChangeFingerprint::of(b"backup"))
            .unwrap()
            .file_name();
        let path = folder.join(name);
        std::fs::write(&path, b"backup").unwrap();
        // 2C-3: preparation must treat protected backups as recovery evidence
        let snap = bootstrap_at(&base);
        assert!(snap.storage_ready && snap.writes_allowed());
        assert_eq!(snap.mode(), &RuntimeMode::Normal);
        assert!(snap.text().is_some());
        assert_eq!(std::fs::read(path).unwrap(), b"backup");
    }

    #[test]
    fn bootstrap_twice_is_noop() {
        let slot = Mutex::new(None);
        let count = std::cell::Cell::new(0);
        let make = || {
            count.set(count.get() + 1);
            sample()
        };
        let first = install_once(&slot, make);
        let second = install_once(&slot, make);
        assert_eq!(count.get(), 1);
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn install_once_recovers_from_poison() {
        let slot = Mutex::new(None);
        std::thread::scope(|scope| {
            let result = scope
                .spawn(|| {
                    let _guard = slot.lock().unwrap();
                    panic!("poison local slot");
                })
                .join();
            assert!(result.is_err());
        });
        assert_eq!(install_once(&slot, sample).mode(), &RuntimeMode::Normal);
    }

    #[test]
    fn snapshot_debug_hides_text() {
        let snap = ConfigSnapshot {
            text: Some("https://example.invalid/secret.ics".into()),
            ..sample()
        };
        let debug = format!("{snap:?}");
        assert!(!debug.contains("secret") && !debug.contains("https"));
        assert!(debug.contains("Some(34 bytes)"));
    }

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
