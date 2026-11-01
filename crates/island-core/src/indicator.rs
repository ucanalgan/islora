//! System indicators, and the logic for which events deserve one.

use std::sync::Arc;

use crate::activity::{Activity, Icon, Kind, Millis};

/// A level stays briefly, since it usually changes in a burst of steps.
const LEVEL_MS: Millis = 1_800;
const NOTICE_MS: Millis = 2_800;
/// A confirmation of something the user just did needs only a glance.
const BRIEF_MS: Millis = 1_200;
/// Long enough to reach it with the pointer and open it.
const SCREENSHOT_MS: Millis = 6_000;
/// Warns once on the way down past each of these percentages.
const BATTERY_WARNINGS: [u8; 2] = [20, 10];

#[derive(Clone, Debug, PartialEq)]
pub enum Indicator {
    Volume { percent: u8, muted: bool },
    Brightness { percent: u8 },
    CapsLock { on: bool },
    NumLock { on: bool },
    PluggedIn { percent: u8 },
    Unplugged { percent: u8 },
    LowBattery { percent: u8 },
    BluetoothConnected { name: String },
    BluetoothDisconnected { name: String },
    BluetoothBattery { name: String, percent: u8 },
    AudioOutput { name: String },
    Microphone { muted: bool },
    MicrophoneInUse { app: String },
    Internet { online: bool, network: Option<String> },
    Wifi { name: String },
    Vpn { connected: bool, name: String },
    AirplaneMode { on: bool },
    Usb { added: bool, name: String },
    Display { added: bool, count: u32 },
    /// `image` is the encoded file, when it could be read in time.
    Screenshot { path: String, image: Option<Arc<[u8]>> },
    /// `what` says the kind of content, never the content itself.
    Copied { what: &'static str },
    BatterySaver { on: bool },
    KeyboardLanguage { code: String },
}

impl Indicator {
    /// The name that turns it off in the settings, for those that can be.
    pub fn setting(&self) -> Option<&'static str> {
        match self {
            Indicator::BluetoothBattery { .. } => Some("bluetooth_battery"),
            Indicator::AudioOutput { .. } => Some("audio_output"),
            Indicator::Microphone { .. } => Some("microphone"),
            Indicator::MicrophoneInUse { .. } => Some("microphone_use"),
            Indicator::Internet { .. } => Some("internet"),
            Indicator::Wifi { .. } => Some("wifi"),
            Indicator::Vpn { .. } => Some("vpn"),
            Indicator::AirplaneMode { .. } => Some("airplane"),
            Indicator::Usb { .. } => Some("usb"),
            Indicator::Display { .. } => Some("display"),
            Indicator::Screenshot { .. } => Some("screenshot"),
            Indicator::Copied { .. } => Some("clipboard"),
            Indicator::BatterySaver { .. } => Some("battery_saver"),
            Indicator::KeyboardLanguage { .. } => Some("keyboard_language"),
            _ => None,
        }
    }

    /// Whether it confirms something small, which an open view can do without.
    pub fn is_brief(&self) -> bool {
        matches!(self, Indicator::Copied { .. } | Indicator::KeyboardLanguage { .. })
    }

    /// Networks come and go while a laptop wakes up, which is not news.
    pub fn is_about_network(&self) -> bool {
        matches!(self, Indicator::Internet { .. } | Indicator::Wifi { .. } | Indicator::Vpn { .. })
    }

    pub fn into_activity(self, now: Millis) -> Activity {
        match self {
            Indicator::Screenshot { path, image } => Activity {
                preview: image,
                file: Some(path),
                expires_at: Some(now + SCREENSHOT_MS),
                ..notice(Icon::Camera, "Ekran görüntüsü", "Açmak için tıkla", now)
            },
            Indicator::Copied { what } => brief(Icon::Clipboard, "Kopyalandı", what, now),
            Indicator::BatterySaver { on } => {
                notice(Icon::Battery, "Pil Tasarrufu", switch_text(on), now)
            }
            Indicator::KeyboardLanguage { code } => brief(Icon::Keyboard, "Klavye", code, now),
            Indicator::BluetoothBattery { name, percent } => {
                notice(Icon::Bluetooth, "Bağlandı", format!("{name} · %{percent}"), now)
            }
            Indicator::AudioOutput { name } => notice(Icon::Speaker, "Ses çıkışı", name, now),
            Indicator::Microphone { muted: true } => {
                notice(Icon::MicrophoneOff, "Mikrofon", "Sessize alındı", now)
            }
            Indicator::Microphone { muted: false } => {
                notice(Icon::Microphone, "Mikrofon", "Açıldı", now)
            }
            Indicator::MicrophoneInUse { app } => {
                notice(Icon::Microphone, "Mikrofon kullanımda", app, now)
            }
            Indicator::Internet { online: false, .. } => {
                notice(Icon::InternetOff, "İnternet kesildi", "Bağlantı yok", now)
            }
            Indicator::Internet { online: true, network } => {
                let detail = network.unwrap_or_else(|| "Bağlantı kuruldu".to_owned());
                notice(Icon::Internet, "İnternet geldi", detail, now)
            }
            Indicator::Wifi { name } => notice(Icon::Wifi, "Wi-Fi", name, now),
            Indicator::Vpn { connected: true, name } => notice(Icon::Vpn, "VPN bağlandı", name, now),
            Indicator::Vpn { connected: false, name } => notice(Icon::Vpn, "VPN kesildi", name, now),
            Indicator::AirplaneMode { on } => notice(Icon::Airplane, "Uçak modu", switch_text(on), now),
            Indicator::Usb { added: true, name } => notice(Icon::Usb, "USB takıldı", name, now),
            Indicator::Usb { added: false, name } => notice(Icon::Usb, "USB çıkarıldı", name, now),
            Indicator::Display { added, count } => {
                let title = if added { "Ekran bağlandı" } else { "Ekran çıkarıldı" };
                notice(Icon::Monitor, title, format!("{count} ekran"), now)
            }
            Indicator::Volume { percent, muted } => {
                let icon = if muted { Icon::Muted } else { Icon::Volume };
                let shown = if muted { 0 } else { percent };
                level(icon, "Ses", shown, now)
            }
            Indicator::Brightness { percent } => level(Icon::Brightness, "Parlaklık", percent, now),
            Indicator::CapsLock { on } => {
                notice(Icon::Keyboard, "Caps Lock", switch_text(on), now)
            }
            Indicator::NumLock { on } => notice(Icon::Keyboard, "Num Lock", switch_text(on), now),
            Indicator::PluggedIn { percent } => {
                notice(Icon::Charging, "Şarj oluyor", format!("%{percent}"), now)
            }
            Indicator::Unplugged { percent } => {
                notice(Icon::Battery, "Pille çalışıyor", format!("%{percent}"), now)
            }
            Indicator::LowBattery { percent } => {
                notice(Icon::BatteryLow, "Pil azaldı", format!("%{percent} kaldı"), now)
            }
            Indicator::BluetoothConnected { name } => {
                notice(Icon::Bluetooth, "Bağlandı", name, now)
            }
            Indicator::BluetoothDisconnected { name } => {
                notice(Icon::Bluetooth, "Bağlantı kesildi", name, now)
            }
        }
    }
}

fn switch_text(on: bool) -> &'static str {
    if on { "Açık" } else { "Kapalı" }
}

fn level(icon: Icon, title: &str, percent: u8, now: Millis) -> Activity {
    let percent = percent.min(100);
    Activity {
        level: Some(percent),
        expires_at: Some(now + LEVEL_MS),
        ..notice(icon, title, format!("%{percent}"), now)
    }
}

fn brief(icon: Icon, leading: &str, trailing: impl Into<String>, now: Millis) -> Activity {
    Activity {
        leading: leading.to_owned(),
        trailing: trailing.into(),
        brief: true,
        expires_at: Some(now + BRIEF_MS),
        icon: Some(icon),
        ..Activity::new(Kind::Alert)
    }
}

fn notice(icon: Icon, title: &str, detail: impl Into<String>, now: Millis) -> Activity {
    Activity {
        title: title.to_owned(),
        detail: detail.into(),
        expires_at: Some(now + NOTICE_MS),
        icon: Some(icon),
        ..Activity::new(Kind::Alert)
    }
}

/// Windows reports the current state as soon as an event source is registered,
/// and repeats values. Only real changes should reach the screen.
#[derive(Debug, Default)]
pub struct SystemTracker {
    on_battery: Option<bool>,
    battery: Option<u8>,
    brightness: Option<u8>,
    volume: Option<(u8, bool)>,
    microphone: Option<bool>,
    audio_output: Option<String>,
    monitors: Option<u32>,
    battery_saver: Option<bool>,
    language: Option<String>,
}

impl SystemTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn battery_saver_changed(&mut self, on: bool) -> Option<Indicator> {
        let previous = self.battery_saver.replace(on)?;
        (previous != on).then_some(Indicator::BatterySaver { on })
    }

    pub fn keyboard_language_changed(&mut self, code: String) -> Option<Indicator> {
        let previous = self.language.replace(code.clone())?;
        (previous != code).then_some(Indicator::KeyboardLanguage { code })
    }

    /// The microphone reports every level change too, which is not what counts.
    pub fn microphone_changed(&mut self, muted: bool) -> Option<Indicator> {
        let previous = self.microphone.replace(muted)?;
        (previous != muted).then_some(Indicator::Microphone { muted })
    }

    pub fn audio_output_changed(&mut self, name: String) -> Option<Indicator> {
        let previous = self.audio_output.replace(name.clone())?;
        (previous != name).then_some(Indicator::AudioOutput { name })
    }

    /// A new resolution reports the same way, with the same number of screens.
    pub fn monitors_changed(&mut self, count: u32) -> Option<Indicator> {
        let previous = self.monitors.replace(count)?;
        (previous != count).then_some(Indicator::Display { added: count > previous, count })
    }

    pub fn power_source_changed(&mut self, on_battery: bool) -> Option<Indicator> {
        let previous = self.on_battery.replace(on_battery)?;
        if previous == on_battery {
            return None;
        }
        let percent = self.battery.unwrap_or(0);
        Some(if on_battery { Indicator::Unplugged { percent } } else { Indicator::PluggedIn { percent } })
    }

    pub fn battery_changed(&mut self, percent: u8) -> Option<Indicator> {
        let previous = self.battery.replace(percent)?;
        if self.on_battery != Some(true) {
            return None;
        }
        BATTERY_WARNINGS
            .into_iter()
            .any(|limit| previous > limit && percent <= limit)
            .then_some(Indicator::LowBattery { percent })
    }

    pub fn brightness_changed(&mut self, percent: u8) -> Option<Indicator> {
        let previous = self.brightness.replace(percent)?;
        (previous != percent).then_some(Indicator::Brightness { percent })
    }

    /// Unlike the others, this source reports nothing at startup, so the very
    /// first event is already a change.
    pub fn volume_changed(&mut self, percent: u8, muted: bool) -> Option<Indicator> {
        let previous = self.volume.replace((percent, muted));
        (previous != Some((percent, muted))).then_some(Indicator::Volume { percent, muted })
    }
}

/// What the network looks like at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkState {
    pub online: bool,
    /// The name of the Wi-Fi network in use.
    pub wifi: Option<String>,
    /// The name of the VPN connection in use.
    pub vpn: Option<String>,
}

/// Turns network states into what is worth saying. Losing and regaining the
/// connection also changes the Wi-Fi name, which would say the same thing twice.
#[derive(Debug, Default)]
pub struct NetworkTracker {
    known: Option<NetworkState>,
}

impl NetworkTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, state: NetworkState) -> Vec<Indicator> {
        let Some(known) = self.known.replace(state.clone()) else { return Vec::new() };
        let mut shown = Vec::new();
        match (known.online, state.online, state.wifi) {
            (true, false, _) => shown.push(Indicator::Internet { online: false, network: None }),
            (false, true, network) => shown.push(Indicator::Internet { online: true, network }),
            (true, true, Some(name)) if known.wifi.as_ref() != Some(&name) => {
                shown.push(Indicator::Wifi { name });
            }
            _ => {}
        }
        match (known.vpn, state.vpn) {
            (before, Some(name)) if before.as_ref() != Some(&name) => {
                shown.push(Indicator::Vpn { connected: true, name });
            }
            (Some(name), None) => shown.push(Indicator::Vpn { connected: false, name }),
            _ => {}
        }
        shown
    }
}

/// Whether a network adapter that is up is a VPN. Windows has no such flag, so
/// this goes by the kind of adapter and the words in its name, while leaving out
/// the tunnels it runs for itself. `kind` is the interface type from Windows.
pub fn is_vpn_adapter(kind: u32, name: &str) -> bool {
    const PPP: u32 = 23;
    const VIRTUAL: u32 = 53;
    const TUNNEL: u32 = 131;
    const SYSTEM_WORDS: [&str; 7] =
        ["teredo", "6to4", "isatap", "ip-https", "loopback", "bluetooth", "miniport"];
    const VPN_WORDS: [&str; 8] =
        ["vpn", "tap-", "wireguard", "tailscale", "wintun", "zerotier", "nord", "warp"];
    let name = name.to_lowercase();
    let says = |words: &[&str]| words.iter().any(|word| name.contains(word));
    !says(&SYSTEM_WORDS) && (matches!(kind, PPP | VIRTUAL | TUNNEL) || says(&VPN_WORDS))
}

/// The two letters of a language from a locale name: `tr-TR` reads as `TR`.
pub fn language_code(locale: &str) -> Option<String> {
    let code = locale.split('-').next()?.to_uppercase();
    (!code.is_empty()).then_some(code)
}

/// Whether a file name is one of the images Windows saves screenshots as.
pub fn is_image_file(name: &str) -> bool {
    let name = name.to_lowercase();
    [".png", ".jpg", ".jpeg"].iter().any(|extension| name.ends_with(extension))
}

/// The drive letters in the mask Windows reports a new or removed volume with.
pub fn drive_letters(mask: u32) -> Vec<char> {
    (0..26).filter(|bit| mask >> bit & 1 != 0).map(|bit| char::from(b'A' + bit as u8)).collect()
}

/// The address in a Bluetooth device id, as it appears in the system's own
/// device names. The id ends in `-aa:bb:cc:dd:ee:ff`.
pub fn bluetooth_address(id: &str) -> Option<String> {
    let address = id.rsplit('-').next()?.replace(':', "").to_uppercase();
    (address.len() == 12 && address.chars().all(|digit| digit.is_ascii_hexdigit())).then_some(address)
}

/// The readable name of an app from its entry in Windows' privacy records:
/// a path with `#` for `\`, or a package name such as `5319275A.WhatsApp_cv1g`.
pub fn capability_app_name(key: &str) -> String {
    let file = key.rsplit('#').next().unwrap_or(key);
    let name = if key.contains('#') {
        file.rsplit_once('.').map_or(file, |(stem, _)| stem)
    } else {
        let package = file.split('_').next().unwrap_or(file);
        package.rsplit('.').next().unwrap_or(package)
    };
    let mut letters = name.chars();
    letters.next().map_or_else(String::new, |first| first.to_uppercase().chain(letters).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn network(online: bool, wifi: Option<&str>) -> NetworkState {
        NetworkState { online, wifi: wifi.map(str::to_owned), vpn: None }
    }

    #[test]
    fn the_first_network_state_is_silent() {
        assert_eq!(NetworkTracker::new().update(network(false, None)), Vec::new());
    }

    #[test]
    fn losing_and_regaining_the_connection_says_so_once() {
        let mut tracker = NetworkTracker::new();
        tracker.update(network(true, Some("Ev")));
        assert_eq!(
            tracker.update(network(false, None)),
            vec![Indicator::Internet { online: false, network: None }]
        );
        assert_eq!(
            tracker.update(network(true, Some("Ev"))),
            vec![Indicator::Internet { online: true, network: Some("Ev".into()) }]
        );
    }

    #[test]
    fn moving_to_another_wifi_names_it() {
        let mut tracker = NetworkTracker::new();
        tracker.update(network(true, Some("Ev")));
        assert_eq!(tracker.update(network(true, Some("Okul"))), vec![Indicator::Wifi { name: "Okul".into() }]);
        assert_eq!(tracker.update(network(true, Some("Okul"))), Vec::new());
    }

    #[test]
    fn moving_to_a_cable_says_nothing() {
        let mut tracker = NetworkTracker::new();
        tracker.update(network(true, Some("Ev")));
        assert_eq!(tracker.update(network(true, None)), Vec::new());
    }

    #[test]
    fn a_vpn_is_reported_coming_and_going() {
        let mut tracker = NetworkTracker::new();
        tracker.update(network(true, None));
        let on = NetworkState { vpn: Some("WireGuard".into()), ..network(true, None) };
        assert_eq!(tracker.update(on), vec![Indicator::Vpn { connected: true, name: "WireGuard".into() }]);
        assert_eq!(
            tracker.update(network(true, None)),
            vec![Indicator::Vpn { connected: false, name: "WireGuard".into() }]
        );
    }

    #[test]
    fn a_screenshot_carries_its_image_on_a_normal_alert() {
        let image = Arc::<[u8]>::from(&b"png"[..]);
        let shot = Indicator::Screenshot { path: "a.png".into(), image: Some(image.clone()) };
        let activity = shot.into_activity(1_000);
        assert_eq!(activity.preview, Some(image));
        assert_eq!(activity.file.as_deref(), Some("a.png"));
        assert_eq!(activity.expires_at, Some(1_000 + SCREENSHOT_MS));
        assert!(!activity.brief);
    }

    #[test]
    fn a_copy_is_only_a_brief_confirmation() {
        let activity = Indicator::Copied { what: "Metin" }.into_activity(1_000);
        assert!(activity.brief);
        assert_eq!(activity.leading, "Kopyalandı");
        assert_eq!(activity.trailing, "Metin");
        assert_eq!(activity.expires_at, Some(1_000 + BRIEF_MS));
    }

    #[test]
    fn the_battery_saver_is_silent_at_first_then_reports_changes() {
        let mut tracker = SystemTracker::new();
        assert_eq!(tracker.battery_saver_changed(false), None);
        assert_eq!(tracker.battery_saver_changed(false), None);
        assert_eq!(tracker.battery_saver_changed(true), Some(Indicator::BatterySaver { on: true }));
    }

    #[test]
    fn only_a_new_language_counts_for_the_keyboard() {
        let mut tracker = SystemTracker::new();
        assert_eq!(tracker.keyboard_language_changed("TR".into()), None);
        assert_eq!(tracker.keyboard_language_changed("TR".into()), None);
        assert_eq!(
            tracker.keyboard_language_changed("EN".into()),
            Some(Indicator::KeyboardLanguage { code: "EN".into() })
        );
    }

    #[test]
    fn a_language_is_read_from_its_locale() {
        assert_eq!(language_code("tr-TR").as_deref(), Some("TR"));
        assert_eq!(language_code("en-US").as_deref(), Some("EN"));
        assert_eq!(language_code(""), None);
    }

    #[test]
    fn only_images_count_as_screenshots() {
        assert!(is_image_file("Ekran görüntüsü 2026-11-01.PNG"));
        assert!(is_image_file("foto.jpeg"));
        assert!(!is_image_file("desktop.ini"));
        assert!(!is_image_file("video.mp4"));
    }

    #[test]
    fn vpn_adapters_are_told_from_the_ones_windows_runs_itself() {
        assert!(is_vpn_adapter(23, "Okul VPN"));
        assert!(is_vpn_adapter(53, "WireGuard Tunnel"));
        assert!(is_vpn_adapter(6, "TAP-Windows Adapter V9"));
        assert!(is_vpn_adapter(131, "Tailscale Tunnel"));
        assert!(!is_vpn_adapter(6, "Hyper-V Virtual Ethernet Adapter"));
        assert!(!is_vpn_adapter(71, "Intel(R) Wi-Fi 6 AX201 160MHz"));
        assert!(!is_vpn_adapter(6, "WAN Miniport (IP)"));
        assert!(!is_vpn_adapter(131, "Teredo Tunneling Pseudo-Interface"));
        assert!(!is_vpn_adapter(131, "Microsoft IP-HTTPS Platform Interface"));
    }

    #[test]
    fn a_drive_mask_gives_its_letters() {
        assert_eq!(drive_letters(0b101_0000), vec!['E', 'G']);
        assert_eq!(drive_letters(0), Vec::<char>::new());
    }

    #[test]
    fn a_bluetooth_address_is_read_from_the_device_id() {
        let id = "Bluetooth#Bluetooth12:34:56:78:9a:bc-aa:bb:cc:dd:ee:ff";
        assert_eq!(bluetooth_address(id).as_deref(), Some("AABBCCDDEEFF"));
        assert_eq!(bluetooth_address("baska bir sey"), None);
    }

    #[test]
    fn app_names_are_read_from_paths_and_packages() {
        assert_eq!(capability_app_name("C:#Program Files#Discord#Discord.exe"), "Discord");
        assert_eq!(capability_app_name("C:#Users#Umut#zoom.exe"), "Zoom");
        assert_eq!(capability_app_name("5319275A.WhatsApp_cv1g1gvanyjgm"), "WhatsApp");
        assert_eq!(capability_app_name("Microsoft.WindowsCamera_8wekyb3d8bbwe"), "WindowsCamera");
        assert_eq!(capability_app_name(""), "");
    }

    #[test]
    fn only_a_change_of_mute_counts_for_the_microphone() {
        let mut tracker = SystemTracker::new();
        assert_eq!(tracker.microphone_changed(false), None);
        assert_eq!(tracker.microphone_changed(false), None);
        assert_eq!(tracker.microphone_changed(true), Some(Indicator::Microphone { muted: true }));
    }

    #[test]
    fn a_new_output_device_is_named_once() {
        let mut tracker = SystemTracker::new();
        assert_eq!(tracker.audio_output_changed("Hoparlör".into()), None);
        assert_eq!(tracker.audio_output_changed("Hoparlör".into()), None);
        assert_eq!(
            tracker.audio_output_changed("Kulaklık".into()),
            Some(Indicator::AudioOutput { name: "Kulaklık".into() })
        );
    }

    #[test]
    fn only_a_change_in_the_number_of_screens_counts() {
        let mut tracker = SystemTracker::new();
        assert_eq!(tracker.monitors_changed(1), None);
        assert_eq!(tracker.monitors_changed(1), None);
        assert_eq!(tracker.monitors_changed(2), Some(Indicator::Display { added: true, count: 2 }));
        assert_eq!(tracker.monitors_changed(1), Some(Indicator::Display { added: false, count: 1 }));
    }

    #[test]
    fn notices_that_can_be_turned_off_have_a_name() {
        assert_eq!(Indicator::AirplaneMode { on: true }.setting(), Some("airplane"));
        assert_eq!(Indicator::CapsLock { on: true }.setting(), None);
    }

    #[test]
    fn the_first_report_of_each_source_is_silent() {
        let mut tracker = SystemTracker::new();
        assert_eq!(tracker.power_source_changed(true), None);
        assert_eq!(tracker.battery_changed(50), None);
        assert_eq!(tracker.brightness_changed(70), None);
    }

    #[test]
    fn repeated_values_are_ignored() {
        let mut tracker = SystemTracker::new();
        tracker.brightness_changed(70);
        assert_eq!(tracker.brightness_changed(70), None);
        tracker.volume_changed(40, false);
        assert_eq!(tracker.volume_changed(40, false), None);
    }

    #[test]
    fn plugging_and_unplugging_report_the_battery_level() {
        let mut tracker = SystemTracker::new();
        tracker.power_source_changed(false);
        tracker.battery_changed(64);
        assert_eq!(tracker.power_source_changed(true), Some(Indicator::Unplugged { percent: 64 }));
        assert_eq!(tracker.power_source_changed(false), Some(Indicator::PluggedIn { percent: 64 }));
    }

    #[test]
    fn low_battery_warns_once_at_each_limit_on_the_way_down() {
        let mut tracker = SystemTracker::new();
        tracker.power_source_changed(true);
        tracker.battery_changed(22);
        assert_eq!(tracker.battery_changed(21), None);
        assert_eq!(tracker.battery_changed(20), Some(Indicator::LowBattery { percent: 20 }));
        assert_eq!(tracker.battery_changed(19), None);
        assert_eq!(tracker.battery_changed(11), None);
        assert_eq!(tracker.battery_changed(10), Some(Indicator::LowBattery { percent: 10 }));
        assert_eq!(tracker.battery_changed(9), None);
    }

    #[test]
    fn a_large_drop_past_a_limit_still_warns_once() {
        let mut tracker = SystemTracker::new();
        tracker.power_source_changed(true);
        tracker.battery_changed(30);
        assert_eq!(tracker.battery_changed(15), Some(Indicator::LowBattery { percent: 15 }));
    }

    #[test]
    fn low_battery_stays_quiet_while_charging() {
        let mut tracker = SystemTracker::new();
        tracker.power_source_changed(false);
        tracker.battery_changed(21);
        assert_eq!(tracker.battery_changed(20), None);
    }

    #[test]
    fn rising_past_a_limit_does_not_warn() {
        let mut tracker = SystemTracker::new();
        tracker.power_source_changed(true);
        tracker.battery_changed(19);
        assert_eq!(tracker.battery_changed(21), None);
    }

    #[test]
    fn the_first_volume_change_is_shown() {
        let mut tracker = SystemTracker::new();
        let shown = tracker.volume_changed(40, false);
        assert_eq!(shown, Some(Indicator::Volume { percent: 40, muted: false }));
    }

    #[test]
    fn muting_counts_as_a_change_even_at_the_same_volume() {
        let mut tracker = SystemTracker::new();
        tracker.volume_changed(40, false);
        let shown = tracker.volume_changed(40, true);
        assert_eq!(shown, Some(Indicator::Volume { percent: 40, muted: true }));
    }

    #[test]
    fn muted_volume_shows_as_empty() {
        let activity = Indicator::Volume { percent: 40, muted: true }.into_activity(0);
        assert_eq!(activity.level, Some(0));
        assert_eq!(activity.icon, Some(Icon::Muted));
    }

    #[test]
    fn levels_leave_sooner_than_notices() {
        let start = 10_000;
        let level = Indicator::Brightness { percent: 50 }.into_activity(start);
        let notice = Indicator::CapsLock { on: true }.into_activity(start);
        assert_eq!(level.expires_at, Some(start + LEVEL_MS));
        assert_eq!(notice.expires_at, Some(start + NOTICE_MS));
    }

    #[test]
    fn indicators_interrupt_as_alerts() {
        assert_eq!(Indicator::NumLock { on: false }.into_activity(0).kind, Kind::Alert);
    }
}
