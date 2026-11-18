//! Replaces the primary configuration file through a verified draft. All operations are relative to the held configuration directory handle.

use std::io;
use std::path::Path;

use super::config_fs::{ConfigDir, Draft, VerifiedFile};
use super::settings::{name, read_verified};

#[allow(
    dead_code,
    reason = "available to callers handling optimistic conflicts"
)]
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
}
