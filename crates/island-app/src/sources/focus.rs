//! Follows and drives Windows focus sessions through the Clock app.
//!
//! Windows reserves its own session API for approved apps, but the Clock app
//! accepts `ms-clock:` links and answers a service, which is how its widget works.
//! The settings record tells when anything changed, so this thread sleeps until
//! then. See `island_core::focus` for how the sources are combined.

use std::mem::size_of;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use island_core::focus::{self, ClockFocus, FocusRecord, FocusStatus};
use windows::ApplicationModel::AppService::{AppServiceConnection, AppServiceConnectionStatus};
use windows::Foundation::Collections::ValueSet;
use windows::Foundation::{IPropertyValue, PropertyValue, TypedEventHandler};
use windows::UI::Shell::FocusSessionManager;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, WIN32_ERROR, WPARAM};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, KEY_READ, REG_NOTIFY_CHANGE_LAST_SET,
    REG_NOTIFY_CHANGE_NAME, RRF_RT_REG_BINARY, RegCloseKey, RegGetValueW, RegNotifyChangeKeyValue,
    RegOpenKeyExW,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForSingleObject};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, SW_HIDE, WM_APP};
use windows::core::{HSTRING, IInspectable, Interface, PCWSTR, Result, h, w};

use super::calendar::unix_now;

pub const MSG_FOCUS: u32 = WM_APP + 8;
/// The Clock app could not take a session. `wParam` holds its length in minutes.
pub const MSG_FOCUS_REFUSED: u32 = WM_APP + 9;

const STORE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\CloudStore\\Store\\DefaultAccount\\Current");
const RECORD_FOLDER: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\CloudStore\\Store\\DefaultAccount\\Current\\default$windows.data.shell.focusactivesessions");
const RECORD: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\CloudStore\\Store\\DefaultAccount\\Current\\default$windows.data.shell.focusactivesessions\\windows.data.shell.focusactivesessions");
const END_LINK: &str = "ms-clock://resetfocustimer";
const CLOCK_SERVICE: &str = "ms-alarmsService";
const CLOCK_PACKAGE: &str = "Microsoft.WindowsAlarms_8wekyb3d8bbwe";
/// Windows writes the record a moment before the session counts as active.
const SETTLE: Duration = Duration::from_millis(400);
/// A stuck Clock app must not stop sessions from being followed.
const CLOCK_TIMEOUT: Duration = Duration::from_secs(8);
/// How long a session may take to begin or end. The Clock app may have to launch.
const SESSION_CHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const SESSION_CHANGE_CHECK: Duration = Duration::from_millis(50);
/// What the Clock app needs between ending a session and taking the next.
const BETWEEN_SESSIONS: Duration = Duration::from_millis(300);

/// Asks Windows to run a focus session. `isAot=false` keeps Clock's floating
/// timer off the screen, since the island already shows the time. A machine
/// without the Clock app answers with `MSG_FOCUS_REFUSED`.
///
/// The Clock app starts a session with the length from the link before, and only
/// then stores the new one. Neither the order of the words nor its `endTime` word
/// changes that. So a first session stores the length and is ended at once, and
/// a second one runs with it, which takes about a second in all.
pub fn start_session(hwnd: HWND, minutes: u64) {
    let window = hwnd.0 as isize;
    let link = format!(
        "ms-clock://createfocustimer?skipBreaks=true&isAot=false&force=true&duration={minutes}"
    );
    std::thread::spawn(move || {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        if focus_is_active() {
            end_and_wait();
        }
        if !open_link(&link) {
            unsafe {
                let _ = PostMessageW(Some(HWND(window as _)), MSG_FOCUS_REFUSED, WPARAM(minutes as usize), LPARAM(0));
            }
            return;
        }
        if wait_for_focus(true) {
            end_and_wait();
            open_link(&link);
        }
    });
}

fn focus_is_active() -> bool {
    FocusSessionManager::GetDefault().and_then(|manager| manager.IsFocusActive()).unwrap_or(false)
}

/// Returns whether it got there in time.
fn wait_for_focus(active: bool) -> bool {
    let deadline = Instant::now() + SESSION_CHANGE_TIMEOUT;
    while focus_is_active() != active {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(SESSION_CHANGE_CHECK);
    }
    true
}

fn end_and_wait() {
    open_link(END_LINK);
    wait_for_focus(false);
    std::thread::sleep(BETWEEN_SESSIONS);
}

pub fn pause_session() {
    open_link_later("ms-clock://pausefocustimer");
}

pub fn resume_session() {
    open_link_later("ms-clock://resumefocustimer");
}

pub fn end_session() {
    open_link_later(END_LINK);
}

/// Handing a link to its app can take a while, which the UI must not wait for.
fn open_link_later(link: &'static str) {
    std::thread::spawn(move || {
        open_link(link);
    });
}

/// Returns whether Windows found an app for it.
fn open_link(link: &str) -> bool {
    let result = unsafe { ShellExecuteW(None, w!("open"), &HSTRING::from(link), None, None, SW_HIDE) };
    // Values up to 32 are error codes.
    result.0 as isize > 32
}

pub struct FocusWatch {
    updates: Receiver<FocusStatus>,
    changed: Option<HANDLE>,
}

impl FocusWatch {
    pub fn start(hwnd: HWND) -> Self {
        let (publish, updates) = mpsc::channel();
        let window = hwnd.0 as isize;
        let changed = unsafe { CreateEventW(None, false, false, None) }.ok();
        if let Some(event) = changed {
            let signal = event.0 as isize;
            let spawned = std::thread::Builder::new().name("focus".into()).spawn(move || {
                run(window, HANDLE(signal as _), publish);
            });
            if let Err(error) = spawned {
                eprintln!("focus thread failed to start: {error}");
            }
        }
        Self { updates, changed }
    }

    pub fn latest(&self) -> Option<FocusStatus> {
        self.updates.try_iter().last()
    }

    /// Asks for a fresh report even though nothing seems to have changed.
    pub fn recheck(&self) {
        if let Some(changed) = self.changed {
            unsafe {
                let _ = SetEvent(changed);
            }
        }
    }
}

fn open(path: PCWSTR) -> Option<HKEY> {
    let mut key = HKEY::default();
    let access = KEY_READ | KEY_NOTIFY;
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path, None, access, &mut key) };
    status.is_ok().then_some(key)
}

fn run(window: isize, changed: HANDLE, publish: Sender<FocusStatus>) {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }

    // The official event catches what the record misses, and the other way round.
    let manager = FocusSessionManager::GetDefault().ok();
    let signal = changed.0 as isize;
    if let Some(manager) = &manager {
        let _ = manager.IsFocusActiveChanged(&TypedEventHandler::<FocusSessionManager, IInspectable>::new(
            move |_, _| {
                unsafe {
                    let _ = SetEvent(HANDLE(signal as _));
                }
                Ok(())
            },
        ));
    }

    let mut folder = None;
    let mut first = true;
    loop {
        // The record only exists once a first session has run. Until then a new
        // key in the store is the sign to look again.
        folder = folder.or_else(|| open(RECORD_FOLDER));
        let store = if folder.is_none() { open(STORE) } else { None };
        let armed = match (folder, store) {
            // Windows replaces the record's key rather than writing into it, so
            // the folder above it is what stays valid to watch.
            (Some(folder), _) => {
                let filter = REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET;
                unsafe { RegNotifyChangeKeyValue(folder, true, filter, Some(changed), true) }
            }
            (None, Some(store)) => unsafe {
                RegNotifyChangeKeyValue(store, false, REG_NOTIFY_CHANGE_NAME, Some(changed), true)
            },
            (None, None) => return,
        };
        if armed != WIN32_ERROR(0) {
            return;
        }
        if !first {
            std::thread::sleep(SETTLE);
        }
        if publish.send(read_status(first)).is_err() {
            return;
        }
        first = false;
        unsafe {
            let _ = PostMessageW(Some(HWND(window as _)), MSG_FOCUS, WPARAM(0), LPARAM(0));
            WaitForSingleObject(changed, INFINITE);
            if let Some(store) = store {
                let _ = RegCloseKey(store);
            }
        }
    }
}

/// At startup the Clock is only asked if a session seems to be on, so opening
/// the island does not wake another app for nothing.
fn read_status(at_startup: bool) -> FocusStatus {
    let active = focus_is_active();
    let record = read_record();
    let now = unix_now();
    let clock = if at_startup && !active && record.is_none_or(|record| record.ends <= now) {
        None
    } else {
        ask_clock()
    };
    focus::combine(active, record, clock, now)
}

/// Gives up after a while, leaving the waiting call on its own thread.
fn ask_clock() -> Option<ClockFocus> {
    let (reply, answer) = mpsc::channel();
    let asking = std::thread::Builder::new().name("focus clock".into()).spawn(move || {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let _ = reply.send(clock_state().ok().flatten());
    });
    asking.ok()?;
    answer.recv_timeout(CLOCK_TIMEOUT).ok().flatten()
}

fn read_record() -> Option<FocusRecord> {
    let key = open(RECORD)?;
    let mut record = [0_u8; 1024];
    let mut size = size_of::<[u8; 1024]>() as u32;
    let status = unsafe {
        RegGetValueW(
            key,
            None,
            w!("Data"),
            RRF_RT_REG_BINARY,
            None,
            Some(record.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    unsafe {
        let _ = RegCloseKey(key);
    }
    if status != WIN32_ERROR(0) {
        return None;
    }
    focus::parse_record(record.get(..size as usize)?)
}

/// `Ok(None)` means the service answered without a usable reply. This can start
/// the Clock app, which takes seconds, so it must stay off the UI thread.
fn clock_state() -> Result<Option<ClockFocus>> {
    let connection = AppServiceConnection::new()?;
    connection.SetAppServiceName(&HSTRING::from(CLOCK_SERVICE))?;
    connection.SetPackageFamilyName(&HSTRING::from(CLOCK_PACKAGE))?;
    if connection.OpenAsync()?.join()? != AppServiceConnectionStatus::Success {
        return Err(windows::core::Error::empty());
    }

    let message = ValueSet::new()?;
    let command = PropertyValue::CreateString(h!("GetFocusSessionState"))?;
    message.Insert(h!("Command"), &command)?;
    let reply = connection.SendMessageAsync(&message)?.join()?.Message()?;
    let json = reply.Lookup(h!("FocusSessionJson"))?.cast::<IPropertyValue>()?.GetString()?;
    Ok(focus::parse_clock_state(&json.to_string()))
}
