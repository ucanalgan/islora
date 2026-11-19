//! Pure configuration observations and runtime write decisions.

use std::{fmt, io};

use island_core::config_state::{
    ChangeFingerprint, PrimaryObservation, RuntimeMode, StartupOutcome,
};
use island_core::migration::{MigrationError, MigrationOutcome};

use super::config_commit;
use super::settings::{classify_ignoring_unsupported, prepared_write, read_verified};

/// Classify the result of reading the primary configuration.
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
pub(crate) fn effective_outcome(outcome: StartupOutcome) -> RuntimeMode {
    match outcome {
        StartupOutcome::Migrate { .. } => RuntimeMode::LegacyCompatible,
        other => other.mode(),
    }
}

/// Return whether the runtime mode permits settings writes.
pub(crate) fn writes_allowed(mode: &RuntimeMode) -> bool {
    match mode {
        RuntimeMode::Normal | RuntimeMode::LegacyCompatible => true,
        RuntimeMode::ReadOnly | RuntimeMode::Recovery => false,
    }
}

/// Identifies a write refused by the runtime mode.
#[derive(Debug)]
pub(crate) struct ModeRefused;

impl fmt::Display for ModeRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("settings are read-only in this mode")
    }
}

impl std::error::Error for ModeRefused {}

/// Construct a typed runtime-mode refusal.
pub(crate) fn mode_refused() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, ModeRefused)
}

/// Recognize a typed runtime-mode refusal.
pub(crate) fn is_mode_refused(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|cause| cause.downcast_ref::<ModeRefused>().is_some())
}

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use super::config_fs::{ConfigDir, VerifiedFile, WriterLock};
use super::settings;
use island_core::config_state::{ArtifactInventory, ProtectedBackupName, plan_startup};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn next_sequence() -> u64 {
    SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1
}

static RELOAD_REQUEST: AtomicBool = AtomicBool::new(false);

pub(crate) fn take_reload_request() -> bool {
    RELOAD_REQUEST.swap(false, Ordering::SeqCst)
}

static STORE: Mutex<Option<Arc<ConfigSnapshot>>> = Mutex::new(None);

pub(crate) struct ConfigSnapshot {
    sequence: u64,
    text: Option<String>,
    mode: RuntimeMode,
    storage_ready: bool,
}

// Readers and writers switch to these accessors in 2C-2 S3.
impl ConfigSnapshot {
    #[allow(dead_code, reason = "readers will use this in 2C-2 S5")]
    pub(crate) fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }
    #[allow(dead_code, reason = "readers will use this in 2C-2 S5")]
    pub(crate) fn mode(&self) -> &RuntimeMode {
        &self.mode
    }
    pub(crate) fn writes_allowed(&self) -> bool {
        self.storage_ready && self.text.is_some() && writes_allowed(&self.mode)
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

fn inventory(dir: &ConfigDir) -> ArtifactInventory {
    let mut inventory = ArtifactInventory::default();
    if let Ok(entries) = dir.entries() {
        for name in entries {
            if name.eq_ignore_ascii_case("config.toml.bak") {
                inventory.rolling_backup = true;
            }
            if let Some(parsed) = name.to_str().and_then(ProtectedBackupName::parse) {
                inventory.protected_backups.push(parsed);
            }
            // Count failure-record presence as evidence without interpreting its contents.
            if name.eq_ignore_ascii_case(config_commit::FAILURE_RECORD_NAME) {
                inventory.leftover_drafts += 1;
            }
            if settings::is_stale_draft_name(&name) {
                inventory.leftover_drafts += 1;
            }
        }
    }
    if let island_core::config_state::StoredFailure::Valid(record) =
        config_commit::read_failure(dir)
    {
        inventory.recorded_failure = Some(record.source);
    }
    inventory
}

fn observe_folder(folder: io::Result<PathBuf>, storage_ready: bool) -> ConfigSnapshot {
    observe_folder_at(folder, storage_ready, next_sequence())
}

fn observe_folder_at(
    folder: io::Result<PathBuf>,
    storage_ready: bool,
    sequence: u64,
) -> ConfigSnapshot {
    let result = folder.and_then(|path| ConfigDir::open(&path).map_err(io::Error::from));
    let dir = match result {
        Ok(dir) => dir,
        Err(error) => {
            let observation = observe(Err(error));
            return ConfigSnapshot {
                sequence,
                text: None,
                mode: effective_outcome(plan_startup(&observation, &ArtifactInventory::default())),
                storage_ready: false,
            };
        }
    };
    observe_directory(&dir, storage_ready, sequence, &mut || {})
}

fn observe_directory(
    dir: &ConfigDir,
    storage_ready: bool,
    sequence: u64,
    checkpoint: &mut impl FnMut(),
) -> ConfigSnapshot {
    checkpoint();
    let inventory = inventory(dir);
    checkpoint();
    let read = VerifiedFile::open(dir, settings::PRIMARY_NAME.as_ref())
        .map_err(io::Error::from)
        .and_then(|mut file| settings::read_verified(&mut file));
    let (observation, text) = match read {
        Ok(text) => (observe(Ok(text.clone())), Some(text)),
        Err(error) => (observe(Err(error)), None),
    };
    checkpoint();
    let snapshot = ConfigSnapshot {
        sequence,
        text,
        mode: effective_outcome(plan_startup(&observation, &inventory)),
        storage_ready,
    };
    checkpoint();
    snapshot
}

fn bootstrap_prepared(
    base: &Path,
    result: io::Result<(bool, WriterLock)>,
    mut checkpoint: impl FnMut(),
) -> ConfigSnapshot {
    match result {
        Ok((_first_run, lock)) => {
            let snapshot =
                observe_directory(lock.directory(), true, next_sequence(), &mut checkpoint);
            drop(lock);
            snapshot
        }
        Err(error) => {
            eprintln!("settings storage could not be prepared: {:?}", error.kind());
            observe_folder(Ok(base.join("Islora")), false)
        }
    }
}

#[cfg(test)]
pub(crate) fn bootstrap_at(base: &Path) -> ConfigSnapshot {
    bootstrap_prepared(base, settings::prepare_storage_locked(base), || {})
}

fn install_once(
    slot: &Mutex<Option<Arc<ConfigSnapshot>>>,
    make: impl FnOnce() -> ConfigSnapshot,
) -> Arc<ConfigSnapshot> {
    if let Some(snapshot) = slot.lock().unwrap_or_else(PoisonError::into_inner).clone() {
        return snapshot;
    }
    // make runs outside the slot mutex; startup calls bootstrap once on one thread,
    // so two preparations cannot overlap.
    let snapshot = Arc::new(make());
    let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
    Arc::clone(guard.get_or_insert(snapshot))
}

fn config_folder() -> io::Result<PathBuf> {
    std::env::var_os("APPDATA")
        .map(|base| PathBuf::from(base).join("Islora"))
        .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
}

pub(crate) fn bootstrap() -> Arc<ConfigSnapshot> {
    install_once(&STORE, || {
        let snapshot = match std::env::var_os("APPDATA") {
            Some(base) => {
                let base = PathBuf::from(base);
                bootstrap_prepared(
                    &base,
                    settings::prepare_storage_locked_recorded(&base),
                    || {},
                )
            }
            None => observe_folder(config_folder(), false),
        };
        eprintln!("settings startup mode: {:?}", snapshot.mode);
        snapshot
    })
}

#[allow(dead_code, reason = "readers will use this in 2C-2 S5")]
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

fn install_newer(
    slot: &Mutex<Option<Arc<ConfigSnapshot>>>,
    snapshot: Arc<ConfigSnapshot>,
) -> Arc<ConfigSnapshot> {
    let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(current) = guard.as_ref() {
        if snapshot.sequence > current.sequence {
            *guard = Some(Arc::clone(&snapshot));
            return snapshot;
        }
        return Arc::clone(current);
    }
    snapshot
}

/// Acceptance row #26: live recovery is not implemented yet.
/// A missing primary on refresh was removed mid-session, so it is Recovery, never a new install.
fn refresh_mode(observation: &PrimaryObservation, inventory: &ArtifactInventory) -> RuntimeMode {
    if matches!(observation, PrimaryObservation::Missing) {
        RuntimeMode::Recovery
    } else {
        effective_outcome(plan_startup(observation, inventory))
    }
}

fn observe_refresh(
    folder: io::Result<PathBuf>,
    previous: Option<&ConfigSnapshot>,
    sequence: u64,
) -> ConfigSnapshot {
    // A preparation failure remains refused until restart.
    let mut storage_ready = previous.is_some_and(|snapshot| snapshot.storage_ready);
    let result = folder.and_then(|path| ConfigDir::open(&path).map_err(io::Error::from));
    let (read, inventory) = match result {
        Ok(dir) => {
            let inventory = inventory(&dir);
            let read = VerifiedFile::open(&dir, settings::PRIMARY_NAME.as_ref())
                .map_err(io::Error::from)
                .and_then(|mut file| settings::read_verified(&mut file));
            (read, inventory)
        }
        Err(error) => {
            storage_ready = false;
            (Err(error), ArtifactInventory::default())
        }
    };
    let (observation, text) = match read {
        Ok(text) => (observe(Ok(text.clone())), Some(text)),
        Err(error) => {
            eprintln!("settings refresh read failed: {:?}", error.kind());
            (observe(Err(error)), None)
        }
    };
    ConfigSnapshot {
        sequence,
        text,
        mode: refresh_mode(&observation, &inventory),
        storage_ready,
    }
}

fn install_refresh(
    slot: &Mutex<Option<Arc<ConfigSnapshot>>>,
    mut snapshot: ConfigSnapshot,
) -> Arc<ConfigSnapshot> {
    let mut guard = slot.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(current) = guard.as_ref() {
        if snapshot.sequence <= current.sequence {
            return Arc::clone(current);
        }
        if snapshot.text.is_none() {
            snapshot.text.clone_from(&current.text);
        }
        let snapshot = Arc::new(snapshot);
        *guard = Some(Arc::clone(&snapshot));
        return snapshot;
    }
    Arc::new(snapshot)
}

fn refresh_folder_at(
    slot: &Mutex<Option<Arc<ConfigSnapshot>>>,
    folder: io::Result<PathBuf>,
    sequence: u64,
) -> Arc<ConfigSnapshot> {
    let previous = slot.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let snapshot = observe_refresh(folder, previous.as_deref(), sequence);
    if previous.is_some() {
        install_refresh(slot, snapshot)
    } else {
        Arc::new(snapshot)
    }
}

fn refresh_folder(
    slot: &Mutex<Option<Arc<ConfigSnapshot>>>,
    folder: io::Result<PathBuf>,
) -> Arc<ConfigSnapshot> {
    refresh_folder_at(slot, folder, next_sequence())
}

#[cfg(test)]
fn refresh_in(slot: &Mutex<Option<Arc<ConfigSnapshot>>>, base: &Path) -> Arc<ConfigSnapshot> {
    refresh_folder(slot, Ok(base.join("Islora")))
}

/// Refresh the installed snapshot; bootstrap must run first.
// The store is used only on the UI thread, so refreshes cannot overlap.
// Installs are still ordered by the write sequence.
pub(crate) fn refresh() -> Arc<ConfigSnapshot> {
    refresh_folder(&STORE, config_folder())
}

fn read_locked(lock: &WriterLock, path: &Path) -> io::Result<String> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "configuration has no name"))?;
    let mut file = VerifiedFile::open(lock.directory(), std::ffi::OsStr::new(name))
        .map_err(io::Error::from)?;
    read_verified(&mut file)
}

#[cfg(test)]
pub(crate) fn try_remember_path(path: &Path, key: &str, value: &str) -> io::Result<()> {
    remember_core(path, key, value, None).0
}

fn remember_core(
    path: &Path,
    key: &str,
    value: &str,
    expected_snapshot: Option<&str>,
) -> (io::Result<()>, Committed, bool) {
    let mut conflict = false;
    let mut committed = Committed::None;
    let result = (|| {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "configuration has no parent")
        })?;
        let lock = WriterLock::acquire(parent).map_err(io::Error::from)?;
        let text = read_locked(&lock, path)?;
        if expected_snapshot.is_some_and(|expected| expected.as_bytes() != text.as_bytes()) {
            conflict = true;
            return Err(io::Error::other(super::config_fs::PrimaryChanged));
        }
        let updated = prepared_write(&text, key, value)?;
        match config_commit::commit_primary(lock.directory(), path, &updated, Some(text.as_bytes()))
        {
            Ok(()) => {
                committed = Committed::Text(updated, next_sequence());
                Ok(())
            }
            Err(error) => {
                conflict |= config_commit::is_primary_changed(&error);
                if config_commit::is_commit_unverified(&error) {
                    committed = Committed::Unverified(next_sequence());
                }
                Err(error)
            }
        }
    })();
    (result, committed, conflict)
}

#[cfg(test)]
pub(crate) fn locked_update_at<T>(
    path: &Path,
    action: impl FnOnce(&str, &mut dyn FnMut(&str, &str) -> io::Result<()>) -> io::Result<T>,
) -> io::Result<T> {
    locked_update_core(path, None, action).0
}

enum Committed {
    None,
    Text(String, u64),
    Unverified(u64),
}

fn locked_update_core<T>(
    path: &Path,
    expected_snapshot: Option<&str>,
    action: impl FnOnce(&str, &mut dyn FnMut(&str, &str) -> io::Result<()>) -> io::Result<T>,
) -> (io::Result<T>, Committed, bool) {
    let mut conflict = false;
    let mut committed = Committed::None;
    let result = (|| {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "configuration has no parent")
        })?;
        let lock = WriterLock::acquire(parent).map_err(io::Error::from)?;
        let text = read_locked(&lock, path)?;
        if expected_snapshot.is_some_and(|expected| expected.as_bytes() != text.as_bytes()) {
            conflict = true;
            return Err(io::Error::other(super::config_fs::PrimaryChanged));
        }
        let mut current = text.clone();
        let mut unverified = false;
        let result = action(&text, &mut |key, value| {
            if unverified {
                return Err(io::Error::other(config_commit::CommitUnverified));
            }
            let updated = prepared_write(&current, key, value)?;
            if let Err(error) = config_commit::commit_primary(
                lock.directory(),
                path,
                &updated,
                Some(current.as_bytes()),
            ) {
                conflict |= config_commit::is_primary_changed(&error);
                unverified = config_commit::is_commit_unverified(&error);
                return Err(error);
            }
            current = updated;
            committed = Committed::Text(current.clone(), next_sequence());
            Ok(())
        });
        if unverified {
            committed = Committed::Unverified(next_sequence());
        }
        result
    })();
    (result, committed, conflict)
}

fn gate(slot: &Mutex<Option<Arc<ConfigSnapshot>>>) -> io::Result<Arc<ConfigSnapshot>> {
    slot.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .filter(|snapshot| snapshot.writes_allowed())
        .ok_or_else(mode_refused)
}

fn finish(
    slot: &Mutex<Option<Arc<ConfigSnapshot>>>,
    path: &Path,
    previous: &ConfigSnapshot,
    committed: Committed,
) {
    match committed {
        Committed::None => {}
        Committed::Unverified(sequence) => {
            refresh_folder_at(
                slot,
                path.parent()
                    .map(Path::to_path_buf)
                    .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound)),
                sequence,
            );
        }
        Committed::Text(text, sequence) => {
            let inventory = path
                .parent()
                .and_then(|p| ConfigDir::open(p).ok())
                .map_or_else(ArtifactInventory::default, |dir| inventory(&dir));
            let mode = effective_outcome(plan_startup(&observe(Ok(text.clone())), &inventory));
            let snapshot = Arc::new(ConfigSnapshot {
                sequence,
                text: Some(text),
                mode,
                storage_ready: previous.storage_ready,
            });
            install_newer(slot, snapshot);
        }
    }
}

fn run_update<T>(
    slot: &Mutex<Option<Arc<ConfigSnapshot>>>,
    path: impl FnOnce() -> io::Result<PathBuf>,
    write: impl FnOnce(&Path, Option<&str>) -> (io::Result<T>, Committed, bool),
) -> (io::Result<T>, bool) {
    let mut conflict = false;
    let result = (|| {
        let previous = gate(slot)?;
        let path = path()?;
        let (result, committed, write_conflict) = write(&path, previous.text());
        conflict = write_conflict;
        finish(slot, &path, &previous, committed);
        result
    })();
    (result, conflict)
}

/// Instance form skips the global storage_writable check.
#[cfg(test)]
fn update_in(
    slot: &Mutex<Option<Arc<ConfigSnapshot>>>,
    path: &Path,
    key: &str,
    value: &str,
) -> io::Result<()> {
    run_update(
        slot,
        || Ok(path.to_path_buf()),
        |path, expected| remember_core(path, key, value, expected),
    )
    .0
}

/// Instance form skips the global storage_writable check.
#[cfg(test)]
fn update_with_in<T>(
    slot: &Mutex<Option<Arc<ConfigSnapshot>>>,
    path: &Path,
    action: impl FnOnce(&str, &mut dyn FnMut(&str, &str) -> io::Result<()>) -> io::Result<T>,
) -> io::Result<T> {
    run_update(
        slot,
        || Ok(path.to_path_buf()),
        |path, expected| locked_update_core(path, expected, action),
    )
    .0
}

fn writable_path() -> io::Result<PathBuf> {
    if !settings::storage_writable() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "configuration storage is read-only",
        ));
    }
    settings::path()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "APPDATA is unavailable"))
}

fn request_reload_on_conflict<T>(flag: &AtomicBool, result: &io::Result<T>, conflict: bool) {
    if conflict
        || result
            .as_ref()
            .is_err_and(config_commit::is_primary_changed)
    {
        flag.store(true, Ordering::SeqCst);
    }
}

pub(crate) fn update(key: &str, value: &str) -> io::Result<()> {
    let (result, conflict) = run_update(&STORE, writable_path, |path, expected| {
        remember_core(path, key, value, expected)
    });
    request_reload_on_conflict(&RELOAD_REQUEST, &result, conflict);
    result
}

pub(crate) fn update_with<T>(
    action: impl FnOnce(&str, &mut dyn FnMut(&str, &str) -> io::Result<()>) -> io::Result<T>,
) -> io::Result<T> {
    let (result, conflict) = run_update(&STORE, writable_path, |path, expected| {
        locked_update_core(path, expected, action)
    });
    request_reload_on_conflict(&RELOAD_REQUEST, &result, conflict);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use island_core::config_state::{ArtifactInventory, ProtectedBackupName, plan_startup};

    #[test]
    fn unverified_commit_rereads_disk() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base))));
        let path = base.join("Islora/config.toml");
        let target = path.clone();
        super::super::config_commit::AFTER_PRIMARY_RENAME.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(target, b"config_version = \"100\"\n").unwrap()
            }));
        });
        let error = update_in(&slot, &path, "media_layout", "compact").unwrap_err();
        assert!(config_commit::is_commit_unverified(&error));
        assert_eq!(
            text_in(&slot).unwrap().as_bytes(),
            std::fs::read(path).unwrap()
        );
        assert_eq!(
            slot.lock().unwrap().as_ref().unwrap().mode,
            RuntimeMode::ReadOnly
        );
    }

    #[test]
    fn refused_on_empty_slot() {
        let slot = Mutex::new(None);
        assert!(is_mode_refused(
            &update_in(&slot, Path::new("unused"), "x", "y").unwrap_err()
        ));
    }

    #[test]
    fn refused_when_storage_not_ready() {
        let slot = Mutex::new(Some(Arc::new(ConfigSnapshot {
            storage_ready: false,
            ..sample()
        })));
        let called = std::cell::Cell::new(false);
        let err = update_with_in(&slot, Path::new("unused"), |_, _| {
            called.set(true);
            Ok(())
        })
        .unwrap_err();
        assert!(is_mode_refused(&err));
        assert!(!called.get());
    }

    #[test]
    fn refused_in_read_only_writes_nothing() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("config.toml");
        std::fs::write(&path, b"config_version = \"100\"\n").unwrap();
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base))));
        let bytes = std::fs::read(&path).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(is_mode_refused(
            &update_in(&slot, &path, "x", "y").unwrap_err()
        ));
        assert_eq!(bytes, std::fs::read(&path).unwrap());
        assert_eq!(
            modified,
            std::fs::metadata(&path).unwrap().modified().unwrap()
        );
    }

    #[test]
    fn refused_in_recovery_runs_no_action() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("config.toml.bak"), b"invalid backup").unwrap();
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base))));
        let called = std::cell::Cell::new(false);
        let err = update_with_in(&slot, &folder.join("config.toml"), |_, _| {
            called.set(true);
            Ok(())
        })
        .unwrap_err();
        assert!(is_mode_refused(&err));
        assert!(!called.get());
    }

    #[test]
    fn write_through_sets_committed_text() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base))));
        let path = base.join("Islora/config.toml");
        let old = slot.lock().unwrap().clone().unwrap();
        update_in(&slot, &path, "media_layout", "compact").unwrap();
        let new = slot.lock().unwrap().clone().unwrap();
        assert!(!Arc::ptr_eq(&old, &new));
        assert_eq!(new.mode, RuntimeMode::Normal);
        assert_eq!(
            text_in(&slot).unwrap().as_bytes(),
            std::fs::read(&path).unwrap()
        );
    }

    #[test]
    fn write_through_after_partial_transaction() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base))));
        let path = base.join("Islora/config.toml");
        let result: io::Result<()> = update_with_in(&slot, &path, |_, save| {
            save("media_layout", "full")?;
            Err(io::Error::other("after save"))
        });
        assert!(result.is_err());
        assert_eq!(
            text_in(&slot).unwrap().as_bytes(),
            std::fs::read(&path).unwrap()
        );
    }

    #[test]
    fn failed_write_keeps_snapshot() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base))));
        let path = base.join("Islora/config.toml");
        let old = slot.lock().unwrap().clone().unwrap();
        let result: io::Result<()> =
            update_with_in(&slot, &path, |_, _| Err(io::Error::other("before save")));
        assert!(result.is_err());
        assert!(Arc::ptr_eq(&old, slot.lock().unwrap().as_ref().unwrap()));
        let error = update_with_in(&slot, &path, |_, save| {
            std::fs::write(&path, b"external change").unwrap();
            save("media_layout", "compact")
        })
        .unwrap_err();
        assert!(config_commit::is_primary_changed(&error));
        assert!(Arc::ptr_eq(&old, slot.lock().unwrap().as_ref().unwrap()));
    }

    #[test]
    fn older_write_through_never_replaces_newer() {
        let slot = Mutex::new(Some(Arc::new(sample())));
        let newer = Arc::new(ConfigSnapshot {
            sequence: 2,
            text: Some("new".into()),
            ..sample()
        });
        let older = Arc::new(ConfigSnapshot {
            sequence: 1,
            text: Some("old".into()),
            ..sample()
        });
        install_newer(&slot, newer);
        install_newer(&slot, older);
        assert_eq!(text_in(&slot).as_deref(), Some("new"));
    }

    #[test]
    fn legacy_compatible_still_writes() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let a = settings::tests::temp_base();
        let b = settings::tests::temp_base();
        for base in [&a, &b] {
            std::fs::create_dir(base.join("Islora")).unwrap();
            std::fs::write(base.join("Islora/config.toml"), b"config_version = \"1\"\n").unwrap();
        }
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&a))));
        let path = a.join("Islora/config.toml");
        let other = b.join("Islora/config.toml");
        update_in(&slot, &path, "media_layout", "compact").unwrap();
        assert_eq!(
            slot.lock().unwrap().as_ref().unwrap().mode,
            RuntimeMode::LegacyCompatible
        );
        try_remember_path(&other, "media_layout", "compact").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), std::fs::read(other).unwrap());
    }

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
    fn older_refresh_does_not_replace_newer_install() {
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        let slot = Mutex::new(Some(Arc::new(sample())));
        let newer = install_newer(
            &slot,
            Arc::new(ConfigSnapshot {
                sequence: 2,
                ..sample()
            }),
        );
        let refreshed = refresh_folder_at(&slot, Ok(folder), 1);
        assert!(Arc::ptr_eq(&newer, &refreshed));
        assert!(Arc::ptr_eq(&newer, slot.lock().unwrap().as_ref().unwrap()));
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
        for _ in 0..2 {
            let snapshot = refresh_in(&slot, &base);
            assert!(!snapshot.storage_ready && !snapshot.writes_allowed());
        }
    }

    #[test]
    fn refresh_failure_keeps_previous_snapshot() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let slot = Mutex::new(None);
        install_once(&slot, || bootstrap_at(&base));
        let old = text_in(&slot);
        std::fs::write(
            base.join("Islora/config.toml"),
            vec![b'x'; island_core::migration::MAX_CONFIG_BYTES + 1],
        )
        .unwrap();
        refresh_in(&slot, &base);
        assert_eq!(text_in(&slot), old);
    }

    #[test]
    fn refresh_to_future_becomes_read_only() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base))));
        let path = base.join("Islora/config.toml");
        let future = "config_version = \"100\"\n";
        std::fs::write(&path, future).unwrap();
        let snapshot = refresh_in(&slot, &base);
        assert_eq!(snapshot.mode(), &RuntimeMode::ReadOnly);
        assert!(!snapshot.writes_allowed());
        assert_eq!(text_in(&slot).as_deref(), Some(future));
    }

    #[test]
    fn refresh_back_to_current_is_normal() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base))));
        let path = base.join("Islora/config.toml");
        let current = text_in(&slot).unwrap();
        std::fs::write(&path, "config_version = \"100\"\n").unwrap();
        assert_eq!(refresh_in(&slot, &base).mode(), &RuntimeMode::ReadOnly);
        std::fs::write(&path, &current).unwrap();
        let snapshot = refresh_in(&slot, &base);
        assert_eq!(snapshot.mode(), &RuntimeMode::Normal);
        assert!(snapshot.writes_allowed());
        assert_eq!(text_in(&slot), Some(current));
    }

    #[test]
    fn refresh_unreadable_primary_keeps_previous_text() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base))));
        let old = text_in(&slot).unwrap();
        let path = base.join("Islora/config.toml");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        refresh_in(&slot, &base);
        assert_eq!(text_in(&slot), Some(old));
    }

    #[test]
    fn failed_refresh_keeps_text_installed_in_between() {
        let initial = Arc::new(ConfigSnapshot {
            text: Some("initial".into()),
            ..sample()
        });
        let slot = Mutex::new(Some(Arc::clone(&initial)));
        let refresh = observe_refresh(Err(io::ErrorKind::NotFound.into()), Some(&initial), 2);
        install_newer(
            &slot,
            Arc::new(ConfigSnapshot {
                sequence: 1,
                text: Some("written".into()),
                ..sample()
            }),
        );
        install_refresh(&slot, refresh);
        assert_eq!(text_in(&slot).as_deref(), Some("written"));
    }

    #[test]
    fn refresh_missing_primary_is_recovery() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base))));
        let old = text_in(&slot);
        std::fs::remove_file(base.join("Islora/config.toml")).unwrap();
        let snapshot = refresh_in(&slot, &base);
        assert_eq!(snapshot.mode(), &RuntimeMode::Recovery);
        assert!(!snapshot.writes_allowed());
        assert_eq!(text_in(&slot), old);
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
        // Opening a regular file as the configuration folder fails.
        let file = base.join("not_a_folder");
        std::fs::write(&file, b"file").unwrap();
        let snapshot = observe_folder(Ok(file.clone()), true);
        assert!(!snapshot.storage_ready && !snapshot.writes_allowed());
        let slot = Mutex::new(Some(Arc::new(ConfigSnapshot {
            sequence: 0,
            text: Some("previous".into()),
            mode: RuntimeMode::Normal,
            storage_ready: true,
        })));
        let refreshed = refresh_in(&slot, &file);
        assert!(!refreshed.storage_ready && !refreshed.writes_allowed());
        assert_eq!(refreshed.text(), Some("previous"));
    }

    fn write_fixture() -> (settings::tests::Temp, Mutex<Option<Arc<ConfigSnapshot>>>) {
        let base = settings::tests::temp_base();
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base))));
        (base, slot)
    }

    #[test]
    fn update_after_external_edit_is_conflict() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (base, slot) = write_fixture();
        let path = base.join("Islora/config.toml");
        let old = slot.lock().unwrap().clone().unwrap();
        let external = "config_version = \"2\"\nexternal = \"yes\"\n";
        std::fs::write(&path, external).unwrap();
        let error = update_in(&slot, &path, "new_key", "value").unwrap_err();
        assert!(config_commit::is_primary_changed(&error));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), external);
        assert!(Arc::ptr_eq(&old, slot.lock().unwrap().as_ref().unwrap()));
    }

    #[test]
    fn update_with_conflict_runs_no_action() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (base, slot) = write_fixture();
        let path = base.join("Islora/config.toml");
        std::fs::write(&path, "config_version = \"2\"\nexternal = \"yes\"\n").unwrap();
        let mut called = false;
        let error = update_with_in(&slot, &path, |_, _| {
            called = true;
            Ok(())
        })
        .unwrap_err();
        assert!(config_commit::is_primary_changed(&error));
        assert!(!called);
    }

    #[test]
    fn conflict_then_reload_then_retry_succeeds() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (base, slot) = write_fixture();
        let path = base.join("Islora/config.toml");
        std::fs::write(&path, "config_version = \"2\"\nexternal = \"yes\"\n").unwrap();
        assert!(config_commit::is_primary_changed(
            &update_in(&slot, &path, "new_key", "value").unwrap_err()
        ));
        refresh_in(&slot, &base);
        update_in(&slot, &path, "new_key", "value").unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("external = \"yes\""));
        assert!(text.contains("new_key = \"value\""));
    }

    #[test]
    fn multi_save_uses_last_committed() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (base, slot) = write_fixture();
        let path = base.join("Islora/config.toml");
        update_with_in(&slot, &path, |_, save| {
            save("first", "one")?;
            save("second", "two")
        })
        .unwrap();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("first = \"one\"") && text.contains("second = \"two\""));
        assert_eq!(text_in(&slot).as_deref(), Some(text.as_str()));
    }

    #[test]
    fn unchanged_file_writes_as_before() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (base, slot) = write_fixture();
        let path = base.join("Islora/config.toml");
        let other = settings::tests::temp_base();
        bootstrap_at(&other);
        let copy = other.join("Islora/config.toml");
        std::fs::copy(&path, &copy).unwrap();
        update_in(&slot, &path, "new_key", "value").unwrap();
        try_remember_path(&copy, "new_key", "value").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), std::fs::read(copy).unwrap());
    }

    #[test]
    fn no_snapshot_text_refuses_write_even_if_file_is_readable() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (base, _) = write_fixture();
        let slot = Mutex::new(Some(Arc::new(sample())));
        let path = base.join("Islora/config.toml");
        let before = std::fs::read(&path).unwrap();
        assert!(is_mode_refused(
            &update_in(&slot, &path, "new_key", "value").unwrap_err()
        ));
        assert_eq!(std::fs::read(path).unwrap(), before);
    }

    #[test]
    fn conflict_inside_action_reports_conflict() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (base, slot) = write_fixture();
        let path = base.join("Islora/config.toml");
        let (result, conflict) = run_update(
            &slot,
            || Ok(path.clone()),
            |path, expected| {
                locked_update_core(path, expected, |_, save| {
                    std::fs::write(path, "config_version = \"2\"\nexternal = \"yes\"\n")?;
                    let error = save("new_key", "value").unwrap_err();
                    assert!(config_commit::is_primary_changed(&error));
                    Ok(())
                })
            },
        );
        assert!(result.is_ok());
        assert!(conflict);
    }

    #[test]
    fn reload_request_helper_records_conflicts_once() {
        let flag = AtomicBool::new(false);
        request_reload_on_conflict(&flag, &Ok::<(), io::Error>(()), true);
        assert!(flag.swap(false, Ordering::SeqCst));
        assert!(!flag.swap(false, Ordering::SeqCst));
        request_reload_on_conflict(&flag, &Err::<(), _>(io::Error::other("ordinary")), false);
        assert!(!flag.swap(false, Ordering::SeqCst));
        request_reload_on_conflict(
            &flag,
            &Err::<(), _>(io::Error::other(super::super::config_fs::PrimaryChanged)),
            false,
        );
        assert!(flag.swap(false, Ordering::SeqCst));
    }

    fn sample() -> ConfigSnapshot {
        ConfigSnapshot {
            sequence: 0,
            text: None,
            mode: RuntimeMode::Normal,
            storage_ready: true,
        }
    }

    #[test]
    fn bootstrap_missing_primary_still_new_install() {
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
    fn bootstrap_holds_one_lock_through_snapshot() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let result = settings::prepare_storage_locked(&base);
        let folder = base.join("Islora");
        let mut checkpoints = 0;
        let snapshot = bootstrap_prepared(&base, result, || {
            checkpoints += 1;
            let contender = folder.clone();
            assert!(
                std::thread::spawn(move || WriterLock::acquire(&contender).is_err())
                    .join()
                    .unwrap()
            );
        });
        assert_eq!(checkpoints, 4);
        assert!(snapshot.writes_allowed());
        assert!(
            std::thread::spawn(move || WriterLock::acquire(&folder).is_ok())
                .join()
                .unwrap()
        );
    }

    #[test]
    fn busy_bootstrap_observes_without_preparation_or_writes() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("config.toml");
        let bytes = b"config_version = \"1\"\n";
        std::fs::write(&path, bytes).unwrap();
        let lock = WriterLock::acquire(&folder).unwrap();
        let other = base.to_path_buf();
        let snapshot = std::thread::spawn(move || bootstrap_at(&other))
            .join()
            .unwrap();
        assert_eq!(snapshot.text(), Some(std::str::from_utf8(bytes).unwrap()));
        assert_eq!(snapshot.mode(), &RuntimeMode::LegacyCompatible);
        assert!(!snapshot.storage_ready && !snapshot.writes_allowed());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        drop(lock);
    }

    #[test]
    fn empty_folder_observes_missing_before_preparation() {
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        let dir = ConfigDir::open(&folder).unwrap();
        let read = VerifiedFile::open(&dir, "config.toml".as_ref())
            .map_err(io::Error::from)
            .and_then(|mut file| settings::read_verified(&mut file));
        assert_eq!(observe(read), PrimaryObservation::Missing);
        let snapshot = observe_folder(Ok(folder), true);
        assert_eq!(snapshot.mode(), &RuntimeMode::Normal);
        assert!(snapshot.text().is_none());
    }

    #[test]
    fn inventory_reports_recorded_failure() {
        use island_core::config_state::{ChangeFingerprint, FailureRecord, FailureStage};
        let base = settings::tests::temp_base();
        let record = FailureRecord {
            source: ChangeFingerprint::of(b"source"),
            from_schema: 1,
            stage: FailureStage::Backup,
            attempts: 1,
        };
        std::fs::write(
            base.join(config_commit::FAILURE_RECORD_NAME),
            record.encode(),
        )
        .unwrap();
        let found = inventory(&ConfigDir::open(&base).unwrap());
        assert_eq!(found.recorded_failure, Some(record.source));
    }

    #[test]
    fn corrupt_record_not_a_fingerprint() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join(config_commit::FAILURE_RECORD_NAME), b"corrupt").unwrap();
        assert_eq!(
            inventory(&ConfigDir::open(&folder).unwrap()).recorded_failure,
            None
        );
        let snapshot = bootstrap_at(&base);
        assert_eq!(snapshot.mode(), &RuntimeMode::Recovery);
        assert!(!folder.join("config.toml").exists());
    }

    #[test]
    fn inventory_rolling_backup_is_case_insensitive() {
        let lower = settings::tests::temp_base();
        let upper = settings::tests::temp_base();
        std::fs::write(lower.join("config.toml.bak"), b"backup").unwrap();
        std::fs::write(upper.join("CONFIG.TOML.BAK"), b"backup").unwrap();
        let lower_inventory = inventory(&ConfigDir::open(&lower).unwrap());
        let upper_inventory = inventory(&ConfigDir::open(&upper).unwrap());
        assert!(lower_inventory.rolling_backup);
        assert_eq!(upper_inventory, lower_inventory);
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
        let path = folder.join("config.toml");
        let bytes = vec![b'x'; island_core::migration::MAX_CONFIG_BYTES + 1];
        std::fs::write(&path, &bytes).unwrap();
        let snap = bootstrap_at(&base);
        assert_eq!(snap.mode(), &RuntimeMode::LegacyCompatible);
        assert!(snap.text().is_none());
        assert!(snap.storage_ready);
        assert!(!snap.writes_allowed());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
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
    fn missing_primary_with_protected_backup_refuses_template() {
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
        let snap = bootstrap_at(&base);
        assert!(!snap.storage_ready && !snap.writes_allowed());
        assert_eq!(snap.mode(), &RuntimeMode::Recovery);
        assert!(!folder.join("config.toml").exists());
        assert!(snap.text().is_none());
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
            sequence: 0,
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
        for (version, schema) in [
            (100_u64, 100),
            (u64::from(u32::MAX), u32::MAX),
            (u64::from(u32::MAX) + 1, u32::MAX),
            (u64::MAX, u32::MAX),
        ] {
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
            (
                PrimaryObservation::Missing,
                [
                    RuntimeMode::Normal,
                    RuntimeMode::Recovery,
                    RuntimeMode::Recovery,
                    RuntimeMode::Recovery,
                    RuntimeMode::Recovery,
                ],
            ),
            (
                PrimaryObservation::Unreadable,
                [const { RuntimeMode::LegacyCompatible }; 5],
            ),
            (
                PrimaryObservation::Invalid { fingerprint },
                [const { RuntimeMode::LegacyCompatible }; 5],
            ),
            (
                PrimaryObservation::Future {
                    schema: 3,
                    fingerprint,
                },
                [const { RuntimeMode::ReadOnly }; 5],
            ),
            (
                PrimaryObservation::Current { fingerprint },
                [const { RuntimeMode::Normal }; 5],
            ),
            (
                PrimaryObservation::Legacy {
                    schema: 1,
                    fingerprint,
                },
                [const { RuntimeMode::LegacyCompatible }; 5],
            ),
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
        let mut saw_migrate = false;
        for (observation, expected_modes) in &observations {
            for (inventory, expected) in inventories.iter().zip(expected_modes) {
                let plan = plan_startup(observation, inventory);
                saw_migrate |= matches!(observation, PrimaryObservation::Legacy { .. })
                    && matches!(plan, StartupOutcome::Migrate { .. });
                assert_eq!(&effective_outcome(plan), expected);
            }
        }
        assert!(saw_migrate);
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
