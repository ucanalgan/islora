//! AirPods' battery and ear state, read from what they advertise over
//! Bluetooth LE while they are connected.
//!
//! Windows reports one battery level for AirPods at best. The earbuds also
//! broadcast Apple's proximity pairing message, which `island_core::airpods`
//! reads and its `Tracker` turns into notices. Scanning happens only while a
//! matching headset is connected: it is never done without a reason, for
//! privacy and for the battery.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use island_core::activity::Millis;
use island_core::airpods::{APPLE_COMPANY_ID, Tracker, apple_audio_pid, parse};
use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisement, BluetoothLEAdvertisementFilter,
    BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
    BluetoothLEAdvertisementWatcherStoppedEventArgs, BluetoothLEManufacturerData,
    BluetoothLEScanningMode,
};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind};
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::{DataReader, IBuffer};
use windows::core::HSTRING;
use windows_collections::IIterable;

use super::IndicatorSink;

/// A scan that is running, and which one, so that the stop of an earlier scan
/// is not mistaken for the stop of this one.
struct Scan {
    generation: u64,
    watcher: BluetoothLEAdvertisementWatcher,
}

#[derive(Default)]
struct State {
    /// The model id of every connected Bluetooth device that is AirPods, by
    /// device id. The classic and the LE connection of one pair of AirPods are
    /// two devices.
    devices: HashMap<String, u16>,
    scan: Option<Scan>,
    scans_started: u64,
    /// The app is shutting down, and nothing may start scanning again.
    closed: bool,
}

struct Inner {
    sink: IndicatorSink,
    /// The tracker's clock. It only counts up, unlike the wall clock.
    started: Instant,
    /// Locked briefly by the advertisement handler, so it is kept apart from
    /// `state`, whose lock is held while a scan starts or stops.
    tracker: Mutex<Tracker>,
    state: Mutex<State>,
}

/// Shared by the Bluetooth device watch, whose callbacks and threads tell it
/// which AirPods are connected, and the advertisement handler.
#[derive(Clone)]
pub struct AirPods {
    inner: Arc<Inner>,
}

impl AirPods {
    pub fn new(sink: IndicatorSink) -> Self {
        Self {
            inner: Arc::new(Inner {
                sink,
                started: Instant::now(),
                tracker: Mutex::default(),
                state: Mutex::default(),
            }),
        }
    }

    /// A Bluetooth device that is AirPods has connected. `quiet` is for the
    /// ones that were connected when the app started, which is not news.
    /// Starts scanning if it is not already.
    pub fn connected(&self, device_id: &str, name: &str, model_id: u16, quiet: bool) {
        let now = self.inner.now();
        // The state is always locked before the tracker, never the other way.
        let mut state = lock(&self.inner.state);
        if state.closed {
            return;
        }
        {
            let mut tracker = lock(&self.inner.tracker);
            if quiet {
                tracker.connected_quietly(name.to_owned(), model_id, now);
            } else {
                tracker.connected(name.to_owned(), model_id, now);
            }
        }
        state.devices.insert(device_id.to_owned(), model_id);
        if state.scan.is_none() {
            self.start_scan(&mut state);
        }
    }

    /// Gives up waiting for the AirPods' own connect notice. Returns whether
    /// it was still owed, in which case the caller says what it has instead,
    /// and no AirPods connect notice comes any more for this connection.
    pub fn give_up_announcing(&self, model_id: u16) -> bool {
        lock(&self.inner.tracker).give_up_announcing(model_id)
    }

    /// A Bluetooth device has disconnected. Nothing happens unless it was
    /// connected here as AirPods. The scan stops with the last of them.
    ///
    /// A connection stays registered for as long as any device id of that
    /// model does, so a removal event that never arrives keeps the scan going
    /// until the app ends or the other id goes too.
    pub fn disconnected(&self, device_id: &str) {
        let scan = {
            let mut state = lock(&self.inner.state);
            let Some(model_id) = state.devices.remove(device_id) else {
                return;
            };
            if !state.devices.values().any(|&other| other == model_id) {
                lock(&self.inner.tracker).disconnected(model_id);
            }
            if state.devices.is_empty() {
                state.scan.take()
            } else {
                None
            }
        };
        stop(scan);
    }

    /// Stops scanning, for when the app shuts down.
    pub fn shutdown(&self) {
        let scan = {
            let mut state = lock(&self.inner.state);
            state.closed = true;
            state.scan.take()
        };
        stop(scan);
    }

    /// Starts the scan, or leaves things as they are if it cannot be started,
    /// such as without a Bluetooth LE adapter or with the radio off. The next
    /// connection tries again.
    fn start_scan(&self, state: &mut State) {
        let generation = state.scans_started + 1;
        let Some(watcher) = self.make_watcher(generation) else {
            return;
        };
        state.scans_started = generation;
        state.scan = Some(Scan {
            generation,
            watcher,
        });
    }

    fn make_watcher(&self, generation: u64) -> Option<BluetoothLEAdvertisementWatcher> {
        let watcher = BluetoothLEAdvertisementWatcher::new().ok()?;
        // Listening without asking the earbuds for anything.
        watcher
            .SetScanningMode(BluetoothLEScanningMode::Passive)
            .ok()?;
        // Without the filter the handler would hear every advertisement
        // around. The scan still works if it cannot be set, and the handler
        // checks the company id itself either way.
        let _ = filter_for_apple(&watcher);

        // The handlers only hold a weak hold on what they report to, so that
        // the watcher they belong to does not keep it alive in a circle.
        let weak = Arc::downgrade(&self.inner);
        watcher
            .Received(&TypedEventHandler::<
                BluetoothLEAdvertisementWatcher,
                BluetoothLEAdvertisementReceivedEventArgs,
            >::new(move |_, args| {
                if let (Some(inner), Some(args)) = (weak.upgrade(), args.as_ref()) {
                    inner.received(args);
                }
                Ok(())
            }))
            .ok()?;

        let weak = Arc::downgrade(&self.inner);
        watcher
            .Stopped(&TypedEventHandler::<
                BluetoothLEAdvertisementWatcher,
                BluetoothLEAdvertisementWatcherStoppedEventArgs,
            >::new(move |_, _| {
                if let Some(inner) = weak.upgrade() {
                    inner.scan_stopped(generation);
                }
                Ok(())
            }))
            .ok()?;

        watcher.Start().ok()?;
        Some(watcher)
    }
}

/// Asks Windows to report only advertisements with Apple's manufacturer data.
///
/// This is not verified on a real device: a filter section with a company id
/// and no data bytes should match any data of that company, but if Windows
/// reads it as data that must be empty, no advertisement would ever arrive.
fn filter_for_apple(watcher: &BluetoothLEAdvertisementWatcher) -> windows::core::Result<()> {
    let apple = BluetoothLEManufacturerData::new()?;
    apple.SetCompanyId(APPLE_COMPANY_ID)?;
    let pattern = BluetoothLEAdvertisement::new()?;
    pattern.ManufacturerData()?.Append(&apple)?;
    let filter = BluetoothLEAdvertisementFilter::new()?;
    filter.SetAdvertisement(&pattern)?;
    watcher.SetAdvertisementFilter(&filter)
}

impl Inner {
    fn now(&self) -> Millis {
        self.started.elapsed().as_millis() as Millis
    }

    /// Runs on a system thread. Only the tracker is locked, and only for the
    /// moment it takes to look at the message.
    fn received(&self, args: &BluetoothLEAdvertisementReceivedEventArgs) {
        let Some(status) = apple_message(args) else {
            return;
        };
        let Ok(signal) = args.RawSignalStrengthInDBm() else {
            return;
        };
        let now = self.now();
        let indicator = lock(&self.tracker).advertised(&status, signal, now);
        if let Some(indicator) = indicator {
            self.sink.send(indicator);
        }
    }

    /// The scan ended on its own, as when the radio is turned off. It starts
    /// again with the next connection. A scan that was stopped on purpose is
    /// already gone, and a newer one is not this one.
    fn scan_stopped(&self, generation: u64) {
        let mut state = lock(&self.state);
        if state
            .scan
            .as_ref()
            .is_some_and(|scan| scan.generation == generation)
        {
            state.scan = None;
        }
    }
}

/// Stops a scan once the state is unlocked, so that a stop handler that runs
/// at once cannot wait for the lock held by its own caller.
fn stop(scan: Option<Scan>) {
    if let Some(scan) = scan {
        let _ = scan.watcher.Stop();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The AirPods status in an advertisement's Apple manufacturer data, if any.
fn apple_message(
    args: &BluetoothLEAdvertisementReceivedEventArgs,
) -> Option<island_core::airpods::Status> {
    let sections = args.Advertisement().ok()?.ManufacturerData().ok()?;
    sections.into_iter().find_map(|section| {
        if section.CompanyId().ok()? != APPLE_COMPANY_ID {
            return None;
        }
        parse(&buffer_bytes(&section.Data().ok()?)?)
    })
}

fn buffer_bytes(buffer: &IBuffer) -> Option<Vec<u8>> {
    let length = buffer.Length().ok()?;
    let reader = DataReader::FromBuffer(buffer).ok()?;
    let mut bytes = vec![0; length as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(bytes)
}

/// The model id of the AirPods at this Bluetooth address, if the device is
/// one. Windows lists a device's profiles as devices of their own, whose
/// instance ids end in the address and, for Apple's, carry its vendor and
/// product id. Searching for the address alone finds them all, where the
/// device itself would only be found by its `DEV_` name, which has neither.
///
/// This blocks, so it is not for the callbacks.
pub fn airpods_model_id(address: &str) -> Option<u16> {
    let filter = HSTRING::from(format!("System.Devices.DeviceInstanceId:~~\"{address}\""));
    let none = IIterable::<HSTRING>::from(Vec::new());
    let devices = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(
        &filter,
        &none,
        DeviceInformationKind::Device,
    )
    .ok()?
    .join()
    .ok()?;
    let ids: Vec<String> = devices
        .into_iter()
        .filter_map(|device| Some(device.Id().ok()?.to_string()))
        .collect();
    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
    apple_audio_pid(&ids)
}
