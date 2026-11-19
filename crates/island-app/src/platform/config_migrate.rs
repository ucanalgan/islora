//! Read-only preflight for a migration under the held writer lock.

use std::{io, path::Path};

use island_core::config_state::{
    FailureStage, PrimaryObservation, ProtectedBackupName, retry_decision,
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
mod tests {
    use super::*;
    use island_core::config_state::{ChangeFingerprint, FailureRecord};
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
                } => {
                    assert_eq!(text, LEGACY);
                    assert_eq!(schema, 1);
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
}
