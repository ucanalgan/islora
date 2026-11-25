//! Bluetooth devices connecting and disconnecting. A device watcher filtered on
//! "connected" reports a device as added or removed when that status changes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use island_core::Indicator;
use island_core::indicator::bluetooth_address;
use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::{
    DeviceInformation, DeviceInformationKind, DeviceInformationUpdate, DeviceWatcher,
};
use windows::Foundation::{IPropertyValue, TypedEventHandler};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::core::{HSTRING, IInspectable, Interface, Result};
use windows_collections::IIterable;

use super::IndicatorSink;
use super::airpods::{AirPods, find_airpods};

/// Where Windows keeps a device's battery level, for the devices that report one.
const BATTERY_PROPERTY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";
/// A device tells its battery level a moment after connecting.
const BATTERY_DELAY: Duration = Duration::from_secs(3);
/// How long AirPods have to announce themselves before the generic notice is
/// given instead: scanning may be impossible, or what is advertised may not be
/// what Windows lists.
const AIRPODS_PATIENCE: Duration = Duration::from_secs(10);

/// What is remembered of a connected device from when it was added. A removal
/// only carries the id.
struct Known {
    name: String,
    /// Which time it was added. The same device can be removed and added again
    /// while a lookup for its earlier time is still running.
    generation: u64,
}

type Names = Arc<Mutex<HashMap<String, Known>>>;

/// Counts every addition of a device, of any selector.
static ADDITIONS: AtomicU64 = AtomicU64::new(0);

/// Whether the device is still connected as it was when this lookup started.
fn is_current(names: &HashMap<String, Known>, id: &str, generation: u64) -> bool {
    names
        .get(id)
        .is_some_and(|known| known.generation == generation)
}

pub struct BluetoothWatch {
    watchers: Vec<DeviceWatcher>,
    airpods: AirPods,
}

impl BluetoothWatch {
    pub fn start(sink: IndicatorSink) -> Result<Self> {
        let connected = BluetoothConnectionStatus::Connected;
        let selectors = [
            BluetoothDevice::GetDeviceSelectorFromConnectionStatus(connected)?,
            BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(connected)?,
        ];
        let airpods = AirPods::new(sink.clone());
        let mut watchers: Vec<DeviceWatcher> = Vec::with_capacity(selectors.len());
        for selector in &selectors {
            match watch(selector, sink.clone(), airpods.clone()) {
                Ok(watcher) => watchers.push(watcher),
                Err(error) => {
                    // The watchers that did start would otherwise go on, with
                    // nothing left to stop them.
                    for started in &watchers {
                        let _ = started.Stop();
                    }
                    airpods.shutdown();
                    return Err(error);
                }
            }
        }
        Ok(Self { watchers, airpods })
    }
}

impl Drop for BluetoothWatch {
    fn drop(&mut self) {
        for watcher in &self.watchers {
            let _ = watcher.Stop();
        }
        self.airpods.shutdown();
    }
}

fn watch(selector: &HSTRING, sink: IndicatorSink, airpods: AirPods) -> Result<DeviceWatcher> {
    let watcher = DeviceInformation::CreateWatcherAqsFilter(selector)?;
    // A removal only carries the id, so names are remembered from when added.
    let names: Names = Arc::default();
    // Everything already connected is listed first, which is not news.
    let enumerated = Arc::new(AtomicBool::new(false));

    let added_names = names.clone();
    let added_sink = sink.clone();
    let added_airpods = airpods.clone();
    let added_enumerated = enumerated.clone();
    watcher.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(
        move |_, device| {
            let Some(device) = device.as_ref() else {
                return Ok(());
            };
            let name = device.Name()?.to_string();
            let id = device.Id()?.to_string();
            let generation = ADDITIONS.fetch_add(1, Ordering::Relaxed) + 1;
            added_names.lock().unwrap().insert(
                id.clone(),
                Known {
                    name: name.clone(),
                    generation,
                },
            );
            // Which device this is takes a lookup that blocks, so the rest is
            // done on a thread of its own. Devices already connected are only
            // looked at in case they are AirPods, to warn when they run low.
            let news = added_enumerated.load(Ordering::Relaxed);
            if news {
                added_sink.send(Indicator::BluetoothConnected { name: name.clone() });
            }
            identify(
                Device {
                    id,
                    name,
                    generation,
                },
                news,
                added_names.clone(),
                added_airpods.clone(),
                added_sink.clone(),
            );
            Ok(())
        },
    ))?;

    watcher.Removed(
        &TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, device| {
            let Some(device) = device.as_ref() else {
                return Ok(());
            };
            let id = device.Id()?.to_string();
            let name = names.lock().unwrap().remove(&id).map(|known| known.name);
            // After the name is gone, so that a lookup still running for this
            // device does not register it as connected.
            airpods.disconnected(&id);
            if let Some(name) = name {
                sink.send(Indicator::BluetoothDisconnected { name });
            }
            Ok(())
        }),
    )?;

    // The watcher only delivers removals when this handler exists too.
    watcher.Updated(
        &TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(|_, _| Ok(())),
    )?;

    watcher.EnumerationCompleted(&TypedEventHandler::<DeviceWatcher, IInspectable>::new(
        move |_, _| {
            enumerated.store(true, Ordering::Relaxed);
            Ok(())
        },
    ))?;

    watcher.Start()?;
    Ok(watcher)
}

struct Device {
    id: String,
    name: String,
    generation: u64,
}

/// Finds out whether a connected device is AirPods. If it is, they are handed
/// to the AirPods source and the generic battery notice stays out of the way,
/// since that one tells a single level. If nothing comes of them within a
/// while, the generic notice is given after all: the user gets one of the two.
/// Otherwise a new connection gets the battery notice a moment after it
/// connects, as always.
fn identify(device: Device, news: bool, names: Names, airpods: AirPods, sink: IndicatorSink) {
    std::thread::spawn(move || {
        let started = Instant::now();
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let address = bluetooth_address(&device.id);
        if let Some(found) = address.as_deref().and_then(find_airpods) {
            let model_id = found.model_id;
            // Under the lock, so that a removal, or one followed by another
            // addition, cannot slip in between the check and the registration.
            {
                let names = names.lock().unwrap();
                if !is_current(&names, &device.id, device.generation) {
                    return;
                }
                airpods.connected(&device.id, &device.name, found, !news);
            }
            if !news {
                return;
            }
            std::thread::sleep(AIRPODS_PATIENCE.saturating_sub(started.elapsed()));
            // Still under the lock, and only the one that is owed the notice
            // gives it, so it is never both and never neither.
            let owed = {
                let names = names.lock().unwrap();
                is_current(&names, &device.id, device.generation)
                    && airpods.give_up_announcing(model_id)
            };
            if !owed {
                return;
            }
        } else if news {
            std::thread::sleep(BATTERY_DELAY.saturating_sub(started.elapsed()));
        } else {
            return;
        }
        if let Some(percent) = address.and_then(|address| battery_level(&address)) {
            sink.send(Indicator::BluetoothBattery {
                name: device.name,
                percent,
            });
        }
    });
}

fn battery_level(address: &str) -> Option<u8> {
    let filter = HSTRING::from(format!(
        "System.Devices.DeviceInstanceId:~~\"DEV_{address}\""
    ));
    let property = HSTRING::from(BATTERY_PROPERTY);
    let wanted = IIterable::<HSTRING>::from(vec![property.clone()]);
    let devices = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(
        &filter,
        &wanted,
        DeviceInformationKind::Device,
    )
    .ok()?
    .join()
    .ok()?;
    devices.into_iter().find_map(|device| {
        let value = device
            .Properties()
            .ok()?
            .Lookup(&property)
            .ok()?
            .cast::<IPropertyValue>()
            .ok()?;
        value.GetUInt8().ok().filter(|&percent| percent <= 100)
    })
}
