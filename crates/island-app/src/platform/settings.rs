//! The settings file, which the user edits by hand and may hold a secret.

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
timer_presets = \"5, 15, 25, 45\"

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

/// Tells files from before these notices that they can be turned off too.
const LATER_NOTICES: &str = "
# notices_off için ek seçenekler: screenshot, clipboard, track_change,
# battery_saver, keyboard_language
";

const LEVEL_NOTICES: &str = "
# notices_off için ek seçenekler: volume, brightness. Bunlardan biri kapalıysa
# Windows'un ses/parlaklık göstergesi de görünür kalır.
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
    path().is_some_and(|path| !path.exists())
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
    if !path.exists() {
        std::fs::create_dir_all(path.parent()?).ok()?;
        let text = format!("{TEMPLATE}{START_SETTINGS}{NOTICE_SETTINGS}{LEVEL_NOTICES}{WAVE_SETTINGS}{WAVE_STYLE_SETTINGS}{MATERIAL_SETTINGS}{PROGRESS_SETTINGS}");
        std::fs::write(&path, text).ok()?;
    }
    let mut text = std::fs::read_to_string(&path).ok()?;
    let mut added = false;
    let sections = [
        ("timer_presets", START_SETTINGS),
        ("notices_off", NOTICE_SETTINGS),
        ("keyboard_language", LATER_NOTICES),
        ("# notices_off için ek seçenekler: volume, brightness", LEVEL_NOTICES),
        ("sound_wave", WAVE_SETTINGS),
        ("wave_style", WAVE_STYLE_SETTINGS),
        ("material", MATERIAL_SETTINGS),
        ("progress_style", PROGRESS_SETTINGS),
    ];
    for (setting, section) in sections {
        if !text.contains(setting) {
            text.push_str(section);
            added = true;
        }
    }
    if added {
        replace(&path, &text);
    }
    Some(text)
}

/// Stores a choice made in the island. Every other line stays as it was.
pub fn remember(key: &str, value: &str) {
    if let (Some(path), Some(text)) = (path(), read()) {
        replace(&path, &write_value(&text, key, value));
    }
}

/// A crash half way must not cost the user their calendar address.
fn replace(path: &Path, text: &str) {
    let draft = path.with_extension("toml.new");
    let written = std::fs::write(&draft, text).and_then(|()| std::fs::rename(&draft, path));
    if written.is_err() {
        eprintln!("settings could not be saved");
    }
}

pub fn open_in_editor() {
    let Some(path) = path() else { return };
    // Handing a file to its app can take a while, which the UI must not wait for.
    std::thread::spawn(move || unsafe {
        let file = HSTRING::from(format!("\"{}\"", path.display()));
        ShellExecuteW(None, w!("open"), w!("notepad.exe"), &file, None, SW_SHOWNORMAL);
    });
}
