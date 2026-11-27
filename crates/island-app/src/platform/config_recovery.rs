//! Recovery artifacts next to the primary configuration and, from 2D-2 S3 on, the candidates for a restore.

use std::{ffi::OsStr, io};

use island_core::config_state::{
    ArtifactInventory, CandidateSource, CandidateValidity, ChangeFingerprint, ProtectedBackupName,
    RecoveryCandidate, StoredFailure,
};
use island_core::migration::{self, MigrationError, MigrationOutcome};

use super::{
    config_commit,
    config_fs::{ConfigDir, ConfigFsError, VerifiedFile},
    settings,
};

const ROLLING_BACKUP_NAME: &str = "config.toml.bak";

/// What startup or a reload found for the primary after recovery ran.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryOutcome {
    /// The primary already existed; nothing was listed or written.
    Present,
    /// The primary was missing and was restored from a backup of this schema.
    Restored { schema: u32 },
    /// The primary remains missing; evidence excludes stale drafts.
    Missing { evidence: bool },
}

pub(crate) fn has_evidence(inventory: &ArtifactInventory) -> bool {
    let mut without_drafts = inventory.clone();
    without_drafts.leftover_drafts = 0;
    without_drafts.has_recovery_artifacts()
}

/// The caller holds the WriterLock and has already swept stale drafts.
pub(crate) fn recover_locked(dir: &ConfigDir) -> io::Result<RecoveryOutcome> {
    recover_locked_with_hook(
        dir,
        #[cfg(test)]
        || {},
    )
}

pub(crate) fn recover_locked_with_hook(
    dir: &ConfigDir,
    #[cfg(test)] hook: impl FnOnce(),
) -> io::Result<RecoveryOutcome> {
    use island_core::config_state::{RecoveryPlan, plan_recovery};
    if settings::inspect_in(dir, settings::PRIMARY_NAME)? {
        return Ok(RecoveryOutcome::Present);
    }
    let inventory = inventory(dir)?;
    let candidates = candidates(dir, &inventory);
    match plan_recovery(&inventory, &candidates) {
        RecoveryPlan::Blocked => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration recovery artifacts need review",
        )),
        RecoveryPlan::Restore {
            source,
            schema,
            fingerprint,
        } => {
            let name = match source {
                CandidateSource::RollingBackup => ROLLING_BACKUP_NAME.to_owned(),
                CandidateSource::Protected(name) => name.file_name(),
            };
            #[cfg(test)]
            config_commit::restore_primary_with_hook(dir, OsStr::new(&name), fingerprint, hook)?;
            #[cfg(not(test))]
            config_commit::restore_primary(dir, OsStr::new(&name), fingerprint)?;
            if schema == u32::try_from(migration::CURRENT_VERSION).unwrap_or(u32::MAX)
                && let Err(error) = config_commit::clear_failure(dir)
            {
                eprintln!("recovery failure record cleanup failed: {:?}", error.kind());
            }
            Ok(RecoveryOutcome::Restored { schema })
        }
        RecoveryPlan::NoneValid | RecoveryPlan::NothingToRestore => Ok(RecoveryOutcome::Missing {
            evidence: has_evidence(&inventory),
        }),
    }
}

/// Classify original bytes with the primary rule, rejecting candidates with no
/// non-comment, non-blank line retained by the unsupported-line filter. A
/// supported line is any key = value line accepted by the core parser on its
/// own, including non-critical keys; `foo = "x"` alone is valid schema 1.
fn classify_candidate(bytes: &[u8]) -> CandidateValidity {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return CandidateValidity::Invalid;
    };
    let has_supported_line = text.split_inclusive('\n').enumerate().any(|(index, line)| {
        let line = if index == 0 {
            line.strip_prefix('\u{feff}').unwrap_or(line)
        } else {
            line
        };
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return false;
        }
        // Only the original first line may have a BOM; mirror the primary filter.
        let protected;
        let candidate = if index > 0 && line.starts_with('\u{feff}') {
            protected = format!(" {line}");
            protected.as_str()
        } else {
            line
        };
        !matches!(
            migration::prepare(candidate),
            Err(MigrationError::UnsupportedFormat { .. })
        )
    });
    if !has_supported_line {
        return CandidateValidity::Invalid;
    }
    let fingerprint = ChangeFingerprint::of(bytes);
    match settings::classify_ignoring_unsupported(text) {
        Ok(MigrationOutcome::AlreadyCurrent) => CandidateValidity::Valid {
            schema: u32::try_from(migration::CURRENT_VERSION).unwrap_or(u32::MAX),
            fingerprint,
        },
        Ok(MigrationOutcome::Prepared { .. }) => CandidateValidity::Valid {
            schema: 1,
            fingerprint,
        },
        Ok(MigrationOutcome::Future { .. }) => CandidateValidity::Future,
        Err(MigrationError::InputTooLarge)
        | Err(MigrationError::OutputTooLarge)
        | Err(MigrationError::DuplicateCriticalKey(_))
        | Err(MigrationError::MalformedCriticalSetting(_))
        | Err(MigrationError::InvalidSchemaVersion)
        | Err(MigrationError::InvalidLegacyValue)
        | Err(MigrationError::InvalidModernValue(_))
        | Err(MigrationError::UnsupportedFormat { .. })
        | Err(MigrationError::InvalidOutput) => CandidateValidity::Invalid,
    }
}

/// Read a candidate without changing it (a read-only protected backup opens as a source).
fn read_candidate(dir: &ConfigDir, name: &OsStr) -> CandidateValidity {
    match VerifiedFile::open_source(dir, name)
        .and_then(|mut file| file.read_bounded(migration::MAX_CONFIG_BYTES))
    {
        Ok(bytes) => classify_candidate(&bytes),
        Err(ConfigFsError::TooLarge) => CandidateValidity::Invalid,
        Err(
            ConfigFsError::Locked
            | ConfigFsError::ReparsePoint
            | ConfigFsError::NotADirectory
            | ConfigFsError::IsDirectory
            | ConfigFsError::ExtraHardLinks
            | ConfigFsError::OutsideDirectory
            | ConfigFsError::InvalidName
            | ConfigFsError::Io(_),
        ) => CandidateValidity::Unreadable,
    }
}

/// Observe every restore candidate, rolling backup first. Protected validity is
/// not filtered here; `plan_recovery` applies `protected_validity`.
pub(crate) fn candidates(dir: &ConfigDir, inventory: &ArtifactInventory) -> Vec<RecoveryCandidate> {
    let mut found = Vec::new();
    if inventory.rolling_backup {
        found.push(RecoveryCandidate {
            source: CandidateSource::RollingBackup,
            validity: read_candidate(dir, OsStr::new(ROLLING_BACKUP_NAME)),
        });
    }
    for name in &inventory.protected_backups {
        found.push(RecoveryCandidate {
            source: CandidateSource::Protected(name.clone()),
            validity: read_candidate(dir, OsStr::new(&name.file_name())),
        });
    }
    found
}

pub(crate) fn inventory(dir: &ConfigDir) -> io::Result<ArtifactInventory> {
    let mut inventory = ArtifactInventory::default();
    for name in dir.entries().map_err(io::Error::from)? {
        let Some(original) = name.to_str() else {
            inventory.other_evidence = true;
            inventory.ambiguous_leftovers = true;
            continue;
        };
        let lower = original.to_ascii_lowercase();
        if lower == ROLLING_BACKUP_NAME {
            inventory.rolling_backup = true;
        }
        if matches!(
            lower.as_str(),
            "config.toml.bak.new" | "config.toml.recover"
        ) || (lower.starts_with("config.pre-migration") && lower.ends_with(".bak"))
        {
            inventory.ambiguous_leftovers = true;
        }
        if lower == "config.toml.new" {
            inventory.other_evidence = true;
        }
        if lower == config_commit::FAILURE_RECORD_NAME {
            inventory.failure_record_present = true;
        }
        if settings::is_stale_draft_name(&name) {
            inventory.leftover_drafts += 1;
        }
        if let Some(backup) = ProtectedBackupName::parse(original) {
            inventory.protected_backups.push(backup);
        }
    }
    if let StoredFailure::Valid(record) = config_commit::read_failure(dir) {
        inventory.recorded_failure = Some(record.source);
    }
    Ok(inventory)
}

/// For observation paths that cannot fail: an unreadable folder listing counts as
/// evidence that blocks any restore, never as an empty folder.
pub(crate) fn inventory_or_closed(dir: &ConfigDir) -> ArtifactInventory {
    close_on_error(inventory(dir))
}

fn close_on_error(result: io::Result<ArtifactInventory>) -> ArtifactInventory {
    result.unwrap_or_else(|_| closed())
}

pub(crate) fn closed() -> ArtifactInventory {
    ArtifactInventory {
        other_evidence: true,
        ambiguous_leftovers: true,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use island_core::config_state::{ChangeFingerprint, FailureRecord, FailureStage};
    use std::fs;

    struct ClearReadonly(std::path::PathBuf);

    impl Drop for ClearReadonly {
        fn drop(&mut self) {
            if let Ok(metadata) = fs::metadata(&self.0) {
                let mut permissions = metadata.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                permissions.set_readonly(false);
                let _ = fs::set_permissions(&self.0, permissions);
            }
        }
    }

    fn folder() -> (settings::tests::Temp, ConfigDir) {
        let path = settings::tests::temp_base();
        let dir = ConfigDir::open(&path).unwrap();
        (path, dir)
    }

    #[test]
    fn recover_locked_present_primary_lists_nothing() {
        let (path, dir) = folder();
        fs::write(path.join(settings::PRIMARY_NAME), b"existing").unwrap();
        fs::write(path.join("config.toml.recover"), b"ambiguous").unwrap();
        assert_eq!(recover_locked(&dir).unwrap(), RecoveryOutcome::Present);
    }

    #[test]
    fn recover_locked_missing_reports_evidence() {
        let (path, dir) = folder();
        assert_eq!(
            recover_locked(&dir).unwrap(),
            RecoveryOutcome::Missing { evidence: false }
        );
        fs::write(path.join("config.toml.1-2-3.tmp"), b"draft").unwrap();
        assert_eq!(
            recover_locked(&dir).unwrap(),
            RecoveryOutcome::Missing { evidence: false }
        );
        fs::write(path.join(config_commit::FAILURE_RECORD_NAME), b"failure").unwrap();
        assert_eq!(
            recover_locked(&dir).unwrap(),
            RecoveryOutcome::Missing { evidence: true }
        );
    }

    #[test]
    fn recover_locked_blocked_is_invalid_data() {
        for ambiguous in [true, false] {
            let (path, dir) = folder();
            let first = b"config_version = \"2\"\n# first\n";
            let second = b"config_version = \"2\"\n# second\n";
            let files = if ambiguous {
                vec![
                    (ROLLING_BACKUP_NAME.to_owned(), first.as_slice()),
                    ("config.toml.bak.new".to_owned(), second.as_slice()),
                ]
            } else {
                vec![
                    (
                        ProtectedBackupName::new(3, 2, ChangeFingerprint::of(first))
                            .unwrap()
                            .file_name(),
                        first.as_slice(),
                    ),
                    (
                        ProtectedBackupName::new(3, 2, ChangeFingerprint::of(second))
                            .unwrap()
                            .file_name(),
                        second.as_slice(),
                    ),
                ]
            };
            for (name, bytes) in &files {
                fs::write(path.join(name), bytes).unwrap();
            }
            assert_eq!(
                recover_locked(&dir).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert!(!path.join(settings::PRIMARY_NAME).exists());
            assert_eq!(fs::read_dir(&*path).unwrap().count(), files.len());
            for (name, bytes) in files {
                assert_eq!(fs::read(path.join(name)).unwrap(), bytes);
            }
        }
    }

    #[test]
    fn recover_locked_clear_error_keeps_restore() {
        let (path, dir) = folder();
        let bytes = b"config_version = \"2\"\n";
        fs::write(path.join(ROLLING_BACKUP_NAME), bytes).unwrap();
        fs::create_dir(path.join(config_commit::FAILURE_RECORD_NAME)).unwrap();
        assert_eq!(
            recover_locked(&dir).unwrap(),
            RecoveryOutcome::Restored { schema: 2 }
        );
        assert_eq!(fs::read(path.join(settings::PRIMARY_NAME)).unwrap(), bytes);
        assert!(path.join(config_commit::FAILURE_RECORD_NAME).is_dir());
    }

    #[test]
    fn recover_locked_stops_after_restore_error() {
        let (path, dir) = folder();
        let original = b"config_version = \"2\"\n";
        let protected = b"config_version = \"2\"\n# protected\n";
        let backup = path.join(ROLLING_BACKUP_NAME);
        let name = ProtectedBackupName::new(1, 2, ChangeFingerprint::of(protected))
            .unwrap()
            .file_name();
        fs::write(&backup, original).unwrap();
        fs::write(path.join(&name), protected).unwrap();
        let result = recover_locked_with_hook(&dir, || {
            fs::remove_file(&backup).unwrap();
            fs::write(&backup, b"config_version = \"2\"\n# swapped\n").unwrap();
        });
        assert!(result.is_err());
        assert!(!path.join(settings::PRIMARY_NAME).exists());
        assert_eq!(
            fs::read(&backup).unwrap(),
            b"config_version = \"2\"\n# swapped\n"
        );
        assert_eq!(fs::read(path.join(name)).unwrap(), protected);
        assert_eq!(fs::read_dir(&*path).unwrap().count(), 2);
    }

    #[test]
    fn inventory_classifies_every_known_name() {
        let (path, dir) = folder();
        let backup = ProtectedBackupName::new(1, 2, ChangeFingerprint::of(b"backup")).unwrap();
        for name in [
            "CONFIG.TOML.BAK",
            "CONFIG.TOML.BAK.NEW",
            "CONFIG.TOML.RECOVER",
            "CONFIG.PRE-MIGRATION-x.BAK",
            "CONFIG.TOML.NEW",
            "CONFIG.TOML.FAILURE",
            "config.toml.1-2-3.tmp",
            &backup.file_name(),
        ] {
            fs::write(path.join(name), b"x").unwrap();
        }
        let found = inventory(&dir).unwrap();
        assert!(found.rolling_backup);
        assert_eq!(found.protected_backups, vec![backup]);
        assert_eq!(found.leftover_drafts, 1);
        assert!(found.ambiguous_leftovers);
        assert!(found.failure_record_present);
        assert!(found.other_evidence);
        assert_eq!(found.recorded_failure, None);
    }

    #[test]
    fn inventory_ambiguous_names_set_only_ambiguous() {
        for name in [
            "config.toml.bak.new",
            "config.toml.recover",
            "config.pre-migration-x.bak",
        ] {
            let (path, dir) = folder();
            fs::write(path.join(name), b"x").unwrap();
            assert_eq!(
                inventory(&dir).unwrap(),
                ArtifactInventory {
                    ambiguous_leftovers: true,
                    ..Default::default()
                }
            );
        }
    }

    #[test]
    fn inventory_uppercase_ambiguous_names_set_only_ambiguous() {
        for name in [
            "CONFIG.TOML.BAK.NEW",
            "CONFIG.TOML.RECOVER",
            "CONFIG.PRE-MIGRATION-X.BAK",
        ] {
            let (path, dir) = folder();
            fs::write(path.join(name), b"x").unwrap();
            assert_eq!(
                inventory(&dir).unwrap(),
                ArtifactInventory {
                    ambiguous_leftovers: true,
                    ..Default::default()
                }
            );
        }
    }

    #[test]
    fn inventory_stale_draft_variants_count_only_as_drafts() {
        for name in ["config.toml.bak.1-2-3.tmp", "config.toml.failure.1-2-3.tmp"] {
            let (path, dir) = folder();
            fs::write(path.join(name), b"x").unwrap();
            assert_eq!(
                inventory(&dir).unwrap(),
                ArtifactInventory {
                    leftover_drafts: 1,
                    ..Default::default()
                }
            );
        }
    }

    #[test]
    fn inventory_non_unicode_name_is_ambiguous_evidence() {
        use std::os::windows::ffi::OsStringExt;
        let (path, dir) = folder();
        let name = std::ffi::OsString::from_wide(&[
            b'x' as u16,
            0xd800,
            b'.' as u16,
            b't' as u16,
            b'x' as u16,
            b't' as u16,
        ]);
        if let Err(error) = fs::write(path.join(name), b"x") {
            eprintln!("skipping non-Unicode filename test: {error}");
            return;
        }
        assert_eq!(
            inventory(&dir).unwrap(),
            ArtifactInventory {
                other_evidence: true,
                ambiguous_leftovers: true,
                ..Default::default()
            }
        );
    }

    #[test]
    fn inventory_config_toml_new_is_evidence_not_ambiguous() {
        let (path, dir) = folder();
        fs::write(path.join("config.toml.new"), b"x").unwrap();
        assert_eq!(
            inventory(&dir).unwrap(),
            ArtifactInventory {
                other_evidence: true,
                ..Default::default()
            }
        );
    }

    #[test]
    fn inventory_failure_record_counts_once() {
        let (path, dir) = folder();
        let record = FailureRecord {
            source: ChangeFingerprint::of(b"source"),
            from_schema: 1,
            stage: FailureStage::Backup,
            attempts: 1,
        };
        fs::write(
            path.join(config_commit::FAILURE_RECORD_NAME),
            record.encode(),
        )
        .unwrap();
        let found = inventory(&dir).unwrap();
        assert!(found.failure_record_present);
        assert_eq!(found.leftover_drafts, 0);
        assert_eq!(found.recorded_failure, Some(record.source));
        fs::write(path.join(config_commit::FAILURE_RECORD_NAME), b"corrupt").unwrap();
        let found = inventory(&dir).unwrap();
        assert!(found.failure_record_present);
        assert_eq!(found.leftover_drafts, 0);
        assert_eq!(found.recorded_failure, None);
    }

    #[test]
    fn inventory_empty_folder_has_no_evidence() {
        let (_, dir) = folder();
        assert_eq!(inventory(&dir).unwrap(), ArtifactInventory::default());
    }

    #[test]
    fn inventory_or_closed_blocks_on_listing_error() {
        let found = close_on_error(Err(io::Error::other("listing failed")));
        assert_eq!(
            found,
            ArtifactInventory {
                other_evidence: true,
                ambiguous_leftovers: true,
                ..Default::default()
            }
        );
        assert!(found.has_recovery_artifacts());
    }

    #[test]
    fn classify_candidate_follows_primary_rule() {
        use island_core::config_state::CandidateValidity as V;
        for (text, expected) in [
            (
                "config_version = \"2\"\n",
                V::Valid {
                    schema: 2,
                    fingerprint: ChangeFingerprint::of(b"config_version = \"2\"\n"),
                },
            ),
            (
                "config_version = \"1\"\n",
                V::Valid {
                    schema: 1,
                    fingerprint: ChangeFingerprint::of(b"config_version = \"1\"\n"),
                },
            ),
            (
                "foo = \"x\"\n",
                V::Valid {
                    schema: 1,
                    fingerprint: ChangeFingerprint::of(b"foo = \"x\"\n"),
                },
            ),
            (
                "config_version = \"2\"\r\nfoo = \"x\"\r\n",
                V::Valid {
                    schema: 2,
                    fingerprint: ChangeFingerprint::of(
                        b"config_version = \"2\"\r\nfoo = \"x\"\r\n",
                    ),
                },
            ),
            (
                "[section]\nconfig_version = \"2\"\n",
                V::Valid {
                    schema: 2,
                    fingerprint: ChangeFingerprint::of(b"[section]\nconfig_version = \"2\"\n"),
                },
            ),
            ("config_version = \"8\"\n", V::Future),
            ("config_version = \"x\"\n", V::Invalid),
            (
                "indicator_style = \"ring\"\nindicator_style = \"ring\"\n",
                V::Invalid,
            ),
            ("ui_density = \"minimal\n", V::Invalid),
        ] {
            assert_eq!(classify_candidate(text.as_bytes()), expected, "{text:?}");
        }
    }

    #[test]
    fn classify_candidate_keeps_unsupported_lines() {
        use island_core::config_state::CandidateValidity as V;
        for text in [
            "config_version = \"2\"\nstray line\n",
            "\u{feff}config_version = \"2\"\nstray line\n",
        ] {
            assert_eq!(
                classify_candidate(text.as_bytes()),
                V::Valid {
                    schema: 2,
                    fingerprint: ChangeFingerprint::of(text.as_bytes())
                }
            );
        }
    }

    #[test]
    fn classify_candidate_rejects_all_unsupported_and_empty() {
        use island_core::config_state::CandidateValidity as V;
        for text in [
            "stray line\n",
            "# comment\n\nstray line\n[section]\n",
            "",
            "  \n\t",
            "\u{feff}# comment\nstray line\n",
            "stray\n\u{feff}stray\n",
        ] {
            assert_eq!(classify_candidate(text.as_bytes()), V::Invalid, "{text:?}");
        }
    }

    #[test]
    fn classify_candidate_rejects_non_utf8() {
        assert_eq!(
            classify_candidate(&[0xff]),
            island_core::config_state::CandidateValidity::Invalid
        );
    }

    #[test]
    fn read_candidate_maps_errors() {
        use island_core::config_state::CandidateValidity as V;
        let (path, dir) = folder();
        fs::write(
            path.join("large"),
            vec![b'x'; island_core::migration::MAX_CONFIG_BYTES + 1],
        )
        .unwrap();
        fs::create_dir(path.join("directory")).unwrap();
        assert_eq!(
            read_candidate(&dir, std::ffi::OsStr::new("large")),
            V::Invalid
        );
        assert_eq!(
            read_candidate(&dir, std::ffi::OsStr::new("directory")),
            V::Unreadable
        );
        assert_eq!(
            read_candidate(&dir, std::ffi::OsStr::new("missing")),
            V::Unreadable
        );
    }

    #[test]
    fn candidates_reads_read_only_protected_backup() {
        use island_core::config_state::{CandidateSource, CandidateValidity, RecoveryCandidate};
        let (path, dir) = folder();
        let bytes = b"config_version = \"2\"\n";
        let name = ProtectedBackupName::new(1, 2, ChangeFingerprint::of(bytes)).unwrap();
        let file = path.join(name.file_name());
        fs::write(&file, bytes).unwrap();
        let _clear_readonly = ClearReadonly(file.clone());
        let mut permissions = fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&file, permissions).unwrap();
        let inventory = ArtifactInventory {
            protected_backups: vec![name.clone()],
            ..Default::default()
        };
        assert_eq!(
            candidates(&dir, &inventory),
            vec![RecoveryCandidate {
                source: CandidateSource::Protected(name),
                validity: CandidateValidity::Valid {
                    schema: 2,
                    fingerprint: ChangeFingerprint::of(bytes)
                }
            }]
        );
        assert!(fs::metadata(&file).unwrap().permissions().readonly());
        assert_eq!(fs::read(&file).unwrap(), bytes);
    }

    #[test]
    fn candidates_reads_uppercase_rolling_backup() {
        let (path, dir) = folder();
        let bytes = b"config_version = \"2\"\n";
        fs::write(path.join("CONFIG.TOML.BAK"), bytes).unwrap();
        let found = inventory(&dir).unwrap();
        assert_eq!(
            candidates(&dir, &found),
            vec![RecoveryCandidate {
                source: CandidateSource::RollingBackup,
                validity: CandidateValidity::Valid {
                    schema: 2,
                    fingerprint: ChangeFingerprint::of(bytes)
                },
            }]
        );
    }

    #[test]
    fn candidates_lists_rolling_first_and_changes_nothing() {
        use island_core::config_state::{CandidateSource, CandidateValidity, RecoveryCandidate};
        let (path, dir) = folder();
        let current = b"config_version = \"2\"\n";
        let legacy = b"config_version = \"1\"\n";
        let a = ProtectedBackupName::new(1, 1, ChangeFingerprint::of(legacy)).unwrap();
        let b = ProtectedBackupName::new(2, 2, ChangeFingerprint::of(current)).unwrap();
        fs::write(path.join("config.toml.bak"), current).unwrap();
        fs::write(path.join(a.file_name()), legacy).unwrap();
        fs::write(path.join(b.file_name()), current).unwrap();
        let mut before = dir.entries().unwrap();
        before.sort();
        let inventory = ArtifactInventory {
            rolling_backup: true,
            protected_backups: vec![b.clone(), a.clone()],
            ..Default::default()
        };
        assert_eq!(
            candidates(&dir, &inventory),
            vec![
                RecoveryCandidate {
                    source: CandidateSource::RollingBackup,
                    validity: CandidateValidity::Valid {
                        schema: 2,
                        fingerprint: ChangeFingerprint::of(current)
                    }
                },
                RecoveryCandidate {
                    source: CandidateSource::Protected(b.clone()),
                    validity: CandidateValidity::Valid {
                        schema: 2,
                        fingerprint: ChangeFingerprint::of(current)
                    }
                },
                RecoveryCandidate {
                    source: CandidateSource::Protected(a.clone()),
                    validity: CandidateValidity::Valid {
                        schema: 1,
                        fingerprint: ChangeFingerprint::of(legacy)
                    }
                },
            ]
        );
        let mut after = dir.entries().unwrap();
        after.sort();
        assert_eq!(before, after);
        assert_eq!(fs::read(path.join("config.toml.bak")).unwrap(), current);
        assert_eq!(fs::read(path.join(a.file_name())).unwrap(), legacy);
        assert_eq!(fs::read(path.join(b.file_name())).unwrap(), current);
    }
}
