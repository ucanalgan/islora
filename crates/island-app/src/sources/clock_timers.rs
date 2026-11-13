//! Follows the timers of the Windows Clock app.
//!
//! The Clock keeps them in a settings file of its own. Opening that file shares
//! it with the Clock rather than blocking it, and Windows then reports changes,
//! so this thread sleeps until a timer really was started, paused or stopped.
//! The file also holds the user's account and tasks, and only timers are read.
//! See `island_core::clock_timer` for what is in them.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use island_core::clock_timer::{self, ClockTimer};
use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS, HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    HKEY, KEY_READ, REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME, RegCloseKey, RegEnumValueW,
    RegLoadAppKeyW, RegNotifyChangeKeyValue, RegOpenKeyExW,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{HSTRING, PWSTR, w};

pub const MSG_CLOCK_TIMERS: u32 = WM_APP + 10;

const SETTINGS_FILE: &str =
    "Packages\\Microsoft.WindowsAlarms_8wekyb3d8bbwe\\Settings\\settings.dat";
/// The Clock writes a change in several steps.
const SETTLE: Duration = Duration::from_millis(300);
/// Far more than anyone keeps. A timer is one small record.
const MOST_TIMERS: u32 = 64;
const LONGEST_RECORD: usize = 2048;

pub struct ClockTimerWatch {
    updates: Receiver<Vec<ClockTimer>>,
}

impl ClockTimerWatch {
    pub fn start(hwnd: HWND) -> Self {
        let (publish, updates) = mpsc::channel();
        let window = hwnd.0 as isize;
        let spawned = std::thread::Builder::new()
            .name("clock timers".into())
            .spawn(move || {
                run(window, publish);
            });
        if let Err(error) = spawned {
            eprintln!("clock timer thread failed to start: {error}");
        }
        Self { updates }
    }

    pub fn latest(&self) -> Option<Vec<ClockTimer>> {
        self.updates.try_iter().last()
    }
}

/// `None` without the Clock app. The check matters: Windows would otherwise
/// create an empty settings file where the Clock expects its own.
fn open_settings() -> Option<HKEY> {
    let file = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join(SETTINGS_FILE);
    if !file.is_file() {
        return None;
    }
    let (mut hive, mut state) = (HKEY::default(), HKEY::default());
    unsafe {
        RegLoadAppKeyW(
            &HSTRING::from(file.as_os_str()),
            &mut hive,
            KEY_READ.0,
            0,
            None,
        )
        .ok()
        .ok()?;
        RegOpenKeyExW(hive, w!("LocalState"), None, KEY_READ, &mut state)
            .ok()
            .ok()?;
    }
    Some(state)
}

fn run(window: isize, publish: Sender<Vec<ClockTimer>>) {
    // Stays open for good, since that is what makes Windows report changes.
    let Some(state) = open_settings() else { return };
    let Ok(changed) = (unsafe { CreateEventW(None, false, false, None) }) else {
        return;
    };

    loop {
        // The folder above, as the one for timers only exists after a first one.
        let filter = REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET;
        let armed = unsafe { RegNotifyChangeKeyValue(state, true, filter, Some(changed), true) };
        if armed != ERROR_SUCCESS || publish.send(read_timers(state)).is_err() {
            return;
        }
        unsafe {
            let _ = PostMessageW(
                Some(HWND(window as _)),
                MSG_CLOCK_TIMERS,
                WPARAM(0),
                LPARAM(0),
            );
            WaitForSingleObject(changed, INFINITE);
        }
        std::thread::sleep(SETTLE);
    }
}

fn read_timers(state: HKEY) -> Vec<ClockTimer> {
    let mut key = HKEY::default();
    if unsafe { RegOpenKeyExW(state, w!("Timers"), None, KEY_READ, &mut key) } != ERROR_SUCCESS {
        return Vec::new();
    }

    let mut timers = Vec::new();
    let mut name = [0_u16; 64];
    let mut record = [0_u8; LONGEST_RECORD];
    for index in 0..MOST_TIMERS {
        let (mut name_length, mut size) = (name.len() as u32, record.len() as u32);
        let status = unsafe {
            RegEnumValueW(
                key,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut name_length,
                None,
                None,
                Some(record.as_mut_ptr()),
                Some(&mut size),
            )
        };
        match status {
            ERROR_SUCCESS => timers.extend(
                record
                    .get(..size as usize)
                    .and_then(clock_timer::parse_timer),
            ),
            ERROR_MORE_DATA => {}
            _ => break,
        }
    }
    unsafe {
        let _ = RegCloseKey(key);
    }
    timers
}
