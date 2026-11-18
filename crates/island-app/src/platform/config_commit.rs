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

#[cfg(test)]
thread_local! {
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
    fn matching_primary_is_replaced() {
        let temp = Temp::new();
        let path = temp.0.join("config.toml");
        std::fs::write(&path, b"old").unwrap();
        let dir = ConfigDir::open(&temp.0).unwrap();
        commit_primary(&dir, &path, "new", Some(b"old")).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }
}
