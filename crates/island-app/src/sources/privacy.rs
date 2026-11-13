//! Which apps use the microphone and the camera. Windows keeps this for its own
//! privacy page: a key per app, whose last stop time is zero while in use. The
//! layout is not documented, so a missing key just means nothing to report.

use std::sync::mpsc::{self, Receiver, Sender};

use island_core::Indicator;
use island_core::indicator::capability_app_name;
use windows::Win32::Foundation::{ERROR_SUCCESS, HWND, LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, RRF_RT_REG_QWORD, RegCloseKey, RegEnumKeyExW, RegGetValueW,
    RegOpenKeyExW,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{PCWSTR, PWSTR, w};

use super::{IndicatorSink, registry};

pub const MSG_CAMERA: u32 = WM_APP + 11;

const MICROPHONE: PCWSTR = w!(
    "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\microphone"
);
const CAMERA: PCWSTR = w!(
    "Software\\Microsoft\\Windows\\CurrentVersion\\CapabilityAccessManager\\ConsentStore\\webcam"
);
/// Far more apps than anyone has, as a limit for a loop over foreign data.
const MOST_APPS: u32 = 1024;

/// Follows both. The microphone is a passing notice, while the camera shows for
/// as long as it is on, which is why only that one has a state to read.
pub struct CameraWatch {
    updates: Receiver<Option<String>>,
}

impl CameraWatch {
    pub fn start(hwnd: HWND, sink: IndicatorSink) -> Self {
        let (publish, updates) = mpsc::channel();
        let window = hwnd.0 as isize;
        let microphone = std::thread::Builder::new()
            .name("microphone use".into())
            .spawn(move || {
                follow_microphone(&sink);
            });
        let camera = std::thread::Builder::new()
            .name("camera use".into())
            .spawn(move || {
                follow_camera(window, &publish);
            });
        if microphone.is_err() || camera.is_err() {
            eprintln!("privacy threads failed to start");
        }
        Self { updates }
    }

    /// `Some(None)` means the camera was turned off.
    pub fn latest(&self) -> Option<Option<String>> {
        self.updates.try_iter().last()
    }
}

fn open_below(parent: HKEY, path: PCWSTR) -> Option<HKEY> {
    let mut key = HKEY::default();
    let status = unsafe { RegOpenKeyExW(parent, path, None, KEY_READ, &mut key) };
    (status == ERROR_SUCCESS).then_some(key)
}

fn follow_microphone(sink: &IndicatorSink) {
    let Some(store) = open_below(HKEY_CURRENT_USER, MICROPHONE) else {
        return;
    };
    let mut known = apps_in_use(store);
    registry::follow(store, || {
        let now = apps_in_use(store);
        for app in now.iter().filter(|app| !known.contains(app)) {
            sink.send(Indicator::MicrophoneInUse { app: app.clone() });
        }
        known = now;
    });
}

fn follow_camera(window: isize, publish: &Sender<Option<String>>) {
    let Some(store) = open_below(HKEY_CURRENT_USER, CAMERA) else {
        return;
    };
    let tell = |app: Option<String>| {
        if publish.send(app).is_ok() {
            unsafe {
                let _ = PostMessageW(Some(HWND(window as _)), MSG_CAMERA, WPARAM(0), LPARAM(0));
            }
        }
    };
    let mut known = apps_in_use(store).into_iter().next();
    if known.is_some() {
        tell(known.clone());
    }
    registry::follow(store, || {
        let now = apps_in_use(store).into_iter().next();
        if now != known {
            tell(now.clone());
            known = now;
        }
    });
}

/// Store apps sit directly in `store`, and everything else one level down.
fn apps_in_use(store: HKEY) -> Vec<String> {
    let mut apps = Vec::new();
    add_apps_in_use(store, &mut apps);
    if let Some(desktop) = open_below(store, w!("NonPackaged")) {
        add_apps_in_use(desktop, &mut apps);
        unsafe {
            let _ = RegCloseKey(desktop);
        }
    }
    apps.sort();
    apps.dedup();
    apps
}

fn add_apps_in_use(parent: HKEY, apps: &mut Vec<String>) {
    let mut name = [0_u16; 512];
    for index in 0..MOST_APPS {
        let mut length = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExW(
                parent,
                index,
                Some(PWSTR(name.as_mut_ptr())),
                &mut length,
                None,
                None,
                None,
                None,
            )
        };
        if status != ERROR_SUCCESS {
            return;
        }
        let entry = PCWSTR(name.as_ptr());
        let started = moment(parent, entry, w!("LastUsedTimeStart"));
        let stopped = moment(parent, entry, w!("LastUsedTimeStop"));
        if started.is_some_and(|started| started != 0) && stopped == Some(0) {
            apps.push(capability_app_name(&String::from_utf16_lossy(
                &name[..length as usize],
            )));
        }
    }
}

fn moment(parent: HKEY, entry: PCWSTR, value: PCWSTR) -> Option<u64> {
    let mut moment = 0_u64;
    let mut size = size_of::<u64>() as u32;
    let status = unsafe {
        RegGetValueW(
            parent,
            entry,
            value,
            RRF_RT_REG_QWORD,
            None,
            Some((&raw mut moment).cast()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(moment)
}
