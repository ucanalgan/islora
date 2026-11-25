//! Pure configuration observations and runtime write decisions.

use std::{fmt, io};

use island_core::config_state::{
    ChangeFingerprint, LegacyReason, PrimaryObservation, RuntimeMode, StartupOutcome,
};
use island_core::migration::{MigrationError, MigrationOutcome};

use super::config_commit;
use super::config_migrate::{self, FallbackReason, MigrationReport};
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
    migration_report: Arc<Mutex<Option<MigrationReport>>>,
}

// Readers and writers switch to these accessors in 2C-2 S3.
impl ConfigSnapshot {
    #[allow(dead_code, reason = "report consumers arrive after S6b")]
    pub(crate) fn take_migration_report(&self) -> Option<MigrationReport> {
        self.migration_report
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
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
                migration_report: Arc::new(Mutex::new(None)),
            };
        }
    };
    observe_directory(&dir, storage_ready, sequence, &mut || {}).0
}

fn observe_directory(
    dir: &ConfigDir,
    storage_ready: bool,
    sequence: u64,
    checkpoint: &mut impl FnMut(),
) -> (ConfigSnapshot, StartupOutcome) {
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
    let outcome = plan_startup(&observation, &inventory);
    let snapshot = ConfigSnapshot {
        sequence,
        text,
        mode: effective_outcome(outcome.clone()),
        storage_ready,
        migration_report: Arc::new(Mutex::new(None)),
    };
    checkpoint();
    (snapshot, outcome)
}

fn bootstrap_prepared(
    base: &Path,
    result: io::Result<(bool, WriterLock)>,
    migrate: bool,
    mut checkpoint: impl FnMut(),
) -> ConfigSnapshot {
    match result {
        Ok((_first_run, lock)) => {
            let (mut snapshot, outcome) =
                observe_directory(lock.directory(), true, next_sequence(), &mut checkpoint);
            let report = if migrate && matches!(outcome, StartupOutcome::Migrate { .. }) {
                let path = base.join("Islora").join(settings::PRIMARY_NAME);
                let result = config_migrate::migrate_locked(
                    &lock,
                    &path,
                    snapshot
                        .text
                        .as_ref()
                        .expect("migration requires text")
                        .as_bytes(),
                );
                let report = match result {
                    Ok(report) => report,
                    Err(error) => {
                        eprintln!("migration I/O failed: {:?}", error.kind());
                        MigrationReport::Fallback(FallbackReason::Io(error.kind()))
                    }
                };
                // Re-observe under the same lock even on fallback or an I/O error.
                // A failed read produces no text and therefore refuses writes.
                snapshot =
                    observe_directory(lock.directory(), true, next_sequence(), &mut checkpoint).0;
                report
            } else if migrate
                && matches!(
                    outcome,
                    StartupOutcome::LegacyCompatible {
                        reason: LegacyReason::RetrySuppressed
                    }
                )
            {
                MigrationReport::Fallback(FallbackReason::Suppressed)
            } else {
                MigrationReport::NotNeeded
            };
            *snapshot
                .migration_report
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(report);
            snapshot
        }
        Err(error) => {
            eprintln!("settings storage could not be prepared: {:?}", error.kind());
            let reason = if error.kind() == io::ErrorKind::WouldBlock
                || matches!(error.raw_os_error(), Some(32 | 33))
            {
                FallbackReason::Busy
            } else {
                FallbackReason::Unprepared
            };
            let snapshot = observe_folder(Ok(base.join("Islora")), false);
            *snapshot
                .migration_report
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(MigrationReport::Fallback(reason));
            snapshot
        }
    }
}

#[cfg(test)]
pub(crate) fn bootstrap_at(base: &Path, migrate: bool) -> ConfigSnapshot {
    bootstrap_prepared(base, settings::prepare_storage_locked(base), migrate, || {})
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
                    false,
                    || {},
                )
            }
            None => {
                let snapshot = observe_folder(config_folder(), false);
                *snapshot
                    .migration_report
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) =
                    Some(MigrationReport::Fallback(FallbackReason::Unprepared));
                snapshot
            }
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
        migration_report: Arc::new(Mutex::new(None)),
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
        snapshot.migration_report = Arc::clone(&current.migration_report);
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
                migration_report: Arc::clone(&previous.migration_report),
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
    use super::super::config_fs::TEST_WRITER_SERIAL;
    use super::*;
    use island_core::config::{contains_setting, read_value};
    use island_core::config_state::{
        ArtifactInventory, ChangeFingerprint, FailureRecord, FailureStage, MAX_PROTECTED_SEQUENCE,
        ProtectedBackupName, plan_startup,
    };
    use std::fs;

    #[test]
    fn unverified_commit_rereads_disk() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base, false))));
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
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base, false))));
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
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base, false))));
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
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base, false))));
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
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base, false))));
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
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base, false))));
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
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&a, false))));
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
        install_once(&slot, || bootstrap_at(&base, false));
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
        let old = install_once(&slot, || bootstrap_at(&base, false));
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
        install_once(&slot, || bootstrap_at(&base, false));
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
        install_once(&slot, || bootstrap_at(&base, false));
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
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base, false))));
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
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base, false))));
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
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base, false))));
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
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base, false))));
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
        bootstrap_at(&base, false);
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
            migration_report: Arc::new(Mutex::new(None)),
        })));
        let refreshed = refresh_in(&slot, &file);
        assert!(!refreshed.storage_ready && !refreshed.writes_allowed());
        assert_eq!(refreshed.text(), Some("previous"));
    }

    fn write_fixture() -> (settings::tests::Temp, Mutex<Option<Arc<ConfigSnapshot>>>) {
        let base = settings::tests::temp_base();
        let slot = Mutex::new(Some(Arc::new(bootstrap_at(&base, false))));
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
        bootstrap_at(&other, false);
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
            migration_report: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn opt_in_migrates_once_and_reports_once() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        let _cleanup = RemoveProtectedReadOnly(folder.clone());
        let path = folder.join("config.toml");
        let legacy = b"ui_density = \"minimal\"\n";
        std::fs::write(&path, legacy).unwrap();
        let first = bootstrap_at(&base, true);
        let report = first.take_migration_report().unwrap();
        let MigrationReport::Migrated { backup } = report else {
            panic!("expected migration")
        };
        assert_eq!(
            std::fs::read(folder.join(backup.file_name())).unwrap(),
            legacy
        );
        assert_eq!(first.mode(), &RuntimeMode::Normal);
        assert_eq!(
            first.text().unwrap().as_bytes(),
            std::fs::read(&path).unwrap()
        );
        assert!(first.writes_allowed());
        assert_eq!(first.take_migration_report(), None);
        let migrated = std::fs::read(&path).unwrap();
        let second = bootstrap_at(&base, true);
        assert_eq!(
            second.take_migration_report(),
            Some(MigrationReport::NotNeeded)
        );
        assert_eq!(std::fs::read(&path).unwrap(), migrated);
    }

    #[test]
    fn disabled_and_refresh_never_migrate() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        let path = folder.join("config.toml");
        let legacy = b"ui_density = \"minimal\"\n";
        std::fs::write(&path, legacy).unwrap();
        let first = bootstrap_at(&base, false);
        assert_eq!(
            first.take_migration_report(),
            Some(MigrationReport::NotNeeded)
        );
        assert_eq!(first.mode(), &RuntimeMode::LegacyCompatible);
        assert!(first.writes_allowed());
        let slot = Mutex::new(Some(Arc::new(first)));
        let refreshed = refresh_in(&slot, &base);
        assert_eq!(refreshed.text().unwrap().as_bytes(), legacy);
        assert_eq!(std::fs::read(&path).unwrap(), legacy);
        assert!(
            inventory(&ConfigDir::open(&folder).unwrap())
                .protected_backups
                .is_empty()
        );
    }

    // Protected backups are read-only on Windows; restore their attributes before
    // the unique temporary directory is removed, including on a test failure.
    struct RemoveProtectedReadOnly(PathBuf);
    impl Drop for RemoveProtectedReadOnly {
        fn drop(&mut self) {
            if let Ok(entries) = std::fs::read_dir(&self.0) {
                for entry in entries.flatten() {
                    if entry
                        .file_name()
                        .to_str()
                        .and_then(ProtectedBackupName::parse)
                        .is_some()
                        && let Ok(metadata) = entry.metadata()
                    {
                        let mut permissions = metadata.permissions();
                        #[allow(clippy::permissions_set_readonly_false)]
                        permissions.set_readonly(false);
                        let _ = std::fs::set_permissions(entry.path(), permissions);
                    }
                }
            }
        }
    }

    fn assert_no_protected_backup(folder: &Path) {
        assert!(fs::read_dir(folder).unwrap().all(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_str()
                .and_then(ProtectedBackupName::parse)
                .is_none()
        }));
    }

    #[test]
    fn acceptance_a_fresh_install() {
        let _serial = TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        assert!(fs::read_dir(&*base).unwrap().next().is_none());
        let folder = base.join("Islora");
        let _cleanup = RemoveProtectedReadOnly(folder.clone());

        let snapshot = bootstrap_at(&base, true);

        assert_eq!(snapshot.mode(), &RuntimeMode::Normal);
        assert_eq!(
            snapshot.take_migration_report(),
            Some(MigrationReport::NotNeeded)
        );
        assert!(snapshot.writes_allowed());
        let disk = fs::read_to_string(folder.join(settings::PRIMARY_NAME)).unwrap();
        assert_eq!(snapshot.text(), Some(disk.as_str()));
        assert_eq!(read_value(&disk, "config_version").as_deref(), Some("2"));
        assert_no_protected_backup(&folder);
        assert!(!folder.join("config.toml.bak").try_exists().unwrap());
        assert!(
            !folder
                .join(config_commit::FAILURE_RECORD_NAME)
                .try_exists()
                .unwrap()
        );
    }

    fn assert_acceptance_migration(legacy_settings: &str, indicator: &str, media: &str) {
        let _serial = TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        fs::create_dir(&folder).unwrap();
        let _cleanup = RemoveProtectedReadOnly(folder.clone());
        let path = folder.join(settings::PRIMARY_NAME);
        let unrelated = "# Keep this comment\r\ncustom_setting = \"keep me\" # and this one\r\n";
        let legacy = format!("{unrelated}{legacy_settings}");
        fs::write(&path, &legacy).unwrap();

        let snapshot = bootstrap_at(&base, true);

        let report = snapshot.take_migration_report();
        let Some(MigrationReport::Migrated { backup }) = report else {
            panic!("expected a successful migration, got {report:?}");
        };
        let backup_path = folder.join(backup.file_name());
        assert_eq!(fs::read(&backup_path).unwrap(), legacy.as_bytes());
        assert!(fs::metadata(backup_path).unwrap().permissions().readonly());
        assert_eq!(snapshot.mode(), &RuntimeMode::Normal);
        assert!(snapshot.writes_allowed());
        let disk = fs::read_to_string(path).unwrap();
        assert_eq!(snapshot.text(), Some(disk.as_str()));
        assert_eq!(read_value(&disk, "config_version").as_deref(), Some("2"));
        assert!(!contains_setting(&disk, "ui_density"));
        assert_eq!(
            read_value(&disk, "indicator_style").as_deref(),
            Some(indicator)
        );
        assert_eq!(read_value(&disk, "media_layout").as_deref(), Some(media));
        assert!(disk.contains(unrelated));
    }

    #[test]
    fn acceptance_b_minimal_migrates_to_ring_and_compact() {
        assert_acceptance_migration("ui_density = \"minimal\"\r\n", "ring", "compact");
    }

    #[test]
    fn acceptance_c_balanced_migrates_to_edge_and_standard() {
        assert_acceptance_migration("ui_density = \"balanced\"\r\n", "edge", "standard");
    }

    #[test]
    fn acceptance_d_detailed_migrates_to_context_and_informative() {
        assert_acceptance_migration("ui_density = \"detailed\"\r\n", "context", "informative");
    }

    #[test]
    fn acceptance_e_explicit_modern_preference_wins() {
        assert_acceptance_migration(
            "config_version = \"1\"\r\nui_density = \"minimal\"\r\nindicator_style = \"context\"\r\n",
            "context",
            "compact",
        );
        assert_acceptance_migration(
            "config_version = \"1\"\r\nui_density = \"minimal\"\r\nmedia_layout = \"informative\"\r\n",
            "ring",
            "informative",
        );
    }

    fn assert_acceptance_no_migration(
        source: &str,
        mode: RuntimeMode,
        check: impl FnOnce(ConfigSnapshot, &Path),
    ) {
        let _serial = TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        fs::create_dir(&folder).unwrap();
        let _cleanup = RemoveProtectedReadOnly(folder.clone());
        let path = folder.join(settings::PRIMARY_NAME);
        let past = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        {
            let mut file = fs::File::create(&path).unwrap();
            io::Write::write_all(&mut file, source.as_bytes()).unwrap();
            file.set_modified(past).unwrap();
        }
        let modified = fs::metadata(&path).unwrap().modified().unwrap();

        let snapshot = bootstrap_at(&base, true);

        assert_eq!(snapshot.mode(), &mode);
        assert_eq!(
            snapshot.take_migration_report(),
            Some(MigrationReport::NotNeeded)
        );
        assert_eq!(snapshot.text(), Some(source));
        check(snapshot, &path);
        assert_eq!(fs::read(&path).unwrap(), source.as_bytes());
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        assert_no_protected_backup(&folder);
        assert!(!folder.join("config.toml.bak").try_exists().unwrap());
        assert!(
            !folder
                .join(config_commit::FAILURE_RECORD_NAME)
                .try_exists()
                .unwrap()
        );
    }

    #[test]
    fn acceptance_f_current_schema_is_untouched() {
        assert_acceptance_no_migration(
            "# Preserve these bytes\r\nconfig_version = \"2\"\r\nindicator_style = \"ring\"\r\n",
            RuntimeMode::Normal,
            |snapshot, _| assert!(snapshot.writes_allowed()),
        );
    }

    #[test]
    fn acceptance_g_future_schema_refuses_update() {
        assert_acceptance_no_migration(
            "config_version = \"100\"\r\nindicator_style = \"ring\"\r\n",
            RuntimeMode::ReadOnly,
            |snapshot, path| {
                let slot = Mutex::new(Some(Arc::new(snapshot)));
                let error = update_in(&slot, path, "indicator_style", "edge").unwrap_err();
                assert!(is_mode_refused(&error));
            },
        );
    }

    #[test]
    fn acceptance_h_invalid_legacy_value_is_untouched() {
        assert_acceptance_no_migration(
            "config_version = \"1\"\r\nui_density = \"unknown\"\r\n",
            RuntimeMode::LegacyCompatible,
            |_, _| {},
        );
    }

    #[test]
    fn acceptance_i_duplicate_critical_key_is_untouched() {
        assert_acceptance_no_migration(
            "ui_density = \"minimal\"\r\nui_density = \"minimal\"\r\n",
            RuntimeMode::LegacyCompatible,
            |_, _| {},
        );
    }

    const ACCEPTANCE_LEGACY: &str = "ui_density = \"minimal\"\r\ncustom_setting = \"before\"\r\n";
    const ACCEPTANCE_EDITED: &str = "ui_density = \"minimal\"\r\ncustom_setting = \"after\"\r\n";
    const ACCEPTANCE_ROLLING: &str = "# Previous save\r\nui_density = \"detailed\"\r\n";

    fn acceptance_legacy_fixture() -> (settings::tests::Temp, PathBuf) {
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        fs::create_dir(&folder).unwrap();
        fs::write(folder.join(settings::PRIMARY_NAME), ACCEPTANCE_LEGACY).unwrap();
        fs::write(folder.join("config.toml.bak"), ACCEPTANCE_ROLLING).unwrap();
        (base, folder)
    }

    fn acceptance_backup(sequence: u32, source: &str) -> ProtectedBackupName {
        ProtectedBackupName::new(sequence, 1, ChangeFingerprint::of(source.as_bytes())).unwrap()
    }

    fn assert_acceptance_files(folder: &Path, expected: &[(&str, &str)]) {
        use std::collections::BTreeSet;
        let actual: BTreeSet<_> = fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        let names: BTreeSet<_> = expected
            .iter()
            .map(|(name, _)| std::ffi::OsString::from(*name))
            .collect();
        assert_eq!(actual, names);
        for (name, text) in expected {
            let path = folder.join(name);
            assert_eq!(fs::read(&path).unwrap(), text.as_bytes(), "{name}");
            if ProtectedBackupName::parse(name).is_some() {
                assert!(
                    fs::metadata(path).unwrap().permissions().readonly(),
                    "{name}"
                );
            }
        }
    }

    fn assert_acceptance_snapshot(
        snapshot: &ConfigSnapshot,
        mode: RuntimeMode,
        source: &str,
        report: MigrationReport,
        writes_allowed: bool,
    ) {
        assert_eq!(snapshot.mode(), &mode);
        assert_eq!(snapshot.text(), Some(source));
        assert_eq!(snapshot.take_migration_report(), Some(report));
        assert_eq!(snapshot.take_migration_report(), None);
        assert_eq!(snapshot.writes_allowed(), writes_allowed);
    }

    fn acceptance_failure(stage: FailureStage, source: &str, attempts: u32) -> String {
        FailureRecord {
            source: ChangeFingerprint::of(source.as_bytes()),
            from_schema: 1,
            stage,
            attempts,
        }
        .encode()
    }

    fn assert_acceptance_failed_then_suppressed(
        base: &Path,
        folder: &Path,
        stage: FailureStage,
        backup: (&str, &str),
    ) -> ConfigSnapshot {
        let record = acceptance_failure(stage, ACCEPTANCE_LEGACY, 1);
        let expected = [
            (settings::PRIMARY_NAME, ACCEPTANCE_LEGACY),
            ("config.toml.bak", ACCEPTANCE_ROLLING),
            (config_commit::FAILURE_RECORD_NAME, &record),
            backup,
        ];
        let start = |reason| {
            let snapshot = bootstrap_at(base, true);
            assert_acceptance_snapshot(
                &snapshot,
                RuntimeMode::LegacyCompatible,
                ACCEPTANCE_LEGACY,
                MigrationReport::Fallback(reason),
                true,
            );
            assert_acceptance_files(folder, &expected);
            snapshot
        };
        start(FallbackReason::Failed(stage));
        start(FallbackReason::Suppressed)
    }

    fn assert_acceptance_migrated(
        snapshot: &ConfigSnapshot,
        source: &str,
        backup: ProtectedBackupName,
    ) -> String {
        let island_core::migration::MigrationOutcome::Prepared { text } =
            island_core::migration::prepare(source).unwrap()
        else {
            panic!("expected a migratable fixture");
        };
        assert_acceptance_snapshot(
            snapshot,
            RuntimeMode::Normal,
            &text,
            MigrationReport::Migrated { backup },
            true,
        );
        text
    }

    #[test]
    fn acceptance_j_protected_backup_failure_preserves_source() {
        let _serial = TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (base, folder) = acceptance_legacy_fixture();
        let _cleanup = RemoveProtectedReadOnly(folder.clone());
        let backup = acceptance_backup(MAX_PROTECTED_SEQUENCE, ACCEPTANCE_ROLLING).file_name();
        let backup_path = folder.join(&backup);
        fs::write(&backup_path, ACCEPTANCE_ROLLING).unwrap();
        let mut permissions = fs::metadata(&backup_path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&backup_path, permissions).unwrap();

        let snapshot = assert_acceptance_failed_then_suppressed(
            &base,
            &folder,
            FailureStage::Backup,
            (&backup, ACCEPTANCE_ROLLING),
        );
        let slot = Mutex::new(Some(Arc::new(snapshot)));
        update_in(
            &slot,
            &folder.join(settings::PRIMARY_NAME),
            "custom_setting",
            "after",
        )
        .unwrap();
        assert_eq!(text_in(&slot).as_deref(), Some(ACCEPTANCE_EDITED));

        // The edit permits a retry while the exhausted backup sequence still fails.
        let retry = bootstrap_at(&base, true);
        assert_acceptance_snapshot(
            &retry,
            RuntimeMode::LegacyCompatible,
            ACCEPTANCE_EDITED,
            MigrationReport::Fallback(FallbackReason::Failed(FailureStage::Backup)),
            true,
        );
        let record = acceptance_failure(FailureStage::Backup, ACCEPTANCE_EDITED, 2);
        assert_acceptance_files(
            &folder,
            &[
                (settings::PRIMARY_NAME, ACCEPTANCE_EDITED),
                ("config.toml.bak", ACCEPTANCE_LEGACY),
                (config_commit::FAILURE_RECORD_NAME, &record),
                (&backup, ACCEPTANCE_ROLLING),
            ],
        );
    }

    #[test]
    fn acceptance_k_primary_draft_sync_failure_keeps_backups() {
        let _serial = TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (base, folder) = acceptance_legacy_fixture();
        let _cleanup = RemoveProtectedReadOnly(folder.clone());
        let original_backup = acceptance_backup(1, ACCEPTANCE_LEGACY).file_name();
        config_migrate::AFTER_BACKUP.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(|| {
                super::super::config_fs::FAIL_DRAFT_SYNC.with(|flag| flag.set(true));
            }));
        });

        let snapshot = assert_acceptance_failed_then_suppressed(
            &base,
            &folder,
            FailureStage::Commit,
            (&original_backup, ACCEPTANCE_LEGACY),
        );
        let slot = Mutex::new(Some(Arc::new(snapshot)));
        update_in(
            &slot,
            &folder.join(settings::PRIMARY_NAME),
            "custom_setting",
            "after",
        )
        .unwrap();
        assert_eq!(text_in(&slot).as_deref(), Some(ACCEPTANCE_EDITED));

        let retry = bootstrap_at(&base, true);
        let backup = acceptance_backup(2, ACCEPTANCE_EDITED);
        let migrated = assert_acceptance_migrated(&retry, ACCEPTANCE_EDITED, backup.clone());
        assert_acceptance_files(
            &folder,
            &[
                (settings::PRIMARY_NAME, &migrated),
                ("config.toml.bak", ACCEPTANCE_EDITED),
                (&original_backup, ACCEPTANCE_LEGACY),
                (&backup.file_name(), ACCEPTANCE_EDITED),
            ],
        );
    }

    #[test]
    fn acceptance_l_interrupted_migration_never_promotes_a_draft() {
        let _serial = TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for primary_survived in [true, false] {
            let (base, folder) = acceptance_legacy_fixture();
            let _cleanup = RemoveProtectedReadOnly(folder.clone());
            let earlier_backup = {
                let lock = WriterLock::acquire(&folder).unwrap();
                config_commit::create_protected_backup(lock.directory(), ACCEPTANCE_LEGACY, 1)
                    .unwrap()
            };
            let uncommitted = "config_version = \"2\"\r\ncustom_setting = \"uncommitted\"\r\n";
            fs::write(folder.join("config.toml.new"), uncommitted).unwrap();
            fs::write(folder.join("config.toml.9-8-7.tmp"), uncommitted).unwrap();
            let source = if primary_survived {
                ACCEPTANCE_LEGACY
            } else {
                fs::remove_file(folder.join(settings::PRIMARY_NAME)).unwrap();
                ACCEPTANCE_ROLLING
            };

            let restarted = bootstrap_at(&base, true);
            let backup = acceptance_backup(2, source);
            let migrated = assert_acceptance_migrated(&restarted, source, backup.clone());
            assert_acceptance_files(
                &folder,
                &[
                    (settings::PRIMARY_NAME, &migrated),
                    ("config.toml.bak", source),
                    ("config.toml.new", uncommitted),
                    (&earlier_backup.file_name(), ACCEPTANCE_LEGACY),
                    (&backup.file_name(), source),
                ],
            );
        }
    }

    #[test]
    fn acceptance_m_contenders_cannot_write_during_migration() {
        let _serial = TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (base, folder) = acceptance_legacy_fixture();
        let _cleanup = RemoveProtectedReadOnly(folder.clone());
        let writer = Mutex::new(Some(Arc::new(bootstrap_at(&base, false))));
        let contender_base = base.to_path_buf();
        let contender_folder = folder.clone();
        let backup = acceptance_backup(1, ACCEPTANCE_LEGACY);
        let protected = contender_folder.join(backup.file_name());
        config_migrate::AFTER_BACKUP.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::thread::spawn(move || {
                    let busy = bootstrap_at(&contender_base, true);
                    assert_acceptance_snapshot(
                        &busy,
                        RuntimeMode::LegacyCompatible,
                        ACCEPTANCE_LEGACY,
                        MigrationReport::Fallback(FallbackReason::Busy),
                        false,
                    );
                    let path = contender_folder.join(settings::PRIMARY_NAME);
                    let error = update_with_in(&writer, &path, |_, _| -> io::Result<()> {
                        panic!("the contending writer must never start its action");
                    })
                    .unwrap_err();
                    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
                    assert_eq!(text_in(&writer).as_deref(), Some(ACCEPTANCE_LEGACY));
                    assert_eq!(fs::read(path).unwrap(), ACCEPTANCE_LEGACY.as_bytes());
                    assert_eq!(
                        fs::read(contender_folder.join("config.toml.bak")).unwrap(),
                        ACCEPTANCE_ROLLING.as_bytes()
                    );
                    assert_eq!(fs::read(protected).unwrap(), ACCEPTANCE_LEGACY.as_bytes());
                    assert!(
                        !contender_folder
                            .join(config_commit::FAILURE_RECORD_NAME)
                            .try_exists()
                            .unwrap()
                    );
                })
                .join()
                .unwrap();
            }));
        });

        let first = bootstrap_at(&base, true);
        let migrated = assert_acceptance_migrated(&first, ACCEPTANCE_LEGACY, backup.clone());
        let backup_name = backup.file_name();
        let expected = [
            (settings::PRIMARY_NAME, migrated.as_str()),
            ("config.toml.bak", ACCEPTANCE_LEGACY),
            (backup_name.as_str(), ACCEPTANCE_LEGACY),
        ];
        assert_acceptance_files(&folder, &expected);
        let retry = bootstrap_at(&base, true);
        assert_acceptance_snapshot(
            &retry,
            RuntimeMode::Normal,
            &migrated,
            MigrationReport::NotNeeded,
            true,
        );
        assert_acceptance_files(&folder, &expected);
    }

    #[test]
    fn acceptance_n_edit_during_migration_aborts_and_can_retry() {
        let _serial = TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (base, folder) = acceptance_legacy_fixture();
        let _cleanup = RemoveProtectedReadOnly(folder.clone());
        let path = folder.join(settings::PRIMARY_NAME);
        let earlier_backup = acceptance_backup(1, ACCEPTANCE_LEGACY).file_name();
        config_migrate::AFTER_BACKUP.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::write(path, ACCEPTANCE_EDITED).unwrap()
            }));
        });

        let first = bootstrap_at(&base, true);
        assert_acceptance_snapshot(
            &first,
            RuntimeMode::LegacyCompatible,
            ACCEPTANCE_EDITED,
            MigrationReport::Fallback(FallbackReason::SourceChanged),
            true,
        );
        assert_acceptance_files(
            &folder,
            &[
                (settings::PRIMARY_NAME, ACCEPTANCE_EDITED),
                ("config.toml.bak", ACCEPTANCE_ROLLING),
                (&earlier_backup, ACCEPTANCE_LEGACY),
            ],
        );

        let retry = bootstrap_at(&base, true);
        let backup = acceptance_backup(2, ACCEPTANCE_EDITED);
        let migrated = assert_acceptance_migrated(&retry, ACCEPTANCE_EDITED, backup.clone());
        assert_acceptance_files(
            &folder,
            &[
                (settings::PRIMARY_NAME, &migrated),
                ("config.toml.bak", ACCEPTANCE_EDITED),
                (&earlier_backup, ACCEPTANCE_LEGACY),
                (&backup.file_name(), ACCEPTANCE_EDITED),
            ],
        );
    }

    fn acceptance_current() -> String {
        let island_core::migration::MigrationOutcome::Prepared { text } =
            island_core::migration::prepare(ACCEPTANCE_LEGACY).unwrap()
        else {
            panic!("expected a migratable fixture");
        };
        text
    }

    fn acceptance_write_protected(folder: &Path, sequence: u32, source: &str) -> String {
        let name = acceptance_backup(sequence, source).file_name();
        let path = folder.join(&name);
        fs::write(&path, source).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(path, permissions).unwrap();
        name
    }

    fn acceptance_assert_recovery(snapshot: ConfigSnapshot, path: &Path) {
        assert_eq!(snapshot.mode(), &RuntimeMode::Recovery);
        assert_eq!(snapshot.text(), None);
        assert!(!snapshot.writes_allowed());
        assert_eq!(
            snapshot.take_migration_report(),
            Some(MigrationReport::Fallback(FallbackReason::Unprepared))
        );
        let slot = Mutex::new(Some(Arc::new(snapshot)));
        assert!(is_mode_refused(
            &update_in(&slot, path, "indicator_style", "edge").unwrap_err()
        ));
        let mut calls = 0;
        let error = update_with_in(&slot, path, |_, _| {
            calls += 1;
            Ok(())
        })
        .unwrap_err();
        assert!(is_mode_refused(&error));
        assert_eq!(calls, 0);
    }

    #[test]
    fn acceptance_o_trusted_rolling_backup_restores_beside_protected_backup() {
        let _serial = TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        fs::create_dir(&folder).unwrap();
        let _cleanup = RemoveProtectedReadOnly(folder.clone());
        let protected = acceptance_write_protected(&folder, 1, ACCEPTANCE_LEGACY);
        let current = acceptance_current();
        fs::write(folder.join("config.toml.bak"), &current).unwrap();

        let snapshot = bootstrap_at(&base, true);
        assert_acceptance_snapshot(
            &snapshot,
            RuntimeMode::Normal,
            &current,
            MigrationReport::NotNeeded,
            true,
        );
        assert_acceptance_files(
            &folder,
            &[
                (settings::PRIMARY_NAME, &current),
                ("config.toml.bak", &current),
                (&protected, ACCEPTANCE_LEGACY),
            ],
        );
        let slot = Mutex::new(Some(Arc::new(snapshot)));
        update_in(
            &slot,
            &folder.join(settings::PRIMARY_NAME),
            "indicator_style",
            "edge",
        )
        .unwrap();
    }

    #[test]
    fn acceptance_o_legacy_rolling_backup_restores_then_migrates() {
        let _serial = TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        fs::create_dir(&folder).unwrap();
        let _cleanup = RemoveProtectedReadOnly(folder.clone());
        let old = acceptance_write_protected(&folder, 1, ACCEPTANCE_ROLLING);
        fs::write(folder.join("config.toml.bak"), ACCEPTANCE_LEGACY).unwrap();

        let snapshot = bootstrap_at(&base, true);
        let backup = acceptance_backup(2, ACCEPTANCE_LEGACY);
        let migrated = assert_acceptance_migrated(&snapshot, ACCEPTANCE_LEGACY, backup.clone());
        assert_acceptance_files(
            &folder,
            &[
                (settings::PRIMARY_NAME, &migrated),
                ("config.toml.bak", ACCEPTANCE_LEGACY),
                (&old, ACCEPTANCE_ROLLING),
                (&backup.file_name(), ACCEPTANCE_LEGACY),
            ],
        );
    }

    #[test]
    fn acceptance_p_ambiguous_artifacts_refuse_restore() {
        let _serial = TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for name in [
            "config.toml.bak.new",
            "config.toml.recover",
            "config.pre-migration-1.bak",
        ] {
            let base = settings::tests::temp_base();
            let folder = base.join("Islora");
            fs::create_dir(&folder).unwrap();
            let current = acceptance_current();
            let ambiguous = "different artifact contents\n";
            fs::write(folder.join("config.toml.bak"), &current).unwrap();
            fs::write(folder.join(name), ambiguous).unwrap();
            let expected = [("config.toml.bak", current.as_str()), (name, ambiguous)];
            for _ in 0..2 {
                acceptance_assert_recovery(
                    bootstrap_at(&base, true),
                    &folder.join(settings::PRIMARY_NAME),
                );
                assert_acceptance_files(&folder, &expected);
            }
        }
    }

    #[test]
    fn acceptance_q_recovery_evidence_without_rolling_backup_stays_recovery() {
        let _serial = TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        fs::create_dir(&folder).unwrap();
        let _cleanup = RemoveProtectedReadOnly(folder.clone());
        let protected = acceptance_write_protected(&folder, 1, ACCEPTANCE_LEGACY);
        let failure = acceptance_failure(FailureStage::Commit, ACCEPTANCE_LEGACY, 1);
        fs::write(folder.join(config_commit::FAILURE_RECORD_NAME), &failure).unwrap();
        let expected = [
            (protected.as_str(), ACCEPTANCE_LEGACY),
            (config_commit::FAILURE_RECORD_NAME, failure.as_str()),
        ];
        for _ in 0..2 {
            acceptance_assert_recovery(
                bootstrap_at(&base, true),
                &folder.join(settings::PRIMARY_NAME),
            );
            assert_acceptance_files(&folder, &expected);
        }
    }

    #[test]
    fn acceptance_q_untrusted_rolling_backup_is_not_restored() {
        let _serial = TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for source in [
            "config_version = \"99\"\r\nindicator_style = \"ring\"\r\n",
            "ui_density = \"minimal\"\r\nui_density = \"minimal\"\r\n",
        ] {
            let base = settings::tests::temp_base();
            let folder = base.join("Islora");
            fs::create_dir(&folder).unwrap();
            fs::write(folder.join("config.toml.bak"), source).unwrap();
            acceptance_assert_recovery(
                bootstrap_at(&base, true),
                &folder.join(settings::PRIMARY_NAME),
            );
            assert_acceptance_files(&folder, &[("config.toml.bak", source)]);
        }
    }

    #[derive(Clone, Copy)]
    enum ReobserveCase {
        SourceChanged,
        Verify,
        Io,
    }

    fn assert_reobserved(case: ReobserveCase, unreadable: bool) {
        use island_core::config_state::FailureStage;
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        let _cleanup = RemoveProtectedReadOnly(folder.clone());
        let path = folder.join("config.toml");
        let original = b"ui_density = \"minimal\"\n";
        std::fs::write(&path, original).unwrap();
        let replacement = if unreadable {
            vec![b'x'; island_core::migration::MAX_CONFIG_BYTES + 1]
        } else {
            b"config_version = \"100\"\n".to_vec()
        };
        let changed_path = path.clone();
        let replacement_for_hook = replacement.clone();
        match case {
            ReobserveCase::SourceChanged => {
                super::super::config_migrate::AFTER_BACKUP.with(|hook| {
                    *hook.borrow_mut() = Some(Box::new(move || {
                        std::fs::write(changed_path, replacement_for_hook).unwrap();
                    }));
                });
            }
            ReobserveCase::Verify => {
                super::super::config_commit::AFTER_PRIMARY_RENAME.with(|hook| {
                    *hook.borrow_mut() = Some(Box::new(move || {
                        std::fs::write(changed_path, replacement_for_hook).unwrap();
                    }));
                });
            }
            ReobserveCase::Io => {}
        }
        let result = settings::prepare_storage_locked(&base);
        assert!(result.is_ok());
        let mut checkpoints = 0;
        let snapshot = bootstrap_prepared(&base, result, true, || {
            checkpoints += 1;
            if matches!(case, ReobserveCase::Io) && checkpoints == 4 {
                std::fs::remove_file(&path).unwrap();
            }
            if matches!(case, ReobserveCase::Io) && checkpoints == 5 {
                std::fs::write(&path, &replacement).unwrap();
            }
            // Check both sides of the second inventory/read/snapshot sequence.
            if checkpoints == 5 || checkpoints == 8 {
                let contender = folder.clone();
                assert!(
                    std::thread::spawn(move || WriterLock::acquire(&contender).is_err())
                        .join()
                        .unwrap()
                );
            }
        });
        assert_eq!(checkpoints, 8);
        let expected = match case {
            ReobserveCase::SourceChanged => FallbackReason::SourceChanged,
            ReobserveCase::Verify => FallbackReason::Failed(FailureStage::Verify),
            ReobserveCase::Io => FallbackReason::Io(io::ErrorKind::NotFound),
        };
        assert_eq!(
            snapshot.take_migration_report(),
            Some(MigrationReport::Fallback(expected))
        );
        assert_eq!(snapshot.take_migration_report(), None);
        assert_eq!(std::fs::read(&path).unwrap(), replacement);
        if unreadable {
            assert_eq!(snapshot.text(), None);
        } else {
            assert_eq!(snapshot.text().unwrap().as_bytes(), replacement);
            assert_eq!(snapshot.mode(), &RuntimeMode::ReadOnly);
        }
        assert_ne!(
            snapshot.text().map(str::as_bytes),
            Some(original.as_slice())
        );
        assert!(!snapshot.writes_allowed());
        let slot = Mutex::new(Some(Arc::new(snapshot)));
        assert!(is_mode_refused(
            &update_in(&slot, &path, "x", "y").unwrap_err()
        ));
        assert_eq!(std::fs::read(&path).unwrap(), replacement);
    }

    #[test]
    fn source_changed_reobserves_readable_and_unreadable_under_lock() {
        assert_reobserved(ReobserveCase::SourceChanged, false);
        assert_reobserved(ReobserveCase::SourceChanged, true);
    }

    #[test]
    fn verify_failure_reobserves_readable_and_unreadable_under_lock() {
        assert_reobserved(ReobserveCase::Verify, false);
        assert_reobserved(ReobserveCase::Verify, true);
    }

    #[test]
    fn source_removed_reobserves_new_protected_backup_inventory() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let folder = base.join("Islora");
        std::fs::create_dir(&folder).unwrap();
        let _cleanup = RemoveProtectedReadOnly(folder.clone());
        let path = folder.join("config.toml");
        std::fs::write(&path, b"ui_density = \"minimal\"\n").unwrap();
        let changed_path = path.clone();
        super::super::config_migrate::AFTER_BACKUP.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::remove_file(changed_path).unwrap();
            }));
        });
        let result = settings::prepare_storage_locked(&base);
        assert!(result.is_ok());
        let mut checkpoints = 0;
        let snapshot = bootstrap_prepared(&base, result, true, || {
            checkpoints += 1;
            if checkpoints == 5 || checkpoints == 8 {
                let contender = folder.clone();
                assert!(
                    std::thread::spawn(move || WriterLock::acquire(&contender).is_err())
                        .join()
                        .unwrap()
                );
            }
        });
        assert_eq!(checkpoints, 8);
        assert_eq!(
            snapshot.take_migration_report(),
            Some(MigrationReport::Fallback(FallbackReason::SourceChanged))
        );
        assert_eq!(snapshot.mode(), &RuntimeMode::Recovery);
        assert_eq!(snapshot.text(), None);
        assert!(!snapshot.writes_allowed());
        assert!(!path.exists());
        assert_eq!(
            inventory(&ConfigDir::open(&folder).unwrap())
                .protected_backups
                .len(),
            1
        );
    }

    #[test]
    fn migration_io_error_reobserves_readable_and_unreadable_under_lock() {
        assert_reobserved(ReobserveCase::Io, false);
        assert_reobserved(ReobserveCase::Io, true);
    }

    #[test]
    fn migration_report_follows_installed_snapshots_and_is_one_shot() {
        for consume_before in [false, true] {
            let initial = Arc::new(sample());
            *initial.migration_report.lock().unwrap() =
                Some(MigrationReport::Fallback(FallbackReason::Busy));
            let slot = Mutex::new(Some(Arc::clone(&initial)));
            if consume_before {
                assert_eq!(
                    initial.take_migration_report(),
                    Some(MigrationReport::Fallback(FallbackReason::Busy))
                );
            }
            let observed = observe_refresh(Err(io::ErrorKind::NotFound.into()), Some(&initial), 1);
            let refreshed = install_refresh(&slot, observed);
            finish(
                &slot,
                Path::new("config.toml"),
                &refreshed,
                Committed::Text("config_version = \"2\"\n".into(), 2),
            );
            let updated = slot.lock().unwrap().as_ref().unwrap().clone();
            assert!(!Arc::ptr_eq(&initial, &refreshed));
            assert!(!Arc::ptr_eq(&refreshed, &updated));
            if !consume_before {
                assert_eq!(
                    initial.take_migration_report(),
                    Some(MigrationReport::Fallback(FallbackReason::Busy))
                );
            }
            assert_eq!(initial.take_migration_report(), None);
            assert_eq!(refreshed.take_migration_report(), None);
            assert_eq!(updated.take_migration_report(), None);
        }
    }

    #[test]
    fn distinct_snapshots_do_not_share_bootstrap_report() {
        let first = sample();
        let second = sample();
        *first.migration_report.lock().unwrap() = Some(MigrationReport::NotNeeded);
        *second.migration_report.lock().unwrap() = Some(MigrationReport::NotNeeded);
        assert_eq!(
            first.take_migration_report(),
            Some(MigrationReport::NotNeeded)
        );
        assert_eq!(first.take_migration_report(), None);
        assert_eq!(
            second.take_migration_report(),
            Some(MigrationReport::NotNeeded)
        );
        assert_eq!(second.take_migration_report(), None);
    }

    #[test]
    fn migration_report_survives_refresh_and_is_shared_with_old_snapshot() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for consume_before in [false, true] {
            let base = settings::tests::temp_base();
            let first = Arc::new(bootstrap_at(&base, false));
            let slot = Mutex::new(Some(Arc::clone(&first)));
            if consume_before {
                assert_eq!(
                    first.take_migration_report(),
                    Some(MigrationReport::NotNeeded)
                );
            }
            let refreshed = refresh_in(&slot, &base);
            assert!(!Arc::ptr_eq(&first, &refreshed));
            if !consume_before {
                assert_eq!(
                    refreshed.take_migration_report(),
                    Some(MigrationReport::NotNeeded)
                );
            }
            assert_eq!(first.take_migration_report(), None);
            assert_eq!(refreshed.take_migration_report(), None);
        }
    }

    #[test]
    fn migration_report_survives_update_and_is_shared_with_old_snapshot() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for consume_before in [false, true] {
            let base = settings::tests::temp_base();
            let first = Arc::new(bootstrap_at(&base, false));
            let path = base.join("Islora/config.toml");
            let slot = Mutex::new(Some(Arc::clone(&first)));
            if consume_before {
                assert_eq!(
                    first.take_migration_report(),
                    Some(MigrationReport::NotNeeded)
                );
            }
            update_in(&slot, &path, "media_layout", "compact").unwrap();
            let updated = slot.lock().unwrap().as_ref().unwrap().clone();
            assert!(!Arc::ptr_eq(&first, &updated));
            if !consume_before {
                assert_eq!(
                    updated.take_migration_report(),
                    Some(MigrationReport::NotNeeded)
                );
            }
            assert_eq!(first.take_migration_report(), None);
            assert_eq!(updated.take_migration_report(), None);
        }
    }

    #[test]
    fn migration_report_survives_failed_refresh_and_update() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let first = Arc::new(bootstrap_at(&base, false));
        let slot = Mutex::new(Some(Arc::clone(&first)));
        let missing = base.join("missing");
        let refreshed = refresh_in(&slot, &missing);
        assert!(!refreshed.writes_allowed());
        assert!(update_in(&slot, &missing.join("config.toml"), "x", "y").is_err());
        assert_eq!(
            refreshed.take_migration_report(),
            Some(MigrationReport::NotNeeded)
        );
        assert_eq!(first.take_migration_report(), None);
        assert_eq!(refreshed.take_migration_report(), None);
    }

    #[test]
    fn migration_report_survives_unverified_update_refresh() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let first = Arc::new(bootstrap_at(&base, false));
        let path = base.join("Islora/config.toml");
        let slot = Mutex::new(Some(Arc::clone(&first)));
        let target = path.clone();
        super::super::config_commit::AFTER_PRIMARY_RENAME.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                std::fs::write(target, b"config_version = \"100\"\n").unwrap();
            }));
        });
        let error = update_in(&slot, &path, "media_layout", "compact").unwrap_err();
        assert!(config_commit::is_commit_unverified(&error));
        let refreshed = slot.lock().unwrap().as_ref().unwrap().clone();
        assert!(!Arc::ptr_eq(&first, &refreshed));
        assert_eq!(
            refreshed.take_migration_report(),
            Some(MigrationReport::NotNeeded)
        );
        assert_eq!(first.take_migration_report(), None);
        assert_eq!(refreshed.take_migration_report(), None);
    }

    #[test]
    fn matching_and_corrupt_failure_records_report_suppressed_only_when_enabled() {
        use island_core::config_state::{FailureRecord, FailureStage};
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let legacy = b"ui_density = \"minimal\"\n";
        let record = FailureRecord {
            source: ChangeFingerprint::of(legacy),
            from_schema: 1,
            stage: FailureStage::Backup,
            attempts: 1,
        }
        .encode();
        for failure in [record.as_bytes(), b"corrupt".as_slice()] {
            for migrate in [false, true] {
                let base = settings::tests::temp_base();
                let folder = base.join("Islora");
                std::fs::create_dir(&folder).unwrap();
                let path = folder.join("config.toml");
                let record_path = folder.join(config_commit::FAILURE_RECORD_NAME);
                std::fs::write(&path, legacy).unwrap();
                std::fs::write(&record_path, failure).unwrap();
                let snapshot = bootstrap_at(&base, migrate);
                let expected = if migrate {
                    MigrationReport::Fallback(FallbackReason::Suppressed)
                } else {
                    MigrationReport::NotNeeded
                };
                assert_eq!(snapshot.take_migration_report(), Some(expected));
                assert_eq!(snapshot.take_migration_report(), None);
                assert_eq!(snapshot.text().unwrap().as_bytes(), legacy);
                assert_eq!(snapshot.mode(), &RuntimeMode::LegacyCompatible);
                assert!(snapshot.writes_allowed());
                assert_eq!(std::fs::read(&path).unwrap(), legacy);
                assert_eq!(std::fs::read(&record_path).unwrap(), failure);
                let mut names: Vec<_> = std::fs::read_dir(&folder)
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name())
                    .collect();
                names.sort();
                let mut expected_names: Vec<std::ffi::OsString> = vec![
                    settings::PRIMARY_NAME.into(),
                    config_commit::FAILURE_RECORD_NAME.into(),
                ];
                expected_names.sort();
                assert_eq!(names, expected_names);
            }
        }
    }

    #[test]
    fn matching_failure_does_not_override_future_or_invalid_precedence() {
        use island_core::config_state::{FailureRecord, FailureStage};
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        for (source, mode) in [
            (
                b"config_version = \"100\"\n".as_slice(),
                RuntimeMode::ReadOnly,
            ),
            (
                b"config_version = \"invalid\"\n".as_slice(),
                RuntimeMode::LegacyCompatible,
            ),
        ] {
            let base = settings::tests::temp_base();
            let folder = base.join("Islora");
            std::fs::create_dir(&folder).unwrap();
            let path = folder.join("config.toml");
            let record_path = folder.join(config_commit::FAILURE_RECORD_NAME);
            let record = FailureRecord {
                source: ChangeFingerprint::of(source),
                from_schema: 1,
                stage: FailureStage::Backup,
                attempts: 1,
            }
            .encode();
            std::fs::write(&path, source).unwrap();
            std::fs::write(&record_path, &record).unwrap();
            let snapshot = bootstrap_at(&base, true);
            assert_eq!(
                snapshot.take_migration_report(),
                Some(MigrationReport::NotNeeded)
            );
            assert_eq!(snapshot.take_migration_report(), None);
            assert_eq!(snapshot.mode(), &mode);
            assert_eq!(std::fs::read(&path).unwrap(), source);
            assert_eq!(std::fs::read(&record_path).unwrap(), record.as_bytes());
        }
    }

    #[test]
    fn bootstrap_missing_primary_still_new_install() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let base = settings::tests::temp_base();
        let snap = bootstrap_at(&base, false);
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
        let snapshot = bootstrap_prepared(&base, result, false, || {
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
        let snapshot = std::thread::spawn(move || bootstrap_at(&other, true))
            .join()
            .unwrap();
        assert_eq!(snapshot.text(), Some(std::str::from_utf8(bytes).unwrap()));
        assert_eq!(snapshot.mode(), &RuntimeMode::LegacyCompatible);
        assert!(!snapshot.storage_ready && !snapshot.writes_allowed());
        assert_eq!(
            snapshot.take_migration_report(),
            Some(MigrationReport::Fallback(FallbackReason::Busy))
        );
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
        let snapshot = bootstrap_at(&base, false);
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
        let snap = bootstrap_at(&base, false);
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
        let snap = bootstrap_at(&base, false);
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
        let snap = bootstrap_at(&base, false);
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
        let snap = bootstrap_at(&base, true);
        assert!(!snap.storage_ready);
        assert_eq!(
            snap.take_migration_report(),
            Some(MigrationReport::Fallback(FallbackReason::Unprepared))
        );
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
        let snap = bootstrap_at(&base, false);
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
