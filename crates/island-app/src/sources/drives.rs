//! USB drives coming and going. Windows tells every top-level window by itself,
//! so nothing runs here until one does.

use std::collections::HashMap;

use island_core::Indicator;
use island_core::indicator::drive_letters;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumeInformationW};
use windows::Win32::UI::WindowsAndMessaging::{
    DBT_DEVICEARRIVAL, DBT_DEVICEREMOVECOMPLETE, DBT_DEVTYP_VOLUME, DBTF_NET, DEV_BROADCAST_HDR,
    DEV_BROADCAST_VOLUME,
};
use windows::core::HSTRING;

const DRIVE_REMOVABLE: u32 = 2;
/// External disks count as fixed, but only new ones ever arrive.
const DRIVE_FIXED: u32 = 3;

/// A removed drive can no longer be asked its name, so it is kept from before.
#[derive(Default)]
pub struct Drives {
    names: HashMap<char, String>,
}

impl Drives {
    /// Decodes `WM_DEVICECHANGE`, which Windows also sends for much else.
    pub fn changed(&mut self, wparam: WPARAM, lparam: LPARAM) -> Vec<Indicator> {
        let event = wparam.0 as u32;
        let arrived = event == DBT_DEVICEARRIVAL;
        if !arrived && event != DBT_DEVICEREMOVECOMPLETE {
            return Vec::new();
        }
        let is_volume = unsafe { (lparam.0 as *const DEV_BROADCAST_HDR).as_ref() }
            .is_some_and(|header| header.dbch_devicetype == DBT_DEVTYP_VOLUME);
        if !is_volume {
            return Vec::new();
        }
        let volume = unsafe { &*(lparam.0 as *const DEV_BROADCAST_VOLUME) };
        if volume.dbcv_flags.0 & DBTF_NET.0 != 0 {
            return Vec::new();
        }
        let letters = drive_letters(volume.dbcv_unitmask);
        letters.into_iter().filter_map(|letter| self.drive_changed(letter, arrived)).collect()
    }

    fn drive_changed(&mut self, letter: char, arrived: bool) -> Option<Indicator> {
        if !arrived {
            let name = self.names.remove(&letter).unwrap_or_else(|| format!("{letter}:"));
            return Some(Indicator::Usb { added: false, name });
        }
        let root = HSTRING::from(format!("{letter}:\\"));
        let kind = unsafe { GetDriveTypeW(&root) };
        if kind != DRIVE_REMOVABLE && kind != DRIVE_FIXED {
            return None;
        }
        let name = drive_name(&root, letter);
        self.names.insert(letter, name.clone());
        Some(Indicator::Usb { added: true, name })
    }
}

fn drive_name(root: &HSTRING, letter: char) -> String {
    let mut label = [0_u16; 64];
    let read = unsafe { GetVolumeInformationW(root, Some(&mut label), None, None, None, None) };
    let length = label.iter().position(|&unit| unit == 0).unwrap_or(label.len());
    if read.is_err() || length == 0 {
        return format!("{letter}:");
    }
    format!("{} ({letter}:)", String::from_utf16_lossy(&label[..length]))
}
