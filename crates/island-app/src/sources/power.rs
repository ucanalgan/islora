//! Power source, battery level and screen brightness, all delivered as
//! `WM_POWERBROADCAST`. Windows sends one report of each right after
//! registering, which `SystemTracker` uses as the starting point.

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WPARAM};
use windows::Win32::System::Power::{
    HPOWERNOTIFY, POWERBROADCAST_SETTING, RegisterPowerSettingNotification,
    UnregisterPowerSettingNotification,
};
use windows::Win32::System::SystemServices::{
    GUID_ACDC_POWER_SOURCE, GUID_BATTERY_PERCENTAGE_REMAINING, GUID_POWER_SAVING_STATUS,
    GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS,
};
use windows::Win32::UI::WindowsAndMessaging::{DEVICE_NOTIFY_WINDOW_HANDLE, PBT_POWERSETTINGCHANGE};
use windows::core::Result;

#[derive(Debug, PartialEq)]
pub enum PowerEvent {
    Source { on_battery: bool },
    Battery(u8),
    Brightness(u8),
    BatterySaver(bool),
}

pub struct PowerWatch {
    registrations: Vec<HPOWERNOTIFY>,
}

impl PowerWatch {
    pub fn start(hwnd: HWND) -> Result<Self> {
        let settings = [
            GUID_ACDC_POWER_SOURCE,
            GUID_BATTERY_PERCENTAGE_REMAINING,
            GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS,
            GUID_POWER_SAVING_STATUS,
        ];
        let mut registrations = Vec::with_capacity(settings.len());
        for setting in &settings {
            let registration = unsafe {
                RegisterPowerSettingNotification(HANDLE(hwnd.0), setting, DEVICE_NOTIFY_WINDOW_HANDLE)
            }?;
            registrations.push(registration);
        }
        Ok(Self { registrations })
    }
}

impl Drop for PowerWatch {
    fn drop(&mut self) {
        for registration in self.registrations.drain(..) {
            unsafe {
                let _ = UnregisterPowerSettingNotification(registration);
            }
        }
    }
}

/// Decodes `WM_POWERBROADCAST` for the settings registered above.
pub fn decode(wparam: WPARAM, lparam: LPARAM) -> Option<PowerEvent> {
    if wparam.0 as u32 != PBT_POWERSETTINGCHANGE {
        return None;
    }
    let setting = unsafe { &*(lparam.0 as *const POWERBROADCAST_SETTING) };
    if (setting.DataLength as usize) < size_of::<u32>() {
        return None;
    }
    let value = unsafe { setting.Data.as_ptr().cast::<u32>().read_unaligned() };
    match setting.PowerSetting {
        GUID_ACDC_POWER_SOURCE => Some(PowerEvent::Source { on_battery: value != 0 }),
        GUID_BATTERY_PERCENTAGE_REMAINING => Some(PowerEvent::Battery(value.min(100) as u8)),
        GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS => Some(PowerEvent::Brightness(value.min(100) as u8)),
        GUID_POWER_SAVING_STATUS => Some(PowerEvent::BatterySaver(value != 0)),
        _ => None,
    }
}
