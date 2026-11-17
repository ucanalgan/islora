//! The settings file, which the user edits by hand and may hold a secret.
//! If deleted at runtime, try_remember fails and shows the save-failed dialog;
//! preparation creates the file only at startup.

use std::io;
use std::path::{Path, PathBuf};

use super::config_fs::{ConfigDir, VerifiedFile, WriterLock, WriterMutex};
use island_core::config::write_value;
use island_core::migration::{CriticalKey, MigrationError, MigrationOutcome, prepare};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, w};

const TEMPLATE: &str = "\
# Islora ayarları
#
# Google Takvim adresi. Google Takvim'de: Ayarlar > sol taraftan takviminin adı >
# Takvimi entegre et > \"iCal biçiminde gizli adres\". Bu adres şifre gibidir,
# kimseyle paylaşma. Ekledikten sonra tepsi menüsünden \"Takvimi ve ayarları
# yenile\" de.
calendar_ics_url = \"\"
";

/// Also added to files made before these settings existed.
const START_SETTINGS: &str = "
# Ana görünümdeki süre düğmeleri, dakika olarak. Tam dört tane olmalı.
timer_presets = \"5, 10, 15, 25\"

# Düğmelerin ne başlatacağı: focus, Windows Odak oturumu demek ve Windows
# ayarlarına göre bildirimleri susturabilir. timer, yalnızca island'ın kendi
# sayacıdır. Ana görünümden de değiştirilebilir.
start_mode = \"focus\"

# Görünüm: island, üst kenardan ayrı durur. notch, üst kenara yapışıktır. Tepsi
# menüsünden de değiştirilebilir.
island_style = \"island\"
";

/// Added on its own to files made before it existed.
const NOTICE_SETTINGS: &str = "
# Görmek istemediğin bildirimler, virgülle ayrılmış olarak. Seçenekler:
# volume, brightness, audio_output, microphone, microphone_use, camera, internet,
# wifi, vpn, airplane,
# bluetooth_battery, usb, display, screenshot, clipboard, track_change,
# battery_saver, keyboard_language
notices_off = \"\"
";

const LEVEL_NOTICES: &str = "
# notices_off için ek seçenekler: volume, brightness. Windows'un kendi ses ve
# parlaklık göstergesi de görünür kalır.
";

const WAVE_SETTINGS: &str = "
# Küçük pill'deki ses dalgası: on veya off. Kapalıyken dalga görünmez.
# Enerji tasarrufu açıkken dalga da görünmez; kapatılınca seçimin geri gelir.
sound_wave = \"on\"
";

const WAVE_STYLE_SETTINGS: &str = "
# Ses dalgasının hareketi: calm sakin, normal, lively canlı.
wave_style = \"normal\"
";

const MATERIAL_SETTINGS: &str = "
# Tasarım: black düz siyah, glass arkasını bulanık gösteren camdır. Cam, enerji
# tasarrufunda ve Windows şeffaflığı kapalıyken siyah görünür.
material = \"black\"
";

const PROGRESS_SETTINGS: &str = "
# Telefona gönderirken açık görünümde ilerleme: ring halka, edge kenar ışığı,
# fill dolgu. Küçük pill her durumda küçük bir halka gösterir.
progress_style = \"ring\"
";

#[cfg(test)]
fn is_new_install(path: &Path) -> bool {
    let Ok(dir) = ConfigDir::open(path.parent().unwrap()) else {
        return true;
    };
    is_new_install_in(&dir)
}
fn is_new_install_in(dir: &ConfigDir) -> bool {
    matches!(inspect_in(dir, "config.toml"), Ok(false))
        && !has_recovery_artifacts(dir).unwrap_or(true)
}
fn inspect_in(dir: &ConfigDir, name: &str) -> io::Result<bool> {
    match VerifiedFile::open(dir, std::ffi::OsStr::new(name)) {
        Ok(_) => Ok(true),
        Err(error) => {
            let error = io::Error::from(error);
            if error.kind() == io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(error)
            }
        }
    }
}
fn verified_file(path: &Path) -> io::Result<VerifiedFile> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "configuration has no parent")
    })?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "configuration has no name"))?;
    let dir = ConfigDir::open(parent).map_err(io::Error::from)?;
    VerifiedFile::open(&dir, name).map_err(io::Error::from)
}

fn has_recovery_artifacts(dir: &ConfigDir) -> io::Result<bool> {
    artifacts(dir, false)
}
fn has_ambiguous_recovery_artifacts(dir: &ConfigDir) -> io::Result<bool> {
    artifacts(dir, true)
}
fn artifacts(dir: &ConfigDir, ambiguous: bool) -> io::Result<bool> {
    for name in dir.entries().map_err(io::Error::from)? {
        let Some(name) = name.to_str() else {
            return Ok(true);
        };
        let name = name.to_ascii_lowercase();
        if matches!(name.as_str(), "config.toml.bak.new" | "config.toml.recover")
            || (!ambiguous && matches!(name.as_str(), "config.toml.bak" | "config.toml.new"))
            || (name.starts_with("config.pre-migration") && name.ends_with(".bak"))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn path() -> Option<PathBuf> {
    let roaming = PathBuf::from(std::env::var_os("APPDATA")?);
    let folder = roaming.join("Islora");
    Some(folder.join("config.toml"))
}

/// Reads the existing settings file without creating it.
pub fn read() -> Option<String> {
    let path = path()?;
    match try_read_path(&path) {
        Ok(text) => Some(text),
        Err(error) => {
            eprintln!("settings could not be read: {error}");
            None
        }
    }
}

fn try_read_path(path: &Path) -> io::Result<String> {
    read_bounded(path)
}

/// Prepare storage once, before readers start. The mutex also covers the
/// bootstrap interval before the directory and its lock file exist.
pub fn prepare_storage() -> io::Result<bool> {
    let base = PathBuf::from(
        std::env::var_os("APPDATA")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "APPDATA is unavailable"))?,
    );
    let result = prepare_storage_at(&base);
    match &result {
        Ok(_) => STORAGE_STATE.store(1, std::sync::atomic::Ordering::Relaxed),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
        Err(_) => STORAGE_STATE.store(2, std::sync::atomic::Ordering::Relaxed),
    }
    result
}

static PREPARATION_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());
static STORAGE_STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

fn prepare_storage_at(base: &Path) -> io::Result<bool> {
    let folder = base.join("Islora");
    let mutex = WriterMutex::acquire().map_err(io::Error::from)?;
    if !folder.exists() {
        match std::fs::rename(base.join("DynamicIsland"), &folder) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    std::fs::create_dir_all(&folder)?;
    let lock = WriterLock::with_mutex(&folder, mutex).map_err(io::Error::from)?;
    let path = folder.join("config.toml");
    let dir = lock.directory();
    let first_run = is_new_install_in(dir);
    if !inspect_in(dir, "config.toml")? && has_ambiguous_recovery_artifacts(dir)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration recovery artifacts need review",
        ));
    }
    if !inspect_in(dir, "config.toml")?
        && !inspect_in(dir, "config.toml.bak")?
        && has_recovery_artifacts(dir)?
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "configuration is missing and recovery artifacts need review",
        ));
    }
    recover_in(dir, &path)?;
    if !inspect_in(dir, "config.toml")? {
        let template = format!(
            "{TEMPLATE}{START_SETTINGS}{NOTICE_SETTINGS}{LEVEL_NOTICES}{WAVE_SETTINGS}{WAVE_STYLE_SETTINGS}{MATERIAL_SETTINGS}{PROGRESS_SETTINGS}"
        );
        let mut text = island_core::settings::reset_all(&template);
        text.push_str("config_version = \"2\"\n");
        replace_in(dir, &path, &text)?;
    }
    Ok(first_run)
}

/// Refuse oversized or non-UTF-8 settings without replacing or truncating
/// the file. Read one byte beyond the limit to detect oversized input.
fn read_bounded(path: &Path) -> io::Result<String> {
    let mut file = verified_file(path)?;
    read_verified(&mut file)
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

fn read_verified(file: &mut VerifiedFile) -> io::Result<String> {
    let bytes = file
        .read_bounded(island_core::migration::MAX_CONFIG_BYTES)
        .map_err(io::Error::from)?;
    String::from_utf8(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration is not valid UTF-8",
        )
    })
}

pub fn storage_writable() -> bool {
    let _guard = PREPARATION_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    match STORAGE_STATE.load(std::sync::atomic::Ordering::Relaxed) {
        1 => true,
        // Lazy preparation occurs after startup, so its first-run result is no longer used.
        0 => prepare_storage().is_ok(),
        _ => false,
    }
}

/// Applies a complete update through the same backup and recovery path.
pub fn try_remember(key: &str, value: &str) -> io::Result<()> {
    if !storage_writable() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "configuration storage is read-only",
        ));
    }
    let path =
        path().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "APPDATA is unavailable"))?;
    try_remember_path(&path, key, value)
}

/// Why an ordinary write must leave the configuration alone. Variants name a
/// category and at most a critical key, never a configuration value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteRefusal {
    FutureSchema,
    InvalidSchemaVersion,
    DuplicateCriticalKey(CriticalKey),
    MalformedCriticalSetting(CriticalKey),
    TooLarge,
    UpdateTooLarge,
}

impl From<WriteRefusal> for io::Error {
    fn from(refusal: WriteRefusal) -> Self {
        match refusal {
            WriteRefusal::FutureSchema => io::Error::new(
                io::ErrorKind::PermissionDenied,
                "future configuration is read-only",
            ),
            WriteRefusal::InvalidSchemaVersion => io::Error::new(
                io::ErrorKind::InvalidData,
                "configuration schema version is invalid",
            ),
            WriteRefusal::DuplicateCriticalKey(key) => io::Error::new(
                io::ErrorKind::InvalidData,
                format!("configuration has a duplicate {} setting", key.name()),
            ),
            WriteRefusal::MalformedCriticalSetting(key) => io::Error::new(
                io::ErrorKind::InvalidData,
                format!("configuration has a malformed {} setting", key.name()),
            ),
            WriteRefusal::TooLarge => io::Error::new(
                io::ErrorKind::InvalidData,
                "configuration exceeds the size limit",
            ),
            WriteRefusal::UpdateTooLarge => io::Error::new(
                io::ErrorKind::InvalidData,
                "updated configuration exceeds the size limit",
            ),
        }
    }
}

/// Classify critical settings after omitting lines the core cannot interpret.
pub(crate) fn classify_ignoring_unsupported(
    text: &str,
) -> Result<MigrationOutcome, MigrationError> {
    let initial = prepare(text);
    // The migration parser stops at the first unsupported line. Classify each
    // line with the same parser and omit only unsupported lines on a retry,
    // so critical settings later in the file are still checked together.
    if matches!(initial, Err(MigrationError::UnsupportedFormat { .. })) {
        let mut supported = String::new();
        for (index, line) in text.split_inclusive('\n').enumerate() {
            let line = if index == 0 {
                line.strip_prefix('\u{feff}').unwrap_or(line)
            } else {
                line
            };
            // A BOM is special only on the original first line.
            let protect_bom = index > 0 && line.starts_with('\u{feff}');
            let protected;
            let candidate = if protect_bom {
                protected = format!(" {line}");
                protected.as_str()
            } else {
                line
            };
            if !matches!(
                prepare(candidate),
                Err(MigrationError::UnsupportedFormat { .. })
            ) {
                if supported.is_empty() && protect_bom {
                    supported.push(' ');
                }
                supported.push_str(line);
            }
        }
        prepare(&supported)
    } else {
        initial
    }
}

/// Whether an ordinary write may replace this configuration. Only a future or
/// invalid schema version, a duplicate or malformed critical key, or an
/// oversized source refuses it. Content the migration core merely cannot
/// migrate (unsupported lines, invalid presentation values) stays writable,
/// because the line writer preserves every line it does not change.
pub(crate) fn write_permission(text: &str) -> Result<(), WriteRefusal> {
    match classify_ignoring_unsupported(text) {
        Ok(MigrationOutcome::Future { .. }) => Err(WriteRefusal::FutureSchema),
        Err(MigrationError::InvalidSchemaVersion) => Err(WriteRefusal::InvalidSchemaVersion),
        Err(MigrationError::DuplicateCriticalKey(key)) => {
            Err(WriteRefusal::DuplicateCriticalKey(key))
        }
        Err(MigrationError::MalformedCriticalSetting(key)) => {
            Err(WriteRefusal::MalformedCriticalSetting(key))
        }
        Err(MigrationError::InputTooLarge) => Err(WriteRefusal::TooLarge),
        Ok(MigrationOutcome::Prepared { .. } | MigrationOutcome::AlreadyCurrent)
        | Err(
            MigrationError::UnsupportedFormat { .. }
            | MigrationError::InvalidLegacyValue
            | MigrationError::InvalidModernValue(_)
            | MigrationError::OutputTooLarge
            | MigrationError::InvalidOutput,
        ) => Ok(()),
    }
}

/// The text an ordinary write of `key = value` would store, or why that write
/// must leave the configuration alone, including when the result would
/// exceed the size limit.
pub(crate) fn prepared_write(text: &str, key: &str, value: &str) -> Result<String, WriteRefusal> {
    write_permission(text)?;
    let updated = write_value(text, key, value);
    if updated.len() > island_core::migration::MAX_CONFIG_BYTES {
        return Err(WriteRefusal::UpdateTooLarge);
    }
    Ok(updated)
}

fn try_remember_path(path: &Path, key: &str, value: &str) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "configuration has no parent")
    })?;
    let lock = WriterLock::acquire(parent).map_err(io::Error::from)?;
    let text = read_locked(&lock, path)?;
    let updated = prepared_write(&text, key, value)?;
    replace_in(lock.directory(), path, &updated)
}

pub(crate) fn locked_update<T>(
    action: impl FnOnce(&str, &mut dyn FnMut(&str, &str) -> io::Result<()>) -> io::Result<T>,
) -> io::Result<T> {
    if !storage_writable() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "configuration storage is read-only",
        ));
    }
    let path =
        path().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "APPDATA is unavailable"))?;
    locked_update_at(&path, action)
}

pub(crate) fn locked_update_at<T>(
    path: &Path,
    action: impl FnOnce(&str, &mut dyn FnMut(&str, &str) -> io::Result<()>) -> io::Result<T>,
) -> io::Result<T> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "configuration has no parent")
    })?;
    let lock = WriterLock::acquire(parent).map_err(io::Error::from)?;
    let text = read_locked(&lock, path)?;
    action(&text, &mut |key, value| {
        replace_in(lock.directory(), path, &prepared_write(&text, key, value)?)
    })
}

/// Complete a change already applied at runtime. The rollback must report
/// whether it actually restored the previous state.
pub(crate) fn save_with_rollback(
    save: impl FnOnce() -> io::Result<()>,
    rollback: impl FnOnce() -> bool,
) -> Result<(), bool> {
    match save() {
        Ok(()) => Ok(()),
        Err(error) => {
            eprintln!("settings could not be saved: {error}");
            Err(rollback())
        }
    }
}

pub fn persist_or_rollback(key: &str, value: &str, rollback: impl FnOnce() -> bool) -> bool {
    match save_with_rollback(|| try_remember(key, value), rollback) {
        Ok(()) => true,
        Err(restored) => {
            report_persistence_failure(restored);
            false
        }
    }
}

pub(crate) fn report_persistence_failure(restored: bool) {
    report_error(if restored {
        "Ayar kaydedilemedi. Önceki seçim geri yüklendi. config.toml erişimini kontrol edin."
    } else {
        "Ayar kaydedilemedi ve önceki durum geri yüklenemedi. Değişiklik bu oturumda veya Windows'ta etkin kalmış olabilir. Ayarları kontrol edin."
    });
}

pub fn report_error(message: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    unsafe {
        let message = HSTRING::from(message);
        MessageBoxW(None, &message, w!("Islora"), MB_OK | MB_ICONERROR);
    }
}

fn name(path: &Path) -> io::Result<&std::ffi::OsStr> {
    path.file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "configuration has no name"))
}
fn read_in(dir: &ConfigDir, path: &Path) -> io::Result<String> {
    let mut file = VerifiedFile::open(dir, name(path)?).map_err(io::Error::from)?;
    read_verified(&mut file)
}
#[cfg(test)]
fn replace(path: &Path, text: &str) -> io::Result<()> {
    let dir = ConfigDir::open(path.parent().unwrap()).map_err(io::Error::from)?;
    replace_in(&dir, path, text)
}
/// A crash half way must not cost the user their calendar address.
fn replace_in(dir: &ConfigDir, path: &Path, text: &str) -> io::Result<()> {
    let draft = path.with_extension("toml.new");
    VerifiedFile::write_file(dir, name(&draft)?, text.as_bytes()).map_err(io::Error::from)?;
    if inspect_in(
        dir,
        name(path)?
            .to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid name"))?,
    )? {
        let backup_draft = path.with_extension("toml.bak.new");
        let original = read_in(dir, path)?;
        VerifiedFile::write_file(dir, name(&backup_draft)?, original.as_bytes())
            .map_err(io::Error::from)?;
        VerifiedFile::rename(
            dir,
            name(&backup_draft)?,
            name(&path.with_extension("toml.bak"))?,
        )
        .map_err(io::Error::from)?;
    }
    VerifiedFile::rename(dir, name(&draft)?, name(path)?).map_err(io::Error::from)
}
#[cfg(test)]
fn recover(path: &Path) -> io::Result<()> {
    let dir = ConfigDir::open(path.parent().unwrap()).map_err(io::Error::from)?;
    recover_in(&dir, path)
}
/// An incomplete draft is never promoted: recover the last completed file.
fn recover_in(dir: &ConfigDir, path: &Path) -> io::Result<()> {
    let backup = path.with_extension("toml.bak");
    let primary = name(path)?
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid name"))?;
    let backup_name = name(&backup)?
        .to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid name"))?;
    if !inspect_in(dir, primary)? && inspect_in(dir, backup_name)? {
        let candidate = read_in(dir, &backup)?;
        match island_core::migration::prepare(&candidate) {
            Ok(
                island_core::migration::MigrationOutcome::Prepared { .. }
                | island_core::migration::MigrationOutcome::AlreadyCurrent,
            ) => {}
            Ok(island_core::migration::MigrationOutcome::Future { .. }) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "future configuration backup needs review",
                ));
            }
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "configuration backup is invalid",
                ));
            }
        }
        let recovery = path.with_extension("toml.recover");
        VerifiedFile::write_file(dir, name(&recovery)?, candidate.as_bytes())
            .map_err(io::Error::from)?;
        if read_in(dir, &recovery)? != candidate {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "configuration backup changed during recovery",
            ));
        }
        VerifiedFile::rename(dir, name(&recovery)?, name(path)?).map_err(io::Error::from)?;
    }
    Ok(())
}

pub fn open_in_editor() {
    let Some(path) = path() else { return };
    // Handing a file to its app can take a while, which the UI must not wait for.
    std::thread::spawn(move || unsafe {
        let file = HSTRING::from(format!("\"{}\"", path.display()));
        ShellExecuteW(
            None,
            w!("open"),
            w!("notepad.exe"),
            &file,
            None,
            SW_SHOWNORMAL,
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "islora-prepare-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn locked_update_observes_latest_choice_and_rolls_back_before_unlock() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let base = temp_base();
        let path = base.join("config.toml");
        std::fs::write(&path, "config_version = \"2\"\nautostart = \"off\"\n").unwrap();
        let contender = base.clone();
        locked_update_at(&path, |_, save| {
            let error = std::thread::spawn(move || WriterLock::acquire(&contender).err().unwrap())
                .join()
                .unwrap();
            assert!(matches!(
                error,
                super::super::config_fs::ConfigFsError::Locked
            ));
            save("autostart", "on")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            island_core::config::read_value(&std::fs::read_to_string(&path).unwrap(), "autostart")
                .as_deref(),
            Some("on")
        );
        let large = "x".repeat(island_core::migration::MAX_CONFIG_BYTES + 1);
        let error = locked_update_at(&path, |_, save| save("x", &large)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn reads_leave_empty_and_recovery_directories_unchanged() {
        let base = temp_base();
        let path = base.join("config.toml");
        assert!(try_read_path(&path).is_err());
        assert_eq!(std::fs::read_dir(&base).unwrap().count(), 0);
        std::fs::write(base.join("config.toml.bak"), b"saved").unwrap();
        std::fs::write(base.join("config.toml.recover"), b"draft").unwrap();
        let listing = || {
            let mut entries: Vec<_> = std::fs::read_dir(&base)
                .unwrap()
                .map(|e| {
                    let e = e.unwrap();
                    (e.file_name(), std::fs::read(e.path()).unwrap())
                })
                .collect();
            entries.sort();
            entries
        };
        let before = listing();
        assert!(try_read_path(&path).is_err());
        assert_eq!(listing(), before);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn preparation_is_idempotent_and_recovers_legacy_backup() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let base = temp_base();
        assert!(prepare_storage_at(&base).unwrap());
        let path = base.join("Islora/config.toml");
        let before = std::fs::read(&path).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(!prepare_storage_at(&base).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            modified
        );
        std::fs::remove_dir_all(base.join("Islora")).unwrap();
        std::fs::create_dir(base.join("DynamicIsland")).unwrap();
        std::fs::write(
            base.join("DynamicIsland/config.toml.bak"),
            b"config_version = \"2\"\n",
        )
        .unwrap();
        prepare_storage_at(&base).unwrap();
        assert!(!base.join("DynamicIsland").exists());
        assert_eq!(std::fs::read(&path).unwrap(), b"config_version = \"2\"\n");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn busy_preparation_leaves_directory_unchanged() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let base = temp_base();
        let mutex = WriterMutex::acquire().unwrap();
        let other = base.clone();
        let error = std::thread::spawn(move || prepare_storage_at(&other).unwrap_err())
            .join()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(std::fs::read_dir(&base).unwrap().count(), 0);
        drop(mutex);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn locked_writer_preserves_configuration() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let base = temp_base();
        let path = base.join("config.toml");
        std::fs::write(&path, b"config_version = \"2\"\n").unwrap();
        let lock = WriterLock::acquire(&base).unwrap();
        let target = path.clone();
        let error = std::thread::spawn(move || try_remember_path(&target, "x", "y").unwrap_err())
            .join()
            .unwrap();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(std::fs::read(&path).unwrap(), b"config_version = \"2\"\n");
        drop(lock);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn fresh_configuration_is_created_with_current_schema() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let folder = std::env::temp_dir().join(format!(
            "islora-fresh-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = folder.join("Islora").join("config.toml");
        assert!(is_new_install(&path));
        assert!(prepare_storage_at(&folder).unwrap());
        assert!(!prepare_storage_at(&folder).unwrap());
        let text = try_read_path(&path).unwrap();
        assert_eq!(
            island_core::config::read_value(&text, "config_version").as_deref(),
            Some("2")
        );
        assert_eq!(
            island_core::migration::prepare(&text),
            Ok(island_core::migration::MigrationOutcome::AlreadyCurrent)
        );
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(try_read_path(&path).unwrap(), text);
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            before
        );
        assert!(!path.with_extension("toml.bak").exists());
        assert!(!is_new_install(&path));
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(folder.join("Islora")).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn missing_primary_with_recovery_evidence_does_not_create_defaults() {
        let folder = std::env::temp_dir().join(format!(
            "islora-recovery-evidence-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("config.toml");
        for name in [
            "config.toml.new",
            "config.toml.bak.new",
            "config.toml.recover",
            "config.pre-migration.bak",
            "config.pre-migration.2.bak",
            "CONFIG.PRE-MIGRATION.3.BAK",
        ] {
            let artifact = folder.join(name);
            std::fs::write(&artifact, "evidence").unwrap();
            assert!(!is_new_install(&path));
            assert!(try_read_path(&path).is_err());
            assert!(!path.exists());
            assert_eq!(std::fs::read_to_string(&artifact).unwrap(), "evidence");
            std::fs::remove_file(artifact).unwrap();
        }
        assert!(is_new_install(&path));
        let normal = folder.join("config.toml.bak");
        let protected = folder.join("config.pre-migration.bak");
        std::fs::write(&normal, "normal = \"saved\"\n").unwrap();
        std::fs::write(&protected, "protected").unwrap();
        assert!(try_read_path(&path).is_err());
        assert!(!path.exists());
        assert_eq!(
            std::fs::read_to_string(&normal).unwrap(),
            "normal = \"saved\"\n"
        );
        assert_eq!(std::fs::read_to_string(&protected).unwrap(), "protected");
        std::fs::remove_file(normal).unwrap();
        std::fs::remove_file(protected).unwrap();
        let protected = folder.join("config.pre-migration.bak");
        std::fs::write(&protected, "protected").unwrap();
        std::fs::write(&path, "config_version = \"2\"\n").unwrap();
        assert_eq!(try_read_path(&path).unwrap(), "config_version = \"2\"\n");
        assert_eq!(std::fs::read_to_string(&protected).unwrap(), "protected");
        std::fs::remove_file(protected).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn invalid_or_future_rolling_backup_is_not_restored() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let folder = std::env::temp_dir().join(format!(
            "islora-invalid-backup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let base = folder;
        let folder = base.join("Islora");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("config.toml");
        let backup = path.with_extension("toml.bak");
        for candidate in ["ui_density = \"invalid\"\n", "config_version = \"8\"\n"] {
            std::fs::write(&backup, candidate).unwrap();
            assert!(prepare_storage_at(&base).is_err());
            assert!(!path.exists());
            assert!(!path.with_extension("toml.recover").exists());
            assert_eq!(std::fs::read_to_string(&backup).unwrap(), candidate);
        }
        std::fs::remove_file(backup).unwrap();
        std::fs::remove_dir(folder).unwrap();
        std::fs::remove_dir(base).unwrap();
    }

    #[test]
    fn reparse_file_is_rejected_when_symlinks_are_available() {
        let folder = std::env::temp_dir().join(format!(
            "islora-reparse-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let target = folder.join("target.toml");
        let link = folder.join("config.toml");
        std::fs::write(&target, "config_version = \"2\"\n").unwrap();
        match std::os::windows::fs::symlink_file(&target, &link) {
            Ok(()) => {
                assert!(read_bounded(&link).is_err());
                assert!(try_read_path(&link).is_err());
                assert_eq!(
                    std::fs::read_to_string(&target).unwrap(),
                    "config_version = \"2\"\n"
                );
                std::fs::remove_file(link).unwrap();
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::PermissionDenied | io::ErrorKind::Unsupported
                ) || matches!(error.raw_os_error(), Some(50 | 1314)) =>
            {
                // Developer mode and symlink privileges are not prerequisites.
                eprintln!(
                    "symlink test skipped: creation is unavailable without additional system privileges or filesystem support"
                );
            }
            Err(error) => panic!("symlink setup failed: {error}"),
        }
        std::fs::remove_file(target).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn settings_writes_reject_future_and_invalid_sources_without_creating_drafts() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let folder = std::env::temp_dir().join(format!(
            "islora-write-gate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("config.toml");
        for source in [
            "config_version = \"3\"\ncalendar_ics_url = \"https://example.invalid/\"\n",
            "config_version = \"2\"\nconfig_version = \"2\"\n",
        ] {
            std::fs::write(&path, source).unwrap();
            assert!(try_remember_path(&path, "media_layout", "compact").is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
            assert!(!path.with_extension("toml.new").exists());
            assert!(!path.with_extension("toml.bak").exists());
        }
        let source = "config_version = \"2\"\n# preserve\nunknown = \"value\"\n";
        std::fs::write(&path, source).unwrap();
        try_remember_path(&path, "media_layout", "compact").unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("# preserve\nunknown = \"value\"\n"));
        assert_eq!(
            island_core::config::read_value(&saved, "media_layout").as_deref(),
            Some("compact")
        );
        std::fs::remove_file(path.with_extension("toml.bak")).unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn unsupported_lines_do_not_block_ordinary_writes() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let folder = std::env::temp_dir().join(format!(
            "islora-unsupported-write-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("config.toml");
        for source in [
            "config_version = \"2\"\nstray line\ncalendar_ics_url = \"https://example.invalid/\"\n",
            "[section]\nconfig_version = \"2\"\nunknown = \"keep\"\n",
            "stray\nui_density = \"minimal\"\n",
        ] {
            assert!(matches!(
                prepare(source),
                Err(MigrationError::UnsupportedFormat { .. })
            ));
            std::fs::write(&path, source).unwrap();
            try_remember_path(&path, "media_layout", "compact").unwrap();
            let saved = std::fs::read_to_string(&path).unwrap();
            assert_eq!(saved, write_value(source, "media_layout", "compact"));
            assert_eq!(
                std::fs::read_to_string(path.with_extension("toml.bak")).unwrap(),
                source
            );
        }
        std::fs::remove_file(path.with_extension("toml.bak")).unwrap();
        for source in [
            "config_version = \"3\"\nmedia_layout = \"standard\"\n",
            "config_version = \"2\"\nindicator_style = \"ring\"\nindicator_style = \"ring\"\n",
            "stray\nindicator_style = \"ring\"\nindicator_style = \"ring\"\n",
        ] {
            std::fs::write(&path, source).unwrap();
            assert!(try_remember_path(&path, "media_layout", "compact").is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
            assert!(!path.with_extension("toml.new").exists());
            assert!(!path.with_extension("toml.bak").exists());
        }
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn write_permission_refuses_only_unsafe_sources() {
        for source in [
            "",
            "config_version = \"2\"\n",
            "ui_density = \"minimal\"\n",
            "config_version = \"2\"\nindicator_style = \"unknown\"\n",
            "ui_density = \"unknown\"\n",
            "orphan\n",
            "[section]\n",
        ] {
            assert_eq!(write_permission(source), Ok(()), "{source:?}");
        }
        for (source, refusal) in [
            ("config_version = \"3\"\n", WriteRefusal::FutureSchema),
            (
                "config_version = \"0\"\n",
                WriteRefusal::InvalidSchemaVersion,
            ),
            (
                "orphan\nmedia_layout = \"compact\"\nmedia_layout = \"compact\"\n",
                WriteRefusal::DuplicateCriticalKey(CriticalKey::MediaLayout),
            ),
            (
                "ui_density = \"minimal\" trailing\n",
                WriteRefusal::MalformedCriticalSetting(CriticalKey::UiDensity),
            ),
            (
                "\u{feff}config_version : 2\r\n",
                WriteRefusal::MalformedCriticalSetting(CriticalKey::ConfigVersion),
            ),
        ] {
            assert_eq!(write_permission(source), Err(refusal), "{source:?}");
        }
        let oversized = "#".repeat(island_core::migration::MAX_CONFIG_BYTES + 1);
        assert_eq!(write_permission(&oversized), Err(WriteRefusal::TooLarge));
        let marker = "SYNTHETIC_PRIVATE_MARKER";
        let source = format!("calendar_ics_url = \"{marker}\"\nconfig_version = \"{marker}\"\n");
        let error = io::Error::from(write_permission(&source).unwrap_err());
        assert!(!error.to_string().contains(marker));
    }

    #[test]
    fn unsupported_lines_do_not_hide_unsafe_settings() {
        for (source, refusal) in [
            ("bare\nconfig_version = \"3\"\n", WriteRefusal::FutureSchema),
            (
                "[section]\nconfig_version = \"3\"\n",
                WriteRefusal::FutureSchema,
            ),
            (
                "bare\nconfig_version = \"bad\"\n",
                WriteRefusal::InvalidSchemaVersion,
            ),
            (
                "bare\nconfig_version = \"18446744073709551616\"\n",
                WriteRefusal::InvalidSchemaVersion,
            ),
            (
                "[section]\nmedia_layout = \"compact\"\nmedia_layout = \"compact\"\n",
                WriteRefusal::DuplicateCriticalKey(CriticalKey::MediaLayout),
            ),
            (
                "bare\nindicator_style = \"ring\" trailing\n",
                WriteRefusal::MalformedCriticalSetting(CriticalKey::IndicatorStyle),
            ),
            (
                "\u{feff}bare\r\nconfig_version = \"3\"\r\n",
                WriteRefusal::FutureSchema,
            ),
            (
                "bare\n\u{feff}config_version = \"3\"\nconfig_version = \"3\"\n",
                WriteRefusal::FutureSchema,
            ),
            (
                "bare\nconfig_version 3",
                WriteRefusal::MalformedCriticalSetting(CriticalKey::ConfigVersion),
            ),
            (
                "x\r\n[s]\r\nui_density = \"minimal\"\r\ny\r\nui_density = \"minimal\"\r\n",
                WriteRefusal::DuplicateCriticalKey(CriticalKey::UiDensity),
            ),
            ("config_version = \"3\"\nbare\n", WriteRefusal::FutureSchema),
        ] {
            assert_eq!(write_permission(source), Err(refusal), "{source:?}");
        }
        for source in [
            "bare\n[section]\nconfig_version = \"2\"\nother bare\n",
            "one\nconfig_version = \"2\"\ntwo\nindicator_style = \"ring\"\nthree\nmedia_layout = \"compact\"\n",
            "config_version = \"2\"\nindicator_style = \"ring\"\nstray\n",
            "config_version = \"2\"\nindicator_style = \"ring\"\nbare\n[section]\n",
        ] {
            assert_eq!(write_permission(source), Ok(()), "{source:?}");
        }
    }

    #[test]
    fn prepared_write_refuses_an_update_past_the_size_limit() {
        let max = island_core::migration::MAX_CONFIG_BYTES;
        let header = "config_version = \"2\"\n";
        let addition = "autostart = \"on\"\n".len();
        let padded = |len: usize| format!("{header}#{}\n", "x".repeat(len - header.len() - 2));
        let fits = padded(max - addition);
        assert_eq!(
            prepared_write(&fits, "autostart", "on"),
            Ok(write_value(&fits, "autostart", "on"))
        );
        let overflows = padded(max - addition + 1);
        assert_eq!(write_permission(&overflows), Ok(()));
        assert_eq!(
            prepared_write(&overflows, "autostart", "on"),
            Err(WriteRefusal::UpdateTooLarge)
        );
        assert_eq!(
            prepared_write("config_version = \"3\"\n", "autostart", "on"),
            Err(WriteRefusal::FutureSchema)
        );
    }

    #[test]
    fn bounded_read_rejects_invalid_utf8_and_oversized_files_without_changes() {
        let folder = std::env::temp_dir().join(format!(
            "islora-bounded-read-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("config.toml");
        let original =
            b"\xef\xbb\xbf# heading\r\ncalendar_ics_url = \"https://example.invalid/\"\r\n";
        std::fs::write(&path, original).unwrap();
        assert_eq!(read_bounded(&path).unwrap().as_bytes(), original);
        let invalid = b"# invalid UTF-8\n\xff";
        std::fs::write(&path, invalid).unwrap();
        assert_eq!(
            read_bounded(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(std::fs::read(&path).unwrap(), invalid);
        let max = island_core::migration::MAX_CONFIG_BYTES;
        std::fs::write(&path, vec![b'x'; max - 1]).unwrap();
        assert_eq!(read_bounded(&path).unwrap().len(), max - 1);
        std::fs::write(&path, vec![b'x'; max]).unwrap();
        assert_eq!(read_bounded(&path).unwrap().len(), max);
        std::fs::write(&path, vec![b'x'; max + 1]).unwrap();
        assert_eq!(
            read_bounded(&path).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(std::fs::metadata(&path).unwrap().len(), (max + 1) as u64);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn validation_error_does_not_disclose_configuration_values() {
        let _serial = super::super::config_fs::TEST_WRITER_SERIAL
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let folder = std::env::temp_dir().join(format!(
            "islora-redaction-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("config.toml");
        let marker = "SYNTHETIC_PRIVATE_MARKER";
        let content = format!(
            "calendar_ics_url = \"https://example.invalid/?token={marker}\"\nconfig_version = \"bad\"\n"
        );
        std::fs::write(&path, &content).unwrap();
        let error = try_remember_path(&path, "media_layout", "compact").unwrap_err();
        assert!(!error.to_string().contains(marker));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn save_success_and_failure_restore_runtime_and_renderer() {
        use std::cell::Cell;
        let runtime = Cell::new("new");
        let renderer = Cell::new("new");
        let unrelated = Cell::new("untouched");
        assert_eq!(
            save_with_rollback(
                || Ok(()),
                || { panic!("successful save must not roll back") }
            ),
            Ok(())
        );
        let result = save_with_rollback(
            || Err(io::Error::other("save failed")),
            || {
                runtime.set("old");
                renderer.set("old");
                true
            },
        );
        assert_eq!(result, Err(true));
        assert_eq!(
            (runtime.get(), renderer.get(), unrelated.get()),
            ("old", "old", "untouched")
        );
        assert_eq!(
            save_with_rollback(|| Err(io::Error::other("save failed")), || false),
            Err(false)
        );
    }

    #[test]
    fn draft_and_backup_failures_preserve_the_primary_file() {
        let folder = std::env::temp_dir().join(format!(
            "islora-write-stages-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("config.toml");
        let original =
            "# keep\ncalendar_ics_url = \"https://example.test/?secret=1\"\nunknown = \"keep\"\n";
        std::fs::write(&path, original).unwrap();
        let draft = path.with_extension("toml.new");
        std::fs::create_dir(&draft).unwrap();
        assert!(replace(&path, "new").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        std::fs::remove_dir(&draft).unwrap();
        let backup_draft = path.with_extension("toml.bak.new");
        std::fs::create_dir(&backup_draft).unwrap();
        assert!(replace(&path, "new").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        std::fs::remove_file(draft).unwrap();
        std::fs::remove_dir(backup_draft).unwrap();
        let blocked_backup = path.with_extension("toml.bak");
        std::fs::create_dir(&blocked_backup).unwrap();
        assert!(replace(&path, "new").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        std::fs::remove_file(path.with_extension("toml.new")).unwrap();
        std::fs::remove_file(path.with_extension("toml.bak.new")).unwrap();
        std::fs::remove_dir(blocked_backup).unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn saved_update_keeps_unrelated_comments_and_calendar_secret() {
        let folder = std::env::temp_dir().join(format!(
            "islora-save-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("config.toml");
        let original = "# keep\ncalendar_ics_url = \"https://example.test/?secret=1\"\nunknown = \"keep\"\nmedia_layout = \"standard\"\n";
        std::fs::write(&path, original).unwrap();
        let updated = write_value(original, "media_layout", "informative");
        replace(&path, &updated).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), updated);
        assert!(updated.contains(
            "# keep\ncalendar_ics_url = \"https://example.test/?secret=1\"\nunknown = \"keep\""
        ));
        assert_eq!(
            std::fs::read_to_string(path.with_extension("toml.bak")).unwrap(),
            original
        );
        std::fs::remove_file(path.with_extension("toml.bak")).unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }

    #[test]
    fn failed_write_returns_an_error() {
        let folder = std::env::temp_dir().join(format!(
            "islora-missing-settings-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        assert!(replace(&folder.join("config.toml"), "new value").is_err());
        assert!(!folder.exists());
    }

    #[test]
    fn redirected_ancestor_keeps_replace_and_recover_in_held_directory() {
        let base = temp_base();
        let original = base.join("original");
        let decoy = base.join("decoy");
        let link = base.join("link");
        std::fs::create_dir(&original).unwrap();
        std::fs::create_dir(&decoy).unwrap();
        let make_link = |target: &Path| {
            assert!(
                std::process::Command::new("cmd")
                    .args(["/c", "mklink", "/J"])
                    .arg(&link)
                    .arg(target)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        std::fs::create_dir(original.join("settings")).unwrap();
        std::fs::create_dir(decoy.join("settings")).unwrap();
        make_link(&original);
        let path = link.join("settings/config.toml");
        std::fs::write(
            original.join("settings/config.toml"),
            "config_version = \"2\"\n",
        )
        .unwrap();
        std::fs::write(decoy.join("settings/config.toml"), "decoy").unwrap();
        let dir = ConfigDir::open(&link.join("settings")).unwrap();
        std::fs::remove_dir(&link).unwrap();
        make_link(&decoy);
        replace_in(&dir, &path, "config_version = \"2\"\n# updated\n").unwrap();
        assert_eq!(
            read_in(&dir, &path).unwrap(),
            "config_version = \"2\"\n# updated\n"
        );
        std::fs::write(
            original.join("settings/config.toml.bak"),
            "config_version = \"2\"\n# backup\n",
        )
        .unwrap();
        std::fs::remove_file(original.join("settings/config.toml")).unwrap();
        recover_in(&dir, &path).unwrap();
        assert_eq!(
            read_in(&dir, &path).unwrap(),
            "config_version = \"2\"\n# backup\n"
        );
        assert_eq!(
            std::fs::read_to_string(decoy.join("settings/config.toml")).unwrap(),
            "decoy"
        );
        std::fs::remove_dir(&link).unwrap();
        drop(dir);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn handle_rename_supports_long_turkish_name() {
        let base = temp_base();
        let dir = ConfigDir::open(&base).unwrap();
        let target = format!(
            "{}-\u{0130}stanbul-\u{015f}\u{00f6}\u{011f}\u{00fc}\u{0131}.txt",
            "long".repeat(45)
        );
        std::fs::write(base.join("source"), "content").unwrap();
        VerifiedFile::rename(
            &dir,
            std::ffi::OsStr::new("source"),
            std::ffi::OsStr::new(&target),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(base.join(target)).unwrap(),
            "content"
        );
        drop(dir);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn interrupted_update_recovers_last_complete_settings() {
        let folder = temp_base();
        let path = folder.join("config.toml");
        std::fs::write(&path, "original = \"safe\"\n").unwrap();
        replace(&path, "updated = \"saved\"\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "updated = \"saved\"\n"
        );
        std::fs::write(path.with_extension("toml.new"), "partial").unwrap();
        std::fs::remove_file(&path).unwrap();
        assert!(!is_new_install(&path));
        recover(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "original = \"safe\"\n"
        );
        std::fs::remove_file(path.with_extension("toml.new")).unwrap();
        std::fs::remove_file(path.with_extension("toml.bak")).unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }
}
