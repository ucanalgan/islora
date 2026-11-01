//! Sleeping until a registry key changes, which costs nothing until it does.

use std::time::Duration;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME, RegNotifyChangeKeyValue,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};

/// Writers usually touch several values in a row.
const SETTLE: Duration = Duration::from_millis(300);

/// Calls `changed` after each change in or below `key`, for as long as it can.
pub fn follow(key: HKEY, mut changed: impl FnMut()) {
    let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else { return };
    let filter = REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET;
    while unsafe { RegNotifyChangeKeyValue(key, true, filter, Some(event), true) } == ERROR_SUCCESS {
        unsafe { WaitForSingleObject(event, INFINITE) };
        std::thread::sleep(SETTLE);
        changed();
    }
}
