//! Read-only preflight for a migration under the held writer lock.

use std::{io, path::Path};

use island_core::config_state::{
    ChangeFingerprint, FailureRecord, FailureStage, PrimaryObservation, ProtectedBackupName,
    StoredFailure, retry_decision,
};
use island_core::migration::{MigrationOutcome, prepare};

use super::config_fs::{ConfigFsError, VerifiedFile, WriterLock};
use super::{config_commit, config_store, settings};

// Used by migration from 2C-3 S4b/S6.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum MigrationReport {
    NotNeeded,
    #[allow(dead_code, reason = "constructed by migration in 2C-3 S4b")]
    Migrated {
        backup: ProtectedBackupName,
    },
    Fallback(FallbackReason),
}

// Used by migration from 2C-3 S4b/S6.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FallbackReason {
    #[allow(dead_code, reason = "constructed by bootstrap in 2C-3 S6")]
    Busy,
    #[allow(dead_code, reason = "constructed by bootstrap in 2C-3 S6")]
    Unprepared,
    SourceChanged,
    Suppressed,
    Unpreparable,
    #[allow(dead_code, reason = "constructed by migration in 2C-3 S4b")]
    Failed(FailureStage),
}

// Used by migration from 2C-3 S4b/S6.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
enum Preflight {
    Ready {
        text: String,
        prepared: String,
        schema: u32,
        fingerprint: ChangeFingerprint,
        previous: Option<FailureRecord>,
    },
    Done(MigrationReport),
}

// Used by migration from 2C-3 S4b/S6.
#[cfg_attr(not(test), allow(dead_code))]
fn preflight(lock: &WriterLock, path: &Path, bytes: &[u8]) -> io::Result<Preflight> {
    let mut file =
        VerifiedFile::open(lock.directory(), settings::name(path)?).map_err(io::Error::from)?;
    match file.read_bounded(island_core::migration::MAX_CONFIG_BYTES) {
        Ok(actual) if actual == bytes => {}
        Ok(_) | Err(ConfigFsError::TooLarge) => {
            return Ok(Preflight::Done(MigrationReport::Fallback(
                FallbackReason::SourceChanged,
            )));
        }
        Err(error) => return Err(error.into()),
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Ok(Preflight::Done(MigrationReport::NotNeeded));
    };
    let PrimaryObservation::Legacy {
        schema,
        fingerprint,
    } = config_store::observe(Ok(text.to_owned()))
    else {
        return Ok(Preflight::Done(MigrationReport::NotNeeded));
    };
    let stored = config_commit::read_failure(lock.directory());
    if !retry_decision(&stored, fingerprint).allows_attempt() {
        return Ok(Preflight::Done(MigrationReport::Fallback(
            FallbackReason::Suppressed,
        )));
    }
    match prepare(text) {
        Ok(MigrationOutcome::Prepared { text: prepared }) => {
            if !matches!(prepare(&prepared), Ok(MigrationOutcome::AlreadyCurrent)) {
                return Ok(Preflight::Done(MigrationReport::Fallback(
                    FallbackReason::Unpreparable,
                )));
            }
            Ok(Preflight::Ready {
                text: text.to_owned(),
                prepared,
                schema,
                fingerprint,
                previous: match stored {
                    StoredFailure::Valid(record) => Some(record),
                    StoredFailure::None | StoredFailure::Corrupt => None,
                },
            })
        }
        Ok(MigrationOutcome::AlreadyCurrent | MigrationOutcome::Future { .. }) => {
            Ok(Preflight::Done(MigrationReport::NotNeeded))
        }
        Err(_) => Ok(Preflight::Done(MigrationReport::Fallback(
            FallbackReason::Unpreparable,
        ))),
    }
}

#[cfg(test)]
thread_local! {
    static AFTER_BACKUP: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::new(None);
}

// Called from bootstrap in 2C-3 S6.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn migrate_locked(
    lock: &WriterLock,
    path: &Path,
    bytes: &[u8],
) -> io::Result<MigrationReport> {
    let (text, prepared, schema, fingerprint, previous) = match preflight(lock, path, bytes)? {
        Preflight::Ready {
            text,
            prepared,
            schema,
            fingerprint,
            previous,
        } => (text, prepared, schema, fingerprint, previous),
        Preflight::Done(report) => return Ok(report),
    };
    let record_failure = |stage| {
        let record = FailureRecord {
            source: fingerprint,
            from_schema: schema,
            stage,
            attempts: previous
                .as_ref()
                .map_or(1, |record| record.attempts.saturating_add(1)),
        };
        if let Err(error) = config_commit::write_failure(lock.directory(), &record) {
            eprintln!("migration failure record write failed: {:?}", error.kind());
        }
    };
    let backup = match config_commit::create_protected_backup(lock.directory(), &text, schema) {
        Ok(backup) => backup,
        Err(error) => {
            eprintln!("migration backup failed: {:?}", error.kind());
            record_failure(FailureStage::Backup);
            return Ok(MigrationReport::Fallback(FallbackReason::Failed(
                FailureStage::Backup,
            )));
        }
    };
    #[cfg(test)]
    AFTER_BACKUP.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    if let Err(error) =
        config_commit::commit_primary(lock.directory(), path, &prepared, Some(bytes))
    {
        let reason = if config_commit::is_primary_changed(&error) {
            FallbackReason::SourceChanged
        } else if config_commit::is_commit_unverified(&error) {
            FallbackReason::Failed(FailureStage::Verify)
        } else {
            FallbackReason::Failed(FailureStage::Commit)
        };
        eprintln!("migration commit failed: {:?}", error.kind());
        if let FallbackReason::Failed(stage) = reason {
            record_failure(stage);
        }
        return Ok(MigrationReport::Fallback(reason));
    }
    if let Err(error) = config_commit::clear_failure(lock.directory()) {
        eprintln!(
            "migration failure record cleanup failed: {:?}",
            error.kind()
        );
    }
    if let Err(error) = config_commit::apply_protected_retention(lock.directory(), &backup, 3) {
        eprintln!("migration backup retention failed: {:?}", error.kind());
    }
    Ok(MigrationReport::Migrated { backup })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::BTreeMap,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);
    const LEGACY: &str = "ui_density = \"minimal\"\n";

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "islora-migrate-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            if let Ok(entries) = std::fs::read_dir(&self.0) {
                for entry in entries.flatten() {
                    if let Ok(metadata) = entry.metadata() {
                        let mut permissions = metadata.permissions();
                        // Windows-only module: remove the read-only attribute from test files.
                        #[allow(clippy::permissions_set_readonly_false)]
                        permissions.set_readonly(false);
                        let _ = std::fs::set_permissions(entry.path(), permissions);
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let name = entry.file_name().to_string_lossy().into_owned();
                let bytes = if name == "config.lock" {
                    Vec::new()
                } else {
                    std::fs::read(entry.path()).unwrap()
                };
                (name, bytes)
            })
            .collect()
    }

    fn run_case(
        source: &[u8],
        expected: &[u8],
        record: Option<&[u8]>,
        check: impl FnOnce(Preflight),
    ) {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let path = temp.0.join("config.toml");
        std::fs::write(&path, source).unwrap();
        if let Some(record) = record {
            std::fs::write(temp.0.join(config_commit::FAILURE_RECORD_NAME), record).unwrap();
        }
        let lock = WriterLock::acquire(&temp.0).unwrap();
        let before = snapshot(&temp.0);
        check(preflight(&lock, &path, expected).unwrap());
        assert_eq!(before, snapshot(&temp.0));
    }

    fn record(source: &[u8]) -> String {
        FailureRecord {
            source: ChangeFingerprint::of(source),
            from_schema: 1,
            stage: FailureStage::Commit,
            attempts: 1,
        }
        .encode()
    }

    #[test]
    fn byte_mismatch_aborts_without_record() {
        run_case(
            LEGACY.as_bytes(),
            b"different",
            Some(b"existing"),
            |result| {
                assert_eq!(
                    result,
                    Preflight::Done(MigrationReport::Fallback(FallbackReason::SourceChanged))
                )
            },
        );
    }
    #[test]
    fn oversized_source_aborts_without_writes() {
        let source = vec![b'x'; island_core::migration::MAX_CONFIG_BYTES + 1];
        run_case(&source, LEGACY.as_bytes(), Some(b"existing"), |result| {
            assert_eq!(
                result,
                Preflight::Done(MigrationReport::Fallback(FallbackReason::SourceChanged))
            );
        });
    }

    #[test]
    fn missing_source_returns_error_without_writes() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let path = temp.0.join("config.toml");
        std::fs::write(temp.0.join(config_commit::FAILURE_RECORD_NAME), b"existing").unwrap();
        let lock = WriterLock::acquire(&temp.0).unwrap();
        let before = snapshot(&temp.0);
        assert_eq!(
            preflight(&lock, &path, LEGACY.as_bytes())
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(before, snapshot(&temp.0));
    }
    #[test]
    fn same_source_record_suppresses() {
        run_case(
            LEGACY.as_bytes(),
            LEGACY.as_bytes(),
            Some(record(LEGACY.as_bytes()).as_bytes()),
            |result| {
                assert_eq!(
                    result,
                    Preflight::Done(MigrationReport::Fallback(FallbackReason::Suppressed))
                )
            },
        );
    }
    #[test]
    fn corrupt_record_suppresses() {
        run_case(
            LEGACY.as_bytes(),
            LEGACY.as_bytes(),
            Some(b"broken"),
            |result| {
                assert_eq!(
                    result,
                    Preflight::Done(MigrationReport::Fallback(FallbackReason::Suppressed))
                )
            },
        );
    }
    #[test]
    fn unpreparable_legacy_falls_back_without_write() {
        let source = b"ui_density = \"minimal\"\n[unsupported]\n";
        run_case(source, source, None, |result| {
            assert_eq!(
                result,
                Preflight::Done(MigrationReport::Fallback(FallbackReason::Unpreparable))
            )
        });
    }
    #[test]
    fn changed_source_record_allows_preflight() {
        run_case(
            LEGACY.as_bytes(),
            LEGACY.as_bytes(),
            Some(record(b"other").as_bytes()),
            |result| match result {
                Preflight::Ready {
                    text,
                    prepared,
                    schema,
                    fingerprint,
                    previous,
                } => {
                    assert_eq!(text, LEGACY);
                    assert_eq!(schema, 1);
                    assert_eq!(fingerprint, ChangeFingerprint::of(LEGACY.as_bytes()));
                    assert_eq!(previous.unwrap().source, ChangeFingerprint::of(b"other"));
                    assert_eq!(prepare(&prepared), Ok(MigrationOutcome::AlreadyCurrent));
                }
                other => panic!("unexpected: {other:?}"),
            },
        );
    }
    #[test]
    fn nonlegacy_sources_are_not_prepared() {
        for source in [
            &b"config_version = \"2\"\n"[..],
            &b"config_version = \"999\"\n"[..],
            &b"config_version = nope\n"[..],
            &b"\xff"[..],
        ] {
            run_case(source, source, None, |result| {
                assert_eq!(result, Preflight::Done(MigrationReport::NotNeeded))
            });
        }
    }

    #[test]
    fn legacy_migration_preserves_exact_backup_and_writes_prepared_bytes() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for input in [
            "ui_density = \"minimal\"\n",
            "ui_density = \"balanced\"\n",
            "ui_density = \"detailed\"\n",
            "ui_density = \"minimal\"\nindicator_style = \"context\"\n",
        ] {
            let temp = Temp::new();
            let path = temp.0.join("config.toml");
            std::fs::write(&path, input).unwrap();
            std::fs::write(
                temp.0.join(config_commit::FAILURE_RECORD_NAME),
                record(b"older"),
            )
            .unwrap();
            let lock = WriterLock::acquire(&temp.0).unwrap();
            let expected = match prepare(input).unwrap() {
                MigrationOutcome::Prepared { text } => text,
                other => panic!("unexpected: {other:?}"),
            };
            let MigrationReport::Migrated { backup } =
                migrate_locked(&lock, &path, input.as_bytes()).unwrap()
            else {
                panic!("expected migration");
            };
            assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
            let (indicator, media) = if input.contains("balanced") {
                ("edge", "standard")
            } else if input.contains("detailed") {
                ("context", "informative")
            } else if input.contains("indicator_style") {
                ("context", "compact")
            } else {
                ("ring", "compact")
            };
            assert_eq!(
                island_core::config::read_value(&expected, "indicator_style").as_deref(),
                Some(indicator)
            );
            assert_eq!(
                island_core::config::read_value(&expected, "media_layout").as_deref(),
                Some(media)
            );
            let backup_path = temp.0.join(backup.file_name());
            assert_eq!(std::fs::read(&backup_path).unwrap(), input.as_bytes());
            assert!(
                std::fs::metadata(backup_path)
                    .unwrap()
                    .permissions()
                    .readonly()
            );
            assert!(!temp.0.join(config_commit::FAILURE_RECORD_NAME).exists());
        }
    }

    #[test]
    fn clear_failure_error_keeps_verified_migration() {
        use std::os::windows::fs::OpenOptionsExt;
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let path = temp.0.join("config.toml");
        let failure_path = temp.0.join(config_commit::FAILURE_RECORD_NAME);
        let old_record = record(b"older");
        std::fs::write(&path, LEGACY).unwrap();
        std::fs::write(&failure_path, &old_record).unwrap();
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 2)
            .open(&failure_path)
            .unwrap();
        let lock = WriterLock::acquire(&temp.0).unwrap();
        let MigrationOutcome::Prepared { text: expected } = prepare(LEGACY).unwrap() else {
            panic!("expected prepared configuration");
        };
        let MigrationReport::Migrated { backup } =
            migrate_locked(&lock, &path, LEGACY.as_bytes()).unwrap()
        else {
            panic!("expected migration despite cleanup failure");
        };
        assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
        let backup_path = temp.0.join(backup.file_name());
        assert_eq!(std::fs::read(&backup_path).unwrap(), LEGACY.as_bytes());
        assert!(
            std::fs::metadata(backup_path)
                .unwrap()
                .permissions()
                .readonly()
        );
        assert_eq!(std::fs::read(&failure_path).unwrap(), old_record.as_bytes());
        drop(held);
    }

    #[test]
    fn retention_error_keeps_verified_migration() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let path = temp.0.join("config.toml");
        std::fs::write(&path, LEGACY).unwrap();
        let lock = WriterLock::acquire(&temp.0).unwrap();
        let old: Vec<_> = (0..3)
            .map(|i| {
                config_commit::create_protected_backup(lock.directory(), &format!("old {i}"), 1)
                    .unwrap()
            })
            .collect();
        super::super::config_fs::FAIL_DELETE_DISPOSITION.with(|flag| flag.set(true));
        let MigrationOutcome::Prepared { text: expected } = prepare(LEGACY).unwrap() else {
            panic!("expected prepared configuration");
        };
        let MigrationReport::Migrated { backup } =
            migrate_locked(&lock, &path, LEGACY.as_bytes()).unwrap()
        else {
            panic!("expected migration despite retention failure");
        };
        assert!(!super::super::config_fs::FAIL_DELETE_DISPOSITION.with(|flag| flag.get()));
        assert_eq!(std::fs::read(&path).unwrap(), expected.as_bytes());
        let backup_path = temp.0.join(backup.file_name());
        assert_eq!(std::fs::read(&backup_path).unwrap(), LEGACY.as_bytes());
        assert!(
            std::fs::metadata(backup_path)
                .unwrap()
                .permissions()
                .readonly()
        );
        for name in old {
            assert!(temp.0.join(name.file_name()).exists());
        }
    }

    #[test]
    fn migration_refuses_changed_source_before_creating_backup() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let path = temp.0.join("config.toml");
        std::fs::write(&path, LEGACY).unwrap();
        let lock = WriterLock::acquire(&temp.0).unwrap();
        let before = snapshot(&temp.0);
        assert_eq!(
            migrate_locked(&lock, &path, b"older").unwrap(),
            MigrationReport::Fallback(FallbackReason::SourceChanged)
        );
        assert_eq!(snapshot(&temp.0), before);
    }

    #[test]
    fn unverified_commit_keeps_original_protected_backup_and_records_verify() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let path = temp.0.join("config.toml");
        std::fs::write(&path, LEGACY).unwrap();
        let old_record = record(b"older");
        std::fs::write(temp.0.join(config_commit::FAILURE_RECORD_NAME), &old_record).unwrap();
        let lock = WriterLock::acquire(&temp.0).unwrap();
        let changed = path.clone();
        config_commit::AFTER_PRIMARY_RENAME.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(changed, b"external").unwrap()
            }));
        });
        assert_eq!(
            migrate_locked(&lock, &path, LEGACY.as_bytes()).unwrap(),
            MigrationReport::Fallback(FallbackReason::Failed(FailureStage::Verify))
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"external");
        assert_eq!(
            config_commit::read_failure(lock.directory()),
            StoredFailure::Valid(FailureRecord {
                source: ChangeFingerprint::of(LEGACY.as_bytes()),
                from_schema: 1,
                stage: FailureStage::Verify,
                attempts: 2,
            })
        );
        let backups: Vec<_> = std::fs::read_dir(&temp.0)
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(ProtectedBackupName::parse)
                    .is_some()
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(backups[0].path()).unwrap(), LEGACY.as_bytes());
    }

    #[test]
    fn failed_backup_or_commit_preserves_primary_and_records_failure() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for (fail_at, stage, expected_backups) in
            [(1, FailureStage::Backup, 0), (2, FailureStage::Commit, 1)]
        {
            let temp = Temp::new();
            let path = temp.0.join("config.toml");
            std::fs::write(&path, LEGACY).unwrap();
            let old_record = record(b"older");
            std::fs::write(temp.0.join(config_commit::FAILURE_RECORD_NAME), &old_record).unwrap();
            let lock = WriterLock::acquire(&temp.0).unwrap();
            config_commit::FAIL_VERIFICATION_AT.with(|count| count.set(fail_at));
            assert_eq!(
                migrate_locked(&lock, &path, LEGACY.as_bytes()).unwrap(),
                MigrationReport::Fallback(FallbackReason::Failed(stage))
            );
            assert_eq!(std::fs::read(&path).unwrap(), LEGACY.as_bytes());
            assert_eq!(
                config_commit::read_failure(lock.directory()),
                StoredFailure::Valid(FailureRecord {
                    source: ChangeFingerprint::of(LEGACY.as_bytes()),
                    from_schema: 1,
                    stage,
                    attempts: 2,
                })
            );
            let backups: Vec<_> = std::fs::read_dir(&temp.0)
                .unwrap()
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .and_then(ProtectedBackupName::parse)
                        .is_some()
                })
                .collect();
            assert_eq!(backups.len(), expected_backups);
            for backup in backups {
                assert_eq!(std::fs::read(backup.path()).unwrap(), LEGACY.as_bytes());
            }
        }
    }

    #[test]
    fn source_changed_after_backup_is_not_overwritten() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let path = temp.0.join("config.toml");
        std::fs::write(&path, LEGACY).unwrap();
        let old_record = record(b"older");
        std::fs::write(temp.0.join(config_commit::FAILURE_RECORD_NAME), &old_record).unwrap();
        let lock = WriterLock::acquire(&temp.0).unwrap();
        let changed = path.clone();
        AFTER_BACKUP.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(changed, b"external").unwrap()
            }))
        });
        assert_eq!(
            migrate_locked(&lock, &path, LEGACY.as_bytes()).unwrap(),
            MigrationReport::Fallback(FallbackReason::SourceChanged)
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"external");
        assert_eq!(
            std::fs::read(temp.0.join(config_commit::FAILURE_RECORD_NAME)).unwrap(),
            old_record.as_bytes()
        );
        let backups: Vec<_> = std::fs::read_dir(&temp.0)
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(ProtectedBackupName::parse)
                    .is_some()
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(backups[0].path()).unwrap(), LEGACY.as_bytes());
    }

    #[test]
    fn failure_attempts_start_at_one_and_saturate() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        for previous in [None, Some(u32::MAX)] {
            let temp = Temp::new();
            let path = temp.0.join("config.toml");
            std::fs::write(&path, LEGACY).unwrap();
            if let Some(attempts) = previous {
                let old = FailureRecord {
                    source: ChangeFingerprint::of(b"older"),
                    from_schema: 1,
                    stage: FailureStage::Commit,
                    attempts,
                };
                std::fs::write(
                    temp.0.join(config_commit::FAILURE_RECORD_NAME),
                    old.encode(),
                )
                .unwrap();
            }
            let lock = WriterLock::acquire(&temp.0).unwrap();
            config_commit::FAIL_VERIFICATION_AT.with(|count| count.set(1));
            assert_eq!(
                migrate_locked(&lock, &path, LEGACY.as_bytes()).unwrap(),
                MigrationReport::Fallback(FallbackReason::Failed(FailureStage::Backup))
            );
            assert_eq!(
                config_commit::read_failure(lock.directory()),
                StoredFailure::Valid(FailureRecord {
                    source: ChangeFingerprint::of(LEGACY.as_bytes()),
                    from_schema: 1,
                    stage: FailureStage::Backup,
                    attempts: previous.unwrap_or(1),
                })
            );
        }
    }

    #[test]
    fn failure_record_write_error_preserves_verify_report_and_old_record() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let temp = Temp::new();
        let path = temp.0.join("config.toml");
        std::fs::write(&path, LEGACY).unwrap();
        let old_record = record(b"older");
        std::fs::write(temp.0.join(config_commit::FAILURE_RECORD_NAME), &old_record).unwrap();
        let lock = WriterLock::acquire(&temp.0).unwrap();
        let changed = path.clone();
        config_commit::AFTER_PRIMARY_RENAME.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(changed, b"external").unwrap();
                config_commit::FAIL_VERIFICATION_AT.with(|count| count.set(1));
            }));
        });
        assert_eq!(
            migrate_locked(&lock, &path, LEGACY.as_bytes()).unwrap(),
            MigrationReport::Fallback(FallbackReason::Failed(FailureStage::Verify))
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"external");
        assert_eq!(
            std::fs::read(temp.0.join(config_commit::FAILURE_RECORD_NAME)).unwrap(),
            old_record.as_bytes()
        );
        let backups: Vec<_> = std::fs::read_dir(&temp.0)
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .and_then(ProtectedBackupName::parse)
                    .is_some()
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(backups[0].path()).unwrap(), LEGACY.as_bytes());
    }
}
