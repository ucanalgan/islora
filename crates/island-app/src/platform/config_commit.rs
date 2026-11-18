//! Replaces the primary configuration file through a verified draft. All operations are relative to the held configuration directory handle.

use std::io;
use std::path::Path;

use super::config_fs::{ConfigDir, Draft, VerifiedFile};
use super::settings::{name, read_verified};

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
    static FAIL_VERIFICATION_AT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
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

/// The primary configuration file name, whose security a protected backup copies.
#[allow(dead_code, reason = "called by migration in 2C-3")]
const PRIMARY_NAME: &str = "config.toml";

/// Writes one protected backup of `text` next to the primary under a core-built unique name.
/// The file is created exclusively (an existing name is an `AlreadyExists` error, never
/// overwritten), written through, read back and compared, then marked read-only. Any failure
/// deletes only the file this call created. Retention is not handled here.
#[allow(dead_code, reason = "called by migration in 2C-3")]
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
}
