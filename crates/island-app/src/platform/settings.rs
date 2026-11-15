//! The settings file, which the user edits by hand and may hold a secret.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use island_core::config::write_value;
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

/// Whether this is the very first start on this account.
pub fn is_first_run() -> bool {
    path().is_some_and(|path| is_new_install(&path))
}

fn is_new_install(path: &Path) -> bool {
    !path.exists() && !path.with_extension("toml.bak").is_file()
}

fn path() -> Option<PathBuf> {
    let roaming = PathBuf::from(std::env::var_os("APPDATA")?);
    let folder = roaming.join("Islora");
    // Settings from when the app had another name come along.
    if !folder.exists() {
        let _ = std::fs::rename(roaming.join("DynamicIsland"), &folder);
    }
    Some(folder.join("config.toml"))
}

/// Creates the file on first run, so there is something to fill in.
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

/// Existing recovery and first-run creation remain explicit side effects.
fn try_read_path(path: &Path) -> io::Result<String> {
    recover(path)?;
    if !path.exists() {
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "configuration has no parent directory",
            )
        })?;
        std::fs::create_dir_all(parent)?;
        let template = format!(
            "{TEMPLATE}{START_SETTINGS}{NOTICE_SETTINGS}{LEVEL_NOTICES}{WAVE_SETTINGS}{WAVE_STYLE_SETTINGS}{MATERIAL_SETTINGS}{PROGRESS_SETTINGS}"
        );
        // Keep the editable guidance, but leave resettable preferences absent
        // until the user chooses them. The calendar entry is integration data.
        let text = island_core::settings::reset_all(&template);
        replace(path, &text)?;
    }
    read_bounded(path)
}

/// Refuse oversized or non-UTF-8 settings without replacing or truncating
/// the file. Recheck while reading because it may grow after metadata lookup.
fn read_bounded(path: &Path) -> io::Result<String> {
    let file = std::fs::File::open(path)?;
    let limit = island_core::migration::MAX_CONFIG_BYTES;
    if file.metadata()?.len() > limit as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration exceeds the size limit",
        ));
    }
    read_bounded_content(file, limit)
}

fn read_bounded_content(reader: impl Read, limit: usize) -> io::Result<String> {
    let mut bytes = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration exceeds the size limit",
        ));
    }
    String::from_utf8(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "configuration is not valid UTF-8",
        )
    })
}

/// Applies a complete update through the same backup and recovery path.
pub fn try_remember(key: &str, value: &str) -> io::Result<()> {
    let path =
        path().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "APPDATA is unavailable"))?;
    let text = try_read_path(&path)?;
    replace(&path, &write_value(&text, key, value))
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

/// A crash half way must not cost the user their calendar address.
fn replace(path: &Path, text: &str) -> io::Result<()> {
    let draft = path.with_extension("toml.new");
    let mut file = std::fs::File::create(&draft)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    if path.exists() {
        let backup_draft = path.with_extension("toml.bak.new");
        std::fs::copy(path, &backup_draft)?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&backup_draft)?
            .sync_all()?;
        std::fs::rename(backup_draft, path.with_extension("toml.bak"))?;
    }
    std::fs::rename(draft, path)
}

/// An incomplete draft is never promoted: recover the last completed file.
fn recover(path: &Path) -> io::Result<()> {
    let backup = path.with_extension("toml.bak");
    if !path.exists() && backup.is_file() {
        let recovery = path.with_extension("toml.recover");
        std::fs::copy(backup, &recovery)?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&recovery)?
            .sync_all()?;
        std::fs::rename(recovery, path)?;
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
    fn bounded_content_read_detects_growth_after_metadata_check() {
        let mut growing = std::io::Cursor::new(b"12345".to_vec());
        assert_eq!(
            read_bounded_content(&mut growing, 4).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(growing.position(), 5);
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
    fn interrupted_update_recovers_last_complete_settings() {
        let folder =
            std::env::temp_dir().join(format!("islora-settings-recovery-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
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
