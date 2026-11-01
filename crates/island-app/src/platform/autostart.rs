//! Starting with Windows, through the per-user list Task Manager also shows.

use std::iter::once;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::{PCWSTR, w};

const RUN: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const NAME: PCWSTR = w!("Islora");
const LONGEST_COMMAND: usize = 1024;

fn this_copy() -> Option<String> {
    Some(format!("\"{}\"", std::env::current_exe().ok()?.display()))
}

fn stored() -> Option<String> {
    let mut text = [0_u16; LONGEST_COMMAND];
    let mut size = size_of_val(&text) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            RUN,
            NAME,
            RRF_RT_REG_SZ,
            None,
            Some(text.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    let units = (size as usize / size_of::<u16>()).saturating_sub(1);
    (status == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&text[..units]))
}

/// Only true for this copy, as an entry for one that moved would start nothing.
pub fn is_on() -> bool {
    stored().is_some_and(|stored| Some(stored) == this_copy())
}

pub fn set(on: bool) {
    let status = unsafe {
        if on {
            let Some(command) = this_copy() else { return };
            let text: Vec<u16> = command.encode_utf16().chain(once(0)).collect();
            let size = size_of_val(text.as_slice()) as u32;
            RegSetKeyValueW(HKEY_CURRENT_USER, RUN, NAME, REG_SZ.0, Some(text.as_ptr().cast()), size)
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN, NAME)
        }
    };
    if status != ERROR_SUCCESS {
        eprintln!("starting with Windows could not be changed");
    }
}
