//! A read-only look at what Islora's AirPods support (step 2b) assumes of the
//! real AirPods: which device ids Windows lists for them, and what they
//! advertise over Bluetooth LE.
//!
//! It writes nothing: no files, no registry, no settings. It does not pair or
//! connect to anything and does not touch the mouse or keyboard. Everything
//! goes to the console. Bluetooth addresses are never printed in full, and of
//! an advertisement only the first bytes are shown, never the encrypted rest.
//!
//! `airpods_probe.exe [seconds]`, 90 seconds by default. Ctrl+C ends it early.

use std::collections::BTreeSet;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use island_core::airpods::{APPLE_COMPANY_ID, apple_audio_pid, parse};
use island_core::indicator::bluetooth_address;
use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisement, BluetoothLEAdvertisementFilter,
    BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
    BluetoothLEAdvertisementWatcherStoppedEventArgs, BluetoothLEManufacturerData,
    BluetoothLEScanningMode,
};
use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind};
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::{DataReader, IBuffer};
use windows::Win32::System::Console::SetConsoleCtrlHandler;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::{BOOL, HSTRING};
use windows_collections::IIterable;

const DEFAULT_SECONDS: u64 = 90;
/// A line for the same thing is repeated at least this often.
const REPEAT_EVERY: Duration = Duration::from_secs(5);
/// Signal strength changes smaller than this are not worth a line.
const RSSI_BUCKET_DB: i16 = 5;
/// The proximity pairing message is a type byte, a length byte and this many
/// more.
const PROXIMITY_TYPE: u8 = 0x07;
const PROXIMITY_LEN: usize = 25;
/// How much of a proximity message is shown: the type, the length and the
/// first nine bytes of its body, which are the ones that are read. The rest is
/// an encrypted payload that belongs to one device.
const HEAD_LEN: usize = 11;

static STOP: AtomicBool = AtomicBool::new(false);

fn say(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{text}");
    let _ = out.flush();
}

unsafe extern "system" fn on_ctrl(_kind: u32) -> BOOL {
    STOP.store(true, Ordering::Relaxed);
    BOOL(1)
}

fn main() {
    let seconds = std::env::args()
        .nth(1)
        .and_then(|text| text.parse::<u64>().ok())
        .unwrap_or(DEFAULT_SECONDS);
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
        let _ = SetConsoleCtrlHandler(Some(on_ctrl), true);
    }
    say("airpods_probe: read-only, nothing is written, paired or connected.");
    say("");
    say("== 1. Connected Bluetooth devices ==");
    list_devices();
    say("");
    say(&format!(
        "== 2. Advertisements for {seconds} s (Ctrl+C to end early) =="
    ));
    advertisements(Duration::from_secs(seconds));
}

// ------------------------------------------------------------ section 1

fn list_devices() {
    let connected = BluetoothConnectionStatus::Connected;
    let selectors = [
        (
            "classic",
            BluetoothDevice::GetDeviceSelectorFromConnectionStatus(connected),
        ),
        (
            "LE",
            BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(connected),
        ),
    ];
    let mut found = 0;
    for (kind, selector) in selectors {
        let Ok(selector) = selector else {
            say(&format!("[{kind}] no selector"));
            continue;
        };
        let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector).and_then(|op| op.join());
        let devices = match devices {
            Ok(devices) => devices,
            Err(error) => {
                say(&format!("[{kind}] listing failed: {error}"));
                continue;
            }
        };
        for device in devices {
            found += 1;
            let name = device.Name().map(|n| n.to_string()).unwrap_or_default();
            let id = device.Id().map(|i| i.to_string()).unwrap_or_default();
            let address = bluetooth_address(&id);
            say(&format!(
                "[{kind}] name={name:?} id={}",
                mask(&id, address.as_deref())
            ));
            match address {
                Some(address) => look_up(&address),
                None => say("    no address in the id, no lookup"),
            }
        }
    }
    if found == 0 {
        say("no connected Bluetooth device was listed.");
    }
}

/// What step 2b looks for: the device nodes whose instance id holds the
/// address, which include the profile children, and then the old `DEV_` one.
fn look_up(address: &str) {
    let ids = instance_ids(&format!("System.Devices.DeviceInstanceId:~~\"{address}\""));
    say(&format!(
        "    lookup ~~\"ADDRESS\" (what 2b uses): {} id(s)",
        ids.as_ref().map_or(0, Vec::len)
    ));
    match &ids {
        Some(ids) => {
            for id in ids {
                say(&format!("      {}", mask(id, Some(address))));
            }
            let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
            match apple_audio_pid(&refs) {
                Some(pid) => say(&format!(
                    "    apple_audio_pid -> 0x{pid:04X} ({})",
                    model_of(pid)
                )),
                None => say("    apple_audio_pid -> none"),
            }
        }
        None => say("      the lookup failed"),
    }

    let old = instance_ids(&format!(
        "System.Devices.DeviceInstanceId:~~\"DEV_{address}\""
    ));
    say(&format!(
        "    lookup ~~\"DEV_ADDRESS\" (the battery lookup, for comparison): {} id(s)",
        old.as_ref().map_or(0, Vec::len)
    ));
    for id in old.iter().flatten() {
        say(&format!("      {}", mask(id, Some(address))));
    }
}

fn instance_ids(filter: &str) -> Option<Vec<String>> {
    let none = IIterable::<HSTRING>::from(Vec::new());
    let devices = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(
        &HSTRING::from(filter),
        &none,
        DeviceInformationKind::Device,
    )
    .ok()?
    .join()
    .ok()?;
    Some(
        devices
            .into_iter()
            .filter_map(|device| Some(device.Id().ok()?.to_string()))
            .collect(),
    )
}

/// The model a product id maps to, found the way the app finds it: by reading
/// a message that carries it. (The mapping itself is private to the core.)
fn model_of(pid: u16) -> String {
    let [low, high] = pid.to_le_bytes();
    let mut data = vec![PROXIMITY_TYPE, PROXIMITY_LEN as u8, 0x01, low, high];
    data.resize(2 + PROXIMITY_LEN, 0);
    parse(&data).map_or_else(
        || "unreadable".into(),
        |status| format!("{:?}", status.model),
    )
}

/// The text with the last six hex digits of the address hidden, both as it is
/// in instance ids (`A1B2C3D4E5F6`) and with colons (`a1:b2:c3:d4:e5:f6`).
fn mask(text: &str, address: Option<&str>) -> String {
    let Some(address) = address else {
        return text.to_owned();
    };
    let mut out = text.to_owned();
    let plain = address.to_ascii_lowercase();
    let colons: String = plain
        .as_bytes()
        .chunks(2)
        .map(|pair| String::from_utf8_lossy(pair).into_owned())
        .collect::<Vec<_>>()
        .join(":");
    for (needle, hidden_from, hidden) in [(plain, 6, "xxxxxx"), (colons, 9, "xx:xx:xx")] {
        let mut from = 0;
        loop {
            // Lowercasing ASCII keeps every position, so they carry over.
            let lower = out.to_ascii_lowercase();
            let Some(found) = lower[from..].find(&needle) else {
                break;
            };
            let start = from + found + hidden_from;
            out.replace_range(start..start + hidden.len(), hidden);
            from = start + hidden.len();
        }
    }
    out
}

// ------------------------------------------------------------ section 2

#[derive(Default)]
struct Stats {
    /// Every advertisement the watcher handed over.
    ads: u64,
    /// The ones with Apple manufacturer data.
    apple_ads: u64,
    proximity: u64,
    models: BTreeSet<u16>,
    rssi: Vec<i16>,
    last: Option<Last>,
}

struct Last {
    summary: String,
    bucket: i16,
    at: Instant,
}

struct Watch {
    label: &'static str,
    started: Instant,
    stats: Mutex<Stats>,
}

impl Watch {
    fn stats(&self) -> std::sync::MutexGuard<'_, Stats> {
        self.stats.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn print(&self, rssi: i16, what: &str, summary: &str) {
        let at = self.started.elapsed().as_secs_f32();
        say(&format!(
            "t=+{at:.1}s [{}] rssi={rssi} {what} -> {summary}",
            self.label
        ));
    }
}

fn advertisements(length: Duration) {
    let started = Instant::now();
    let make = |label, filtered| {
        let watch = Arc::new(Watch {
            label,
            started,
            stats: Mutex::default(),
        });
        let watcher = start_watcher(&watch, filtered);
        (watch, watcher)
    };
    let (filtered, filtered_watcher) = make("F", true);
    let (unfiltered, unfiltered_watcher) = make("U", false);

    while !STOP.load(Ordering::Relaxed) && started.elapsed() < length {
        std::thread::sleep(Duration::from_millis(100));
    }
    for watcher in [filtered_watcher, unfiltered_watcher].into_iter().flatten() {
        let _ = watcher.Stop();
    }
    // A moment for the last handlers to finish.
    std::thread::sleep(Duration::from_millis(300));

    say("");
    say("== Summary ==");
    for watch in [&filtered, &unfiltered] {
        summarise(watch);
    }
    let heard = filtered.stats().ads > 0;
    say(&format!(
        "F (with the app's filter) received anything at all: {}",
        if heard { "yes" } else { "NO" }
    ));
}

fn start_watcher(watch: &Arc<Watch>, filtered: bool) -> Option<BluetoothLEAdvertisementWatcher> {
    let label = watch.label;
    let watcher = match BluetoothLEAdvertisementWatcher::new() {
        Ok(watcher) => watcher,
        Err(error) => {
            say(&format!("[{label}] no watcher: {error}"));
            return None;
        }
    };
    let _ = watcher.SetScanningMode(BluetoothLEScanningMode::Passive);
    if filtered && let Err(error) = filter_for_apple(&watcher) {
        say(&format!("[{label}] the filter could not be set: {error}"));
        return None;
    }

    let received = watch.clone();
    let _ = watcher.Received(&TypedEventHandler::<
        BluetoothLEAdvertisementWatcher,
        BluetoothLEAdvertisementReceivedEventArgs,
    >::new(move |_, args| {
        if let Some(args) = args.as_ref() {
            on_received(&received, args);
        }
        Ok(())
    }));

    let stopped = watch.clone();
    let _ = watcher.Stopped(&TypedEventHandler::<
        BluetoothLEAdvertisementWatcher,
        BluetoothLEAdvertisementWatcherStoppedEventArgs,
    >::new(move |_, args| {
        let error = args
            .as_ref()
            .and_then(|args| args.Error().ok())
            .map_or_else(|| "unknown".into(), |error| format!("{error:?}"));
        let at = stopped.started.elapsed().as_secs_f32();
        say(&format!(
            "t=+{at:.1}s [{}] watcher stopped, error status: {error}",
            stopped.label
        ));
        Ok(())
    }));

    match watcher.Start() {
        Ok(()) => {
            say(&format!(
                "[{label}] {} watcher started (passive)",
                if filtered {
                    "filtered (company 0x004C, no data bytes)"
                } else {
                    "unfiltered"
                }
            ));
            Some(watcher)
        }
        Err(error) => {
            say(&format!("[{label}] could not start: {error}"));
            None
        }
    }
}

/// The filter the app sets, copied from `sources/airpods.rs`.
fn filter_for_apple(watcher: &BluetoothLEAdvertisementWatcher) -> windows::core::Result<()> {
    let apple = BluetoothLEManufacturerData::new()?;
    apple.SetCompanyId(APPLE_COMPANY_ID)?;
    let pattern = BluetoothLEAdvertisement::new()?;
    pattern.ManufacturerData()?.Append(&apple)?;
    let filter = BluetoothLEAdvertisementFilter::new()?;
    filter.SetAdvertisement(&pattern)?;
    watcher.SetAdvertisementFilter(&filter)
}

fn on_received(watch: &Watch, args: &BluetoothLEAdvertisementReceivedEventArgs) {
    watch.stats().ads += 1;
    let Ok(rssi) = args.RawSignalStrengthInDBm() else {
        return;
    };
    let Some(sections) = args
        .Advertisement()
        .ok()
        .and_then(|advertisement| advertisement.ManufacturerData().ok())
    else {
        return;
    };
    let mut apple = false;
    for section in sections {
        if section.CompanyId().ok() != Some(APPLE_COMPANY_ID) {
            continue;
        }
        apple = true;
        let Some(data) = section.Data().ok().and_then(|buffer| bytes(&buffer)) else {
            continue;
        };
        report(watch, rssi, &data);
    }
    if apple {
        watch.stats().apple_ads += 1;
    }
}

fn bytes(buffer: &IBuffer) -> Option<Vec<u8>> {
    let length = buffer.Length().ok()?;
    let reader = DataReader::FromBuffer(buffer).ok()?;
    let mut bytes = vec![0; length as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(bytes)
}

/// What one Apple manufacturer section holds: a run of messages, each a type
/// byte, a length byte and that many bytes.
fn report(watch: &Watch, rssi: i16, data: &[u8]) {
    let mut others = Vec::new();
    let mut rest = data;
    let mut offset = 0;
    let mut proximity = false;
    while let [kind, len, tail @ ..] = rest {
        let len = usize::from(*len);
        if tail.len() < len {
            others.push(format!("0x{kind:02X}:{len}(cut)"));
            break;
        }
        let message = &data[offset..offset + 2 + len];
        if *kind == PROXIMITY_TYPE
            && len == PROXIMITY_LEN
            && let Some(status) = parse(message)
        {
            proximity = true;
            let model = status.model.id();
            {
                let mut stats = watch.stats();
                stats.proximity += 1;
                stats.models.insert(model);
                stats.rssi.push(rssi);
            }
            let head: Vec<String> = message
                .iter()
                .take(HEAD_LEN)
                .map(|byte| format!("{byte:02X}"))
                .collect();
            let what = format!(
                "type=0x{kind:02X} len={len} model=0x{model:04X} head={}",
                head.join(" ")
            );
            let summary = format!(
                "L={} R={} case={} both_in_case={} from_left={} lid_open={}",
                pod_text(&status.left),
                pod_text(&status.right),
                battery_text(status.case.battery, status.case.charging),
                status.both_in_case,
                status.from_left,
                status
                    .lid_open
                    .map_or("n/a".to_owned(), |open| open.to_string()),
            );
            line(watch, rssi, &what, &summary);
        } else {
            others.push(format!("0x{kind:02X}:{len}"));
        }
        offset += 2 + len;
        rest = &tail[len..];
    }
    if !proximity {
        let what = format!("messages=[{}]", others.join(","));
        line(watch, rssi, &what, "not proximity pairing");
    }
}

fn battery_text(battery: Option<u8>, charging: bool) -> String {
    let level = battery.map_or("?".to_owned(), |percent| format!("{percent}%"));
    if charging {
        format!("{level}+chg")
    } else {
        level
    }
}

fn pod_text(pod: &island_core::airpods::Pod) -> String {
    let mut text = battery_text(pod.battery, pod.charging);
    if pod.in_ear {
        text.push_str("+ear");
    }
    text
}

/// Prints a line unless it says what the last one did, at about the same
/// signal strength, a moment ago.
fn line(watch: &Watch, rssi: i16, what: &str, summary: &str) {
    let bucket = rssi.div_euclid(RSSI_BUCKET_DB);
    {
        let mut stats = watch.stats();
        let quiet = stats.last.as_ref().is_some_and(|last| {
            last.summary == summary && last.bucket == bucket && last.at.elapsed() < REPEAT_EVERY
        });
        if quiet {
            return;
        }
        stats.last = Some(Last {
            summary: summary.to_owned(),
            bucket,
            at: Instant::now(),
        });
    }
    watch.print(rssi, what, summary);
}

fn summarise(watch: &Watch) {
    let stats = watch.stats();
    let models: Vec<String> = stats
        .models
        .iter()
        .map(|model| format!("0x{model:04X}"))
        .collect();
    say(&format!(
        "[{}] ads={} apple_ads={} proximity_messages={} models=[{}]",
        watch.label,
        stats.ads,
        stats.apple_ads,
        stats.proximity,
        models.join(", ")
    ));
    let mut rssi = stats.rssi.clone();
    rssi.sort_unstable();
    match (rssi.first(), rssi.get(rssi.len() / 2), rssi.last()) {
        (Some(min), Some(median), Some(max)) => say(&format!(
            "[{}] proximity rssi min={min} median={median} max={max} dBm",
            watch.label
        )),
        _ => say(&format!("[{}] no proximity messages", watch.label)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_six_digits_of_the_address_are_hidden_in_both_spellings() {
        let id = r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004c_PID&2027\7&2a1b&0&A1B2C3D4E5F6_C00000000";
        assert_eq!(
            mask(id, Some("A1B2C3D4E5F6")),
            r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004c_PID&2027\7&2a1b&0&A1B2C3xxxxxx_C00000000"
        );
        assert_eq!(
            mask("Bluetooth#x-a1:b2:c3:d4:e5:f6", Some("A1B2C3D4E5F6")),
            "Bluetooth#x-a1:b2:c3:xx:xx:xx"
        );
        assert_eq!(mask("no address", None), "no address");
    }

    #[test]
    fn a_model_id_maps_to_its_model() {
        assert_eq!(model_of(0x2027), "AirPodsPro3");
    }
}
