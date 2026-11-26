//! Replaces the primary configuration file through a verified draft. All operations are relative to the held configuration directory handle.

use std::io;
use std::path::Path;

use super::config_fs::{ConfigDir, Draft, VerifiedFile};
use super::settings::{PRIMARY_NAME, name, read_verified};

pub(crate) const FAILURE_RECORD_NAME: &str = "config.toml.failure";

pub(crate) fn read_failure(dir: &ConfigDir) -> island_core::config_state::StoredFailure {
    use island_core::config_state::{FAILURE_RECORD_MAX_BYTES, FailureRecord, StoredFailure};
    let mut file = match VerifiedFile::open(dir, FAILURE_RECORD_NAME.as_ref()) {
        Ok(file) => file,
        Err(super::config_fs::ConfigFsError::Io(error))
            if error.kind() == io::ErrorKind::NotFound =>
        {
            return StoredFailure::None;
        }
        Err(_) => return StoredFailure::Corrupt,
    };
    match file.read_bounded(FAILURE_RECORD_MAX_BYTES) {
        Ok(bytes) => FailureRecord::decode(&bytes)
            .map(StoredFailure::Valid)
            .unwrap_or(StoredFailure::Corrupt),
        Err(_) => StoredFailure::Corrupt,
    }
}

// Used by migration from 2C-3 S5.
pub(crate) fn write_failure(
    dir: &ConfigDir,
    record: &island_core::config_state::FailureRecord,
) -> io::Result<()> {
    let target = std::ffi::OsStr::new(FAILURE_RECORD_NAME);
    let bytes = record.encode();
    let mut draft = Draft::create(dir, target, None).map_err(io::Error::from)?;
    draft.write(bytes.as_bytes()).map_err(io::Error::from)?;
    verify_draft(&mut draft, bytes.as_bytes())?;
    draft
        .rename(dir, target, None, None, true, None)
        .map_err(io::Error::from)
}

// Used by migration from 2C-3 S5.
pub(crate) fn clear_failure(dir: &ConfigDir) -> io::Result<()> {
    match VerifiedFile::delete_verified(dir, FAILURE_RECORD_NAME.as_ref()) {
        Ok(()) => Ok(()),
        Err(super::config_fs::ConfigFsError::Io(error))
            if error.kind() == io::ErrorKind::NotFound =>
        {
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn is_primary_changed(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|cause| cause.is::<super::config_fs::PrimaryChanged>())
}

#[derive(Debug)]
pub(crate) struct CommitUnverified;
impl std::fmt::Display for CommitUnverified {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("configuration commit could not be verified")
    }
}
impl std::error::Error for CommitUnverified {}

pub(crate) fn is_commit_unverified(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|cause| cause.is::<CommitUnverified>())
}

#[cfg(test)]
thread_local! {
    pub(crate) static AFTER_PRIMARY_RENAME: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = std::cell::RefCell::new(None);
    pub(crate) static FAIL_VERIFICATION_AT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn verify_draft(draft: &mut Draft, expected: &[u8]) -> io::Result<()> {
    let actual = draft.read_back().map_err(io::Error::from)?;
    #[cfg(test)]
    let actual = FAIL_VERIFICATION_AT.with(|count| {
        let remaining = count.get();
        if remaining > 0 {
            count.set(remaining - 1);
        }
        let mut actual = actual;
        if remaining == 1 {
            actual.push(0);
        }
        actual
    });
    if actual != expected {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration draft did not verify",
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn replace(path: &Path, text: &str) -> io::Result<()> {
    let dir = ConfigDir::open(path.parent().unwrap()).map_err(io::Error::from)?;
    let expected = std::fs::read(path).ok();
    commit_primary(&dir, path, text, expected.as_deref())
}
/// A crash half way must not cost the user their calendar address.
pub(crate) fn commit_primary(
    dir: &ConfigDir,
    path: &Path,
    text: &str,
    expected: Option<&[u8]>,
) -> io::Result<()> {
    commit_primary_with_hook(
        dir,
        path,
        text,
        expected,
        #[cfg(test)]
        || {},
    )
}
pub(crate) fn commit_primary_with_hook(
    dir: &ConfigDir,
    path: &Path,
    text: &str,
    expected: Option<&[u8]>,
    #[cfg(test)] hook: impl FnOnce(),
) -> io::Result<()> {
    #[cfg(test)]
    let after_primary_rename = AFTER_PRIMARY_RENAME.with(|hook| hook.borrow_mut().take());
    let target = name(path)?;
    let mut source = match VerifiedFile::open_source(dir, target) {
        Ok(file) => Some(file),
        Err(super::config_fs::ConfigFsError::Io(e)) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if let Some(expected) = expected {
        let Some(source) = source.as_mut() else {
            return Err(io::Error::other(super::config_fs::PrimaryChanged));
        };
        match source.read_bounded(island_core::migration::MAX_CONFIG_BYTES) {
            Ok(actual) if actual == expected => {}
            Ok(_) | Err(super::config_fs::ConfigFsError::TooLarge) => {
                return Err(io::Error::other(super::config_fs::PrimaryChanged));
            }
            Err(error) => return Err(error.into()),
        }
    }
    let security = source
        .as_ref()
        .map(VerifiedFile::security)
        .transpose()
        .map_err(io::Error::from)?;
    let mut draft = Draft::create(dir, name(path)?, security.as_ref()).map_err(io::Error::from)?;
    draft.write(text.as_bytes()).map_err(io::Error::from)?;
    verify_draft(&mut draft, text.as_bytes())?;
    if let Some(source) = source.as_mut() {
        source.rewind().map_err(io::Error::from)?;
        let original = read_verified(source)?;
        let backup = path.with_extension("toml.bak");
        let mut backup_draft =
            Draft::create(dir, name(&backup)?, security.as_ref()).map_err(io::Error::from)?;
        backup_draft
            .write(original.as_bytes())
            .map_err(io::Error::from)?;
        verify_draft(&mut backup_draft, original.as_bytes())?;
        backup_draft
            .rename(
                dir,
                name(&backup)?,
                security.as_ref(),
                Some((source.try_clone().map_err(io::Error::from)?, target)),
                true,
                None,
            )
            .map_err(io::Error::from)?;
    }
    #[cfg(test)]
    hook();
    let draft_identity = draft.identity().map_err(io::Error::from)?;
    let verify_primary = security.is_some();
    draft
        .rename(
            dir,
            name(path)?,
            security.as_ref(),
            source.map(|f| (f, target)),
            security.is_some(),
            expected,
        )
        .map_err(io::Error::from)?;
    #[cfg(test)]
    if let Some(hook) = after_primary_rename
        && verify_primary
    {
        hook();
    }
    if verify_primary {
        let verified = (|| {
            let mut file = VerifiedFile::open(dir, target)
                .map_err(|error| {
                    eprintln!(
                        "configuration verification open failed: {:?}",
                        io::Error::from(error).kind()
                    );
                })
                .ok()?;
            let identity = file.identity();
            let bytes = file
                .read_bounded(island_core::migration::MAX_CONFIG_BYTES)
                .map_err(|error| {
                    eprintln!(
                        "configuration verification read failed: {:?}",
                        io::Error::from(error).kind()
                    );
                })
                .ok()?;
            Some(identity == draft_identity && bytes == text.as_bytes())
        })();
        if verified != Some(true) {
            return Err(io::Error::other(CommitUnverified));
        }
    }
    #[cfg(test)]
    if let Some(expected) = security.as_ref() {
        let actual = VerifiedFile::open(dir, name(path)?)
            .map_err(io::Error::from)?
            .security()
            .map_err(io::Error::from)?;
        assert!(expected.same_dacl(&actual));
    }
    Ok(())
}

/// Writes one protected backup of `text` next to the primary under a core-built unique name.
/// The file is created exclusively (an existing name is an `AlreadyExists` error, never
/// overwritten), written through, read back and compared, then marked read-only. Any failure
/// deletes only the file this call created. Retention is not handled here.
pub(crate) fn create_protected_backup(
    dir: &ConfigDir,
    text: &str,
    schema: u32,
) -> io::Result<island_core::config_state::ProtectedBackupName> {
    use island_core::config_state::{ChangeFingerprint, ProtectedBackupName, next_sequence};
    let existing: Vec<ProtectedBackupName> = dir
        .entries()
        .map_err(io::Error::from)?
        .iter()
        .filter_map(|entry| ProtectedBackupName::parse(entry.to_str()?))
        .collect();
    let sequence = next_sequence(&existing)
        .ok_or_else(|| io::Error::other("protected backup sequence exhausted"))?;
    let backup = ProtectedBackupName::new(sequence, schema, ChangeFingerprint::of(text.as_bytes()))
        .ok_or_else(|| io::Error::other("invalid protected backup name"))?;
    let security = match VerifiedFile::open_source(dir, std::ffi::OsStr::new(PRIMARY_NAME)) {
        Ok(primary) => Some(primary.security().map_err(io::Error::from)?),
        Err(super::config_fs::ConfigFsError::Io(e)) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let file_name = backup.file_name();
    let mut file = Draft::create_exact(dir, std::ffi::OsStr::new(&file_name), security.as_ref())
        .map_err(io::Error::from)?;
    file.write(text.as_bytes()).map_err(io::Error::from)?;
    verify_draft(&mut file, text.as_bytes())?;
    file.keep_read_only().map_err(io::Error::from)?;
    Ok(backup)
}

/// Deletes the oldest protected backups beyond `keep`, as chosen by the core retention rule,
/// and returns the deleted names oldest first. Only names that parse as a canonical
/// `ProtectedBackupName` are ever considered; every other file is never opened. `just_created`
/// is never deleted, and if it is not among the parsed names the call fails with `NotFound`
/// before deleting anything. Every file operation is relative to the held directory handle.
/// Deletion is per file and stops at the first failure, so backups deleted before a failure
/// stay deleted (they were excess) and every file not yet reached is unchanged; the file that
/// failed keeps its original attributes and still exists.
pub(crate) fn apply_protected_retention(
    dir: &ConfigDir,
    just_created: &island_core::config_state::ProtectedBackupName,
    keep: usize,
) -> io::Result<Vec<island_core::config_state::ProtectedBackupName>> {
    use island_core::config_state::{ProtectedBackupName, retention_excess};
    let parsed: Vec<ProtectedBackupName> = dir
        .entries()
        .map_err(io::Error::from)?
        .iter()
        .filter_map(|entry| ProtectedBackupName::parse(entry.to_str()?))
        .collect();
    if !parsed.contains(just_created) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the protected backup just created is not in the folder",
        ));
    }
    let mut excess = retention_excess(&parsed, keep);
    excess.retain(|name| name != just_created);
    let mut deleted = Vec::with_capacity(excess.len());
    for name in excess {
        VerifiedFile::delete_read_only(dir, std::ffi::OsStr::new(&name.file_name()))
            .map_err(io::Error::from)?;
        deleted.push(name);
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Temp(std::path::PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "islora-config-commit-{}-{}-{}",
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

    fn failure_record() -> island_core::config_state::FailureRecord {
        use island_core::config_state::{ChangeFingerprint, FailureRecord, FailureStage};
        FailureRecord {
            source: ChangeFingerprint::of(b"source"),
            from_schema: 1,
            stage: FailureStage::Commit,
            attempts: 1,
        }
    }

    #[test]
    fn failure_record_round_trips() {
        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let record = failure_record();
        write_failure(&dir, &record).unwrap();
        assert_eq!(
            read_failure(&dir),
            island_core::config_state::StoredFailure::Valid(record)
        );
    }

    #[test]
    fn missing_record_is_none() {
        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        assert_eq!(
            read_failure(&dir),
            island_core::config_state::StoredFailure::None
        );
    }

    #[test]
    fn oversized_record_is_corrupt() {
        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        for bytes in [vec![b'x'; 257], vec![0xff], b"garbage".to_vec()] {
            std::fs::write(temp.0.join(FAILURE_RECORD_NAME), bytes).unwrap();
            assert_eq!(
                read_failure(&dir),
                island_core::config_state::StoredFailure::Corrupt
            );
        }
    }

    #[test]
    fn directory_failure_record_is_corrupt() {
        let temp = Temp::new();
        std::fs::create_dir(temp.0.join(FAILURE_RECORD_NAME)).unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        assert_eq!(
            read_failure(&dir),
            island_core::config_state::StoredFailure::Corrupt
        );
    }

    #[test]
    fn longest_valid_failure_record_is_readable() {
        use island_core::config_state::{FAILURE_RECORD_MAX_BYTES, FailureStage, StoredFailure};
        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let mut record = failure_record();
        record.from_schema = u32::MAX;
        record.attempts = u32::MAX;
        record.stage = FailureStage::Commit;
        let encoded = record.encode();
        // All stage names have six bytes; both numeric fields are at their maximum.
        assert!(encoded.len() < FAILURE_RECORD_MAX_BYTES);
        std::fs::write(temp.0.join(FAILURE_RECORD_NAME), encoded).unwrap();
        assert_eq!(read_failure(&dir), StoredFailure::Valid(record));
    }

    #[test]
    fn write_failure_replaces_previous() {
        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let mut record = failure_record();
        write_failure(&dir, &record).unwrap();
        record.attempts = 2;
        write_failure(&dir, &record).unwrap();
        assert_eq!(
            read_failure(&dir),
            island_core::config_state::StoredFailure::Valid(record)
        );
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 1);
    }

    #[test]
    fn clear_failure_removes_and_is_idempotent() {
        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        std::fs::write(temp.0.join("other"), b"unchanged").unwrap();
        write_failure(&dir, &failure_record()).unwrap();
        clear_failure(&dir).unwrap();
        assert_eq!(
            read_failure(&dir),
            island_core::config_state::StoredFailure::None
        );
        clear_failure(&dir).unwrap();
        assert_eq!(std::fs::read(temp.0.join("other")).unwrap(), b"unchanged");
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 1);
    }

    #[test]
    fn failure_write_verification_error_preserves_previous() {
        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let record = failure_record();
        write_failure(&dir, &record).unwrap();
        FAIL_VERIFICATION_AT.with(|count| count.set(1));
        assert!(write_failure(&dir, &record).is_err());
        assert_eq!(
            read_failure(&dir),
            island_core::config_state::StoredFailure::Valid(record)
        );
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 1);
    }

    #[test]
    fn verification_mismatch_leaves_files_unchanged_and_removes_drafts() {
        for fail_at in 1..=2 {
            let temp = Temp::new();
            let path = temp.0.join("config.toml");
            let backup = temp.0.join("config.toml.bak");
            std::fs::write(&path, b"old").unwrap();
            std::fs::write(&backup, b"previous backup").unwrap();
            let dir = ConfigDir::open(&temp.0).unwrap();
            FAIL_VERIFICATION_AT.with(|count| count.set(fail_at));
            let error = commit_primary(&dir, &path, "new", Some(b"old")).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
            assert!(!is_primary_changed(&error));
            assert_eq!(std::fs::read(&path).unwrap(), b"old");
            assert_eq!(std::fs::read(&backup).unwrap(), b"previous backup");
            assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 2);
        }
    }
    #[test]
    fn changed_bytes_abort_and_remove_draft() {
        let temp = Temp::new();
        let path = temp.0.join("config.toml");
        std::fs::write(&path, b"old").unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let error = commit_primary_with_hook(&dir, &path, "new", Some(b"old"), || {
            std::fs::write(&path, b"external").unwrap();
        })
        .unwrap_err();
        assert!(is_primary_changed(&error));
        assert_eq!(std::fs::read(&path).unwrap(), b"external");
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 2);
    }

    #[test]
    fn external_edits_refuse_save_without_replacing_backup() {
        for external in [
            vec![0xff],
            vec![b'x'; island_core::migration::MAX_CONFIG_BYTES + 1],
            b"external".to_vec(),
        ] {
            let temp = Temp::new();
            let path = temp.0.join("config.toml");
            let backup = temp.0.join("config.toml.bak");
            std::fs::write(&path, b"old").unwrap();
            std::fs::write(&backup, b"previous backup").unwrap();
            let dir = ConfigDir::open(&temp.0).unwrap();
            std::fs::write(&path, &external).unwrap();
            let error = commit_primary(&dir, &path, "new", Some(b"old")).unwrap_err();
            assert!(is_primary_changed(&error));
            assert_eq!(std::fs::read(&path).unwrap(), external);
            assert_eq!(std::fs::read(&backup).unwrap(), b"previous backup");
            assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 2);
        }
    }

    #[test]
    fn deleted_primary_aborts_and_removes_draft() {
        let temp = Temp::new();
        let path = temp.0.join("config.toml");
        std::fs::write(&path, b"old").unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let error = commit_primary_with_hook(&dir, &path, "new", Some(b"old"), || {
            std::fs::remove_file(&path).unwrap();
        })
        .unwrap_err();
        assert!(is_primary_changed(&error));
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 1);
        let error = commit_primary(&dir, &path, "new", Some(b"old")).unwrap_err();
        assert!(is_primary_changed(&error));
        assert!(!path.exists());
    }

    #[test]
    fn post_rename_changes_are_unverified() {
        for mode in 0..3 {
            let temp = Temp::new();
            let path = temp.0.join("config.toml");
            std::fs::write(&path, b"old").unwrap();
            let dir = ConfigDir::open(&temp.0).unwrap();
            let hook_path = path.clone();
            AFTER_PRIMARY_RENAME.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || match mode {
                    0 => std::fs::write(&hook_path, b"changed").unwrap(),
                    1 => {
                        std::fs::rename(&hook_path, hook_path.with_extension("old")).unwrap();
                        std::fs::write(&hook_path, b"new").unwrap();
                    }
                    _ => std::fs::remove_file(&hook_path).unwrap(),
                }));
            });
            let error = commit_primary(&dir, &path, "new", Some(b"old")).unwrap_err();
            assert!(is_commit_unverified(&error));
            assert!(!is_primary_changed(&error));
            assert_eq!(
                std::fs::read(temp.0.join("config.toml.bak")).unwrap(),
                b"old"
            );
            if mode == 2 {
                assert!(!path.exists());
            }
        }
    }

    #[test]
    fn early_failure_discards_post_rename_hook() {
        let temp = Temp::new();
        let path = temp.0.join("config.toml");
        std::fs::write(&path, b"old").unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        AFTER_PRIMARY_RENAME.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|| panic!("hook must not run")));
        });
        let error = commit_primary(&dir, &path, "new", Some(b"different")).unwrap_err();
        assert!(is_primary_changed(&error));
        commit_primary(&dir, &path, "new", Some(b"old")).unwrap();
    }

    #[test]
    fn matching_primary_is_replaced() {
        let temp = Temp::new();
        let path = temp.0.join("config.toml");
        std::fs::write(&path, b"old").unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        commit_primary(&dir, &path, "new", Some(b"old")).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn first_run_discards_post_rename_hook() {
        let temp = Temp::new();
        let path = temp.0.join("config.toml");
        let dir = ConfigDir::open(&temp.0).unwrap();
        AFTER_PRIMARY_RENAME.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|| panic!("hook must not run")));
        });
        commit_primary(&dir, &path, "new", None).unwrap();
        AFTER_PRIMARY_RENAME.with(|hook| assert!(hook.borrow().is_none()));
        commit_primary(&dir, &path, "updated", Some(b"new")).unwrap();
    }

    fn is_readonly(path: &Path) -> bool {
        std::fs::metadata(path).unwrap().permissions().readonly()
    }

    /// Clears the read-only attribute so the temporary folder can be removed.
    fn clear_readonly(folder: &Path) {
        for entry in std::fs::read_dir(folder).unwrap().flatten() {
            let mut permissions = entry.metadata().unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(entry.path(), permissions);
        }
    }

    #[test]
    fn protected_backup_has_canonical_name_bytes_and_readonly() {
        use island_core::config_state::{ChangeFingerprint, ProtectedBackupName};
        let temp = Temp::new();
        std::fs::write(temp.0.join("config.toml"), b"old").unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let backup = create_protected_backup(&dir, "config text", 2).unwrap();
        let file_name = backup.file_name();
        assert_eq!(ProtectedBackupName::parse(&file_name), Some(backup.clone()));
        assert_eq!(backup.sequence(), 1);
        assert_eq!(backup.source_schema(), 2);
        assert_eq!(backup.fingerprint(), ChangeFingerprint::of(b"config text"));
        let path = temp.0.join(&file_name);
        assert_eq!(std::fs::read(&path).unwrap(), b"config text");
        assert!(is_readonly(&path));
        drop(dir);
        clear_readonly(&temp.0);
    }

    #[test]
    fn second_protected_backup_takes_next_sequence_and_keeps_first() {
        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let first = create_protected_backup(&dir, "first", 2).unwrap();
        let second = create_protected_backup(&dir, "second", 2).unwrap();
        assert_eq!(second.sequence(), first.sequence() + 1);
        let first_path = temp.0.join(first.file_name());
        assert_eq!(std::fs::read(&first_path).unwrap(), b"first");
        assert!(is_readonly(&first_path));
        let second_path = temp.0.join(second.file_name());
        assert_eq!(std::fs::read(&second_path).unwrap(), b"second");
        assert!(is_readonly(&second_path));
        drop(dir);
        clear_readonly(&temp.0);
    }

    #[test]
    fn protected_backup_ignores_unrelated_names() {
        let temp = Temp::new();
        let unrelated = [
            ("config.toml", &b"primary"[..]),
            ("config.toml.bak", b"previous backup"),
            ("config.toml.1-2-3.tmp", b"draft"),
            ("config.protected.7.v2.0000000000000000.bak", b"bad name"),
            ("notes.txt", b"notes"),
        ];
        for (name, bytes) in unrelated {
            std::fs::write(temp.0.join(name), bytes).unwrap();
        }
        let dir = ConfigDir::open(&temp.0).unwrap();
        let backup = create_protected_backup(&dir, "text", 2).unwrap();
        assert_eq!(backup.sequence(), 1);
        for (name, bytes) in unrelated {
            assert_eq!(std::fs::read(temp.0.join(name)).unwrap(), bytes);
            assert!(!is_readonly(&temp.0.join(name)));
        }
        assert_eq!(
            std::fs::read_dir(&temp.0).unwrap().count(),
            unrelated.len() + 1
        );
        drop(dir);
        clear_readonly(&temp.0);
    }

    #[test]
    fn colliding_name_returns_already_exists_and_keeps_existing_file() {
        use island_core::config_state::{ChangeFingerprint, ProtectedBackupName};
        let temp = Temp::new();
        // An upper-case spelling does not parse, so the sequence stays 1, but the
        // case-insensitive filesystem reports the exact canonical name as taken.
        let canonical = ProtectedBackupName::new(1, 2, ChangeFingerprint::of(b"text"))
            .unwrap()
            .file_name();
        let planted = temp.0.join(canonical.to_ascii_uppercase());
        std::fs::write(&planted, b"planted").unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let error = create_protected_backup(&dir, "text", 2).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&planted).unwrap(), b"planted");
        assert!(!is_readonly(&planted));
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 1);
    }

    #[test]
    fn protected_backup_readonly_readback_failure_leaves_no_file() {
        let temp = Temp::new();
        std::fs::write(temp.0.join("config.toml"), b"old").unwrap();
        std::fs::write(temp.0.join("config.toml.bak"), b"previous").unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        super::super::config_fs::FAIL_READONLY_READBACK.with(|flag| flag.set(true));
        let error = create_protected_backup(&dir, "text", 2).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 2);
        assert_eq!(std::fs::read(temp.0.join("config.toml")).unwrap(), b"old");
        assert_eq!(
            std::fs::read(temp.0.join("config.toml.bak")).unwrap(),
            b"previous"
        );
        let backup = create_protected_backup(&dir, "text", 2).unwrap();
        assert_eq!(backup.sequence(), 1);
        drop(dir);
        clear_readonly(&temp.0);
    }

    #[test]
    fn protected_backup_readback_mismatch_leaves_no_file() {
        let temp = Temp::new();
        std::fs::write(temp.0.join("config.toml"), b"old").unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        FAIL_VERIFICATION_AT.with(|count| count.set(1));
        let error = create_protected_backup(&dir, "text", 2).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 1);
        assert_eq!(std::fs::read(temp.0.join("config.toml")).unwrap(), b"old");
        let backup = create_protected_backup(&dir, "text", 2).unwrap();
        assert_eq!(backup.sequence(), 1);
        drop(dir);
        clear_readonly(&temp.0);
    }

    use island_core::config_state::ProtectedBackupName;

    /// Creates `count` protected backups with distinct text, oldest first.
    fn make_backups(dir: &ConfigDir, count: usize) -> Vec<ProtectedBackupName> {
        (1..=count)
            .map(|index| create_protected_backup(dir, &format!("backup {index}"), 2).unwrap())
            .collect()
    }

    fn path_of(temp: &Temp, name: &ProtectedBackupName) -> std::path::PathBuf {
        temp.0.join(name.file_name())
    }

    /// Bytes and attributes of every file in the folder, to prove nothing changed.
    fn snapshot(folder: &Path) -> Vec<(String, Vec<u8>, u32)> {
        use std::os::windows::fs::MetadataExt;
        let mut files: Vec<_> = std::fs::read_dir(folder)
            .unwrap()
            .flatten()
            .map(|entry| {
                let attributes = entry.metadata().unwrap().file_attributes();
                let bytes = std::fs::read(entry.path()).unwrap_or_default();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    bytes,
                    attributes,
                )
            })
            .collect();
        files.sort();
        files
    }

    #[test]
    fn retention_keeps_newest_and_deletes_older() {
        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let backups = make_backups(&dir, 5);
        let deleted = apply_protected_retention(&dir, &backups[4], 3).unwrap();
        assert_eq!(deleted, backups[..2].to_vec());
        for old in &backups[..2] {
            assert!(!path_of(&temp, old).exists());
        }
        for (index, kept) in backups.iter().enumerate().skip(2) {
            let path = path_of(&temp, kept);
            assert_eq!(
                std::fs::read(&path).unwrap(),
                format!("backup {}", index + 1).as_bytes()
            );
            assert!(is_readonly(&path));
        }
        assert_eq!(std::fs::read_dir(&temp.0).unwrap().count(), 3);
        drop(dir);
        clear_readonly(&temp.0);
    }

    #[test]
    fn retention_under_limit_deletes_nothing() {
        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let backups = make_backups(&dir, 2);
        let before = snapshot(&temp.0);
        let deleted = apply_protected_retention(&dir, &backups[1], 3).unwrap();
        assert!(deleted.is_empty());
        assert_eq!(snapshot(&temp.0), before);
        drop(dir);
        clear_readonly(&temp.0);
    }

    #[test]
    fn retention_never_deletes_just_created() {
        use island_core::config_state::ChangeFingerprint;
        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let backups = make_backups(&dir, 2);
        let planted = ProtectedBackupName::new(900, 2, ChangeFingerprint::of(b"planted")).unwrap();
        let planted_path = path_of(&temp, &planted);
        std::fs::write(&planted_path, b"planted").unwrap();
        let deleted = apply_protected_retention(&dir, &backups[1], 1).unwrap();
        assert_eq!(deleted, vec![backups[0].clone()]);
        let survivor = path_of(&temp, &backups[1]);
        assert_eq!(std::fs::read(&survivor).unwrap(), b"backup 2");
        assert!(is_readonly(&survivor));
        assert_eq!(std::fs::read(&planted_path).unwrap(), b"planted");
        drop(dir);
        clear_readonly(&temp.0);
    }

    #[test]
    fn retention_ignores_unparsed_names() {
        let temp = Temp::new();
        let unrelated = [
            "config.toml",
            "config.toml.bak",
            "config.toml.1-2-3.tmp",
            "config.protected.000001.v2.0000000000000000.BAK",
            "config.protected.000001.v2.0000000000000000.bak.tmp",
            "notes.txt",
        ];
        for name in unrelated {
            std::fs::write(temp.0.join(name), name.as_bytes()).unwrap();
        }
        let dir = ConfigDir::open(&temp.0).unwrap();
        let backups = make_backups(&dir, 4);
        let before: Vec<_> = snapshot(&temp.0)
            .into_iter()
            .filter(|(name, _, _)| unrelated.contains(&name.as_str()))
            .collect();
        assert_eq!(before.len(), unrelated.len());
        let deleted = apply_protected_retention(&dir, &backups[3], 1).unwrap();
        assert_eq!(deleted, backups[..3].to_vec());
        let after: Vec<_> = snapshot(&temp.0)
            .into_iter()
            .filter(|(name, _, _)| unrelated.contains(&name.as_str()))
            .collect();
        assert_eq!(after, before);
        assert_eq!(
            std::fs::read_dir(&temp.0).unwrap().count(),
            unrelated.len() + 1
        );
        drop(dir);
        clear_readonly(&temp.0);
    }

    #[test]
    fn retention_missing_just_created_deletes_nothing() {
        use island_core::config_state::ChangeFingerprint;
        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        make_backups(&dir, 4);
        let missing = ProtectedBackupName::new(5, 2, ChangeFingerprint::of(b"absent")).unwrap();
        let before = snapshot(&temp.0);
        let error = apply_protected_retention(&dir, &missing, 1).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(snapshot(&temp.0), before);
        drop(dir);
        clear_readonly(&temp.0);
    }

    #[test]
    fn retention_refuses_directory_with_backup_name() {
        use island_core::config_state::ChangeFingerprint;
        let temp = Temp::new();
        let folder_name = ProtectedBackupName::new(1, 2, ChangeFingerprint::of(b"dir")).unwrap();
        let folder = path_of(&temp, &folder_name);
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("inside.txt"), b"inside").unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let backups = make_backups(&dir, 4);
        assert_eq!(backups[0].sequence(), 2);
        let before = snapshot(&temp.0);
        let error = apply_protected_retention(&dir, &backups[3], 3).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(folder.is_dir());
        assert_eq!(std::fs::read(folder.join("inside.txt")).unwrap(), b"inside");
        assert_eq!(snapshot(&temp.0), before);
        drop(dir);
        clear_readonly(&temp.0);
    }

    #[test]
    fn retention_failure_leaves_file_and_later_files() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let backups = make_backups(&dir, 5);
        let before = snapshot(&temp.0);
        // Read and write sharing only, so deleting the oldest excess backup is refused.
        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 2)
            .open(path_of(&temp, &backups[0]))
            .unwrap();
        let result = apply_protected_retention(&dir, &backups[4], 3);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().raw_os_error(), Some(32));
        assert!(is_readonly(&path_of(&temp, &backups[0])));
        assert_eq!(snapshot(&temp.0), before);
        drop(held);
        drop(dir);
        clear_readonly(&temp.0);
    }

    #[test]
    fn retention_delete_failure_restores_readonly() {
        let temp = Temp::new();
        let dir = ConfigDir::open(&temp.0).unwrap();
        let backups = make_backups(&dir, 5);
        let before = snapshot(&temp.0);
        super::super::config_fs::FAIL_DELETE_DISPOSITION.with(|flag| flag.set(true));
        let error = apply_protected_retention(&dir, &backups[4], 3).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
        assert!(is_readonly(&path_of(&temp, &backups[0])));
        assert_eq!(snapshot(&temp.0), before);
        let deleted = apply_protected_retention(&dir, &backups[4], 3).unwrap();
        assert_eq!(deleted, backups[..2].to_vec());
        drop(dir);
        clear_readonly(&temp.0);
    }
}
