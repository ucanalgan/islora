//! Slow monitor brightness calls run on a worker, only when the panel requests them.

use std::sync::mpsc::{self, Receiver, Sender};
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, PHYSICAL_MONITOR, SetMonitorBrightness,
};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

mod integrated;

pub const MSG_QUICK_BRIGHTNESS: u32 = WM_APP + 27;
enum Command {
    Read,
    Set(u8),
}

pub struct QuickBrightness {
    commands: Sender<Command>,
    updates: Receiver<Option<u8>>,
}

impl QuickBrightness {
    pub fn start(hwnd: HWND) -> Self {
        let (commands, inbox) = mpsc::channel();
        let (publish, updates) = mpsc::channel();
        let window = hwnd.0 as isize;
        if let Err(error) = std::thread::Builder::new()
            .name("quick-brightness".into())
            .spawn(move || {
                let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
                while let Ok(command) = inbox.recv() {
                    let hwnd = HWND(window as _);
                    let requested = match command {
                        Command::Read => None,
                        Command::Set(value) => Some(value),
                    };
                    let value = brightness(hwnd, requested).or_else(|| {
                        // Without a verified monitor-to-WMI mapping, never change a
                        // different display in a multi-monitor configuration.
                        if initialized && unsafe { GetSystemMetrics(SM_CMONITORS) } == 1 {
                            integrated::brightness(hwnd, requested)
                        } else {
                            None
                        }
                    });
                    if publish.send(value).is_err() {
                        break;
                    }
                    unsafe {
                        let _ =
                            PostMessageW(Some(hwnd), MSG_QUICK_BRIGHTNESS, WPARAM(0), LPARAM(0));
                    }
                }
                if initialized {
                    unsafe { CoUninitialize() };
                }
            })
        {
            eprintln!("brightness worker failed: {error}");
        }
        Self { commands, updates }
    }

    pub fn read(&self) {
        if self.commands.send(Command::Read).is_err() {
            eprintln!("brightness worker is unavailable");
        }
    }
    pub fn set(&self, percent: u8) {
        if self.commands.send(Command::Set(percent.min(100))).is_err() {
            eprintln!("brightness worker is unavailable");
        }
    }
    pub fn latest(&self) -> Option<Option<u8>> {
        self.updates.try_iter().last()
    }
}

fn brightness(hwnd: HWND, requested: Option<u8>) -> Option<u8> {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut count = 0;
        GetNumberOfPhysicalMonitorsFromHMONITOR(monitor, &mut count).ok()?;
        if count == 0 || count > 16 {
            return None;
        }
        let mut physical = vec![PHYSICAL_MONITOR::default(); count as usize];
        GetPhysicalMonitorsFromHMONITOR(monitor, &mut physical).ok()?;
        let value = physical.iter().find_map(|monitor| {
            let (mut min, mut current, mut max) = (0u32, 0u32, 0u32);
            if GetMonitorBrightness(monitor.hPhysicalMonitor, &mut min, &mut current, &mut max) == 0
                || max <= min
            {
                return None;
            }
            if let Some(percent) = requested {
                let target = min + ((max - min) as u64 * u64::from(percent) / 100) as u32;
                if SetMonitorBrightness(monitor.hPhysicalMonitor, target) == 0 {
                    eprintln!("monitor brightness change failed");
                    return None;
                }
                if GetMonitorBrightness(monitor.hPhysicalMonitor, &mut min, &mut current, &mut max)
                    == 0
                    || max <= min
                {
                    return None;
                }
            }
            Some(((current.saturating_sub(min) as u64 * 100) / u64::from(max - min)).min(100) as u8)
        });
        if let Err(error) = DestroyPhysicalMonitors(&physical) {
            eprintln!("monitor handle cleanup failed: {error}");
        }
        value
    }
}
