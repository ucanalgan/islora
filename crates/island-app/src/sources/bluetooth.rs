//! Bluetooth devices connecting and disconnecting. A device watcher filtered on
//! "connected" reports a device as added or removed when that status changes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

/// Where Windows keeps a device's battery level, for the devices that report one.
const BATTERY_PROPERTY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";
/// A device tells its battery level a moment after connecting.
const BATTERY_DELAY: Duration = Duration::from_secs(3);

pub struct BluetoothWatch {
    watchers: Vec<DeviceWatcher>,
}

impl BluetoothWatch {
    pub fn start(sink: IndicatorSink) -> Result<Self> {
        let connected = BluetoothConnectionStatus::Connected;
        let selectors = [
            BluetoothDevice::GetDeviceSelectorFromConnectionStatus(connected)?,
            BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(connected)?,
        ];
        let mut watchers = Vec::with_capacity(selectors.len());
        for selector in &selectors {
            watchers.push(watch(selector, sink.clone())?);
        }
        Ok(Self { watchers })
    }
}

impl Drop for BluetoothWatch {
    fn drop(&mut self) {
        for watcher in &self.watchers {
            let _ = watcher.Stop();
        }
    }
}

fn watch(selector: &HSTRING, sink: IndicatorSink) -> Result<DeviceWatcher> {
    let watcher = DeviceInformation::CreateWatcherAqsFilter(selector)?;
    // A removal only carries the id, so names are remembered from when added.
    let names: Arc<Mutex<HashMap<String, String>>> = Arc::default();
    // Everything already connected is listed first, which is not news.
    let enumerated = Arc::new(AtomicBool::new(false));

    let added_names = names.clone();
    let added_sink = sink.clone();
    let added_enumerated = enumerated.clone();
    watcher.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(
        move |_, device| {
            let Some(device) = device.as_ref() else {
                return Ok(());
            };
            let name = device.Name()?.to_string();
            let id = device.Id()?.to_string();
            added_names.lock().unwrap().insert(id.clone(), name.clone());
            if added_enumerated.load(Ordering::Relaxed) {
                added_sink.send(Indicator::BluetoothConnected { name: name.clone() });
                report_battery(id, name, added_sink.clone());
            }
            Ok(())
        },
    ))?;

    watcher.Removed(
        &TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, device| {
            let Some(device) = device.as_ref() else {
                return Ok(());
            };
            if let Some(name) = names.lock().unwrap().remove(&device.Id()?.to_string()) {
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

fn report_battery(id: String, name: String, sink: IndicatorSink) {
    std::thread::spawn(move || {
        std::thread::sleep(BATTERY_DELAY);
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        if let Some(percent) = bluetooth_address(&id).and_then(|address| battery_level(&address)) {
            sink.send(Indicator::BluetoothBattery { name, percent });
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
